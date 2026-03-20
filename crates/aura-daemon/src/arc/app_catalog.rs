//! App catalog with Hebbian decay scoring.
//!
//! Tracks application usage, co-launch relationships, and produces a
//! Hebbian-weighted quick-access list via exponential decay scoring:
//!
//!   decay_score(pkg) = use_count × exp(−0.1 × days_since_last_use)
//!
//! Persisted to `~/.config/aura/app_catalog.json`.

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default config directory relative to the user home.
const CONFIG_SUBDIR: &str = ".config/aura";
/// JSON file name.
const CATALOG_FILE: &str = "app_catalog.json";
/// Hebbian decay rate (λ in exp(−λ·days)).
const DECAY_LAMBDA: f64 = 0.1;

// ---------------------------------------------------------------------------
// AppUsageEntry
// ---------------------------------------------------------------------------

/// Per-app usage record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppUsageEntry {
    /// Android/Linux package name or bundle identifier.
    pub package_name: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Total number of opens recorded.
    pub use_count: u32,
    /// Timestamp of the most recent open (seconds since UNIX_EPOCH).
    pub last_used_secs: u64,
    /// Co-launch counts: package_name → number of co-launches within the window.
    pub co_launches: HashMap<String, u32>,
}

impl AppUsageEntry {
    fn new(package_name: &str, display_name: &str) -> Self {
        Self {
            package_name: package_name.to_string(),
            display_name: display_name.to_string(),
            use_count: 0,
            last_used_secs: 0,
            co_launches: HashMap::new(),
        }
    }

    /// Reconstruct `SystemTime` from the stored seconds-since-epoch.
    pub fn last_used(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(self.last_used_secs)
    }
}

// ---------------------------------------------------------------------------
// AppCatalog
// ---------------------------------------------------------------------------

/// Catalog of installed/used apps with Hebbian decay ranking.
#[derive(Debug, Serialize, Deserialize)]
pub struct AppCatalog {
    /// Map from package name → usage entry.
    pub entries: HashMap<String, AppUsageEntry>,
    /// Path to the backing JSON file (not serialized — reconstructed on load).
    #[serde(skip)]
    pub path: PathBuf,
}

impl Default for AppCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl AppCatalog {
    /// Create an empty catalog with the default config path.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            path: Self::default_path(),
        }
    }

    /// Canonical path: `~/.config/aura/app_catalog.json`.
    pub fn default_path() -> PathBuf {
        let base = home_dir();
        base.join(CONFIG_SUBDIR).join(CATALOG_FILE)
    }

    /// Load catalog from `~/.config/aura/app_catalog.json`.
    /// Returns an empty default catalog on any error (missing file, parse error).
    pub fn load() -> Self {
        let path = Self::default_path();
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<AppCatalog>(&bytes) {
                Ok(mut catalog) => {
                    catalog.path = path;
                    debug!(
                        entries = catalog.entries.len(),
                        "app catalog loaded from disk"
                    );
                    catalog
                }
                Err(e) => {
                    warn!("app catalog parse error ({e}), starting fresh");
                    let mut c = Self::new();
                    c.path = path;
                    c
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!("app catalog not found, starting fresh");
                let mut c = Self::new();
                c.path = path;
                c
            }
            Err(e) => {
                warn!("app catalog read error ({e}), starting fresh");
                let mut c = Self::new();
                c.path = path;
                c
            }
        }
    }

    /// Persist the catalog to disk.  Atomic write via temp-file rename.
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)?;
        std::fs::rename(&tmp, &self.path)?;
        debug!(
            path = %self.path.display(),
            size_bytes = json.len(),
            "app catalog saved"
        );
        Ok(())
    }

    /// Record that `pkg` was opened (with human-readable `name`).
    pub fn record_open(&mut self, pkg: &str, name: &str) {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let entry = self
            .entries
            .entry(pkg.to_string())
            .or_insert_with(|| AppUsageEntry::new(pkg, name));
        entry.display_name = name.to_string(); // refresh display name
        entry.use_count = entry.use_count.saturating_add(1);
        entry.last_used_secs = now_secs;
        debug!(pkg, use_count = entry.use_count, "app open recorded");
    }

    /// Record that `a` and `b` were co-launched (opened within the 10-minute window).
    ///
    /// Call this when two apps are opened within `CO_LAUNCH_WINDOW_SECS` of each other.
    pub fn record_co_launch(&mut self, a: &str, b: &str) {
        if a == b {
            return;
        }
        if let Some(entry_a) = self.entries.get_mut(a) {
            *entry_a.co_launches.entry(b.to_string()).or_insert(0) += 1;
        }
        if let Some(entry_b) = self.entries.get_mut(b) {
            *entry_b.co_launches.entry(a.to_string()).or_insert(0) += 1;
        }
    }

    /// Compute the Hebbian decay score for `pkg`.
    ///
    ///   score = use_count × exp(−0.1 × days_since_last_use)
    ///
    /// Returns 0.0 for unknown packages.
    pub fn decay_score(&self, pkg: &str) -> f64 {
        let Some(entry) = self.entries.get(pkg) else {
            return 0.0;
        };
        let days = days_since_secs(entry.last_used_secs);
        (entry.use_count as f64) * (-DECAY_LAMBDA * days).exp()
    }

    /// Return up to `limit` package names sorted by `decay_score` descending.
    pub fn quick_access(&self, limit: usize) -> Vec<String> {
        let mut scored: Vec<(&str, f64)> = self
            .entries
            .keys()
            .map(|pkg| (pkg.as_str(), self.decay_score(pkg)))
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .into_iter()
            .take(limit)
            .map(|(pkg, _)| pkg.to_string())
            .collect()
    }

    /// Compute Hebbian association boost for `pkg` given recently-used apps.
    ///
    /// Returns a value in [0.0, 1.0]:
    ///   boost = max_co_launches(pkg, r) / (1 + max_co_launches) for r in `recent`
    ///
    /// Returns 0.0 if `pkg` is unknown or has no co-launches with `recent`.
    pub fn hebbian_boost(&self, pkg: &str, recent: &[String]) -> f64 {
        let Some(entry) = self.entries.get(pkg) else {
            return 0.0;
        };
        if recent.is_empty() || entry.co_launches.is_empty() {
            return 0.0;
        }

        let max_co: u32 = recent
            .iter()
            .filter(|r| r.as_str() != pkg)
            .filter_map(|r| entry.co_launches.get(r.as_str()).copied())
            .max()
            .unwrap_or(0);

        if max_co == 0 {
            return 0.0;
        }

        // Sigmoid-like saturation: 10 co-launches → ~0.91 boost.
        (max_co as f64 / (1.0 + max_co as f64)).clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Days elapsed since the UNIX timestamp `secs`.  Returns 0.0 on clock errors.
fn days_since_secs(secs: u64) -> f64 {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    now_secs.saturating_sub(secs) as f64 / 86_400.0
}

/// Return home directory, falling back to `/tmp` if unavailable.
fn home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home);
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(userprofile) = std::env::var("USERPROFILE") {
            return PathBuf::from(userprofile);
        }
        let drive = std::env::var("HOMEDRIVE").unwrap_or_else(|_| "C:".to_string());
        let homepath = std::env::var("HOMEPATH").unwrap_or_else(|_| "\\Users\\user".to_string());
        return PathBuf::from(format!("{}{}", drive, homepath));
    }
    #[allow(unreachable_code)]
    PathBuf::from("/tmp")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_open_increments_use_count() {
        let mut catalog = AppCatalog::new();
        catalog.record_open("com.example.app", "Example");
        catalog.record_open("com.example.app", "Example");
        assert_eq!(catalog.entries["com.example.app"].use_count, 2);
    }

    #[test]
    fn test_decay_score_unknown_is_zero() {
        let catalog = AppCatalog::new();
        assert_eq!(catalog.decay_score("com.unknown"), 0.0);
    }

    #[test]
    fn test_decay_score_recently_used() {
        let mut catalog = AppCatalog::new();
        catalog.record_open("com.fresh.app", "Fresh");
        let score = catalog.decay_score("com.fresh.app");
        // Just opened → days ≈ 0, score ≈ use_count * 1.0
        assert!(
            score > 0.9,
            "recently opened app should have score > 0.9, got {score}"
        );
    }

    #[test]
    fn test_quick_access_ordering() {
        let mut catalog = AppCatalog::new();
        catalog.record_open("com.a", "A");
        catalog.record_open("com.b", "B");
        catalog.record_open("com.b", "B");
        catalog.record_open("com.b", "B");
        let top = catalog.quick_access(2);
        assert_eq!(top[0], "com.b", "most-used app should rank first");
    }

    #[test]
    fn test_hebbian_boost_no_colaunch() {
        let mut catalog = AppCatalog::new();
        catalog.record_open("com.a", "A");
        let boost = catalog.hebbian_boost("com.a", &["com.b".to_string()]);
        assert_eq!(boost, 0.0);
    }

    #[test]
    fn test_hebbian_boost_with_colaunch() {
        let mut catalog = AppCatalog::new();
        catalog.record_open("com.a", "A");
        catalog.record_open("com.b", "B");
        catalog.record_co_launch("com.a", "com.b");
        catalog.record_co_launch("com.a", "com.b");
        let boost = catalog.hebbian_boost("com.a", &["com.b".to_string()]);
        assert!(boost > 0.0, "should have positive boost after co-launches");
        assert!(boost <= 1.0, "boost must not exceed 1.0");
    }
}
