//! "Owned" sessions: Claude Code processes that agentwatch itself launches
//! (`claude -p --input-format stream-json --output-format stream-json`) and
//! talks to over stdin/stdout, instead of only watching them through hooks.
//!
//! Life of one owned session:
//! 1. `launch` starts the process and files it under a temporary id
//!    (`pending-...`). The first user message is written to its stdin.
//! 2. A background task reads the process's stdout, one JSON object per line,
//!    and turns each into `TranscriptItem`s.
//! 3. The first `system/init` line carries the real `session_id`; the entry
//!    and the widget card are re-keyed from the temporary id to the real one
//!    (the same id the hooks use, so hook events and process data share one
//!    card).
//! 4. `send_message` writes more user messages; `end` closes stdin (the CLI
//!    exits when its input ends) and kills it after a grace period.
//!
//! 5. `interrupt` stops the current turn with a stream-json control request
//!    (shape verified live, see `Owned::interrupt`); the session stays open.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Notify};

use crate::config::Config;
use crate::server::{OnChange, SharedStore};
use crate::sessions::{Millis, SessionState};

/// Tool output / input shown in the transcript is cut to this many bytes.
const PREVIEW_BYTES: usize = 2048;
/// How much of the process's stderr we remember.
const STDERR_KEEP: usize = 4096;
/// After `end` closes stdin, the process gets this long to exit by itself.
const END_GRACE: Duration = Duration::from_secs(5);
/// Windows flag: do not pop up a console window for the child process.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// One line of the transcript window. `tag = "kind"` makes the JSON look like
/// `{"kind":"assistant_text","text":"..."}`; the TypeScript union in
/// `src/types.ts` mirrors it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranscriptItem {
    User {
        text: String,
    },
    AssistantText {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        summary: String,
        input_json: String,
    },
    ToolResult {
        tool_use_id: String,
        is_error: bool,
        output_preview: String,
    },
    TurnResult {
        subtype: String,
        /// The CLI reports the running total for the whole session.
        cost_usd: f64,
        duration_ms: u64,
    },
    Notice {
        text: String,
    },
}

/// Payload of the `transcript` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TranscriptEvent {
    pub session_id: String,
    pub from_index: usize,
    pub items: Vec<TranscriptItem>,
}

/// Payload of the `session-rekeyed` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rekeyed {
    pub from: String,
    pub to: String,
}

/// Things the UI wants to hear about. `lib.rs` turns these into Tauri events.
#[derive(Debug, Clone, PartialEq)]
pub enum OwnedEvent {
    Transcript(TranscriptEvent),
    Rekeyed(Rekeyed),
}

pub type OnEvent = Arc<dyn Fn(OwnedEvent) + Send + Sync>;

// ---------------------------------------------------------------------
// Parsing the CLI's stdout
// ---------------------------------------------------------------------

/// Cut `s` to at most `max` bytes (never in the middle of a character) and
/// add "…" when something was removed.
pub fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    // A `str` must stay valid UTF-8, so step back to a character boundary.
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// The last `max` bytes of `s`, starting on a character boundary.
fn tail_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// One-line description of a tool call, e.g. `Bash: git status` or
/// `Edit App.tsx`.
pub fn summarize_tool(name: &str, input: &Value) -> String {
    let field = |k: &str| input.get(k).and_then(Value::as_str);
    match name {
        "Bash" | "PowerShell" => match field("command") {
            Some(cmd) => {
                let first: String = cmd.lines().next().unwrap_or("").chars().take(80).collect();
                format!("{name}: {first}")
            }
            None => name.to_string(),
        },
        "Edit" | "Write" | "Read" | "MultiEdit" => match field("file_path") {
            Some(p) => format!("{name} {}", basename(p)),
            None => name.to_string(),
        },
        "Grep" | "Glob" => match field("pattern") {
            Some(p) => format!("{name} {p}"),
            None => name.to_string(),
        },
        _ => name.to_string(),
    }
}

/// What one stdout line means.
#[derive(Debug, PartialEq)]
pub enum Parsed {
    /// `system/init`: carries the real session id.
    Init {
        session_id: String,
    },
    Items(Vec<TranscriptItem>),
    Ignore,
}

/// A tool result's `content` is either a string or a list of `{type:"text"}`
/// blocks.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Turn one parsed stdout line into transcript items.
///
/// Messages from subagents (non-null `parent_tool_use_id`) are skipped: they
/// would interleave with the main conversation and confuse the tool-call /
/// tool-result pairing. Unknown event types are ignored.
pub fn parse_line(v: &Value) -> Parsed {
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    if v.get("parent_tool_use_id").is_some_and(|p| !p.is_null()) {
        return Parsed::Ignore;
    }
    match kind {
        "system" => match v.get("subtype").and_then(Value::as_str) {
            Some("init") => match v.get("session_id").and_then(Value::as_str) {
                Some(id) if !id.is_empty() => Parsed::Init {
                    session_id: id.to_string(),
                },
                _ => Parsed::Ignore,
            },
            Some("api_retry") => {
                let attempt = v.get("attempt").and_then(Value::as_u64).unwrap_or(0);
                let max = v.get("max_retries").and_then(Value::as_u64).unwrap_or(0);
                let error = v.get("error").and_then(Value::as_str).unwrap_or("error");
                Parsed::Items(vec![TranscriptItem::Notice {
                    text: format!("API retry {attempt}/{max} ({error})"),
                }])
            }
            _ => Parsed::Ignore,
        },
        "assistant" => {
            let mut items = Vec::new();
            let blocks = v.pointer("/message/content").and_then(Value::as_array);
            for block in blocks.into_iter().flatten() {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                        if !text.trim().is_empty() {
                            items.push(TranscriptItem::AssistantText {
                                text: text.to_string(),
                            });
                        }
                    }
                    Some("tool_use") => {
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("Tool");
                        let input = block.get("input").cloned().unwrap_or(Value::Null);
                        items.push(TranscriptItem::ToolUse {
                            id: block
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            name: name.to_string(),
                            summary: summarize_tool(name, &input),
                            input_json: truncate_bytes(&input.to_string(), PREVIEW_BYTES),
                        });
                    }
                    _ => {} // thinking blocks etc.
                }
            }
            nonempty(items)
        }
        "user" => {
            // Only tool results matter; plain user text is echoed by us.
            let mut items = Vec::new();
            let blocks = v.pointer("/message/content").and_then(Value::as_array);
            for block in blocks.into_iter().flatten() {
                if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                    let output = content_text(block.get("content").unwrap_or(&Value::Null));
                    items.push(TranscriptItem::ToolResult {
                        tool_use_id: block
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        is_error: block
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        output_preview: truncate_bytes(&output, PREVIEW_BYTES),
                    });
                }
            }
            nonempty(items)
        }
        "result" => {
            let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or("");
            let mut items = vec![TranscriptItem::TurnResult {
                subtype: subtype.to_string(),
                cost_usd: v
                    .get("total_cost_usd")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
                duration_ms: v.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
            }];
            // A failed turn: also show what the CLI said went wrong.
            if v.get("is_error").and_then(Value::as_bool) == Some(true) {
                if let Some(text) = v.get("result").and_then(Value::as_str) {
                    items.push(TranscriptItem::Notice {
                        text: truncate_bytes(text, PREVIEW_BYTES),
                    });
                }
            }
            Parsed::Items(items)
        }
        _ => Parsed::Ignore,
    }
}

fn nonempty(items: Vec<TranscriptItem>) -> Parsed {
    if items.is_empty() {
        Parsed::Ignore
    } else {
        Parsed::Items(items)
    }
}

// ---------------------------------------------------------------------
// Finding the claude executable
// ---------------------------------------------------------------------

/// First file called one of `names` in a directory of `path_var` (a PATH-style
/// list of folders).
pub fn find_in_path_var(names: &[&str], path_var: &OsStr) -> Option<PathBuf> {
    // `split_paths` handles `;` on Windows and `:` elsewhere.
    for dir in std::env::split_paths(path_var) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Which program (plus leading arguments) to run for "claude":
/// 1. `claude_command` in config.json (a whole command; used by tests),
/// 2. `claude_path` in config.json,
/// 3. `claude` on PATH,
/// 4. `~/.local/bin/claude`.
pub fn resolve_command(config: &Config) -> Result<(String, Vec<String>), String> {
    if let Some(cmd) = &config.claude_command {
        if let Some((program, rest)) = cmd.split_first() {
            return Ok((program.clone(), rest.to_vec()));
        }
    }
    if let Some(path) = config.claude_path.as_deref().filter(|p| !p.is_empty()) {
        return Ok((path.to_string(), Vec::new()));
    }
    let names: &[&str] = if cfg!(windows) {
        &["claude.exe", "claude.cmd"]
    } else {
        &["claude"]
    };
    if let Some(found) = std::env::var_os("PATH").and_then(|p| find_in_path_var(names, &p)) {
        return Ok((found.to_string_lossy().into_owned(), Vec::new()));
    }
    if let Some(home) = dirs::home_dir() {
        let exe = if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        };
        let local = home.join(".local").join("bin").join(exe);
        if local.is_file() {
            return Ok((local.to_string_lossy().into_owned(), Vec::new()));
        }
    }
    Err("could not find the claude executable; set claude_path in ~/.agentwatch/config.json".into())
}

// ---------------------------------------------------------------------
// The registry of running / finished owned sessions
// ---------------------------------------------------------------------

/// Messages for the task that owns the process's stdin.
enum Cmd {
    Line(String),
    Close,
}

struct Entry {
    transcript: Vec<TranscriptItem>,
    /// Channel into the stdin writer task. `None` once the process is gone.
    tx: Option<mpsc::UnboundedSender<Cmd>>,
    /// `notify_one()` tells the supervisor task to kill the process.
    kill: Arc<Notify>,
    pid: Option<u32>,
    running: bool,
}

#[derive(Default)]
struct Registry {
    entries: HashMap<String, Entry>,
    /// Old (temporary) id -> current id, so commands that still use the
    /// temporary id keep working after the re-key.
    aliases: HashMap<String, String>,
}

impl Registry {
    fn resolve(&self, id: &str) -> String {
        self.aliases
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

fn temp_id(now: Millis) -> String {
    format!(
        "pending-{now:x}-{}",
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    )
}

/// Best-effort synchronous kill, used when the app is exiting and the async
/// runtime may no longer run our tasks. `/T` also kills child processes.
fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Handle to all owned sessions. Cloning is cheap (everything inside is an
/// `Arc`), so each background task gets its own clone.
#[derive(Clone)]
pub struct Owned {
    registry: Arc<Mutex<Registry>>,
    store: SharedStore,
    on_change: OnChange,
    on_event: OnEvent,
    config: Arc<Config>,
    now: fn() -> Millis,
}

impl Owned {
    pub fn new(
        config: Config,
        store: SharedStore,
        on_change: OnChange,
        on_event: OnEvent,
        now: fn() -> Millis,
    ) -> Owned {
        Owned {
            registry: Arc::new(Mutex::new(Registry::default())),
            store,
            on_change,
            on_event,
            config: Arc::new(config),
            now,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Registry> {
        // A poisoned lock still holds valid data; recover it.
        self.registry.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Send the current store snapshot to the UI (call without holding locks).
    fn emit_store(&self) {
        let snapshot = self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot();
        (self.on_change)(snapshot);
    }

    /// Run `f` on the store; emit a snapshot if it reports a change.
    fn update_store(&self, f: impl FnOnce(&mut crate::sessions::SessionStore) -> bool) {
        let changed = f(&mut self.store.lock().unwrap_or_else(|e| e.into_inner()));
        if changed {
            self.emit_store();
        }
    }

    /// Append items to a session's transcript and tell the UI.
    fn push_items(&self, id: &str, items: Vec<TranscriptItem>) {
        let event = {
            let mut reg = self.lock();
            let real = reg.resolve(id);
            let Some(entry) = reg.entries.get_mut(&real) else {
                return;
            };
            let from_index = entry.transcript.len();
            entry.transcript.extend(items.iter().cloned());
            TranscriptEvent {
                session_id: real,
                from_index,
                items,
            }
        };
        (self.on_event)(OwnedEvent::Transcript(event));
    }

    /// Start `claude` in `cwd` with `prompt` as the first message. Returns the
    /// temporary session id (the real one arrives later via `session-rekeyed`).
    ///
    /// `async` because spawning a process with `tokio::process` must happen
    /// inside the tokio runtime; it also keeps the later `tokio::spawn` calls
    /// valid.
    pub async fn launch(&self, cwd: &str, prompt: &str) -> Result<String, String> {
        if prompt.trim().is_empty() {
            return Err("prompt is empty".to_string());
        }
        if !Path::new(cwd).is_dir() {
            return Err(format!("not a folder: {cwd}"));
        }
        let (program, leading_args) = resolve_command(&self.config)?;

        let mut command = Command::new(program);
        command
            .args(leading_args)
            .args([
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "default",
            ])
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // If our `Child` handle is dropped, the process is killed.
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);

        let mut child = command
            .spawn()
            .map_err(|e| format!("could not start claude: {e}"))?;
        // `take()` moves the pipe out of the `Option` inside `child`.
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err("could not open the process pipes".to_string());
        };

        let now = (self.now)();
        let id = temp_id(now);
        let (tx, rx) = mpsc::unbounded_channel::<Cmd>();
        let kill = Arc::new(Notify::new());
        self.lock().entries.insert(
            id.clone(),
            Entry {
                transcript: Vec::new(),
                tx: Some(tx.clone()),
                kill: kill.clone(),
                pid: child.id(),
                running: true,
            },
        );
        self.update_store(|s| {
            s.add_owned(&id, cwd, prompt, now);
            true
        });
        self.push_items(
            &id,
            vec![TranscriptItem::User {
                text: prompt.to_string(),
            }],
        );
        let _ = tx.send(Cmd::Line(user_message_line(prompt)));

        tokio::spawn(write_stdin(stdin, rx));
        tokio::spawn(
            self.clone()
                .supervise(id.clone(), child, stdout, stderr, kill),
        );
        Ok(id)
    }

    /// Read the process's output until it exits, then clean up.
    async fn supervise(
        self,
        mut id: String,
        mut child: Child,
        stdout: tokio::process::ChildStdout,
        mut stderr: tokio::process::ChildStderr,
        kill: Arc<Notify>,
    ) {
        // Keep only the last few KB of stderr, for the crash notice.
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let tail_writer = stderr_tail.clone();
        let stderr_task = tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            loop {
                match stderr.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut tail = tail_writer.lock().unwrap_or_else(|e| e.into_inner());
                        tail.push_str(&String::from_utf8_lossy(&buf[..n]));
                        let keep = tail_bytes(&tail, STDERR_KEEP).to_string();
                        *tail = keep;
                    }
                }
            }
        });

        // `lines()` yields one line of stdout at a time, waiting (without
        // blocking a thread) until the next one is available.
        let mut lines = BufReader::new(stdout).lines();
        loop {
            // `select!` waits for whichever of the two futures finishes first.
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => self.handle_line(&mut id, &line),
                    // `Ok(None)` = end of output; `Err` = unreadable. Either way, done.
                    _ => break,
                },
                _ = kill.notified() => {
                    let _ = child.start_kill();
                    break;
                }
            }
        }

        let status = child.wait().await;
        // Give the stderr reader a moment to deliver the last bytes.
        let _ = tokio::time::timeout(Duration::from_millis(500), stderr_task).await;
        let code = status.ok().and_then(|s| s.code());
        if code != Some(0) {
            let mut text = match code {
                Some(c) => format!("agent exited (code {c})"),
                None => "agent exited (code unknown)".to_string(),
            };
            let tail = stderr_tail.lock().unwrap_or_else(|e| e.into_inner());
            if !tail.trim().is_empty() {
                text.push('\n');
                text.push_str(tail.trim());
            }
            self.push_items(&id, vec![TranscriptItem::Notice { text }]);
        }
        {
            let mut reg = self.lock();
            let real = reg.resolve(&id);
            if let Some(entry) = reg.entries.get_mut(&real) {
                entry.running = false;
                entry.tx = None;
            }
        }
        let now = (self.now)();
        self.update_store(|s| s.set_owned_state(&id, SessionState::Ended, None, now));
    }

    fn handle_line(&self, id: &mut String, line: &str) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return; // not JSON: ignore
        };
        match parse_line(&value) {
            Parsed::Init { session_id } => self.rekey(id, &session_id),
            Parsed::Items(items) => {
                let turn_done = items
                    .iter()
                    .any(|i| matches!(i, TranscriptItem::TurnResult { .. }));
                self.push_items(id, items);
                if turn_done {
                    let now = (self.now)();
                    self.update_store(|s| {
                        s.set_owned_state(
                            id,
                            SessionState::WaitingForReply,
                            Some(SessionState::Working),
                            now,
                        )
                    });
                }
            }
            Parsed::Ignore => {}
        }
    }

    /// The real session id arrived: move the registry entry and the widget
    /// card from the temporary id to it.
    fn rekey(&self, id: &mut String, real: &str) {
        if id == real {
            return;
        }
        let old = id.clone();
        {
            let mut reg = self.lock();
            // Re-keying a `HashMap` entry = remove under the old key and
            // insert under the new one (keys cannot be changed in place).
            if let Some(entry) = reg.entries.remove(&old) {
                reg.entries.insert(real.to_string(), entry);
            }
            for target in reg.aliases.values_mut() {
                if *target == old {
                    *target = real.to_string();
                }
            }
            reg.aliases.insert(old.clone(), real.to_string());
        }
        let now = (self.now)();
        self.update_store(|s| {
            s.rekey(&old, real);
            s.mark_owned(real, "", now);
            true
        });
        *id = real.to_string();
        (self.on_event)(OwnedEvent::Rekeyed(Rekeyed {
            from: old,
            to: real.to_string(),
        }));
    }

    /// Send another user message to a running session.
    pub fn send_message(&self, id: &str, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Err("message is empty".to_string());
        }
        let (real, tx) = {
            let reg = self.lock();
            let real = reg.resolve(id);
            let entry = reg.entries.get(&real).ok_or("unknown session")?;
            let tx = entry
                .tx
                .clone()
                .filter(|_| entry.running)
                .ok_or("agent is not running")?;
            (real, tx)
        };
        // Record the message first so it appears before the reply.
        self.push_items(
            &real,
            vec![TranscriptItem::User {
                text: text.to_string(),
            }],
        );
        tx.send(Cmd::Line(user_message_line(text)))
            .map_err(|_| "agent is not running".to_string())?;
        let now = (self.now)();
        self.update_store(|s| s.set_owned_state(&real, SessionState::Working, None, now));
        Ok(())
    }

    /// Stop the current turn without ending the session. Sends the stream-json
    /// control request `{"type":"control_request","request_id":…,
    /// "request":{"subtype":"interrupt"}}`. Verified live against Claude Code
    /// 2.1.325 (prototype/spike, 2026-11-14): the CLI answers with a
    /// `control_response` (subtype "success"), ends the turn with a `result`
    /// of subtype "error_during_execution", and keeps accepting messages.
    pub fn interrupt(&self, id: &str) -> Result<(), String> {
        let tx = {
            let reg = self.lock();
            let real = reg.resolve(id);
            let entry = reg.entries.get(&real).ok_or("unknown session")?;
            entry
                .tx
                .clone()
                .filter(|_| entry.running)
                .ok_or("agent is not running")?
        };
        tx.send(Cmd::Line(interrupt_line(&next_request_id())))
            .map_err(|_| "agent is not running".to_string())
    }

    /// Close the process's stdin (the CLI then finishes and exits), and kill
    /// it if it is still alive after the grace period.
    pub fn end(&self, id: &str) -> Result<(), String> {
        let kill = {
            let mut reg = self.lock();
            let real = reg.resolve(id);
            let entry = reg.entries.get_mut(&real).ok_or("unknown session")?;
            if !entry.running {
                return Ok(());
            }
            if let Some(tx) = entry.tx.take() {
                let _ = tx.send(Cmd::Close);
            }
            entry.kill.clone()
        };
        tokio::spawn(async move {
            tokio::time::sleep(END_GRACE).await;
            // If the process already exited, nobody is listening: harmless.
            kill.notify_one();
        });
        Ok(())
    }

    pub fn get_transcript(&self, id: &str) -> Result<Vec<TranscriptItem>, String> {
        let reg = self.lock();
        let real = reg.resolve(id);
        reg.entries
            .get(&real)
            .map(|e| e.transcript.clone())
            .ok_or_else(|| "unknown session".to_string())
    }

    /// How many owned processes are alive (for the quit confirmation).
    pub fn running_count(&self) -> usize {
        self.lock().entries.values().filter(|e| e.running).count()
    }

    /// Kill every owned process now. Called when the app quits.
    pub fn kill_all(&self) {
        let reg = self.lock();
        for entry in reg.entries.values().filter(|e| e.running) {
            entry.kill.notify_one();
            if let Some(pid) = entry.pid {
                kill_pid(pid);
            }
        }
    }
}

/// The JSON line the CLI expects for a user message.
fn user_message_line(text: &str) -> String {
    json!({"type": "user", "message": {"role": "user", "content": text}}).to_string()
}

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

fn next_request_id() -> String {
    format!(
        "agentwatch-{}",
        NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
    )
}

fn interrupt_line(request_id: &str) -> String {
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {"subtype": "interrupt"}
    })
    .to_string()
}

/// Owns the process's stdin: writes each queued line, and closes the pipe
/// (by dropping it) on `Close` or when every sender is gone.
async fn write_stdin(mut stdin: tokio::process::ChildStdin, mut rx: mpsc::UnboundedReceiver<Cmd>) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Cmd::Line(mut line) => {
                line.push('\n');
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                    break; // process is gone
                }
            }
            Cmd::Close => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::SessionStore;

    // ---- pure helpers -------------------------------------------------

    #[test]
    fn interrupt_line_matches_verified_shape() {
        let v: Value = serde_json::from_str(&interrupt_line("agentwatch-7")).unwrap();
        assert_eq!(
            v,
            json!({"type": "control_request", "request_id": "agentwatch-7",
                   "request": {"subtype": "interrupt"}})
        );
        assert_ne!(next_request_id(), next_request_id());
    }

    #[test]
    fn summary_table() {
        let cases: Vec<(&str, Value, &str)> = vec![
            ("Bash", json!({"command": "git status"}), "Bash: git status"),
            (
                "Bash",
                json!({"command": "echo one\necho two"}),
                "Bash: echo one",
            ),
            (
                "PowerShell",
                json!({"command": "Get-ChildItem"}),
                "PowerShell: Get-ChildItem",
            ),
            (
                "Edit",
                json!({"file_path": "C:\\proj\\src\\App.tsx"}),
                "Edit App.tsx",
            ),
            ("Write", json!({"file_path": "/a/b/c.txt"}), "Write c.txt"),
            ("Read", json!({"file_path": "x.md"}), "Read x.md"),
            (
                "MultiEdit",
                json!({"file_path": "/q/z.rs"}),
                "MultiEdit z.rs",
            ),
            ("Grep", json!({"pattern": "TODO"}), "Grep TODO"),
            ("Glob", json!({"pattern": "**/*.ts"}), "Glob **/*.ts"),
            ("WebFetch", json!({"url": "x"}), "WebFetch"),
            ("Bash", json!({}), "Bash"),
            ("Edit", Value::Null, "Edit"),
        ];
        for (name, input, want) in cases {
            assert_eq!(summarize_tool(name, &input), want);
        }
        let long = "x".repeat(200);
        let s = summarize_tool("Bash", &json!({ "command": long }));
        assert_eq!(s.chars().count(), "Bash: ".len() + 80);
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        assert_eq!(truncate_bytes("short", 10), "short");
        assert_eq!(truncate_bytes("abcdef", 3), "abc…");
        // "é" is two bytes; cutting at 1 must not split it.
        assert_eq!(truncate_bytes("éé", 3), "é…");
        assert_eq!(tail_bytes("abcdef", 3), "def");
        assert_eq!(tail_bytes("éé", 3), "é");
    }

    fn items(v: Value) -> Vec<TranscriptItem> {
        match parse_line(&v) {
            Parsed::Items(i) => i,
            other => panic!("expected items, got {other:?}"),
        }
    }

    #[test]
    fn parses_real_shaped_lines() {
        assert_eq!(
            parse_line(&json!({"type":"system","subtype":"init","session_id":"abc","tools":[]})),
            Parsed::Init {
                session_id: "abc".into()
            }
        );
        // Other system events and unknown types are ignored.
        assert_eq!(
            parse_line(&json!({"type":"system","subtype":"hook_started"})),
            Parsed::Ignore
        );
        assert_eq!(
            parse_line(&json!({"type":"rate_limit_event"})),
            Parsed::Ignore
        );

        let a = items(
            json!({"type":"assistant","parent_tool_use_id":null,"message":{"role":"assistant","content":[
                {"type":"thinking","thinking":"hmm"},
                {"type":"text","text":"Hello"},
                {"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/x/a.ts","old_string":"a"}}
            ]}}),
        );
        assert_eq!(a.len(), 2);
        assert_eq!(
            a[0],
            TranscriptItem::AssistantText {
                text: "Hello".into()
            }
        );
        match &a[1] {
            TranscriptItem::ToolUse {
                id,
                name,
                summary,
                input_json,
            } => {
                assert_eq!((id.as_str(), name.as_str()), ("t1", "Edit"));
                assert_eq!(summary, "Edit a.ts");
                assert!(input_json.contains("old_string"));
            }
            other => panic!("{other:?}"),
        }

        let r = items(json!({"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"line1"},{"type":"text","text":"line2"}],"is_error":true}
        ]}}));
        assert_eq!(
            r,
            vec![TranscriptItem::ToolResult {
                tool_use_id: "t1".into(),
                is_error: true,
                output_preview: "line1\nline2".into()
            }]
        );
        // Plain user text is not a transcript item (we echo it ourselves).
        assert_eq!(
            parse_line(&json!({"type":"user","message":{"role":"user","content":"hi"}})),
            Parsed::Ignore
        );

        let t = items(json!({"type":"result","subtype":"success","is_error":false,
            "result":"done","total_cost_usd":0.5,"duration_ms":1234,"session_id":"abc"}));
        assert_eq!(
            t,
            vec![TranscriptItem::TurnResult {
                subtype: "success".into(),
                cost_usd: 0.5,
                duration_ms: 1234
            }]
        );
        let failed = items(json!({"type":"result","subtype":"error_during_execution",
            "is_error":true,"result":"boom"}));
        assert_eq!(failed.len(), 2);
        assert_eq!(
            failed[1],
            TranscriptItem::Notice {
                text: "boom".into()
            }
        );

        let retry = items(json!({"type":"system","subtype":"api_retry","attempt":1,
            "max_retries":5,"error":"rate_limit"}));
        assert_eq!(
            retry,
            vec![TranscriptItem::Notice {
                text: "API retry 1/5 (rate_limit)".into()
            }]
        );
    }

    #[test]
    fn subagent_messages_and_big_output_are_handled() {
        assert_eq!(
            parse_line(&json!({"type":"assistant","parent_tool_use_id":"toolu_1",
                "message":{"content":[{"type":"text","text":"inner"}]}})),
            Parsed::Ignore
        );
        let big = "y".repeat(5000);
        let r = items(json!({"type":"user","message":{"content":[
            {"type":"tool_result","tool_use_id":"t","content": big}]}}));
        match &r[0] {
            TranscriptItem::ToolResult { output_preview, .. } => {
                assert!(output_preview.len() <= PREVIEW_BYTES + 3);
                assert!(output_preview.ends_with('…'));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn transcript_item_json_shape() {
        let v = serde_json::to_value(TranscriptItem::AssistantText { text: "x".into() }).unwrap();
        assert_eq!(v, json!({"kind":"assistant_text","text":"x"}));
        let v = serde_json::to_value(TranscriptItem::TurnResult {
            subtype: "success".into(),
            cost_usd: 0.1,
            duration_ms: 5,
        })
        .unwrap();
        assert_eq!(
            v,
            json!({"kind":"turn_result","subtype":"success","cost_usd":0.1,"duration_ms":5})
        );
    }

    #[test]
    fn resolve_command_order() {
        // 1. explicit command wins and splits into program + args.
        let c = Config {
            claude_command: Some(vec!["bun".into(), "fake.ts".into()]),
            claude_path: Some("/ignored".into()),
            ..Config::default()
        };
        assert_eq!(
            resolve_command(&c).unwrap(),
            ("bun".to_string(), vec!["fake.ts".to_string()])
        );
        // 2. claude_path.
        let c = Config {
            claude_path: Some("/opt/claude".into()),
            ..Config::default()
        };
        assert_eq!(
            resolve_command(&c).unwrap(),
            ("/opt/claude".to_string(), vec![])
        );
        // An empty command list falls through to the next rule.
        let c = Config {
            claude_command: Some(vec![]),
            claude_path: Some("/opt/claude".into()),
            ..Config::default()
        };
        assert_eq!(resolve_command(&c).unwrap().0, "/opt/claude");
    }

    #[test]
    fn path_lookup_finds_files_in_listed_folders() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(b.path().join("claude"), "").unwrap();
        let var = std::env::join_paths([a.path(), b.path()]).unwrap();
        assert_eq!(
            find_in_path_var(&["claude", "claude.exe"], &var),
            Some(b.path().join("claude"))
        );
        assert_eq!(find_in_path_var(&["nope"], &var), None);
    }

    // ---- integration tests with the fake claude ----------------------

    /// Find `bun` on PATH (or in its default install folder).
    fn find_bun() -> Option<PathBuf> {
        let names = ["bun", "bun.exe"];
        if let Some(p) = std::env::var_os("PATH").and_then(|p| find_in_path_var(&names, &p)) {
            return Some(p);
        }
        let dir = dirs::home_dir()?.join(".bun").join("bin");
        find_in_path_var(&names, dir.as_os_str())
    }

    struct Harness {
        owned: Owned,
        store: SharedStore,
        events: Arc<Mutex<Vec<OwnedEvent>>>,
        dir: tempfile::TempDir,
    }

    /// `None` (test skipped) when bun is not installed.
    fn harness() -> Option<Harness> {
        let Some(bun) = find_bun() else {
            eprintln!("bun not found; skipping fake-claude test");
            return None;
        };
        let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-claude.ts");
        let config = Config {
            claude_command: Some(vec![
                bun.to_string_lossy().into_owned(),
                fake.to_string_lossy().into_owned(),
            ]),
            ..Config::default()
        };
        let store: SharedStore = Arc::new(Mutex::new(SessionStore::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let owned = Owned::new(
            config,
            store.clone(),
            Arc::new(|_| {}),
            Arc::new(move |e| sink.lock().unwrap().push(e)),
            crate::server::now_millis,
        );
        Some(Harness {
            owned,
            store,
            events,
            dir: tempfile::tempdir().unwrap(),
        })
    }

    async fn wait_for(what: &str, cond: impl Fn() -> bool) {
        for _ in 0..400 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("timed out waiting for {what}");
    }

    fn turn_results(o: &Owned, id: &str) -> usize {
        o.get_transcript(id)
            .map(|t| {
                t.iter()
                    .filter(|i| matches!(i, TranscriptItem::TurnResult { .. }))
                    .count()
            })
            .unwrap_or(0)
    }

    fn state_of(h: &Harness, id: &str) -> Option<SessionState> {
        h.store.lock().unwrap().get(id).map(|s| s.state)
    }

    #[tokio::test]
    async fn launch_rekey_two_turns_end() {
        let Some(h) = harness() else { return };
        let cwd = h.dir.path().to_string_lossy().into_owned();
        let temp = h.owned.launch(&cwd, "hello there").await.unwrap();
        assert!(temp.starts_with("pending-"));
        assert_eq!(h.owned.running_count(), 1);

        let o = h.owned.clone();
        let t = temp.clone();
        wait_for("first turn", move || turn_results(&o, &t) == 1).await;

        // Re-keyed: the card moved to the real id and is flagged owned.
        let real = {
            let events = h.events.lock().unwrap();
            events
                .iter()
                .find_map(|e| match e {
                    OwnedEvent::Rekeyed(r) => Some(r.clone()),
                    _ => None,
                })
                .expect("rekey event")
        };
        assert_eq!(real.from, temp);
        assert!(real.to.starts_with("fake-"));
        {
            let store = h.store.lock().unwrap();
            assert!(store.get(&temp).is_none());
            let card = store.get(&real.to).unwrap();
            assert!(card.owned);
            assert_eq!(card.state, SessionState::WaitingForReply);
            assert_eq!(store.snapshot().len(), 1);
        }
        // The old id still works as an alias.
        assert_eq!(
            h.owned.get_transcript(&temp).unwrap(),
            h.owned.get_transcript(&real.to).unwrap()
        );

        // Second turn, with a tool call.
        h.owned.send_message(&real.to, "please run a TOOL").unwrap();
        assert_eq!(state_of(&h, &real.to), Some(SessionState::Working));
        let o = h.owned.clone();
        let id = real.to.clone();
        wait_for("second turn", move || turn_results(&o, &id) == 2).await;

        let t = h.owned.get_transcript(&real.to).unwrap();
        let kinds: Vec<&str> = t
            .iter()
            .map(|i| match i {
                TranscriptItem::User { .. } => "user",
                TranscriptItem::AssistantText { .. } => "text",
                TranscriptItem::ToolUse { .. } => "tool_use",
                TranscriptItem::ToolResult { .. } => "tool_result",
                TranscriptItem::TurnResult { .. } => "turn",
                TranscriptItem::Notice { .. } => "notice",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "user",
                "text",
                "turn",
                "user",
                "text",
                "tool_use",
                "tool_result",
                "text",
                "turn"
            ]
        );
        match &t[5] {
            TranscriptItem::ToolUse { summary, .. } => assert_eq!(summary, "Bash: git status"),
            other => panic!("{other:?}"),
        }
        match &t[6] {
            TranscriptItem::ToolResult {
                output_preview,
                is_error,
                ..
            } => {
                assert!(output_preview.contains("nothing to commit"));
                assert!(!is_error);
            }
            other => panic!("{other:?}"),
        }
        match &t[8] {
            TranscriptItem::TurnResult { cost_usd, .. } => assert!((cost_usd - 0.02).abs() < 1e-9),
            other => panic!("{other:?}"),
        }

        // Transcript events are contiguous and cover the whole transcript.
        let mut next = 0;
        for e in h.events.lock().unwrap().iter() {
            if let OwnedEvent::Transcript(ev) = e {
                assert_eq!(ev.from_index, next);
                next += ev.items.len();
            }
        }
        assert_eq!(next, t.len());

        // End: stdin closes, the fake exits 0, the card is Ended.
        h.owned.end(&real.to).unwrap();
        let o = h.owned.clone();
        wait_for("exit", move || o.running_count() == 0).await;
        let (store, id) = (h.store.clone(), real.to.clone());
        wait_for("ended card", move || {
            store.lock().unwrap().get(&id).map(|s| s.state) == Some(SessionState::Ended)
        })
        .await;
        // A clean exit adds no crash notice.
        assert!(!h
            .owned
            .get_transcript(&real.to)
            .unwrap()
            .iter()
            .any(|i| matches!(i, TranscriptItem::Notice { .. })));
        // Messages to an ended session are refused.
        assert!(h.owned.send_message(&real.to, "late").is_err());
    }

    #[tokio::test]
    async fn crash_adds_notice_with_stderr_and_ends() {
        let Some(h) = harness() else { return };
        let cwd = h.dir.path().to_string_lossy().into_owned();
        let id = h.owned.launch(&cwd, "CRASH now").await.unwrap();
        let o = h.owned.clone();
        wait_for("exit", move || o.running_count() == 0).await;
        let t = h.owned.get_transcript(&id).unwrap();
        let notice = t
            .iter()
            .find_map(|i| match i {
                TranscriptItem::Notice { text } => Some(text.clone()),
                _ => None,
            })
            .expect("notice");
        assert!(notice.contains("agent exited (code 3)"), "{notice}");
        assert!(notice.contains("simulated crash"), "{notice}");
        assert_eq!(state_of(&h, &id), Some(SessionState::Ended));
    }

    #[tokio::test]
    async fn rejects_bad_input_and_skips_subagent_text() {
        let Some(h) = harness() else { return };
        let cwd = h.dir.path().to_string_lossy().into_owned();
        assert_eq!(
            h.owned.launch(&cwd, "   ").await.unwrap_err(),
            "prompt is empty"
        );
        let missing = h.dir.path().join("nope").to_string_lossy().into_owned();
        assert!(h.owned.launch(&missing, "hi").await.is_err());

        let id = h.owned.launch(&cwd, "SUBAGENT SLOW").await.unwrap();
        assert!(h.owned.send_message(&id, "  \n").is_err());
        assert!(h.owned.send_message("no-such-id", "hi").is_err());
        // Interrupt is accepted for a running agent (the fake ignores it)
        // and rejected for an unknown one.
        assert!(h.owned.interrupt(&id).is_ok());
        assert!(h.owned.interrupt("no-such-id").is_err());
        let o = h.owned.clone();
        let i = id.clone();
        wait_for("turn", move || turn_results(&o, &i) == 1).await;
        let t = h.owned.get_transcript(&id).unwrap();
        assert!(!t.iter().any(
            |i| matches!(i, TranscriptItem::AssistantText { text } if text.contains("inner"))
        ));
        h.owned.end(&id).unwrap();
        let o = h.owned.clone();
        wait_for("exit", move || o.running_count() == 0).await;
    }

    #[tokio::test]
    async fn kill_all_stops_a_running_process() {
        let Some(h) = harness() else { return };
        let cwd = h.dir.path().to_string_lossy().into_owned();
        let id = h.owned.launch(&cwd, "hello").await.unwrap();
        let o = h.owned.clone();
        let i = id.clone();
        wait_for("turn", move || turn_results(&o, &i) == 1).await;
        assert_eq!(h.owned.running_count(), 1);
        h.owned.kill_all();
        let o = h.owned.clone();
        wait_for("exit", move || o.running_count() == 0).await;
    }
}
