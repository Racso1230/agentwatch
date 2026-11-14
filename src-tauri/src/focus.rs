//! Bringing a VS Code window to the front. Running `code <folder>` makes VS
//! Code focus the window that already has that folder open. Only sessions
//! started from the VS Code extension qualify; terminal sessions have no
//! window we can find.

use std::path::Path;
use std::process::{Command, Stdio};

/// The entrypoint value Claude Code reports for the VS Code extension.
const VSCODE_ENTRYPOINT: &str = "claude-vscode";

/// On Windows `code` is a `code.cmd` script, which `Command` only finds when
/// the extension is spelled out.
#[cfg(windows)]
const CODE_PROGRAM: &str = "code.cmd";
#[cfg(not(windows))]
const CODE_PROGRAM: &str = "code";

/// Checks that we may focus this session and that `cwd` is a safe argument.
/// Pure (apart from one directory check), so it is easy to test.
pub fn check_target(entrypoint: Option<&str>, cwd: &str) -> Result<(), String> {
    if entrypoint != Some(VSCODE_ENTRYPOINT) {
        return Err("terminal session: no window to focus".to_string());
    }
    if cwd.is_empty() {
        return Err("session has no folder".to_string());
    }
    // A folder starting with `-` could be read by `code` as an option.
    if cwd.starts_with('-') {
        return Err("unsafe folder name".to_string());
    }
    if !Path::new(cwd).is_dir() {
        return Err(format!("folder not found: {cwd}"));
    }
    Ok(())
}

/// Run `code <cwd>`. The folder is passed as a separate argument (never
/// through a shell string), so odd characters in it cannot run commands.
pub fn focus_vscode(entrypoint: Option<&str>, cwd: &str) -> Result<(), String> {
    check_target(entrypoint, cwd)?;
    let mut command = Command::new(CODE_PROGRAM);
    command
        .arg(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // `cfg` blocks are compiled only on that OS. CREATE_NO_WINDOW stops a
    // console window from flashing up on Windows.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not run `{CODE_PROGRAM}` (is it on PATH?): {e}"))?;
    // Reap the child on its own thread so it never lingers as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_sessions_cannot_be_focused() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        for entry in [None, Some("cli"), Some("sdk-ts")] {
            assert_eq!(
                check_target(entry, cwd),
                Err("terminal session: no window to focus".to_string())
            );
        }
    }

    #[test]
    fn vscode_session_with_existing_folder_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            check_target(Some("claude-vscode"), dir.path().to_str().unwrap()),
            Ok(())
        );
    }

    #[test]
    fn bad_folders_are_rejected() {
        assert!(check_target(Some("claude-vscode"), "").is_err());
        assert!(check_target(Some("claude-vscode"), "--reuse-window").is_err());
        assert!(check_target(Some("claude-vscode"), "/definitely/not/here").is_err());
    }
}
