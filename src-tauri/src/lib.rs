pub mod config;
pub mod constants;
pub mod endpoint;
pub mod focus;
pub mod notify;
pub mod owned;
pub mod pending;
pub mod policy;
pub mod server;
pub mod sessions;
pub mod settings;
pub mod snap;
pub mod timers;
pub mod transcript;
pub mod tray;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager, RunEvent};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_notification::NotificationExt as _;
use tauri_plugin_window_state::StateFlags;

use config::Config;
use notify::Notifier;
use owned::{Owned, OwnedEvent, TranscriptItem};
use pending::{Pending, PermissionAnswer};
use policy::Mode;
use server::{AppState, SharedStore};
use sessions::{SessionSnapshot, SessionStore};
use settings::Settings;

/// Name of the Tauri event the frontend listens to.
pub const SESSIONS_EVENT: &str = "sessions";

/// Emitted with the full `Settings` whenever they change (panel or tray).
pub const SETTINGS_EVENT: &str = "settings";

/// The current settings plus the folder they are saved in. Kept in Tauri's
/// managed state; any thread can reach it with `app.state::<SettingsState>()`.
pub struct SettingsState {
    pub current: Mutex<Settings>,
    dir: PathBuf,
}

/// Make the app match `settings`: click-through, autostart, tray checkbox.
/// Takes `&mut` because a failed autostart change is written back, so what
/// we save and show is what is actually true.
fn apply_settings(app: &tauri::AppHandle, settings: &mut Settings) {
    // Click-through without a tray icon would be impossible to turn off.
    if app.try_state::<tray::TrayHandles>().is_none() {
        settings.click_through = false;
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_ignore_cursor_events(settings.click_through);
    }
    // `app.autolaunch()` comes from the autostart plugin's `ManagerExt` trait.
    let autostart = app.autolaunch();
    if let Ok(enabled) = autostart.is_enabled() {
        if enabled != settings.autostart {
            let result = if settings.autostart {
                autostart.enable()
            } else {
                autostart.disable()
            };
            if let Err(e) = result {
                eprintln!("agentwatch: autostart change failed: {e}");
                settings.autostart = enabled;
            }
        }
    }
    if let Some(handles) = app.try_state::<tray::TrayHandles>() {
        let _ = handles.click_through.set_checked(settings.click_through);
    }
}

/// Apply, save and broadcast a new settings value; returns what was stored.
fn commit_settings(app: &tauri::AppHandle, wanted: Settings) -> Result<Settings, String> {
    let state = app.state::<SettingsState>();
    let mut settings = wanted.clamped();
    apply_settings(app, &mut settings);
    settings.save(&state.dir).map_err(|e| e.to_string())?;
    *state.current.lock().unwrap_or_else(|e| e.into_inner()) = settings.clone();
    let _ = app.emit(SETTINGS_EVENT, settings.clone());
    Ok(settings)
}

/// Tray "Click-through" item: flip the setting.
pub fn toggle_click_through(app: &tauri::AppHandle) {
    let mut next = app
        .state::<SettingsState>()
        .current
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    next.click_through = !next.click_through;
    if let Err(e) = commit_settings(app, next) {
        eprintln!("agentwatch: saving settings failed: {e}");
    }
}

#[tauri::command]
fn get_settings(state: tauri::State<'_, SettingsState>) -> Settings {
    state
        .current
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Validates (clamps), applies side effects, saves, emits the `settings`
/// event, and returns the settings that were actually stored.
#[tauri::command]
fn update_settings(app: tauri::AppHandle, settings: Settings) -> Result<Settings, String> {
    commit_settings(&app, settings)
}

/// Mute or unmute desktop notifications for one session. Returns whether
/// anything changed.
#[tauri::command]
fn set_muted(state: tauri::State<'_, AppState>, session_id: String, muted: bool) -> bool {
    let snapshot = {
        let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        if store.set_muted(&session_id, muted) {
            Some(store.snapshot())
        } else {
            None
        }
    };
    match snapshot {
        Some(snapshot) => {
            (state.on_change)(snapshot);
            true
        }
        None => false,
    }
}

/// Bring a VS Code session's window to the front. Terminal sessions return
/// an error string the UI can show.
#[tauri::command]
fn focus_session(state: tauri::State<'_, AppState>, session_id: String) -> Result<(), String> {
    let (entrypoint, cwd) = {
        let store = state.store.lock().unwrap_or_else(|e| e.into_inner());
        let s = store.get(&session_id).ok_or("unknown session")?;
        (s.entrypoint.clone(), s.cwd.clone())
    };
    focus::focus_vscode(entrypoint.as_deref(), &cwd)
}

/// Show a desktop notification for each session change worth one, then send
/// the snapshot to the frontend. A failed emit (window closing) is harmless.
fn emit_sessions(app: &tauri::AppHandle, snapshot: Vec<SessionSnapshot>) {
    show_notifications(app, &snapshot);
    let _ = app.emit(SESSIONS_EVENT, snapshot);
}

fn show_notifications(app: &tauri::AppHandle, snapshot: &[SessionSnapshot]) {
    // `try_state` gives `None` instead of panicking if setup has not
    // registered the value yet.
    let (Some(notifier), Some(settings)) = (
        app.try_state::<Notifier>(),
        app.try_state::<SettingsState>(),
    ) else {
        return;
    };
    let settings = settings
        .current
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    for notice in notifier.notices(snapshot, &settings, focused) {
        let mut builder = app
            .notification()
            .builder()
            .title(notice.title)
            .body(notice.body);
        if settings.sound {
            // "default" asks the OS for its standard notification sound.
            builder = builder.sound("default");
        }
        if let Err(e) = builder.show() {
            eprintln!("agentwatch: notification failed: {e}");
        }
    }
}

/// The `~/.agentwatch` directory, kept in Tauri's managed state so the exit
/// handler can find it again.
struct DataDir(PathBuf);

/// Frontend asks for the current list once at startup; after that it just
/// listens for `sessions` events. `tauri::State` hands us the value that
/// `app.manage(...)` registered in `setup`.
#[tauri::command]
fn get_sessions(store: tauri::State<'_, SharedStore>) -> Vec<SessionSnapshot> {
    store.lock().unwrap_or_else(|e| e.into_inner()).snapshot()
}

/// `decision` is "allow", "deny" or "none" ("Let Claude ask"). Returns
/// whether a prompt was actually waiting (false = it was already resolved).
#[tauri::command]
fn answer_permission(
    state: tauri::State<'_, AppState>,
    session_id: String,
    decision: String,
) -> Result<bool, String> {
    let answer = match decision.as_str() {
        "allow" => PermissionAnswer::Allow,
        "deny" => PermissionAnswer::Deny,
        "none" => PermissionAnswer::None,
        other => return Err(format!("unknown decision: {other}")),
    };
    Ok(state.answer_permission(&session_id, answer))
}

/// Send the user's reply to a waiting Stop hook. Empty text is rejected.
#[tauri::command]
fn send_reply(
    state: tauri::State<'_, AppState>,
    session_id: String,
    text: String,
) -> Result<bool, String> {
    if text.trim().is_empty() {
        return Err("reply is empty".to_string());
    }
    Ok(state.send_reply(&session_id, &text))
}

/// Close the reply box without replying.
#[tauri::command]
fn dismiss_reply(state: tauri::State<'_, AppState>, session_id: String) -> bool {
    state.dismiss_reply(&session_id)
}

/// `mode` arrives as JSON like `{"kind":"snoozed","until":1700000000000}`.
#[tauri::command]
fn set_mode(state: tauri::State<'_, AppState>, session_id: String, mode: Mode) -> bool {
    state.set_mode(&session_id, mode)
}

/// Emitted when the tray's Quit is chosen while owned agents are running.
/// The payload is how many; the UI asks the user, then calls `quit_app`.
pub const CONFIRM_QUIT_EVENT: &str = "confirm-quit";

/// Quit now, unless owned agents are running: then show the window and ask
/// the UI to confirm first (tray "Quit" uses this).
pub fn request_quit(app: &tauri::AppHandle) {
    let running = app
        .try_state::<Owned>()
        .map(|o| o.running_count())
        .unwrap_or(0);
    if running == 0 {
        app.exit(0);
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
    let _ = app.emit(CONFIRM_QUIT_EVENT, running);
}

/// Start a Claude Code agent in `cwd`. Returns the temporary session id; the
/// real one follows in a `session-rekeyed` event. Remembers the folder.
#[tauri::command]
async fn launch_session(
    app: tauri::AppHandle,
    owned: tauri::State<'_, Owned>,
    cwd: String,
    prompt: String,
) -> Result<String, String> {
    let id = owned.launch(&cwd, &prompt).await?;
    let mut next = app
        .state::<SettingsState>()
        .current
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    next.add_recent_folder(&cwd);
    if let Err(e) = commit_settings(&app, next) {
        eprintln!("agentwatch: saving recent folders failed: {e}");
    }
    Ok(id)
}

#[tauri::command]
fn get_transcript(
    owned: tauri::State<'_, Owned>,
    session_id: String,
) -> Result<Vec<TranscriptItem>, String> {
    owned.get_transcript(&session_id)
}

/// Send a follow-up message to an owned agent. Empty text is rejected.
#[tauri::command]
fn send_message(
    owned: tauri::State<'_, Owned>,
    session_id: String,
    text: String,
) -> Result<(), String> {
    owned.send_message(&session_id, &text)
}

/// Stop the agent's current turn; the session stays open (see `owned.rs`).
#[tauri::command]
fn interrupt_session(owned: tauri::State<'_, Owned>, session_id: String) -> Result<(), String> {
    owned.interrupt(&session_id)
}

/// Close the agent's input; it is killed after 5 s if it does not exit.
/// `async` so a tokio runtime is available for the grace timer.
#[tauri::command]
async fn end_session(owned: tauri::State<'_, Owned>, session_id: String) -> Result<(), String> {
    owned.end(&session_id)
}

/// Window label for a session's transcript. Labels may only contain letters,
/// digits, `-`, `/`, `:` and `_`, so anything else in the id becomes `_`.
pub fn transcript_label(session_id: &str) -> String {
    let clean: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("transcript-{clean}")
}

/// Show the window called `label` if it exists; returns whether it did.
fn focus_existing(app: &tauri::AppHandle, label: &str) -> bool {
    match app.get_webview_window(label) {
        Some(w) => {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
            true
        }
        None => false,
    }
}

/// Open (or focus) the "new agent" launcher window. `async` so window
/// creation does not run on the main thread (it would deadlock on Windows).
#[tauri::command]
async fn open_launcher(app: tauri::AppHandle) -> Result<(), String> {
    if focus_existing(&app, "launcher") {
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        &app,
        "launcher",
        tauri::WebviewUrl::App("index.html?view=launcher".into()),
    )
    .title("New agent")
    .inner_size(520.0, 460.0)
    .min_inner_size(380.0, 340.0)
    .build()
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Open (or focus) the transcript window for one owned session.
#[tauri::command]
async fn open_transcript(app: tauri::AppHandle, session_id: String) -> Result<(), String> {
    let label = transcript_label(&session_id);
    if focus_existing(&app, &label) {
        return Ok(());
    }
    // The id goes into a URL query: percent-encode the few unsafe characters.
    let encoded: String = session_id
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    tauri::WebviewWindowBuilder::new(
        &app,
        label,
        tauri::WebviewUrl::App(format!("index.html?view=transcript&session={encoded}").into()),
    )
    .title("agentwatch transcript")
    .inner_size(720.0, 640.0)
    .min_inner_size(420.0, 320.0)
    .build()
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// How many owned agents are still running (for the quit confirmation).
#[tauri::command]
fn owned_running_count(owned: tauri::State<'_, Owned>) -> usize {
    owned.running_count()
}

/// Kill all owned agents and exit. The UI calls this after the user agreed.
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    if let Some(owned) = app.try_state::<Owned>() {
        owned.kill_all();
    }
    app.exit(0);
}

pub fn run() {
    // Plugins are added to the builder before `.build()`; each one registers
    // itself with the app (and may add commands or managed state).
    let app = tauri::Builder::default()
        // Remember window position and size between runs. Only those two:
        // the default flags would also restore visibility/decorations and
        // fight the settings in tauri.conf.json and the tray.
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(StateFlags::POSITION | StateFlags::SIZE)
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        // Autostart stays off unless the setting turns it on.
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            get_settings,
            update_settings,
            set_muted,
            focus_session,
            get_sessions,
            answer_permission,
            send_reply,
            dismiss_reply,
            set_mode,
            launch_session,
            get_transcript,
            send_message,
            interrupt_session,
            end_session,
            owned_running_count,
            quit_app,
            open_launcher,
            open_transcript
        ])
        .setup(|app| {
            // `?` converts the error and returns early from setup; Tauri
            // then reports it and the app does not start.
            let dir = endpoint::default_dir().ok_or("cannot determine home directory")?;
            let token = endpoint::load_or_create_token(&dir)?;

            let store: SharedStore = Arc::new(Mutex::new(SessionStore::new()));
            app.manage(store.clone());
            app.manage(DataDir(dir.clone()));
            app.manage(Notifier::default());

            // The callback owns a clone of the app handle (`move`), and sends
            // every changed snapshot to the frontend.
            let handle = app.handle().clone();
            let state = AppState {
                store,
                pending: Pending::new(),
                config: Config::load(&dir),
                token: Arc::from(token.as_str()),
                on_change: Arc::new(move |snapshot| emit_sessions(&handle, snapshot)),
                now: server::now_millis,
            };

            // Owned sessions: processes agentwatch launches itself. Their
            // events become Tauri events for the transcript window.
            let owned_handle = app.handle().clone();
            app.manage(Owned::new(
                state.config.clone(),
                state.store.clone(),
                state.on_change.clone(),
                Arc::new(move |event| match event {
                    OwnedEvent::Transcript(e) => {
                        let _ = owned_handle.emit("transcript", e);
                    }
                    OwnedEvent::Rekeyed(e) => {
                        let _ = owned_handle.emit("session-rekeyed", e);
                    }
                }),
                server::now_millis,
            ));

            // Bind now so a failure shows up at startup, then publish the
            // port for the shim and serve on Tauri's async runtime.
            let (listener, port) = tauri::async_runtime::block_on(server::bind())?;
            endpoint::write_endpoint(&dir, port, &token)?;
            // Cloning `Arc`s / handles is cheap; each background task gets
            // its own copy because `async move` takes ownership.
            let sweep_store = state.store.clone();
            let sweep_handle = app.handle().clone();
            let watch_store = state.store.clone();
            let watch_pending = state.pending.clone();
            app.manage(state.clone());
            let watch_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = server::serve(listener, state).await {
                    eprintln!("agentwatch: server stopped: {e}");
                }
            });
            // Idle/stale sweep every few seconds.
            tauri::async_runtime::spawn(timers::run(sweep_store, move |snapshot| {
                emit_sessions(&sweep_handle, snapshot)
            }));
            // Transcript watcher for prompts denied outside the widget.
            tauri::async_runtime::spawn(transcript::run(
                watch_store,
                watch_pending,
                move |snapshot| emit_sessions(&watch_handle, snapshot),
            ));

            // Settings, tray and edge snapping.
            let mut settings = Settings::load(&dir);
            app.manage(SettingsState {
                current: Mutex::new(settings.clone()),
                dir: dir.clone(),
            });
            // A missing tray (e.g. a Linux desktop without one) is not fatal,
            // but then click-through is switched off by `apply_settings`.
            if let Err(e) = tray::build(app.handle(), settings.click_through) {
                eprintln!("agentwatch: tray icon unavailable: {e}");
            }
            apply_settings(app.handle(), &mut settings);
            if let Ok(mut current) = app.state::<SettingsState>().current.lock() {
                *current = settings;
            }
            snap::watch(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        // Clean exit: remove endpoint.json so the shim finds nothing.
        if let RunEvent::Exit = event {
            // Never leave launched agents running after the app is gone.
            if let Some(owned) = handle.try_state::<Owned>() {
                owned.kill_all();
            }
            if let Some(dir) = handle.try_state::<DataDir>() {
                endpoint::remove_endpoint(&dir.0);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::transcript_label;

    #[test]
    fn labels_are_sanitised() {
        assert_eq!(transcript_label("pending-abc"), "transcript-pending-abc");
        assert_eq!(transcript_label("a b/c.d"), "transcript-a_b_c_d");
    }
}
