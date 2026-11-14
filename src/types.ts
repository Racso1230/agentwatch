// Mirrors the Rust `Session` struct in src-tauri/src/sessions.rs (serde output).
export type SessionState =
  | "working"
  | "needs_you"
  | "waiting_for_reply"
  | "idle"
  | "stale"
  | "ended"
  | "error";

export interface Session {
  session_id: string;
  cwd: string;
  project: string;
  state: SessionState;
  /** Unix milliseconds when the current state began. */
  state_since: number;
  /** Unix milliseconds of the last event received. */
  last_event_at: number;
  permission_mode: string | null;
  /** e.g. "Write bye.txt" while the session is in needs_you. */
  pending_tool: string | null;
  transcript_path: string | null;
  /** Unix milliseconds of SessionEnd, if ended. */
  ended_at: number | null;
}
