import { useEffect, useRef, useState } from "react";
import type { Mode } from "../types";
import { setMode, setMuted } from "../lib/api";
import { snoozeMode } from "../lib/respond";

const OPTIONS: { label: string; kind: Mode["kind"] }[] = [
  { label: "Ask me", kind: "ask_me" },
  { label: "Auto-approve safe", kind: "auto_safe" },
  { label: "Hands off", kind: "hands_off" },
];

/** "⋯" button that opens a compact inline menu of per-session modes. */
export function ModeMenu({
  sessionId,
  mode,
  muted,
}: {
  sessionId: string;
  mode: Mode;
  muted: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [snoozeOpen, setSnoozeOpen] = useState(false);
  const root = useRef<HTMLSpanElement>(null);

  // Close on outside click or Escape while open.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  function choose(next: Mode) {
    setOpen(false);
    setSnoozeOpen(false);
    setMode(sessionId, next).catch(() => {});
  }

  return (
    <span className="modemenu" ref={root}>
      <button
        type="button"
        className="more"
        aria-label="Change mode"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        ⋯
      </button>
      {open && (
        <div className="menu" role="menu">
          {OPTIONS.map((o) => (
            <button
              key={o.kind}
              type="button"
              role="menuitemradio"
              aria-checked={mode.kind === o.kind}
              className="menu-item"
              onClick={() => choose({ kind: o.kind } as Mode)}
            >
              {mode.kind === o.kind ? "✓ " : ""}
              {o.label}
            </button>
          ))}
          <button
            type="button"
            role="menuitem"
            aria-expanded={snoozeOpen}
            className="menu-item"
            onClick={() => setSnoozeOpen((s) => !s)}
          >
            {mode.kind === "snoozed" ? "✓ " : ""}Snooze {snoozeOpen ? "▾" : "▸"}
          </button>
          {snoozeOpen &&
            [
              [15, "15 min"],
              [30, "30 min"],
              [60, "1 h"],
            ].map(([m, label]) => (
              <button
                key={m}
                type="button"
                role="menuitem"
                className="menu-item menu-sub"
                onClick={() => choose(snoozeMode(Number(m), Date.now()))}
              >
                {label}
              </button>
            ))}
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            onClick={() => {
              setOpen(false);
              setSnoozeOpen(false);
              setMuted(sessionId, !muted).catch(() => {});
            }}
          >
            {muted ? "Unmute" : "Mute notifications"}
          </button>
        </div>
      )}
    </span>
  );
}
