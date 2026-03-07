//! Android Doze mode awareness and wakelock management.
//!
//! Android Doze restricts background network, jobs, and alarms.  AURA must:
//!
//! 1. Detect Doze entry/exit and adjust behavior accordingly.
//! 2. Queue deferred work and flush during maintenance windows.
//! 3. Use `AlarmManager.setExactAndAllowWhileIdle()` for critical timers.
//! 4. Manage wakelocks sparingly (max 10 s acquisition, tracked).
//!
//! # Spec Reference
//!
//! See `AURA-V4-POWER-AGENCY-REBALANCE.md` §2 — Android Power Architecture.

use std::time::{Duration, Instant};

use aura_types::errors::PlatformError;
use serde::{Deserialize, Serialize};

// ─── Constants ──────────────────────────────────────────────────────────────

/// Maximum time a wakelock may be held (hard ceiling).
const MAX_WAKELOCK_DURATION: Duration = Duration::from_secs(10);

/// Maximum number of items in the deferred work queue.
const MAX_DEFERRED_QUEUE: usize = 64;

/// Maximum number of tracked wakelock acquisitions in the history ring.
const MAX_WAKELOCK_HISTORY: usize = 32;

// ─── Doze State ─────────────────────────────────────────────────────────────

/// Which phase of Doze the device is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DozePhase {
    /// Device is active — no Doze restrictions.
    Active,
    /// Light Doze — jobs/syncs deferred but ForegroundService continues.
    LightDoze,
    /// Deep Doze — network suspended, wakelocks ignored (except FG service).
    DeepDoze,
    /// Maintenance window — brief period during Doze where network is available.
    MaintenanceWindow,
}

impl std::fmt::Display for DozePhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "Active"),
            Self::LightDoze => write!(f, "Light Doze"),
            Self::DeepDoze => write!(f, "Deep Doze"),
            Self::MaintenanceWindow => write!(f, "Maintenance Window"),
        }
    }
}

// ─── Deferred Work ──────────────────────────────────────────────────────────

/// Priority of a deferred work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DeferredPriority {
    /// Must execute in the next maintenance window.
    Critical,
    /// Should execute when convenient.
    Normal,
    /// Can wait until device exits Doze entirely.
    Low,
}

/// A work item queued for execution during a Doze maintenance window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeferredWork {
    /// Human-readable tag for logging.
    pub tag: String,
    /// Priority determines flush order.
    pub priority: DeferredPriority,
    /// When this item was queued.
    #[serde(skip)]
    pub queued_at: Option<Instant>,
    /// Opaque payload identifier — the daemon uses this to dispatch.
    pub payload_id: u64,
}

// ─── Wakelock Tracking ──────────────────────────────────────────────────────

/// Record of a single wakelock acquisition.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields used for diagnostics and future audit APIs.
struct WakelockRecord {
    tag: String,
    acquired_at: Instant,
    released_at: Option<Instant>,
    duration: Duration,
}

// ─── DozeManager ────────────────────────────────────────────────────────────

/// Tracks Android Doze phase and manages deferred work + wakelocks.
pub struct DozeManager {
    /// Current Doze phase.
    phase: DozePhase,
    /// Whether Doze is active (simplified boolean for fast checks).
    doze_active: bool,
    /// Timestamp of last Doze state change.
    last_phase_change: Instant,
    /// Count of maintenance windows observed (for expanding-interval tracking).
    maintenance_window_count: u32,
    /// Deferred work queue (bounded to [`MAX_DEFERRED_QUEUE`]).
    deferred_queue: Vec<DeferredWork>,
    /// Active wakelock tag (only one at a time; None if not held).
    active_wakelock: Option<(String, Instant)>,
    /// Wakelock history ring buffer.
    wakelock_history: Vec<WakelockRecord>,
    /// Total wakelock time accumulated today (for self-monitoring).
    total_wakelock_time_today: Duration,
}

impl DozeManager {
    /// Create a new `DozeManager` in the Active phase.
    pub fn new() -> Self {
        Self {
            phase: DozePhase::Active,
            doze_active: false,
            last_phase_change: Instant::now(),
            maintenance_window_count: 0,
            deferred_queue: Vec::new(),
            active_wakelock: None,
            wakelock_history: Vec::new(),
            total_wakelock_time_today: Duration::ZERO,
        }
    }

    /// Update Doze state from the platform.
    ///
    /// `doze_active` is the value from `PowerManager.isDeviceIdleMode()`.
    /// Returns `true` if the doze state changed.
    pub fn update_doze_state(&mut self, doze_active: bool) -> bool {
        let changed = self.doze_active != doze_active;
        self.doze_active = doze_active;

        if changed {
            let old_phase = self.phase;
            self.phase = if doze_active {
                DozePhase::DeepDoze
            } else {
                DozePhase::Active
            };
            self.last_phase_change = Instant::now();

            tracing::info!(
                from = %old_phase,
                to = %self.phase,
                "doze phase changed"
            );
        }

        changed
    }

    /// Notify the manager that a maintenance window has opened.
    ///
    /// Returns the deferred work items that should be flushed, sorted by
    /// priority (Critical first).
    pub fn on_maintenance_window(&mut self) -> Vec<DeferredWork> {
        self.phase = DozePhase::MaintenanceWindow;
        self.maintenance_window_count += 1;
        self.last_phase_change = Instant::now();

        tracing::info!(
            window_count = self.maintenance_window_count,
            queued = self.deferred_queue.len(),
            "maintenance window opened"
        );

        // Drain and return items sorted by priority.
        let mut items: Vec<DeferredWork> = self.deferred_queue.drain(..).collect();
        items.sort_by_key(|w| w.priority);
        items
    }

    /// Notify the manager that the maintenance window has closed.
    pub fn on_maintenance_window_closed(&mut self) {
        self.phase = DozePhase::DeepDoze;
        self.last_phase_change = Instant::now();

        tracing::info!("maintenance window closed, returning to Deep Doze");
    }

    /// Queue deferred work for the next maintenance window.
    ///
    /// # Errors
    /// Returns an error if the queue is full.
    pub fn defer_work(&mut self, mut work: DeferredWork) -> Result<(), PlatformError> {
        if self.deferred_queue.len() >= MAX_DEFERRED_QUEUE {
            tracing::warn!(
                max = MAX_DEFERRED_QUEUE,
                tag = %work.tag,
                "deferred work queue full, dropping item"
            );
            return Err(PlatformError::DozeStateUnknown(format!(
                "deferred work queue full (max {})",
                MAX_DEFERRED_QUEUE
            )));
        }
        work.queued_at = Some(Instant::now());
        self.deferred_queue.push(work);
        Ok(())
    }

    /// Number of items currently in the deferred queue.
    pub fn deferred_queue_len(&self) -> usize {
        self.deferred_queue.len()
    }

    // ─── Wakelock Management ────────────────────────────────────────────

    /// Acquire a wakelock with the given tag.
    ///
    /// Only one wakelock can be held at a time. The wakelock is automatically
    /// limited to [`MAX_WAKELOCK_DURATION`].
    ///
    /// # Errors
    /// Returns an error if a wakelock is already held.
    pub fn acquire_wakelock(&mut self, tag: &str) -> Result<(), PlatformError> {
        if self.active_wakelock.is_some() {
            return Err(PlatformError::WakelockFailed(
                "wakelock already held".to_string(),
            ));
        }

        tracing::debug!(tag, "wakelock acquired");

        #[cfg(target_os = "android")]
        {
            // Real implementation: call JNI to acquire PARTIAL_WAKE_LOCK
            // with tag "aura:{tag}" and MAX_WAKELOCK_DURATION timeout.
            acquire_android_wakelock(tag)?;
        }

        self.active_wakelock = Some((tag.to_string(), Instant::now()));
        Ok(())
    }

    /// Release the currently held wakelock.
    ///
    /// No-op if no wakelock is held (idempotent release).
    pub fn release_wakelock(&mut self) {
        if let Some((tag, acquired_at)) = self.active_wakelock.take() {
            let held_for = acquired_at.elapsed();

            tracing::debug!(
                tag = %tag,
                held_ms = held_for.as_millis(),
                "wakelock released"
            );

            // Track history.
            if self.wakelock_history.len() >= MAX_WAKELOCK_HISTORY {
                self.wakelock_history.remove(0);
            }
            self.wakelock_history.push(WakelockRecord {
                tag,
                acquired_at,
                released_at: Some(Instant::now()),
                duration: held_for,
            });
            self.total_wakelock_time_today += held_for;

            #[cfg(target_os = "android")]
            {
                release_android_wakelock();
            }
        }
    }

    /// Check and auto-release wakelocks that have exceeded the time limit.
    ///
    /// Called periodically by the daemon heartbeat.
    pub fn enforce_wakelock_timeout(&mut self) {
        if let Some((ref tag, acquired_at)) = self.active_wakelock {
            if acquired_at.elapsed() > MAX_WAKELOCK_DURATION {
                tracing::warn!(
                    tag = %tag,
                    held_ms = acquired_at.elapsed().as_millis(),
                    max_ms = MAX_WAKELOCK_DURATION.as_millis(),
                    "wakelock exceeded max duration, force-releasing"
                );
                self.release_wakelock();
            }
        }
    }

    /// Whether a wakelock is currently held.
    pub fn is_wakelock_held(&self) -> bool {
        self.active_wakelock.is_some()
    }

    /// Total wakelock time accumulated today.
    pub fn total_wakelock_time_today(&self) -> Duration {
        self.total_wakelock_time_today
    }

    /// Reset the daily wakelock time counter (call at midnight).
    pub fn reset_daily_wakelock_time(&mut self) {
        self.total_wakelock_time_today = Duration::ZERO;
    }

    // ─── Read-only Queries ──────────────────────────────────────────────

    /// Current Doze phase.
    pub fn current_phase(&self) -> DozePhase {
        self.phase
    }

    /// Whether the device is in any form of Doze.
    pub fn is_doze_active(&self) -> bool {
        self.doze_active
    }

    /// How long the device has been in the current phase.
    pub fn phase_duration(&self) -> Duration {
        self.last_phase_change.elapsed()
    }

    /// Number of maintenance windows observed since boot / last reset.
    pub fn maintenance_window_count(&self) -> u32 {
        self.maintenance_window_count
    }
}

impl Default for DozeManager {
    fn default() -> Self {
        Self::new()
    }
}

// ─── OEM Kill Prevention ────────────────────────────────────────────────────

/// Known OEM vendors with aggressive background-app killers.
///
/// Each vendor has different settings UIs and intents that users must interact
/// with to whitelist AURA from being killed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OemVendor {
    /// Xiaomi / MIUI — "Autostart" + "Battery Saver" app list.
    Xiaomi,
    /// Samsung — "Device Care" / "Sleeping Apps" exclusion.
    Samsung,
    /// Huawei / EMUI — "App Launch" settings → manual manage.
    Huawei,
    /// OPPO / ColorOS — "App Auto-Launch" management.
    Oppo,
    /// Vivo / FuntouchOS — "Background App Management".
    Vivo,
    /// OnePlus / OxygenOS — "Battery Optimization" + "Auto-Launch".
    OnePlus,
    /// Generic / AOSP — standard battery optimization only.
    Generic,
}

impl std::fmt::Display for OemVendor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Xiaomi => write!(f, "Xiaomi (MIUI)"),
            Self::Samsung => write!(f, "Samsung (OneUI)"),
            Self::Huawei => write!(f, "Huawei (EMUI)"),
            Self::Oppo => write!(f, "OPPO (ColorOS)"),
            Self::Vivo => write!(f, "Vivo (FuntouchOS)"),
            Self::OnePlus => write!(f, "OnePlus (OxygenOS)"),
            Self::Generic => write!(f, "Generic (AOSP)"),
        }
    }
}

/// Guidance for users to whitelist AURA on a specific OEM device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OemWhitelistGuidance {
    /// The detected OEM vendor.
    pub vendor: OemVendor,
    /// User-facing steps to whitelist the app.
    pub steps: Vec<String>,
    /// Android intent action to open the relevant settings screen (if known).
    pub settings_intent: Option<String>,
    /// Whether autostart permission is granted.
    pub has_autostart: bool,
    /// Whether battery optimization exemption is granted.
    pub has_battery_exemption: bool,
}

/// Detect the OEM vendor from the device manufacturer string.
pub fn detect_oem_vendor(manufacturer: &str) -> OemVendor {
    let lower = manufacturer.to_lowercase();
    if lower.contains("xiaomi") || lower.contains("redmi") || lower.contains("poco") {
        OemVendor::Xiaomi
    } else if lower.contains("samsung") {
        OemVendor::Samsung
    } else if lower.contains("huawei") || lower.contains("honor") {
        OemVendor::Huawei
    } else if lower.contains("oppo") || lower.contains("realme") {
        OemVendor::Oppo
    } else if lower.contains("vivo") || lower.contains("iqoo") {
        OemVendor::Vivo
    } else if lower.contains("oneplus") {
        OemVendor::OnePlus
    } else {
        OemVendor::Generic
    }
}

/// Generate whitelist guidance for the given OEM vendor.
pub fn oem_whitelist_guidance(
    vendor: OemVendor,
    has_autostart: bool,
    has_battery_exemption: bool,
) -> OemWhitelistGuidance {
    let (steps, intent) = match vendor {
        OemVendor::Xiaomi => (
            vec![
                "Open Settings → Apps → Manage Apps → AURA".to_string(),
                "Enable 'Autostart' permission".to_string(),
                "Under 'Battery Saver', set to 'No restrictions'".to_string(),
                "Lock AURA in Recent Apps (swipe down on app card)".to_string(),
            ],
            Some("miui.intent.action.APP_PERM_EDITOR".to_string()),
        ),
        OemVendor::Samsung => (
            vec![
                "Open Settings → Device Care → Battery".to_string(),
                "Tap 'Background usage limits'".to_string(),
                "Remove AURA from 'Sleeping apps' and 'Deep sleeping apps'".to_string(),
                "Add AURA to 'Never sleeping apps'".to_string(),
            ],
            Some("com.samsung.android.lool".to_string()),
        ),
        OemVendor::Huawei => (
            vec![
                "Open Settings → Battery → App Launch".to_string(),
                "Find AURA and set to 'Manage manually'".to_string(),
                "Enable 'Auto-launch', 'Secondary launch', and 'Run in background'".to_string(),
            ],
            Some("huawei.intent.action.HSM_PROTECTED_APPS".to_string()),
        ),
        OemVendor::Oppo => (
            vec![
                "Open Settings → Battery → More battery settings".to_string(),
                "Tap 'Optimize battery use' → Don't optimize for AURA".to_string(),
                "Also check 'Auto-launch' in App Management".to_string(),
            ],
            Some("com.coloros.safecenter".to_string()),
        ),
        OemVendor::Vivo => (
            vec![
                "Open Settings → Battery → Background power consumption management".to_string(),
                "Allow AURA to run in background".to_string(),
                "Also check 'Autostart' in iManager → App Manager".to_string(),
            ],
            None,
        ),
        OemVendor::OnePlus => (
            vec![
                "Open Settings → Battery → Battery Optimization".to_string(),
                "Set AURA to 'Don't optimize'".to_string(),
                "Enable 'Auto-launch' for AURA".to_string(),
            ],
            Some("android.settings.IGNORE_BATTERY_OPTIMIZATION_SETTINGS".to_string()),
        ),
        OemVendor::Generic => (
            vec![
                "Open Settings → Battery → Battery Optimization".to_string(),
                "Set AURA to 'Don't optimize'".to_string(),
            ],
            Some("android.settings.IGNORE_BATTERY_OPTIMIZATION_SETTINGS".to_string()),
        ),
    };

    OemWhitelistGuidance {
        vendor,
        steps,
        settings_intent: intent,
        has_autostart,
        has_battery_exemption,
    }
}

/// Check OEM kill prevention status using JNI calls.
///
/// On Android, reads the manufacturer and checks autostart permission.
/// On desktop, returns a Generic vendor with all permissions granted.
pub fn check_oem_status() -> Result<OemWhitelistGuidance, PlatformError> {
    let manufacturer = read_manufacturer()?;
    let vendor = detect_oem_vendor(&manufacturer);
    let has_autostart = read_autostart_permission()?;
    let has_battery_exemption = read_battery_optimization_exemption()?;

    Ok(oem_whitelist_guidance(
        vendor,
        has_autostart,
        has_battery_exemption,
    ))
}

#[cfg(target_os = "android")]
fn read_manufacturer() -> Result<String, PlatformError> {
    super::jni_bridge::jni_get_device_manufacturer()
}

#[cfg(not(target_os = "android"))]
fn read_manufacturer() -> Result<String, PlatformError> {
    Ok("generic".to_string())
}

#[cfg(target_os = "android")]
fn read_autostart_permission() -> Result<bool, PlatformError> {
    super::jni_bridge::jni_has_autostart_permission()
}

#[cfg(not(target_os = "android"))]
fn read_autostart_permission() -> Result<bool, PlatformError> {
    Ok(true)
}

#[cfg(target_os = "android")]
fn read_battery_optimization_exemption() -> Result<bool, PlatformError> {
    super::jni_bridge::jni_is_ignoring_battery_optimizations()
}

#[cfg(not(target_os = "android"))]
fn read_battery_optimization_exemption() -> Result<bool, PlatformError> {
    Ok(true)
}

// ─── Android JNI Wakelock Implementation ────────────────────────────────────

#[cfg(target_os = "android")]
fn acquire_android_wakelock(tag: &str) -> Result<(), PlatformError> {
    let timeout_ms = MAX_WAKELOCK_DURATION.as_millis() as i64;
    super::jni_bridge::jni_acquire_wakelock(&format!("aura:{tag}"), timeout_ms)
}

#[cfg(target_os = "android")]
fn release_android_wakelock() {
    if let Err(e) = super::jni_bridge::jni_release_wakelock() {
        tracing::warn!("JNI release wakelock failed: {e}");
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_is_active() {
        let dm = DozeManager::new();
        assert_eq!(dm.current_phase(), DozePhase::Active);
        assert!(!dm.is_doze_active());
    }

    #[test]
    fn test_doze_entry_and_exit() {
        let mut dm = DozeManager::new();

        let changed = dm.update_doze_state(true);
        assert!(changed);
        assert!(dm.is_doze_active());
        assert_eq!(dm.current_phase(), DozePhase::DeepDoze);

        let changed = dm.update_doze_state(false);
        assert!(changed);
        assert!(!dm.is_doze_active());
        assert_eq!(dm.current_phase(), DozePhase::Active);
    }

    #[test]
    fn test_no_change_returns_false() {
        let mut dm = DozeManager::new();
        let changed = dm.update_doze_state(false);
        assert!(!changed);
    }

    #[test]
    fn test_deferred_work_queue() {
        let mut dm = DozeManager::new();

        let work = DeferredWork {
            tag: "telegram-flush".to_string(),
            priority: DeferredPriority::Critical,
            queued_at: None,
            payload_id: 1,
        };
        dm.defer_work(work).expect("should queue");

        let work2 = DeferredWork {
            tag: "log-sync".to_string(),
            priority: DeferredPriority::Low,
            queued_at: None,
            payload_id: 2,
        };
        dm.defer_work(work2).expect("should queue");

        assert_eq!(dm.deferred_queue_len(), 2);

        // Maintenance window flushes and sorts by priority.
        let items = dm.on_maintenance_window();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].tag, "telegram-flush"); // Critical first
        assert_eq!(items[1].tag, "log-sync"); // Low last
        assert_eq!(dm.deferred_queue_len(), 0);
    }

    #[test]
    fn test_deferred_queue_capacity() {
        let mut dm = DozeManager::new();
        for i in 0..MAX_DEFERRED_QUEUE {
            let work = DeferredWork {
                tag: format!("work-{i}"),
                priority: DeferredPriority::Normal,
                queued_at: None,
                payload_id: i as u64,
            };
            dm.defer_work(work).expect("within capacity");
        }
        // One more should fail.
        let work = DeferredWork {
            tag: "overflow".to_string(),
            priority: DeferredPriority::Normal,
            queued_at: None,
            payload_id: 999,
        };
        assert!(dm.defer_work(work).is_err());
    }

    #[test]
    fn test_wakelock_acquire_release() {
        let mut dm = DozeManager::new();

        dm.acquire_wakelock("inference").expect("should acquire");
        assert!(dm.is_wakelock_held());

        dm.release_wakelock();
        assert!(!dm.is_wakelock_held());
        assert_eq!(dm.wakelock_history.len(), 1);
    }

    #[test]
    fn test_wakelock_double_acquire_fails() {
        let mut dm = DozeManager::new();
        dm.acquire_wakelock("first").expect("should succeed");

        let result = dm.acquire_wakelock("second");
        assert!(result.is_err());
    }

    #[test]
    fn test_wakelock_release_is_idempotent() {
        let mut dm = DozeManager::new();
        dm.release_wakelock(); // Should not panic.
        dm.release_wakelock(); // Still fine.
    }

    #[test]
    fn test_maintenance_window_counter() {
        let mut dm = DozeManager::new();
        assert_eq!(dm.maintenance_window_count(), 0);

        let _ = dm.on_maintenance_window();
        assert_eq!(dm.maintenance_window_count(), 1);
        assert_eq!(dm.current_phase(), DozePhase::MaintenanceWindow);

        dm.on_maintenance_window_closed();
        assert_eq!(dm.current_phase(), DozePhase::DeepDoze);
    }

    #[test]
    fn test_doze_phase_display() {
        assert_eq!(DozePhase::Active.to_string(), "Active");
        assert_eq!(DozePhase::DeepDoze.to_string(), "Deep Doze");
        assert_eq!(
            DozePhase::MaintenanceWindow.to_string(),
            "Maintenance Window"
        );
    }

    // ── OEM Kill Prevention Tests ───────────────────────────────────────

    #[test]
    fn test_detect_oem_vendor_xiaomi() {
        assert_eq!(detect_oem_vendor("Xiaomi"), OemVendor::Xiaomi);
        assert_eq!(detect_oem_vendor("Redmi"), OemVendor::Xiaomi);
        assert_eq!(detect_oem_vendor("POCO"), OemVendor::Xiaomi);
    }

    #[test]
    fn test_detect_oem_vendor_samsung() {
        assert_eq!(detect_oem_vendor("samsung"), OemVendor::Samsung);
        assert_eq!(detect_oem_vendor("Samsung"), OemVendor::Samsung);
    }

    #[test]
    fn test_detect_oem_vendor_huawei() {
        assert_eq!(detect_oem_vendor("HUAWEI"), OemVendor::Huawei);
        assert_eq!(detect_oem_vendor("HONOR"), OemVendor::Huawei);
    }

    #[test]
    fn test_detect_oem_vendor_oppo() {
        assert_eq!(detect_oem_vendor("OPPO"), OemVendor::Oppo);
        assert_eq!(detect_oem_vendor("realme"), OemVendor::Oppo);
    }

    #[test]
    fn test_detect_oem_vendor_vivo() {
        assert_eq!(detect_oem_vendor("vivo"), OemVendor::Vivo);
        assert_eq!(detect_oem_vendor("iQOO"), OemVendor::Vivo);
    }

    #[test]
    fn test_detect_oem_vendor_oneplus() {
        assert_eq!(detect_oem_vendor("OnePlus"), OemVendor::OnePlus);
    }

    #[test]
    fn test_detect_oem_vendor_generic() {
        assert_eq!(detect_oem_vendor("Google"), OemVendor::Generic);
        assert_eq!(detect_oem_vendor("Pixel"), OemVendor::Generic);
        assert_eq!(detect_oem_vendor("unknown"), OemVendor::Generic);
    }

    #[test]
    fn test_oem_whitelist_guidance_has_steps() {
        let guidance = oem_whitelist_guidance(OemVendor::Xiaomi, false, false);
        assert_eq!(guidance.vendor, OemVendor::Xiaomi);
        assert!(!guidance.steps.is_empty());
        assert!(guidance.settings_intent.is_some());
        assert!(!guidance.has_autostart);
    }

    #[test]
    fn test_oem_whitelist_guidance_all_vendors() {
        // Ensure no vendor panics and all have non-empty steps.
        let vendors = [
            OemVendor::Xiaomi,
            OemVendor::Samsung,
            OemVendor::Huawei,
            OemVendor::Oppo,
            OemVendor::Vivo,
            OemVendor::OnePlus,
            OemVendor::Generic,
        ];
        for vendor in vendors {
            let guidance = oem_whitelist_guidance(vendor, true, true);
            assert!(!guidance.steps.is_empty(), "vendor {vendor} has no steps");
        }
    }

    #[test]
    fn test_check_oem_status_desktop() {
        let status = check_oem_status().expect("desktop stub should work");
        assert_eq!(status.vendor, OemVendor::Generic);
        assert!(status.has_autostart);
        assert!(status.has_battery_exemption);
    }

    #[test]
    fn test_oem_vendor_display() {
        assert_eq!(OemVendor::Xiaomi.to_string(), "Xiaomi (MIUI)");
        assert_eq!(OemVendor::Samsung.to_string(), "Samsung (OneUI)");
        assert_eq!(OemVendor::Generic.to_string(), "Generic (AOSP)");
    }
}
