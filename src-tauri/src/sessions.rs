//! The session state machine. Pure logic: no I/O, no Tauri, no clock.
//! The caller passes "now" (Unix milliseconds) into every method, which
//! makes the whole module trivial to unit test.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::constants::{DROP_AFTER_END, DROP_AFTER_STALE, IDLE_AFTER, STALE_AFTER};
use crate::pending::PermissionInfo;
use crate::policy::Mode;

/// Unix time in milliseconds. A plain alias keeps signatures readable.
pub type Millis = i64;

/// `#[derive(...)]` asks the compiler to generate trait implementations for
/// us. `Serialize` lets serde turn the enum into JSON; `Clone`/`Copy` let us
/// duplicate it cheaply; `PartialEq` enables `==` (and `assert_eq!`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Working,
    NeedsYou,
    WaitingForReply,
    Idle,
    Stale,
    Ended,
    Error,
}

/// One card in the widget. This struct is also exactly what the frontend
/// receives (see `SessionSnapshot`). Timestamps are Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Session {
    pub session_id: String,
    pub cwd: String,
    /// Display name: last path component of `cwd`.
    pub project: String,
    pub state: SessionState,
    pub state_since: Millis,
    pub last_event_at: Millis,
    /// `Option<T>` is Rust's "maybe a value": `Some(x)` or `None` (null in JSON).
    pub permission_mode: Option<String>,
    /// e.g. "Write bye.txt"; only set while `NeedsYou`.
    pub pending_tool: Option<String>,
    pub transcript_path: Option<String>,
    pub ended_at: Option<Millis>,
    /// Widget behaviour for this session (ask / auto-safe / hands off / snooze).
    pub mode: Mode,
    /// A permission prompt the widget is holding open for the user.
    pub pending_permission: Option<PermissionInfo>,
    /// `true` while the widget shows a reply box for this session.
    pub reply_open: bool,
    /// Internal: identifies which held `/stop` request owns `reply_open`, so
    /// a replaced request cannot close its successor's box.
    #[serde(skip)]
    reply_ticket: u64,
    /// Internal: a PermissionRequest was seen this turn, so a later
    /// `Notification(permission_prompt)` is only a confirmation, not news.
    /// `skip` keeps it out of the JSON sent to the frontend.
    #[serde(skip)]
    saw_permission_request: bool,
}

/// What we send to the frontend. Same shape as `Session`.
pub type SessionSnapshot = Session;

/// A hook payload from Claude Code. Every field except the event name is
/// optional because different events carry different fields; serde fills
/// missing ones with `None` and ignores fields we don't list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HookEvent {
    pub hook_event_name: String,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub transcript_path: Option<String>,
    pub permission_mode: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<Value>,
    pub notification_type: Option<String>,
    pub agent_id: Option<String>,
    pub reason: Option<String>,
    pub source: Option<String>,
}

/// Owns all cards. A `Vec` keeps cards in the order they first appeared;
/// with a handful of sessions a linear search is plenty fast.
#[derive(Debug, Default)]
pub struct SessionStore {
    sessions: Vec<Session>,
}

fn ms(d: std::time::Duration) -> Millis {
    d.as_millis() as Millis
}

/// Basename of a path, splitting on both `\` and `/` so Windows paths work
/// on any OS.
pub fn project_name(cwd: &str) -> String {
    cwd.trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

/// "Write bye.txt" style summary: tool name plus a short target.
pub fn tool_summary(tool_name: &str, tool_input: Option<&Value>) -> String {
    let target = tool_input.and_then(|input| {
        // Path-like inputs: show just the file name.
        for key in ["file_path", "path", "notebook_path"] {
            if let Some(p) = input.get(key).and_then(Value::as_str) {
                return Some(project_name(p));
            }
        }
        // Commands and the like: first line, truncated.
        for key in ["command", "url", "pattern", "query"] {
            if let Some(c) = input.get(key).and_then(Value::as_str) {
                let first = c.lines().next().unwrap_or("").trim();
                return Some(truncate(first, 40));
            }
        }
        None
    });
    match target {
        Some(t) if !t.is_empty() => format!("{tool_name} {t}"),
        _ => tool_name.to_string(),
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cards in first-seen order (cloned, so the caller can send them anywhere).
    pub fn snapshot(&self) -> Vec<SessionSnapshot> {
        self.sessions.clone()
    }

    pub fn get(&self, session_id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.session_id == session_id)
    }

    /// Apply one hook event. Returns `true` if anything the UI shows changed
    /// (so the caller knows whether to emit a new snapshot).
    ///
    /// `&mut self` = exclusive borrow: we may modify the store while this
    /// method runs, and nobody else can touch it at the same time.
    pub fn apply(&mut self, event: &HookEvent, now: Millis) -> bool {
        // Subagent events never move the parent session or create a card.
        if event.agent_id.as_deref().is_some_and(|a| !a.is_empty()) {
            return false;
        }
        let name = event.hook_event_name.as_str();
        if matches!(name, "MessageDisplay" | "ConfigChange") {
            return false;
        }
        // `let ... else` returns early when the pattern does not match.
        let Some(session_id) = event.session_id.as_deref().filter(|s| !s.is_empty()) else {
            return false;
        };

        let mut changed = false;
        let idx = match self
            .sessions
            .iter()
            .position(|s| s.session_id == session_id)
        {
            Some(i) => i,
            None => {
                // Unknown session on any event: create a card (the widget may
                // have started mid-session).
                self.sessions.push(Session {
                    session_id: session_id.to_string(),
                    cwd: String::new(),
                    project: "unknown".to_string(),
                    state: SessionState::WaitingForReply,
                    state_since: now,
                    last_event_at: now,
                    permission_mode: None,
                    pending_tool: None,
                    transcript_path: None,
                    ended_at: None,
                    mode: Mode::default(),
                    pending_permission: None,
                    reply_open: false,
                    reply_ticket: 0,
                    saw_permission_request: false,
                });
                changed = true;
                self.sessions.len() - 1
            }
        };
        // Borrow one card mutably for the rest of the function.
        let s = &mut self.sessions[idx];
        s.last_event_at = now;

        if let Some(cwd) = event.cwd.as_deref().filter(|c| !c.is_empty()) {
            if s.cwd != cwd {
                s.cwd = cwd.to_string();
                s.project = project_name(cwd);
                changed = true;
            }
        }
        if let Some(t) = event.transcript_path.as_deref().filter(|t| !t.is_empty()) {
            if s.transcript_path.as_deref() != Some(t) {
                s.transcript_path = Some(t.to_string());
                changed = true;
            }
        }
        if let Some(m) = event.permission_mode.as_deref() {
            if s.permission_mode.as_deref() != Some(m) {
                s.permission_mode = Some(m.to_string());
                changed = true;
            }
        }

        // What should the state be after this event? `None` = no opinion.
        let target: Option<(SessionState, Option<String>)> = match name {
            "SessionStart" => Some((SessionState::WaitingForReply, None)),
            "UserPromptSubmit" => Some((SessionState::Working, None)),
            "PreToolUse" => {
                // PreToolUse fires BEFORE PermissionRequest, and parallel tools
                // can interleave, so it must not clear a pending prompt.
                if s.state == SessionState::NeedsYou {
                    None
                } else {
                    Some((SessionState::Working, None))
                }
            }
            "PostToolUse" | "PostToolUseFailure" | "PostToolBatch" => {
                Some((SessionState::Working, None))
            }
            "PermissionRequest" => {
                s.saw_permission_request = true;
                let summary = tool_summary(
                    event.tool_name.as_deref().unwrap_or("Tool"),
                    event.tool_input.as_ref(),
                );
                Some((SessionState::NeedsYou, Some(summary)))
            }
            "Notification" => match event.notification_type.as_deref() {
                // Fallback only if no PermissionRequest was seen this turn;
                // otherwise a late notification (arrives ~6 s after the
                // request) could re-open a prompt the user already answered.
                Some("permission_prompt") if !s.saw_permission_request => {
                    Some((SessionState::NeedsYou, s.pending_tool.clone()))
                }
                Some("idle_prompt")
                    if matches!(s.state, SessionState::WaitingForReply | SessionState::Stale) =>
                {
                    Some((SessionState::Idle, None))
                }
                _ => None,
            },
            "Stop" => Some((SessionState::WaitingForReply, None)),
            "StopFailure" => Some((SessionState::Error, None)),
            "SessionEnd" => Some((SessionState::Ended, None)),
            _ => None,
        };

        // Turn boundaries reset the per-turn permission flag.
        if matches!(
            name,
            "SessionStart" | "UserPromptSubmit" | "Stop" | "SessionEnd"
        ) {
            s.saw_permission_request = false;
        }

        // Ended cards only come back to life on a new session/prompt
        // (e.g. `claude --resume` reusing the id); stragglers are ignored.
        let revives = matches!(name, "SessionStart" | "UserPromptSubmit");
        let target = if s.state == SessionState::Ended && !revives {
            None
        } else if target.is_none() && s.state == SessionState::Stale {
            // Any sign of life un-greys a stale card.
            Some((SessionState::WaitingForReply, None))
        } else {
            target
        };

        if let Some((state, pending)) = target {
            if s.state != state {
                s.state = state;
                s.state_since = now;
                changed = true;
            }
            if s.pending_tool != pending {
                s.pending_tool = pending;
                changed = true;
            }
            if state == SessionState::Ended {
                s.ended_at = Some(now);
            } else if s.ended_at.is_some() {
                s.ended_at = None;
            }
        }

        // The session moved on, so any prompt / reply box the widget was
        // holding is obsolete (the terminal answered first, or the user typed).
        if matches!(
            name,
            "PostToolUse" | "PostToolUseFailure" | "Stop" | "UserPromptSubmit" | "SessionEnd"
        ) && s.pending_permission.take().is_some()
        {
            changed = true;
        }
        if matches!(name, "UserPromptSubmit" | "SessionEnd") && s.reply_open {
            s.reply_open = false;
            changed = true;
        }
        changed
    }

    pub fn mode(&self, session_id: &str) -> Mode {
        self.get(session_id).map(|s| s.mode).unwrap_or_default()
    }

    fn find_mut(&mut self, session_id: &str) -> Option<&mut Session> {
        self.sessions
            .iter_mut()
            .find(|s| s.session_id == session_id)
    }

    /// Returns `true` if the mode actually changed.
    pub fn set_mode(&mut self, session_id: &str, mode: Mode) -> bool {
        match self.find_mut(session_id) {
            Some(s) if s.mode != mode => {
                s.mode = mode;
                true
            }
            _ => false,
        }
    }

    pub fn set_pending_permission(&mut self, session_id: &str, info: PermissionInfo) -> bool {
        match self.find_mut(session_id) {
            Some(s) => {
                s.pending_permission = Some(info);
                true
            }
            None => false,
        }
    }

    /// Clear the held prompt. With `Some(ticket)` only if that ticket still
    /// owns it (a newer request may have replaced it); `None` = always.
    pub fn clear_pending_permission(&mut self, session_id: &str, ticket: Option<u64>) -> bool {
        match self.find_mut(session_id) {
            Some(s)
                if s.pending_permission
                    .as_ref()
                    .is_some_and(|p| ticket.is_none_or(|t| p.ticket == t)) =>
            {
                s.pending_permission = None;
                true
            }
            _ => false,
        }
    }

    pub fn open_reply(&mut self, session_id: &str, ticket: u64) -> bool {
        match self.find_mut(session_id) {
            Some(s) => {
                s.reply_open = true;
                s.reply_ticket = ticket;
                true
            }
            None => false,
        }
    }

    pub fn clear_reply(&mut self, session_id: &str, ticket: u64) -> bool {
        match self.find_mut(session_id) {
            Some(s) if s.reply_open && s.reply_ticket == ticket => {
                s.reply_open = false;
                true
            }
            _ => false,
        }
    }

    /// The transcript watcher saw "[Request interrupted by user": the prompt
    /// was denied/escaped outside the widget and no hook event will follow.
    pub fn mark_interrupted(&mut self, session_id: &str, now: Millis) -> bool {
        match self
            .sessions
            .iter_mut()
            .find(|s| s.session_id == session_id)
        {
            Some(s) if s.state == SessionState::NeedsYou => {
                s.state = SessionState::WaitingForReply;
                s.state_since = now;
                s.pending_tool = None;
                s.pending_permission = None;
                s.saw_permission_request = false;
                true
            }
            _ => false,
        }
    }

    /// Timer pass: idle after 5 min waiting, stale after 30 min of silence,
    /// and removal of ended / long-stale cards. `NeedsYou` never times out.
    /// Returns `true` if anything changed.
    pub fn sweep(&mut self, now: Millis) -> bool {
        let before = self.sessions.len();
        let mut changed = false;
        for s in &mut self.sessions {
            // An expired snooze goes back to "ask me".
            if matches!(s.mode, Mode::Snoozed { until } if until <= now) {
                s.mode = Mode::AskMe;
                changed = true;
            }
            let silent_for = now - s.last_event_at;
            match s.state {
                SessionState::WaitingForReply
                | SessionState::Idle
                | SessionState::Working
                | SessionState::Error
                    if silent_for >= ms(STALE_AFTER) =>
                {
                    s.state = SessionState::Stale;
                    s.state_since = now;
                    changed = true;
                }
                SessionState::WaitingForReply if silent_for >= ms(IDLE_AFTER) => {
                    s.state = SessionState::Idle;
                    s.state_since = now;
                    changed = true;
                }
                _ => {}
            }
        }
        // `retain` keeps only the cards for which the closure returns true.
        self.sessions.retain(|s| match s.state {
            SessionState::Ended => s
                .ended_at
                .map(|t| now - t < ms(DROP_AFTER_END))
                .unwrap_or(true),
            SessionState::Stale => now - s.state_since < ms(DROP_AFTER_STALE),
            _ => true,
        });
        changed || self.sessions.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Sanitised real payloads live in tests/fixtures/*.json. `include_str!`
    /// embeds the file into the test binary at compile time.
    fn load(raw: &str) -> Vec<HookEvent> {
        serde_json::from_str(raw).expect("fixture parses")
    }

    fn terminal_default() -> Vec<HookEvent> {
        load(include_str!("../tests/fixtures/terminal_default.json"))
    }

    fn event(v: Value) -> HookEvent {
        serde_json::from_value(v).expect("event parses")
    }

    /// Feed events one by one, 1 second apart; returns state after each.
    fn feed(store: &mut SessionStore, events: &[HookEvent], start: Millis) -> Vec<SessionState> {
        events
            .iter()
            .enumerate()
            .map(|(i, e)| {
                store.apply(e, start + (i as i64) * 1000);
                store.snapshot()[0].state
            })
            .collect()
    }

    fn position(events: &[HookEvent], name: &str) -> usize {
        events
            .iter()
            .position(|e| e.hook_event_name == name)
            .expect("event present in fixture")
    }

    #[test]
    fn project_name_handles_both_separators() {
        assert_eq!(project_name("C:\\work\\demo"), "demo");
        assert_eq!(project_name("/home/me/proj/"), "proj");
        assert_eq!(project_name("c:\\work\\demo\\"), "demo");
        assert_eq!(project_name(""), "unknown");
    }

    #[test]
    fn tool_summary_shortens_targets() {
        let w = json!({"file_path": "C:\\work\\demo\\bye.txt", "content": "x"});
        assert_eq!(tool_summary("Write", Some(&w)), "Write bye.txt");
        let c = json!({"command": "a".repeat(100)});
        assert_eq!(tool_summary("Bash", Some(&c)).chars().count(), 5 + 41);
        assert_eq!(tool_summary("Bash", None), "Bash");
    }

    #[test]
    fn terminal_default_full_sequence() {
        use SessionState::*;
        let events = terminal_default();
        let mut store = SessionStore::new();
        let states = feed(&mut store, &events, 1_000);

        // After PermissionRequest the card needs the user.
        let perm = position(&events, "PermissionRequest");
        assert_eq!(states[perm], NeedsYou);
        // The late Notification(permission_prompt) must not change anything.
        assert_eq!(events[perm + 1].hook_event_name, "Notification");
        assert_eq!(states[perm + 1], NeedsYou);
        // PostToolUse clears it.
        assert_eq!(states[position(&events, "PostToolUse")], Working);
        // Stop -> waiting for reply, and the subagent event after it is ignored.
        let stop = position(&events, "Stop");
        assert_eq!(states[stop], WaitingForReply);
        assert_eq!(states[stop + 1], WaitingForReply);
        // The trailing Notification(idle_prompt) makes the card idle.
        assert_eq!(*states.last().unwrap(), Idle);
        assert_eq!(store.snapshot().len(), 1);
        assert_eq!(store.snapshot()[0].project, "demo");
        assert_eq!(store.snapshot()[0].pending_tool, None);
    }

    #[test]
    fn permission_request_sets_tool_summary() {
        let events = terminal_default();
        let mut store = SessionStore::new();
        for (i, e) in events.iter().enumerate() {
            store.apply(e, i as i64);
            if e.hook_event_name == "PermissionRequest" {
                let s = &store.snapshot()[0];
                assert_eq!(s.state, SessionState::NeedsYou);
                assert!(s.pending_tool.as_deref().unwrap().starts_with("Write "));
                return;
            }
        }
        panic!("fixture has no PermissionRequest");
    }

    #[test]
    fn vscode_deny_stays_needs_you_until_interrupted() {
        let events = load(include_str!("../tests/fixtures/vscode_deny.json"));
        let mut store = SessionStore::new();
        let states = feed(&mut store, &events, 1_000);
        // The log ends right after the second PermissionRequest and a
        // SubagentStop: nothing clears the prompt.
        assert_eq!(*states.last().unwrap(), SessionState::NeedsYou);
        let id = events[0].session_id.clone().unwrap();
        assert!(store.mark_interrupted(&id, 99_000));
        let s = store.get(&id).unwrap();
        assert_eq!(s.state, SessionState::WaitingForReply);
        assert_eq!(s.pending_tool, None);
        assert_eq!(s.state_since, 99_000);
        // Second call is a no-op.
        assert!(!store.mark_interrupted(&id, 100_000));
    }

    #[test]
    fn subagent_event_neither_changes_state_nor_creates_card() {
        let mut store = SessionStore::new();
        // A subagent event for a session we have never seen: no card.
        let stray = event(json!({
            "hook_event_name": "SubagentStop", "session_id": "other", "agent_id": "abc"
        }));
        assert!(!store.apply(&stray, 1));
        assert!(store.snapshot().is_empty());

        // After Stop, a SubagentStop leaves the state alone.
        let events = terminal_default();
        let stop = position(&events, "Stop");
        feed(&mut store, &events[..=stop], 10);
        let before = store.snapshot();
        let sub = events.iter().find(|e| e.agent_id.is_some()).unwrap();
        assert!(!store.apply(sub, 99_000));
        assert_eq!(store.snapshot(), before);
    }

    #[test]
    fn auto_mode_never_needs_you() {
        let events = load(include_str!("../tests/fixtures/auto_mode.json"));
        let mut store = SessionStore::new();
        let states = feed(&mut store, &events, 1_000);
        assert!(!states.contains(&SessionState::NeedsYou));
        assert_eq!(*states.last().unwrap(), SessionState::Ended);
        assert_eq!(store.snapshot()[0].permission_mode.as_deref(), Some("auto"));
    }

    #[test]
    fn print_mode_start_to_end_ends() {
        let events = load(include_str!("../tests/fixtures/print_mode.json"));
        let mut store = SessionStore::new();
        let states = feed(&mut store, &events, 1_000);
        assert_eq!(states[0], SessionState::WaitingForReply);
        assert_eq!(*states.last().unwrap(), SessionState::Ended);
        let ended_at = store.snapshot()[0].ended_at.expect("ended_at set");
        // After the drop delay the card disappears.
        assert!(store.sweep(ended_at + 11_000));
        assert!(store.snapshot().is_empty());
    }

    #[test]
    fn unknown_session_mid_way_creates_card() {
        let mut store = SessionStore::new();
        let e = event(json!({
            "hook_event_name": "PreToolUse", "session_id": "s1",
            "cwd": "c:\\work\\demo", "tool_name": "Read", "permission_mode": "default"
        }));
        assert!(store.apply(&e, 5));
        let s = &store.snapshot()[0];
        assert_eq!(s.state, SessionState::Working);
        assert_eq!(s.project, "demo");
        // Same event again changes nothing visible.
        assert!(!store.apply(&e, 6));
    }

    #[test]
    fn ignored_and_malformed_events() {
        let mut store = SessionStore::new();
        let md = event(json!({"hook_event_name": "MessageDisplay", "session_id": "s"}));
        let cc = event(json!({"hook_event_name": "ConfigChange", "session_id": "s"}));
        let no_id = event(json!({"hook_event_name": "Stop"}));
        for e in [md, cc, no_id] {
            assert!(!store.apply(&e, 1));
        }
        assert!(store.snapshot().is_empty());
    }

    #[test]
    fn stop_failure_and_prompt_recovery() {
        let mut store = SessionStore::new();
        let mk =
            |name: &str| event(json!({"hook_event_name": name, "session_id": "s", "cwd": "/a/b"}));
        store.apply(&mk("UserPromptSubmit"), 1);
        store.apply(&mk("StopFailure"), 2);
        assert_eq!(store.get("s").unwrap().state, SessionState::Error);
        store.apply(&mk("UserPromptSubmit"), 3);
        assert_eq!(store.get("s").unwrap().state, SessionState::Working);
    }

    #[test]
    fn permission_prompt_notification_fallback() {
        let mut store = SessionStore::new();
        let n = event(json!({
            "hook_event_name": "Notification", "session_id": "s",
            "notification_type": "permission_prompt"
        }));
        store.apply(&n, 1);
        assert_eq!(store.get("s").unwrap().state, SessionState::NeedsYou);
    }

    #[test]
    fn sweep_idles_then_stales_then_drops() {
        let mut store = SessionStore::new();
        let e = event(json!({"hook_event_name": "Stop", "session_id": "s"}));
        store.apply(&e, 0);
        assert!(!store.sweep(60_000));
        assert!(store.sweep(5 * 60_000));
        assert_eq!(store.get("s").unwrap().state, SessionState::Idle);
        assert!(store.sweep(30 * 60_000));
        assert_eq!(store.get("s").unwrap().state, SessionState::Stale);
        // Any event wakes it up again.
        store.apply(&e, 31 * 60_000);
        assert_eq!(store.get("s").unwrap().state, SessionState::WaitingForReply);
        // Needs-you never goes stale.
        let p = event(
            json!({"hook_event_name": "PermissionRequest", "session_id": "s", "tool_name": "Bash"}),
        );
        store.apply(&p, 32 * 60_000);
        assert!(!store.sweep(200 * 60_000));
        assert_eq!(store.get("s").unwrap().state, SessionState::NeedsYou);
    }

    #[test]
    fn stale_card_is_dropped_after_two_hours() {
        let mut store = SessionStore::new();
        let e = event(json!({"hook_event_name": "Stop", "session_id": "s"}));
        store.apply(&e, 0);
        store.sweep(31 * 60_000);
        assert_eq!(store.get("s").unwrap().state, SessionState::Stale);
        store.sweep(31 * 60_000 + 2 * 3600 * 1000);
        assert!(store.snapshot().is_empty());
    }

    fn ev(name: &str) -> HookEvent {
        event(json!({"hook_event_name": name, "session_id": "s"}))
    }

    fn info(ticket: u64) -> PermissionInfo {
        PermissionInfo {
            tool_name: "Bash".into(),
            input_preview: "ls".into(),
            expires_at: 99,
            ticket,
        }
    }

    #[test]
    fn mode_defaults_to_ask_me_and_snooze_expires_in_sweep() {
        let mut store = SessionStore::new();
        store.apply(&ev("Stop"), 0);
        assert_eq!(store.get("s").unwrap().mode, Mode::AskMe);
        assert_eq!(store.mode("unknown"), Mode::AskMe);
        assert!(store.set_mode("s", Mode::Snoozed { until: 10_000 }));
        assert!(!store.set_mode("s", Mode::Snoozed { until: 10_000 }));
        assert!(!store.set_mode("nope", Mode::HandsOff));
        assert!(!store.sweep(9_999));
        assert!(store.sweep(10_000));
        assert_eq!(store.get("s").unwrap().mode, Mode::AskMe);
        // Other modes are never touched by the sweep.
        store.set_mode("s", Mode::HandsOff);
        assert!(!store.sweep(20_000));
        assert_eq!(store.get("s").unwrap().mode, Mode::HandsOff);
    }

    #[test]
    fn pending_permission_cleared_by_moving_events() {
        for name in [
            "PostToolUse",
            "PostToolUseFailure",
            "Stop",
            "UserPromptSubmit",
            "SessionEnd",
        ] {
            let mut store = SessionStore::new();
            store.apply(&ev("Stop"), 0);
            assert!(store.set_pending_permission("s", info(1)));
            assert!(store.apply(&ev(name), 1));
            assert!(
                store.get("s").unwrap().pending_permission.is_none(),
                "{name}"
            );
        }
        // Other events leave it alone.
        let mut store = SessionStore::new();
        store.apply(&ev("Stop"), 0);
        store.set_pending_permission("s", info(1));
        store.apply(&ev("PreToolUse"), 1);
        assert!(store.get("s").unwrap().pending_permission.is_some());
    }

    #[test]
    fn ticketed_clear_ignores_replaced_requests() {
        let mut store = SessionStore::new();
        store.apply(&ev("Stop"), 0);
        store.set_pending_permission("s", info(2));
        assert!(!store.clear_pending_permission("s", Some(1)));
        assert!(store.clear_pending_permission("s", Some(2)));
        assert!(!store.clear_pending_permission("s", None));
        store.open_reply("s", 5);
        assert!(!store.clear_reply("s", 4));
        assert!(store.get("s").unwrap().reply_open);
        assert!(store.clear_reply("s", 5));
    }

    #[test]
    fn reply_closed_by_prompt_or_end_not_by_stop() {
        let mut store = SessionStore::new();
        store.apply(&ev("Stop"), 0);
        store.open_reply("s", 1);
        store.apply(&ev("Stop"), 1);
        assert!(store.get("s").unwrap().reply_open);
        store.apply(&ev("UserPromptSubmit"), 2);
        assert!(!store.get("s").unwrap().reply_open);
        store.open_reply("s", 2);
        store.apply(&ev("SessionEnd"), 3);
        assert!(!store.get("s").unwrap().reply_open);
    }

    #[test]
    fn interrupt_clears_pending_permission() {
        let mut store = SessionStore::new();
        let p = event(
            json!({"hook_event_name": "PermissionRequest", "session_id": "s", "tool_name": "Bash"}),
        );
        store.apply(&p, 0);
        store.set_pending_permission("s", info(1));
        assert!(store.mark_interrupted("s", 5));
        assert!(store.get("s").unwrap().pending_permission.is_none());
    }
}
