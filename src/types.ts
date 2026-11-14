// Mirrors the Rust `Session` struct in src-tauri/src/sessions.rs (serde output).
export type SessionState =
  | "working"
  | "needs_you"
  | "waiting_for_reply"
  | "idle"
  | "stale"
  | "ended"
  | "error";

/** Per-session widget behaviour. Mirrors Rust `policy::Mode` (tag = "kind"). */
export type Mode =
  | { kind: "ask_me" }
  | { kind: "auto_safe" }
  | { kind: "hands_off" }
  /** `until` is Unix milliseconds; afterwards the mode returns to ask_me. */
  | { kind: "snoozed"; until: number };

/** A permission prompt the widget is holding open (Rust `PermissionInfo`). */
export interface PendingPermission {
  tool_name: string;
  /** Command, file path, or first lines of a Write; max ~300 chars. */
  input_preview: string;
  /** Unix milliseconds when the widget stops waiting (countdown target). */
  expires_at: number;
}

export type PermissionDecision = "allow" | "deny" | "none";

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
  mode: Mode;
  pending_permission: PendingPermission | null;
  /** True while the widget should show a reply box for this session. */
  reply_open: boolean;
}
