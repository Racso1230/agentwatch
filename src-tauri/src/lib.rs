pub mod config;
pub mod constants;
pub mod endpoint;
pub mod pending;
pub mod policy;
pub mod server;
pub mod settings;
pub mod sessions;
pub mod timers;
pub mod transcript;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager, RunEvent};

use config::Config;
use pending::{Pending, PermissionAnswer};
use policy::Mode;
use server::{AppState, SharedStore};
use sessions::{SessionSnapshot, SessionStore};

/// Name of the Tauri event the frontend listens to.
pub const SESSIONS_EVENT: &str = "sessions";

/// Send a snapshot to the frontend. A failed emit (window closing) is harmless.
fn emit_sessions(app: &tauri::AppHandle, snapshot: Vec<SessionSnapshot>) {
    let _ = app.emit(SESSIONS_EVENT, snapshot);
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

pub fn run() {
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_sessions,
            answer_permission,
            send_reply,
            dismiss_reply,
            set_mode
        ])
        .setup(|app| {
            // `?` converts the error and returns early from setup; Tauri
            // then reports it and the app does not start.
            let dir = endpoint::default_dir().ok_or("cannot determine home directory")?;
            let token = endpoint::load_or_create_token(&dir)?;

            let store: SharedStore = Arc::new(Mutex::new(SessionStore::new()));
            app.manage(store.clone());
            app.manage(DataDir(dir.clone()));

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
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        // Clean exit: remove endpoint.json so the shim finds nothing.
        if let RunEvent::Exit = event {
            if let Some(dir) = handle.try_state::<DataDir>() {
                endpoint::remove_endpoint(&dir.0);
            }
        }
    });
}
