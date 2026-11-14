import { useLayoutEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { dismissReply, sendReply } from "../lib/api";
import { replyKeyAction } from "../lib/respond";

const MAX_HEIGHT = 5 * 17 + 10; // ~5 lines of text plus padding

/** Multi-line reply box. Enter sends, Shift+Enter adds a line. */
export function ReplyBox({ sessionId }: { sessionId: string }) {
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLTextAreaElement>(null);

  // Auto-grow: reset to auto so it can shrink, then fit the content.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT)}px`;
  }, [text]);

  function send() {
    const body = text.trim();
    if (!body || sending) return;
    setSending(true);
    setError(null);
    sendReply(sessionId, body)
      .then(() => setText(""))
      .catch((e: unknown) => setError(String(e)))
      .finally(() => setSending(false));
  }

  function dismiss() {
    if (sending) return;
    setSending(true);
    dismissReply(sessionId)
      .catch((e: unknown) => setError(String(e)))
      .finally(() => setSending(false));
  }

  function onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    const action = replyKeyAction({
      key: e.key,
      shiftKey: e.shiftKey,
      isComposing: e.nativeEvent.isComposing,
    });
    if (action === "send") {
      e.preventDefault();
      send();
    }
    // "newline" falls through to the textarea's default behaviour.
  }

  return (
    <div className="reply">
      <textarea
        ref={ref}
        className="reply-input"
        rows={1}
        value={text}
        placeholder="Reply to Claude…"
        aria-label="Reply to Claude"
        disabled={sending}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={onKeyDown}
      />
      <div className="reply-actions">
        <button
          type="button"
          className="btn btn-allow"
          aria-label="Send reply"
          disabled={sending || text.trim() === ""}
          onClick={send}
        >
          Send
        </button>
        <button
          type="button"
          className="btn btn-subtle"
          aria-label="Dismiss, no reply"
          disabled={sending}
          onClick={dismiss}
        >
          No reply
        </button>
        {error && (
          <span className="reply-error" role="alert">
            {error}
          </span>
        )}
      </div>
    </div>
  );
}
