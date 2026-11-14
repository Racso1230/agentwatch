//! All timing and size limits in one place so they are easy to tweak.

use std::time::Duration;

/// `WaitingForReply` for this long (with no events) becomes `Idle`.
pub const IDLE_AFTER: Duration = Duration::from_secs(5 * 60);

/// No events at all for this long turns a card grey ("stale").
pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);

/// A card is removed this long after `SessionEnd`.
pub const DROP_AFTER_END: Duration = Duration::from_secs(10);

/// A stale card is removed after being stale this long.
pub const DROP_AFTER_STALE: Duration = Duration::from_secs(2 * 60 * 60);

/// How often the timer sweep runs (used by the timers module).
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

/// How often the transcript watcher polls a file while a card is `NeedsYou`.
pub const TRANSCRIPT_POLL: Duration = Duration::from_millis(500);

/// Largest request body the HTTP listener accepts (1 MB).
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
