//! The files the hook shim reads to find us: `~/.agentwatch/token` (generated
//! once, stable) and `~/.agentwatch/endpoint.json` (rewritten on every start,
//! removed on clean exit). Every function takes the directory as a parameter
//! so tests can use a temp dir instead of the real home directory.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const TOKEN_FILE: &str = "token";
const ENDPOINT_FILE: &str = "endpoint.json";

/// Contents of `endpoint.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

/// `~/.agentwatch`, or `None` if the home directory cannot be determined.
pub fn default_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".agentwatch"))
}

/// A fresh random token: 32 random bytes as 64 hex characters.
fn generate_token() -> String {
    let bytes: [u8; 32] = rand::random();
    // `{:02x}` prints each byte as two lowercase hex digits.
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Read the saved token, or create and save a new one.
///
/// The return type `io::Result<String>` means "a String, or an I/O error".
/// The `?` operator below returns early with the error if a step fails, so
/// we don't need nested `match` blocks.
pub fn load_or_create_token(dir: &Path) -> io::Result<String> {
    fs::create_dir_all(dir)?;
    let path = dir.join(TOKEN_FILE);
    if let Ok(existing) = fs::read_to_string(&path) {
        let existing = existing.trim().to_string();
        // Only trust a well-formed token; otherwise regenerate.
        if existing.len() == 64 && existing.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(existing);
        }
    }
    let token = generate_token();
    fs::write(&path, &token)?;
    restrict_permissions(&path);
    Ok(token)
}

/// On Unix make the file readable by the owner only. Best effort: a failure
/// here must not stop the app, so the result is ignored on purpose.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

/// Write `endpoint.json`. Writes a temp file first and renames it, so the
/// shim never reads a half-written file.
pub fn write_endpoint(dir: &Path, port: u16, token: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let endpoint = Endpoint {
        port,
        token: token.to_string(),
        pid: std::process::id(),
    };
    let json = serde_json::to_string(&endpoint).map_err(io::Error::other)?;
    let tmp = dir.join(format!("{ENDPOINT_FILE}.tmp"));
    fs::write(&tmp, json)?;
    restrict_permissions(&tmp);
    fs::rename(&tmp, dir.join(ENDPOINT_FILE))
}

/// Delete `endpoint.json` (called on clean exit), but only if it still
/// belongs to this process: a second instance may have overwritten it.
/// A missing file is fine.
pub fn remove_endpoint(dir: &Path) {
    match read_endpoint(dir) {
        Some(e) if e.pid != std::process::id() => {}
        _ => {
            let _ = fs::remove_file(dir.join(ENDPOINT_FILE));
        }
    }
}

pub fn read_endpoint(dir: &Path) -> Option<Endpoint> {
    let raw = fs::read_to_string(dir.join(ENDPOINT_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_created_once_and_reused() {
        let tmp = tempfile::tempdir().unwrap();
        let first = load_or_create_token(tmp.path()).unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        let second = load_or_create_token(tmp.path()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn malformed_token_file_is_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(TOKEN_FILE), "short").unwrap();
        let token = load_or_create_token(tmp.path()).unwrap();
        assert_eq!(token.len(), 64);
        assert_eq!(
            fs::read_to_string(tmp.path().join(TOKEN_FILE)).unwrap(),
            token
        );
    }

    #[test]
    fn endpoint_file_roundtrip_and_removal() {
        let tmp = tempfile::tempdir().unwrap();
        write_endpoint(tmp.path(), 4242, "tok").unwrap();
        let e = read_endpoint(tmp.path()).unwrap();
        assert_eq!(e.port, 4242);
        assert_eq!(e.token, "tok");
        assert_eq!(e.pid, std::process::id());
        // The raw file has exactly the documented keys.
        let raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(tmp.path().join(ENDPOINT_FILE)).unwrap())
                .unwrap();
        assert!(
            raw.get("port").is_some() && raw.get("token").is_some() && raw.get("pid").is_some()
        );

        remove_endpoint(tmp.path());
        assert!(read_endpoint(tmp.path()).is_none());
        // Removing again is harmless.
        remove_endpoint(tmp.path());
    }

    #[test]
    fn creates_missing_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a").join("b");
        write_endpoint(&nested, 1, "t").unwrap();
        assert!(read_endpoint(&nested).is_some());
    }
}
