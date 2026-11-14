import { Fragment, useState } from "react";
import type { Settings } from "../types";
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

/** In-window settings screen. Every change goes to `onSave` (update_settings). */
export function SettingsPanel({
  settings,
  error,
  onSave,
  onPreviewOpacity,
  onBack,
}: {
  settings: Settings;
  error: string | null;
  onSave: (next: Settings) => void;
  /** Live preview while dragging; null clears the preview. */
  onPreviewOpacity: (v: number | null) => void;
  onBack: () => void;
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
      </div>
    </div>
  );
}
