import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { launchSession, openTranscript } from "../lib/api";
import { useSettings } from "../lib/useSettings";
import "../App.css";

/** Last path segment, for compact chips. */
function folderName(path: string): string {
  const parts = path.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** "New agent" window: pick a folder, type a first prompt, launch. */
export function Launcher() {
  const { settings } = useSettings();
  const [cwd, setCwd] = useState("");
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const promptRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    document.title = "New agent";
  }, []);

  async function browse() {
    try {
      // directory: true makes the dialog pick a folder, not a file.
      const picked = await open({
        directory: true,
        multiple: false,
        defaultPath: cwd || undefined,
      });
      if (typeof picked === "string") {
        setCwd(picked);
        promptRef.current?.focus();
      }
    } catch (e) {
      setError(String(e));
    }
  }

  const canLaunch = cwd.trim() !== "" && prompt.trim() !== "" && !busy;

  async function launch() {
    if (!canLaunch) return;
    setBusy(true);
    setError(null);
    try {
      const id = await launchSession(cwd.trim(), prompt.trim());
      await openTranscript(id);
      await getCurrentWindow().close();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  function onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    // Enter is a newline here; Ctrl/Cmd+Enter launches.
    if (
      e.key === "Enter" &&
      (e.ctrlKey || e.metaKey) &&
      !e.nativeEvent.isComposing
    ) {
      e.preventDefault();
      void launch();
    }
  }

  const recent = settings?.recent_folders ?? [];

  return (
    <div className="win launcher">
      <label className="field-label" htmlFor="launch-cwd">
        Folder
      </label>
      <div className="folder-row">
        <input
          id="launch-cwd"
          className="text-input"
          type="text"
          value={cwd}
          placeholder="Choose the project folder"
          onChange={(e) => setCwd(e.target.value)}
        />
        <button
          type="button"
          className="btn btn-subtle"
          onClick={() => void browse()}
        >
          Browse...
        </button>
      </div>
      {recent.length > 0 && (
        <div className="chips" aria-label="Recent folders">
          {recent.map((f) => (
            <button
              type="button"
              key={f}
              className={`chip${f === cwd ? " chip-on" : ""}`}
              title={f}
              onClick={() => setCwd(f)}
            >
              {folderName(f)}
            </button>
          ))}
        </div>
      )}
      <label className="field-label" htmlFor="launch-prompt">
        First prompt
      </label>
      <textarea
        id="launch-prompt"
        ref={promptRef}
        className="text-input prompt-input"
        value={prompt}
        placeholder="What should the agent do?"
        onChange={(e) => setPrompt(e.target.value)}
        onKeyDown={onKeyDown}
      />
      <p className="note">
        Permission requests appear in the widget and the transcript. Unanswered
        = denied.
      </p>
      {error && (
        <div className="form-error" role="alert">
          {error}
        </div>
      )}
      <div className="form-actions">
        <span className="hint-key">Ctrl+Enter to launch</span>
        <button
          type="button"
          className="btn btn-allow btn-big"
          disabled={!canLaunch}
          onClick={() => void launch()}
        >
          {busy ? "Launching..." : "Launch"}
        </button>
      </div>
    </div>
  );
}
