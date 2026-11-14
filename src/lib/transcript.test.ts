import { describe, expect, test } from "bun:test";
import type { TranscriptItem } from "../types";
import {
  appendTranscript,
  formatCost,
  nearBottom,
  prettyJson,
  resultsById,
  splitCodeFences,
  splitInlineCode,
  turnLabel,
} from "./transcript";

const u = (text: string): TranscriptItem => ({ kind: "user", text });

describe("appendTranscript", () => {
  test("appends at the end", () => {
    const r = appendTranscript(
      [u("a")],
      { session_id: "s", from_index: 1, items: [u("b")] },
      "s",
    );
    expect(r).toEqual([u("a"), u("b")]);
  });
  test("ignores other sessions", () => {
    const items = [u("a")];
    expect(
      appendTranscript(
        items,
        { session_id: "x", from_index: 1, items: [u("b")] },
        "s",
      ),
    ).toBe(items);
  });
  test("overlap does not duplicate", () => {
    const r = appendTranscript(
      [u("a"), u("b")],
      { session_id: "s", from_index: 1, items: [u("b"), u("c")] },
      "s",
    );
    expect(r).toEqual([u("a"), u("b"), u("c")]);
  });
  test("gap returns null", () => {
    expect(
      appendTranscript(
        [],
        { session_id: "s", from_index: 3, items: [u("x")] },
        "s",
      ),
    ).toBeNull();
  });
});

describe("turnLabel / formatCost", () => {
  test("done", () => {
    expect(
      turnLabel({
        kind: "turn_result",
        subtype: "success",
        cost_usd: 0.424,
        duration_ms: 7800,
      }),
    ).toBe("turn done · $0.42 total · 7.8 s");
  });
  test("interrupted", () => {
    expect(
      turnLabel({
        kind: "turn_result",
        subtype: "error_during_execution",
        cost_usd: 1,
        duration_ms: 1,
      }),
    ).toBe("interrupted");
  });
  test("failed", () => {
    expect(
      turnLabel({
        kind: "turn_result",
        subtype: "error_max_turns",
        cost_usd: 0,
        duration_ms: 1000,
      }),
    ).toContain("turn failed");
  });
  test("cost", () => {
    expect(formatCost(0)).toBe("$0.00");
    expect(formatCost(0.004)).toBe("<$0.01");
    expect(formatCost(12.345)).toBe("$12.35");
    expect(formatCost(NaN)).toBe("$0.00");
  });
});

describe("splitCodeFences", () => {
  test("plain text", () => {
    expect(splitCodeFences("hi\nthere")).toEqual([
      { type: "text", text: "hi\nthere" },
    ]);
  });
  test("fence in the middle", () => {
    expect(splitCodeFences("a\n```ts\nlet x\n```\nb")).toEqual([
      { type: "text", text: "a" },
      { type: "code", text: "let x", lang: "ts" },
      { type: "text", text: "b" },
    ]);
  });
  test("unclosed fence runs to the end", () => {
    expect(splitCodeFences("```\nabc")).toEqual([
      { type: "code", text: "abc", lang: "" },
    ]);
  });
});

describe("misc helpers", () => {
  test("splitInlineCode", () => {
    expect(splitInlineCode("run `ls` now")).toEqual([
      { type: "text", text: "run " },
      { type: "code", text: "ls" },
      { type: "text", text: " now" },
    ]);
  });
  test("resultsById", () => {
    const m = resultsById([
      {
        kind: "tool_result",
        tool_use_id: "t1",
        is_error: true,
        output_preview: "x",
      },
    ]);
    expect(m.get("t1")?.is_error).toBe(true);
  });
  test("prettyJson", () => {
    expect(prettyJson('{"a":1}')).toBe('{\n  "a": 1\n}');
    expect(prettyJson('{"a":')).toBe('{"a":');
  });
  test("nearBottom", () => {
    expect(
      nearBottom({ scrollHeight: 1000, scrollTop: 590, clientHeight: 400 }),
    ).toBe(true);
    expect(
      nearBottom({ scrollHeight: 1000, scrollTop: 0, clientHeight: 400 }),
    ).toBe(false);
  });
});
