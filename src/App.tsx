import { useEffect, useMemo, useState, type CSSProperties } from "react";
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
import { focusSession } from "./lib/api";
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
  const clickable = canFocus(session);
  const [focusError, setFocusError] = useState<string | null>(null);

  // Auto-hide the inline focus error after ~3 seconds.
  useEffect(() => {
    if (!focusError) return;
    const id = setTimeout(() => setFocusError(null), 3000);
    return () => clearTimeout(id);
  }, [focusError]);

  function focus() {
    if (!clickable) return;
    focusSession(session.session_id).catch((e: unknown) =>
      setFocusError(String(e)),
    );
  }

  return (
    <li className={`row row-${session.state}`}>
      <div className="row-main">
        <div
          className={`row-focus${clickable ? " clickable" : ""}`}
          title={focusHint(session)}
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
              {where && <span className="where">{where}</span>}
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
      {session.reply_open && <ReplyBox sessionId={session.session_id} />}
      {session.pending_permission ? (
        <PermissionCard
          key={session.pending_permission.expires_at}
          sessionId={session.session_id}
          permission={session.pending_permission}
          now={now}
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
          onClick={() => void getCurrentWindow().close()}
        >
          ×
        </button>
      </header>
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
