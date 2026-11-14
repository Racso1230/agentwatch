//! Local HTTP listener the hook shim POSTs to. Knows nothing about Tauri:
//! when the session list changes it calls a callback, and `lib.rs` plugs in
//! one that emits a Tauri event.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::net::TcpListener;

use crate::constants::MAX_BODY_BYTES;
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

    // Hold the lock only while mutating, and never across an `.await`.
    let snapshot = {
        // A poisoned mutex (another thread panicked) still holds valid data
        // for us, so recover it instead of panicking too.
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        if store.apply(&event, (state.now)()) {
            Some(store.snapshot())
        } else {
            None
        }
    };
    if let Some(snapshot) = snapshot {
        (state.on_change)(snapshot);
    }
    (StatusCode::OK, Json(json!({})))
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/event", post(post_event))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
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
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let state = AppState {
            store: Arc::new(Mutex::new(SessionStore::new())),
            token: Arc::from(TOKEN),
            on_change: Arc::new(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
            now: || 1_000,
        };
        (state, calls)
    }

    fn request(auth: Option<&str>, body: String) -> Request<Body> {
        let mut builder = Request::post("/event");
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
}
