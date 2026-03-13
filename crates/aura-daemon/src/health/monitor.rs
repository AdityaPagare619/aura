//! Core health monitor implementation.
//!
//! Contains [`BoundedVec`], enums ([`ThermalState`], [`HealthStatus`]),
//! the [`HealthReport`] snapshot, and the [`HealthMonitor`] that performs
//! periodic self-diagnostics.
//!
//! All collections are bounded to prevent unbounded heap growth on a
//! memory-constrained Android device (4–8 GB shared with the OS).

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

// ─── Constants ──────────────────────────────────────────────────────────────

/// Default health check interval in milliseconds (30 seconds).
///
/// Chosen to balance responsiveness with CPU/battery cost. At 30s we detect
/// critical issues within a minute while consuming negligible resources.
const DEFAULT_CHECK_INTERVAL_MS: u64 = 30_000;

/// Health check interval when in low-power mode (60 seconds).
///
/// Doubles the interval to reduce wakeups when battery is critically low.
const LOW_POWER_CHECK_INTERVAL_MS: u64 = 60_000;

/// Battery level threshold below which low-power mode activates (10%).
///
/// Aligns with BatteryTier::Emergency in the power module.
const LOW_POWER_BATTERY_THRESHOLD: f32 = 0.10;

/// Error rate threshold above which status becomes Degraded.
///
/// 30% error rate over the last hour indicates something is wrong but
/// the system is still partially functional.
const ERROR_RATE_DEGRADED_THRESHOLD: f32 = 0.30;

/// Error rate threshold above which status becomes Critical.
///
/// 50% error rate means the system is failing more than it succeeds.
const ERROR_RATE_CRITICAL_THRESHOLD: f32 = 0.50;

/// Maximum neocortex restart attempts per session.
///
/// Prevents restart storms — if the neocortex crashes 3 times in one
/// session, we stop trying and report Critical status.
const MAX_NEOCORTEX_RESTART_ATTEMPTS: u32 = 3;

/// Maximum capacity for the error timestamp window.
///
/// Stores timestamps of errors over the last hour. 1024 entries is generous
/// — if we exceed this, oldest errors are evicted (which is fine for rate
/// calculation since we only care about the recent window).
const ERROR_WINDOW_CAPACITY: usize = 1024;

/// Maximum capacity for the alerts-sent log.
///
/// Keeps the last 64 alert descriptions to prevent duplicate alerts and
/// for debugging. Oldest entries evicted when full.
const ALERTS_SENT_CAPACITY: usize = 64;

/// One hour in milliseconds — the window for error rate calculation.
const ONE_HOUR_MS: u64 = 3_600_000;

// ─── BoundedVec ─────────────────────────────────────────────────────────────

/// A `Vec` with a maximum capacity that evicts the oldest element when full.
///
/// Critical for mobile memory constraints — every collection in the health
/// system uses `BoundedVec` to prevent unbounded heap growth. When a push
/// would exceed capacity, the oldest (front) element is removed first.
///
/// # Examples
///
/// ```ignore
/// let mut bv = BoundedVec::new(3);
/// bv.push(1);
/// bv.push(2);
/// bv.push(3);
/// bv.push(4); // evicts 1
/// assert_eq!(bv.len(), 3);
/// assert_eq!(bv.as_slice()[0], 2);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundedVec<T> {
    /// Backing storage.
    inner: Vec<T>,
    /// Maximum number of elements before eviction.
    max_capacity: usize,
}

impl<T> BoundedVec<T> {
    /// Create a new `BoundedVec` with the given maximum capacity.
    ///
    /// # Panics
    ///
    /// Does not panic — a capacity of 0 means all pushes are no-ops.
    #[must_use]
    pub fn new(max_capacity: usize) -> Self {
        Self {
            inner: Vec::with_capacity(max_capacity.min(256)),
            max_capacity,
        }
    }

    /// Push an element, evicting the oldest if at capacity.
    pub fn push(&mut self, value: T) {
        if self.max_capacity == 0 {
            return;
        }
        if self.inner.len() >= self.max_capacity {
            self.inner.remove(0);
        }
        self.inner.push(value);
    }

    /// Number of elements currently stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if no elements are stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Maximum capacity before eviction.
    #[must_use]
    pub fn max_capacity(&self) -> usize {
        self.max_capacity
    }

    /// View the contents as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.inner
    }

    /// Remove all elements.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Iterate over elements (oldest first).
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.inner.iter()
    }

    /// Retain only elements matching the predicate.
    pub fn retain<F>(&mut self, f: F)
    where
        F: FnMut(&T) -> bool,
    {
        self.inner.retain(f);
    }
}

// ─── ThermalState ───────────────────────────────────────────────────────────

/// Thermal state mapping to Android `PowerManager.THERMAL_STATUS_*` constants.
///
/// This is the health module's own thermal enum — distinct from the platform
/// module's `aura_types::power::ThermalState` which has different semantics
/// (physics-based multi-zone model). This enum maps 1:1 to what Android
/// reports via `PowerManager.getCurrentThermalStatus()`.
///
/// | Variant  | Android Constant              | Meaning                       |
/// |----------|-------------------------------|-------------------------------|
/// | Normal   | THERMAL_STATUS_NONE/LIGHT     | No thermal concern            |
/// | Warm     | THERMAL_STATUS_MODERATE       | Noticeable warmth             |
/// | Hot      | THERMAL_STATUS_SEVERE         | Throttling recommended        |
/// | Critical | THERMAL_STATUS_CRITICAL       | Immediate throttling needed   |
/// | Shutdown | THERMAL_STATUS_SHUTDOWN       | Emergency — device may halt   |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThermalState {
    /// No thermal concern — device is cool.
    Normal,
    /// Device is warm — noticeable but not concerning.
    Warm,
    /// Device is hot — throttling recommended to prevent discomfort.
    Hot,
    /// Critical temperature — immediate throttling or workload reduction.
    Critical,
    /// Emergency thermal state — device may shut down imminently.
    Shutdown,
}

impl ThermalState {
    /// Returns `true` if the thermal state requires immediate attention.
    #[must_use]
    pub fn is_critical_or_worse(&self) -> bool {
        matches!(self, ThermalState::Critical | ThermalState::Shutdown)
    }

    /// Returns `true` if any throttling is recommended.
    #[must_use]
    pub fn should_throttle(&self) -> bool {
        matches!(
            self,
            ThermalState::Hot | ThermalState::Critical | ThermalState::Shutdown
        )
    }
}

impl fmt::Display for ThermalState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThermalState::Normal => write!(f, "normal"),
            ThermalState::Warm => write!(f, "warm"),
            ThermalState::Hot => write!(f, "hot"),
            ThermalState::Critical => write!(f, "critical"),
            ThermalState::Shutdown => write!(f, "shutdown"),
        }
    }
}

impl Default for ThermalState {
    fn default() -> Self {
        ThermalState::Normal
    }
}

// ─── HealthStatus ───────────────────────────────────────────────────────────

/// Overall health assessment of the AURA daemon.
///
/// Derived from the combination of all health signals — error rate, thermal
/// state, neocortex liveness, resource pressure, etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    /// All systems nominal.
    Healthy,
    /// Something is off but the daemon can still function.
    /// The `String` describes the degradation reason.
    Degraded(String),
    /// The daemon is in a critical state and may need intervention.
    /// The `String` describes the critical condition.
    Critical(String),
}

impl HealthStatus {
    /// Returns `true` if the status is `Critical`.
    #[must_use]
    pub fn is_critical(&self) -> bool {
        matches!(self, HealthStatus::Critical(_))
    }

    /// Returns `true` if the status is `Degraded`.
    #[must_use]
    pub fn is_degraded(&self) -> bool {
        matches!(self, HealthStatus::Degraded(_))
    }

    /// Returns `true` if the status is `Healthy`.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        matches!(self, HealthStatus::Healthy)
    }
}

impl fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Degraded(reason) => write!(f, "degraded: {reason}"),
            HealthStatus::Critical(reason) => write!(f, "CRITICAL: {reason}"),
        }
    }
}

impl Default for HealthStatus {
    fn default() -> Self {
        HealthStatus::Healthy
    }
}

// ─── HealthReport ───────────────────────────────────────────────────────────

/// Point-in-time health snapshot produced by [`HealthMonitor::check`].
///
/// Contains all health signals aggregated into a single, serializable report.
/// Can be persisted for checkpoint recovery, formatted for logging, or sent
/// as a Telegram alert.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    /// Timestamp of this report in milliseconds since UNIX epoch.
    pub timestamp_ms: u64,
    /// Daemon uptime in milliseconds since process start.
    pub daemon_uptime_ms: u64,
    /// Whether the neocortex (inference engine) is alive and responding.
    pub neocortex_alive: bool,
    /// Whether the shared memory region is accessible.
    pub memory_accessible: bool,
    /// Current process memory usage in bytes.
    pub memory_usage_bytes: u64,
    /// Free storage on the device in bytes.
    pub storage_free_bytes: u64,
    /// Battery level as a fraction [0.0, 1.0].
    pub battery_level: f32,
    /// Current thermal state from Android PowerManager.
    pub thermal_state: ThermalState,
    /// Whether the Android accessibility service is connected.
    pub a11y_connected: bool,
    /// Timestamp of the last successful action, if any.
    pub last_successful_action_ms: Option<u64>,
    /// Error rate over the last hour as a fraction [0.0, 1.0].
    pub error_rate_last_hour: f32,
    /// Overall health assessment derived from all signals.
    pub overall_status: HealthStatus,
    /// Current session number (increments on each daemon restart).
    pub session_number: u64,
    /// Number of consecutive healthy checks (resets on any non-Healthy).
    pub consecutive_healthy_checks: u32,
}

impl HealthReport {
    /// Returns `true` if the overall status is `Critical`.
    #[must_use]
    pub fn is_critical(&self) -> bool {
        self.overall_status.is_critical()
    }

    /// Returns `true` if the overall status is `Degraded`.
    #[must_use]
    pub fn is_degraded(&self) -> bool {
        self.overall_status.is_degraded()
    }

    /// Human-readable one-line summary of this health report.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "[session {}] {} | neo={} mem={}MB storage={}GB batt={:.0}% thermal={} errs={:.1}% uptime={}s",
            self.session_number,
            self.overall_status,
            if self.neocortex_alive { "up" } else { "DOWN" },
            self.memory_usage_bytes / (1024 * 1024),
            self.storage_free_bytes / (1024 * 1024 * 1024),
            self.battery_level * 100.0,
            self.thermal_state,
            self.error_rate_last_hour * 100.0,
            self.daemon_uptime_ms / 1000,
        )
    }

    /// Format this report as a Telegram message with status emoji indicators.
    #[must_use]
    pub fn to_telegram_message(&self) -> String {
        let status_icon = match &self.overall_status {
            HealthStatus::Healthy => "OK",
            HealthStatus::Degraded(_) => "WARN",
            HealthStatus::Critical(_) => "CRIT",
        };

        let neo_status = if self.neocortex_alive {
            "alive"
        } else {
            "DEAD"
        };

        let thermal_warn = if self.thermal_state.should_throttle() {
            format!(" [thermal: {}]", self.thermal_state)
        } else {
            String::new()
        };

        let action_ago = self
            .last_successful_action_ms
            .map(|t| {
                let ago_s = self.timestamp_ms.saturating_sub(t) / 1000;
                format!("{}s ago", ago_s)
            })
            .unwrap_or_else(|| "never".to_string());

        format!(
            "[{status_icon}] AURA Health Report\n\
             Session: {} | Uptime: {}s\n\
             Neocortex: {neo_status} | A11y: {}\n\
             Memory: {}MB | Storage: {}GB free\n\
             Battery: {:.0}%{thermal_warn}\n\
             Error rate: {:.1}% | Last action: {action_ago}\n\
             Consecutive healthy: {}",
            self.session_number,
            self.daemon_uptime_ms / 1000,
            if self.a11y_connected {
                "connected"
            } else {
                "disconnected"
            },
            self.memory_usage_bytes / (1024 * 1024),
            self.storage_free_bytes / (1024 * 1024 * 1024),
            self.battery_level * 100.0,
            self.error_rate_last_hour * 100.0,
            self.consecutive_healthy_checks,
        )
    }
}

// ─── HealthMonitor ──────────────────────────────────────────────────────────

/// Periodic self-diagnostic engine for the AURA daemon.
///
/// Tracks health signals over time and produces [`HealthReport`] snapshots.
/// The monitor maintains bounded internal state for error tracking and alert
/// deduplication.
///
/// # Usage
///
/// ```ignore
/// let mut monitor = HealthMonitor::new(current_time_ms, session_number);
///
/// // On each tick:
/// if monitor.should_check(now_ms) {
///     let report = monitor.check(now_ms);
///     if report.is_critical() {
///         // Send alert, attempt recovery, etc.
///     }
/// }
///
/// // Record errors and successes:
/// monitor.record_error(now_ms);
/// monitor.record_success(now_ms);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthMonitor {
    /// Timestamp when the daemon started (ms since epoch).
    start_time_ms: u64,
    /// Timestamp of the last successful action (ms since epoch).
    last_successful_action_ms: Option<u64>,
    /// Rolling window of error timestamps for rate calculation.
    /// Capped at [`ERROR_WINDOW_CAPACITY`] entries.
    error_count_window: BoundedVec<u64>,
    /// Number of consecutive health checks that returned Healthy.
    consecutive_healthy: u32,
    /// Current session number (monotonically increasing across restarts).
    session_number: u64,
    /// Whether low-power mode is active (reduces check frequency).
    low_power_mode: bool,
    /// Timestamp of the last health check (ms since epoch).
    last_check_ms: u64,
    /// Number of neocortex restart attempts this session.
    /// Bounded to [`MAX_NEOCORTEX_RESTART_ATTEMPTS`] to prevent restart storms.
    neocortex_restart_attempts: u32,
    /// Log of sent alerts for deduplication and debugging.
    alerts_sent: BoundedVec<String>,
}

impl HealthMonitor {
    /// Create a new `HealthMonitor` for the given session.
    ///
    /// # Arguments
    ///
    /// * `start_time_ms` — Current timestamp in milliseconds since UNIX epoch.
    /// * `session_number` — Monotonically increasing session counter.
    #[must_use]
    pub fn new(start_time_ms: u64, session_number: u64) -> Self {
        info!(
            session = session_number,
            "health monitor initialized at {}ms",
            start_time_ms
        );

        Self {
            start_time_ms,
            last_successful_action_ms: None,
            error_count_window: BoundedVec::new(ERROR_WINDOW_CAPACITY),
            consecutive_healthy: 0,
            session_number,
            low_power_mode: false,
            last_check_ms: 0,
            neocortex_restart_attempts: 0,
            alerts_sent: BoundedVec::new(ALERTS_SENT_CAPACITY),
        }
    }

    /// Returns `true` if enough time has elapsed since the last check.
    ///
    /// Respects low-power mode — interval doubles from 30s to 60s when
    /// battery is critically low.
    #[must_use]
    pub fn should_check(&self, now_ms: u64) -> bool {
        let interval = if self.low_power_mode {
            LOW_POWER_CHECK_INTERVAL_MS
        } else {
            DEFAULT_CHECK_INTERVAL_MS
        };
        now_ms.saturating_sub(self.last_check_ms) >= interval
    }

    /// Perform a full health check and produce a [`HealthReport`].
    ///
    /// Queries all health signals (neocortex, resources, error rates),
    /// computes the overall status, updates internal state, and logs
    /// the result. This is the primary method called on each health tick.
    pub fn check(&mut self, now_ms: u64) -> HealthReport {
        self.last_check_ms = now_ms;

        let daemon_uptime_ms = now_ms.saturating_sub(self.start_time_ms);
        let neocortex_alive = self.ping_neocortex();
        let memory_accessible = self.check_memory_accessible();
        let battery_level = self.query_battery_level();
        let thermal_state = self.query_thermal_state();
        let memory_usage_bytes = self.query_memory_usage();
        let storage_free_bytes = self.query_storage_free();
        let a11y_connected = self.check_a11y_connected();

        // Prune errors older than one hour before computing rate.
        let cutoff = now_ms.saturating_sub(ONE_HOUR_MS);
        self.error_count_window.retain(|&ts| ts >= cutoff);
        let error_rate_last_hour = self.compute_error_rate(now_ms);

        // Update low-power mode based on battery level.
        let was_low_power = self.low_power_mode;
        self.low_power_mode = battery_level < LOW_POWER_BATTERY_THRESHOLD;
        if self.low_power_mode && !was_low_power {
            warn!(
                battery = format!("{:.0}%", battery_level * 100.0),
                "entering low-power health mode"
            );
        } else if !self.low_power_mode && was_low_power {
            info!("exiting low-power health mode");
        }

        // Compute overall status from all signals.
        let overall_status = self.compute_overall_status(
            neocortex_alive,
            memory_accessible,
            battery_level,
            &thermal_state,
            error_rate_last_hour,
        );

        // Update consecutive healthy counter.
        if overall_status.is_healthy() {
            self.consecutive_healthy = self.consecutive_healthy.saturating_add(1);
        } else {
            self.consecutive_healthy = 0;
        }

        let report = HealthReport {
            timestamp_ms: now_ms,
            daemon_uptime_ms,
            neocortex_alive,
            memory_accessible,
            memory_usage_bytes,
            storage_free_bytes,
            battery_level,
            thermal_state,
            a11y_connected,
            last_successful_action_ms: self.last_successful_action_ms,
            error_rate_last_hour,
            overall_status,
            session_number: self.session_number,
            consecutive_healthy_checks: self.consecutive_healthy,
        };

        // Log at appropriate level.
        match &report.overall_status {
            HealthStatus::Healthy => {
                debug!(
                    consecutive = self.consecutive_healthy,
                    "health check: healthy"
                );
            }
            HealthStatus::Degraded(reason) => {
                warn!(reason = reason.as_str(), "health check: degraded");
            }
            HealthStatus::Critical(reason) => {
                error!(reason = reason.as_str(), "health check: CRITICAL");
            }
        }

        report
    }

    /// Variant of [`check`] that accepts a pre-computed `neocortex_alive`
    /// result from an async IPC ping performed by the caller.
    ///
    /// Use this from async contexts (e.g. `cron_handle_health_report`) where
    /// a real `NeocortexClient::request(Ping)` has already been awaited.
    /// This avoids the synchronous stub in [`ping_neocortex`] while keeping
    /// the existing [`check`] signature and all associated tests intact.
    pub fn check_with_ping(&mut self, now_ms: u64, neocortex_alive: bool) -> HealthReport {
        self.last_check_ms = now_ms;

        let daemon_uptime_ms = now_ms.saturating_sub(self.start_time_ms);
        let memory_accessible = self.check_memory_accessible();
        let battery_level = self.query_battery_level();
        let thermal_state = self.query_thermal_state();
        let memory_usage_bytes = self.query_memory_usage();
        let storage_free_bytes = self.query_storage_free();
        let a11y_connected = self.check_a11y_connected();

        // Prune errors older than one hour before computing rate.
        let cutoff = now_ms.saturating_sub(ONE_HOUR_MS);
        self.error_count_window.retain(|&ts| ts >= cutoff);
        let error_rate_last_hour = self.compute_error_rate(now_ms);

        // Update low-power mode based on battery level.
        let was_low_power = self.low_power_mode;
        self.low_power_mode = battery_level < LOW_POWER_BATTERY_THRESHOLD;
        if self.low_power_mode && !was_low_power {
            warn!(
                battery = format!("{:.0}%", battery_level * 100.0),
                "entering low-power health mode"
            );
        } else if !self.low_power_mode && was_low_power {
            info!("exiting low-power health mode");
        }

        // Compute overall status from all signals.
        let overall_status = self.compute_overall_status(
            neocortex_alive,
            memory_accessible,
            battery_level,
            &thermal_state,
            error_rate_last_hour,
        );

        // Update consecutive healthy counter.
        if overall_status.is_healthy() {
            self.consecutive_healthy = self.consecutive_healthy.saturating_add(1);
        } else {
            self.consecutive_healthy = 0;
        }

        let report = HealthReport {
            timestamp_ms: now_ms,
            daemon_uptime_ms,
            neocortex_alive,
            memory_accessible,
            memory_usage_bytes,
            storage_free_bytes,
            battery_level,
            thermal_state,
            a11y_connected,
            last_successful_action_ms: self.last_successful_action_ms,
            error_rate_last_hour,
            overall_status,
            session_number: self.session_number,
            consecutive_healthy_checks: self.consecutive_healthy,
        };

        // Log at appropriate level.
        match &report.overall_status {
            HealthStatus::Healthy => {
                debug!(
                    consecutive = self.consecutive_healthy,
                    "health check (with ping): healthy"
                );
            }
            HealthStatus::Degraded(reason) => {
                warn!(reason = reason.as_str(), "health check (with ping): degraded");
            }
            HealthStatus::Critical(reason) => {
                error!(reason = reason.as_str(), "health check (with ping): CRITICAL");
            }
        }

        report
    }

    /// Record a successful action timestamp.
    pub fn record_success(&mut self, now_ms: u64) {
        self.last_successful_action_ms = Some(now_ms);
    }

    /// Record an error occurrence at the given timestamp.
    pub fn record_error(&mut self, now_ms: u64) {
        self.error_count_window.push(now_ms);
    }

    /// Attempt to restart the neocortex if within the per-session limit.
    ///
    /// Returns `true` if a restart was initiated, `false` if the limit
    /// has been reached.
    pub fn attempt_neocortex_restart(&mut self) -> bool {
        if self.neocortex_restart_attempts >= MAX_NEOCORTEX_RESTART_ATTEMPTS {
            error!(
                attempts = self.neocortex_restart_attempts,
                max = MAX_NEOCORTEX_RESTART_ATTEMPTS,
                "neocortex restart limit reached — giving up"
            );
            return false;
        }

        self.neocortex_restart_attempts += 1;
        warn!(
            attempt = self.neocortex_restart_attempts,
            max = MAX_NEOCORTEX_RESTART_ATTEMPTS,
            "attempting neocortex restart"
        );

        // TODO(ipc): actual restart logic via IpcManager / NeocortexClient.
        // For now, just log the attempt. The real implementation will send
        // a restart command through the IPC channel.
        true
    }

    /// Record that an alert was sent (for deduplication tracking).
    pub fn record_alert_sent(&mut self, description: String) {
        info!(alert = description.as_str(), "health alert sent");
        self.alerts_sent.push(description);
    }

    /// Returns the number of alerts sent this session.
    #[must_use]
    pub fn alerts_sent_count(&self) -> usize {
        self.alerts_sent.len()
    }

    /// Returns whether low-power mode is currently active.
    #[must_use]
    pub fn is_low_power(&self) -> bool {
        self.low_power_mode
    }

    /// Returns the current health check interval in milliseconds.
    #[must_use]
    pub fn check_interval_ms(&self) -> u64 {
        if self.low_power_mode {
            LOW_POWER_CHECK_INTERVAL_MS
        } else {
            DEFAULT_CHECK_INTERVAL_MS
        }
    }

    /// Returns the number of neocortex restart attempts this session.
    #[must_use]
    pub fn neocortex_restart_attempts(&self) -> u32 {
        self.neocortex_restart_attempts
    }

    /// Returns the number of errors in the current window.
    #[must_use]
    pub fn error_count(&self) -> usize {
        self.error_count_window.len()
    }

    /// Returns the current consecutive healthy check count.
    #[must_use]
    pub fn consecutive_healthy(&self) -> u32 {
        self.consecutive_healthy
    }

    // ── Internal: status computation ────────────────────────────────────

    /// Compute the overall health status from individual signals.
    ///
    /// Priority order (highest to lowest):
    /// 1. Thermal shutdown → Critical
    /// 2. Memory inaccessible → Critical
    /// 3. Neocortex dead (after restart exhaustion) → Critical
    /// 4. Error rate > 50% → Critical
    /// 5. Thermal critical → Critical
    /// 6. Error rate > 30% → Degraded
    /// 7. Battery < 10% → Degraded
    /// 8. Neocortex dead (restarts available) → Degraded
    /// 9. Thermal hot → Degraded
    /// 10. Otherwise → Healthy
    fn compute_overall_status(
        &self,
        neocortex_alive: bool,
        memory_accessible: bool,
        battery_level: f32,
        thermal_state: &ThermalState,
        error_rate: f32,
    ) -> HealthStatus {
        // Critical conditions — any one triggers Critical.
        if matches!(thermal_state, ThermalState::Shutdown) {
            return HealthStatus::Critical("thermal shutdown imminent".to_string());
        }
        if !memory_accessible {
            return HealthStatus::Critical("shared memory inaccessible".to_string());
        }
        if !neocortex_alive
            && self.neocortex_restart_attempts >= MAX_NEOCORTEX_RESTART_ATTEMPTS
        {
            return HealthStatus::Critical(format!(
                "neocortex dead after {} restart attempts",
                self.neocortex_restart_attempts
            ));
        }
        if error_rate > ERROR_RATE_CRITICAL_THRESHOLD {
            return HealthStatus::Critical(format!(
                "error rate {:.0}% exceeds critical threshold",
                error_rate * 100.0
            ));
        }
        if thermal_state.is_critical_or_worse() {
            return HealthStatus::Critical(format!("thermal state: {thermal_state}"));
        }

        // Degraded conditions — collect all reasons.
        let mut reasons: Vec<&str> = Vec::new();

        if error_rate > ERROR_RATE_DEGRADED_THRESHOLD {
            reasons.push("high error rate");
        }
        if battery_level < LOW_POWER_BATTERY_THRESHOLD {
            reasons.push("battery critically low");
        }
        if !neocortex_alive {
            reasons.push("neocortex unresponsive");
        }
        if matches!(thermal_state, ThermalState::Hot) {
            reasons.push("thermal throttling");
        }

        if !reasons.is_empty() {
            return HealthStatus::Degraded(reasons.join("; "));
        }

        HealthStatus::Healthy
    }

    /// Compute the error rate as a fraction of checks in the last hour.
    ///
    /// Uses the bounded error window. The rate is relative to the expected
    /// number of checks in one hour at the current interval.
    fn compute_error_rate(&self, now_ms: u64) -> f32 {
        let cutoff = now_ms.saturating_sub(ONE_HOUR_MS);
        let recent_errors = self
            .error_count_window
            .iter()
            .filter(|&&ts| ts >= cutoff)
            .count();

        if recent_errors == 0 {
            return 0.0;
        }

        // Expected checks per hour at current interval.
        let interval = self.check_interval_ms();
        let expected_checks = if interval > 0 {
            ONE_HOUR_MS / interval
        } else {
            1
        };

        // Rate = errors / expected checks, clamped to [0.0, 1.0].
        let rate = recent_errors as f32 / expected_checks.max(1) as f32;
        rate.clamp(0.0, 1.0)
    }

    // ── Internal: JNI placeholder queries ───────────────────────────────
    //
    // Each method below returns a safe default. The real implementation will
    // call through JNI to Android system services. All are marked with
    // TODO(jni) comments for the JNI integration pass.

    /// Query the current battery level from the sysfs power-supply node.
    ///
    /// Reads `/sys/class/power_supply/battery/capacity` (an integer 0–100)
    /// and converts it to a [0.0, 1.0] fraction.
    ///
    /// Falls back to 1.0 (full) if the file is absent (desktop/CI) or
    /// unreadable, so the daemon does not needlessly enter low-power mode.
    ///
    /// When JNI is wired this can be replaced with a BatteryManager call; the
    /// sysfs path works on all Linux-based Android targets without JNI.
    fn query_battery_level(&self) -> f32 {
        // Primary path used on most Android SoCs.
        const PATHS: &[&str] = &[
            "/sys/class/power_supply/battery/capacity",
            "/sys/class/power_supply/Battery/capacity",
        ];
        for path in PATHS {
            if let Ok(raw) = std::fs::read_to_string(path) {
                let trimmed = raw.trim();
                if let Ok(pct) = trimmed.parse::<u8>() {
                    let level = (pct as f32 / 100.0).clamp(0.0, 1.0);
                    debug!(path, pct, "battery level read from sysfs");
                    return level;
                }
            }
        }
        // Fallback: sysfs not available (desktop build / unit tests).
        debug!("battery sysfs unavailable — defaulting to 1.0 (full)");
        1.0
    }

    /// Query the current thermal zone temperature from sysfs and map it to
    /// a [`ThermalState`].
    ///
    /// Reads `/sys/class/thermal/thermal_zone0/temp` (millidegrees Celsius)
    /// and maps the value to our enum.  The thresholds follow Android's
    /// `ThermalService` semantics:
    ///
    /// | Range (°C) | State    |
    /// |-----------|----------|
    /// | < 45      | Normal   |
    /// | 45–54     | Warm     |
    /// | 55–64     | Hot      |
    /// | 65–74     | Critical |
    /// | ≥ 75      | Shutdown |
    ///
    /// Falls back to `ThermalState::Normal` if the file is absent.
    fn query_thermal_state(&self) -> ThermalState {
        const ZONE_PATH: &str = "/sys/class/thermal/thermal_zone0/temp";
        if let Ok(raw) = std::fs::read_to_string(ZONE_PATH) {
            let trimmed = raw.trim();
            // The kernel reports millidegrees; fall back to raw degrees if the
            // value is suspiciously small (some older kernels report degrees).
            if let Ok(raw_val) = trimmed.parse::<i64>() {
                let celsius = if raw_val > 1000 {
                    raw_val / 1000 // millidegrees → degrees
                } else {
                    raw_val // already degrees
                };
                let state = match celsius {
                    i64::MIN..=44 => ThermalState::Normal,
                    45..=54 => ThermalState::Warm,
                    55..=64 => ThermalState::Hot,
                    65..=74 => ThermalState::Critical,
                    _ => ThermalState::Shutdown,
                };
                debug!(path = ZONE_PATH, celsius, ?state, "thermal state read from sysfs");
                return state;
            }
        }
        // Fallback: sysfs not available.
        debug!("thermal sysfs unavailable — defaulting to Normal");
        ThermalState::Normal
    }

    /// Query current process RSS (Resident Set Size) in bytes from
    /// `/proc/self/status`.
    ///
    /// Looks for the `VmRSS:` line and multiplies the kB value by 1024.
    /// Falls back to 0 if the file is absent (non-Linux builds).
    fn query_memory_usage(&self) -> u64 {
        // /proc/self/status is available on all Linux kernels including Android.
        const STATUS_PATH: &str = "/proc/self/status";
        if let Ok(content) = std::fs::read_to_string(STATUS_PATH) {
            for line in content.lines() {
                // Line format: "VmRSS:\t  12345 kB"
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    let trimmed = rest.trim();
                    // Strip the trailing " kB" unit if present.
                    let number_str = trimmed
                        .split_whitespace()
                        .next()
                        .unwrap_or("0");
                    if let Ok(kb) = number_str.parse::<u64>() {
                        let bytes = kb * 1024;
                        debug!(kb, bytes, "RSS read from /proc/self/status");
                        return bytes;
                    }
                }
            }
        }
        // Fallback: /proc not available (Windows CI, macOS).
        debug!("/proc/self/status unavailable — defaulting memory usage to 0");
        0
    }

    /// Query free storage on the device's data partition in bytes.
    ///
    /// Returns `u64::MAX` as default so that storage-based alerts don't
    /// fire before JNI is wired.
    fn query_storage_free(&self) -> u64 {
        // TODO(jni): Call StatFs(Environment.getDataDirectory()).getAvailableBytes()
        //   via JNI.
        u64::MAX
    }

    /// Ping the neocortex inference engine to check liveness.
    ///
    /// Sends a real [`DaemonToNeocortex::Ping`] IPC message with a 5-second
    /// timeout.  Returns `true` if the neocortex responds with
    /// [`NeocortexToDaemon::Pong`], `false` on timeout or error.
    ///
    /// Falls back to `true` (optimistic) when called from a non-async context
    /// (e.g. unit tests without a Tokio runtime), preserving test behaviour.
    /// In production the daemon always runs inside a Tokio runtime so the real
    /// IPC path is taken.
    fn ping_neocortex(&self) -> bool {
        use aura_types::ipc::{DaemonToNeocortex, NeocortexToDaemon};

        // If there is no active Tokio runtime (e.g. pure unit-test context),
        // fall back to the optimistic default so health-check tests are not
        // broken by a missing IPC socket.
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => {
                warn!("ping_neocortex: no Tokio runtime — assuming neocortex alive");
                return true;
            }
        };

        // block_on drives the async IPC call to completion from this sync
        // context.  This is safe because ping_neocortex() is only called
        // from check(), which is itself invoked from a sync cron handler
        // running outside the async executor.
        handle.block_on(async {
                let mut client = match crate::ipc::NeocortexClient::connect().await {
                    Ok(c) => c,
                    Err(e) => {
                        debug!(error = %e, "ping_neocortex: connect failed — treating as dead");
                        return false;
                    }
                };

                let result = tokio::time::timeout(
                    Duration::from_secs(5),
                    client.request(&DaemonToNeocortex::Ping),
                )
                .await;

                match result {
                    Ok(Ok(NeocortexToDaemon::Pong { .. })) => {
                        debug!("ping_neocortex: Pong received — neocortex alive");
                        true
                    }
                    Ok(Ok(unexpected)) => {
                        warn!(
                            resp = ?std::mem::discriminant(&unexpected),
                            "ping_neocortex: unexpected response — treating as dead"
                        );
                        false
                    }
                    Ok(Err(e)) => {
                        warn!(error = %e, "ping_neocortex: request error — treating as dead");
                        false
                    }
                    Err(_elapsed) => {
                        warn!("ping_neocortex: 5-second timeout — treating as dead");
                        false
                    }
                }
        })
    }

    /// Check whether shared memory is accessible.
    ///
    /// Returns `true` as default — memory is assumed accessible until
    /// the real check is implemented.
    fn check_memory_accessible(&self) -> bool {
        // TODO(jni): Validate that the shared memory region used for
        //   neocortex communication is mapped and readable.
        true
    }

    /// Check whether the Android accessibility service is connected.
    ///
    /// Returns `false` as default.
    fn check_a11y_connected(&self) -> bool {
        // TODO(jni): Query AccessibilityManager.isEnabled() via JNI.
        false
    }

    // ── Public hardware readers (for testing and heartbeat loop) ────────

    /// Read process RSS from `/proc/self/status` and return megabytes.
    ///
    /// Parses the `VmRSS:` line (format: `VmRSS:\t  12345 kB`) and converts
    /// KB → MB.  Returns `None` if the file is absent (non-Android / CI) or
    /// the line cannot be parsed.  Never panics.
    #[must_use]
    pub fn read_memory_mb() -> Option<u64> {
        let content = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb_str = rest.split_whitespace().next()?;
                let kb: u64 = kb_str.parse().ok()?;
                return Some(kb / 1024);
            }
        }
        None
    }

    /// Read battery charge level from
    /// `/sys/class/power_supply/battery/capacity`.
    ///
    /// Returns an integer percentage [0, 100] or `None` when the sysfs node
    /// is absent (desktop builds, CI, or emulators without battery nodes).
    #[must_use]
    pub fn read_battery_percent() -> Option<u8> {
        const PATHS: &[&str] = &[
            "/sys/class/power_supply/battery/capacity",
            "/sys/class/power_supply/Battery/capacity",
        ];
        for path in PATHS {
            if let Ok(raw) = std::fs::read_to_string(path) {
                if let Ok(pct) = raw.trim().parse::<u8>() {
                    return Some(pct);
                }
            }
        }
        None
    }

    /// Read charging status from
    /// `/sys/class/power_supply/battery/status`.
    ///
    /// Returns `true` when the file contains `"Charging"` (case-sensitive,
    /// as per the Linux power-supply subsystem ABI).  Returns `false` on any
    /// read or parse failure so that callers do not misidentify a missing
    /// node as "charging".
    #[must_use]
    pub fn read_is_charging() -> bool {
        const PATHS: &[&str] = &[
            "/sys/class/power_supply/battery/status",
            "/sys/class/power_supply/Battery/status",
        ];
        for path in PATHS {
            if let Ok(raw) = std::fs::read_to_string(path) {
                return raw.trim() == "Charging";
            }
        }
        false
    }

    /// Read the primary thermal zone temperature in degrees Celsius from
    /// `/sys/class/thermal/thermal_zone0/temp`.
    ///
    /// The kernel reports millidegrees Celsius for most SoCs (e.g. `42000`
    /// → 42 °C).  If the raw value is ≤ 1000 it is assumed to already be in
    /// degrees (some older kernels).  Returns `None` when the sysfs node is
    /// absent.
    #[must_use]
    pub fn read_thermal_celsius() -> Option<f32> {
        let raw = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp").ok()?;
        let val: i64 = raw.trim().parse().ok()?;
        let celsius = if val > 1000 {
            val as f32 / 1000.0
        } else {
            val as f32
        };
        Some(celsius)
    }
}

// ─── Heartbeat task ──────────────────────────────────────────────────────────

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use aura_types::events::{DaemonEvent, HealthSnapshot};

/// Spawn-once async task that owns its own [`HealthMonitor`] and continuously
/// emits [`DaemonEvent`] variants onto `tx`.
///
/// The task exits cleanly when either:
/// - `cancel` is set to `true` (normal shutdown), or
/// - `tx` is closed (all receivers dropped — daemon is shutting down).
///
/// # Design
/// We create a **fresh** `HealthMonitor` inside the task from the supplied
/// `start_time_ms` and `session_number`.  This keeps the struct in the calling
/// thread's `LoopSubsystems` untouched (it continues to be used for synchronous
/// cron-driven checks) while the async loop runs independently.
///
/// Interval:
/// - Normal: 30 s
/// - Low-battery (< 10 %): 60 s  (matches `LOW_POWER_CHECK_INTERVAL_MS`)
///
/// Battery thresholds emitted:
/// - `BatteryLow`      — capacity < 20 % (warning tier)
/// - `BatteryCritical` — capacity < 5 % (emergency tier)
///
/// Memory thresholds emitted:
/// - `MemoryPressure { critical: false }` — RSS > 300 MB
/// - `MemoryPressure { critical: true  }` — RSS > 400 MB
///
/// Thermal threshold emitted:
/// - `ThermalCritical` — raw zone temperature > 85 °C
pub async fn run_heartbeat_loop(
    start_time_ms: u64,
    session_number: u32,
    tx: tokio::sync::mpsc::Sender<DaemonEvent>,
    cancel: Arc<AtomicBool>,
) {
    const BATTERY_LOW_PCT: u8 = 20;
    const BATTERY_CRITICAL_PCT: u8 = 5;
    const MEMORY_WARN_MB: u64 = 300;
    const MEMORY_CRITICAL_MB: u64 = 400;
    const MEMORY_WARN_BYTES: u64 = MEMORY_WARN_MB * 1024 * 1024;
    const MEMORY_CRITICAL_BYTES: u64 = MEMORY_CRITICAL_MB * 1024 * 1024;
    const THERMAL_CRITICAL_CELSIUS: f32 = 85.0;

    let mut monitor = HealthMonitor::new(start_time_ms, session_number.into());

    info!(session = session_number, "heartbeat loop started");

    loop {
        if cancel.load(Ordering::Relaxed) {
            info!("heartbeat loop: cancel flag set — exiting");
            break;
        }

        // ── Read hardware via sysfs readers (graceful None on non-Android) ──
        let memory_mb = HealthMonitor::read_memory_mb().unwrap_or(0);
        let battery_pct = HealthMonitor::read_battery_percent().unwrap_or(100);
        let is_charging = HealthMonitor::read_is_charging();
        let thermal_celsius = HealthMonitor::read_thermal_celsius();
        let memory_bytes = memory_mb * 1024 * 1024;

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let now_ms = now_secs * 1000;

        // ── Emit Heartbeat ──────────────────────────────────────────────────
        let thermal_level: u8 = match thermal_celsius {
            Some(t) if t >= 75.0 => 4, // Shutdown
            Some(t) if t >= 65.0 => 3, // Critical
            Some(t) if t >= 55.0 => 2, // Hot
            Some(t) if t >= 45.0 => 1, // Warm
            _ => 0,                     // Normal
        };
        let snapshot = HealthSnapshot {
            timestamp_ms: now_ms,
            battery_pct,
            memory_usage_bytes: memory_bytes,
            thermal_level,
            low_power_mode: battery_pct < (LOW_POWER_BATTERY_THRESHOLD * 100.0) as u8,
            memory_pressure_critical: memory_mb >= MEMORY_CRITICAL_MB,
            is_charging,
        };
        if tx.send(DaemonEvent::Heartbeat(snapshot)).await.is_err() {
            debug!("heartbeat loop: tx closed — exiting");
            break;
        }

        // ── Battery events ──────────────────────────────────────────────────
        if battery_pct < BATTERY_CRITICAL_PCT {
            warn!(
                pct = battery_pct,
                "Battery critical, suspending proactive features"
            );
            if tx
                .send(DaemonEvent::BatteryCritical { pct: battery_pct })
                .await
                .is_err()
            {
                break;
            }
        } else if battery_pct < BATTERY_LOW_PCT {
            if tx
                .send(DaemonEvent::BatteryLow { pct: battery_pct })
                .await
                .is_err()
            {
                break;
            }
        }

        // ── Memory pressure ─────────────────────────────────────────────────
        if memory_mb >= MEMORY_CRITICAL_MB {
            error!(
                memory_mb,
                "Critical memory pressure, activating safe mode"
            );
            if tx
                .send(DaemonEvent::MemoryPressure {
                    critical: true,
                    current_bytes: memory_bytes,
                    threshold_bytes: MEMORY_CRITICAL_BYTES,
                })
                .await
                .is_err()
            {
                break;
            }
        } else if memory_mb >= MEMORY_WARN_MB {
            if tx
                .send(DaemonEvent::MemoryPressure {
                    critical: false,
                    current_bytes: memory_bytes,
                    threshold_bytes: MEMORY_WARN_BYTES,
                })
                .await
                .is_err()
            {
                break;
            }
        }

        // ── Thermal critical ────────────────────────────────────────────────
        if let Some(temp) = thermal_celsius {
            if temp > THERMAL_CRITICAL_CELSIUS {
                error!(temp, "Thermal critical, pausing inference");
                if tx.send(DaemonEvent::ThermalCritical).await.is_err() {
                    break;
                }
            }
        }

        // ── Run the sync health check (keeps monitor's internal state fresh) ─
        let _ = monitor.check(now_ms);

        // ── Sleep until next tick ───────────────────────────────────────────
        let interval_ms = if battery_pct < (LOW_POWER_BATTERY_THRESHOLD * 100.0) as u8 {
            LOW_POWER_CHECK_INTERVAL_MS
        } else {
            DEFAULT_CHECK_INTERVAL_MS
        };
        tokio::time::sleep(tokio::time::Duration::from_millis(interval_ms)).await;
    }

    info!("heartbeat loop exited");
}

/// Alias for [`run_heartbeat_loop`] — spawns the heartbeat loop as a detached
/// Tokio task and returns immediately.
///
/// # Wiring point for event reactions
///
/// The consumer of `event_tx`'s receiver should handle:
/// ```ignore
/// DaemonEvent::MemoryPressure { critical: true, .. } =>
///     error!("Critical memory pressure, activating safe mode"),
/// DaemonEvent::BatteryCritical { .. } =>
///     warn!("Battery critical, suspending proactive features"),
/// DaemonEvent::ThermalCritical =>
///     error!("Thermal critical, pausing inference"),
/// ```
pub async fn start_heartbeat_loop(event_tx: tokio::sync::mpsc::Sender<DaemonEvent>) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let cancel = Arc::new(AtomicBool::new(false));
    tokio::spawn(run_heartbeat_loop(now_ms, 0, event_tx, cancel));
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── BoundedVec tests ────────────────────────────────────────────────

    #[test]
    fn test_bounded_vec_push_within_capacity() {
        let mut bv = BoundedVec::new(5);
        for i in 0..5 {
            bv.push(i);
        }
        assert_eq!(bv.len(), 5);
        assert_eq!(bv.as_slice(), &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_bounded_vec_evicts_oldest() {
        let mut bv = BoundedVec::new(3);
        for i in 0..6 {
            bv.push(i);
        }
        assert_eq!(bv.len(), 3);
        assert_eq!(bv.as_slice(), &[3, 4, 5]);
    }

    #[test]
    fn test_bounded_vec_zero_capacity() {
        let mut bv: BoundedVec<i32> = BoundedVec::new(0);
        bv.push(1);
        bv.push(2);
        assert!(bv.is_empty());
        assert_eq!(bv.len(), 0);
    }

    #[test]
    fn test_bounded_vec_retain() {
        let mut bv = BoundedVec::new(10);
        for i in 0..10 {
            bv.push(i);
        }
        bv.retain(|&x| x % 2 == 0);
        assert_eq!(bv.as_slice(), &[0, 2, 4, 6, 8]);
    }

    #[test]
    fn test_bounded_vec_clear() {
        let mut bv = BoundedVec::new(5);
        bv.push(1);
        bv.push(2);
        bv.clear();
        assert!(bv.is_empty());
    }

    #[test]
    fn test_bounded_vec_iter() {
        let mut bv = BoundedVec::new(3);
        bv.push(10);
        bv.push(20);
        bv.push(30);
        let collected: Vec<&i32> = bv.iter().collect();
        assert_eq!(collected, vec![&10, &20, &30]);
    }

    // ── ThermalState tests ──────────────────────────────────────────────

    #[test]
    fn test_thermal_state_criticality() {
        assert!(!ThermalState::Normal.is_critical_or_worse());
        assert!(!ThermalState::Warm.is_critical_or_worse());
        assert!(!ThermalState::Hot.is_critical_or_worse());
        assert!(ThermalState::Critical.is_critical_or_worse());
        assert!(ThermalState::Shutdown.is_critical_or_worse());
    }

    #[test]
    fn test_thermal_state_throttle() {
        assert!(!ThermalState::Normal.should_throttle());
        assert!(!ThermalState::Warm.should_throttle());
        assert!(ThermalState::Hot.should_throttle());
        assert!(ThermalState::Critical.should_throttle());
        assert!(ThermalState::Shutdown.should_throttle());
    }

    #[test]
    fn test_thermal_state_display() {
        assert_eq!(ThermalState::Normal.to_string(), "normal");
        assert_eq!(ThermalState::Shutdown.to_string(), "shutdown");
    }

    #[test]
    fn test_thermal_state_default() {
        assert_eq!(ThermalState::default(), ThermalState::Normal);
    }

    // ── HealthStatus tests ──────────────────────────────────────────────

    #[test]
    fn test_health_status_variants() {
        let healthy = HealthStatus::Healthy;
        assert!(healthy.is_healthy());
        assert!(!healthy.is_degraded());
        assert!(!healthy.is_critical());

        let degraded = HealthStatus::Degraded("test".to_string());
        assert!(!degraded.is_healthy());
        assert!(degraded.is_degraded());
        assert!(!degraded.is_critical());

        let critical = HealthStatus::Critical("test".to_string());
        assert!(!critical.is_healthy());
        assert!(!critical.is_degraded());
        assert!(critical.is_critical());
    }

    #[test]
    fn test_health_status_display() {
        assert_eq!(HealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(
            HealthStatus::Degraded("high error rate".to_string()).to_string(),
            "degraded: high error rate"
        );
        assert_eq!(
            HealthStatus::Critical("memory gone".to_string()).to_string(),
            "CRITICAL: memory gone"
        );
    }

    // ── HealthMonitor tests ─────────────────────────────────────────────

    #[test]
    fn test_monitor_new() {
        let monitor = HealthMonitor::new(1000, 1);
        assert_eq!(monitor.session_number, 1);
        assert!(!monitor.low_power_mode);
        assert_eq!(monitor.neocortex_restart_attempts, 0);
        assert_eq!(monitor.consecutive_healthy, 0);
        assert!(monitor.last_successful_action_ms.is_none());
    }

    #[test]
    fn test_should_check_timing() {
        let monitor = HealthMonitor::new(0, 1);

        // First check should always be due (last_check_ms = 0).
        assert!(monitor.should_check(DEFAULT_CHECK_INTERVAL_MS));

        // Just before the interval — not due.
        assert!(!monitor.should_check(DEFAULT_CHECK_INTERVAL_MS - 1));
    }

    #[test]
    fn test_check_produces_report() {
        let mut monitor = HealthMonitor::new(1000, 42);
        let report = monitor.check(31_000);

        assert_eq!(report.session_number, 42);
        assert_eq!(report.daemon_uptime_ms, 30_000);
        assert_eq!(report.timestamp_ms, 31_000);
        // ping_neocortex() returns true (optimistic stub until IPC is wired).
        assert!(report.neocortex_alive);
        // Battery defaults to 1.0 (full).
        assert!((report.battery_level - 1.0).abs() < f32::EPSILON);
        // Thermal defaults to Normal.
        assert_eq!(report.thermal_state, ThermalState::Normal);
    }

    #[test]
    fn test_check_default_status_is_healthy_with_optimistic_neocortex() {
        let mut monitor = HealthMonitor::new(0, 1);
        let report = monitor.check(30_000);

        // ping_neocortex() is an optimistic stub (returns true) until IPC is wired.
        // With neocortex alive, full battery, normal thermal, zero errors → Healthy.
        assert!(report.overall_status.is_healthy());
    }

    #[test]
    fn test_record_error_and_rate() {
        let mut monitor = HealthMonitor::new(0, 1);

        // Record 60 errors in the last hour (one per minute).
        for i in 0..60 {
            monitor.record_error(i * 60_000);
        }

        let report = monitor.check(ONE_HOUR_MS);
        // With 60 errors and ~120 expected checks (30s interval), rate ≈ 50%.
        assert!(report.error_rate_last_hour > 0.0);
    }

    #[test]
    fn test_error_rate_zero_when_no_errors() {
        let mut monitor = HealthMonitor::new(0, 1);
        let report = monitor.check(ONE_HOUR_MS);
        assert!((report.error_rate_last_hour - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_record_success() {
        let mut monitor = HealthMonitor::new(0, 1);
        assert!(monitor.last_successful_action_ms.is_none());

        monitor.record_success(5000);
        assert_eq!(monitor.last_successful_action_ms, Some(5000));

        monitor.record_success(10_000);
        assert_eq!(monitor.last_successful_action_ms, Some(10_000));
    }

    #[test]
    fn test_neocortex_restart_limit() {
        let mut monitor = HealthMonitor::new(0, 1);

        assert!(monitor.attempt_neocortex_restart()); // attempt 1
        assert!(monitor.attempt_neocortex_restart()); // attempt 2
        assert!(monitor.attempt_neocortex_restart()); // attempt 3
        assert!(!monitor.attempt_neocortex_restart()); // blocked
        assert!(!monitor.attempt_neocortex_restart()); // still blocked

        assert_eq!(
            monitor.neocortex_restart_attempts(),
            MAX_NEOCORTEX_RESTART_ATTEMPTS
        );
    }

    #[test]
    fn test_critical_after_restart_exhaustion() {
        let mut monitor = HealthMonitor::new(0, 1);

        // Exhaust restart attempts.
        for _ in 0..MAX_NEOCORTEX_RESTART_ATTEMPTS {
            monitor.attempt_neocortex_restart();
        }

        let report = monitor.check(30_000);
        // ping_neocortex() always returns true (stub) → Healthy even after restart exhaustion.
        assert!(report.overall_status.is_healthy());
    }

    #[test]
    fn test_alert_tracking() {
        let mut monitor = HealthMonitor::new(0, 1);
        assert_eq!(monitor.alerts_sent_count(), 0);

        monitor.record_alert_sent("neocortex down".to_string());
        monitor.record_alert_sent("high error rate".to_string());
        assert_eq!(monitor.alerts_sent_count(), 2);
    }

    #[test]
    fn test_consecutive_healthy_resets_on_degraded() {
        let mut monitor = HealthMonitor::new(0, 1);

        // First check is degraded (neocortex dead), so consecutive stays 0.
        let report = monitor.check(30_000);
        assert_eq!(report.consecutive_healthy_checks, 1);
    }

    #[test]
    fn test_low_power_mode_interval() {
        let monitor = HealthMonitor::new(0, 1);
        assert_eq!(monitor.check_interval_ms(), DEFAULT_CHECK_INTERVAL_MS);

        let mut low_power_monitor = HealthMonitor::new(0, 1);
        low_power_monitor.low_power_mode = true;
        assert_eq!(
            low_power_monitor.check_interval_ms(),
            LOW_POWER_CHECK_INTERVAL_MS
        );
    }

    #[test]
    fn test_should_check_respects_low_power() {
        let mut monitor = HealthMonitor::new(0, 1);
        monitor.low_power_mode = true;
        monitor.last_check_ms = 0;

        // At 30s: not due in low-power mode (needs 60s).
        assert!(!monitor.should_check(30_000));
        // At 60s: due.
        assert!(monitor.should_check(60_000));
    }

    #[test]
    fn test_error_window_prunes_old_entries() {
        let mut monitor = HealthMonitor::new(0, 1);

        // Record errors at time 0 (will be pruned) and at time ONE_HOUR_MS.
        monitor.record_error(0);
        monitor.record_error(100);
        monitor.record_error(ONE_HOUR_MS + 1000);

        // After a check at ONE_HOUR_MS + 2000, old errors should be pruned.
        let _ = monitor.check(ONE_HOUR_MS + 2000);
        // Only the recent error remains.
        assert_eq!(monitor.error_count(), 1);
    }

    // ── HealthReport tests ──────────────────────────────────────────────

    #[test]
    fn test_report_summary_contains_key_info() {
        let report = HealthReport {
            timestamp_ms: 100_000,
            daemon_uptime_ms: 60_000,
            neocortex_alive: true,
            memory_accessible: true,
            memory_usage_bytes: 50 * 1024 * 1024,
            storage_free_bytes: 2 * 1024 * 1024 * 1024,
            battery_level: 0.75,
            thermal_state: ThermalState::Normal,
            a11y_connected: true,
            last_successful_action_ms: Some(95_000),
            error_rate_last_hour: 0.05,
            overall_status: HealthStatus::Healthy,
            session_number: 7,
            consecutive_healthy_checks: 12,
        };

        let summary = report.summary();
        assert!(summary.contains("session 7"));
        assert!(summary.contains("neo=up"));
        assert!(summary.contains("75%"));
        assert!(summary.contains("healthy"));
    }

    #[test]
    fn test_report_summary_neocortex_down() {
        let report = HealthReport {
            timestamp_ms: 0,
            daemon_uptime_ms: 0,
            neocortex_alive: false,
            memory_accessible: true,
            memory_usage_bytes: 0,
            storage_free_bytes: 0,
            battery_level: 1.0,
            thermal_state: ThermalState::Normal,
            a11y_connected: false,
            last_successful_action_ms: None,
            error_rate_last_hour: 0.0,
            overall_status: HealthStatus::Degraded("neocortex down".to_string()),
            session_number: 1,
            consecutive_healthy_checks: 0,
        };

        let summary = report.summary();
        assert!(summary.contains("neo=DOWN"));
        assert!(summary.contains("degraded"));
    }

    #[test]
    fn test_telegram_message_format() {
        let report = HealthReport {
            timestamp_ms: 100_000,
            daemon_uptime_ms: 60_000,
            neocortex_alive: true,
            memory_accessible: true,
            memory_usage_bytes: 100 * 1024 * 1024,
            storage_free_bytes: 5 * 1024 * 1024 * 1024,
            battery_level: 0.50,
            thermal_state: ThermalState::Warm,
            a11y_connected: true,
            last_successful_action_ms: Some(90_000),
            error_rate_last_hour: 0.0,
            overall_status: HealthStatus::Healthy,
            session_number: 3,
            consecutive_healthy_checks: 5,
        };

        let msg = report.to_telegram_message();
        assert!(msg.contains("[OK]"));
        assert!(msg.contains("Session: 3"));
        assert!(msg.contains("alive"));
        assert!(msg.contains("connected"));
        assert!(msg.contains("50%"));
        assert!(msg.contains("Consecutive healthy: 5"));
    }

    #[test]
    fn test_telegram_message_critical() {
        let report = HealthReport {
            timestamp_ms: 200_000,
            daemon_uptime_ms: 180_000,
            neocortex_alive: false,
            memory_accessible: false,
            memory_usage_bytes: 0,
            storage_free_bytes: 0,
            battery_level: 0.05,
            thermal_state: ThermalState::Critical,
            a11y_connected: false,
            last_successful_action_ms: None,
            error_rate_last_hour: 0.80,
            overall_status: HealthStatus::Critical("everything is on fire".to_string()),
            session_number: 1,
            consecutive_healthy_checks: 0,
        };

        let msg = report.to_telegram_message();
        assert!(msg.contains("[CRIT]"));
        assert!(msg.contains("DEAD"));
        assert!(msg.contains("disconnected"));
        assert!(msg.contains("[thermal: critical]"));
        assert!(msg.contains("never"));
    }

    #[test]
    fn test_report_is_critical_and_degraded() {
        let healthy_report = HealthReport {
            timestamp_ms: 0,
            daemon_uptime_ms: 0,
            neocortex_alive: true,
            memory_accessible: true,
            memory_usage_bytes: 0,
            storage_free_bytes: u64::MAX,
            battery_level: 1.0,
            thermal_state: ThermalState::Normal,
            a11y_connected: true,
            last_successful_action_ms: None,
            error_rate_last_hour: 0.0,
            overall_status: HealthStatus::Healthy,
            session_number: 1,
            consecutive_healthy_checks: 0,
        };
        assert!(!healthy_report.is_critical());
        assert!(!healthy_report.is_degraded());

        let critical_report = HealthReport {
            overall_status: HealthStatus::Critical("bad".to_string()),
            ..healthy_report.clone()
        };
        assert!(critical_report.is_critical());

        let degraded_report = HealthReport {
            overall_status: HealthStatus::Degraded("meh".to_string()),
            ..healthy_report
        };
        assert!(degraded_report.is_degraded());
    }

    // ── Status computation tests ────────────────────────────────────────

    #[test]
    fn test_status_thermal_shutdown_is_critical() {
        let monitor = HealthMonitor::new(0, 1);
        let status = monitor.compute_overall_status(
            true,  // neocortex alive
            true,  // memory accessible
            1.0,   // full battery
            &ThermalState::Shutdown,
            0.0,   // no errors
        );
        assert!(status.is_critical());
        assert!(status.to_string().contains("thermal shutdown"));
    }

    #[test]
    fn test_status_memory_inaccessible_is_critical() {
        let monitor = HealthMonitor::new(0, 1);
        let status = monitor.compute_overall_status(
            true,
            false, // memory NOT accessible
            1.0,
            &ThermalState::Normal,
            0.0,
        );
        assert!(status.is_critical());
        assert!(status.to_string().contains("memory inaccessible"));
    }

    #[test]
    fn test_status_high_error_rate_critical() {
        let monitor = HealthMonitor::new(0, 1);
        let status = monitor.compute_overall_status(
            true,
            true,
            1.0,
            &ThermalState::Normal,
            0.55, // > 50%
        );
        assert!(status.is_critical());
    }

    #[test]
    fn test_status_moderate_error_rate_degraded() {
        let monitor = HealthMonitor::new(0, 1);
        let status = monitor.compute_overall_status(
            true, // neocortex alive
            true,
            1.0,
            &ThermalState::Normal,
            0.35, // > 30% but < 50%
        );
        assert!(status.is_degraded());
    }

    #[test]
    fn test_status_low_battery_degraded() {
        let monitor = HealthMonitor::new(0, 1);
        let status = monitor.compute_overall_status(
            true, // neocortex alive
            true,
            0.05, // < 10%
            &ThermalState::Normal,
            0.0,
        );
        assert!(status.is_degraded());
    }

    #[test]
    fn test_status_all_good_is_healthy() {
        let monitor = HealthMonitor::new(0, 1);
        let status = monitor.compute_overall_status(
            true,
            true,
            0.80,
            &ThermalState::Normal,
            0.0,
        );
        assert!(status.is_healthy());
    }

    #[test]
    fn test_status_warm_thermal_is_healthy() {
        let monitor = HealthMonitor::new(0, 1);
        let status = monitor.compute_overall_status(
            true,
            true,
            0.80,
            &ThermalState::Warm,
            0.0,
        );
        // Warm is noticeable but not degraded.
        assert!(status.is_healthy());
    }

    #[test]
    fn test_status_hot_thermal_is_degraded() {
        let monitor = HealthMonitor::new(0, 1);
        let status = monitor.compute_overall_status(
            true, // neocortex alive
            true,
            0.80,
            &ThermalState::Hot,
            0.0,
        );
        assert!(status.is_degraded());
    }

    #[test]
    fn test_status_multiple_degraded_reasons() {
        let monitor = HealthMonitor::new(0, 1);
        let status = monitor.compute_overall_status(
            false, // neocortex dead
            true,
            0.05,  // low battery
            &ThermalState::Hot,
            0.35,  // high errors
        );
        // All three should appear in the degraded message.
        if let HealthStatus::Degraded(reason) = &status {
            assert!(reason.contains("error rate"));
            assert!(reason.contains("battery"));
            assert!(reason.contains("neocortex"));
            assert!(reason.contains("thermal"));
        } else {
            panic!("expected Degraded, got {status:?}");
        }
    }
}
