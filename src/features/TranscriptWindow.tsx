import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { KeyboardEvent } from "react";
import { listen } from "@tauri-apps/api/event";
import type { SessionRekeyed, TranscriptEvent, TranscriptItem } from "../types";
import {
  endSession,
  getTranscript,
  interruptSession,
  sendMessage,
} from "../lib/api";
import { replyKeyAction } from "../lib/respond";
import { stateColor, stateLabel } from "../lib/sessions";
import {
  appendTranscript,
  formatCost,
  nearBottom,
  prettyJson,
  resultsById,
  splitCodeFences,
  splitInlineCode,
  turnLabel,
  type ToolResult,
} from "../lib/transcript";
import { useNow, useSessions } from "../lib/useSessions";
import { PermissionCard } from "../components/PermissionCard";
import "../App.css";

/**
 * Loads one session's transcript, then follows `transcript` events. The
 * session starts under a temporary "pending-..." id; `session-rekeyed` tells
 * us the real one. If the window opened after that event already fired, the
 * first event with an unknown id is matched against the temp transcript.
 */
function useTranscript(initialId: string): {
  id: string;
  items: TranscriptItem[];
} {
  const [id, setId] = useState(initialId);
  const [items, setItems] = useState<TranscriptItem[]>([]);
  // Refs hold the latest values for use inside event callbacks.
  const idRef = useRef(initialId);
  const itemsRef = useRef<TranscriptItem[]>([]);

  const commit = useCallback((list: TranscriptItem[]) => {
    itemsRef.current = list;
    setItems(list);
  }, []);

  useEffect(() => {
    let cancelled = false;
    const unlisteners: (() => void)[] = [];
    const keep = (p: Promise<() => void>) =>
      p
        .then((fn) => {
          if (cancelled) fn();
          else unlisteners.push(fn);
        })
        .catch(() => {});

    // Reload everything; never replace a longer list with a shorter one
    // (an event may have arrived while the request was in flight).
    const reload = (target: string) =>
      getTranscript(target)
        .then((list) => {
          if (!cancelled && list.length >= itemsRef.current.length) {
            commit(list);
          }
        })
        .catch(() => {});

    keep(
      listen<TranscriptEvent>("transcript", (e) => {
        const ev = e.payload;
        const cur = idRef.current;
        if (ev.session_id === cur) {
          const next = appendTranscript(itemsRef.current, ev, cur);
          if (next === null) void reload(cur);
          else if (next !== itemsRef.current) commit(next);
        } else if (cur.startsWith("pending-")) {
          // Maybe our session under its real id: compare with the temp copy.
          getTranscript(cur)
            .then((full) => {
              if (cancelled || idRef.current !== cur) return;
              const slice = full.slice(
                ev.from_index,
                ev.from_index + ev.items.length,
              );
              if (JSON.stringify(slice) === JSON.stringify(ev.items)) {
                idRef.current = ev.session_id;
                setId(ev.session_id);
                commit(full);
              }
            })
            .catch(() => {});
        }
      }),
    );
    keep(
      listen<SessionRekeyed>("session-rekeyed", (e) => {
        if (e.payload.from === idRef.current) {
          idRef.current = e.payload.to;
          setId(e.payload.to);
        }
      }),
    );
    void reload(initialId);

    return () => {
      cancelled = true;
      unlisteners.forEach((fn) => fn());
    };
  }, [initialId, commit]);

  return { id, items };
}

/** Plain text with `inline code` spans; newlines are kept by CSS. */
function InlineText({ text }: { text: string }) {
  return (
    <>
      {splitInlineCode(text).map((seg, i) =>
        seg.type === "code" ? (
          <code key={i} className="tx-inline">
            {seg.text}
          </code>
        ) : (
          <span key={i}>{seg.text}</span>
        ),
      )}
    </>
  );
}

/** Markdown-lite: fenced code blocks and inline code, nothing else. */
function RichText({ text }: { text: string }) {
  return (
    <>
      {splitCodeFences(text).map((seg, i) =>
        seg.type === "code" ? (
          <pre key={i} className="tx-code">
            {seg.text}
          </pre>
        ) : (
          <div key={i} className="tx-text">
            <InlineText text={seg.text} />
          </div>
        ),
      )}
    </>
  );
}

function ToolLine({
  item,
  result,
}: {
  item: Extract<TranscriptItem, { kind: "tool_use" }>;
  result: ToolResult | undefined;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className="tx-tool">
      <button
        type="button"
        className="tx-tool-head"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <span className="tx-chevron">{open ? "▾" : "▸"}</span>
        <span className="tx-tool-summary">{item.summary}</span>
        {result?.is_error && <span className="tx-tool-err">error</span>}
        {!result && <span className="tx-tool-run">running</span>}
      </button>
      {open && (
        <div className="tx-tool-body">
          <div className="tx-label">Input</div>
          <pre className="tx-code">{prettyJson(item.input_json)}</pre>
          {result && (
            <>
              <div className="tx-label">
                {result.is_error ? "Error" : "Result"}
              </div>
              <pre
                className={`tx-code${result.is_error ? " tx-code-err" : ""}`}
              >
                {result.output_preview || "(empty)"}
              </pre>
            </>
          )}
        </div>
      )}
    </div>
  );
}

function TranscriptList({ items }: { items: TranscriptItem[] }) {
  const results = useMemo(() => resultsById(items), [items]);
  const toolIds = useMemo(() => {
    const ids = new Set<string>();
    for (const it of items) if (it.kind === "tool_use") ids.add(it.id);
    return ids;
  }, [items]);

  return (
    <>
      {items.map((it, i) => {
        switch (it.kind) {
          case "user":
            return (
              <div key={i} className="tx-user">
                <div className="tx-user-bubble">{it.text}</div>
              </div>
            );
          case "assistant_text":
            return (
              <div key={i} className="tx-assistant">
                <RichText text={it.text} />
              </div>
            );
          case "tool_use":
            return <ToolLine key={i} item={it} result={results.get(it.id)} />;
          case "tool_result":
            // Shown inside its tool line; only orphans get their own row.
            return toolIds.has(it.tool_use_id) ? null : (
              <pre
                key={i}
                className={`tx-code${it.is_error ? " tx-code-err" : ""}`}
              >
                {it.output_preview}
              </pre>
            );
          case "turn_result":
            return (
              <div key={i} className="tx-turn" role="separator">
                <span>{turnLabel(it)}</span>
              </div>
            );
          case "notice":
            return (
              <div key={i} className="tx-notice">
                {it.text}
              </div>
            );
        }
      })}
    </>
  );
}

export function TranscriptWindow({ initialId }: { initialId: string }) {
  const { id, items } = useTranscript(initialId);
  const sessions = useSessions();
  const now = useNow();
  const session = sessions.find((s) => s.session_id === id) ?? null;
  const ended = session?.state === "ended";
  const running =
    session?.state === "working" || session?.state === "needs_you";
  const project = session?.project ?? "agent";

  const cost = useMemo(() => {
    for (let i = items.length - 1; i >= 0; i--) {
      const it = items[i];
      if (it?.kind === "turn_result") return it.cost_usd;
    }
    return 0;
  }, [items]);

  useEffect(() => {
    document.title = `${project} - agentwatch`;
  }, [project]);

  // Follow new items unless the user scrolled up.
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickRef = useRef(true);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && stickRef.current) el.scrollTop = el.scrollHeight;
  }, [items]);

  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmEnd, setConfirmEnd] = useState(false);

  function send() {
    const body = text.trim();
    if (!body || sending || ended) return;
    setSending(true);
    setError(null);
    sendMessage(id, body)
      .then(() => {
        setText("");
        stickRef.current = true;
      })
      .catch((e: unknown) => setError(String(e)))
      .finally(() => setSending(false));
  }

  function onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    // Enter sends, Shift+Enter is a newline, and IME composition is ignored.
    const action = replyKeyAction({
      key: e.key,
      shiftKey: e.shiftKey,
      isComposing: e.nativeEvent.isComposing,
    });
    if (action === "send") {
      e.preventDefault();
      send();
    }
  }

  function run(action: Promise<void>) {
    setError(null);
    action.catch((e: unknown) => setError(String(e)));
  }

  return (
    <div className="tx">
      <header className="tx-header">
        <span
          className="dot"
          style={{ background: stateColor(session?.state ?? "idle") }}
        />
        <span className="tx-project" title={session?.cwd}>
          {project}
        </span>
        <span className="tx-state">
          {session ? stateLabel(session.state) : ""}
        </span>
        <span className="tx-cost" title="Running cost for this session">
          {formatCost(cost)}
        </span>
      </header>
      {session?.pending_permission && (
        <div className="tx-banner">
          <PermissionCard
            key={session.pending_permission.expires_at}
            sessionId={session.session_id}
            permission={session.pending_permission}
            now={now}
            owned
          />
        </div>
      )}
      <div
        className="tx-scroll"
        ref={scrollRef}
        onScroll={(e) => {
          stickRef.current = nearBottom(e.currentTarget);
        }}
      >
        <TranscriptList items={items} />
        {items.length === 0 && <div className="tx-empty">Starting...</div>}
      </div>
      <footer className="tx-footer">
        {error && (
          <div className="form-error" role="alert">
            {error}
          </div>
        )}
        <div className="tx-input-row">
          <textarea
            className="text-input tx-input"
            rows={2}
            value={text}
            disabled={ended}
            placeholder={
              ended
                ? "Session ended"
                : "Message (Enter sends, Shift+Enter newline)"
            }
            onChange={(e) => setText(e.target.value)}
            onKeyDown={onKeyDown}
          />
          <div className="tx-buttons">
            <button
              type="button"
              className="btn btn-allow"
              disabled={ended || sending || text.trim() === ""}
              onClick={send}
            >
              Send
            </button>
            {running && (
              <button
                type="button"
                className="btn btn-deny"
                title="Stop the current turn; the session stays open"
                onClick={() => run(interruptSession(id))}
              >
                Stop
              </button>
            )}
            {!ended &&
              (confirmEnd ? (
                <span className="tx-confirm">
                  End session?{" "}
                  <button
                    type="button"
                    className="btn btn-deny"
                    onClick={() => {
                      setConfirmEnd(false);
                      run(endSession(id));
                    }}
                  >
                    End
                  </button>{" "}
                  <button
                    type="button"
                    className="btn btn-subtle"
                    onClick={() => setConfirmEnd(false)}
                  >
                    Cancel
                  </button>
                </span>
              ) : (
                <button
                  type="button"
                  className="btn btn-subtle"
                  title="Close this agent"
                  onClick={() => setConfirmEnd(true)}
                >
                  End
                </button>
              ))}
          </div>
        </div>
      </footer>
    </div>
  );
}
