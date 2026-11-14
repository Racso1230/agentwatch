import { useEffect, useMemo, useState, type CSSProperties } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Session } from "./types";
import {
  canFocus,
  entrypointLabel,
  focusHint,
  subtitle,
  formatDuration,
  sortSessions,
  stateColor,
  stateLabel,
} from "./lib/sessions";
import { ModeMenu } from "./components/ModeMenu";
import { modeLabel } from "./lib/respond";
import { ReplyBox } from "./components/ReplyBox";
import {
  focusSession,
  openLauncher,
  openTranscript,
  ownedRunningCount,
  quitApp,
} from "./lib/api";
import { SettingsPanel } from "./components/SettingsPanel";
import { useSettings } from "./lib/useSettings";
import { panelAlpha } from "./lib/settings";
import { useNow, useSessions } from "./lib/useSessions";
import { PermissionCard } from "./components/PermissionCard";
import "./App.css";

function SessionRow({ session, now }: { session: Session; now: number }) {
  const color = stateColor(session.state);
  const badge = modeLabel(session.mode, now);
  const sub = subtitle(session);
  const where = entrypointLabel(session.entrypoint);
  const owned = session.owned;
  const clickable = owned || canFocus(session);
  const [focusError, setFocusError] = useState<string | null>(null);

  // Auto-hide the inline focus error after ~3 seconds.
  useEffect(() => {
    if (!focusError) return;
    const id = setTimeout(() => setFocusError(null), 3000);
    return () => clearTimeout(id);
  }, [focusError]);

  function focus() {
    if (!clickable) return;
    // Owned agents have no editor window: open their transcript instead.
    const action = owned
      ? openTranscript(session.session_id)
      : focusSession(session.session_id);
    action.catch((e: unknown) => setFocusError(String(e)));
  }

  return (
    <li className={`row row-${session.state}`}>
      <div className="row-main">
        <div
          className={`row-focus${clickable ? " clickable" : ""}`}
          title={owned ? "Open transcript" : focusHint(session)}
          role={clickable ? "button" : undefined}
          tabIndex={clickable ? 0 : undefined}
          onClick={clickable ? focus : undefined}
          onKeyDown={
            clickable
              ? (e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    focus();
                  }
                }
              : undefined
          }
        >
          <span className="dot" style={{ background: color }} />
          <span className="names">
            <span className="project" title={session.cwd}>
              {session.project}
              {owned ? (
                <span className="where owned-badge">agentwatch</span>
              ) : (
                where && <span className="where">{where}</span>
              )}
            </span>
            {sub && (
              <span className="subtitle" title={sub}>
                {sub}
              </span>
            )}
          </span>
        </div>
        <span className="label" style={{ color }}>
          {stateLabel(session.state)}
        </span>
        {session.muted && (
          <span className="muted-badge" title="Notifications muted">
            🔕
          </span>
        )}
        {badge && <span className="badge">{badge}</span>}
        <span className="time">
          {formatDuration(now - session.state_since)}
        </span>
        <ModeMenu
          sessionId={session.session_id}
          mode={session.mode}
          muted={session.muted}
        />
      </div>
      {focusError && (
        <div className="focus-error" role="alert">
          {focusError}
        </div>
      )}
      {session.reply_open && !owned && (
        <ReplyBox sessionId={session.session_id} />
      )}
      {session.pending_permission ? (
        <PermissionCard
          key={session.pending_permission.expires_at}
          sessionId={session.session_id}
          permission={session.pending_permission}
          now={now}
          owned={owned}
        />
      ) : (
        session.state === "needs_you" && (
          <div className="pending">
            {session.pending_tool ?? "Needs you"}
            <span className="pending-hint"> · answer in Claude</span>
          </div>
        )
      )}
    </li>
  );
}

export default function App() {
  const sessions = useSessions();
  const now = useNow();
  const sorted = useMemo(() => sortSessions(sessions), [sessions]);
  const { settings, error, save, openRequest } = useSettings();
  const [showSettings, setShowSettings] = useState(false);
  const [previewOpacity, setPreviewOpacity] = useState<number | null>(null);

  // Tray "Settings..." asks us to open the panel. Adjusting state during
  // render (instead of in an effect) is React's recommended pattern here.
  const [seenOpen, setSeenOpen] = useState(0);
  if (openRequest !== seenOpen) {
    setSeenOpen(openRequest);
    setShowSettings(true);
  }

  // Quit confirmation: number of owned agents that would stop, or null.
  const [confirmQuit, setConfirmQuit] = useState<number | null>(null);

  function requestClose() {
    ownedRunningCount()
      .then((n) => {
        if (n > 0) setConfirmQuit(n);
        else void getCurrentWindow().close();
      })
      .catch(() => void getCurrentWindow().close());
  }

  // The tray's Quit asks us to confirm when owned agents are running.
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen<number>("confirm-quit", (e) => setConfirmQuit(e.payload))
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const alpha = panelAlpha(previewOpacity ?? settings?.opacity ?? 0.95);

  return (
    <div className="panel" style={{ "--panel-alpha": alpha } as CSSProperties}>
      <header className="header" data-tauri-drag-region>
        <span className="title" data-tauri-drag-region>
          agentwatch
        </span>
        <span className="count" data-tauri-drag-region>
          {sessions.length}
        </span>
        <button
          type="button"
          className="gear"
          aria-label="New agent"
          title="New agent"
          onClick={() => void openLauncher()}
        >
          ＋
        </button>
        <button
          type="button"
          className="gear"
          aria-label="Settings"
          aria-pressed={showSettings}
          title="Settings"
          onClick={() => setShowSettings((v) => !v)}
        >
          ⚙
        </button>
        <button
          type="button"
          className="close"
          aria-label="Close agentwatch"
          onClick={requestClose}
        >
          ×
        </button>
      </header>
      {confirmQuit !== null && (
        <div
          className="quit-confirm"
          role="alertdialog"
          aria-label="Quit agentwatch"
        >
          <p>
            {confirmQuit === 1
              ? "1 agent launched from agentwatch will stop."
              : `${confirmQuit} agents launched from agentwatch will stop.`}{" "}
            Quit anyway?
          </p>
          <div className="perm-actions">
            <button
              type="button"
              className="btn btn-deny"
              onClick={() => void quitApp()}
            >
              Quit
            </button>
            <button
              type="button"
              className="btn btn-subtle"
              onClick={() => setConfirmQuit(null)}
            >
              Cancel
            </button>
          </div>
        </div>
      )}
      {showSettings && settings ? (
        <SettingsPanel
          settings={settings}
          error={error}
          onSave={save}
          onPreviewOpacity={setPreviewOpacity}
          onBack={() => setShowSettings(false)}
        />
      ) : sorted.length === 0 ? (
        <div className="empty">
          <p>No Claude Code sessions yet</p>
          <p className="hint">
            run <code>bun run hooks:install</code>, then start Claude Code
          </p>
        </div>
      ) : (
        <ul className="list">
          {sorted.map((s) => (
            <SessionRow key={s.session_id} session={s} now={now} />
          ))}
        </ul>
      )}
    </div>
  );
}
