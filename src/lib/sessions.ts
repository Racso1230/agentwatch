import type { Session, SessionState } from "../types";

/** "12s", "3m", "1h 4m", "2d 3h". Negative or NaN input clamps to "0s". */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.floor((Number.isFinite(ms) ? ms : 0) / 1000));
  if (total < 60) return `${total}s`;
  const minutes = Math.floor(total / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    const m = minutes % 60;
    return m === 0 ? `${hours}h` : `${hours}h ${m}m`;
  }
  const days = Math.floor(hours / 24);
  const h = hours % 24;
  return h === 0 ? `${days}d` : `${days}d ${h}h`;
}

const ORDER: SessionState[] = [
  "needs_you",
  "working",
  "waiting_for_reply",
  "idle",
  "error",
  "stale",
  "ended",
];

/** Group by state priority; stable within a group (snapshot = first-seen order). */
export function sortSessions(sessions: Session[]): Session[] {
  return sessions
    .map((s, i) => ({ s, i }))
    .sort(
      (a, b) =>
        ORDER.indexOf(a.s.state) - ORDER.indexOf(b.s.state) || a.i - b.i,
    )
    .map(({ s }) => s);
}

const LABELS: Record<SessionState, string> = {
  working: "Working",
  needs_you: "Needs you",
  waiting_for_reply: "Waiting for you",
  idle: "Idle",
  stale: "Stale",
  ended: "Ended",
  error: "Error",
};

export function stateLabel(state: SessionState): string {
  return LABELS[state];
}

const COLORS: Record<SessionState, string> = {
  working: "#4f9cff",
  needs_you: "#ffb020",
  waiting_for_reply: "#3ecf6e",
  idle: "#8a8f98",
  stale: "#8a8f98",
  ended: "#8a8f98",
  error: "#ff5d5d",
};

export function stateColor(state: SessionState): string {
  return COLORS[state];
}
