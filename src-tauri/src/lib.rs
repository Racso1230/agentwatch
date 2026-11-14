pub mod constants;
pub mod endpoint;
pub mod server;
pub mod sessions;
pub mod timers;
pub mod transcript;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager, RunEvent};

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

pub fn run() {
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_sessions])
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
            tauri::async_runtime::spawn(transcript::run(watch_store, move |snapshot| {
                emit_sessions(&watch_handle, snapshot)
            }));
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
