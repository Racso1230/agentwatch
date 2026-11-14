//! Local HTTP listener the hook shim POSTs to. Knows nothing about Tauri:
//! when the session list changes it calls a callback, and `lib.rs` plugs in
//! one that emits a Tauri event.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::net::TcpListener;

use crate::config::Config;
use crate::constants::MAX_BODY_BYTES;
use crate::pending::{input_preview, Pending, PermissionAnswer, PermissionInfo, ReplyAnswer};
use crate::policy::{self, Decision, Mode};
use crate::sessions::{HookEvent, Millis, SessionSnapshot, SessionStore};

/// `Arc` = shared ownership across threads (a reference-counted pointer).
/// `Mutex` = only one thread may touch the store at a time. Together,
/// `Arc<Mutex<T>>` is the standard way to share mutable data between the
/// HTTP handler, timers and Tauri commands.
pub type SharedStore = Arc<Mutex<SessionStore>>;

/// Called (outside the lock) with the full snapshot whenever state changed.
/// `dyn Fn` is a "trait object": any closure with this signature fits.
/// `Send + Sync` promise it is safe to call from any thread.
pub type OnChange = Arc<dyn Fn(Vec<SessionSnapshot>) + Send + Sync>;

/// Everything a request handler needs. `Clone` is cheap: it only copies the
/// `Arc` pointers, not the data behind them.
#[derive(Clone)]
pub struct AppState {
    pub store: SharedStore,
    pub token: Arc<str>,
    pub on_change: OnChange,
    /// Injectable clock so tests are deterministic.
    pub now: fn() -> Millis,
    /// Held permission prompts / reply boxes (see `pending.rs`).
    pub pending: Pending,
    /// Allowlist and timeouts from `~/.agentwatch/config.json`.
    pub config: Config,
}

pub fn now_millis() -> Millis {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as Millis)
        .unwrap_or(0)
}

/// Compare two strings in time that does not depend on where they differ,
/// so an attacker cannot guess the token byte by byte from response times.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|given| constant_time_eq(given.as_bytes(), token.as_bytes()))
}

/// Emit the current snapshot to the UI. Call this WITHOUT holding the lock.
fn emit(state: &AppState) {
    let snapshot = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .snapshot();
    (state.on_change)(snapshot);
}

/// Apply one hook event to the store, resolve any held request the event
/// makes obsolete, and notify the UI if something changed. Shared by
/// `/event`, `/permission` and `/stop`.
fn apply_event(state: &AppState, event: &HookEvent) {
    // Hold the lock only while mutating, and never across an `.await`.
    let snapshot = {
        // A poisoned mutex (another thread panicked) still holds valid data
        // for us, so recover it instead of panicking too.
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        if store.apply(event, (state.now)()) {
            Some(store.snapshot())
        } else {
            None
        }
    };

    // Subagent events say nothing about the parent session's prompt.
    let is_subagent = event.agent_id.as_deref().is_some_and(|a| !a.is_empty());
    if let (Some(id), false) = (event.session_id.as_deref(), is_subagent) {
        let name = event.hook_event_name.as_str();
        // The session moved on: the terminal (or the user) answered first.
        if matches!(
            name,
            "PostToolUse" | "PostToolUseFailure" | "Stop" | "UserPromptSubmit" | "SessionEnd"
        ) {
            state.pending.cancel_permission(id);
        }
        if matches!(name, "UserPromptSubmit" | "SessionEnd") {
            state.pending.cancel_reply(id);
        }
    }
    if let Some(snapshot) = snapshot {
        (state.on_change)(snapshot);
    }
}

/// `POST /event`. Always answers quickly; the shim ignores the body.
async fn post_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    if !authorized(&headers, &state.token) {
        return (StatusCode::UNAUTHORIZED, Json(json!({})));
    }
    let Ok(event) = serde_json::from_slice::<HookEvent>(&body) else {
        return (StatusCode::BAD_REQUEST, Json(json!({})));
    };
    apply_event(&state, &event);
    (StatusCode::OK, Json(json!({})))
}

#[derive(Clone, Copy)]
enum Kind {
    Permission,
    Reply,
}

/// Cleans up a held request when it goes out of scope.
///
/// Rust calls `drop` automatically when a value is destroyed, including when
/// an async handler is cancelled. That is exactly what happens if the HTTP
/// client disconnects (the shim was killed or timed out): the framework
/// drops the handler's future, and this guard still runs, so a stale
/// "needs you" card never lingers.
struct HeldGuard {
    state: AppState,
    session_id: String,
    ticket: u64,
    kind: Kind,
}

impl Drop for HeldGuard {
    fn drop(&mut self) {
        let changed = {
            let mut store = self.state.store.lock().unwrap_or_else(|e| e.into_inner());
            match self.kind {
                Kind::Permission => {
                    self.state
                        .pending
                        .release_permission(&self.session_id, self.ticket);
                    store.clear_pending_permission(&self.session_id, Some(self.ticket))
                }
                Kind::Reply => {
                    self.state
                        .pending
                        .release_reply(&self.session_id, self.ticket);
                    store.clear_reply(&self.session_id, self.ticket)
                }
            }
        };
        if changed {
            emit(&self.state);
        }
    }
}

fn ms(d: Duration) -> Millis {
    d.as_millis() as Millis
}

/// The session id of an event, if present and non-empty.
fn session_of(event: &HookEvent) -> Option<String> {
    event.session_id.clone().filter(|s| !s.is_empty())
}

/// `POST /permission`: the shim's PermissionRequest hook. The response is
/// `{"decision":"allow"|"deny"|"none"}`. "none" tells the shim to print
/// nothing so the terminal prompt decides.
async fn post_permission(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    if !authorized(&headers, &state.token) {
        return (StatusCode::UNAUTHORIZED, Json(json!({})));
    }
    let Ok(event) = serde_json::from_slice::<HookEvent>(&body) else {
        return (StatusCode::BAD_REQUEST, Json(json!({})));
    };
    let none = (StatusCode::OK, Json(json!({"decision": "none"})));
    let Some(session_id) = session_of(&event) else {
        return none;
    };
    let tool_name = event.tool_name.clone().unwrap_or_default();
    let now = (state.now)();

    let mode = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .mode(&session_id);
    match policy::decide(
        &mode,
        &tool_name,
        event.tool_input.as_ref(),
        &state.config.safe_commands,
        now,
    ) {
        // Auto-approved: don't even show a "needs you" card.
        Decision::Allow => return (StatusCode::OK, Json(json!({"decision": "allow"}))),
        Decision::NoDecision => {
            // The terminal asks; the card turns to "needs you" as usual.
            apply_event(&state, &event);
            return none;
        }
        Decision::AskUser => {}
    }

    // Record the PermissionRequest (state becomes NeedsYou), then hold.
    apply_event(&state, &event);
    let timeout = state.config.permission_timeout();
    let (ticket, receiver) = state.pending.register_permission(&session_id);
    // From here on, however this function ends, the guard cleans up.
    let _guard = HeldGuard {
        state: state.clone(),
        session_id: session_id.clone(),
        ticket,
        kind: Kind::Permission,
    };
    let info = PermissionInfo {
        tool_name: tool_name.clone(),
        input_preview: input_preview(&tool_name, event.tool_input.as_ref()),
        expires_at: now + ms(timeout),
        ticket,
    };
    let stored = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_pending_permission(&session_id, info);
    if stored {
        emit(&state);
    }

    // Long-poll: sleep until the UI answers, the session moves on, or the
    // timeout fires. `timeout(...)` wraps a future in a deadline and returns
    // `Err(Elapsed)` if it runs out. The inner `receiver.await` is a
    // `Result` too: `Err` means the sender was dropped without an answer.
    let answer = match tokio::time::timeout(timeout, receiver).await {
        Ok(Ok(answer)) => answer,
        Ok(Err(_)) | Err(_) => PermissionAnswer::None,
    };
    let decision = match answer {
        PermissionAnswer::Allow => "allow",
        PermissionAnswer::Deny => "deny",
        PermissionAnswer::None => "none",
    };
    (StatusCode::OK, Json(json!({"decision": decision})))
    // `_guard` is dropped here: clears the pending info and emits.
}

/// `POST /stop`: the shim's Stop hook. The response is `{"reply":"text"}`
/// or `{"reply":null}`.
async fn post_stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    if !authorized(&headers, &state.token) {
        return (StatusCode::UNAUTHORIZED, Json(json!({})));
    }
    let Ok(event) = serde_json::from_slice::<HookEvent>(&body) else {
        return (StatusCode::BAD_REQUEST, Json(json!({})));
    };
    let no_reply = (StatusCode::OK, Json(json!({"reply": null})));
    apply_event(&state, &event);
    let Some(session_id) = session_of(&event) else {
        return no_reply;
    };
    // Hands off / snoozed: stay silent, no reply box.
    let mode = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .mode(&session_id);
    if mode.is_silent((state.now)()) {
        return no_reply;
    }

    let max_wait = state.config.reply_max_wait();
    let (ticket, receiver) = state.pending.register_reply(&session_id);
    let _guard = HeldGuard {
        state: state.clone(),
        session_id: session_id.clone(),
        ticket,
        kind: Kind::Reply,
    };
    let opened = state
        .store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .open_reply(&session_id, ticket);
    if opened {
        emit(&state);
    }

    match tokio::time::timeout(max_wait, receiver).await {
        Ok(Ok(ReplyAnswer::Reply(text))) => (StatusCode::OK, Json(json!({"reply": text}))),
        _ => no_reply,
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/event", post(post_event))
        .route("/permission", post(post_permission))
        .route("/stop", post(post_stop))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

/// Actions the UI triggers (called from Tauri commands in `lib.rs`). They
/// live here so they can be unit tested without a running Tauri app.
impl AppState {
    /// Deliver Approve / Deny / "Let Claude ask" to the waiting request.
    pub fn answer_permission(&self, session_id: &str, answer: PermissionAnswer) -> bool {
        self.pending.answer_permission(session_id, answer)
    }

    /// Send the user's reply text to the waiting `/stop` request.
    pub fn send_reply(&self, session_id: &str, text: &str) -> bool {
        self.pending
            .answer_reply(session_id, ReplyAnswer::Reply(text.to_string()))
    }

    /// Close the reply box without replying.
    pub fn dismiss_reply(&self, session_id: &str) -> bool {
        self.pending.cancel_reply(session_id)
    }

    /// Change a session's mode and tell the UI.
    pub fn set_mode(&self, session_id: &str, mode: Mode) -> bool {
        let changed = self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_mode(session_id, mode);
        if changed {
            emit(self);
        }
        changed
    }
}

/// Bind to 127.0.0.1 on a random free port (port 0 = "OS, you choose").
pub async fn bind() -> std::io::Result<(TcpListener, u16)> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

/// Run the server until the process exits.
pub async fn serve(listener: TcpListener, state: AppState) -> std::io::Result<()> {
    axum::serve(listener, router(state)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt; // gives us `.oneshot(request)`

    const TOKEN: &str = "secret-token";

    /// Returns the app state plus a counter of how many times on_change fired.
    fn test_state() -> (AppState, Arc<AtomicUsize>) {
        state_with(Config::default())
    }

    fn state_with(config: Config) -> (AppState, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let state = AppState {
            store: Arc::new(Mutex::new(SessionStore::new())),
            token: Arc::from(TOKEN),
            on_change: Arc::new(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
            now: || 1_000,
            pending: Pending::new(),
            config,
        };
        (state, calls)
    }

    fn request(auth: Option<&str>, body: String) -> Request<Body> {
        request_to("/event", auth, body)
    }

    fn request_to(path: &str, auth: Option<&str>, body: String) -> Request<Body> {
        let mut builder = Request::post(path);
        if let Some(a) = auth {
            builder = builder.header(header::AUTHORIZATION, a);
        }
        builder.body(Body::from(body)).unwrap()
    }

    const START: &str =
        r#"{"hook_event_name":"SessionStart","session_id":"s1","cwd":"c:\\work\\demo"}"#;

    #[tokio::test]
    async fn missing_or_wrong_token_is_401() {
        let (state, calls) = test_state();
        let app = router(state.clone());
        for auth in [None, Some("Bearer nope"), Some("secret-token")] {
            let res = app
                .clone()
                .oneshot(request(auth, START.to_string()))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "auth = {auth:?}");
        }
        assert!(state.store.lock().unwrap().snapshot().is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn good_token_updates_state_and_notifies() {
        let (state, calls) = test_state();
        let app = router(state.clone());
        let res = app
            .clone()
            .oneshot(request(Some("Bearer secret-token"), START.to_string()))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = http_body_util::BodyExt::collect(res.into_body())
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(&bytes[..], b"{}");

        let snap = state.store.lock().unwrap().snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].project, "demo");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // An event that changes nothing visible does not notify again.
        let res = app
            .oneshot(request(Some("Bearer secret-token"), START.to_string()))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn oversized_body_is_rejected() {
        let (state, _) = test_state();
        let app = router(state.clone());
        let big = format!(
            r#"{{"hook_event_name":"Stop","session_id":"s","pad":"{}"}}"#,
            "x".repeat(MAX_BODY_BYTES + 10)
        );
        let res = app
            .oneshot(request(Some("Bearer secret-token"), big))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(state.store.lock().unwrap().snapshot().is_empty());
    }

    #[tokio::test]
    async fn invalid_json_is_400() {
        let (state, _) = test_state();
        let res = router(state)
            .oneshot(request(Some("Bearer secret-token"), "not json".into()))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn serves_on_loopback_random_port() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (state, _) = test_state();
        let (listener, port) = bind().await.unwrap();
        assert_ne!(port, 0);
        assert!(listener.local_addr().unwrap().ip().is_loopback());
        let store = state.store.clone();
        tokio::spawn(serve(listener, state));

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let req = format!(
            "POST /event HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {TOKEN}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{START}",
            START.len()
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "got: {response}");
        assert_eq!(store.lock().unwrap().snapshot().len(), 1);
    }

    // ---- long-poll routes -------------------------------------------------

    use crate::sessions::SessionState;

    /// POST to `path` with a good token and return status + parsed JSON body.
    async fn call(app: &Router, path: &str, body: &str) -> (StatusCode, Value) {
        let res = app
            .clone()
            .oneshot(request_to(
                path,
                Some("Bearer secret-token"),
                body.to_string(),
            ))
            .await
            .unwrap();
        let status = res.status();
        let bytes = http_body_util::BodyExt::collect(res.into_body())
            .await
            .unwrap()
            .to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(json!(null)),
        )
    }

    fn hook(name: &str, tool: &str, input: Value) -> String {
        json!({
            "hook_event_name": name, "session_id": "s1", "cwd": "c:\\work\\demo",
            "tool_name": tool, "tool_input": input
        })
        .to_string()
    }

    fn plain(name: &str) -> String {
        json!({"hook_event_name": name, "session_id": "s1", "cwd": "c:\\work\\demo"}).to_string()
    }

    /// Poll (up to 2 s) until `cond` is true; panics otherwise.
    async fn wait_for(what: &str, cond: impl Fn() -> bool) {
        for _ in 0..200 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for: {what}");
    }

    fn session(state: &AppState) -> crate::sessions::Session {
        state.store.lock().unwrap().get("s1").unwrap().clone()
    }

    /// Start a /permission call in the background and wait until it is held.
    async fn hold_permission(
        state: &AppState,
        app: &Router,
        tool: &str,
        input: Value,
    ) -> tokio::task::JoinHandle<(StatusCode, Value)> {
        let app2 = app.clone();
        let body = hook("PermissionRequest", tool, input);
        let handle = tokio::spawn(async move { call(&app2, "/permission", &body).await });
        let pending = state.pending.clone();
        wait_for("permission held", move || pending.has_permission("s1")).await;
        handle
    }

    #[tokio::test]
    async fn permission_answer_allow_and_deny() {
        for (answer, expected) in [
            (PermissionAnswer::Allow, "allow"),
            (PermissionAnswer::Deny, "deny"),
            (PermissionAnswer::None, "none"),
        ] {
            let (state, _) = test_state();
            let app = router(state.clone());
            let handle =
                hold_permission(&state, &app, "Bash", json!({"command": "rm -rf x"})).await;

            // While held, the snapshot shows the prompt.
            let s = session(&state);
            assert_eq!(s.state, SessionState::NeedsYou);
            let p = s.pending_permission.expect("pending info");
            assert_eq!(p.tool_name, "Bash");
            assert_eq!(p.input_preview, "rm -rf x");
            assert_eq!(p.expires_at, 1_000 + 120_000);

            assert!(state.answer_permission("s1", answer));
            let (status, body) = handle.await.unwrap();
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, json!({"decision": expected}));
            // Cleaned up and announced.
            assert!(session(&state).pending_permission.is_none());
            assert!(!state.pending.has_permission("s1"));
        }
    }

    #[tokio::test]
    async fn permission_timeout_gives_none_and_clears() {
        let (state, _) = state_with(Config {
            permission_timeout_secs: 0,
            ..Config::default()
        });
        let app = router(state.clone());
        let (status, body) = call(
            &app,
            "/permission",
            &hook("PermissionRequest", "Bash", json!({"command": "rm x"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"decision": "none"}));
        assert!(session(&state).pending_permission.is_none());
        assert!(!state.pending.has_permission("s1"));
    }

    #[tokio::test]
    async fn post_tool_use_while_waiting_gives_none() {
        let (state, _) = test_state();
        let app = router(state.clone());
        let handle = hold_permission(&state, &app, "Write", json!({"file_path": "a"})).await;
        // The user answered in the terminal: PostToolUse arrives on /event.
        call(&app, "/event", &hook("PostToolUse", "Write", json!({}))).await;
        let (_, body) = handle.await.unwrap();
        assert_eq!(body, json!({"decision": "none"}));
        assert!(session(&state).pending_permission.is_none());
        assert_eq!(session(&state).state, SessionState::Working);
    }

    #[tokio::test]
    async fn subagent_event_does_not_cancel_prompt() {
        let (state, _) = test_state();
        let app = router(state.clone());
        let handle = hold_permission(&state, &app, "Write", json!({"file_path": "a"})).await;
        let sub = json!({
            "hook_event_name": "PostToolUse", "session_id": "s1", "agent_id": "sub"
        })
        .to_string();
        call(&app, "/event", &sub).await;
        assert!(state.pending.has_permission("s1"));
        state.answer_permission("s1", PermissionAnswer::Allow);
        let (_, body) = handle.await.unwrap();
        assert_eq!(body, json!({"decision": "allow"}));
    }

    #[tokio::test]
    async fn auto_safe_read_is_allowed_immediately() {
        let (state, _) = test_state();
        let app = router(state.clone());
        call(&app, "/event", &plain("SessionStart")).await;
        assert!(state.set_mode("s1", Mode::AutoSafe));
        let (_, body) = call(
            &app,
            "/permission",
            &hook("PermissionRequest", "Read", json!({"file_path": "a"})),
        )
        .await;
        assert_eq!(body, json!({"decision": "allow"}));
        assert!(!state.pending.has_permission("s1"));
        // No needs-you flash for auto-approved requests.
        assert_eq!(session(&state).state, SessionState::WaitingForReply);

        // An unsafe command still waits for the user.
        let handle =
            hold_permission(&state, &app, "Bash", json!({"command": "git status; rm x"})).await;
        state.answer_permission("s1", PermissionAnswer::Deny);
        assert_eq!(handle.await.unwrap().1, json!({"decision": "deny"}));
    }

    #[tokio::test]
    async fn hands_off_and_snooze_give_none_immediately() {
        let (state, _) = test_state();
        let app = router(state.clone());
        call(&app, "/event", &plain("SessionStart")).await;
        let write = hook("PermissionRequest", "Write", json!({"file_path": "a"}));

        state.set_mode("s1", Mode::HandsOff);
        let (_, body) = call(&app, "/permission", &write).await;
        assert_eq!(body, json!({"decision": "none"}));
        assert!(session(&state).pending_permission.is_none());
        // The terminal asks, so the card still says needs-you.
        assert_eq!(session(&state).state, SessionState::NeedsYou);

        state.set_mode("s1", Mode::Snoozed { until: 5_000 });
        let (_, body) = call(&app, "/permission", &write).await;
        assert_eq!(body, json!({"decision": "none"}));

        // Expired snooze (clock says 1_000, until is in the past) = ask me.
        state.set_mode("s1", Mode::Snoozed { until: 500 });
        let handle = hold_permission(&state, &app, "Write", json!({"file_path": "a"})).await;
        state.answer_permission("s1", PermissionAnswer::Allow);
        assert_eq!(handle.await.unwrap().1, json!({"decision": "allow"}));
    }

    #[tokio::test]
    async fn stop_then_send_reply_returns_text() {
        let (state, _) = test_state();
        let app = router(state.clone());
        let app2 = app.clone();
        let handle = tokio::spawn(async move { call(&app2, "/stop", &plain("Stop")).await });
        let pending = state.pending.clone();
        wait_for("reply held", move || pending.has_reply("s1")).await;
        assert!(session(&state).reply_open);

        assert!(state.send_reply("s1", "line one\nline two"));
        let (status, body) = handle.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"reply": "line one\nline two"}));
        assert!(!session(&state).reply_open);
        assert_eq!(session(&state).state, SessionState::WaitingForReply);
    }

    #[tokio::test]
    async fn stop_then_user_prompt_gives_null() {
        let (state, _) = test_state();
        let app = router(state.clone());
        let app2 = app.clone();
        let handle = tokio::spawn(async move { call(&app2, "/stop", &plain("Stop")).await });
        let pending = state.pending.clone();
        wait_for("reply held", move || pending.has_reply("s1")).await;

        call(&app, "/event", &plain("UserPromptSubmit")).await;
        let (_, body) = handle.await.unwrap();
        assert_eq!(body, json!({"reply": null}));
        assert!(!session(&state).reply_open);
    }

    #[tokio::test]
    async fn stop_dismiss_session_end_and_timeout_give_null() {
        let (state, _) = test_state();
        let app = router(state.clone());

        // Dismiss.
        let app2 = app.clone();
        let handle = tokio::spawn(async move { call(&app2, "/stop", &plain("Stop")).await });
        let pending = state.pending.clone();
        wait_for("reply held", move || pending.has_reply("s1")).await;
        assert!(state.dismiss_reply("s1"));
        assert_eq!(handle.await.unwrap().1, json!({"reply": null}));
        assert!(!state.dismiss_reply("s1"));

        // SessionEnd.
        let app2 = app.clone();
        let handle = tokio::spawn(async move { call(&app2, "/stop", &plain("Stop")).await });
        let pending = state.pending.clone();
        wait_for("reply held", move || pending.has_reply("s1")).await;
        call(&app, "/event", &plain("SessionEnd")).await;
        assert_eq!(handle.await.unwrap().1, json!({"reply": null}));

        // Timeout.
        let (state, _) = state_with(Config {
            reply_max_wait_secs: 0,
            ..Config::default()
        });
        let app = router(state.clone());
        let (_, body) = call(&app, "/stop", &plain("Stop")).await;
        assert_eq!(body, json!({"reply": null}));
        assert!(!session(&state).reply_open);

        // Hands off: no reply box at all.
        let (state, _) = test_state();
        let app = router(state.clone());
        call(&app, "/event", &plain("SessionStart")).await;
        state.set_mode("s1", Mode::HandsOff);
        let (_, body) = call(&app, "/stop", &plain("Stop")).await;
        assert_eq!(body, json!({"reply": null}));
        assert!(!state.pending.has_reply("s1"));
    }

    #[tokio::test]
    async fn new_permission_replaces_the_old_one() {
        let (state, _) = test_state();
        let app = router(state.clone());
        let first = hold_permission(&state, &app, "Write", json!({"file_path": "a"})).await;
        let app2 = app.clone();
        let body = hook("PermissionRequest", "Write", json!({"file_path": "b"}));
        let second = tokio::spawn(async move { call(&app2, "/permission", &body).await });
        // The first one is resolved as none by the replacement.
        assert_eq!(first.await.unwrap().1, json!({"decision": "none"}));
        // The second is still held and its info is still shown.
        wait_for("second held", || {
            session(&state).pending_permission.is_some()
        })
        .await;
        assert!(state.pending.has_permission("s1"));
        assert_eq!(
            session(&state).pending_permission.unwrap().input_preview,
            "b"
        );
        state.answer_permission("s1", PermissionAnswer::Allow);
        assert_eq!(second.await.unwrap().1, json!({"decision": "allow"}));
    }

    #[tokio::test]
    async fn new_routes_require_the_token() {
        let (state, _) = test_state();
        let app = router(state);
        for path in ["/permission", "/stop"] {
            let res = app
                .clone()
                .oneshot(request_to(path, None, plain("Stop")))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{path}");
            let res = app
                .clone()
                .oneshot(request_to(path, Some("Bearer secret-token"), "x".into()))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{path}");
        }
    }

    #[tokio::test]
    async fn client_disconnect_cleans_up_pending_entries() {
        use tokio::io::AsyncWriteExt;
        let (state, _) = test_state();
        let (listener, port) = bind().await.unwrap();
        tokio::spawn(serve(listener, state.clone()));

        for (path, body) in [
            (
                "/permission",
                hook("PermissionRequest", "Write", json!({"file_path": "a"})),
            ),
            ("/stop", plain("Stop")),
        ] {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            let req = format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {TOKEN}\r\n\
                 Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(req.as_bytes()).await.unwrap();
            let is_perm = path == "/permission";
            let st = state.clone();
            wait_for("request held", move || {
                if is_perm {
                    st.pending.has_permission("s1")
                } else {
                    st.pending.has_reply("s1")
                }
            })
            .await;

            // The shim is killed: the connection closes with no answer.
            drop(stream);

            let st = state.clone();
            wait_for("cleanup after disconnect", move || {
                let s = st.store.lock().unwrap().get("s1").unwrap().clone();
                !st.pending.has_permission("s1")
                    && !st.pending.has_reply("s1")
                    && s.pending_permission.is_none()
                    && !s.reply_open
            })
            .await;
        }
    }
}
