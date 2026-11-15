//! agentwatch hook command (ADR-5, ADR-8). Claude Code runs this for every hook
//! event: the event JSON arrives on stdin and is forwarded to the widget over
//! localhost.
//!
//! FAIL OPEN: the widget being closed (or anything else going wrong) must be
//! invisible to Claude Code. So this program
//!  - always drains stdin (exiting early could give Claude Code a broken pipe),
//!  - writes to stdout ONLY the PermissionRequest decision JSON,
//!  - writes to stderr ONLY the Stop reply text,
//!  - exits 0 in every case except a Stop reply (exit 2 wakes an asyncRewake
//!    Stop hook with the stderr text; any other non-zero code shows an error),
//!  - gives up after a hard deadline, because hooks without a timeout never end.
//!
//! Routes (by hook_event_name):
//!  - PermissionRequest -> POST /permission (long poll, up to ~2 min)
//!  - Stop              -> POST /stop       (long poll, up to ~30 min)
//!  - everything else   -> POST /event      (fire and forget, ~0.5 s)

mod http;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::exit;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

const MAX_BODY_BYTES: usize = 1024 * 1024; // matches the server's 1 MB limit
const EVENT_FETCH_MS: u64 = 400;
const EVENT_DEADLINE_MS: u64 = 500;
// Server holds /permission up to 120 s; the hook config timeout is 130 s.
const PERMISSION_FETCH_MS: u64 = 125_000;
// Server holds /stop up to 1800 s plus a small margin.
const STOP_FETCH_MS: u64 = 1_805_000;
// The hard deadline must never fire before the fetch timeout, so it is longer.
const DEADLINE_MARGIN_MS: u64 = 1_000;

const ALLOW_JSON: &str = r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}"#;
const DENY_JSON: &str = r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny","message":"Denied from agentwatch"}}}"#;
const STOP_PREFIX: &str = "The user replied from the agentwatch widget:\n";

/// Env overrides exist for tests only; production uses the defaults.
fn env_ms(name: &str, fallback: u64) -> u64 {
    let parsed = std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok());
    match parsed {
        Some(n) if n.is_finite() && n >= 1.0 => n as u64,
        _ => fallback,
    }
}

struct Endpoint {
    port: u16,
    token: String,
}

fn data_dir() -> Option<PathBuf> {
    match std::env::var("AGENTWATCH_DIR") {
        Ok(d) if !d.is_empty() => Some(PathBuf::from(d)),
        // USERPROFILE on Windows, HOME on Linux/macOS.
        _ => std::env::home_dir().map(|h| h.join(".agentwatch")),
    }
}

/// Reads and validates `endpoint.json`; `None` means the widget is not running.
fn read_endpoint() -> Option<Endpoint> {
    let text = std::fs::read_to_string(data_dir()?.join("endpoint.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let obj = v.as_object()?;
    // JS numbers have no int/float split, so 8080.0 counts as an integer there.
    let port = obj.get("port")?.as_f64()?;
    if port.fract() != 0.0 || !(1.0..=65535.0).contains(&port) {
        return None;
    }
    let token = obj.get("token")?.as_str()?;
    if token.is_empty() {
        return None;
    }
    Some(Endpoint {
        port: port as u16,
        token: token.to_string(),
    })
}

/// Reads stdin to the end. When `keep` is false the data is discarded (but still
/// drained). Returns `None` if discarded or larger than the limit.
fn read_stdin(keep: bool) -> Option<Vec<u8>> {
    let mut stdin = std::io::stdin().lock();
    let mut data = Vec::new();
    let mut total = 0usize;
    let mut buf = [0u8; 8192];
    loop {
        match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                total += n;
                if keep && total <= MAX_BODY_BYTES {
                    data.extend_from_slice(&buf[..n]);
                }
            }
        }
    }
    if !keep || total > MAX_BODY_BYTES {
        return None; // too big: the server would answer 413
    }
    Some(data)
}

/// A positive integer from the environment, else JSON null.
fn env_pid(name: &str) -> Value {
    let parsed = std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok());
    match parsed {
        Some(n) if n > 0 => json!(n),
        _ => Value::Null,
    }
}

/// Adds a top-level `agentwatch` key with facts only the hook's environment
/// knows. Bodies that are not a JSON object are forwarded unchanged.
fn with_meta(text: &str) -> String {
    // `let ... else` runs the else block when the pattern does not match.
    let Ok(Value::Object(mut obj)) = serde_json::from_str::<Value>(text) else {
        return text.to_string();
    };
    let entrypoint = match std::env::var("CLAUDE_CODE_ENTRYPOINT") {
        Ok(v) if !v.is_empty() => json!(v),
        _ => Value::Null,
    };
    let meta = json!({
        "entrypoint": entrypoint,
        "claude_pid": env_pid("CLAUDE_PID"),
        "vscode_pid": env_pid("VSCODE_PID"),
    });
    obj.insert("agentwatch".to_string(), meta);
    serde_json::to_string(&Value::Object(obj)).unwrap_or_else(|_| text.to_string())
}

fn event_name(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("hook_event_name")?.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Parsed JSON object of a 200 response, else `None`.
fn json_of(res: Option<http::Response>) -> Option<Map<String, Value>> {
    let res = res?;
    if res.status != 200 {
        return None;
    }
    match serde_json::from_slice::<Value>(&res.body).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn write_stdout(text: &str) {
    // Errors (closed pipe etc.) are ignored on purpose: fail open.
    let mut s = std::io::stdout();
    let _ = s.write_all(text.as_bytes());
    let _ = s.flush();
}

fn write_stderr(text: &str) {
    let mut s = std::io::stderr();
    let _ = s.write_all(text.as_bytes());
    let _ = s.flush();
}

/// Starts a thread that ends the process with exit code 0 once the deadline
/// passes. The deadline (ms since `start`) can be moved later via the atomic.
fn start_watchdog(start: Instant, deadline_ms: Arc<AtomicU64>) {
    std::thread::spawn(move || loop {
        let limit = deadline_ms.load(Ordering::SeqCst);
        if start.elapsed() >= Duration::from_millis(limit) {
            exit(0);
        }
        std::thread::sleep(Duration::from_millis(10));
    });
}

/// Moves the hard deadline to `fetch_ms` (+ margin) from now.
fn set_deadline(start: Instant, deadline_ms: &AtomicU64, fetch_ms: u64) {
    let now = start.elapsed().as_millis() as u64;
    deadline_ms.store(now + fetch_ms + DEADLINE_MARGIN_MS, Ordering::SeqCst);
}

fn run(start: Instant, deadline_ms: &AtomicU64) -> i32 {
    // Check the endpoint first so a closed widget costs no network; stdin is
    // still drained.
    let endpoint = read_endpoint();
    let body = read_stdin(endpoint.is_some());
    let (Some(endpoint), Some(body)) = (endpoint, body) else {
        return 0;
    };
    if body.is_empty() {
        return 0;
    }
    let text = with_meta(&String::from_utf8_lossy(&body));
    // A closure that captures `endpoint` and `text` by reference.
    let post = |path: &str, fetch_ms: u64| {
        http::post(
            endpoint.port,
            path,
            &endpoint.token,
            text.as_bytes(),
            Duration::from_millis(fetch_ms),
        )
    };

    match event_name(&text).as_str() {
        "PermissionRequest" => {
            let ms = env_ms("AGENTWATCH_PERMISSION_TIMEOUT_MS", PERMISSION_FETCH_MS);
            set_deadline(start, deadline_ms, ms);
            if let Some(json) = json_of(post("/permission", ms)) {
                match json.get("decision").and_then(Value::as_str) {
                    Some("allow") => write_stdout(ALLOW_JSON),
                    Some("deny") => write_stdout(DENY_JSON),
                    _ => {} // "none" or anything else: the terminal prompt decides
                }
            }
            0
        }
        "Stop" => {
            let ms = env_ms("AGENTWATCH_REPLY_TIMEOUT_MS", STOP_FETCH_MS);
            set_deadline(start, deadline_ms, ms);
            if let Some(json) = json_of(post("/stop", ms)) {
                if let Some(reply) = json.get("reply").and_then(Value::as_str) {
                    if !reply.trim().is_empty() {
                        write_stderr(&format!("{STOP_PREFIX}{reply}"));
                        return 2; // the only non-zero exit: wakes Claude (asyncRewake)
                    }
                }
            }
            0
        }
        _ => {
            // The response is deliberately ignored for plain events.
            post("/event", EVENT_FETCH_MS);
            0
        }
    }
}

fn main() {
    // With `panic = "abort"` a panic would end the process with a crash code.
    // This hook runs first and turns any panic into a silent exit 0.
    std::panic::set_hook(Box::new(|_| exit(0)));

    let start = Instant::now();
    // The initial deadline also covers a stdin that never closes. `Arc` is a
    // shared pointer so the watchdog thread and `run` can both hold the value.
    let deadline_ms = Arc::new(AtomicU64::new(EVENT_DEADLINE_MS));
    start_watchdog(start, deadline_ms.clone());
    let code = run(start, &deadline_ms);
    exit(code);
}
