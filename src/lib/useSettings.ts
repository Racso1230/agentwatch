import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { Settings } from "../types";
import { getSettings, updateSettings } from "./api";

export interface SettingsState {
  settings: Settings | null;
  /** Last save error, shown inline in the settings panel. */
  error: string | null;
  /** Save a changed settings object; the server's reply is the truth. */
  save: (next: Settings) => void;
  /** Counter that increments when the tray asks to open the panel. */
  openRequest: number;
}

/** Loads settings, follows `settings` events and `open-settings` requests. */
export function useSettings(): SettingsState {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [openRequest, setOpenRequest] = useState(0);

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

    getSettings()
      .then((s) => {
        if (!cancelled) setSettings(s);
      })
      .catch(() => {});
    keep(
      listen<Settings>("settings", (e) => {
        setSettings(e.payload);
        setError(null);
      }),
    );
    keep(listen("open-settings", () => setOpenRequest((n) => n + 1)));

    return () => {
      cancelled = true;
      unlisteners.forEach((fn) => fn());
    };
  }, []);

  const save = useCallback((next: Settings) => {
    setError(null);
    updateSettings(next)
      .then(setSettings)
      .catch((e: unknown) => {
        setError(String(e));
        // Re-sync with what the server actually has.
        getSettings()
          .then(setSettings)
          .catch(() => {});
      });
  }, []);

  return { settings, error, save, openRequest };
}
