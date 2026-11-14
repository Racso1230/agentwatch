//! Registry of HTTP requests the widget is deliberately holding open
//! ("long-polling"): a permission prompt waiting for Approve/Deny, or a
//! `/stop` waiting for the user's reply.
//!
//! Each held request owns the receiving half of a `tokio::sync::oneshot`
//! channel. A oneshot channel carries exactly ONE value from a sender to a
//! receiver: the HTTP handler `.await`s the receiver (sleeping, not
//! blocking a thread) and the UI / event code later calls `send` on the
//! matching sender to wake it up. If the sender is dropped without sending,
//! the receiver wakes with an error, which the handler treats as "none".
//!
//! The text shown in the widget (tool, preview, countdown) lives on the
//! `Session` in the store, so every snapshot already contains it; this
//! registry only owns the senders.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::sessions::Millis;

/// The user's choice for a permission prompt. `None` = "no decision": the
/// terminal prompt decides (user clicked "Let Claude ask", timeout, or the
/// session moved on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionAnswer {
    Allow,
    Deny,
    None,
}

/// An enum variant can carry data: `Reply("text")` holds the reply itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyAnswer {
    Reply(String),
    None,
}

/// What the widget shows for a held permission prompt.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PermissionInfo {
    pub tool_name: String,
    /// Short text: the command, the file path (+ first lines of a Write).
    pub input_preview: String,
    /// Unix ms when the widget stops waiting (drives the countdown).
    pub expires_at: Millis,
    /// Which held request this belongs to; internal, not sent to the UI.
    #[serde(skip)]
    pub ticket: u64,
}

/// One held request: an id (to tell old from new) and the sender half.
struct Held<T> {
    id: u64,
    sender: oneshot::Sender<T>,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    permissions: HashMap<String, Held<PermissionAnswer>>,
    replies: HashMap<String, Held<ReplyAnswer>>,
}

/// Cheap to clone (it is an `Arc` inside); all clones share the same maps.
#[derive(Clone, Default)]
pub struct Pending {
    inner: Arc<Mutex<Inner>>,
}

/// Generic helper shared by the permission and reply maps. `T: Clone` lets
/// us hand the "none" value to the request we are replacing.
fn register<T: Clone>(
    map: &mut HashMap<String, Held<T>>,
    next_id: &mut u64,
    session_id: &str,
    none: T,
) -> (u64, oneshot::Receiver<T>) {
    *next_id += 1;
    let id = *next_id;
    let (sender, receiver) = oneshot::channel();
    // `insert` returns the previous value for that key, if any: at most one
    // held request per session, so the old one is resolved as "none".
    if let Some(old) = map.insert(session_id.to_string(), Held { id, sender }) {
        let _ = old.sender.send(none);
    }
    (id, receiver)
}

/// Resolve and remove. Returns `true` if a request was waiting for it.
fn resolve<T>(map: &mut HashMap<String, Held<T>>, session_id: &str, answer: T) -> bool {
    match map.remove(session_id) {
        Some(held) => held.sender.send(answer).is_ok(),
        None => false,
    }
}

fn release<T>(map: &mut HashMap<String, Held<T>>, session_id: &str, id: u64) {
    if map.get(session_id).is_some_and(|h| h.id == id) {
        map.remove(session_id);
    }
}

impl Pending {
    pub fn new() -> Self {
        Self::default()
    }

    /// A poisoned lock still holds valid data; recover it rather than panic.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Start holding a permission request. Returns its ticket and the
    /// receiver the HTTP handler awaits. Replaces (resolves as `None`) any
    /// older one for the same session.
    pub fn register_permission(
        &self,
        session_id: &str,
    ) -> (u64, oneshot::Receiver<PermissionAnswer>) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        register(
            &mut inner.permissions,
            &mut inner.next_id,
            session_id,
            PermissionAnswer::None,
        )
    }

    /// Deliver the user's decision. `false` if nothing was waiting.
    pub fn answer_permission(&self, session_id: &str, answer: PermissionAnswer) -> bool {
        resolve(&mut self.lock().permissions, session_id, answer)
    }

    /// Resolve as "no decision" (the session moved on).
    pub fn cancel_permission(&self, session_id: &str) -> bool {
        self.answer_permission(session_id, PermissionAnswer::None)
    }

    /// Forget a request that has finished (only if `ticket` still owns the slot).
    pub fn release_permission(&self, session_id: &str, ticket: u64) {
        release(&mut self.lock().permissions, session_id, ticket);
    }

    pub fn has_permission(&self, session_id: &str) -> bool {
        self.lock().permissions.contains_key(session_id)
    }

    pub fn register_reply(&self, session_id: &str) -> (u64, oneshot::Receiver<ReplyAnswer>) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        register(
            &mut inner.replies,
            &mut inner.next_id,
            session_id,
            ReplyAnswer::None,
        )
    }

    pub fn answer_reply(&self, session_id: &str, answer: ReplyAnswer) -> bool {
        resolve(&mut self.lock().replies, session_id, answer)
    }

    pub fn cancel_reply(&self, session_id: &str) -> bool {
        self.answer_reply(session_id, ReplyAnswer::None)
    }

    pub fn release_reply(&self, session_id: &str, ticket: u64) {
        release(&mut self.lock().replies, session_id, ticket);
    }

    pub fn has_reply(&self, session_id: &str) -> bool {
        self.lock().replies.contains_key(session_id)
    }
}

/// Max characters of a preview sent to the widget.
const PREVIEW_MAX_CHARS: usize = 300;
/// Lines of a Write's content included after the path.
const PREVIEW_CONTENT_LINES: usize = 5;

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}

/// Short human-readable description of what a tool wants to do.
pub fn input_preview(tool_name: &str, input: Option<&Value>) -> String {
    let get = |key: &str| {
        input
            .and_then(|i| i.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let text = match tool_name {
        "Bash" | "PowerShell" => get("command").unwrap_or_default(),
        "Write" => {
            let path = get("file_path").unwrap_or_default();
            let content = get("content").unwrap_or_default();
            let head: Vec<&str> = content.lines().take(PREVIEW_CONTENT_LINES).collect();
            if head.is_empty() {
                path
            } else {
                format!("{path}\n{}", head.join("\n"))
            }
        }
        "Edit" | "MultiEdit" | "Read" | "NotebookEdit" => get("file_path")
            .or_else(|| get("notebook_path"))
            .unwrap_or_default(),
        // Anything else: the raw input as compact JSON.
        _ => input.map(|i| i.to_string()).unwrap_or_default(),
    };
    truncate(&text, PREVIEW_MAX_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn answer_reaches_the_waiting_receiver() {
        let p = Pending::new();
        let (_, rx) = p.register_permission("s");
        assert!(p.has_permission("s"));
        assert!(p.answer_permission("s", PermissionAnswer::Deny));
        assert_eq!(rx.await.unwrap(), PermissionAnswer::Deny);
        assert!(!p.has_permission("s"));
        // Nothing waiting any more.
        assert!(!p.answer_permission("s", PermissionAnswer::Allow));
    }

    #[tokio::test]
    async fn new_registration_resolves_the_old_one_as_none() {
        let p = Pending::new();
        let (id1, rx1) = p.register_permission("s");
        let (id2, rx2) = p.register_permission("s");
        assert_ne!(id1, id2);
        assert_eq!(rx1.await.unwrap(), PermissionAnswer::None);
        // Releasing the OLD ticket must not remove the new request.
        p.release_permission("s", id1);
        assert!(p.has_permission("s"));
        p.answer_permission("s", PermissionAnswer::Allow);
        assert_eq!(rx2.await.unwrap(), PermissionAnswer::Allow);
    }

    #[tokio::test]
    async fn cancel_and_release() {
        let p = Pending::new();
        let (_, rx) = p.register_permission("s");
        assert!(p.cancel_permission("s"));
        assert_eq!(rx.await.unwrap(), PermissionAnswer::None);
        assert!(!p.cancel_permission("s"));

        let (id, _rx) = p.register_permission("t");
        p.release_permission("t", id);
        assert!(!p.has_permission("t"));
    }

    #[tokio::test]
    async fn sessions_are_independent_and_replies_separate() {
        let p = Pending::new();
        let (_, a) = p.register_permission("a");
        let (_, b) = p.register_permission("b");
        let (_, r) = p.register_reply("a");
        p.answer_permission("b", PermissionAnswer::Allow);
        assert_eq!(b.await.unwrap(), PermissionAnswer::Allow);
        assert!(p.has_permission("a"));
        assert!(p.answer_reply("a", ReplyAnswer::Reply("hi".into())));
        assert_eq!(r.await.unwrap(), ReplyAnswer::Reply("hi".into()));
        assert!(p.has_permission("a"));
        drop(a);
    }

    #[tokio::test]
    async fn reply_replacement_and_cancel() {
        let p = Pending::new();
        let (_, r1) = p.register_reply("s");
        let (_, r2) = p.register_reply("s");
        assert_eq!(r1.await.unwrap(), ReplyAnswer::None);
        assert!(p.cancel_reply("s"));
        assert_eq!(r2.await.unwrap(), ReplyAnswer::None);
        assert!(!p.has_reply("s"));
    }

    #[test]
    fn previews() {
        let bash = json!({"command": "git status"});
        assert_eq!(input_preview("Bash", Some(&bash)), "git status");
        assert_eq!(input_preview("PowerShell", Some(&bash)), "git status");

        let edit = json!({"file_path": "c:\\a\\b.txt", "old_string": "x"});
        assert_eq!(input_preview("Edit", Some(&edit)), "c:\\a\\b.txt");
        assert_eq!(input_preview("Read", Some(&edit)), "c:\\a\\b.txt");

        let write = json!({"file_path": "a.txt", "content": "1\n2\n3\n4\n5\n6\n7"});
        assert_eq!(input_preview("Write", Some(&write)), "a.txt\n1\n2\n3\n4\n5");

        let long = json!({"command": "x".repeat(500)});
        assert_eq!(input_preview("Bash", Some(&long)).chars().count(), 301);

        let mcp = json!({"q": 1});
        assert_eq!(input_preview("mcp__x__y", Some(&mcp)), r#"{"q":1}"#);
        assert_eq!(input_preview("Bash", None), "");
    }
}
