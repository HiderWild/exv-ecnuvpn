
//! Config-directory and file-path resolution.
//!
//! Defaults to `%USERPROFILE%\.exv` on Windows (falling back to `HOME` for
//! non-Windows builds/tests), overridable via the `EXV_CONFIG_DIR` env var.
//! The layout mirrors the C++ `~/.exv/` credential home:
//!
//! - `<dir>/config.json` — user config (server/username/password/routes/...)
//! - `<dir>/key.bin`     — 32-byte AES key, key-separation file

use std::path::{Path, PathBuf};

/// Name of the user configuration file.
pub const CONFIG_FILE: &str = "config.json";
/// Name of the 32-byte AES key file.
pub const KEY_FILE: &str = "key.bin";
/// Directory under the user home where EXV keeps config and credentials.
pub const DEFAULT_CONFIG_SUBDIR: &str = ".exv";

/// Resolve the config directory, honoring the `EXV_CONFIG_DIR` override.
///
/// Order of precedence: `EXV_CONFIG_DIR` env var, then `USERPROFILE` (Windows),
/// then `HOME` (POSIX), then a relative `./.exv` as a last resort.
#[must_use]
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("EXV_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(profile) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
        return PathBuf::from(profile).join(DEFAULT_CONFIG_SUBDIR);
    }
    if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(home).join(DEFAULT_CONFIG_SUBDIR);
    }
    PathBuf::from(DEFAULT_CONFIG_SUBDIR)
}

/// Path to `config.json` within `dir`.
#[must_use]
pub fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE)
}

/// Path to `key.bin` within `dir`.
#[must_use]
pub fn key_path(dir: &Path) -> PathBuf {
    dir.join(KEY_FILE)
}

