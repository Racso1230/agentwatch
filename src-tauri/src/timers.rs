//! Background sweep: every few seconds ask the store to move quiet sessions
//! to idle/stale and drop finished ones.

use crate::constants::SWEEP_INTERVAL;
use crate::server::{now_millis, SharedStore};
use crate::sessions::{Millis, SessionSnapshot};

/// Is a process with this id still running? Uses `sysinfo`, which works on
/// Windows, Linux and macOS. Refreshing just one pid keeps it cheap.
pub fn process_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(pid).is_some()
}

/// One sweep. Returns the new snapshot only if something changed. `now` is
/// passed in (not read from the clock) so tests can use fixed times.
pub fn tick(store: &SharedStore, now: Millis) -> Option<Vec<SessionSnapshot>> {
    // The lock is released at the end of this function, before the caller
    // emits anything to the frontend.
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    if store.sweep_with(now, &process_alive) {
        Some(store.snapshot())
    } else {
        None
    }
}

/// Runs forever. `async fn` + `.await` lets one thread juggle many waiting
/// tasks; `interval.tick().await` sleeps until the next beat without
/// blocking a thread. `emit` is called with each changed snapshot.
pub async fn run<F>(store: SharedStore, emit: F)
where
    F: Fn(Vec<SessionSnapshot>),
{
    let mut interval = tokio::time::interval(SWEEP_INTERVAL);
    loop {
        interval.tick().await;
        if let Some(snapshot) = tick(&store, now_millis()) {
            emit(snapshot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::{HookEvent, SessionState, SessionStore};
    use std::sync::{Arc, Mutex};

    fn store_with_waiting_session(at: Millis) -> SharedStore {
        let mut s = SessionStore::new();
        let ev = HookEvent {
            hook_event_name: "SessionStart".into(),
            session_id: Some("a".into()),
            cwd: Some("/x/proj".into()),
            ..Default::default()
        };
        s.apply(&ev, at);
        Arc::new(Mutex::new(s))
    }

    #[test]
    fn process_alive_sees_this_process_but_not_a_huge_pid() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(u32::MAX - 1));
    }

    #[test]
    fn nothing_changes_before_the_idle_threshold() {
        let store = store_with_waiting_session(0);
        assert!(tick(&store, 60_000).is_none());
    }

    #[test]
    fn becomes_idle_then_stale_and_is_reported_once() {
        let store = store_with_waiting_session(0);
        let snap = tick(&store, 5 * 60_000).expect("idle after 5 min");
        assert_eq!(snap[0].state, SessionState::Idle);
        assert!(tick(&store, 5 * 60_000 + 5_000).is_none());
        let snap = tick(&store, 30 * 60_000).expect("stale after 30 min");
        assert_eq!(snap[0].state, SessionState::Stale);
    }
}
