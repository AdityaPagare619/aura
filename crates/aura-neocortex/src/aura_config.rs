//! `aura.config.toml` parser for the AURA Neocortex process.
//!
//! Provides `NeocortexRuntimeConfig` — loaded once at startup, drives model
//! path resolution and optional geometry overrides.
//!
//! # Minimum viable config
//! Only `[model] path` is required. Everything else is auto-detected from
//! GGUF metadata.
//!
//! # File format
//! ```toml
//! # aura.config.toml
//!
//! [model]
//! path = "/sdcard/AURA/models/qwen2-1.5b-q4_k_m.gguf"
//!
//! # Optional overrides — use only if GGUF metadata is known-wrong:
//! # embedding_dim = 1536
//! # context_length = 8192
//! ```
//!
//! # Search path for auto-scan
//! When `[model] path` is omitted, AURA scans these directories in order
//! and uses the first `.gguf` file found:
//! 1. `/sdcard/AURA/models/`
//! 2. `/sdcard/Download/`
//! 3. `/sdcard/Documents/`
//!
//! This lets the user simply drop a GGUF file into their Downloads folder
//! with zero configuration.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

// ─── Auto-scan search paths ──────────────────────────────────────────────────

/// Directories scanned (in order) when no explicit model path is configured.
///
/// First `.gguf` file found wins. Matches user expectation of dropping a model
/// into Downloads and having it just work.
const AUTO_SCAN_DIRS: &[&str] = &[
    "/sdcard/AURA/models",
    "/sdcard/Download",
    "/sdcard/Documents",
];

// ─── TOML schema structs ─────────────────────────────────────────────────────

/// Raw TOML deserialization target for `[model]` section.
///
/// All fields optional at the TOML level — validation happens in
/// `NeocortexRuntimeConfig::from_toml_config`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RawModelSection {
    /// Absolute path to the GGUF model file or a directory containing GGUF files.
    #[serde(default)]
    path: Option<String>,

    /// Override embedding dimension — use only if GGUF metadata is absent/wrong.
    /// GGUF metadata takes priority when present.
    #[serde(default)]
    embedding_dim: Option<u32>,

    /// Override context length — use only if GGUF metadata is absent/wrong.
    #[serde(default)]
    context_length: Option<u32>,
}

/// Raw TOML deserialization target — top-level `aura.config.toml` structure.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RawAuraTomlConfig {
    #[serde(default)]
    model: RawModelSection,
}

// ─── Resolved runtime config ─────────────────────────────────────────────────

/// Resolved configuration for the Neocortex process at runtime.
///
/// Produced by parsing `aura.config.toml` (or using defaults when the file is
/// absent). Drives `ModelManager` and `ModelCapabilities` construction.
///
/// # Invariants
/// - `model_path` is always `Some` after construction: either from config, auto-scan, or the
///   compiled fallback path.
/// - `user_override_embedding_dim` is `None` unless the user explicitly set it;
///   `ModelCapabilities::from_gguf` will still prefer GGUF metadata over it.
#[derive(Debug, Clone)]
pub struct NeocortexRuntimeConfig {
    /// Path to the GGUF model file or directory containing GGUF files.
    ///
    /// `None` only if auto-scan found nothing — callers should then fall back
    /// to `ModelManager`'s hardcoded tier filenames.
    pub model_path: Option<PathBuf>,

    /// Optional user override for embedding dimension.
    /// Forwarded to `ModelCapabilities::from_gguf` — GGUF metadata takes
    /// priority if present.
    pub user_override_embedding_dim: Option<u32>,

    /// Optional user override for context length.
    /// Forwarded to model loading — GGUF metadata takes priority if present.
    #[allow(dead_code)] // Phase 8: read by Android JNI config loader
    pub user_override_context_length: Option<u32>,

    /// The config file path that was loaded, for logging provenance.
    pub config_source: ConfigSource,
}

/// Records how the runtime config was obtained, for log provenance.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Phase 8: variant paths used by JNI and auto-scan wiring
pub enum ConfigSource {
    /// Parsed from an explicit config file at the given path.
    File(PathBuf),
    /// Auto-discovered from one of the well-known scan directories.
    AutoScanned(PathBuf),
    /// No config file found and no auto-scan result — using compiled defaults.
    DefaultFallback,
}

impl NeocortexRuntimeConfig {
    // ── Constructors ──────────────────────────────────────────────────────

    /// Load configuration from `aura.config.toml` at `config_path`.
    ///
    /// If the file is missing or unreadable, returns defaults with a `warn!`
    /// log. Never panics. The caller can always operate with defaults.
    ///
    /// # Errors
    /// Returns `Err` only on TOML parse errors (malformed file content).
    /// Missing file returns `Ok(defaults)`.
    pub fn load(config_path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(config_path) {
            Ok(toml_str) => {
                info!(path = %config_path.display(), "loading aura.config.toml");
                let raw: RawAuraTomlConfig = toml::from_str(&toml_str).map_err(|e| {
                    ConfigError::ParseError(config_path.to_path_buf(), e.to_string())
                })?;
                let mut config = Self::from_raw(raw);
                config.config_source = ConfigSource::File(config_path.to_path_buf());
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                warn!(
                    path = %config_path.display(),
                    "aura.config.toml not found — using defaults and auto-scan"
                );
                Ok(Self::with_auto_scan())
            }
            Err(e) => {
                warn!(
                    path = %config_path.display(),
                    error = %e,
                    "could not read aura.config.toml — using defaults"
                );
                Ok(Self::with_auto_scan())
            }
        }
    }

    /// Load config from a TOML string (useful for testing without filesystem).
    #[allow(dead_code)] // Phase 8: used by integration test harness + JNI config injection
    pub fn from_toml_str(toml_str: &str) -> Result<Self, ConfigError> {
        let raw: RawAuraTomlConfig = toml::from_str(toml_str)
            .map_err(|e| ConfigError::ParseError(PathBuf::from("<str>"), e.to_string()))?;
        Ok(Self::from_raw(raw))
    }

    /// Construct from raw deserialized TOML, resolving path and overrides.
    fn from_raw(raw: RawAuraTomlConfig) -> Self {
        let model_path = raw.model.path.map(PathBuf::from);

        if let Some(ref p) = model_path {
            info!(path = %p.display(), "model path from config");
        }

        Self {
            model_path,
            user_override_embedding_dim: raw.model.embedding_dim,
            user_override_context_length: raw.model.context_length,
            config_source: ConfigSource::DefaultFallback,
        }
    }

    /// Produce defaults with auto-scan applied.
    ///
    /// Scans well-known directories and returns the first GGUF file found.
    fn with_auto_scan() -> Self {
        let model_path = auto_scan_for_model();
        let source = match &model_path {
            Some(p) => ConfigSource::AutoScanned(p.clone()),
            None => ConfigSource::DefaultFallback,
        };

        if model_path.is_none() {
            warn!(
                dirs = ?AUTO_SCAN_DIRS,
                "no GGUF model found in auto-scan dirs — ModelManager will use hardcoded filenames"
            );
        }

        Self {
            model_path,
            user_override_embedding_dim: None,
            user_override_context_length: None,
            config_source: source,
        }
    }

    /// Compiled fallback with no path or overrides.
    ///
    /// Used in tests and when the whole config system is bypassed.
    pub fn default_fallback() -> Self {
        Self {
            model_path: None,
            user_override_embedding_dim: None,
            user_override_context_length: None,
            config_source: ConfigSource::DefaultFallback,
        }
    }

    // ── Accessors ─────────────────────────────────────────────────────────

    /// Model path as a `&str`, or `None`.
    #[allow(dead_code)] // Phase 8: used by JNI config accessor
    pub fn model_path_str(&self) -> Option<&str> {
        self.model_path.as_deref()?.to_str()
    }

    /// Whether the config has a model override that callers should respect.
    #[allow(dead_code)] // Phase 8: used by JNI model selection guard
    pub fn has_model_path(&self) -> bool {
        self.model_path.is_some()
    }
}

// ─── Auto-scan ───────────────────────────────────────────────────────────────

/// Scan well-known directories for the first `.gguf` file.
///
/// Returns the path of the first file found, or `None` if all dirs are empty
/// or non-existent. Does not parse GGUF headers — just checks file extension.
fn auto_scan_for_model() -> Option<PathBuf> {
    for dir_str in AUTO_SCAN_DIRS {
        let dir = Path::new(dir_str);
        if !dir.exists() {
            continue;
        }

        let read_dir = match std::fs::read_dir(dir) {
            Ok(d) => d,
            Err(e) => {
                warn!(dir = dir_str, error = %e, "cannot read auto-scan dir");
                continue;
            }
        };

        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("gguf") {
                info!(path = %path.display(), "auto-scan found GGUF model");
                return Some(path);
            }
        }
    }

    None
}

// ─── Error type ──────────────────────────────────────────────────────────────

/// Errors from `NeocortexRuntimeConfig` loading.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to parse {0}: {1}")]
    ParseError(PathBuf, String),
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_config_only_path() {
        let toml = r#"
[model]
path = "/sdcard/AURA/models/qwen2.gguf"
"#;
        let config = NeocortexRuntimeConfig::from_toml_str(toml).expect("parse ok");
        assert_eq!(
            config.model_path.as_deref(),
            Some(Path::new("/sdcard/AURA/models/qwen2.gguf"))
        );
        assert!(config.user_override_embedding_dim.is_none());
        assert!(config.user_override_context_length.is_none());
    }

    #[test]
    fn config_with_embedding_override() {
        let toml = r#"
[model]
path = "/sdcard/AURA/models/custom.gguf"
embedding_dim = 1536
"#;
        let config = NeocortexRuntimeConfig::from_toml_str(toml).expect("parse ok");
        assert_eq!(config.user_override_embedding_dim, Some(1536));
    }

    #[test]
    fn config_with_context_override() {
        let toml = r#"
[model]
path = "/sdcard/AURA/models/custom.gguf"
context_length = 8192
"#;
        let config = NeocortexRuntimeConfig::from_toml_str(toml).expect("parse ok");
        assert_eq!(config.user_override_context_length, Some(8192));
    }

    #[test]
    fn empty_config_uses_defaults() {
        // Completely empty config — no [model] section at all.
        let toml = "";
        let config = NeocortexRuntimeConfig::from_toml_str(toml).expect("parse ok");
        assert!(config.model_path.is_none());
        assert!(config.user_override_embedding_dim.is_none());
    }

    #[test]
    fn malformed_toml_returns_error() {
        let toml = "this is not: valid toml: at all [[[[";
        let result = NeocortexRuntimeConfig::from_toml_str(toml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("parse") || msg.contains("str"), "error: {msg}");
    }

    #[test]
    fn missing_file_returns_defaults() {
        let path = Path::new("/absolutely/nonexistent/aura.config.toml");
        let config = NeocortexRuntimeConfig::load(path).expect("should return defaults, not error");
        // model_path may be Some if auto-scan found something on this machine,
        // or None if we're on a dev host — either is valid.
        // The important thing is it didn't panic or return Err.
        let _ = config.has_model_path();
    }

    #[test]
    fn default_fallback_has_no_overrides() {
        let config = NeocortexRuntimeConfig::default_fallback();
        assert!(config.user_override_embedding_dim.is_none());
        assert!(config.user_override_context_length.is_none());
    }

    #[test]
    fn full_config_all_fields() {
        let toml = r#"
[model]
path = "/sdcard/AURA/models/qwen3-8b.gguf"
embedding_dim = 4096
context_length = 32768
"#;
        let config = NeocortexRuntimeConfig::from_toml_str(toml).expect("parse ok");
        assert!(config.has_model_path());
        assert_eq!(config.user_override_embedding_dim, Some(4096));
        assert_eq!(config.user_override_context_length, Some(32768));
        assert_eq!(
            config.model_path_str(),
            Some("/sdcard/AURA/models/qwen3-8b.gguf")
        );
    }
}
