import type { TranscriptEvent, TranscriptItem } from "../types";

export type TurnResult = Extract<TranscriptItem, { kind: "turn_result" }>;
export type ToolResult = Extract<TranscriptItem, { kind: "tool_result" }>;

/**
 * Apply a `transcript` event to the list we hold. Returns the new list, the
 * same list when the event is for another session, or `null` when we missed
 * items (a gap) and should reload the whole transcript.
 */
export function appendTranscript(
  items: TranscriptItem[],
  event: TranscriptEvent,
  trackedId: string,
): TranscriptItem[] | null {
  if (event.session_id !== trackedId) return items;
  if (event.from_index > items.length) return null;
  // from_index <= length: overwrite any overlap so duplicates never appear.
  return items.slice(0, event.from_index).concat(event.items);
}

/** Text of the thin divider after a turn. */
export function turnLabel(item: TurnResult): string {
  if (item.subtype === "error_during_execution") return "interrupted";
  const secs = `${(item.duration_ms / 1000).toFixed(1)} s`;
  const head = item.subtype === "success" ? "turn done" : "turn failed";
  return `${head} · ${formatCost(item.cost_usd)} total · ${secs}`;
}

/** "$0.42"; very small non-zero amounts show as "<$0.01". */
export function formatCost(usd: number): string {
  if (!Number.isFinite(usd) || usd <= 0) return "$0.00";
  if (usd < 0.01) return "<$0.01";
  return `$${usd.toFixed(2)}`;
}

export interface Segment {
  type: "text" | "code";
  text: string;
  /** Language tag after the opening fence, for code segments. */
  lang?: string;
}

/** Split text into plain and fenced-code segments. An unclosed fence runs to the end. */
export function splitCodeFences(text: string): Segment[] {
  const out: Segment[] = [];
  const lines = text.split("\n");
  let current: string[] = [];
  let inCode = false;
  let lang = "";
  const flush = () => {
    if (inCode) {
      out.push({ type: "code", text: current.join("\n"), lang });
    } else if (current.length > 0) {
      out.push({ type: "text", text: current.join("\n") });
    }
    current = [];
  };
  for (const line of lines) {
    const fence = /^\s*```(.*)$/.exec(line);
    if (fence) {
      flush();
      inCode = !inCode;
      lang = inCode ? (fence[1] ?? "").trim() : "";
    } else {
      current.push(line);
    }
  }
  flush();
  return out;
}

/** Split a text segment into plain and `inline code` pieces. */
export function splitInlineCode(text: string): Segment[] {
  const out: Segment[] = [];
  const re = /`([^`\n]+)`/g;
  let last = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > last)
      out.push({ type: "text", text: text.slice(last, m.index) });
    out.push({ type: "code", text: m[1] ?? "" });
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push({ type: "text", text: text.slice(last) });
  return out;
}

/** Map tool_use id to its result, for pairing in the UI. */
export function resultsById(items: TranscriptItem[]): Map<string, ToolResult> {
  const map = new Map<string, ToolResult>();
  for (const it of items) {
    if (it.kind === "tool_result") map.set(it.tool_use_id, it);
  }
  return map;
}

/** Pretty-print stored tool input JSON; falls back to the raw text (it may be cut off). */
export function prettyJson(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

/** Whether the scroll position is close enough to the bottom to follow new items. */
export function nearBottom(
  el: { scrollHeight: number; scrollTop: number; clientHeight: number },
  slack = 40,
): boolean {
  return el.scrollHeight - el.scrollTop - el.clientHeight <= slack;
}
