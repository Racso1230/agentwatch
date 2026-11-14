import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Session } from "./types";

// Placeholder UI: Phase 1 Agent C replaces this with the real session list.
export default function App() {
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

  return (
    <main>
      <h1>agentwatch</h1>
      <pre>{JSON.stringify(sessions, null, 2)}</pre>
    </main>
  );
}
