//! Snapping the widget flush to a screen edge after the user drags it close
//! to one. The maths is a pure function; `watch` wires it to window events.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Manager, WindowEvent};

use crate::SettingsState;

/// Distance (physical pixels) within which an edge pulls the window in.
pub const SNAP_THRESHOLD: i32 = 16;
/// Wait this long after the last move before snapping.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// A rectangle in physical pixels: top-left corner plus size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Snap one axis. `start`/`len` describe the window, `area_start`/`area_len`
/// the work area. Returns the new start if an edge is within `threshold`.
fn snap_axis(start: i32, len: i32, area_start: i32, area_len: i32, threshold: i32) -> i32 {
    let to_start = (start - area_start).abs();
    let area_end = area_start + area_len;
    let to_end = (start + len - area_end).abs();
    if to_start <= threshold && to_start <= to_end {
        area_start
    } else if to_end <= threshold {
        area_end - len
    } else {
        start
    }
}

/// Where should the window move so it sits flush against the nearby edges of
/// the work area? `None` when nothing needs to move.
pub fn snap(window: Rect, work_area: Rect, threshold: i32) -> Option<(i32, i32)> {
    let x = snap_axis(window.x, window.w, work_area.x, work_area.w, threshold);
    let y = snap_axis(window.y, window.h, work_area.y, work_area.h, threshold);
    if (x, y) == (window.x, window.y) {
        None
    } else {
        Some((x, y))
    }
}

/// Snap the main window once, if enabled and it is near an edge.
fn snap_now(app: &AppHandle) {
    let enabled = app
        .state::<SettingsState>()
        .current
        .lock()
        .map(|s| s.snap_to_edges)
        .unwrap_or(false);
    if !enabled {
        return;
    }
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    // Each call returns a `Result`; `let ... else` leaves quietly on failure.
    let (Ok(pos), Ok(size), Ok(Some(monitor))) = (
        window.outer_position(),
        window.outer_size(),
        window.current_monitor(),
    ) else {
        return;
    };
    let wa = monitor.work_area();
    let area = Rect {
        x: wa.position.x,
        y: wa.position.y,
        w: wa.size.width as i32,
        h: wa.size.height as i32,
    };
    let rect = Rect {
        x: pos.x,
        y: pos.y,
        w: size.width as i32,
        h: size.height as i32,
    };
    if let Some((x, y)) = snap(rect, area, SNAP_THRESHOLD) {
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

/// Listen for window moves and snap 150 ms after the last one. Every move
/// bumps a counter; the delayed task only snaps if no newer move arrived
/// while it slept. That is the debounce. Our own `set_position` triggers one
/// more move event, but then the window is already flush, so `snap` says `None`.
pub fn watch(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let latest = Arc::new(AtomicU64::new(0));
    let handle = app.clone();
    // `move` closures take ownership of the values they use (`latest`, `handle`).
    window.on_window_event(move |event| {
        if let WindowEvent::Moved(_) = event {
            let id = latest.fetch_add(1, Ordering::SeqCst) + 1;
            let latest = latest.clone();
            let handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(DEBOUNCE).await;
                if latest.load(Ordering::SeqCst) == id {
                    snap_now(&handle);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1040,
    };

    fn win(x: i32, y: i32) -> Rect {
        Rect {
            x,
            y,
            w: 320,
            h: 520,
        }
    }

    #[test]
    fn far_from_every_edge_does_nothing() {
        assert_eq!(snap(win(500, 200), AREA, 16), None);
    }

    #[test]
    fn snaps_to_left_and_top() {
        assert_eq!(snap(win(10, 12), AREA, 16), Some((0, 0)));
        assert_eq!(snap(win(-5, 300), AREA, 16), Some((0, 300)));
    }

    #[test]
    fn snaps_to_right_and_bottom() {
        // Right edge at 1930: 10 px past the area edge.
        assert_eq!(snap(win(1610, 300), AREA, 16), Some((1600, 300)));
        // Bottom edge at 1050 vs 1040.
        assert_eq!(snap(win(500, 530), AREA, 16), Some((500, 520)));
    }

    #[test]
    fn already_flush_gives_none() {
        assert_eq!(snap(win(0, 0), AREA, 16), None);
        assert_eq!(snap(win(1600, 520), AREA, 16), None);
    }

    #[test]
    fn threshold_is_inclusive_and_respected() {
        assert_eq!(snap(win(16, 300), AREA, 16), Some((0, 300)));
        assert_eq!(snap(win(17, 300), AREA, 16), None);
    }

    #[test]
    fn works_with_a_work_area_that_does_not_start_at_zero() {
        // Second monitor to the right, taskbar offset at the top.
        let area = Rect {
            x: 1920,
            y: 40,
            w: 1920,
            h: 1000,
        };
        assert_eq!(snap(win(1930, 50), area, 16), Some((1920, 40)));
    }
}
