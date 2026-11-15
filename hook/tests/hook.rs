//! Integration tests: spawn the real binary against a tiny mock widget server.
//! Ported from the old shim tests (shim/agentwatch-hook.test.ts).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_agentwatch-hook");

// ---------- mock server ----------

#[derive(Clone, Debug, Default)]
struct Seen {
    method: String,
    path: String,
    auth: Option<String>,
    body: String,
}

/// What the mock does with a request.
enum Reply {
    /// Status code and body.
    Json(u16, String),
    /// Accept the connection and never answer.
    Hang,
    /// Wait, then answer.
    Delay(Duration, u16, String),
}

struct Mock {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Mock {
    /// Binds 127.0.0.1:0 (the OS picks a free port) and serves in the background.
    /// `handler` is a boxed closure called for each request.
    fn start(handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let seen2 = seen.clone();
        let handler = Arc::new(handler);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (seen, handler) = (seen2.clone(), handler.clone());
                thread::spawn(move || serve(stream, seen, handler));
            }
        });
        Mock { port, seen }
    }

    fn json(status: u16, body: Value) -> impl Fn(&Seen) -> Reply {
        let text = body.to_string();
        move |_| Reply::Json(status, text.clone())
    }

    fn raw(status: u16, body: &str) -> impl Fn(&Seen) -> Reply {
        let text = body.to_string();
        move |_| Reply::Json(status, text.clone())
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn serve(mut stream: TcpStream, seen: Arc<Mutex<Vec<Seen>>>, handler: Arc<dyn Fn(&Seen) -> Reply>) {
    // Read until the end of the headers.
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("").to_string();
    let path = first.next().unwrap_or("").to_string();
    let mut auth = None;
    let mut len = 0usize;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "authorization" => auth = Some(v.trim().to_string()),
                "content-length" => len = v.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    let mut body = raw[head_end + 4..].to_vec();
    while body.len() < len {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => body.extend_from_slice(&buf[..n]),
        }
    }
    let req = Seen {
        method,
        path,
        auth,
        body: String::from_utf8_lossy(&body).into_owned(),
    };
    seen.lock().unwrap().push(req.clone());
    let (status, text) = match handler(&req) {
        Reply::Json(s, t) => (s, t),
        Reply::Delay(d, s, t) => {
            thread::sleep(d);
            (s, t)
        }
        Reply::Hang => {
            thread::sleep(Duration::from_secs(60));
            return;
        }
    };
    let resp = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    let _ = stream.write_all(resp.as_bytes());
}

// ---------- helpers ----------

fn temp_dir(endpoint: Option<Value>) -> TempDir {
    let dir = TempDir::new().unwrap();
    if let Some(e) = endpoint {
        std::fs::write(dir.path().join("endpoint.json"), e.to_string()).unwrap();
    }
    dir
}

fn endpoint_dir(port: u16, token: &str) -> TempDir {
    temp_dir(Some(json!({ "port": port, "token": token, "pid": 1 })))
}

/// A port that is very likely unused: bind, remember, close.
fn closed_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Out {
    stdout: String,
    stderr: String,
    code: i32,
    ms: u128,
}

fn run(dir: &Path, stdin: &str, env: &[(&str, &str)]) -> Out {
    let mut cmd = Command::new(BIN);
    cmd.env("AGENTWATCH_DIR", dir)
        // Drop the variables the hook reads, so a run inside Claude Code itself
        // does not leak real values into the forwarded body.
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .env_remove("CLAUDE_PID")
        .env_remove("VSCODE_PID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let start = Instant::now();
    let mut child = cmd.spawn().unwrap();
    // Take stdin out of the child so dropping it closes the pipe (EOF).
    let mut pipe = child.stdin.take().unwrap();
    let _ = pipe.write_all(stdin.as_bytes());
    drop(pipe);
    let o = child.wait_with_output().unwrap();
    Out {
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        code: o.status.code().unwrap_or(-1),
        ms: start.elapsed().as_millis(),
    }
}

fn expect_silent_success(o: &Out) {
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "");
    assert_eq!(o.stderr, "");
}

/// Body as the server saw it, with the `agentwatch` meta key removed.
fn strip_meta(body: &str) -> String {
    let mut v: Value = serde_json::from_str(body).unwrap();
    v.as_object_mut().unwrap().remove("agentwatch");
    v.to_string()
}

fn event() -> String {
    json!({ "hook_event_name": "PreToolUse", "session_id": "abc" }).to_string()
}
fn perm() -> String {
    json!({ "hook_event_name": "PermissionRequest", "session_id": "abc",
            "tool_name": "Bash", "tool_input": { "command": "ls" } })
    .to_string()
}
fn stop() -> String {
    json!({ "hook_event_name": "Stop", "session_id": "abc" }).to_string()
}

const ALLOW_JSON: &str = r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}"#;
const DENY_JSON: &str = r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny","message":"Denied from agentwatch"}}}"#;
const SHORT: [(&str, &str); 2] = [
    ("AGENTWATCH_PERMISSION_TIMEOUT_MS", "300"),
    ("AGENTWATCH_REPLY_TIMEOUT_MS", "300"),
];

// ---------- generic route ----------

#[test]
fn no_endpoint_file() {
    expect_silent_success(&run(temp_dir(None).path(), &event(), &[]));
}

#[test]
fn startup_latency_without_endpoint() {
    let dir = temp_dir(None);
    let o = run(dir.path(), &event(), &[]);
    expect_silent_success(&o);
    println!("[timing] no endpoint: {} ms", o.ms);
    assert!(o.ms < 1000, "took {} ms", o.ms);
}

#[test]
fn invalid_endpoint_file() {
    let dir = temp_dir(None);
    std::fs::write(dir.path().join("endpoint.json"), "not json{").unwrap();
    expect_silent_success(&run(dir.path(), &event(), &[]));
}

#[test]
fn endpoint_validation_rejects_bad_values() {
    let mock = Mock::start(Mock::json(200, json!({})));
    let bad = [
        json!({ "port": 0, "token": "t" }),
        json!({ "port": 70000, "token": "t" }),
        json!({ "port": 1.5, "token": "t" }),
        json!({ "port": mock.port.to_string(), "token": "t" }),
        json!({ "port": mock.port, "token": "" }),
        json!({ "port": mock.port }),
        json!([1, 2]),
    ];
    for e in bad {
        expect_silent_success(&run(temp_dir(Some(e)).path(), &event(), &[]));
    }
    assert!(mock.requests().is_empty());
}

#[test]
fn endpoint_points_at_a_closed_port() {
    let dir = endpoint_dir(closed_port(), "t");
    expect_silent_success(&run(dir.path(), &event(), &[]));
}

#[test]
fn wrong_token_gets_401() {
    let mock = Mock::start(Mock::raw(401, "nope"));
    let dir = endpoint_dir(mock.port, "bad");
    expect_silent_success(&run(dir.path(), &event(), &[]));
}

#[test]
fn hung_server_exits_within_the_deadline() {
    let mock = Mock::start(|_| Reply::Hang);
    let dir = endpoint_dir(mock.port, "t");
    let o = run(dir.path(), &event(), &[]);
    expect_silent_success(&o);
    assert!(o.ms < 1200, "took {} ms", o.ms);
}

#[test]
fn stdin_that_never_closes_hits_the_initial_deadline() {
    let dir = endpoint_dir(closed_port(), "t");
    let start = Instant::now();
    let mut child = Command::new(BIN)
        .env("AGENTWATCH_DIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Keep the pipe open and write nothing.
    let _keep_open = child.stdin.take();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(0));
    assert!(start.elapsed() < Duration::from_millis(1500));
}

#[test]
fn invalid_stdin() {
    let mock = Mock::start(Mock::raw(400, "bad"));
    let dir = endpoint_dir(mock.port, "t");
    expect_silent_success(&run(dir.path(), "this is not json", &[]));
    expect_silent_success(&run(dir.path(), "", &[]));
    // Not-JSON is still forwarded as a generic event; empty stdin never is.
    assert_eq!(mock.requests().len(), 1);
}

#[test]
fn oversized_stdin_is_not_sent() {
    let mock = Mock::start(Mock::json(200, json!({})));
    let dir = endpoint_dir(mock.port, "t");
    let big = format!(
        r#"{{"hook_event_name":"PreToolUse","x":"{}"}}"#,
        "a".repeat(1_100_000)
    );
    expect_silent_success(&run(dir.path(), &big, &[]));
    assert!(mock.requests().is_empty());
}

#[test]
fn happy_path_delivers_exact_body_and_bearer_token() {
    let mock = Mock::start(Mock::json(200, json!({})));
    let dir = endpoint_dir(mock.port, "secret-token");
    let body = json!({ "hook_event_name": "PreToolUse", "note": "ünï\n" }).to_string();
    expect_silent_success(&run(dir.path(), &body, &[]));
    let got = mock.requests();
    assert_eq!(got.len(), 1);
    assert_eq!(strip_meta(&got[0].body), body);
    assert_eq!(got[0].auth.as_deref(), Some("Bearer secret-token"));
    assert_eq!(got[0].method, "POST");
    assert_eq!(got[0].path, "/event");
}

#[test]
fn other_events_go_to_event_and_any_response_is_ignored() {
    let mock = Mock::start(Mock::json(
        200,
        json!({ "decision": "allow", "reply": "x" }),
    ));
    let dir = endpoint_dir(mock.port, "t");
    let body = json!({ "hook_event_name": "PreToolUse" }).to_string();
    expect_silent_success(&run(dir.path(), &body, &[]));
    assert_eq!(mock.requests()[0].path, "/event");
}

#[test]
fn invalid_json_stdin_is_a_generic_event() {
    let mock = Mock::start(Mock::raw(200, "{}"));
    let dir = endpoint_dir(mock.port, "t");
    expect_silent_success(&run(dir.path(), "{ nope", &SHORT));
    assert_eq!(mock.requests()[0].path, "/event");
}

// ---------- PermissionRequest ----------

#[test]
fn permission_allow_prints_exact_decision_json() {
    let mock = Mock::start(Mock::json(200, json!({ "decision": "allow" })));
    let dir = endpoint_dir(mock.port, "t");
    let o = run(dir.path(), &perm(), &[]);
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, ALLOW_JSON);
    assert_eq!(o.stderr, "");
}

#[test]
fn permission_deny_prints_exact_decision_json() {
    let mock = Mock::start(Mock::json(200, json!({ "decision": "deny" })));
    let dir = endpoint_dir(mock.port, "t");
    let o = run(dir.path(), &perm(), &[]);
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, DENY_JSON);
    assert_eq!(o.stderr, "");
}

#[test]
fn permission_none_non200_bad_json_and_unknown_print_nothing() {
    let cases: Vec<(u16, String)> = vec![
        (200, json!({ "decision": "none" }).to_string()),
        (500, "nope".to_string()),
        (200, "not json".to_string()),
        (200, json!({ "decision": "maybe" }).to_string()),
    ];
    for (status, body) in cases {
        let mock = Mock::start(move |_| Reply::Json(status, body.clone()));
        let dir = endpoint_dir(mock.port, "t");
        expect_silent_success(&run(dir.path(), &perm(), &[]));
    }
}

#[test]
fn permission_hung_server_exits_silently_at_the_fetch_timeout() {
    let mock = Mock::start(|_| Reply::Hang);
    let dir = endpoint_dir(mock.port, "t");
    let o = run(dir.path(), &perm(), &SHORT);
    expect_silent_success(&o);
    assert!(o.ms >= 250, "returned after {} ms (should wait)", o.ms);
    assert!(o.ms < 2500, "took {} ms", o.ms);
}

#[test]
fn permission_long_poll_is_not_cut_off_by_the_event_deadline() {
    let mock = Mock::start(|_| {
        Reply::Delay(
            Duration::from_millis(900),
            200,
            json!({ "decision": "allow" }).to_string(),
        )
    });
    let dir = endpoint_dir(mock.port, "t");
    let o = run(
        dir.path(),
        &perm(),
        &[("AGENTWATCH_PERMISSION_TIMEOUT_MS", "5000")],
    );
    assert_eq!(o.stdout, ALLOW_JSON);
    assert_eq!(o.code, 0);
}

#[test]
fn permission_closed_port_and_missing_endpoint_are_silent_and_quick() {
    let a = run(endpoint_dir(closed_port(), "t").path(), &perm(), &[]);
    expect_silent_success(&a);
    assert!(a.ms < 1500);
    let b = run(temp_dir(None).path(), &perm(), &[]);
    expect_silent_success(&b);
    assert!(b.ms < 1500);
}

#[test]
fn permission_forwards_body_token_method_and_path() {
    let mock = Mock::start(Mock::json(200, json!({ "decision": "none" })));
    let dir = endpoint_dir(mock.port, "tok-p");
    expect_silent_success(&run(dir.path(), &perm(), &[]));
    let got = &mock.requests()[0];
    assert_eq!(strip_meta(&got.body), perm());
    assert_eq!(got.auth.as_deref(), Some("Bearer tok-p"));
    assert_eq!(got.method, "POST");
    assert_eq!(got.path, "/permission");
}

// ---------- Stop ----------

#[test]
fn stop_reply_exits_2_with_exact_stderr_and_empty_stdout() {
    let mock = Mock::start(Mock::json(200, json!({ "reply": "line one\nline two" })));
    let dir = endpoint_dir(mock.port, "t");
    let o = run(dir.path(), &stop(), &[]);
    assert_eq!(o.code, 2);
    assert_eq!(o.stdout, "");
    assert_eq!(
        o.stderr,
        "The user replied from the agentwatch widget:\nline one\nline two"
    );
}

#[test]
fn stop_null_empty_and_malformed_replies_exit_0_silently() {
    let cases: Vec<(u16, String)> = vec![
        (200, json!({ "reply": null }).to_string()),
        (200, json!({ "reply": "" }).to_string()),
        (200, json!({ "reply": "  \n" }).to_string()),
        (200, "{}".to_string()),
        (200, "bad json".to_string()),
        (500, "err".to_string()),
    ];
    for (status, body) in cases {
        let mock = Mock::start(move |_| Reply::Json(status, body.clone()));
        let dir = endpoint_dir(mock.port, "t");
        expect_silent_success(&run(dir.path(), &stop(), &[]));
    }
}

#[test]
fn stop_hung_server_exits_silently_at_the_fetch_timeout() {
    let mock = Mock::start(|_| Reply::Hang);
    let dir = endpoint_dir(mock.port, "t");
    let o = run(dir.path(), &stop(), &SHORT);
    expect_silent_success(&o);
    assert!(o.ms >= 250, "returned after {} ms (should wait)", o.ms);
    assert!(o.ms < 2500, "took {} ms", o.ms);
}

#[test]
fn stop_closed_port_and_missing_endpoint_are_silent_and_quick() {
    let a = run(endpoint_dir(closed_port(), "t").path(), &stop(), &[]);
    expect_silent_success(&a);
    assert!(a.ms < 1500);
    let b = run(temp_dir(None).path(), &stop(), &[]);
    expect_silent_success(&b);
    assert!(b.ms < 1500);
}

#[test]
fn stop_forwards_body_token_method_and_path() {
    let mock = Mock::start(Mock::json(200, json!({ "reply": null })));
    let dir = endpoint_dir(mock.port, "tok-s");
    expect_silent_success(&run(dir.path(), &stop(), &[]));
    let got = &mock.requests()[0];
    assert_eq!(strip_meta(&got.body), stop());
    assert_eq!(got.auth.as_deref(), Some("Bearer tok-s"));
    assert_eq!(got.method, "POST");
    assert_eq!(got.path, "/stop");
}

// ---------- agentwatch meta ----------

fn body_for(stdin: &str, env: &[(&str, &str)]) -> String {
    let mock = Mock::start(Mock::raw(200, "{}"));
    let dir = endpoint_dir(mock.port, "t");
    expect_silent_success(&run(dir.path(), stdin, env));
    let reqs = mock.requests();
    reqs[0].body.clone()
}

#[test]
fn meta_env_vars_are_forwarded_under_the_agentwatch_key() {
    let body = body_for(
        &event(),
        &[
            ("CLAUDE_CODE_ENTRYPOINT", "claude-vscode"),
            ("CLAUDE_PID", "1234"),
            ("VSCODE_PID", "5678"),
        ],
    );
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["hook_event_name"], "PreToolUse");
    assert_eq!(v["session_id"], "abc");
    assert_eq!(
        v["agentwatch"],
        json!({ "entrypoint": "claude-vscode", "claude_pid": 1234, "vscode_pid": 5678 })
    );
    // The meta key is added last, after the original keys.
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["hook_event_name", "session_id", "agentwatch"]);
}

#[test]
fn meta_unset_or_invalid_env_vars_become_null() {
    let body = body_for(&event(), &[("CLAUDE_PID", "abc")]);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        v["agentwatch"],
        json!({ "entrypoint": null, "claude_pid": null, "vscode_pid": null })
    );
}

#[test]
fn meta_non_object_bodies_are_forwarded_unchanged() {
    assert_eq!(body_for("[1,2]", &[]), "[1,2]");
    assert_eq!(body_for("not json", &[]), "not json");
}
