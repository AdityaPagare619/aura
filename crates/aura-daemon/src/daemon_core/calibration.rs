//! Device calibration — profile the device, select model tier, benchmark,
//! discover apps, and establish operational baselines.
//!
//! Runs during onboarding Phase 5 (or standalone) to understand what the
//! hardware can do so AURA can pick the right model tier and budget.

use aura_types::errors::OnboardingError;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Minimum usable RAM (MB) — below this AURA runs in ultra-light mode.
const MIN_USABLE_RAM_MB: u64 = 512;

/// Threshold for "low" storage (MB).
const LOW_STORAGE_THRESHOLD_MB: u64 = 500;

/// Benchmark duration cap (ms) — don't spend more than this benchmarking.
const MAX_BENCHMARK_DURATION_MS: u64 = 5_000;

/// Number of benchmark iterations for CPU scoring.
const BENCHMARK_ITERATIONS: u32 = 100_000;

// ---------------------------------------------------------------------------
// Device profile
// ---------------------------------------------------------------------------

/// Hardware capabilities of the device running AURA.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceProfile {
    /// Total RAM in MB.
    pub total_ram_mb: u64,
    /// Available RAM in MB at calibration time.
    pub available_ram_mb: u64,
    /// Number of CPU cores.
    pub cpu_cores: u32,
    /// CPU architecture string (e.g. "aarch64", "x86_64").
    pub cpu_arch: String,
    /// Total storage in MB.
    pub total_storage_mb: u64,
    /// Available storage in MB.
    pub available_storage_mb: u64,
    /// Battery level at calibration (0–100), `None` if not a battery device.
    pub battery_percent: Option<u8>,
    /// Whether the device is currently charging.
    pub is_charging: bool,
    /// Android API level (0 on non-Android).
    pub api_level: u32,
    /// CPU benchmark score (higher = faster).
    pub cpu_benchmark_score: u32,
    /// Timestamp of calibration (ms).
    pub calibrated_at_ms: u64,
}

impl Default for DeviceProfile {
    fn default() -> Self {
        Self {
            total_ram_mb: 0,
            available_ram_mb: 0,
            cpu_cores: 1,
            cpu_arch: String::new(),
            total_storage_mb: 0,
            available_storage_mb: 0,
            battery_percent: None,
            is_charging: false,
            api_level: 0,
            cpu_benchmark_score: 0,
            calibrated_at_ms: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Model tier
// ---------------------------------------------------------------------------

/// Model tier — determines which LLM model size AURA loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelTier {
    /// Ultra-light: <1B params, minimal RAM, fast but limited.
    UltraLight,
    /// Light: 1–3B params, modest RAM, good for most tasks.
    Light,
    /// Standard: 3–7B params, moderate RAM, full capabilities.
    Standard,
    /// Heavy: 7–13B params, high RAM, maximum quality.
    Heavy,
}

impl ModelTier {
    /// Minimum RAM (MB) required for this tier.
    pub fn min_ram_mb(&self) -> u64 {
        match self {
            Self::UltraLight => 512,
            Self::Light => 2048,
            Self::Standard => 4096,
            Self::Heavy => 8192,
        }
    }

    /// Human-readable description.
    pub fn description(&self) -> &'static str {
        match self {
            Self::UltraLight => "Ultra-Light (fast responses, basic capabilities)",
            Self::Light => "Light (balanced speed and quality)",
            Self::Standard => "Standard (full capabilities, moderate speed)",
            Self::Heavy => "Heavy (maximum quality, slower responses)",
        }
    }
}

// ---------------------------------------------------------------------------
// App category (discovered apps)
// ---------------------------------------------------------------------------

/// Category of a discovered app on the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppCategory {
    Communication,
    Social,
    Productivity,
    Entertainment,
    Finance,
    Health,
    Education,
    Navigation,
    Shopping,
    Utility,
    System,
    Other,
}

/// A discovered app on the device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredApp {
    /// Package name (e.g. "com.whatsapp").
    pub package: String,
    /// Human-readable label.
    pub label: String,
    /// Inferred category.
    pub category: AppCategory,
    /// Whether AURA has been granted automation access for this app.
    pub automation_enabled: bool,
}

// ---------------------------------------------------------------------------
// CalibrationResult — output of the calibration process
// ---------------------------------------------------------------------------

/// Complete calibration result produced during onboarding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationResult {
    /// Device hardware profile.
    pub device: DeviceProfile,
    /// Selected model tier based on hardware.
    pub model_tier: ModelTier,
    /// Discovered apps on the device.
    pub discovered_apps: Vec<DiscoveredApp>,
    /// Operational baseline: average response latency (ms) from benchmark.
    pub baseline_latency_ms: u64,
    /// Whether calibration completed fully or was partial.
    pub complete: bool,
}

// ---------------------------------------------------------------------------
// CalibrationEngine
// ---------------------------------------------------------------------------

/// Engine that runs device calibration during onboarding.
#[derive(Debug)]
pub struct CalibrationEngine {
    /// Whether benchmarking is enabled (can be disabled in config).
    benchmark_enabled: bool,
}

impl CalibrationEngine {
    /// Create a new calibration engine.
    pub fn new(benchmark_enabled: bool) -> Self {
        Self { benchmark_enabled }
    }

    /// Run the full calibration sequence.
    ///
    /// 1. Profile hardware (RAM, CPU, storage, battery)
    /// 2. Select model tier
    /// 3. Run CPU benchmark (if enabled)
    /// 4. Discover installed apps
    /// 5. Establish baseline
    pub fn run(&self, now_ms: u64) -> Result<CalibrationResult, OnboardingError> {
        info!("starting device calibration");

        // Step 1: Profile hardware
        let mut device = self.profile_hardware(now_ms)?;
        debug!(
            ram = device.total_ram_mb,
            cores = device.cpu_cores,
            arch = %device.cpu_arch,
            "hardware profiled"
        );

        // Step 2: Run benchmark (optional)
        if self.benchmark_enabled {
            let score = self.run_cpu_benchmark()?;
            device.cpu_benchmark_score = score;
            debug!(score, "CPU benchmark complete");
        }

        // Step 3: Select model tier
        let model_tier = self.select_model_tier(&device);
        info!(tier = ?model_tier, "model tier selected");

        // Step 4: Discover apps
        let discovered_apps = self.discover_apps()?;
        debug!(count = discovered_apps.len(), "apps discovered");

        // Step 5: Establish baseline latency
        let baseline_latency_ms = self.estimate_baseline_latency(&device);

        let result = CalibrationResult {
            device,
            model_tier,
            discovered_apps,
            baseline_latency_ms,
            complete: true,
        };

        info!(
            tier = ?result.model_tier,
            latency_ms = result.baseline_latency_ms,
            apps = result.discovered_apps.len(),
            "calibration complete"
        );

        Ok(result)
    }

    /// Profile the device hardware.
    fn profile_hardware(&self, now_ms: u64) -> Result<DeviceProfile, OnboardingError> {
        // On a real Android device these would read from /proc, sysfs, etc.
        // For now, use std::mem and num_cpus-like heuristics.
        let cpu_cores = std::thread::available_parallelism()
            .map(|p| p.get() as u32)
            .unwrap_or(1);

        let cpu_arch = if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else if cfg!(target_arch = "arm") {
            "arm"
        } else if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else if cfg!(target_arch = "x86") {
            "x86"
        } else {
            "unknown"
        };

        // RAM: In production, read /proc/meminfo on Android.
        // Fallback to reasonable defaults for development/testing.
        let (total_ram_mb, available_ram_mb) = self.read_memory_info();

        // Storage: In production, use statvfs on the data directory.
        let (total_storage_mb, available_storage_mb) = self.read_storage_info();

        Ok(DeviceProfile {
            total_ram_mb,
            available_ram_mb,
            cpu_cores,
            cpu_arch: cpu_arch.to_string(),
            total_storage_mb,
            available_storage_mb,
            battery_percent: None, // Set by platform-specific code.
            is_charging: false,
            api_level: 0, // Set by Android JNI layer.
            cpu_benchmark_score: 0,
            calibrated_at_ms: now_ms,
        })
    }

    /// Read memory information.
    ///
    /// On non-Android platforms, returns reasonable test defaults.
    fn read_memory_info(&self) -> (u64, u64) {
        // In production Android, this reads /proc/meminfo.
        // For cross-platform development, return sensible defaults.
        #[cfg(target_os = "android")]
        {
            // TODO: Read /proc/meminfo
            (4096, 2048)
        }
        #[cfg(not(target_os = "android"))]
        {
            // Development fallback
            (4096, 2048)
        }
    }

    /// Read storage information.
    fn read_storage_info(&self) -> (u64, u64) {
        #[cfg(target_os = "android")]
        {
            // TODO: Use statvfs on data directory
            (32768, 16384)
        }
        #[cfg(not(target_os = "android"))]
        {
            (32768, 16384)
        }
    }

    /// Run a simple CPU benchmark to estimate processing capability.
    fn run_cpu_benchmark(&self) -> Result<u32, OnboardingError> {
        let start = std::time::Instant::now();
        let mut acc: u64 = 0;

        for i in 0..BENCHMARK_ITERATIONS {
            // Simple arithmetic workload — not cryptographic, just CPU-bound.
            acc = acc.wrapping_add(i as u64).wrapping_mul(7).wrapping_add(13);

            // Safety: don't exceed benchmark time budget.
            if i % 10_000 == 0 && start.elapsed().as_millis() as u64 > MAX_BENCHMARK_DURATION_MS {
                debug!(iterations = i, "benchmark capped at time limit");
                let elapsed_ms = start.elapsed().as_millis().max(1) as u64;
                return Ok((i as u64 * 1000 / elapsed_ms) as u32);
            }
        }

        let elapsed_ms = start.elapsed().as_millis().max(1) as u64;
        let score = (BENCHMARK_ITERATIONS as u64 * 1000 / elapsed_ms) as u32;
        // Use acc to prevent optimisation.
        if acc == 0 {
            debug!("benchmark: acc sentinel");
        }

        Ok(score)
    }

    /// Select the model tier based on device capabilities.
    pub fn select_model_tier(&self, device: &DeviceProfile) -> ModelTier {
        let ram = device.available_ram_mb;

        if ram < MIN_USABLE_RAM_MB {
            warn!(ram, "very low RAM — ultra-light mode");
            return ModelTier::UltraLight;
        }

        // Primary factor: available RAM
        // Secondary factor: CPU cores and benchmark score
        let ram_tier = if ram >= ModelTier::Heavy.min_ram_mb() {
            ModelTier::Heavy
        } else if ram >= ModelTier::Standard.min_ram_mb() {
            ModelTier::Standard
        } else if ram >= ModelTier::Light.min_ram_mb() {
            ModelTier::Light
        } else {
            ModelTier::UltraLight
        };

        // If benchmark score is very low, downgrade one tier.
        if device.cpu_benchmark_score > 0 && device.cpu_benchmark_score < 10_000 {
            match ram_tier {
                ModelTier::Heavy => ModelTier::Standard,
                ModelTier::Standard => ModelTier::Light,
                _ => ram_tier,
            }
        } else {
            ram_tier
        }
    }

    /// Discover installed apps (platform-specific).
    fn discover_apps(&self) -> Result<Vec<DiscoveredApp>, OnboardingError> {
        // On Android: query PackageManager via JNI.
        // On host: return empty list for testing.
        #[cfg(target_os = "android")]
        {
            // TODO: Query PackageManager
            Ok(Vec::new())
        }
        #[cfg(not(target_os = "android"))]
        {
            Ok(Vec::new())
        }
    }

    /// Estimate baseline response latency based on device profile.
    ///
    /// Returns a flat tier value — no weighted arithmetic.
    /// The neocortex interprets these tiers and adjusts its own behaviour;
    /// the daemon does NOT make model-behaviour decisions here.
    fn estimate_baseline_latency(&self, device: &DeviceProfile) -> u64 {
        // Flat latency tiers by available RAM only.
        // CPU benchmark is forwarded as raw data in CalibrationResult;
        // the LLM decides what operational changes to make from it.
        if device.available_ram_mb >= 8192 {
            200
        } else if device.available_ram_mb >= 4096 {
            500
        } else if device.available_ram_mb >= 2048 {
            1000
        } else {
            2000
        }
    }
}

/// Check if the device has sufficient storage for AURA data.
pub fn has_sufficient_storage(device: &DeviceProfile) -> bool {
    device.available_storage_mb >= LOW_STORAGE_THRESHOLD_MB
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

impl CalibrationResult {
    /// Save the calibration result to a SQLite database.
    pub fn save_to_db(&self, db: &rusqlite::Connection) -> Result<(), OnboardingError> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS calibration (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                data BLOB NOT NULL,
                calibrated_at_ms INTEGER NOT NULL
            );",
        )
        .map_err(|e| OnboardingError::CalibrationFailed(format!("create table: {e}")))?;

        let json = serde_json::to_vec(self)
            .map_err(|e| OnboardingError::CalibrationFailed(format!("serialize: {e}")))?;

        db.execute(
            "INSERT INTO calibration (id, data, calibrated_at_ms)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET data = ?1, calibrated_at_ms = ?2;",
            rusqlite::params![json, self.device.calibrated_at_ms as i64],
        )
        .map_err(|e| OnboardingError::CalibrationFailed(format!("upsert: {e}")))?;

        Ok(())
    }

    /// Load the calibration result from a SQLite database.
    pub fn load_from_db(db: &rusqlite::Connection) -> Result<Option<Self>, OnboardingError> {
        let table_exists: bool = db
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='calibration';",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .map_err(|e| OnboardingError::CalibrationFailed(format!("check table: {e}")))?;

        if !table_exists {
            return Ok(None);
        }

        let result: Result<Vec<u8>, _> =
            db.query_row("SELECT data FROM calibration WHERE id = 1;", [], |row| {
                row.get(0)
            });

        match result {
            Ok(data) => {
                let cal: Self = serde_json::from_slice(&data)
                    .map_err(|e| OnboardingError::CalibrationFailed(format!("deserialize: {e}")))?;
                Ok(Some(cal))
            },
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(OnboardingError::CalibrationFailed(format!("load: {e}"))),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_profile_default() {
        let dp = DeviceProfile::default();
        assert_eq!(dp.total_ram_mb, 0);
        assert_eq!(dp.cpu_cores, 1);
        assert!(dp.cpu_arch.is_empty());
    }

    #[test]
    fn test_model_tier_min_ram() {
        assert_eq!(ModelTier::UltraLight.min_ram_mb(), 512);
        assert_eq!(ModelTier::Light.min_ram_mb(), 2048);
        assert_eq!(ModelTier::Standard.min_ram_mb(), 4096);
        assert_eq!(ModelTier::Heavy.min_ram_mb(), 8192);
    }

    #[test]
    fn test_model_tier_description() {
        assert!(!ModelTier::Light.description().is_empty());
        assert!(ModelTier::Heavy.description().contains("maximum"));
    }

    #[test]
    fn test_select_model_tier_low_ram() {
        let engine = CalibrationEngine::new(false);
        let device = DeviceProfile {
            available_ram_mb: 256,
            ..Default::default()
        };
        assert_eq!(engine.select_model_tier(&device), ModelTier::UltraLight);
    }

    #[test]
    fn test_select_model_tier_moderate_ram() {
        let engine = CalibrationEngine::new(false);
        let device = DeviceProfile {
            available_ram_mb: 3000,
            ..Default::default()
        };
        assert_eq!(engine.select_model_tier(&device), ModelTier::Light);
    }

    #[test]
    fn test_select_model_tier_high_ram() {
        let engine = CalibrationEngine::new(false);
        let device = DeviceProfile {
            available_ram_mb: 8192,
            ..Default::default()
        };
        assert_eq!(engine.select_model_tier(&device), ModelTier::Heavy);
    }

    #[test]
    fn test_select_model_tier_slow_cpu_downgrade() {
        let engine = CalibrationEngine::new(false);
        let device = DeviceProfile {
            available_ram_mb: 8192,
            cpu_benchmark_score: 5000, // Very slow
            ..Default::default()
        };
        assert_eq!(engine.select_model_tier(&device), ModelTier::Standard);
    }

    #[test]
    fn test_has_sufficient_storage() {
        let low = DeviceProfile {
            available_storage_mb: 100,
            ..Default::default()
        };
        assert!(!has_sufficient_storage(&low));

        let ok = DeviceProfile {
            available_storage_mb: 1000,
            ..Default::default()
        };
        assert!(has_sufficient_storage(&ok));
    }

    #[test]
    fn test_calibration_engine_run() {
        let engine = CalibrationEngine::new(true);
        let result = engine.run(1000).expect("calibration should succeed");
        assert!(result.complete);
        assert!(result.device.cpu_cores >= 1);
        assert!(!result.device.cpu_arch.is_empty());
        assert!(result.device.cpu_benchmark_score > 0);
    }

    #[test]
    fn test_calibration_engine_no_benchmark() {
        let engine = CalibrationEngine::new(false);
        let result = engine.run(2000).expect("calibration should succeed");
        assert_eq!(result.device.cpu_benchmark_score, 0);
    }

    #[test]
    fn test_estimate_baseline_latency() {
        let engine = CalibrationEngine::new(false);

        // Higher RAM → lower (faster) latency tier.
        let fast = DeviceProfile {
            available_ram_mb: 8192,
            cpu_benchmark_score: 100_000,
            ..Default::default()
        };
        let slow = DeviceProfile {
            available_ram_mb: 1024,
            cpu_benchmark_score: 5000,
            ..Default::default()
        };

        // Flat RAM-tier lookup: 8192 MB → 200 ms, 1024 MB → 2000 ms.
        assert!(engine.estimate_baseline_latency(&fast) < engine.estimate_baseline_latency(&slow));
        // CPU benchmark score is forwarded as raw data; it does NOT adjust latency here.
        assert_eq!(engine.estimate_baseline_latency(&fast), 200);
        assert_eq!(engine.estimate_baseline_latency(&slow), 2000);
    }

    #[test]
    fn test_calibration_db_roundtrip() {
        let db = rusqlite::Connection::open_in_memory().expect("open db");
        let engine = CalibrationEngine::new(false);
        let result = engine.run(3000).expect("calibrate");
        result.save_to_db(&db).expect("save");

        let loaded = CalibrationResult::load_from_db(&db)
            .expect("load")
            .expect("should exist");
        assert_eq!(loaded.device.calibrated_at_ms, 3000);
        assert_eq!(loaded.model_tier, result.model_tier);
    }

    #[test]
    fn test_calibration_db_no_table() {
        let db = rusqlite::Connection::open_in_memory().expect("open db");
        let loaded = CalibrationResult::load_from_db(&db).expect("load");
        assert!(loaded.is_none());
    }

    #[test]
    fn test_cpu_benchmark_returns_nonzero() {
        let engine = CalibrationEngine::new(true);
        let score = engine.run_cpu_benchmark().expect("benchmark");
        assert!(score > 0);
    }
}
