import type { HooksStatus } from "../types";

export type HooksState = "unknown" | "connected" | "disconnected" | "stale";

/** Collapses a status (or null while loading) into one of four UI states. */
export function hooksState(status: HooksStatus | null): HooksState {
  if (!status) return "unknown";
  if (status.stale) return "stale";
  return status.connected ? "connected" : "disconnected";
}

export function hooksLabel(state: HooksState): string {
  switch (state) {
    case "connected":
      return "Connected";
    case "stale":
      return "Needs update";
    case "disconnected":
      return "Not connected";
    default:
      return "Checking…";
  }
}

/** Explanation under the label; null when none is needed. */
export function hooksDetail(state: HooksState): string | null {
  return state === "stale"
    ? "Hooks point to an old location; Reconnect to fix."
    : null;
}

/** Text of the primary button: Reconnect when stale, otherwise Connect. */
export function connectButtonLabel(state: HooksState): string {
  return state === "stale" ? "Reconnect" : "Connect";
}

/** Disconnect only makes sense when there is something to remove. */
export function canDisconnect(state: HooksState): boolean {
  return state === "connected" || state === "stale";
}

/** The consent text shown before Connect. */
export function connectConsent(settingsPath: string): string {
  return `agentwatch will add hooks to ${settingsPath} so it can see your sessions. A backup is saved first. Disconnect removes them.`;
}

/** Whether the empty widget should nudge the user towards Settings. */
export function showConnectHint(status: HooksStatus | null): boolean {
  const s = hooksState(status);
  return s === "disconnected" || s === "stale";
}

export const CONNECT_HINT =
  "Not connected to Claude Code — open Settings to connect.";
export const STALE_HINT = "Claude Code hooks need an update — open Settings.";

export function hintText(status: HooksStatus | null): string {
  return hooksState(status) === "stale" ? STALE_HINT : CONNECT_HINT;
}
