//! App-written user settings in `~/.agentwatch/settings.json`. This is a
//! different file from the hand-edited `config.json`: the widget rewrites
//! this one whenever the user changes something in the settings panel.
//! Like `config.rs`, a missing or broken file is never an error.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

const SETTINGS_FILE: &str = "settings.json";
pub const MIN_OPACITY: f64 = 0.4;
pub const MAX_OPACITY: f64 = 1.0;

/// `#[serde(default)]`: fields missing from the file take their value from
/// `Settings::default()`, so adding a field later never breaks old files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Window opacity, kept between 0.4 and 1.0.
    pub opacity: f64,
    /// Mouse clicks pass through the widget. Only the tray can turn it off.
    pub click_through: bool,
    pub notify_needs_you: bool,
    pub notify_finished: bool,
    /// Play the system's default notification sound.
    pub sound: bool,
    /// Start the widget when the user logs in.
    pub autostart: bool,
    pub snap_to_edges: bool,
    /// Folders agents were launched in, most recent first (max 8).
    pub recent_folders: Vec<String>,
}

/// How many recent folders are remembered.
pub const MAX_RECENT_FOLDERS: usize = 8;

/// Windows paths are case-insensitive, so `C:\A` and `c:\a` are one folder.
fn same_folder(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            opacity: 0.95,
            click_through: false,
            notify_needs_you: true,
            notify_finished: true,
            sound: false,
            autostart: false,
            snap_to_edges: true,
            recent_folders: Vec::new(),
        }
    }
}

impl Settings {
    /// Returns a copy with out-of-range values fixed (used on load and on
    /// every update, so the frontend cannot store nonsense).
    pub fn clamped(mut self) -> Settings {
        self.opacity = if self.opacity.is_finite() {
            self.opacity.clamp(MIN_OPACITY, MAX_OPACITY)
        } else {
            Settings::default().opacity
        };
        self.recent_folders.truncate(MAX_RECENT_FOLDERS);
        self
    }

    /// Put `folder` first in the recent list (removing an older copy) and
    /// keep at most `MAX_RECENT_FOLDERS`.
    pub fn add_recent_folder(&mut self, folder: &str) {
        self.recent_folders.retain(|f| !same_folder(f, folder));
        self.recent_folders.insert(0, folder.to_string());
        self.recent_folders.truncate(MAX_RECENT_FOLDERS);
    }

    /// Read `<dir>/settings.json`; anything wrong gives the defaults.
    /// `dir` is a parameter so tests can use a temp directory.
    pub fn load(dir: &Path) -> Settings {
        match fs::read_to_string(dir.join(SETTINGS_FILE)) {
            Ok(text) => serde_json::from_str::<Settings>(&text)
                .unwrap_or_default()
                .clamped(),
            Err(_) => Settings::default(),
        }
    }

    /// Write atomically: write a temp file, then rename it over the real
    /// one, so a crash mid-write can never leave a half-written file.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = dir.join("settings.json.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, dir.join(SETTINGS_FILE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(dir.path()), Settings::default());
    }

    #[test]
    fn invalid_json_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(SETTINGS_FILE), "{ nope").unwrap();
        assert_eq!(Settings::load(dir.path()), Settings::default());
    }

    #[test]
    fn partial_file_merges_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"sound": true, "opacity": 0.6, "unknown_future_field": 1}"#,
        )
        .unwrap();
        let s = Settings::load(dir.path());
        assert!(s.sound);
        assert_eq!(s.opacity, 0.6);
        assert!(s.notify_needs_you); // default kept
        assert!(s.snap_to_edges);
    }

    #[test]
    fn opacity_is_clamped() {
        let low = Settings {
            opacity: 0.0,
            ..Settings::default()
        };
        assert_eq!(low.clamped().opacity, 0.4);
        let high = Settings {
            opacity: 7.0,
            ..Settings::default()
        };
        assert_eq!(high.clamped().opacity, 1.0);
        let nan = Settings {
            opacity: f64::NAN,
            ..Settings::default()
        };
        assert_eq!(nan.clamped().opacity, 0.95);
    }

    #[test]
    fn save_then_load_round_trips_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings {
            opacity: 0.5,
            click_through: true,
            autostart: true,
            ..Settings::default()
        };
        s.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()), s);
        assert!(!dir.path().join("settings.json.tmp").exists());
    }

    #[test]
    fn recent_folders_dedup_order_and_cap() {
        let mut s = Settings::default();
        for i in 0..10 {
            s.add_recent_folder(&format!("/p/{i}"));
        }
        assert_eq!(s.recent_folders.len(), 8);
        assert_eq!(s.recent_folders[0], "/p/9");
        s.add_recent_folder("/p/5");
        assert_eq!(s.recent_folders[0], "/p/5");
        assert_eq!(s.recent_folders.iter().filter(|f| *f == "/p/5").count(), 1);
        assert_eq!(s.recent_folders.len(), 8);
        if cfg!(windows) {
            s.add_recent_folder("/P/5");
            let same = |f: &&String| f.eq_ignore_ascii_case("/p/5");
            assert_eq!(s.recent_folders.iter().filter(same).count(), 1);
            assert_eq!(s.recent_folders[0], "/P/5");
        }
    }

    #[test]
    fn out_of_range_file_is_clamped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(SETTINGS_FILE), r#"{"opacity": 0.1}"#).unwrap();
        assert_eq!(Settings::load(dir.path()).opacity, 0.4);
    }
}
