//! Storage shared by the per-user libraries kept outside projects (the Font Library and the
//! Colour Palette Library): one data directory, JSON manifests written atomically, and SHA-256
//! fingerprints. A damaged manifest is an error, never an empty library, so nothing overwrites it.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use crate::Session;

/// The data directory the libraries live in: the session's override (tests, the CLI's
/// `--data-dir`), else the folder of the preferences file, else the default data directory.
pub fn data_dir(s: &Session) -> Option<PathBuf> {
    if let Some(d) = &s.library_data_dir {
        return Some(d.clone());
    }
    s.prefs_path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).or_else(crate::autosave::default_data_dir)
}

/// Write `bytes` to `path` through a temporary file and a rename, so a crash never leaves a
/// half-written file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write {}: {e}", path.display())
    })
}

/// Read a JSON manifest: `Ok(None)` when it does not exist, an error when it cannot be read or
/// parsed (`what` names it in the message).
pub fn read_json<T: DeserializeOwned>(path: &Path, what: &str) -> Result<Option<T>, String> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).map(Some).map_err(|e| format!("the {what} {} is damaged: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// Write a JSON manifest atomically, creating its folder.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let b = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    write_atomic(path, &b)
}

/// Lower-case hex SHA-256 of `b`.
pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// The current time as RFC 3339 UTC, to the second.
pub fn now_rfc3339() -> String {
    chrono::DateTime::from_timestamp(crate::autosave::unix_now(), 0).map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap_or_default()
}
