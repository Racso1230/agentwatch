import { expect, test } from "bun:test";
import {
  formatCountdown,
  isQuestionTool,
  modeLabel,
  replyKeyAction,
  snoozeMode,
} from "./respond";

test("formatCountdown", () => {
  expect(formatCountdown(102_000)).toBe("1:42");
  expect(formatCountdown(5_000)).toBe("0:05");
  expect(formatCountdown(5_900)).toBe("0:05");
  expect(formatCountdown(0)).toBe("0:00");
  expect(formatCountdown(-4000)).toBe("0:00");
  expect(formatCountdown(Number.NaN)).toBe("0:00");
  expect(formatCountdown(120_000)).toBe("2:00");
});

test("modeLabel", () => {
  expect(modeLabel({ kind: "ask_me" }, 0)).toBeNull();
  expect(modeLabel({ kind: "auto_safe" }, 0)).toBe("auto");
  expect(modeLabel({ kind: "hands_off" }, 0)).toBe("hands off");
  expect(modeLabel({ kind: "snoozed", until: 12 * 60_000 }, 0)).toBe(
    "snoozed 12m",
  );
  expect(modeLabel({ kind: "snoozed", until: 30_000 }, 0)).toBe("snoozed 1m");
  expect(modeLabel({ kind: "snoozed", until: 100 }, 100)).toBeNull();
});

test("snoozeMode", () => {
  expect(snoozeMode(15, 1000)).toEqual({ kind: "snoozed", until: 901_000 });
});

test("isQuestionTool", () => {
  expect(isQuestionTool("AskUserQuestion")).toBe(true);
  expect(isQuestionTool("ExitPlanMode")).toBe(true);
  expect(isQuestionTool("Bash")).toBe(false);
});

test("replyKeyAction", () => {
  const k = (key: string, shiftKey = false, isComposing = false) =>
    replyKeyAction({ key, shiftKey, isComposing });
  expect(k("Enter")).toBe("send");
  expect(k("Enter", true)).toBe("newline");
  expect(k("Enter", false, true)).toBe("none");
  expect(k("a")).toBe("none");
});
