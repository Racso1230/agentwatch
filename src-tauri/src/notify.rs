//! Deciding when a desktop notification is worth showing. The decision is a
//! pure function (no Tauri, no clock), so it is easy to unit test; `lib.rs`
//! does the actual showing.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::sessions::{Session, SessionState};
use crate::settings::Settings;

/// What to show in the notification.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

/// Should the change from `prev` to `next` raise a notification?
///
/// `prev` is `None` the first time we see a session. `Option<&Session>` is a
/// borrowed, optional reference: we only read, so no cloning is needed.
pub fn notify_decision(
    prev: Option<&Session>,
    next: &Session,
    settings: &Settings,
    widget_focused: bool,
) -> Option<Notice> {
    if widget_focused || next.muted {
        return None;
    }
    let prev_state = prev.map(|p| p.state);
    match next.state {
        // Into needs_you from anything else.
        SessionState::NeedsYou
            if settings.notify_needs_you && prev_state != Some(SessionState::NeedsYou) =>
        {
            let mut title = format!("{} needs you", next.project);
            if let Some(p) = &next.first_prompt {
                title.push_str(&format!(" · {p}"));
            }
            let body = next
                .pending_tool
                .clone()
                .unwrap_or_else(|| "Answer in Claude".to_string());
            Some(Notice { title, body })
        }
        // A finished turn: working -> waiting for reply. A brand-new card
        // (SessionStart) or an un-greyed stale card is not a finished turn.
        SessionState::WaitingForReply
            if settings.notify_finished && prev_state == Some(SessionState::Working) =>
        {
            Some(Notice {
                title: format!("{} finished", next.project),
                body: next
                    .first_prompt
                    .clone()
                    .unwrap_or_else(|| "Waiting for your reply".to_string()),
            })
        }
        _ => None,
    }
}

/// Remembers the previous snapshot of every session so each new snapshot can
/// be compared against it.
#[derive(Default)]
pub struct Notifier {
    last: Mutex<HashMap<String, Session>>,
}

impl Notifier {
    /// Compare `snapshot` with the previous one and return the notices to
    /// show. Always remembers `snapshot` for next time.
    pub fn notices(
        &self,
        snapshot: &[Session],
        settings: &Settings,
        widget_focused: bool,
    ) -> Vec<Notice> {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let notices = snapshot
            .iter()
            .filter_map(|next| {
                notify_decision(last.get(&next.session_id), next, settings, widget_focused)
            })
            .collect();
        // Replace the map: sessions that disappeared are forgotten too.
        *last = snapshot
            .iter()
            .map(|s| (s.session_id.clone(), s.clone()))
            .collect();
        notices
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::{HookEvent, SessionStore};
    use serde_json::json;

    /// A real `Session` built through the store (its fields are private to
    /// construct directly), then tweaked.
    fn session(state: SessionState) -> Session {
        let mut store = SessionStore::new();
        let ev: HookEvent = serde_json::from_value(json!({
            "hook_event_name": "SessionStart", "session_id": "a", "cwd": "/x/demo"
        }))
        .unwrap();
        store.apply(&ev, 0);
        let mut s = store.snapshot().remove(0);
        s.state = state;
        s
    }

    fn on() -> Settings {
        Settings::default()
    }

    #[test]
    fn needs_you_title_and_body() {
        let prev = session(SessionState::Working);
        let mut next = session(SessionState::NeedsYou);
        let n = notify_decision(Some(&prev), &next, &on(), false).unwrap();
        assert_eq!(n.title, "demo needs you");
        assert_eq!(n.body, "Answer in Claude");

        next.first_prompt = Some("fix the login".into());
        next.pending_tool = Some("Write bye.txt".into());
        let n = notify_decision(Some(&prev), &next, &on(), false).unwrap();
        assert_eq!(n.title, "demo needs you · fix the login");
        assert_eq!(n.body, "Write bye.txt");
    }

    #[test]
    fn needs_you_only_fires_on_the_transition() {
        let prev = session(SessionState::NeedsYou);
        let next = session(SessionState::NeedsYou);
        assert_eq!(notify_decision(Some(&prev), &next, &on(), false), None);
        // A card first seen already waiting on the user does notify.
        assert!(notify_decision(None, &next, &on(), false).is_some());
    }

    #[test]
    fn finished_after_working() {
        let prev = session(SessionState::Working);
        let mut next = session(SessionState::WaitingForReply);
        let n = notify_decision(Some(&prev), &next, &on(), false).unwrap();
        assert_eq!(n.title, "demo finished");
        assert_eq!(n.body, "Waiting for your reply");
        next.first_prompt = Some("add dark mode".into());
        let n = notify_decision(Some(&prev), &next, &on(), false).unwrap();
        assert_eq!(n.body, "add dark mode");
    }

    #[test]
    fn new_or_unstaled_cards_are_not_a_finished_turn() {
        let next = session(SessionState::WaitingForReply);
        assert_eq!(notify_decision(None, &next, &on(), false), None);
        let stale = session(SessionState::Stale);
        assert_eq!(notify_decision(Some(&stale), &next, &on(), false), None);
        let needs = session(SessionState::NeedsYou);
        assert_eq!(notify_decision(Some(&needs), &next, &on(), false), None);
    }

    #[test]
    fn toggles_muted_and_focus_suppress() {
        let working = session(SessionState::Working);
        let needs = session(SessionState::NeedsYou);
        let waiting = session(SessionState::WaitingForReply);

        let no_needs = Settings {
            notify_needs_you: false,
            ..on()
        };
        assert_eq!(
            notify_decision(Some(&working), &needs, &no_needs, false),
            None
        );
        assert!(notify_decision(Some(&working), &waiting, &no_needs, false).is_some());

        let no_finished = Settings {
            notify_finished: false,
            ..on()
        };
        assert_eq!(
            notify_decision(Some(&working), &waiting, &no_finished, false),
            None
        );

        assert_eq!(notify_decision(Some(&working), &needs, &on(), true), None);
        let mut muted = needs.clone();
        muted.muted = true;
        assert_eq!(notify_decision(Some(&working), &muted, &on(), false), None);
    }

    #[test]
    fn notifier_compares_against_the_previous_snapshot() {
        let n = Notifier::default();
        let working = session(SessionState::Working);
        assert!(n.notices(&[working], &on(), false).is_empty());
        let waiting = session(SessionState::WaitingForReply);
        let notices = n.notices(std::slice::from_ref(&waiting), &on(), false);
        assert_eq!(notices.len(), 1);
        // Same snapshot again: no repeat.
        assert!(n.notices(&[waiting], &on(), false).is_empty());
    }
}
