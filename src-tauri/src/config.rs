//! User settings in `~/.agentwatch/config.json`. A missing, unreadable or
//! invalid file is never an error: we fall back to the defaults, because the
//! widget must keep working (and fail open) whatever is on disk.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// `#[serde(default)]` on the struct means: any field missing from the JSON
/// file takes its value from `Config::default()`. So a file containing only
/// `{"permission_timeout_secs": 30}` is valid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Shell commands that auto-safe mode may approve (prefix match).
    pub safe_commands: Vec<String>,
    /// How long `/permission` waits for a widget answer.
    pub permission_timeout_secs: u64,
    /// How long `/stop` keeps the reply box open.
    pub reply_max_wait_secs: u64,
    /// Full path to the `claude` executable, if it is not on PATH.
    pub claude_path: Option<String>,
    /// Advanced / tests: a whole command (program + leading args) used
    /// instead of `claude`, e.g. `["bun", "fake-claude.ts"]`.
    pub claude_command: Option<Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            safe_commands: [
                "git status",
                "git diff",
                "git log",
                "git show",
                "git branch",
                "ls",
                "dir",
                "pwd",
                "cat",
                "type",
                "Get-ChildItem",
                "Get-Content",
                "Get-Location",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            permission_timeout_secs: 120,
            reply_max_wait_secs: 1800,
            claude_path: None,
            claude_command: None,
        }
    }
}

impl Config {
    /// Read `<dir>/config.json`. `dir` is a parameter so tests can use a
    /// temp directory instead of the real home directory.
    pub fn load(dir: &Path) -> Config {
        // `match` on a `Result`: `Ok(text)` runs when the read worked,
        // `Err(_)` when it failed (we ignore why and use defaults).
        match std::fs::read_to_string(dir.join("config.json")) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }

    pub fn permission_timeout(&self) -> Duration {
        Duration::from_secs(self.permission_timeout_secs)
    }

    pub fn reply_max_wait(&self) -> Duration {
        Duration::from_secs(self.reply_max_wait_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(dir.path());
        assert_eq!(c, Config::default());
        assert_eq!(c.permission_timeout_secs, 120);
        assert_eq!(c.reply_max_wait_secs, 1800);
        assert!(c.safe_commands.contains(&"git status".to_string()));
        assert_eq!(c.safe_commands.len(), 13);
    }

    #[test]
    fn invalid_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "{ not json").unwrap();
        assert_eq!(Config::load(dir.path()), Config::default());
        std::fs::write(dir.path().join("config.json"), r#"{"safe_commands": 5}"#).unwrap();
        assert_eq!(Config::load(dir.path()), Config::default());
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{"permission_timeout_secs": 30, "safe_commands": ["ls"]}"#,
        )
        .unwrap();
        let c = Config::load(dir.path());
        assert_eq!(c.permission_timeout_secs, 30);
        assert_eq!(c.safe_commands, vec!["ls".to_string()]);
        assert_eq!(c.reply_max_wait_secs, 1800);
        assert_eq!(c.permission_timeout(), Duration::from_secs(30));
    }
}
