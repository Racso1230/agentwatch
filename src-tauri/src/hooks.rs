//! Connect / disconnect: installs agentwatch's hooks into Claude Code's
//! `settings.json` and puts the `agentwatch-hook` binary at a stable path
//! (`~/.agentwatch/bin`). Ported from the old `scripts/hooks-config.ts`.
//!
//! The merge logic works on `serde_json::Value` and never touches the disk.
//! Every function that does touch the disk takes its paths as parameters, so
//! tests can use temp dirs and never the real `~/.claude` or `~/.agentwatch`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Map, Value};

/// Every hook command we write contains this, which is how we recognise (and
/// only ever remove) our own entries. The old Bun shim path contained it too.
pub const MARKER: &str = "agentwatch-hook";

/// Events agentwatch listens to (SubagentStop etc. are intentionally absent).
pub const EVENTS: [&str; 10] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "Notification",
    "Stop",
    "StopFailure",
];

/// Events with no matcher support (docs/hooks-reference.md 1.1).
const NO_MATCHER: [&str; 2] = ["UserPromptSubmit", "Stop"];

/// File name of the hook binary (`agentwatch-hook.exe` on Windows).
fn hook_file_name() -> String {
    format!("{MARKER}{}", std::env::consts::EXE_SUFFIX)
}

// ---------------------------------------------------------------------------
// Pure merge logic
// ---------------------------------------------------------------------------

/// Handler shape per event. PermissionRequest is synchronous (the widget may
/// answer) with a timeout above the server's 120 s hold; Stop uses asyncRewake
/// so exit code 2 can wake Claude without blocking the terminal; the rest are
/// fire-and-forget async hooks.
fn handler_for(event: &str, command: &str) -> Value {
    // `json!` builds a serde_json `Value` from JSON-like syntax.
    match event {
        "PermissionRequest" => json!({ "type": "command", "command": command, "timeout": 130 }),
        "Stop" => json!({ "type": "command", "command": command, "asyncRewake": true }),
        _ => json!({ "type": "command", "command": command, "async": true, "timeout": 5 }),
    }
}

/// Human-readable hook kind, derived from a handler's fields.
pub fn kind_of(handler: &Value) -> String {
    if handler.get("asyncRewake") == Some(&Value::Bool(true)) {
        "asyncRewake".to_string()
    } else if handler.get("async") == Some(&Value::Bool(true)) {
        "async".to_string()
    } else {
        match handler.get("timeout").and_then(Value::as_f64) {
            Some(t) => format!("sync {t}s"),
            None => "sync".to_string(),
        }
    }
}

fn is_ours(handler: &Value) -> bool {
    handler
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains(MARKER))
}

/// Forward slashes work for both Git Bash and cmd on Windows. The path is
/// wrapped in double quotes because it may contain spaces.
pub fn build_command(path: &Path) -> String {
    let fwd = path.to_string_lossy().replace('\\', "/");
    format!("\"{fwd}\"")
}

/// The `hooks.<event>` array, checked to contain only objects. A `Result<T,
/// String>` is "a T, or an error message"; callers use `?` to pass it upward.
fn groups_of(hooks: &Map<String, Value>, event: &str) -> Result<Vec<Value>, String> {
    match hooks.get(event) {
        None => Ok(Vec::new()),
        Some(Value::Array(groups)) if groups.iter().all(Value::is_object) => Ok(groups.clone()),
        Some(_) => Err(format!("hooks.{event} is not an array of groups")),
    }
}

/// `settings.hooks` as a mutable map, `None` when absent.
fn hooks_mut(settings: &mut Map<String, Value>) -> Result<Option<&mut Map<String, Value>>, String> {
    match settings.get_mut("hooks") {
        None => Ok(None),
        Some(Value::Object(map)) => Ok(Some(map)),
        Some(_) => Err("\"hooks\" is not an object".to_string()),
    }
}

/// Remove our entries. Only groups we actually removed from can be dropped when
/// they end up empty; foreign groups (even empty ones) are left alone. An event
/// key is dropped only if we emptied it. `hooks` itself is dropped only if we
/// emptied it, so a pre-existing `"hooks": {}` stays untouched.
/// Returns the new settings and how many handlers were removed.
pub fn uninstall_hooks(settings: &Value) -> Result<(Value, usize), String> {
    // We work on a copy; the input is never modified.
    let mut out = settings.clone();
    let obj = out.as_object_mut().ok_or("settings is not a JSON object")?;
    let mut removed = 0usize;
    let mut drop_hooks = false;

    if let Some(hooks) = hooks_mut(obj)? {
        let events: Vec<String> = hooks.keys().cloned().collect();
        for event in events {
            let groups = groups_of(hooks, &event)?;
            let mut kept: Vec<Value> = Vec::new();
            for mut group in groups.clone() {
                let Some(handlers) = group.get("hooks").and_then(Value::as_array) else {
                    kept.push(group);
                    continue;
                };
                let remaining: Vec<Value> =
                    handlers.iter().filter(|h| !is_ours(h)).cloned().collect();
                let gone = handlers.len() - remaining.len();
                removed += gone;
                let emptied = gone > 0 && remaining.is_empty();
                group["hooks"] = Value::Array(remaining);
                if emptied {
                    continue; // we emptied it, so drop the whole group
                }
                kept.push(group);
            }
            if kept.is_empty() && !groups.is_empty() {
                // `shift_remove` keeps the order of the other keys.
                hooks.shift_remove(&event);
            } else {
                hooks.insert(event, Value::Array(kept));
            }
        }
        drop_hooks = removed > 0 && hooks.is_empty();
    }
    if drop_hooks {
        obj.shift_remove("hooks");
    }
    Ok((out, removed))
}

/// Idempotent: removes our old entries, then adds one group per event.
pub fn install_hooks(settings: &Value, command: &str) -> Result<Value, String> {
    let (mut clean, _) = uninstall_hooks(settings)?;
    let obj = clean
        .as_object_mut()
        .ok_or("settings is not a JSON object")?;
    let mut hooks = match hooks_mut(obj)? {
        Some(existing) => existing.clone(),
        None => Map::new(),
    };
    for event in EVENTS {
        let mut group = Map::new();
        if !NO_MATCHER.contains(&event) {
            group.insert("matcher".to_string(), json!("*"));
        }
        group.insert("hooks".to_string(), json!([handler_for(event, command)]));
        let mut groups = groups_of(&hooks, event)?;
        groups.push(Value::Object(group));
        hooks.insert(event.to_string(), Value::Array(groups));
    }
    obj.insert("hooks".to_string(), Value::Object(hooks));
    Ok(clean)
}

/// What `status_of` finds in a settings object.
#[derive(Debug, Default, PartialEq)]
pub struct StatusInfo {
    /// Events that contain at least one of our handlers, in file order.
    pub events: Vec<String>,
    /// Distinct commands of our handlers, in file order.
    pub commands: Vec<String>,
    /// Hook kind per event (see `kind_of`).
    pub kinds: HashMap<String, String>,
}

pub fn status_of(settings: &Value) -> Result<StatusInfo, String> {
    let mut info = StatusInfo::default();
    let hooks = match settings.get("hooks") {
        None => return Ok(info),
        Some(Value::Object(map)) => map,
        Some(_) => return Err("\"hooks\" is not an object".to_string()),
    };
    for event in hooks.keys() {
        let mut found = false;
        for group in groups_of(hooks, event)? {
            let Some(handlers) = group.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            for h in handlers.iter().filter(|h| is_ours(h)) {
                found = true;
                let command = h["command"].as_str().unwrap_or_default().to_string();
                if !info.commands.contains(&command) {
                    info.commands.push(command);
                }
                info.kinds.insert(event.clone(), kind_of(h));
            }
        }
        if found {
            info.events.push(event.clone());
        }
    }
    Ok(info)
}

// ---------------------------------------------------------------------------
// settings.json file handling
// ---------------------------------------------------------------------------

/// `$CLAUDE_SETTINGS_PATH`, else `~/.claude/settings.json`.
pub fn default_settings_path() -> Option<PathBuf> {
    match std::env::var("CLAUDE_SETTINGS_PATH") {
        Ok(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => dirs::home_dir().map(|h| h.join(".claude").join("settings.json")),
    }
}

/// `{}` for a missing or empty file; an error (and nothing changed) when the
/// file is not valid JSON or not a JSON object.
pub fn read_settings(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text); // UTF-8 BOM
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    let parsed: Value = serde_json::from_str(text).map_err(|e| {
        format!(
            "{} is not valid JSON ({e}). Nothing was changed; fix or remove the file first.",
            path.display()
        )
    })?;
    if !parsed.is_object() {
        return Err(format!(
            "{} does not contain a JSON object. Nothing was changed.",
            path.display()
        ));
    }
    Ok(parsed)
}

/// Current UTC time as `YYYYMMDDHHMMSS`.
fn utc_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Copies the file next to itself; returns the backup path (None if no file).
pub fn backup(path: &Path) -> Result<Option<PathBuf>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let base = format!("{}.agentwatch-backup-{}", path.display(), utc_timestamp());
    // Never overwrite an earlier backup made in the same second.
    let mut dest = PathBuf::from(&base);
    let mut n = 1;
    while dest.exists() {
        dest = PathBuf::from(format!("{base}-{n}"));
        n += 1;
    }
    fs::copy(path, &dest).map_err(|e| format!("cannot back up {}: {e}", path.display()))?;
    Ok(Some(dest))
}

/// Write to a temp file in the same folder, then rename over the target, so a
/// crash can never leave a half-written settings file.
pub fn write_atomic(path: &Path, data: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let tmp = PathBuf::from(format!(
        "{}.agentwatch-tmp-{}",
        path.display(),
        std::process::id()
    ));
    let mut text = serde_json::to_string_pretty(data).map_err(|e| e.to_string())?;
    text.push('\n');
    fs::write(&tmp, text).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("cannot replace {}: {e}", path.display())
    })
}

// ---------------------------------------------------------------------------
// The hook binary
// ---------------------------------------------------------------------------

/// Where the stable copy of the hook lives: `<data_dir>/bin/agentwatch-hook`.
pub fn installed_hook_path(data_dir: &Path) -> PathBuf {
    data_dir.join("bin").join(hook_file_name())
}

/// Locates the hook binary shipped with the app (the source for the copy).
pub fn find_bundled_hook() -> Result<PathBuf, String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    // Compile-time path of this crate, for `cargo run` / `tauri dev`.
    let dev_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("hook");
    find_hook_in(
        std::env::var("AGENTWATCH_HOOK_BIN").ok(),
        exe_dir.as_deref(),
        &dev_root,
    )
}

/// Search order: explicit override, next to the app exe (Tauri puts sidecars
/// there), then the hook crate's build output.
fn find_hook_in(
    env_override: Option<String>,
    exe_dir: Option<&Path>,
    dev_root: &Path,
) -> Result<PathBuf, String> {
    let name = hook_file_name();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = env_override.filter(|p| !p.is_empty()) {
        candidates.push(PathBuf::from(p));
    }
    if let Some(dir) = exe_dir {
        candidates.push(dir.join(&name));
    }
    candidates.push(dev_root.join("target").join("release").join(&name));
    candidates.push(dev_root.join("target").join("debug").join(&name));
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| {
            format!("the {name} binary was not found next to the app. Reinstall agentwatch, or build it with `cargo build --release --manifest-path hook/Cargo.toml`.")
        })
}

/// Copies `source` to `<dest_dir>/agentwatch-hook(.exe)`. Skips the copy when
/// the bytes are identical. A different existing file is renamed aside first,
/// because on Windows a running exe cannot be overwritten but can be renamed.
pub fn install_binary(source: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(dest_dir)
        .map_err(|e| format!("cannot create {}: {e}", dest_dir.display()))?;
    let dest = dest_dir.join(hook_file_name());
    if dest.exists() {
        let same = matches!((fs::read(source), fs::read(&dest)), (Ok(a), Ok(b)) if a == b);
        if same {
            remove_stale_old_files(dest_dir);
            return Ok(dest);
        }
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let aside = dest_dir.join(format!("{}.old-{millis}", hook_file_name()));
        fs::rename(&dest, &aside).map_err(|e| format!("cannot replace {}: {e}", dest.display()))?;
    }
    fs::copy(source, &dest)
        .map_err(|e| format!("cannot copy the hook to {}: {e}", dest.display()))?;
    make_executable(&dest);
    remove_stale_old_files(dest_dir);
    Ok(dest)
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// Best effort: delete `agentwatch-hook*.old-*` files. One still in use on
/// Windows fails to delete; that is fine, a later run retries.
fn remove_stale_old_files(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(MARKER) && name.contains(".old-") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Best effort: delete everything named `agentwatch-hook*` in `<data_dir>/bin`.
fn remove_installed_binaries(data_dir: &Path) {
    let dir = data_dir.join("bin");
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(MARKER) {
            let _ = fs::remove_file(entry.path());
        }
    }
    let _ = fs::remove_dir(&dir); // only succeeds when empty
}

// ---------------------------------------------------------------------------
// Connect / disconnect / status
// ---------------------------------------------------------------------------

/// What the UI shows. Serialised to the frontend as-is.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HooksStatus {
    /// Our hook entries exist in settings.json.
    pub connected: bool,
    /// Events that have our entries.
    pub events: Vec<String>,
    /// The command of the first entry, if any.
    pub command: Option<String>,
    /// Entries exist but are not (fully) in the current form: they point
    /// somewhere other than our stable binary copy (e.g. the old Bun shim),
    /// the copy is missing, or some events are missing. Connect fixes it.
    pub stale: bool,
    pub settings_path: String,
}

pub fn status(settings_path: &Path, data_dir: &Path) -> Result<HooksStatus, String> {
    let info = status_of(&read_settings(settings_path)?)?;
    let installed = installed_hook_path(data_dir);
    let expected = build_command(&installed);
    let connected = !info.events.is_empty();
    let stale = connected
        && (info.commands.iter().any(|c| *c != expected)
            || !installed.is_file()
            || info.events.len() != EVENTS.len());
    Ok(HooksStatus {
        connected,
        command: info.commands.first().cloned(),
        events: info.events,
        stale,
        settings_path: settings_path.display().to_string(),
    })
}

/// Copies the hook to its stable place and writes our entries to settings.json
/// (backing it up first). `source` is the bundled hook binary.
pub fn connect(
    settings_path: &Path,
    data_dir: &Path,
    source: &Path,
) -> Result<HooksStatus, String> {
    // Read first: an invalid settings file aborts before anything is changed.
    let current = read_settings(settings_path)?;
    let installed = install_binary(source, &data_dir.join("bin"))?;
    let next = install_hooks(&current, &build_command(&installed))?;
    let unchanged = serde_json::to_string(&next).ok() == serde_json::to_string(&current).ok();
    if !(unchanged && settings_path.exists()) {
        backup(settings_path)?;
        write_atomic(settings_path, &next)?;
    }
    status(settings_path, data_dir)
}

/// Removes our entries (including old Bun-shim ones) and deletes the binary
/// copy. Foreign hooks are left exactly as they were.
pub fn disconnect(settings_path: &Path, data_dir: &Path) -> Result<HooksStatus, String> {
    if settings_path.exists() {
        let (next, removed) = uninstall_hooks(&read_settings(settings_path)?)?;
        if removed > 0 {
            backup(settings_path)?;
            write_atomic(settings_path, &next)?;
        }
    }
    remove_installed_binaries(data_dir);
    status(settings_path, data_dir)
}

/// App start: when our hooks point at the stable copy, refresh the copy so an
/// app update also updates the hook. Never edits settings.json.
pub fn refresh_on_start(
    settings_path: &Path,
    data_dir: &Path,
    source: &Path,
) -> Result<(), String> {
    let info = status_of(&read_settings(settings_path)?)?;
    let expected = build_command(&installed_hook_path(data_dir));
    if info.commands.contains(&expected) {
        install_binary(source, &data_dir.join("bin"))?;
    }
    Ok(())
}

/// `agentwatch --disconnect-hooks` (called by the Windows uninstaller). Never
/// panics and ignores every error: an uninstaller must not fail on this.
pub fn disconnect_cli() {
    let _ = std::panic::catch_unwind(|| {
        if let (Some(settings), Some(dir)) =
            (default_settings_path(), crate::endpoint::default_dir())
        {
            let _ = disconnect(&settings, &dir);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const CMD: &str = "\"C:/Users/x/.agentwatch/bin/agentwatch-hook.exe\"";

    fn foreign() -> Value {
        json!({
            "theme": "dark",
            "permissions": { "allow": ["Bash(ls)"] },
            "hooks": {
                "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "lint.sh" }] }],
                "Stop": [{ "hooks": [{ "type": "command", "command": "notify.sh" }] }],
                "CwdChanged": [{ "hooks": [{ "type": "command", "command": "cwd.sh" }] }]
            }
        })
    }

    fn install(v: &Value) -> Value {
        install_hooks(v, CMD).unwrap()
    }

    fn keys(v: &Value) -> Vec<String> {
        v.as_object().unwrap().keys().cloned().collect()
    }

    #[test]
    fn install_into_empty_settings() {
        let out = install(&json!({}));
        assert_eq!(keys(&out["hooks"]), EVENTS);
        assert_eq!(
            out["hooks"]["PreToolUse"],
            json!([{ "matcher": "*", "hooks": [{ "type": "command", "command": CMD, "async": true, "timeout": 5 }] }])
        );
        assert_eq!(
            out["hooks"]["Stop"],
            json!([{ "hooks": [{ "type": "command", "command": CMD, "asyncRewake": true }] }])
        );
        assert_eq!(
            out["hooks"]["PermissionRequest"],
            json!([{ "matcher": "*", "hooks": [{ "type": "command", "command": CMD, "timeout": 130 }] }])
        );
        for e in EVENTS {
            if e == "Stop" || e == "PermissionRequest" {
                continue;
            }
            assert_eq!(
                out["hooks"][e][0]["hooks"],
                json!([{ "type": "command", "command": CMD, "async": true, "timeout": 5 }])
            );
        }
        let kinds = status_of(&out).unwrap().kinds;
        assert_eq!(kinds["PermissionRequest"], "sync 130s");
        assert_eq!(kinds["Stop"], "asyncRewake");
        assert_eq!(kinds["PreToolUse"], "async");
        assert!(out["hooks"]["UserPromptSubmit"][0].get("matcher").is_none());
    }

    #[test]
    fn handler_key_order_is_type_command_then_flags() {
        let out = install(&json!({}));
        let handler = &out["hooks"]["PreToolUse"][0]["hooks"][0];
        assert_eq!(keys(handler), ["type", "command", "async", "timeout"]);
    }

    #[test]
    fn merge_keeps_foreign_hooks_and_top_level_keys() {
        let f = foreign();
        let out = install(&f);
        assert_eq!(out["theme"], "dark");
        assert_eq!(out["permissions"], f["permissions"]);
        assert_eq!(out["hooks"]["CwdChanged"], f["hooks"]["CwdChanged"]);
        assert_eq!(out["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(out["hooks"]["PreToolUse"][0], f["hooks"]["PreToolUse"][0]);
        assert_eq!(out["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(out["hooks"]["Stop"][0], f["hooks"]["Stop"][0]);
    }

    #[test]
    fn key_order_of_user_settings_is_preserved() {
        let f = foreign();
        let out = install(&f);
        assert_eq!(keys(&out), ["theme", "permissions", "hooks"]);
        // Existing events keep their place; new ones come after.
        assert_eq!(
            &keys(&out["hooks"])[..3],
            ["PreToolUse", "Stop", "CwdChanged"]
        );
    }

    #[test]
    fn migrates_an_old_install_without_duplicates() {
        let mut old = Map::new();
        for e in EVENTS {
            let mut groups: Vec<Value> = Vec::new();
            if e == "Stop" {
                groups.push(foreign()["hooks"]["Stop"][0].clone());
            }
            let mut g = Map::new();
            if e != "UserPromptSubmit" && e != "Stop" {
                g.insert("matcher".into(), json!("*"));
            }
            g.insert(
                "hooks".into(),
                json!([{ "type": "command", "command": CMD, "async": true, "timeout": 5 }]),
            );
            groups.push(Value::Object(g));
            old.insert(e.to_string(), Value::Array(groups));
        }
        let out = install(&json!({ "hooks": Value::Object(old) }));
        assert_eq!(status_of(&out).unwrap().commands, [CMD]);
        assert_eq!(
            out["hooks"]["PermissionRequest"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            out["hooks"]["PermissionRequest"][0]["hooks"][0],
            json!({ "type": "command", "command": CMD, "timeout": 130 })
        );
        assert_eq!(out["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(out["hooks"]["Stop"][0], foreign()["hooks"]["Stop"][0]);
        assert_eq!(
            out["hooks"]["Stop"][1]["hooks"][0],
            json!({ "type": "command", "command": CMD, "asyncRewake": true })
        );
        assert_eq!(install(&out), out);
    }

    #[test]
    fn old_bun_shim_entries_are_replaced() {
        let bun = "\"C:/bun/bun.exe\" \"C:/x/shim/agentwatch-hook.ts\"";
        let old = install_hooks(&json!({}), bun).unwrap();
        let info = status_of(&old).unwrap();
        assert_eq!(info.commands, [bun]);
        let new = install(&old);
        assert_eq!(status_of(&new).unwrap().commands, [CMD]);
        let (clean, removed) = uninstall_hooks(&old).unwrap();
        assert_eq!(removed, EVENTS.len());
        assert_eq!(clean, json!({}));
    }

    #[test]
    fn install_twice_is_idempotent() {
        let once = install(&foreign());
        assert_eq!(install(&once), once);
        // Even with a changed command, still one entry per event.
        let moved = install_hooks(&once, "\"D:/other/agentwatch-hook.exe\"").unwrap();
        assert_eq!(status_of(&moved).unwrap().commands.len(), 1);
        assert_eq!(moved["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn uninstall_removes_only_ours_and_round_trips() {
        let installed = install(&foreign());
        let (settings, removed) = uninstall_hooks(&installed).unwrap();
        assert_eq!(removed, EVENTS.len());
        assert_eq!(settings, foreign());
        // Order too, not just content.
        assert_eq!(settings.to_string(), foreign().to_string());
    }

    #[test]
    fn uninstall_drops_emptied_hooks_but_keeps_a_pre_existing_empty_one() {
        let installed = install(&json!({ "a": 1 }));
        assert_eq!(uninstall_hooks(&installed).unwrap().0, json!({ "a": 1 }));
        let untouched = json!({ "hooks": {} });
        assert_eq!(uninstall_hooks(&untouched).unwrap(), (untouched.clone(), 0));
    }

    #[test]
    fn a_mixed_group_only_loses_our_hook() {
        let s = json!({ "hooks": { "Stop": [{ "hooks": [
            { "type": "command", "command": "mine.sh" },
            { "type": "command", "command": CMD }
        ] }] } });
        assert_eq!(
            uninstall_hooks(&s).unwrap().0,
            json!({ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "mine.sh" }] }] } })
        );
    }

    #[test]
    fn malformed_hooks_shapes_are_errors() {
        assert!(install_hooks(&json!({ "hooks": [] }), CMD).is_err());
        assert!(install_hooks(&json!({ "hooks": { "Stop": "x" } }), CMD).is_err());
        assert!(uninstall_hooks(&json!([1])).is_err());
    }

    // ----- file handling -----

    fn read(p: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    /// A fake "bundled hook" file with the given contents.
    fn fake_bin(dir: &Path, contents: &str) -> PathBuf {
        let p = dir.join("source-hook");
        fs::write(&p, contents).unwrap();
        p
    }

    struct Env {
        _tmp: TempDir,
        settings: PathBuf,
        data: PathBuf,
        source: PathBuf,
    }

    fn env() -> Env {
        let tmp = TempDir::new().unwrap();
        let settings = tmp.path().join("claude").join("settings.json");
        let data = tmp.path().join("agentwatch");
        let source = fake_bin(tmp.path(), "binary v1");
        Env {
            _tmp: tmp,
            settings,
            data,
            source,
        }
    }

    #[test]
    fn connect_creates_a_missing_file_and_copies_the_binary() {
        let e = env();
        let st = connect(&e.settings, &e.data, &e.source).unwrap();
        assert!(st.connected && !st.stale);
        assert_eq!(st.events, EVENTS);
        let installed = installed_hook_path(&e.data);
        assert_eq!(fs::read_to_string(&installed).unwrap(), "binary v1");
        assert_eq!(st.command, Some(build_command(&installed)));
        assert!(st.command.unwrap().starts_with('"'));
        // No file existed, so no backup.
        assert_eq!(file_names(e.settings.parent().unwrap()), ["settings.json"]);
    }

    #[test]
    fn connect_backs_up_and_disconnect_restores_foreign_content() {
        let e = env();
        fs::create_dir_all(e.settings.parent().unwrap()).unwrap();
        fs::write(&e.settings, foreign().to_string()).unwrap();
        connect(&e.settings, &e.data, &e.source).unwrap();
        let names = file_names(e.settings.parent().unwrap());
        let backups: Vec<&String> = names
            .iter()
            .filter(|n| n.starts_with("settings.json.agentwatch-backup-"))
            .collect();
        assert_eq!(backups.len(), 1);
        let stamp = backups[0].trim_start_matches("settings.json.agentwatch-backup-");
        assert_eq!(stamp.len(), 14);
        assert!(stamp.chars().all(|c| c.is_ascii_digit()));
        assert_eq!(read(&e.settings.with_file_name(backups[0])), foreign());
        assert!(!names.iter().any(|n| n.contains("tmp")));

        let st = disconnect(&e.settings, &e.data).unwrap();
        assert!(!st.connected);
        assert_eq!(read(&e.settings), foreign());
        assert!(!installed_hook_path(&e.data).exists());
    }

    #[test]
    fn connect_twice_changes_and_backs_up_nothing_the_second_time() {
        let e = env();
        connect(&e.settings, &e.data, &e.source).unwrap();
        let before = file_names(e.settings.parent().unwrap());
        connect(&e.settings, &e.data, &e.source).unwrap();
        assert_eq!(file_names(e.settings.parent().unwrap()), before);
    }

    #[test]
    fn invalid_json_aborts_without_writing_anything() {
        let e = env();
        fs::create_dir_all(e.settings.parent().unwrap()).unwrap();
        fs::write(&e.settings, "{ not json").unwrap();
        assert!(connect(&e.settings, &e.data, &e.source).is_err());
        assert!(disconnect(&e.settings, &e.data).is_err());
        assert_eq!(fs::read_to_string(&e.settings).unwrap(), "{ not json");
        assert_eq!(file_names(e.settings.parent().unwrap()), ["settings.json"]);
        assert!(!e.data.exists());
        // A non-object is rejected too.
        fs::write(&e.settings, "[1]").unwrap();
        assert!(read_settings(&e.settings).is_err());
    }

    #[test]
    fn bom_and_empty_files_are_handled() {
        let e = env();
        fs::create_dir_all(e.settings.parent().unwrap()).unwrap();
        fs::write(&e.settings, "\u{feff}{\"a\":1}").unwrap();
        assert_eq!(read_settings(&e.settings).unwrap(), json!({ "a": 1 }));
        fs::write(&e.settings, "  \n").unwrap();
        assert_eq!(read_settings(&e.settings).unwrap(), json!({}));
    }

    #[test]
    fn disconnect_with_nothing_installed_changes_and_backs_up_nothing() {
        let e = env();
        fs::create_dir_all(e.settings.parent().unwrap()).unwrap();
        fs::write(&e.settings, foreign().to_string()).unwrap();
        disconnect(&e.settings, &e.data).unwrap();
        assert_eq!(file_names(e.settings.parent().unwrap()), ["settings.json"]);
        // And a missing settings file is fine.
        let missing = e.data.join("nope.json");
        assert!(!disconnect(&missing, &e.data).unwrap().connected);
    }

    #[test]
    fn status_reports_stale_for_old_bun_shim_entries() {
        let e = env();
        let bun = "\"C:/bun/bun.exe\" \"C:/x/shim/agentwatch-hook.ts\"";
        let old = install_hooks(&json!({}), bun).unwrap();
        write_atomic(&e.settings, &old).unwrap();
        let st = status(&e.settings, &e.data).unwrap();
        assert!(st.connected && st.stale);
        assert_eq!(st.command.as_deref(), Some(bun));
        // Connect migrates it.
        let st = connect(&e.settings, &e.data, &e.source).unwrap();
        assert!(st.connected && !st.stale);
        // Deleting the copy makes it stale again.
        fs::remove_file(installed_hook_path(&e.data)).unwrap();
        assert!(status(&e.settings, &e.data).unwrap().stale);
    }

    #[test]
    fn status_when_not_connected() {
        let e = env();
        let st = status(&e.settings, &e.data).unwrap();
        assert!(!st.connected && !st.stale && st.events.is_empty() && st.command.is_none());
    }

    #[test]
    fn install_binary_skips_identical_and_replaces_different() {
        let e = env();
        let dir = e.data.join("bin");
        let dest = install_binary(&e.source, &dir).unwrap();
        let first = fs::metadata(&dest).unwrap().modified().unwrap();
        install_binary(&e.source, &dir).unwrap();
        assert_eq!(fs::metadata(&dest).unwrap().modified().unwrap(), first);
        assert_eq!(file_names(&dir).len(), 1);

        // A different binary: the old one is renamed aside, then cleaned up on
        // the next call (best effort).
        let v2 = fake_bin(e._tmp.path(), "binary v2");
        install_binary(&v2, &dir).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "binary v2");
        assert!(file_names(&dir).iter().all(|n| !n.contains(".old-")));
    }

    #[test]
    fn stale_old_files_are_deleted_and_disconnect_removes_all_hook_files() {
        let e = env();
        let dir = e.data.join("bin");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{MARKER}.old-1")), "x").unwrap();
        connect(&e.settings, &e.data, &e.source).unwrap();
        assert_eq!(file_names(&dir), [hook_file_name()]);
        fs::write(dir.join(format!("{MARKER}.old-2")), "x").unwrap();
        disconnect(&e.settings, &e.data).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn refresh_on_start_updates_the_copy_only_when_connected() {
        let e = env();
        connect(&e.settings, &e.data, &e.source).unwrap();
        let settings_before = fs::read_to_string(&e.settings).unwrap();
        let v2 = fake_bin(e._tmp.path(), "binary v2");
        refresh_on_start(&e.settings, &e.data, &v2).unwrap();
        assert_eq!(
            fs::read_to_string(installed_hook_path(&e.data)).unwrap(),
            "binary v2"
        );
        assert_eq!(fs::read_to_string(&e.settings).unwrap(), settings_before);

        // Not connected: nothing is copied and settings stay untouched.
        disconnect(&e.settings, &e.data).unwrap();
        refresh_on_start(&e.settings, &e.data, &v2).unwrap();
        assert!(!installed_hook_path(&e.data).exists());

        // Old Bun entries (pointing elsewhere): nothing is copied either.
        let bun = "\"C:/bun/bun.exe\" \"C:/x/shim/agentwatch-hook.ts\"";
        write_atomic(&e.settings, &install_hooks(&json!({}), bun).unwrap()).unwrap();
        refresh_on_start(&e.settings, &e.data, &v2).unwrap();
        assert!(!installed_hook_path(&e.data).exists());
    }

    #[test]
    fn find_hook_search_order() {
        let tmp = TempDir::new().unwrap();
        let name = hook_file_name();
        let dev = tmp.path().join("hook");
        let exe_dir = tmp.path().join("app");
        fs::create_dir_all(dev.join("target").join("debug")).unwrap();
        fs::create_dir_all(&exe_dir).unwrap();
        assert!(find_hook_in(None, Some(&exe_dir), &dev).is_err());

        let debug = dev.join("target").join("debug").join(&name);
        fs::write(&debug, "d").unwrap();
        assert_eq!(find_hook_in(None, Some(&exe_dir), &dev).unwrap(), debug);

        let beside = exe_dir.join(&name);
        fs::write(&beside, "b").unwrap();
        assert_eq!(find_hook_in(None, Some(&exe_dir), &dev).unwrap(), beside);

        let over = tmp.path().join("override-bin");
        fs::write(&over, "o").unwrap();
        let got = find_hook_in(Some(over.display().to_string()), Some(&exe_dir), &dev);
        assert_eq!(got.unwrap(), over);
        // A bad override falls through to the next candidate.
        let got = find_hook_in(Some("/no/such".into()), Some(&exe_dir), &dev);
        assert_eq!(got.unwrap(), beside);
    }

    #[test]
    fn build_command_quotes_and_uses_forward_slashes() {
        assert_eq!(
            build_command(Path::new(
                r"C:\Users\Jo Doe\.agentwatch\bin\agentwatch-hook.exe"
            )),
            "\"C:/Users/Jo Doe/.agentwatch/bin/agentwatch-hook.exe\""
        );
    }

    #[test]
    fn timestamp_has_the_right_shape() {
        let t = utc_timestamp();
        assert_eq!(t.len(), 14);
        assert!(t.starts_with("20"));
    }
}
