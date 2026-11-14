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
  /** How Claude Code was launched: "claude-vscode", "cli", ... */
  entrypoint: string | null;
  claude_pid: number | null;
  /** First ~5 words of the first prompt (subtitle for same-folder sessions). */
  first_prompt: string | null;
  /** Muted sessions raise no desktop notifications. */
  muted: boolean;
  /** Launched from agentwatch itself; replies go through its own process. */
  owned: boolean;
}

/** User settings (Rust `settings::Settings`). Saved in ~/.agentwatch/settings.json. */
export interface Settings {
  /** Window opacity, 0.4 to 1.0. */
  opacity: number;
  /** Clicks pass through the widget; only the tray menu can turn it off. */
  click_through: boolean;
  notify_needs_you: boolean;
  notify_finished: boolean;
  sound: boolean;
  autostart: boolean;
  snap_to_edges: boolean;
  /** Folders agents were launched in, most recent first (max 8). */
  recent_folders: string[];
}

/** One line of an owned session's transcript (Rust `owned::TranscriptItem`, tag = "kind"). */
export type TranscriptItem =
  | { kind: "user"; text: string }
  | { kind: "assistant_text"; text: string }
  | {
      kind: "tool_use";
      id: string;
      name: string;
      /** One line, e.g. "Bash: git status" or "Edit App.tsx". */
      summary: string;
      /** The tool input as JSON text, cut to about 2 KB. */
      input_json: string;
    }
  | {
      kind: "tool_result";
      tool_use_id: string;
      is_error: boolean;
      /** First ~2 KB of the output. */
      output_preview: string;
    }
  | {
      kind: "turn_result";
      subtype: string;
      /** Running total for the whole session, in US dollars. */
      cost_usd: number;
      duration_ms: number;
    }
  | { kind: "notice"; text: string };

/** Payload of the `transcript` event: items appended at `from_index`. */
export interface TranscriptEvent {
  session_id: string;
  from_index: number;
  items: TranscriptItem[];
}

/** Payload of the `session-rekeyed` event (temporary id became the real one). */
export interface SessionRekeyed {
  from: string;
  to: string;
}
