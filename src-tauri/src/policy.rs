//! Auto-approve policy. Pure functions: no I/O, no clock (the caller passes
//! `now`). The rule of thumb: when in doubt, ask the user.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sessions::Millis;

/// Per-session mode. An enum whose variants can carry data ("Snoozed" has
/// an expiry). With `tag = "kind"` serde writes it as
/// `{"kind":"ask_me"}` or `{"kind":"snoozed","until":1700000000000}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mode {
    /// Every prompt is shown in the widget.
    #[default]
    AskMe,
    /// Read-only tools and allowlisted shell commands are approved silently.
    AutoSafe,
    /// The widget never answers; the terminal asks as usual.
    HandsOff,
    /// Like HandsOff until `until` (Unix ms), then like AskMe.
    Snoozed { until: Millis },
}

/// What the widget should do with a permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Approve immediately.
    Allow,
    /// Say nothing; let the terminal ask.
    NoDecision,
    /// Show the prompt in the widget and wait for the user.
    AskUser,
}

impl Mode {
    /// `true` while the widget must stay silent (hands off / active snooze).
    pub fn is_silent(&self, now: Millis) -> bool {
        match self {
            Mode::HandsOff => true,
            Mode::Snoozed { until } => *until > now,
            _ => false,
        }
    }
}

const SAFE_TOOLS: [&str; 6] = ["Read", "Grep", "Glob", "LS", "WebSearch", "WebFetch"];

/// Characters that chain commands or redirect output (plus newlines).
const SHELL_METACHARS: [char; 9] = [';', '&', '|', '>', '<', '`', '\n', '\r', '\0'];

/// Single words that make a command destructive when they appear as a token.
const DESTRUCTIVE_WORDS: [&str; 15] = [
    "rm",
    "del",
    "erase",
    "rmdir",
    "rd",
    "remove-item",
    "ri",
    "format",
    "mkfs",
    "dd",
    "shutdown",
    "stop-process",
    "kill",
    "taskkill",
    "truncate",
];

/// Multi-word destructive phrases (matched on the lower-cased command with
/// whitespace collapsed to single spaces).
const DESTRUCTIVE_PHRASES: [&str; 4] = [
    "git reset --hard",
    "git clean",
    "git checkout --",
    "drop table",
];

pub fn decide(
    mode: &Mode,
    tool_name: &str,
    tool_input: Option<&Value>,
    safe_commands: &[String],
    now: Millis,
) -> Decision {
    match mode {
        Mode::HandsOff => Decision::NoDecision,
        Mode::Snoozed { until } if *until > now => Decision::NoDecision,
        // An expired snooze behaves like "ask me".
        Mode::AskMe | Mode::Snoozed { .. } => Decision::AskUser,
        Mode::AutoSafe => {
            if is_safe(tool_name, tool_input, safe_commands) {
                Decision::Allow
            } else {
                Decision::AskUser
            }
        }
    }
}

fn is_safe(tool_name: &str, tool_input: Option<&Value>, safe_commands: &[String]) -> bool {
    if SAFE_TOOLS.contains(&tool_name) {
        return true;
    }
    if matches!(tool_name, "Bash" | "PowerShell") {
        // `and_then` chains Options: any missing step gives `None`.
        let command = tool_input
            .and_then(|i| i.get("command"))
            .and_then(Value::as_str);
        return command.is_some_and(|c| is_safe_command(c, safe_commands));
    }
    // Write, Edit, MCP tools, anything unknown: ask.
    false
}

pub fn is_safe_command(command: &str, safe_commands: &[String]) -> bool {
    let command = command.trim();
    if command.is_empty()
        || command.contains(SHELL_METACHARS)
        || command.contains("$(")
        || is_destructive(command)
    {
        return false;
    }
    let lower = command.to_lowercase();
    safe_commands.iter().any(|safe| {
        let safe = safe.trim().to_lowercase();
        if safe.is_empty() || !lower.starts_with(&safe) {
            return false;
        }
        // The allowlisted text must end at a word boundary: `ls` allows
        // `ls -la` but not `lsof`.
        lower[safe.len()..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
    })
}

/// Does the command look destructive anywhere in it?
pub fn is_destructive(command: &str) -> bool {
    let lower = command.to_lowercase();
    let tokens: Vec<&str> = lower
        .split_whitespace()
        .map(|t| t.trim_matches(['"', '\'']))
        .collect();
    if tokens.iter().any(|t| DESTRUCTIVE_WORDS.contains(t)) {
        return true;
    }
    let collapsed = tokens.join(" ");
    if DESTRUCTIVE_PHRASES.iter().any(|p| collapsed.contains(p)) {
        return true;
    }
    // `git push` with --force, --force-with-lease or -f anywhere.
    if collapsed.contains("git push")
        && tokens
            .iter()
            .any(|t| t.starts_with("--force") || *t == "-f")
    {
        return true;
    }
    // `git branch -d/-D/-m/-c/-f` modifies branches (lower-cased, so -D = -d).
    if collapsed.contains("git branch")
        && tokens.iter().any(|t| {
            matches!(
                *t,
                "-d" | "-m" | "-c" | "-f" | "--delete" | "--move" | "--copy" | "--force"
            )
        })
    {
        return true;
    }
    // `git diff --output=file` writes a file.
    tokens.iter().any(|t| t.starts_with("--output"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use serde_json::json;

    fn safe() -> Vec<String> {
        Config::default().safe_commands
    }

    fn auto(tool: &str, input: Value) -> Decision {
        decide(&Mode::AutoSafe, tool, Some(&input), &safe(), 1_000)
    }

    fn cmd(tool: &str, c: &str) -> Decision {
        auto(tool, json!({ "command": c }))
    }

    #[test]
    fn safe_shell_commands_are_allowed() {
        for c in [
            "git status",
            "git status --porcelain",
            "  git status  ",
            "git diff HEAD~1",
            "git log --oneline -5",
            "git show abc123",
            "git branch",
            "git branch -a",
            "ls",
            "ls -la src",
            "pwd",
            "cat README.md",
            "dir",
            "type file.txt",
        ] {
            assert_eq!(cmd("Bash", c), Decision::Allow, "bash: {c}");
        }
    }

    #[test]
    fn safe_powershell_commands_are_allowed() {
        for c in [
            "Get-ChildItem",
            "get-childitem -Recurse src",
            "Get-Content .\\a.txt",
            "Get-Location",
            "git status",
        ] {
            assert_eq!(cmd("PowerShell", c), Decision::Allow, "ps: {c}");
        }
    }

    #[test]
    fn chaining_and_redirection_ask() {
        for c in [
            "git status; rm -rf x",
            "git status && del x",
            "git status || echo x",
            "git status | sh",
            "git status & calc",
            "cat a > b",
            "cat a >> b",
            "cat < a",
            "ls $(rm x)",
            "ls `rm x`",
            "ls\nrm x",
            "ls\r\nrm x",
            "Get-ChildItem; Remove-Item x",
            "Get-Content a | Set-Content b",
        ] {
            assert_eq!(cmd("Bash", c), Decision::AskUser, "{c:?}");
            assert_eq!(cmd("PowerShell", c), Decision::AskUser, "{c:?}");
        }
    }

    #[test]
    fn destructive_commands_ask() {
        for c in [
            "rm -rf x",
            "rm file",
            "del x",
            "erase x",
            "rmdir x",
            "rd /s x",
            "Remove-Item x",
            "ri x",
            "git push --force",
            "git push -f origin main",
            "git push origin main --force-with-lease",
            "git reset --hard HEAD~1",
            "git clean -fd",
            "git checkout -- file",
            "format c:",
            "mkfs.ext4 /dev/sda",
            "dd if=/dev/zero of=x",
            "shutdown /s",
            "Stop-Process -Name x",
            "kill -9 1",
            "taskkill /F /IM x",
            "DROP TABLE users",
            "truncate -s 0 x",
            "REMOVE-ITEM x",
            "git branch -D old",
            "git branch --delete old",
            "git diff --output=out.txt",
        ] {
            assert_eq!(cmd("Bash", c), Decision::AskUser, "{c}");
            assert_eq!(cmd("PowerShell", c), Decision::AskUser, "{c}");
        }
    }

    #[test]
    fn destructive_even_when_starting_with_allowlisted_word() {
        let mut list = safe();
        list.push("git".into());
        assert!(!is_safe_command("git push --force", &list));
        assert!(!is_safe_command("git reset --hard", &list));
        assert!(!is_safe_command("git clean -fd", &list));
        assert!(is_safe_command("git status --porcelain", &list));
    }

    #[test]
    fn prefix_must_end_at_word_boundary() {
        for c in ["lsof -i", "catalog", "directory", "pwdx 1", "git statusx"] {
            assert_eq!(cmd("Bash", c), Decision::AskUser, "{c}");
        }
        assert_eq!(cmd("Bash", ""), Decision::AskUser);
        assert_eq!(cmd("Bash", "   "), Decision::AskUser);
        assert_eq!(cmd("Bash", "echo hi"), Decision::AskUser);
        assert_eq!(cmd("Bash", "npm test"), Decision::AskUser);
    }

    #[test]
    fn missing_command_asks() {
        assert_eq!(auto("Bash", json!({})), Decision::AskUser);
        assert_eq!(auto("Bash", json!({"command": 5})), Decision::AskUser);
        assert_eq!(
            decide(&Mode::AutoSafe, "Bash", None, &safe(), 0),
            Decision::AskUser
        );
    }

    #[test]
    fn safe_tools_are_allowed_unsafe_tools_ask() {
        for t in ["Read", "Grep", "Glob", "LS", "WebSearch", "WebFetch"] {
            assert_eq!(auto(t, json!({"file_path": "a"})), Decision::Allow, "{t}");
        }
        for t in [
            "Write",
            "Edit",
            "MultiEdit",
            "NotebookEdit",
            "Task",
            "AskUserQuestion",
            "ExitPlanMode",
            "mcp__github__create_issue",
            "mcp__fs__read_file",
            "SomethingNew",
            "",
        ] {
            assert_eq!(auto(t, json!({"file_path": "a"})), Decision::AskUser, "{t}");
        }
    }

    #[test]
    fn custom_allowlist_is_used() {
        let list = vec!["npm test".to_string()];
        let input = json!({"command": "npm test -- --watch"});
        assert_eq!(
            decide(&Mode::AutoSafe, "Bash", Some(&input), &list, 0),
            Decision::Allow
        );
        let input = json!({"command": "ls"});
        assert_eq!(
            decide(&Mode::AutoSafe, "Bash", Some(&input), &list, 0),
            Decision::AskUser
        );
    }

    #[test]
    fn modes_other_than_auto_safe() {
        let input = json!({"file_path": "a"});
        let d = |m: Mode, now| decide(&m, "Read", Some(&input), &safe(), now);
        assert_eq!(d(Mode::AskMe, 0), Decision::AskUser);
        assert_eq!(d(Mode::HandsOff, 0), Decision::NoDecision);
        assert_eq!(
            d(Mode::Snoozed { until: 5_000 }, 4_999),
            Decision::NoDecision
        );
        // Expired snooze behaves as ask-me (even for a safe tool).
        assert_eq!(d(Mode::Snoozed { until: 5_000 }, 5_000), Decision::AskUser);
        assert_eq!(d(Mode::Snoozed { until: 5_000 }, 9_000), Decision::AskUser);
        let w = json!({"file_path": "a", "content": "x"});
        assert_eq!(
            decide(&Mode::HandsOff, "Write", Some(&w), &safe(), 0),
            Decision::NoDecision
        );
    }

    #[test]
    fn mode_json_shape() {
        assert_eq!(
            serde_json::to_string(&Mode::AskMe).unwrap(),
            r#"{"kind":"ask_me"}"#
        );
        assert_eq!(
            serde_json::to_string(&Mode::Snoozed { until: 7 }).unwrap(),
            r#"{"kind":"snoozed","until":7}"#
        );
        let m: Mode = serde_json::from_str(r#"{"kind":"hands_off"}"#).unwrap();
        assert_eq!(m, Mode::HandsOff);
        let m: Mode = serde_json::from_str(r#"{"kind":"auto_safe"}"#).unwrap();
        assert_eq!(m, Mode::AutoSafe);
        assert!(Mode::HandsOff.is_silent(0));
        assert!(!Mode::Snoozed { until: 5 }.is_silent(5));
    }
}
