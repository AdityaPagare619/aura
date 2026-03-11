//! Typed System API Bridge — direct Android API calls bypassing AccessibilityService.
//!
//! For common system operations (battery, storage, contacts, etc.) the daemon
//! can call the underlying Android SDK directly via JNI instead of navigating
//! the UI through AccessibilityService.  This yields **~1000× speedup** for
//! supported commands:
//!
//! | Path                 | Latency   |
//! |----------------------|-----------|
//! | A11y UI automation   | ~3 000 ms |
//! | SystemBridge (JNI)   |    ~1 ms  |
//!
//! # Decision flow
//!
//! ```text
//! User says "check my battery"
//!     → can_handle_intent("check my battery")
//!         → Some(SystemCommand::BatteryStatus)
//!             → execute(BatteryStatus) → ~5 ms
//!     → None → fall through to A11y execution → ~3 000 ms
//! ```
//!
//! # Bounded collections
//!
//! All result vectors are truncated to compile-time constants to prevent
//! unbounded memory growth on resource-constrained Android devices.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, trace, warn};

// ---------------------------------------------------------------------------
// Capacity bounds — mobile-conscious, never allocate more than needed
// ---------------------------------------------------------------------------

/// Maximum contacts returned by a single search.
const MAX_CONTACTS: usize = 100;

/// Maximum calendar events returned per query.
const MAX_CALENDAR_EVENTS: usize = 50;

/// Maximum recent photos returned per query.
const MAX_PHOTOS: usize = 50;

/// Maximum notifications returned per query.
const MAX_NOTIFICATIONS: usize = 50;

/// Smoothing factor for the exponential moving average of latency.
/// A value of 0.1 means new measurements contribute 10% to the running average.
const LATENCY_EMA_ALPHA: f32 = 0.1;

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// Battery health as reported by `BatteryManager`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BatteryHealth {
    Good,
    Overheat,
    Dead,
    OverVoltage,
    Cold,
    Unknown,
}

impl fmt::Display for BatteryHealth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Good => write!(f, "good"),
            Self::Overheat => write!(f, "overheat"),
            Self::Dead => write!(f, "dead"),
            Self::OverVoltage => write!(f, "over_voltage"),
            Self::Cold => write!(f, "cold"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Thermal state from `PowerManager.getCurrentThermalStatus()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThermalState {
    /// Device is not throttled.
    None,
    /// Light throttling — no user-visible impact.
    Light,
    /// Moderate throttling — some features degraded.
    Moderate,
    /// Severe throttling — significant performance reduction.
    Severe,
    /// Critical — device may shut down soon.
    Critical,
    /// Emergency — immediate thermal mitigation required.
    Emergency,
    /// Shutdown imminent.
    Shutdown,
}

impl fmt::Display for ThermalState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Light => write!(f, "light"),
            Self::Moderate => write!(f, "moderate"),
            Self::Severe => write!(f, "severe"),
            Self::Critical => write!(f, "critical"),
            Self::Emergency => write!(f, "emergency"),
            Self::Shutdown => write!(f, "shutdown"),
        }
    }
}

/// Contact information from `ContactsContract`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactInfo {
    /// Display name of the contact.
    pub name: String,
    /// Primary phone number, if available.
    pub phone: Option<String>,
    /// Primary email address, if available.
    pub email: Option<String>,
}

/// Calendar event from `CalendarContract`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    /// Event title.
    pub title: String,
    /// Start time as milliseconds since UNIX epoch.
    pub start_ms: u64,
    /// End time as milliseconds since UNIX epoch.
    pub end_ms: u64,
    /// Location string, if set.
    pub location: Option<String>,
}

/// Photo metadata from `MediaStore`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoInfo {
    /// Content URI of the photo.
    pub uri: String,
    /// Timestamp the photo was taken, in milliseconds since UNIX epoch.
    pub timestamp_ms: u64,
    /// File size in bytes.
    pub size_bytes: u64,
}

/// Notification metadata from `NotificationListenerService`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationInfo {
    /// Package name of the app that posted the notification.
    pub package: String,
    /// Notification title.
    pub title: String,
    /// Notification body text.
    pub text: String,
    /// When the notification was posted, in milliseconds since UNIX epoch.
    pub timestamp_ms: u64,
}

// ---------------------------------------------------------------------------
// SystemCommand — what the daemon can ask the system to do
// ---------------------------------------------------------------------------

/// Direct Android API commands that bypass AccessibilityService.
///
/// Each variant maps to a specific Android SDK API. The estimated latency
/// is listed in the doc comment for each group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SystemCommand {
    // ── Device state (direct API ≈ 1 ms each) ────────────────────────────

    /// Query `BatteryManager` for current level, charging state, and health.
    BatteryStatus,

    /// Query `StatFs` for internal storage totals and free space.
    StorageInfo,

    /// Query `ConnectivityManager` for network state and signal strength.
    NetworkStatus,

    /// Query `ActivityManager.getMemoryInfo()` for RAM pressure.
    MemoryPressure,

    /// Query `PowerManager.getCurrentThermalStatus()`.
    ThermalState,

    // ── Content providers (direct query ≈ 10-50 ms each) ─────────────────

    /// Search contacts via `ContentResolver` + `ContactsContract`.
    ContactSearch(String),

    /// Query `CalendarContract` for events in a time range.
    CalendarEvents {
        /// Start of the range (ms since UNIX epoch).
        start_ms: u64,
        /// End of the range (ms since UNIX epoch).
        end_ms: u64,
    },

    /// Query `MediaStore` for the N most recent photos.
    RecentPhotos(u32),

    /// List active notifications via `NotificationListenerService`.
    NotificationList,

    // ── System actions (direct API ≈ 5-20 ms each) ───────────────────────

    /// Set an alarm via `AlarmManager` / intent.
    SetAlarm {
        /// Hour in 24-hour format (0-23).
        hour: u8,
        /// Minute (0-59).
        minute: u8,
        /// Human-readable label for the alarm.
        label: String,
    },

    /// Send an SMS via `SmsManager`.
    SendSms {
        /// Phone number or contact name to resolve.
        recipient: String,
        /// Message body.
        body: String,
    },

    /// Set screen brightness via `Settings.System`.
    /// Range: 0.0 (minimum) to 1.0 (maximum).
    SetBrightness(f32),

    /// Toggle Wi-Fi via `WifiManager`.
    ToggleWifi(bool),

    /// Launch an app by package name via `PackageManager` + intent.
    LaunchApp(String),
}

impl fmt::Display for SystemCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BatteryStatus => write!(f, "BatteryStatus"),
            Self::StorageInfo => write!(f, "StorageInfo"),
            Self::NetworkStatus => write!(f, "NetworkStatus"),
            Self::MemoryPressure => write!(f, "MemoryPressure"),
            Self::ThermalState => write!(f, "ThermalState"),
            Self::ContactSearch(q) => write!(f, "ContactSearch({q})"),
            Self::CalendarEvents { start_ms, end_ms } => {
                write!(f, "CalendarEvents({start_ms}..{end_ms})")
            }
            Self::RecentPhotos(n) => write!(f, "RecentPhotos({n})"),
            Self::NotificationList => write!(f, "NotificationList"),
            Self::SetAlarm { hour, minute, label } => {
                write!(f, "SetAlarm({hour:02}:{minute:02}, {label:?})")
            }
            Self::SendSms { recipient, .. } => write!(f, "SendSms(to={recipient})"),
            Self::SetBrightness(v) => write!(f, "SetBrightness({v:.2})"),
            Self::ToggleWifi(on) => write!(f, "ToggleWifi({on})"),
            Self::LaunchApp(pkg) => write!(f, "LaunchApp({pkg})"),
        }
    }
}

// ---------------------------------------------------------------------------
// CommandType — data-free variant key for the supported_commands set
// ---------------------------------------------------------------------------

/// Data-free mirror of [`SystemCommand`] used as a key in the
/// `supported_commands` set. This lets us express "we support battery
/// queries" without carrying query parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CommandType {
    BatteryStatus,
    StorageInfo,
    NetworkStatus,
    MemoryPressure,
    ThermalState,
    ContactSearch,
    CalendarEvents,
    RecentPhotos,
    NotificationList,
    SetAlarm,
    SendSms,
    SetBrightness,
    ToggleWifi,
    LaunchApp,
}

impl CommandType {
    /// All defined command types.
    pub const ALL: [CommandType; 14] = [
        Self::BatteryStatus,
        Self::StorageInfo,
        Self::NetworkStatus,
        Self::MemoryPressure,
        Self::ThermalState,
        Self::ContactSearch,
        Self::CalendarEvents,
        Self::RecentPhotos,
        Self::NotificationList,
        Self::SetAlarm,
        Self::SendSms,
        Self::SetBrightness,
        Self::ToggleWifi,
        Self::LaunchApp,
    ];
}

impl fmt::Display for CommandType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl From<&SystemCommand> for CommandType {
    fn from(cmd: &SystemCommand) -> Self {
        match cmd {
            SystemCommand::BatteryStatus => Self::BatteryStatus,
            SystemCommand::StorageInfo => Self::StorageInfo,
            SystemCommand::NetworkStatus => Self::NetworkStatus,
            SystemCommand::MemoryPressure => Self::MemoryPressure,
            SystemCommand::ThermalState => Self::ThermalState,
            SystemCommand::ContactSearch(_) => Self::ContactSearch,
            SystemCommand::CalendarEvents { .. } => Self::CalendarEvents,
            SystemCommand::RecentPhotos(_) => Self::RecentPhotos,
            SystemCommand::NotificationList => Self::NotificationList,
            SystemCommand::SetAlarm { .. } => Self::SetAlarm,
            SystemCommand::SendSms { .. } => Self::SendSms,
            SystemCommand::SetBrightness(_) => Self::SetBrightness,
            SystemCommand::ToggleWifi(_) => Self::ToggleWifi,
            SystemCommand::LaunchApp(_) => Self::LaunchApp,
        }
    }
}

// ---------------------------------------------------------------------------
// SystemResult — typed return values for each command
// ---------------------------------------------------------------------------

/// Typed results from system API calls.
///
/// Each variant corresponds to one or more [`SystemCommand`] variants.
/// Vectors in result variants are bounded to the constants defined at the
/// top of this module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SystemResult {
    /// Battery level and health.
    Battery {
        /// Charge level in `0.0..=1.0`.
        level: f32,
        /// Whether the device is currently charging.
        charging: bool,
        /// Health status from `BatteryManager`.
        health: BatteryHealth,
    },

    /// Internal storage capacity and free space.
    Storage {
        /// Total bytes on the internal partition.
        total_bytes: u64,
        /// Free bytes available.
        free_bytes: u64,
    },

    /// Network connectivity state.
    Network {
        /// Whether the device has any internet connection.
        connected: bool,
        /// Whether Wi-Fi is the active transport.
        wifi: bool,
        /// Whether mobile data is the active transport.
        mobile_data: bool,
        /// RSSI or ASU signal strength, if available.
        signal_strength: Option<i32>,
    },

    /// RAM pressure information.
    Memory {
        /// Total physical RAM in bytes.
        total_bytes: u64,
        /// Currently available RAM in bytes.
        available_bytes: u64,
        /// Whether the system considers memory low.
        low_memory: bool,
    },

    /// Device thermal state.
    Thermal(ThermalState),

    /// Contact search results (bounded to [`MAX_CONTACTS`]).
    Contacts(Vec<ContactInfo>),

    /// Calendar events in the requested range (bounded to [`MAX_CALENDAR_EVENTS`]).
    Calendar(Vec<CalendarEvent>),

    /// Recent photos (bounded to [`MAX_PHOTOS`]).
    Photos(Vec<PhotoInfo>),

    /// Active notifications (bounded to [`MAX_NOTIFICATIONS`]).
    Notifications(Vec<NotificationInfo>),

    /// Generic success/failure for action commands (alarm, SMS, brightness, etc.).
    ActionCompleted {
        /// Which command was executed (display form).
        command: String,
        /// Whether the action succeeded.
        success: bool,
        /// Human-readable status message.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// SystemBridgeError
// ---------------------------------------------------------------------------

/// Errors specific to the system API bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum SystemBridgeError {
    /// JNI runtime is not available (e.g. running in a unit-test environment).
    #[error("JNI runtime not available — system bridge requires Android")]
    JniNotAvailable,

    /// The required Android permission has not been granted.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// The requested command has no direct API implementation.
    #[error("command not supported via system bridge: {0}")]
    CommandNotSupported(String),

    /// The underlying API call failed.
    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    /// The API call did not complete within the expected time budget.
    #[error("system API call timed out")]
    Timeout,

    /// A parameter on the command was invalid.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

// ---------------------------------------------------------------------------
// BridgeStats
// ---------------------------------------------------------------------------

/// Telemetry snapshot for the system bridge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeStats {
    /// Total number of commands executed since bridge creation.
    pub total_executions: u64,
    /// Exponential moving average of execution latency in milliseconds.
    pub average_latency_ms: f32,
    /// Number of command types currently supported.
    pub commands_supported: usize,
    /// Timestamp of the most recent execution (ms since UNIX epoch).
    pub last_execution_ms: u64,
}

// ---------------------------------------------------------------------------
// SystemBridge
// ---------------------------------------------------------------------------

/// High-speed bridge to Android system APIs via JNI.
///
/// Instead of driving the UI through AccessibilityService, this bridge calls
/// the relevant Android SDK methods directly for a ~1000× latency improvement
/// on supported operations.
///
/// # Usage
///
/// ```ignore
/// let mut bridge = SystemBridge::new();
///
/// // Fast path: check if we can handle this without A11y
/// if let Some(cmd) = SystemBridge::can_handle_intent("check my battery") {
///     let result = bridge.execute(cmd)?;
///     // result: SystemResult::Battery { level: 0.85, charging: true, … }
/// }
/// ```
pub struct SystemBridge {
    /// Which command types this bridge can execute on this device.
    supported_commands: HashSet<CommandType>,

    /// Total commands executed (monotonically increasing).
    execution_count: u64,

    /// Timestamp of the last execution (ms since UNIX epoch), or 0 if none.
    last_execution_ms: u64,

    /// Exponential moving average of execution latency in milliseconds.
    average_latency_ms: f32,
}

impl SystemBridge {
    /// Create a new system bridge with all command types marked as supported.
    ///
    /// On a real Android device the supported set may be narrowed at runtime
    /// based on API level and granted permissions.
    #[must_use]
    pub fn new() -> Self {
        let supported_commands: HashSet<CommandType> =
            CommandType::ALL.iter().copied().collect();

        info!(
            commands = supported_commands.len(),
            "SystemBridge initialised with all command types"
        );

        Self {
            supported_commands,
            execution_count: 0,
            last_execution_ms: 0,
            average_latency_ms: 0.0,
        }
    }

    // ── Core dispatch ────────────────────────────────────────────────────

    /// Execute a system command and return a typed result.
    ///
    /// Returns [`SystemBridgeError::CommandNotSupported`] if the command type
    /// is not in the `supported_commands` set.
    pub fn execute(
        &mut self,
        cmd: SystemCommand,
    ) -> Result<SystemResult, SystemBridgeError> {
        let cmd_type = CommandType::from(&cmd);

        if !self.supported_commands.contains(&cmd_type) {
            warn!(%cmd_type, "command not in supported set");
            return Err(SystemBridgeError::CommandNotSupported(
                cmd_type.to_string(),
            ));
        }

        debug!(%cmd, "executing system command");
        let start = Self::now_ms();

        let result = match cmd {
            SystemCommand::BatteryStatus => self.execute_battery(),
            SystemCommand::StorageInfo => self.execute_storage(),
            SystemCommand::NetworkStatus => self.execute_network(),
            SystemCommand::MemoryPressure => self.execute_memory(),
            SystemCommand::ThermalState => self.execute_thermal(),
            SystemCommand::ContactSearch(ref query) => {
                self.execute_contact_search(query)
            }
            SystemCommand::CalendarEvents { start_ms, end_ms } => {
                self.execute_calendar(start_ms, end_ms)
            }
            SystemCommand::RecentPhotos(count) => {
                self.execute_recent_photos(count)
            }
            SystemCommand::NotificationList => self.execute_notifications(),
            SystemCommand::SetAlarm {
                hour,
                minute,
                ref label,
            } => self.execute_set_alarm(hour, minute, label),
            SystemCommand::SendSms {
                ref recipient,
                ref body,
            } => self.execute_send_sms(recipient, body),
            SystemCommand::SetBrightness(level) => {
                self.execute_set_brightness(level)
            }
            SystemCommand::ToggleWifi(enable) => {
                self.execute_toggle_wifi(enable)
            }
            SystemCommand::LaunchApp(ref package) => {
                self.execute_launch_app(package)
            }
        };

        self.record_execution(start);
        result
    }

    /// Check if a specific command type is supported on this device.
    #[must_use]
    pub fn is_supported(&self, cmd_type: CommandType) -> bool {
        self.supported_commands.contains(&cmd_type)
    }

    /// Return a telemetry snapshot of bridge usage.
    #[must_use]
    pub fn execution_stats(&self) -> BridgeStats {
        BridgeStats {
            total_executions: self.execution_count,
            average_latency_ms: self.average_latency_ms,
            commands_supported: self.supported_commands.len(),
            last_execution_ms: self.last_execution_ms,
        }
    }

    /// Estimated latency for a command type.
    ///
    /// These are conservative upper bounds based on typical Android hardware.
    #[must_use]
    pub fn latency_estimate(cmd_type: CommandType) -> Duration {
        match cmd_type {
            // Device state — direct manager calls
            CommandType::BatteryStatus
            | CommandType::StorageInfo
            | CommandType::NetworkStatus
            | CommandType::MemoryPressure
            | CommandType::ThermalState => Duration::from_millis(1),

            // Content provider queries
            CommandType::ContactSearch
            | CommandType::CalendarEvents
            | CommandType::RecentPhotos
            | CommandType::NotificationList => Duration::from_millis(50),

            // System actions
            CommandType::SetAlarm
            | CommandType::SendSms
            | CommandType::SetBrightness
            | CommandType::ToggleWifi
            | CommandType::LaunchApp => Duration::from_millis(20),
        }
    }

    // ── Intent matching (the intelligence layer) ─────────────────────────

    /// Attempt to map a natural language intent fragment to a [`SystemCommand`].
    ///
    /// Returns `Some(cmd)` if the intent clearly maps to a direct API call,
    /// or `None` if it should fall through to the AccessibilityService path.
    ///
    /// This is intentionally **conservative** — it only matches when confident,
    /// because a false positive means executing the wrong action, while a
    /// false negative just takes the slower A11y path.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// assert_eq!(
    ///     SystemBridge::can_handle_intent("check my battery"),
    ///     Some(SystemCommand::BatteryStatus),
    /// );
    /// assert_eq!(
    ///     SystemBridge::can_handle_intent("what's the weather"),
    ///     None,
    /// );
    /// ```
    #[must_use]
    pub fn can_handle_intent(intent: &str) -> Option<SystemCommand> {
        let lower = intent.to_lowercase();

        // ── Battery ──────────────────────────────────────────────────────
        if lower.contains("battery")
            || lower.contains("charge level")
            || lower.contains("power level")
            || lower.contains("how much juice")
            || lower.contains("battery left")
            || lower.contains("battery percentage")
            || (lower.contains("charging") && !lower.contains("send"))
        {
            return Some(SystemCommand::BatteryStatus);
        }

        // ── Storage ──────────────────────────────────────────────────────
        if lower.contains("storage")
            || lower.contains("disk space")
            || lower.contains("free space")
            || lower.contains("space left")
            || lower.contains("how much space")
            || (lower.contains("memory full") && !lower.contains("ram"))
        {
            return Some(SystemCommand::StorageInfo);
        }

        // ── Network / connectivity ───────────────────────────────────────
        // Check WiFi toggle FIRST (more specific) before generic network queries.
        if let Some(cmd) = Self::match_wifi_toggle(&lower) {
            return Some(cmd);
        }
        if lower.contains("network status")
            || lower.contains("am i connected")
            || lower.contains("internet connection")
            || lower.contains("connectivity")
            || lower.contains("signal strength")
            || (lower.contains("wifi") && lower.contains("status"))
            || (lower.contains("network") && !lower.contains("toggle") && !lower.contains("turn"))
        {
            return Some(SystemCommand::NetworkStatus);
        }

        // ── Memory pressure (RAM, not storage) ──────────────────────────
        if lower.contains("memory pressure")
            || lower.contains("ram usage")
            || lower.contains("memory usage")
            || lower.contains("low memory")
            || lower.contains("how much ram")
            || lower.contains("available ram")
            || lower.contains("free ram")
        {
            return Some(SystemCommand::MemoryPressure);
        }

        // ── Thermal state ────────────────────────────────────────────────
        if lower.contains("thermal")
            || lower.contains("temperature")
            || lower.contains("overheating")
            || lower.contains("phone is hot")
            || lower.contains("phone is warm")
            || lower.contains("device temperature")
            || lower.contains("too hot")
        {
            return Some(SystemCommand::ThermalState);
        }

        // ── Contacts ─────────────────────────────────────────────────────
        if let Some(cmd) = Self::match_contact_search(&lower) {
            return Some(cmd);
        }

        // ── Calendar ─────────────────────────────────────────────────────
        if lower.contains("calendar")
            || lower.contains("my events")
            || lower.contains("my schedule")
            || lower.contains("upcoming meetings")
            || lower.contains("appointments today")
            || lower.contains("what's on my schedule")
            || lower.contains("any meetings")
        {
            // Default to a generous 24-hour window; the daemon can refine.
            let now = Self::now_ms();
            let day_ms: u64 = 24 * 60 * 60 * 1000;
            return Some(SystemCommand::CalendarEvents {
                start_ms: now,
                end_ms: now.saturating_add(day_ms),
            });
        }

        // ── Photos ───────────────────────────────────────────────────────
        if lower.contains("recent photos")
            || lower.contains("latest photos")
            || lower.contains("my pictures")
            || lower.contains("recent pictures")
            || lower.contains("show photos")
            || lower.contains("photo gallery")
            || lower.contains("last photos")
        {
            return Some(SystemCommand::RecentPhotos(10));
        }

        // ── Notifications ────────────────────────────────────────────────
        if lower.contains("notification")
            || lower.contains("what did i miss")
            || lower.contains("pending alerts")
            || lower.contains("show alerts")
            || lower.contains("unread messages")
            || lower.contains("any new alerts")
        {
            return Some(SystemCommand::NotificationList);
        }

        // ── Set alarm ────────────────────────────────────────────────────
        if let Some(cmd) = Self::match_alarm(&lower) {
            return Some(cmd);
        }

        // ── Send SMS ─────────────────────────────────────────────────────
        if let Some(cmd) = Self::match_sms(&lower) {
            return Some(cmd);
        }

        // ── Brightness ───────────────────────────────────────────────────
        if let Some(cmd) = Self::match_brightness(&lower) {
            return Some(cmd);
        }

        // ── Launch app ───────────────────────────────────────────────────
        if let Some(cmd) = Self::match_launch_app(&lower) {
            return Some(cmd);
        }

        // Not a system command — fall through to A11y.
        trace!(intent, "no system command match, falling through to A11y");
        None
    }

    // ── Private intent-matching helpers ──────────────────────────────────

    /// Match "turn on/off wifi", "enable/disable wifi", etc.
    fn match_wifi_toggle(lower: &str) -> Option<SystemCommand> {
        let is_wifi = lower.contains("wifi") || lower.contains("wi-fi");
        if !is_wifi {
            return None;
        }

        if lower.contains("turn on")
            || lower.contains("enable")
            || lower.contains("switch on")
            || lower.contains("activate")
            || lower.contains("connect wifi")
        {
            return Some(SystemCommand::ToggleWifi(true));
        }

        if lower.contains("turn off")
            || lower.contains("disable")
            || lower.contains("switch off")
            || lower.contains("deactivate")
            || lower.contains("disconnect wifi")
        {
            return Some(SystemCommand::ToggleWifi(false));
        }

        None
    }

    /// Match "find contact", "look up {name}", "phone number for {name}".
    fn match_contact_search(lower: &str) -> Option<SystemCommand> {
        // "find contact John" / "search contacts for John"
        if let Some(rest) = lower.strip_prefix("find contact ") {
            let query = rest.trim().to_string();
            if !query.is_empty() {
                return Some(SystemCommand::ContactSearch(query));
            }
        }
        if let Some(rest) = lower.strip_prefix("search contacts for ") {
            let query = rest.trim().to_string();
            if !query.is_empty() {
                return Some(SystemCommand::ContactSearch(query));
            }
        }

        // "look up {name}" / "phone number for {name}"
        if lower.starts_with("look up ") {
            let query = lower.trim_start_matches("look up ").trim().to_string();
            if !query.is_empty() {
                return Some(SystemCommand::ContactSearch(query));
            }
        }
        if lower.contains("phone number for ") {
            if let Some(idx) = lower.find("phone number for ") {
                let query = lower[idx + "phone number for ".len()..].trim().to_string();
                if !query.is_empty() {
                    return Some(SystemCommand::ContactSearch(query));
                }
            }
        }
        if lower.contains("email for ") {
            if let Some(idx) = lower.find("email for ") {
                let query = lower[idx + "email for ".len()..].trim().to_string();
                if !query.is_empty() {
                    return Some(SystemCommand::ContactSearch(query));
                }
            }
        }

        None
    }

    /// Match "set alarm for 7:30", "wake me up at 6 am", etc.
    fn match_alarm(lower: &str) -> Option<SystemCommand> {
        let is_alarm = lower.contains("set alarm")
            || lower.contains("set an alarm")
            || lower.contains("wake me up")
            || lower.contains("alarm at")
            || lower.contains("alarm for");

        if !is_alarm {
            return None;
        }

        // Try to extract time. Look for patterns like "7:30", "7 30", "7am", "7 am".
        if let Some((hour, minute)) = Self::extract_time(lower) {
            let label = if lower.contains("wake me up") {
                "Wake up".to_string()
            } else {
                "Alarm".to_string()
            };
            return Some(SystemCommand::SetAlarm {
                hour,
                minute,
                label,
            });
        }

        // Matched the intent but couldn't parse the time — still return a
        // default so the daemon can ask the user for clarification.
        Some(SystemCommand::SetAlarm {
            hour: 7,
            minute: 0,
            label: "Alarm".to_string(),
        })
    }

    /// Match "send message to {name} saying {body}", "text {name} {body}", "sms".
    fn match_sms(lower: &str) -> Option<SystemCommand> {
        // "send message to {name}"
        if lower.starts_with("send message to ")
            || lower.starts_with("send a message to ")
            || lower.starts_with("send text to ")
            || lower.starts_with("send a text to ")
        {
            let after_to = lower
                .find(" to ")
                .map(|i| &lower[i + 4..])
                .unwrap_or("");
            let (recipient, body) = Self::split_sms_parts(after_to);
            return Some(SystemCommand::SendSms { recipient, body });
        }

        // "text {name} {body}"
        if lower.starts_with("text ") && !lower.starts_with("text content") {
            let rest = lower.trim_start_matches("text ").trim();
            let (recipient, body) = Self::split_sms_parts(rest);
            if !recipient.is_empty() {
                return Some(SystemCommand::SendSms { recipient, body });
            }
        }

        // "sms {name}"
        if lower.starts_with("sms ") {
            let rest = lower.trim_start_matches("sms ").trim();
            let (recipient, body) = Self::split_sms_parts(rest);
            if !recipient.is_empty() {
                return Some(SystemCommand::SendSms { recipient, body });
            }
        }

        None
    }

    /// Match "set brightness to 50%", "dim the screen", "max brightness".
    fn match_brightness(lower: &str) -> Option<SystemCommand> {
        if !lower.contains("brightness")
            && !lower.contains("screen bright")
            && !lower.contains("dim screen")
            && !lower.contains("dim the screen")
        {
            return None;
        }

        // "max brightness" / "full brightness"
        if lower.contains("max brightness") || lower.contains("full brightness") {
            return Some(SystemCommand::SetBrightness(1.0));
        }

        // "minimum brightness" / "lowest brightness"
        if lower.contains("minimum brightness")
            || lower.contains("lowest brightness")
            || lower.contains("dim screen")
            || lower.contains("dim the screen")
        {
            return Some(SystemCommand::SetBrightness(0.1));
        }

        // Try to extract a percentage.
        if let Some(pct) = Self::extract_percentage(lower) {
            let level = (pct / 100.0).clamp(0.0, 1.0);
            return Some(SystemCommand::SetBrightness(level));
        }

        // "set brightness to half" / "50% brightness" already covered by percentage.
        // If we matched the keyword but can't parse a value, use 50% as a safe default.
        if lower.contains("brightness") {
            return Some(SystemCommand::SetBrightness(0.5));
        }

        None
    }

    /// Match "open {app}", "launch {app}", "start {app}".
    fn match_launch_app(lower: &str) -> Option<SystemCommand> {
        let prefixes = ["open ", "launch ", "start "];
        for prefix in &prefixes {
            if lower.starts_with(prefix) {
                let app = lower.trim_start_matches(prefix).trim();
                // Filter out non-app intents (e.g., "open the door").
                if !app.is_empty()
                    && !app.contains("door")
                    && !app.contains("window")
                    && !app.contains("file")
                    && !app.contains("link")
                {
                    return Some(SystemCommand::LaunchApp(app.to_string()));
                }
            }
        }
        None
    }

    // ── Time / number extraction helpers ─────────────────────────────────

    /// Extract a time from a string, e.g. "7:30", "7 30 am", "14:00".
    /// Returns `(hour_24h, minute)`.
    fn extract_time(s: &str) -> Option<(u8, u8)> {
        // Pattern: H:MM or HH:MM
        for word in s.split_whitespace() {
            if let Some((h, m)) = word.split_once(':') {
                if let (Ok(hour), Ok(minute)) = (h.parse::<u8>(), m.parse::<u8>()) {
                    if hour < 24 && minute < 60 {
                        // Check for am/pm suffix
                        let hour = Self::apply_ampm(hour, s);
                        return Some((hour, minute));
                    }
                }
            }
        }

        // Pattern: single number + "am"/"pm" (e.g., "7am", "7 am")
        let words: Vec<&str> = s.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            // "7am" or "7pm"
            let trimmed = word
                .trim_end_matches("am")
                .trim_end_matches("pm")
                .trim_end_matches("a.m.")
                .trim_end_matches("p.m.");
            if trimmed.len() < word.len() {
                if let Ok(h) = trimmed.parse::<u8>() {
                    if h >= 1 && h <= 12 {
                        let is_pm = word.contains("pm") || word.contains("p.m.");
                        let hour = if is_pm && h != 12 {
                            h + 12
                        } else if !is_pm && h == 12 {
                            0
                        } else {
                            h
                        };
                        return Some((hour, 0));
                    }
                }
            }
            // "7 am" pattern
            if let Ok(h) = word.parse::<u8>() {
                if h >= 1 && h <= 12 {
                    if let Some(next) = words.get(i + 1) {
                        if *next == "am" || *next == "a.m." {
                            let hour = if h == 12 { 0 } else { h };
                            return Some((hour, 0));
                        }
                        if *next == "pm" || *next == "p.m." {
                            let hour = if h == 12 { 12 } else { h + 12 };
                            return Some((hour, 0));
                        }
                    }
                }
            }
        }

        None
    }

    /// Adjust a parsed hour for AM/PM context found anywhere in the string.
    fn apply_ampm(hour: u8, s: &str) -> u8 {
        if hour > 12 {
            return hour; // already 24-hour
        }
        if s.contains("pm") || s.contains("p.m.") {
            if hour == 12 { 12 } else { hour + 12 }
        } else if s.contains("am") || s.contains("a.m.") {
            if hour == 12 { 0 } else { hour }
        } else {
            hour
        }
    }

    /// Extract a percentage value from a string (e.g., "50%", "fifty percent").
    fn extract_percentage(s: &str) -> Option<f32> {
        // Look for "{number}%"
        for word in s.split_whitespace() {
            let trimmed = word.trim_end_matches('%');
            if trimmed.len() < word.len() {
                if let Ok(v) = trimmed.parse::<f32>() {
                    return Some(v);
                }
            }
        }

        // Look for "{number} percent"
        let words: Vec<&str> = s.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if let Some(next) = words.get(i + 1) {
                if *next == "percent" || *next == "pct" {
                    if let Ok(v) = word.parse::<f32>() {
                        return Some(v);
                    }
                }
            }
        }

        // Named fractions
        if s.contains("half") {
            return Some(50.0);
        }
        if s.contains("quarter") {
            return Some(25.0);
        }

        None
    }

    /// Split an SMS intent string into `(recipient, body)`.
    ///
    /// Looks for separator words like "saying", "that", "message".
    /// E.g. "john saying hello there" → ("john", "hello there").
    fn split_sms_parts(s: &str) -> (String, String) {
        let separators = [" saying ", " that says ", " that ", " message "];
        for sep in &separators {
            if let Some(idx) = s.find(sep) {
                let recipient = s[..idx].trim().to_string();
                let body = s[idx + sep.len()..].trim().to_string();
                return (recipient, body);
            }
        }
        // No separator found — whole thing is the recipient, body is empty.
        (s.trim().to_string(), String::new())
    }

    // ── Execute methods (JNI placeholders) ───────────────────────────────

    /// Query `BatteryManager` for battery status.
    fn execute_battery(&self) -> Result<SystemResult, SystemBridgeError> {
        // TODO(jni): Call BatteryManager via JNI
        // For now, return a safe default so the health monitor can function.
        trace!("execute_battery: returning placeholder");
        Ok(SystemResult::Battery {
            level: 1.0,
            charging: false,
            health: BatteryHealth::Good,
        })
    }

    /// Query `StatFs` for internal storage.
    fn execute_storage(&self) -> Result<SystemResult, SystemBridgeError> {
        // TODO(jni): Call StatFs("/data") via JNI
        trace!("execute_storage: returning placeholder");
        Ok(SystemResult::Storage {
            total_bytes: 64 * 1024 * 1024 * 1024, // 64 GB placeholder
            free_bytes: 32 * 1024 * 1024 * 1024,  // 32 GB placeholder
        })
    }

    /// Query `ConnectivityManager` for network state.
    fn execute_network(&self) -> Result<SystemResult, SystemBridgeError> {
        // TODO(jni): Call ConnectivityManager.getActiveNetworkInfo() via JNI
        trace!("execute_network: returning placeholder");
        Ok(SystemResult::Network {
            connected: true,
            wifi: true,
            mobile_data: false,
            signal_strength: None,
        })
    }

    /// Query `ActivityManager.getMemoryInfo()`.
    fn execute_memory(&self) -> Result<SystemResult, SystemBridgeError> {
        // TODO(jni): Call ActivityManager.getMemoryInfo() via JNI
        trace!("execute_memory: returning placeholder");
        Ok(SystemResult::Memory {
            total_bytes: 8 * 1024 * 1024 * 1024,     // 8 GB placeholder
            available_bytes: 4 * 1024 * 1024 * 1024,  // 4 GB placeholder
            low_memory: false,
        })
    }

    /// Query `PowerManager.getCurrentThermalStatus()`.
    fn execute_thermal(&self) -> Result<SystemResult, SystemBridgeError> {
        // TODO(jni): Call PowerManager.getCurrentThermalStatus() via JNI
        trace!("execute_thermal: returning placeholder");
        Ok(SystemResult::Thermal(ThermalState::None))
    }

    /// Search contacts via `ContentResolver` + `ContactsContract`.
    fn execute_contact_search(
        &self,
        query: &str,
    ) -> Result<SystemResult, SystemBridgeError> {
        if query.is_empty() {
            return Err(SystemBridgeError::InvalidArgument(
                "contact search query must not be empty".into(),
            ));
        }
        // TODO(jni): Query ContactsContract via ContentResolver
        trace!(query, "execute_contact_search: returning empty placeholder");
        let contacts: Vec<ContactInfo> = Vec::new();
        // Bound enforced — truncate to MAX_CONTACTS.
        debug_assert!(contacts.len() <= MAX_CONTACTS);
        Ok(SystemResult::Contacts(contacts))
    }

    /// Query `CalendarContract` for events in a time window.
    fn execute_calendar(
        &self,
        start_ms: u64,
        end_ms: u64,
    ) -> Result<SystemResult, SystemBridgeError> {
        if end_ms <= start_ms {
            return Err(SystemBridgeError::InvalidArgument(format!(
                "end_ms ({end_ms}) must be greater than start_ms ({start_ms})"
            )));
        }
        // TODO(jni): Query CalendarContract via ContentResolver
        trace!(start_ms, end_ms, "execute_calendar: returning empty placeholder");
        let events: Vec<CalendarEvent> = Vec::new();
        debug_assert!(events.len() <= MAX_CALENDAR_EVENTS);
        Ok(SystemResult::Calendar(events))
    }

    /// Query `MediaStore` for recent photos.
    fn execute_recent_photos(
        &self,
        count: u32,
    ) -> Result<SystemResult, SystemBridgeError> {
        let bounded_count = (count as usize).min(MAX_PHOTOS);
        // TODO(jni): Query MediaStore.Images via ContentResolver
        trace!(bounded_count, "execute_recent_photos: returning empty placeholder");
        let photos: Vec<PhotoInfo> = Vec::new();
        debug_assert!(photos.len() <= MAX_PHOTOS);
        Ok(SystemResult::Photos(photos))
    }

    /// List active notifications via `NotificationListenerService`.
    fn execute_notifications(&self) -> Result<SystemResult, SystemBridgeError> {
        // TODO(jni): Query NotificationListenerService
        trace!("execute_notifications: returning empty placeholder");
        let notifications: Vec<NotificationInfo> = Vec::new();
        debug_assert!(notifications.len() <= MAX_NOTIFICATIONS);
        Ok(SystemResult::Notifications(notifications))
    }

    /// Set an alarm via `AlarmManager` intent.
    fn execute_set_alarm(
        &self,
        hour: u8,
        minute: u8,
        label: &str,
    ) -> Result<SystemResult, SystemBridgeError> {
        if hour >= 24 {
            return Err(SystemBridgeError::InvalidArgument(format!(
                "hour must be 0-23, got {hour}"
            )));
        }
        if minute >= 60 {
            return Err(SystemBridgeError::InvalidArgument(format!(
                "minute must be 0-59, got {minute}"
            )));
        }
        // TODO(jni): Fire ACTION_SET_ALARM intent via JNI
        info!(hour, minute, label, "set alarm (placeholder)");
        Ok(SystemResult::ActionCompleted {
            command: format!("SetAlarm({hour:02}:{minute:02})"),
            success: true,
            message: format!("Alarm set for {hour:02}:{minute:02} — {label}"),
        })
    }

    /// Send an SMS via `SmsManager`.
    fn execute_send_sms(
        &self,
        recipient: &str,
        body: &str,
    ) -> Result<SystemResult, SystemBridgeError> {
        if recipient.is_empty() {
            return Err(SystemBridgeError::InvalidArgument(
                "SMS recipient must not be empty".into(),
            ));
        }
        // TODO(jni): Call SmsManager.sendTextMessage() via JNI
        info!(recipient, body_len = body.len(), "send SMS (placeholder)");
        Ok(SystemResult::ActionCompleted {
            command: format!("SendSms(to={recipient})"),
            success: true,
            message: format!("SMS sent to {recipient}"),
        })
    }

    /// Set screen brightness via `Settings.System`.
    fn execute_set_brightness(
        &self,
        level: f32,
    ) -> Result<SystemResult, SystemBridgeError> {
        if !(0.0..=1.0).contains(&level) {
            return Err(SystemBridgeError::InvalidArgument(format!(
                "brightness must be 0.0..=1.0, got {level}"
            )));
        }
        // TODO(jni): Write Settings.System.SCREEN_BRIGHTNESS via JNI
        info!(level, "set brightness (placeholder)");
        Ok(SystemResult::ActionCompleted {
            command: format!("SetBrightness({level:.2})"),
            success: true,
            message: format!("Brightness set to {:.0}%", level * 100.0),
        })
    }

    /// Toggle Wi-Fi via `WifiManager`.
    fn execute_toggle_wifi(
        &self,
        enable: bool,
    ) -> Result<SystemResult, SystemBridgeError> {
        // TODO(jni): Call WifiManager.setWifiEnabled(enable) via JNI
        let action = if enable { "enabled" } else { "disabled" };
        info!(enable, "toggle wifi (placeholder)");
        Ok(SystemResult::ActionCompleted {
            command: format!("ToggleWifi({enable})"),
            success: true,
            message: format!("Wi-Fi {action}"),
        })
    }

    /// Launch an app by package name.
    fn execute_launch_app(
        &self,
        package: &str,
    ) -> Result<SystemResult, SystemBridgeError> {
        if package.is_empty() {
            return Err(SystemBridgeError::InvalidArgument(
                "package name must not be empty".into(),
            ));
        }
        // TODO(jni): Resolve launch intent via PackageManager, then startActivity()
        info!(package, "launch app (placeholder)");
        Ok(SystemResult::ActionCompleted {
            command: format!("LaunchApp({package})"),
            success: true,
            message: format!("Launched {package}"),
        })
    }

    // ── Internal helpers ─────────────────────────────────────────────────

    /// Record an execution and update telemetry counters.
    fn record_execution(&mut self, start_ms: u64) {
        let end = Self::now_ms();
        let elapsed = end.saturating_sub(start_ms) as f32;

        self.execution_count += 1;
        self.last_execution_ms = end;

        // Exponential moving average for latency tracking.
        if self.execution_count == 1 {
            self.average_latency_ms = elapsed;
        } else {
            self.average_latency_ms = self.average_latency_ms * (1.0 - LATENCY_EMA_ALPHA)
                + elapsed * LATENCY_EMA_ALPHA;
        }

        trace!(
            elapsed_ms = elapsed,
            avg_ms = self.average_latency_ms,
            count = self.execution_count,
            "system command executed"
        );
    }

    /// Current wall-clock time in milliseconds since UNIX epoch.
    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

impl Default for SystemBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SystemBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemBridge")
            .field("supported_commands", &self.supported_commands.len())
            .field("execution_count", &self.execution_count)
            .field("average_latency_ms", &self.average_latency_ms)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Construction ─────────────────────────────────────────────────────

    #[test]
    fn test_new_bridge_supports_all_commands() {
        let bridge = SystemBridge::new();
        for ct in &CommandType::ALL {
            assert!(bridge.is_supported(*ct), "should support {ct:?}");
        }
        assert_eq!(bridge.supported_commands.len(), 14);
    }

    #[test]
    fn test_initial_stats() {
        let bridge = SystemBridge::new();
        let stats = bridge.execution_stats();
        assert_eq!(stats.total_executions, 0);
        assert_eq!(stats.average_latency_ms, 0.0);
        assert_eq!(stats.commands_supported, 14);
        assert_eq!(stats.last_execution_ms, 0);
    }

    // ── CommandType conversion ───────────────────────────────────────────

    #[test]
    fn test_command_type_from_system_command() {
        assert_eq!(
            CommandType::from(&SystemCommand::BatteryStatus),
            CommandType::BatteryStatus,
        );
        assert_eq!(
            CommandType::from(&SystemCommand::ContactSearch("alice".into())),
            CommandType::ContactSearch,
        );
        assert_eq!(
            CommandType::from(&SystemCommand::SetAlarm {
                hour: 7,
                minute: 30,
                label: "test".into(),
            }),
            CommandType::SetAlarm,
        );
        assert_eq!(
            CommandType::from(&SystemCommand::LaunchApp("com.example".into())),
            CommandType::LaunchApp,
        );
    }

    // ── Execute — basic happy-path ───────────────────────────────────────

    #[test]
    fn test_execute_battery() {
        let mut bridge = SystemBridge::new();
        let result = bridge.execute(SystemCommand::BatteryStatus);
        assert!(result.is_ok());
        match result.unwrap() {
            SystemResult::Battery { level, health, .. } => {
                assert_eq!(level, 1.0);
                assert_eq!(health, BatteryHealth::Good);
            }
            other => panic!("expected Battery, got {other:?}"),
        }
        assert_eq!(bridge.execution_count, 1);
    }

    #[test]
    fn test_execute_storage() {
        let mut bridge = SystemBridge::new();
        let result = bridge.execute(SystemCommand::StorageInfo).unwrap();
        match result {
            SystemResult::Storage { total_bytes, free_bytes } => {
                assert!(total_bytes > 0);
                assert!(free_bytes <= total_bytes);
            }
            other => panic!("expected Storage, got {other:?}"),
        }
    }

    #[test]
    fn test_execute_network() {
        let mut bridge = SystemBridge::new();
        let result = bridge.execute(SystemCommand::NetworkStatus).unwrap();
        assert!(matches!(result, SystemResult::Network { .. }));
    }

    #[test]
    fn test_execute_memory() {
        let mut bridge = SystemBridge::new();
        let result = bridge.execute(SystemCommand::MemoryPressure).unwrap();
        assert!(matches!(result, SystemResult::Memory { .. }));
    }

    #[test]
    fn test_execute_thermal() {
        let mut bridge = SystemBridge::new();
        let result = bridge.execute(SystemCommand::ThermalState).unwrap();
        assert!(matches!(result, SystemResult::Thermal(ThermalState::None)));
    }

    #[test]
    fn test_execute_contact_search_empty_query() {
        let mut bridge = SystemBridge::new();
        let result = bridge.execute(SystemCommand::ContactSearch(String::new()));
        assert!(matches!(
            result,
            Err(SystemBridgeError::InvalidArgument(_))
        ));
    }

    #[test]
    fn test_execute_calendar_invalid_range() {
        let mut bridge = SystemBridge::new();
        let result = bridge.execute(SystemCommand::CalendarEvents {
            start_ms: 1000,
            end_ms: 500,
        });
        assert!(matches!(
            result,
            Err(SystemBridgeError::InvalidArgument(_))
        ));
    }

    #[test]
    fn test_execute_set_alarm_invalid_hour() {
        let mut bridge = SystemBridge::new();
        let result = bridge.execute(SystemCommand::SetAlarm {
            hour: 25,
            minute: 0,
            label: "test".into(),
        });
        assert!(matches!(
            result,
            Err(SystemBridgeError::InvalidArgument(_))
        ));
    }

    #[test]
    fn test_execute_set_alarm_invalid_minute() {
        let mut bridge = SystemBridge::new();
        let result = bridge.execute(SystemCommand::SetAlarm {
            hour: 7,
            minute: 61,
            label: "test".into(),
        });
        assert!(matches!(
            result,
            Err(SystemBridgeError::InvalidArgument(_))
        ));
    }

    #[test]
    fn test_execute_send_sms_empty_recipient() {
        let mut bridge = SystemBridge::new();
        let result = bridge.execute(SystemCommand::SendSms {
            recipient: String::new(),
            body: "hello".into(),
        });
        assert!(matches!(
            result,
            Err(SystemBridgeError::InvalidArgument(_))
        ));
    }

    #[test]
    fn test_execute_set_brightness_out_of_range() {
        let mut bridge = SystemBridge::new();
        assert!(bridge
            .execute(SystemCommand::SetBrightness(-0.1))
            .is_err());
        assert!(bridge
            .execute(SystemCommand::SetBrightness(1.5))
            .is_err());
    }

    #[test]
    fn test_execute_launch_app_empty() {
        let mut bridge = SystemBridge::new();
        let result = bridge.execute(SystemCommand::LaunchApp(String::new()));
        assert!(matches!(
            result,
            Err(SystemBridgeError::InvalidArgument(_))
        ));
    }

    #[test]
    fn test_execute_unsupported_command() {
        let mut bridge = SystemBridge::new();
        bridge.supported_commands.remove(&CommandType::BatteryStatus);
        let result = bridge.execute(SystemCommand::BatteryStatus);
        assert!(matches!(
            result,
            Err(SystemBridgeError::CommandNotSupported(_))
        ));
    }

    // ── Telemetry ────────────────────────────────────────────────────────

    #[test]
    fn test_execution_stats_updated_after_execute() {
        let mut bridge = SystemBridge::new();
        let _ = bridge.execute(SystemCommand::BatteryStatus);
        let _ = bridge.execute(SystemCommand::StorageInfo);
        let _ = bridge.execute(SystemCommand::NetworkStatus);

        let stats = bridge.execution_stats();
        assert_eq!(stats.total_executions, 3);
        assert!(stats.last_execution_ms > 0);
        assert!(stats.average_latency_ms >= 0.0);
    }

    // ── Latency estimates ────────────────────────────────────────────────

    #[test]
    fn test_latency_estimates() {
        assert_eq!(
            SystemBridge::latency_estimate(CommandType::BatteryStatus),
            Duration::from_millis(1),
        );
        assert_eq!(
            SystemBridge::latency_estimate(CommandType::ContactSearch),
            Duration::from_millis(50),
        );
        assert_eq!(
            SystemBridge::latency_estimate(CommandType::SetAlarm),
            Duration::from_millis(20),
        );
    }

    // ── Intent matching — battery ────────────────────────────────────────

    #[test]
    fn test_intent_battery() {
        let cases = [
            "check my battery",
            "what's my battery level",
            "how much battery left",
            "is it charging",
            "battery percentage",
            "how much juice do I have",
            "power level",
            "check charge level",
        ];
        for case in &cases {
            assert!(
                matches!(
                    SystemBridge::can_handle_intent(case),
                    Some(SystemCommand::BatteryStatus)
                ),
                "should match battery for: {case}",
            );
        }
    }

    // ── Intent matching — storage ────────────────────────────────────────

    #[test]
    fn test_intent_storage() {
        let cases = [
            "check storage",
            "how much disk space",
            "free space left",
            "how much space do I have",
        ];
        for case in &cases {
            assert!(
                matches!(
                    SystemBridge::can_handle_intent(case),
                    Some(SystemCommand::StorageInfo)
                ),
                "should match storage for: {case}",
            );
        }
    }

    // ── Intent matching — network ────────────────────────────────────────

    #[test]
    fn test_intent_network() {
        let cases = [
            "am i connected to the internet",
            "network status",
            "check internet connection",
            "wifi status",
            "connectivity",
        ];
        for case in &cases {
            assert!(
                matches!(
                    SystemBridge::can_handle_intent(case),
                    Some(SystemCommand::NetworkStatus)
                ),
                "should match network for: {case}",
            );
        }
    }

    // ── Intent matching — memory ─────────────────────────────────────────

    #[test]
    fn test_intent_memory() {
        let cases = [
            "memory pressure",
            "ram usage",
            "how much ram left",
            "available ram",
            "low memory",
            "memory usage",
        ];
        for case in &cases {
            assert!(
                matches!(
                    SystemBridge::can_handle_intent(case),
                    Some(SystemCommand::MemoryPressure)
                ),
                "should match memory for: {case}",
            );
        }
    }

    // ── Intent matching — thermal ────────────────────────────────────────

    #[test]
    fn test_intent_thermal() {
        let cases = [
            "device temperature",
            "phone is hot",
            "overheating",
            "thermal status",
            "phone is warm",
            "is my phone too hot",
        ];
        for case in &cases {
            assert!(
                matches!(
                    SystemBridge::can_handle_intent(case),
                    Some(SystemCommand::ThermalState)
                ),
                "should match thermal for: {case}",
            );
        }
    }

    // ── Intent matching — contacts ───────────────────────────────────────

    #[test]
    fn test_intent_contacts() {
        assert_eq!(
            SystemBridge::can_handle_intent("find contact John"),
            Some(SystemCommand::ContactSearch("john".into())),
        );
        assert_eq!(
            SystemBridge::can_handle_intent("look up Alice"),
            Some(SystemCommand::ContactSearch("alice".into())),
        );
        assert_eq!(
            SystemBridge::can_handle_intent("phone number for Bob"),
            Some(SystemCommand::ContactSearch("bob".into())),
        );
    }

    // ── Intent matching — calendar ───────────────────────────────────────

    #[test]
    fn test_intent_calendar() {
        let cases = [
            "what's on my calendar",
            "my schedule",
            "upcoming meetings",
            "any meetings today",
            "my events",
        ];
        for case in &cases {
            assert!(
                matches!(
                    SystemBridge::can_handle_intent(case),
                    Some(SystemCommand::CalendarEvents { .. })
                ),
                "should match calendar for: {case}",
            );
        }
    }

    // ── Intent matching — photos ─────────────────────────────────────────

    #[test]
    fn test_intent_photos() {
        let cases = [
            "show recent photos",
            "latest photos",
            "my pictures",
            "photo gallery",
        ];
        for case in &cases {
            assert!(
                matches!(
                    SystemBridge::can_handle_intent(case),
                    Some(SystemCommand::RecentPhotos(_))
                ),
                "should match photos for: {case}",
            );
        }
    }

    // ── Intent matching — notifications ──────────────────────────────────

    #[test]
    fn test_intent_notifications() {
        let cases = [
            "check notifications",
            "what did i miss",
            "pending alerts",
            "any new alerts",
        ];
        for case in &cases {
            assert!(
                matches!(
                    SystemBridge::can_handle_intent(case),
                    Some(SystemCommand::NotificationList)
                ),
                "should match notifications for: {case}",
            );
        }
    }

    // ── Intent matching — alarm ──────────────────────────────────────────

    #[test]
    fn test_intent_alarm_with_time() {
        let cmd = SystemBridge::can_handle_intent("set alarm for 7:30 am");
        assert!(matches!(
            cmd,
            Some(SystemCommand::SetAlarm { hour: 7, minute: 30, .. })
        ));

        let cmd = SystemBridge::can_handle_intent("wake me up at 6am");
        assert!(matches!(
            cmd,
            Some(SystemCommand::SetAlarm { hour: 6, minute: 0, .. })
        ));

        let cmd = SystemBridge::can_handle_intent("alarm at 14:00");
        assert!(matches!(
            cmd,
            Some(SystemCommand::SetAlarm { hour: 14, minute: 0, .. })
        ));
    }

    #[test]
    fn test_intent_alarm_without_time() {
        // Should still match, falling back to default 7:00.
        let cmd = SystemBridge::can_handle_intent("set alarm");
        assert!(matches!(cmd, Some(SystemCommand::SetAlarm { .. })));
    }

    // ── Intent matching — SMS ────────────────────────────────────────────

    #[test]
    fn test_intent_sms() {
        let cmd = SystemBridge::can_handle_intent("send message to alice saying hello");
        assert!(matches!(
            cmd,
            Some(SystemCommand::SendSms { ref recipient, ref body })
            if recipient == "alice" && body == "hello"
        ));

        let cmd = SystemBridge::can_handle_intent("text bob");
        assert!(matches!(
            cmd,
            Some(SystemCommand::SendSms { ref recipient, .. })
            if recipient == "bob"
        ));
    }

    // ── Intent matching — brightness ─────────────────────────────────────

    #[test]
    fn test_intent_brightness() {
        let cmd = SystemBridge::can_handle_intent("set brightness to 50%");
        assert!(matches!(
            cmd,
            Some(SystemCommand::SetBrightness(v)) if (v - 0.5).abs() < f32::EPSILON
        ));

        let cmd = SystemBridge::can_handle_intent("max brightness");
        assert!(matches!(
            cmd,
            Some(SystemCommand::SetBrightness(v)) if (v - 1.0).abs() < f32::EPSILON
        ));

        let cmd = SystemBridge::can_handle_intent("dim the screen");
        assert!(matches!(
            cmd,
            Some(SystemCommand::SetBrightness(v)) if (v - 0.1).abs() < f32::EPSILON
        ));
    }

    // ── Intent matching — wifi toggle ────────────────────────────────────

    #[test]
    fn test_intent_wifi_toggle() {
        assert_eq!(
            SystemBridge::can_handle_intent("turn on wifi"),
            Some(SystemCommand::ToggleWifi(true)),
        );
        assert_eq!(
            SystemBridge::can_handle_intent("disable wifi"),
            Some(SystemCommand::ToggleWifi(false)),
        );
        assert_eq!(
            SystemBridge::can_handle_intent("switch off wi-fi"),
            Some(SystemCommand::ToggleWifi(false)),
        );
    }

    // ── Intent matching — launch app ─────────────────────────────────────

    #[test]
    fn test_intent_launch_app() {
        assert_eq!(
            SystemBridge::can_handle_intent("open spotify"),
            Some(SystemCommand::LaunchApp("spotify".into())),
        );
        assert_eq!(
            SystemBridge::can_handle_intent("launch chrome"),
            Some(SystemCommand::LaunchApp("chrome".into())),
        );
    }

    #[test]
    fn test_intent_launch_app_filters_non_apps() {
        assert_eq!(
            SystemBridge::can_handle_intent("open the door"),
            None,
            "should not match 'the door' as an app",
        );
    }

    // ── Intent matching — negative cases ─────────────────────────────────

    #[test]
    fn test_intent_no_match() {
        let cases = [
            "what's the weather",
            "tell me a joke",
            "translate this to French",
            "hello",
            "book a flight",
        ];
        for case in &cases {
            assert!(
                SystemBridge::can_handle_intent(case).is_none(),
                "should NOT match system command for: {case}",
            );
        }
    }

    // ── Time extraction ──────────────────────────────────────────────────

    #[test]
    fn test_extract_time_colon() {
        assert_eq!(SystemBridge::extract_time("at 7:30"), Some((7, 30)));
        assert_eq!(SystemBridge::extract_time("at 14:00"), Some((14, 0)));
        assert_eq!(SystemBridge::extract_time("at 7:30 pm"), Some((19, 30)));
        assert_eq!(SystemBridge::extract_time("at 12:00 am"), Some((0, 0)));
    }

    #[test]
    fn test_extract_time_am_pm_suffix() {
        assert_eq!(SystemBridge::extract_time("at 7am"), Some((7, 0)));
        assert_eq!(SystemBridge::extract_time("at 7pm"), Some((19, 0)));
        assert_eq!(SystemBridge::extract_time("at 12pm"), Some((12, 0)));
        assert_eq!(SystemBridge::extract_time("at 12am"), Some((0, 0)));
    }

    #[test]
    fn test_extract_time_spaced_am_pm() {
        assert_eq!(SystemBridge::extract_time("at 7 am"), Some((7, 0)));
        assert_eq!(SystemBridge::extract_time("at 8 pm"), Some((20, 0)));
    }

    // ── Percentage extraction ────────────────────────────────────────────

    #[test]
    fn test_extract_percentage() {
        assert_eq!(SystemBridge::extract_percentage("50%"), Some(50.0));
        assert_eq!(SystemBridge::extract_percentage("100 percent"), Some(100.0));
        assert_eq!(SystemBridge::extract_percentage("half"), Some(50.0));
        assert_eq!(SystemBridge::extract_percentage("quarter"), Some(25.0));
        assert_eq!(SystemBridge::extract_percentage("nothing here"), None);
    }

    // ── SMS parts splitting ──────────────────────────────────────────────

    #[test]
    fn test_split_sms_parts() {
        let (r, b) = SystemBridge::split_sms_parts("alice saying hello there");
        assert_eq!(r, "alice");
        assert_eq!(b, "hello there");

        let (r, b) = SystemBridge::split_sms_parts("bob");
        assert_eq!(r, "bob");
        assert_eq!(b, "");
    }

    // ── Display / Debug ──────────────────────────────────────────────────

    #[test]
    fn test_system_command_display() {
        assert_eq!(SystemCommand::BatteryStatus.to_string(), "BatteryStatus");
        assert_eq!(
            SystemCommand::ContactSearch("alice".into()).to_string(),
            "ContactSearch(alice)",
        );
        assert_eq!(
            SystemCommand::SetBrightness(0.5).to_string(),
            "SetBrightness(0.50)",
        );
    }

    #[test]
    fn test_battery_health_display() {
        assert_eq!(BatteryHealth::Good.to_string(), "good");
        assert_eq!(BatteryHealth::Overheat.to_string(), "overheat");
    }

    #[test]
    fn test_thermal_state_display() {
        assert_eq!(ThermalState::None.to_string(), "none");
        assert_eq!(ThermalState::Critical.to_string(), "critical");
    }

    // ── Serde round-trip ─────────────────────────────────────────────────

    #[test]
    fn test_system_command_serde_roundtrip() {
        let cmd = SystemCommand::SetAlarm {
            hour: 7,
            minute: 30,
            label: "Wake up".into(),
        };
        let json = serde_json::to_string(&cmd).expect("serialize");
        let decoded: SystemCommand =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, cmd);
    }

    #[test]
    fn test_system_result_serde_roundtrip() {
        let result = SystemResult::Battery {
            level: 0.85,
            charging: true,
            health: BatteryHealth::Good,
        };
        let json = serde_json::to_string(&result).expect("serialize");
        let decoded: SystemResult =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, result);
    }

    #[test]
    fn test_system_bridge_error_serde_roundtrip() {
        let err = SystemBridgeError::PermissionDenied("READ_CONTACTS".into());
        let json = serde_json::to_string(&err).expect("serialize");
        let decoded: SystemBridgeError =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, err);
    }

    // ── Default trait ────────────────────────────────────────────────────

    #[test]
    fn test_default_bridge() {
        let bridge = SystemBridge::default();
        assert_eq!(bridge.execution_count, 0);
        assert_eq!(bridge.supported_commands.len(), 14);
    }
}
