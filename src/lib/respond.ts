import type { Mode } from "../types";

/** "1:42", "0:05". Floors at zero; NaN counts as zero. */
export function formatCountdown(ms: number): string {
  const total = Math.max(0, Math.floor((Number.isFinite(ms) ? ms : 0) / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/** Badge text for a non-default mode; null for ask_me (or an expired snooze). */
export function modeLabel(mode: Mode, now: number): string | null {
  switch (mode.kind) {
    case "ask_me":
      return null;
    case "auto_safe":
      return "auto";
    case "hands_off":
      return "hands off";
    case "snoozed": {
      const left = mode.until - now;
      if (left <= 0) return null;
      return `snoozed ${Math.ceil(left / 60_000)}m`;
    }
  }
}

/** Build a snooze mode that ends `minutes` from `now` (Unix ms). */
export function snoozeMode(minutes: number, now: number): Mode {
  return { kind: "snoozed", until: now + minutes * 60_000 };
}

/** Tools the terminal must answer: the widget can't approve these. */
export function isQuestionTool(toolName: string): boolean {
  return toolName === "AskUserQuestion" || toolName === "ExitPlanMode";
}

/** What Enter in the reply box should do. */
export function replyKeyAction(e: {
  key: string;
  shiftKey: boolean;
  isComposing: boolean;
}): "send" | "newline" | "none" {
  if (e.key !== "Enter" || e.isComposing) return "none";
  return e.shiftKey ? "newline" : "send";
}
