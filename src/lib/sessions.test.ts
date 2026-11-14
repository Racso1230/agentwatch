import { expect, test } from "bun:test";
import type { Session, SessionState } from "../types";
import {
  formatDuration,
  sortSessions,
  stateColor,
  stateLabel,
} from "./sessions";

function make(id: string, state: SessionState): Session {
  return {
    session_id: id,
    cwd: "/x",
    project: id,
    state,
    state_since: 0,
    last_event_at: 0,
    permission_mode: null,
    pending_tool: null,
    transcript_path: null,
    ended_at: null,
    mode: { kind: "ask_me" },
    pending_permission: null,
    reply_open: false,
  };
}

test("formatDuration", () => {
  expect(formatDuration(0)).toBe("0s");
  expect(formatDuration(12_400)).toBe("12s");
  expect(formatDuration(59_999)).toBe("59s");
  expect(formatDuration(60_000)).toBe("1m");
  expect(formatDuration(3 * 60_000 + 20_000)).toBe("3m");
  expect(formatDuration(3_600_000)).toBe("1h");
  expect(formatDuration(3_600_000 + 4 * 60_000)).toBe("1h 4m");
  expect(formatDuration(26 * 3_600_000)).toBe("1d 2h");
  expect(formatDuration(-5000)).toBe("0s");
  expect(formatDuration(Number.NaN)).toBe("0s");
});

test("sortSessions orders by state priority, stable within a group", () => {
  const input = [
    make("ended1", "ended"),
    make("idle1", "idle"),
    make("work1", "working"),
    make("stale1", "stale"),
    make("need1", "needs_you"),
    make("work2", "working"),
    make("err1", "error"),
    make("wait1", "waiting_for_reply"),
  ];
  expect(sortSessions(input).map((s) => s.session_id)).toEqual([
    "need1",
    "work1",
    "work2",
    "wait1",
    "idle1",
    "err1",
    "stale1",
    "ended1",
  ]);
  // The input array is not mutated.
  expect(input[0]?.session_id).toBe("ended1");
});

test("labels and colours exist for every state", () => {
  const states: SessionState[] = [
    "working",
    "needs_you",
    "waiting_for_reply",
    "idle",
    "stale",
    "ended",
    "error",
  ];
  for (const s of states) {
    expect(stateLabel(s).length).toBeGreaterThan(0);
    expect(stateColor(s)).toMatch(/^#[0-9a-f]{6}$/);
  }
  expect(stateColor("needs_you")).not.toBe(stateColor("working"));
});
