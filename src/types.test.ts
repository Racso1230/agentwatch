import { expect, test } from "bun:test";
import type { SessionState } from "./types";

test("session states are the snake_case names Rust serialises", () => {
  const states: SessionState[] = [
    "working",
    "needs_you",
    "waiting_for_reply",
    "idle",
    "stale",
    "ended",
    "error",
  ];
  expect(new Set(states).size).toBe(7);
});
