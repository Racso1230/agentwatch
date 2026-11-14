import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Session } from "./types";
import {
  formatDuration,
  sortSessions,
  stateColor,
  stateLabel,
} from "./lib/sessions";
import { ReplyBox } from "./components/ReplyBox";
import { PermissionCard } from "./components/PermissionCard";
import "./App.css";

/** Re-render every second so "time in state" ticks. */
function useNow(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);
  return now;
}

function useSessions(): Session[] {
  const [sessions, setSessions] = useState<Session[]>([]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;

    // Initial snapshot, then live updates pushed from Rust.
    invoke<Session[]>("get_sessions")
      .then((s) => {
        if (!cancelled) setSessions(s);
      })
      .catch(() => {});
    listen<Session[]>("sessions", (e) => setSessions(e.payload))
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

  return sessions;
}

function SessionRow({ session, now }: { session: Session; now: number }) {
  const color = stateColor(session.state);
  return (
    <li className={`row row-${session.state}`}>
      <div className="row-main">
        <span className="dot" style={{ background: color }} />
        <span className="project" title={session.cwd}>
          {session.project}
        </span>
        <span className="label" style={{ color }}>
          {stateLabel(session.state)}
        </span>
        <span className="time">
          {formatDuration(now - session.state_since)}
        </span>
      </div>
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

  return (
    <div className="panel">
      <header className="header" data-tauri-drag-region>
        <span className="title" data-tauri-drag-region>
          agentwatch
        </span>
        <span className="count" data-tauri-drag-region>
          {sessions.length}
        </span>
        <button
          type="button"
          className="close"
          aria-label="Close agentwatch"
          onClick={() => void getCurrentWindow().close()}
        >
          ×
        </button>
      </header>
      {sorted.length === 0 ? (
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
