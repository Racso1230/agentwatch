// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The Windows uninstaller runs `agentwatch.exe --disconnect-hooks` to
    // remove our hook entries; no window, no Tauri.
    if std::env::args().any(|a| a == "--disconnect-hooks") {
        agentwatch_lib::hooks::disconnect_cli();
        return;
    }
    agentwatch_lib::run()
}
