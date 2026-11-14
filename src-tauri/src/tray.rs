//! The system tray icon and its menu. The tray is also the escape hatch for
//! click-through mode: when clicks pass through the widget, the tray is the
//! only way to turn it off again.

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

/// Event the frontend listens to in order to open its settings panel.
pub const OPEN_SETTINGS_EVENT: &str = "open-settings";

/// Handle to the checkbox item, kept in managed state so we can tick or
/// untick it when the setting changes from somewhere else (the settings panel).
pub struct TrayHandles {
    pub click_through: CheckMenuItem<tauri::Wry>,
}

/// Show the main window if hidden, hide it if visible.
fn toggle_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
    } else {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Build the tray icon + menu. Called once from `setup`.
///
/// `app: &AppHandle` is a cheap, clonable handle to the running app. The
/// closures below `move` their own clone in so they can use it later, when
/// the user clicks a menu item.
pub fn build(app: &AppHandle, click_through: bool) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Show / Hide widget", true, None::<&str>)?;
    let through = CheckMenuItem::with_id(
        app,
        "click_through",
        "Click-through",
        true,
        click_through,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&toggle, &through, &settings, &sep, &quit])?;

    app.manage(TrayHandles {
        click_through: through,
    });

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("agentwatch")
        .menu(&menu)
        // Left click toggles the window; the menu opens on right click.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => toggle_window(app),
            "click_through" => crate::toggle_click_through(app),
            "settings" => {
                show_window(app);
                let _ = app.emit(OPEN_SETTINGS_EVENT, ());
            }
            // Asks first when owned agents are running; otherwise exits (so
            // `RunEvent::Exit` fires and endpoint.json is removed).
            "quit" => crate::request_quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}
