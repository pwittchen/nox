//! Locations under `~/.nox` (overridable with `NOX_HOME`).

use std::io;
use std::path::{Path, PathBuf};

/// `~/.nox`, or `$NOX_HOME` when set (tests, side-by-side installs).
pub fn nox_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("NOX_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(home);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")).join(".nox")
}

/// `~/.nox/config.toml`.
pub fn config_path() -> PathBuf {
    nox_dir().join("config.toml")
}

/// `~/.nox/themes`.
pub fn themes_dir() -> PathBuf {
    nox_dir().join("themes")
}

/// `~/.nox/icons`.
pub fn icons_dir() -> PathBuf {
    nox_dir().join("icons")
}

/// `~/.nox/run` (control socket, pid files).
pub fn run_dir() -> PathBuf {
    nox_dir().join("run")
}

/// The commented config written on first launch (SPEC §11).
pub const DEFAULT_CONFIG_TOML: &str = include_str!("default_config.toml");

/// Writes the commented default config if `path` doesn't exist yet (creating parent
/// directories). Returns whether the file was written.
pub fn ensure_default_file(path: &Path) -> io::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(DEFAULT_CONFIG_TOML.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}
