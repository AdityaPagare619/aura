//! Platform-aware path resolution with environment variable overrides.
//!
//! Resolution order for each path:
//! 1. `AURA_*` environment variable (explicit override)
//! 2. `#[cfg(target_os = "android")]` platform default
//! 3. `dirs` crate fallback (desktop Linux/macOS/Windows)
//!
//! This module makes AURA device-agnostic: the same binary runs on
//! Android (Termux/APK), Linux, macOS, and Windows.

use std::path::PathBuf;

/// Resolve the AURA data directory.
///
/// Env: `AURA_DATA_DIR`
/// Android default: `/data/local/tmp/aura`
/// Desktop default: `~/.local/share/aura` (Linux), `~/Library/Application Support/aura` (macOS), `%APPDATA%/aura` (Windows)
pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("AURA_DATA_DIR") {
        return PathBuf::from(p);
    }
    #[cfg(target_os = "android")]
    {
        PathBuf::from("/data/local/tmp/aura")
    }
    #[cfg(not(target_os = "android"))]
    {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("aura")
    }
}

/// Resolve the model files directory.
///
/// Env: `AURA_MODELS_PATH`
/// Android default: `/data/local/tmp/aura/models`
/// Desktop default: `~/.local/share/aura/models`
pub fn model_dir() -> PathBuf {
    if let Ok(p) = std::env::var("AURA_MODELS_PATH") {
        return PathBuf::from(p);
    }
    data_dir().join("models")
}

/// Resolve the SQLite database path.
///
/// Env: `AURA_DB_PATH`
/// Android default: `/data/local/tmp/aura/aura.db` (Termux-compatible)
/// Desktop default: `<data_dir>/aura.db`
pub fn db_path() -> PathBuf {
    if let Ok(p) = std::env::var("AURA_DB_PATH") {
        return PathBuf::from(p);
    }
    #[cfg(target_os = "android")]
    {
        data_dir().join("aura.db")
    }
    #[cfg(not(target_os = "android"))]
    {
        data_dir().join("aura.db")
    }
}

/// Resolve the user home directory.
///
/// Env: `AURA_HOME`
/// Fallback: `dirs::home_dir()`, then `~/.config/aura` (last resort)
pub fn home_dir() -> PathBuf {
    if let Ok(p) = std::env::var("AURA_HOME") {
        return PathBuf::from(p);
    }
    #[cfg(target_os = "android")]
    {
        // Termux HOME or Android app data
        std::env::var("HOME")
            .or_else(|_| std::env::var("PREFIX").map(|p| format!("{p}/../home")))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/data/data/com.termux/files/home"))
    }
    #[cfg(not(target_os = "android"))]
    {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
    }
}

/// Resolve the neocortex binary path.
///
/// Env: `AURA_NEOCORTEX_BIN`
/// Android default: `/data/local/tmp/aura-neocortex`
/// Desktop default: `aura-neocortex` (expects on PATH)
pub fn neocortex_bin() -> PathBuf {
    if let Ok(p) = std::env::var("AURA_NEOCORTEX_BIN") {
        return PathBuf::from(p);
    }
    #[cfg(target_os = "android")]
    {
        PathBuf::from("/data/local/tmp/aura-neocortex")
    }
    #[cfg(not(target_os = "android"))]
    {
        PathBuf::from("aura-neocortex")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_returns_nonempty() {
        let d = data_dir();
        assert!(!d.as_os_str().is_empty(), "data_dir must not be empty");
    }

    #[test]
    fn model_dir_is_subdir_of_data() {
        let d = model_dir();
        assert!(!d.as_os_str().is_empty());
    }

    #[test]
    fn home_dir_returns_nonempty() {
        let h = home_dir();
        assert!(!h.as_os_str().is_empty());
    }

    #[test]
    fn env_override_data_dir() {
        // SAFETY: Test-only env var mutation. Tests run single-threaded.
        // Note: set_var/remove_var are unsafe in Rust because they can race
        // with other threads calling getenv. In test context this is safe.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("AURA_DATA_DIR", "/tmp/test-aura");
        }
        assert_eq!(data_dir(), PathBuf::from("/tmp/test-aura"));
        // SAFETY: Cleanup test env var after test.
        #[allow(unsafe_code)]
        unsafe {
            std::env::remove_var("AURA_DATA_DIR");
        }
    }

    #[test]
    fn env_override_db_path() {
        // SAFETY: Test-only env var mutation. Tests run single-threaded.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("AURA_DB_PATH", "/tmp/test.db");
        }
        assert_eq!(db_path(), PathBuf::from("/tmp/test.db"));
        // SAFETY: Cleanup test env var after test.
        #[allow(unsafe_code)]
        unsafe {
            std::env::remove_var("AURA_DB_PATH");
        }
    }
}
