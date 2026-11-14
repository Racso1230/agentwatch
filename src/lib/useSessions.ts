import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Session } from "../types";

/** Re-render every second so "time in state" and countdowns tick. */
export function useNow(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);
  return now;
}

/** Current sessions: one snapshot at start, then live `sessions` events. */
export function useSessions(): Session[] {
  const [sessions, setSessions] = useState<Session[]>([]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;

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
