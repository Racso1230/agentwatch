//! Transcript watcher. Denying or interrupting a permission prompt in VS Code
//! or the terminal sends NO hook event, so a card would stay "needs you"
//! forever. Claude Code does write a line containing
//! `[Request interrupted by user` to the session's transcript file, so while a
//! card is NeedsYou we watch for new text appended to that file.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::constants::TRANSCRIPT_POLL;
use crate::server::{now_millis, SharedStore};
use crate::sessions::{Millis, SessionSnapshot, SessionState};

const MARKER: &str = "[Request interrupted by user";

/// Read only the bytes after `from_offset`. Returns the new end-of-file
/// offset and whether the marker appears in the new bytes. `io::Result` is
/// `Result<T, io::Error>`; the `?` operator returns early on an error.
pub fn scan_appended(path: &Path, from_offset: u64) -> io::Result<(u64, bool)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len <= from_offset {
        return Ok((len, false));
    }
    file.seek(SeekFrom::Start(from_offset))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    // Lossy conversion: a multi-byte character cut in half becomes a
    // replacement char instead of an error. Our marker is plain ASCII.
    let text = String::from_utf8_lossy(&bytes);
    Ok((len, text.contains(MARKER)))
}

/// What we remember per NeedsYou episode: where the file ended when we first
/// saw it. Keyed by (session_id, state_since) so a new prompt = a new entry.
type Key = (String, Millis);

/// One poll over a snapshot. Updates `offsets`, and returns the ids of
/// sessions whose prompt was interrupted.
pub fn poll(sessions: &[SessionSnapshot], offsets: &mut HashMap<Key, u64>) -> Vec<String> {
    let mut interrupted = Vec::new();
    let mut live: Vec<Key> = Vec::new();

    for s in sessions
        .iter()
        .filter(|s| s.state == SessionState::NeedsYou)
    {
        let Some(path) = s.transcript_path.as_deref() else {
            continue;
        };
        let key = (s.session_id.clone(), s.state_since);
        live.push(key.clone());
        let path = Path::new(path);

        match offsets.get(&key).copied() {
            None => {
                // First sighting: remember the current end of the file.
                // A missing file is ignored and retried on the next poll.
                if let Ok(meta) = std::fs::metadata(path) {
                    offsets.insert(key, meta.len());
                }
            }
            Some(offset) => {
                if let Ok((new_offset, found)) = scan_appended(path, offset) {
                    if found {
                        interrupted.push(s.session_id.clone());
                    }
                    offsets.insert(key, new_offset);
                }
            }
        }
    }
    // Forget sessions that are no longer NeedsYou.
    offsets.retain(|k, _| live.contains(k));
    interrupted
}

/// Runs forever, polling every `TRANSCRIPT_POLL`.
pub async fn run<F>(store: SharedStore, emit: F)
where
    F: Fn(Vec<SessionSnapshot>),
{
    let mut offsets: HashMap<Key, u64> = HashMap::new();
    let mut interval = tokio::time::interval(TRANSCRIPT_POLL);
    loop {
        interval.tick().await;
        // Copy the snapshot out so file I/O happens without holding the lock.
        let sessions = store.lock().unwrap_or_else(|e| e.into_inner()).snapshot();
        let interrupted = poll(&sessions, &mut offsets);
        if interrupted.is_empty() {
            continue;
        }
        let snapshot = {
            let mut guard = store.lock().unwrap_or_else(|e| e.into_inner());
            let now = now_millis();
            let mut changed = false;
            for id in &interrupted {
                changed |= guard.mark_interrupted(id, now);
            }
            changed.then(|| guard.snapshot())
        };
        if let Some(snapshot) = snapshot {
            emit(snapshot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::{HookEvent, SessionStore};
    use std::io::Write;

    fn append(path: &Path, text: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn normal_lines_are_not_an_interrupt() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        append(&p, "{\"a\":1}\n");
        let (off, _) = scan_appended(&p, 0).unwrap();
        append(&p, "{\"type\":\"assistant\"}\n");
        let (new_off, found) = scan_appended(&p, off).unwrap();
        assert!(!found);
        assert!(new_off > off);
    }

    #[test]
    fn interrupt_line_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        append(&p, "{\"a\":1}\n");
        let (off, _) = scan_appended(&p, 0).unwrap();
        append(
            &p,
            "{\"text\":\"[Request interrupted by user for tool use]\"}\n",
        );
        assert!(scan_appended(&p, off).unwrap().1);
    }

    #[test]
    fn interrupt_before_the_offset_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        append(&p, "[Request interrupted by user]\n");
        let off = std::fs::metadata(&p).unwrap().len();
        assert!(!scan_appended(&p, off).unwrap().1);
    }

    #[test]
    fn missing_file_is_an_error_not_a_panic() {
        assert!(scan_appended(Path::new("/no/such/file.jsonl"), 0).is_err());
    }

    #[test]
    fn poll_detects_interrupt_and_forgets_finished_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        append(&p, "[Request interrupted by user]\n"); // old, before the prompt

        let mut store = SessionStore::new();
        store.apply(
            &HookEvent {
                hook_event_name: "PermissionRequest".into(),
                session_id: Some("s1".into()),
                transcript_path: Some(p.to_string_lossy().into_owned()),
                tool_name: Some("Write".into()),
                ..Default::default()
            },
            1000,
        );
        let mut offsets = HashMap::new();

        assert!(poll(&store.snapshot(), &mut offsets).is_empty()); // records offset
        assert_eq!(offsets.len(), 1);
        append(&p, "[Request interrupted by user for tool use]\n");
        assert_eq!(poll(&store.snapshot(), &mut offsets), vec!["s1"]);

        assert!(store.mark_interrupted("s1", 2000));
        assert!(poll(&store.snapshot(), &mut offsets).is_empty());
        assert!(offsets.is_empty());
    }
}
