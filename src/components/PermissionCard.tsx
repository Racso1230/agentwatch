import { useState } from "react";
import type { KeyboardEvent } from "react";
import type { PendingPermission, PermissionDecision } from "../types";
import { answerPermission } from "../lib/api";
import { formatCountdown, isQuestionTool } from "../lib/respond";

interface Props {
  sessionId: string;
  permission: PendingPermission;
  now: number;
  /** Owned agents have no terminal to fall back to: hide "Let Claude ask". */
  owned?: boolean;
}

/**
 * Expanded "needs you" card for a held permission request. The parent keys it
 * by `expires_at`, so a fresh request remounts it and re-enables the buttons.
 */
export function PermissionCard({
  sessionId,
  permission,
  now,
  owned = false,
}: Props) {
  const [busy, setBusy] = useState(false);
  const question = isQuestionTool(permission.tool_name);

  function answer(decision: PermissionDecision) {
    if (busy) return;
    setBusy(true);
    // Stay disabled until the next snapshot removes the card; re-enable on error.
    answerPermission(sessionId, decision).catch(() => setBusy(false));
  }

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    if (question || e.target !== e.currentTarget) return;
    if (e.key === "a" || e.key === "A") answer("allow");
    else if (e.key === "d" || e.key === "D") answer("deny");
  }

  return (
    <div
      className="perm"
      tabIndex={0}
      role="group"
      aria-label={`Permission request: ${permission.tool_name}`}
      onKeyDown={onKeyDown}
    >
      <div className="perm-head">
        <span className="perm-tool">{permission.tool_name}</span>
        <span
          className="perm-timer"
          aria-live="off"
          title="Time left to answer"
        >
          {formatCountdown(permission.expires_at - now)}
        </span>
      </div>
      {question ? (
        <p className="perm-note">
          {owned
            ? "Questions cannot be answered in headless mode"
            : "Answer in Claude"}
        </p>
      ) : (
        <pre className="perm-preview">{permission.input_preview}</pre>
      )}
      <div className="perm-actions">
        {!question && (
          <>
            <button
              type="button"
              className="btn btn-allow"
              aria-label="Approve this request"
              disabled={busy}
              onClick={() => answer("allow")}
            >
              Approve
            </button>
            <button
              type="button"
              className="btn btn-deny"
              aria-label="Deny this request"
              disabled={busy}
              onClick={() => answer("deny")}
            >
              Deny
            </button>
          </>
        )}
        {owned && !question && (
          <span className="perm-denied-note">Unanswered = denied</span>
        )}
        {!owned && (
          <button
            type="button"
            className="btn btn-subtle"
            aria-label="Let Claude ask in the terminal"
            disabled={busy}
            onClick={() => answer("none")}
          >
            Let Claude ask
          </button>
        )}
      </div>
    </div>
  );
}
