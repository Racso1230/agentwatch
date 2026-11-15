import { Fragment, useCallback, useEffect, useState } from "react";
import type { HooksStatus, Settings } from "../types";
import { hooksConnect, hooksDisconnect, hooksStatus } from "../lib/api";
import {
  canDisconnect,
  connectButtonLabel,
  connectConsent,
  hooksDetail,
  hooksLabel,
  hooksState,
} from "../lib/hooksUi";
import { clampOpacity, opacityLabel } from "../lib/settings";

type ToggleKey = Exclude<keyof Settings, "opacity" | "recent_folders">;

const TOGGLES: { key: ToggleKey; label: string }[] = [
  { key: "click_through", label: "Click-through" },
  { key: "notify_needs_you", label: "Notify when a session needs you" },
  { key: "notify_finished", label: "Notify when a session finishes" },
  { key: "sound", label: "Sound with notifications" },
  { key: "autostart", label: "Start on login" },
  { key: "snap_to_edges", label: "Snap to screen edges" },
];

/** "Claude Code connection": connect / disconnect the hooks, with consent. */
function ConnectionSection({
  onChange,
}: {
  onChange: (s: HooksStatus) => void;
}) {
  const [status, setStatus] = useState<HooksStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);

  const apply = useCallback(
    (s: HooksStatus) => {
      setStatus(s);
      onChange(s);
    },
    [onChange],
  );

  // Load the status when the panel opens.
  useEffect(() => {
    let cancelled = false;
    hooksStatus()
      .then((s) => {
        if (!cancelled) apply(s);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [apply]);

  function run(action: () => Promise<HooksStatus>) {
    setBusy(true);
    setError(null);
    action()
      .then((s) => {
        apply(s);
        setConfirming(false);
      })
      .catch((e: unknown) => setError(String(e)))
      .finally(() => setBusy(false));
  }

  const state = hooksState(status);
  const detail = hooksDetail(state);

  return (
    <section className="conn" aria-label="Claude Code connection">
      <div className="conn-title">Claude Code connection</div>
      <div>
        <span className={`conn-state conn-${state}`}>{hooksLabel(state)}</span>
      </div>
      {detail && <div className="setting-warn conn-detail">{detail}</div>}
      {status && <div className="conn-path">{status.settings_path}</div>}
      {confirming && status ? (
        <>
          <p className="conn-consent">{connectConsent(status.settings_path)}</p>
          <div className="perm-actions">
            <button
              type="button"
              className="btn btn-allow"
              disabled={busy}
              onClick={() => run(hooksConnect)}
            >
              {busy ? "Connecting..." : "Yes, add hooks"}
            </button>
            <button
              type="button"
              className="btn btn-subtle"
              disabled={busy}
              onClick={() => setConfirming(false)}
            >
              Cancel
            </button>
          </div>
        </>
      ) : (
        <div className="perm-actions">
          <button
            type="button"
            className="btn btn-allow"
            disabled={busy || !status || state === "connected"}
            onClick={() => setConfirming(true)}
          >
            {connectButtonLabel(state)}
          </button>
          <button
            type="button"
            className="btn btn-subtle"
            disabled={busy || !canDisconnect(state)}
            onClick={() => run(hooksDisconnect)}
          >
            Disconnect
          </button>
        </div>
      )}
      {error && (
        <div className="reply-error" role="alert">
          {error}
        </div>
      )}
    </section>
  );
}

/** In-window settings screen. Every change goes to `onSave` (update_settings). */
export function SettingsPanel({
  settings,
  error,
  onSave,
  onPreviewOpacity,
  onBack,
  onHooksChange,
}: {
  settings: Settings;
  error: string | null;
  onSave: (next: Settings) => void;
  /** Live preview while dragging; null clears the preview. */
  onPreviewOpacity: (v: number | null) => void;
  onBack: () => void;
  /** Called with the fresh hooks status after load and after each action. */
  onHooksChange: (s: HooksStatus) => void;
}) {
  // While dragging we show the draft; otherwise the server value.
  const [draft, setDraft] = useState<number | null>(null);
  const opacity = draft ?? settings.opacity;

  function commitOpacity() {
    if (draft === null) return;
    const v = clampOpacity(draft);
    setDraft(null);
    onPreviewOpacity(null);
    if (v !== settings.opacity) onSave({ ...settings, opacity: v });
  }

  return (
    <div className="settings">
      <div className="settings-head">
        <button type="button" className="btn btn-subtle" onClick={onBack}>
          ← Back
        </button>
        <span className="settings-title">Settings</span>
      </div>
      <div className="settings-body">
        <label className="setting setting-slider">
          <span className="setting-label">
            Opacity{" "}
            <span className="setting-value">{opacityLabel(opacity)}</span>
          </span>
          <input
            type="range"
            min={0.4}
            max={1}
            step={0.05}
            value={opacity}
            onChange={(e) => {
              const v = clampOpacity(Number(e.target.value));
              setDraft(v);
              onPreviewOpacity(v);
            }}
            onPointerUp={commitOpacity}
            onKeyUp={commitOpacity}
            onBlur={commitOpacity}
          />
        </label>
        {TOGGLES.map((t) => (
          <Fragment key={t.key}>
            <label className="setting">
              <input
                type="checkbox"
                checked={settings[t.key]}
                onChange={(e) =>
                  onSave({ ...settings, [t.key]: e.target.checked })
                }
              />
              <span className="setting-label">{t.label}</span>
            </label>
            {t.key === "click_through" && (
              <div className="setting-warn">Turn off from the tray icon</div>
            )}
          </Fragment>
        ))}
        {error && (
          <div className="reply-error" role="alert">
            {error}
          </div>
        )}
        <ConnectionSection onChange={onHooksChange} />
      </div>
    </div>
  );
}
