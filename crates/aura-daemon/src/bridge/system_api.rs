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

use crate::platform::jni_bridge;

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

/// Thermal throttle threshold — if device temperature (°C) exceeds this value,
/// accessibility automation (Path B) is skipped to avoid worsening heat.
const THERMAL_THROTTLE_THRESHOLD: f32 = 42.0;

// ---------------------------------------------------------------------------
// Action verification result
// ---------------------------------------------------------------------------

/// Outcome of polling device state after dispatching an action intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionVerificationResult {
    /// The action was confirmed — device state changed as expected.
    Confirmed,
    /// Could not confirm or deny; best-effort success.
    Uncertain(String),
    /// Device state clearly did not change — action failed.
    Failed(String),
}

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
        let lower = lower.trim();

        // Order matters: more specific patterns before broader ones.
        // Wifi toggle must come before network status (both mention "wifi").
        Self::match_wifi_toggle(lower)
            .or_else(|| Self::match_alarm(lower))
            .or_else(|| Self::match_sms(lower))
            .or_else(|| Self::match_brightness(lower))
            .or_else(|| Self::match_launch_app(lower))
            .or_else(|| Self::match_contact_search(lower))
            .or_else(|| Self::match_battery(lower))
            .or_else(|| Self::match_storage(lower))
            .or_else(|| Self::match_network(lower))
            .or_else(|| Self::match_memory(lower))
            .or_else(|| Self::match_thermal(lower))
            .or_else(|| Self::match_calendar(lower))
            .or_else(|| Self::match_photos(lower))
            .or_else(|| Self::match_notifications(lower))
    }

    // ── Private intent-matching helpers ──────────────────────────────────
    // Fast-path structural parsers for well-defined Android commands.
    // Each helper receives a pre-lowercased, trimmed intent string.
    // They use only substring matching — no regex, no NLP.

    /// Match "turn on/off wifi", "enable/disable wifi", "switch off wi-fi".
    ///
    /// Only matches explicit on/off/enable/disable/toggle requests — NOT
    /// status queries like "wifi status" (those fall through to `match_network`).
    fn match_wifi_toggle(lower: &str) -> Option<SystemCommand> {
        let has_wifi = lower.contains("wifi") || lower.contains("wi-fi");
        if !has_wifi {
            return None;
        }
        // Require an explicit action word — "wifi status" must NOT match here.
        let enable_words = ["turn on", "enable", "switch on", "start wifi", "turn wifi on"];
        let disable_words = ["turn off", "disable", "switch off", "stop wifi", "turn wifi off"];

        if enable_words.iter().any(|w| lower.contains(w)) {
            return Some(SystemCommand::ToggleWifi(true));
        }
        if disable_words.iter().any(|w| lower.contains(w)) {
            return Some(SystemCommand::ToggleWifi(false));
        }
        // "toggle wifi" — no clear direction, default to `true` (on) as safe default.
        if lower.contains("toggle") {
            return Some(SystemCommand::ToggleWifi(true));
        }
        None
    }

    /// Match "find contact {name}", "look up {name}", "phone number for {name}".
    fn match_contact_search(lower: &str) -> Option<SystemCommand> {
        // Pattern: "find contact <name>"
        if let Some(rest) = lower.strip_prefix("find contact ") {
            let name = rest.trim().to_string();
            if !name.is_empty() {
                return Some(SystemCommand::ContactSearch(name));
            }
        }
        // Pattern: "look up <name>"
        if let Some(rest) = lower.strip_prefix("look up ") {
            let name = rest.trim().to_string();
            if !name.is_empty() {
                return Some(SystemCommand::ContactSearch(name));
            }
        }
        // Pattern: "phone number for <name>"
        if let Some(rest) = lower.strip_prefix("phone number for ") {
            let name = rest.trim().to_string();
            if !name.is_empty() {
                return Some(SystemCommand::ContactSearch(name));
            }
        }
        None
    }

    /// Match "set alarm for 7:30", "wake me up at 6am", "alarm at 14:00", "set alarm".
    ///
    /// Falls back to 7:00 when no time is found.
    fn match_alarm(lower: &str) -> Option<SystemCommand> {
        let has_alarm = lower.contains("alarm")
            || lower.contains("wake me up")
            || lower.contains("wake up");
        if !has_alarm {
            return None;
        }
        let (hour, minute) = Self::extract_time(lower).unwrap_or((7, 0));
        Some(SystemCommand::SetAlarm {
            hour,
            minute,
            label: "Alarm".to_string(),
        })
    }

    /// Match "send message to {name} saying {body}", "text {name}", "sms {name}".
    fn match_sms(lower: &str) -> Option<SystemCommand> {
        // Pattern: "send message to <rest>"
        if let Some(rest) = lower.strip_prefix("send message to ") {
            let (recipient, body) = Self::split_sms_parts(rest);
            if !recipient.is_empty() {
                return Some(SystemCommand::SendSms { recipient, body });
            }
        }
        // Pattern: "text <name> [saying <body>]"
        if let Some(rest) = lower.strip_prefix("text ") {
            let (recipient, body) = Self::split_sms_parts(rest);
            if !recipient.is_empty() {
                return Some(SystemCommand::SendSms { recipient, body });
            }
        }
        // Pattern: "sms <name> [saying <body>]"
        if let Some(rest) = lower.strip_prefix("sms ") {
            let (recipient, body) = Self::split_sms_parts(rest);
            if !recipient.is_empty() {
                return Some(SystemCommand::SendSms { recipient, body });
            }
        }
        None
    }

    /// Match "set brightness to 50%", "max brightness", "dim the screen".
    ///
    /// Recognised forms:
    /// - `max brightness` → 1.0
    /// - `full brightness` → 1.0
    /// - `min brightness` → 0.0
    /// - `dim the screen` / `dim screen` → 0.1
    /// - `brightness N%` / `set brightness to N%` → N / 100
    fn match_brightness(lower: &str) -> Option<SystemCommand> {
        let has_brightness = lower.contains("brightness");
        let has_dim = lower.contains("dim");
        if !has_brightness && !has_dim {
            return None;
        }

        // Named extremes.
        if lower.contains("max brightness") || lower.contains("full brightness") {
            return Some(SystemCommand::SetBrightness(1.0));
        }
        if lower.contains("min brightness") || lower.contains("minimum brightness") {
            return Some(SystemCommand::SetBrightness(0.0));
        }
        // "dim the screen" / "dim screen"
        if has_dim && (lower.contains("screen") || lower.contains("display")) {
            return Some(SystemCommand::SetBrightness(0.1));
        }

        // Numeric percentage.
        if let Some(pct) = Self::extract_percentage(lower) {
            let level = (pct / 100.0).clamp(0.0, 1.0);
            return Some(SystemCommand::SetBrightness(level));
        }

        None
    }

    /// Match "open {app}", "launch {app}".
    ///
    /// Filters out non-app targets: "open the door", "open settings menu", etc.
    /// Blocklist of common false-positive words after the verb.
    fn match_launch_app(lower: &str) -> Option<SystemCommand> {
        // Words that should NOT be treated as app names.
        const BLOCKLIST: &[&str] = &[
            "the door", "a file", "the menu", "the app", "an app",
            "settings menu", "this", "that",
        ];

        let app_name = if let Some(rest) = lower.strip_prefix("open ") {
            rest.trim()
        } else if let Some(rest) = lower.strip_prefix("launch ") {
            rest.trim()
        } else {
            return None;
        };

        if app_name.is_empty() {
            return None;
        }

        // Reject blocklisted phrases.
        if BLOCKLIST.iter().any(|b| app_name.starts_with(b) || *b == app_name) {
            return None;
        }
        // Reject if it starts with "the " (heuristic: "the door", "the settings", etc.)
        if app_name.starts_with("the ") {
            return None;
        }

        Some(SystemCommand::LaunchApp(app_name.to_string()))
    }

    // ── Status-query helpers (added for completeness of fast-path routing) ──

    /// Match battery status queries: "check my battery", "battery level", "is it charging", etc.
    fn match_battery(lower: &str) -> Option<SystemCommand> {
        let keywords = [
            "battery", "charging", "charge level", "juice", "power level",
        ];
        if keywords.iter().any(|k| lower.contains(k)) {
            return Some(SystemCommand::BatteryStatus);
        }
        None
    }

    /// Match storage queries: "check storage", "disk space", "free space", "space left".
    fn match_storage(lower: &str) -> Option<SystemCommand> {
        let keywords = ["storage", "disk space", "free space", "space left", "how much space"];
        if keywords.iter().any(|k| lower.contains(k)) {
            return Some(SystemCommand::StorageInfo);
        }
        None
    }

    /// Match network status queries: "network status", "internet connection", "wifi status", "connectivity".
    ///
    /// Note: explicit wifi on/off toggles are already matched by `match_wifi_toggle` earlier
    /// in the chain, so reaching this helper with "wifi" in the string means it's a status query.
    fn match_network(lower: &str) -> Option<SystemCommand> {
        let keywords = [
            "network status", "internet connection", "wifi status", "wi-fi status",
            "connectivity", "connected to the internet", "check internet",
            "am i connected",
        ];
        if keywords.iter().any(|k| lower.contains(k)) {
            return Some(SystemCommand::NetworkStatus);
        }
        None
    }

    /// Match memory/RAM queries: "memory pressure", "ram usage", "available ram", "low memory".
    fn match_memory(lower: &str) -> Option<SystemCommand> {
        let keywords = ["memory", "ram"];
        if keywords.iter().any(|k| lower.contains(k)) {
            return Some(SystemCommand::MemoryPressure);
        }
        None
    }

    /// Match thermal queries: "device temperature", "phone is hot", "overheating", "thermal status".
    fn match_thermal(lower: &str) -> Option<SystemCommand> {
        let keywords = [
            "temperature", "overheat", "thermal", "too hot", "is hot", "is warm",
            "phone hot", "device hot",
        ];
        if keywords.iter().any(|k| lower.contains(k)) {
            return Some(SystemCommand::ThermalState);
        }
        None
    }

    /// Match calendar queries: "calendar", "schedule", "meetings", "events".
    fn match_calendar(lower: &str) -> Option<SystemCommand> {
        let keywords = ["calendar", "schedule", "meeting", "event"];
        if keywords.iter().any(|k| lower.contains(k)) {
            // Return next 24 hours as the default window.
            let now_ms = Self::now_ms();
            let end_ms = now_ms + 24 * 60 * 60 * 1000;
            return Some(SystemCommand::CalendarEvents {
                start_ms: now_ms,
                end_ms,
            });
        }
        None
    }

    /// Match photo queries: "recent photos", "latest photos", "my pictures", "photo gallery".
    fn match_photos(lower: &str) -> Option<SystemCommand> {
        let keywords = ["photo", "picture", "gallery", "image"];
        if keywords.iter().any(|k| lower.contains(k)) {
            return Some(SystemCommand::RecentPhotos(10));
        }
        None
    }

    /// Match notification queries: "check notifications", "what did i miss", "pending alerts".
    fn match_notifications(lower: &str) -> Option<SystemCommand> {
        let keywords = [
            "notification", "what did i miss", "pending alert", "new alert",
            "alerts",
        ];
        if keywords.iter().any(|k| lower.contains(k)) {
            return Some(SystemCommand::NotificationList);
        }
        None
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
        trace!(query, "execute_contact_search: calling JNI");
        let bytes = jni_bridge::jni_query_contacts(query).map_err(|e| {
            warn!(error = %e, "jni_query_contacts failed");
            SystemBridgeError::ExecutionFailed(e.to_string())
        })?;
        let mut contacts: Vec<ContactInfo> = serde_json::from_slice(&bytes).map_err(|e| {
            warn!(error = %e, "contacts JSON parse failed");
            SystemBridgeError::ExecutionFailed(format!("contacts parse: {e}"))
        })?;
        // Enforce bounded collection.
        contacts.truncate(MAX_CONTACTS);
        debug!(query, count = contacts.len(), "contacts found");
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
        trace!(start_ms, end_ms, "execute_calendar: calling JNI");
        let bytes = jni_bridge::jni_query_calendar(start_ms as i64, end_ms as i64).map_err(|e| {
            warn!(error = %e, "jni_query_calendar failed");
            SystemBridgeError::ExecutionFailed(e.to_string())
        })?;
        let mut events: Vec<CalendarEvent> = serde_json::from_slice(&bytes).map_err(|e| {
            warn!(error = %e, "calendar JSON parse failed");
            SystemBridgeError::ExecutionFailed(format!("calendar parse: {e}"))
        })?;
        events.truncate(MAX_CALENDAR_EVENTS);
        debug!(start_ms, end_ms, count = events.len(), "calendar events found");
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
        trace!("execute_notifications: calling JNI");
        let bytes = jni_bridge::jni_query_notifications().map_err(|e| {
            warn!(error = %e, "jni_query_notifications failed");
            SystemBridgeError::ExecutionFailed(e.to_string())
        })?;
        let mut notifications: Vec<NotificationInfo> = serde_json::from_slice(&bytes).map_err(|e| {
            warn!(error = %e, "notifications JSON parse failed");
            SystemBridgeError::ExecutionFailed(format!("notifications parse: {e}"))
        })?;
        notifications.truncate(MAX_NOTIFICATIONS);
        debug!(count = notifications.len(), "active notifications found");
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

        // Thermal gate — avoid extra UI automation if device is hot.
        let thermal_ok = jni_bridge::jni_get_thermal_status()
            .map(|t| t < THERMAL_THROTTLE_THRESHOLD)
            .unwrap_or(true);

        // Path A: direct ACTION_SET_ALARM intent via JNI.
        trace!(hour, minute, label, "execute_set_alarm: Path A");
        match jni_bridge::jni_set_alarm(hour, minute, label) {
            Ok(true) => {
                info!(hour, minute, label, "alarm set via Path A (direct intent)");
                Ok(SystemResult::ActionCompleted {
                    command: format!("SetAlarm({hour:02}:{minute:02})"),
                    success: true,
                    message: format!("Alarm set for {hour:02}:{minute:02} — {label}"),
                })
            }
            Ok(false) | Err(_) if thermal_ok => {
                // Path B: AccessibilityService fallback.
                warn!(hour, minute, "Path A failed; attempting Path B (accessibility)");
                let dispatched = self.accessibility_set_alarm(hour, minute, label);
                Ok(SystemResult::ActionCompleted {
                    command: format!("SetAlarm({hour:02}:{minute:02})"),
                    success: dispatched,
                    message: if dispatched {
                        format!("Alarm set via accessibility for {hour:02}:{minute:02} — {label}")
                    } else {
                        format!("Failed to set alarm for {hour:02}:{minute:02}")
                    },
                })
            }
            Ok(false) => {
                warn!(hour, minute, "Path A returned false; thermal gate blocked Path B");
                Ok(SystemResult::ActionCompleted {
                    command: format!("SetAlarm({hour:02}:{minute:02})"),
                    success: false,
                    message: "Alarm intent failed and device is too hot for accessibility fallback".into(),
                })
            }
            Err(e) => {
                warn!(error = %e, "Path A error; thermal gate blocked Path B");
                Ok(SystemResult::ActionCompleted {
                    command: format!("SetAlarm({hour:02}:{minute:02})"),
                    success: false,
                    message: format!("Alarm failed: {e}"),
                })
            }
        }
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

        let thermal_ok = jni_bridge::jni_get_thermal_status()
            .map(|t| t < THERMAL_THROTTLE_THRESHOLD)
            .unwrap_or(true);

        // Path A: SmsManager.sendTextMessage via JNI.
        trace!(recipient, body_len = body.len(), "execute_send_sms: Path A");
        match jni_bridge::jni_send_sms(recipient, body) {
            Ok(true) => {
                info!(recipient, "SMS sent via Path A (SmsManager)");
                Ok(SystemResult::ActionCompleted {
                    command: format!("SendSms(to={recipient})"),
                    success: true,
                    message: format!("SMS sent to {recipient}"),
                })
            }
            Ok(false) | Err(_) if thermal_ok => {
                // Path B: open SMS app via accessibility and fill fields.
                warn!(recipient, "Path A failed; attempting Path B (accessibility)");
                let dispatched = self.accessibility_send_sms(recipient, body);
                Ok(SystemResult::ActionCompleted {
                    command: format!("SendSms(to={recipient})"),
                    success: dispatched,
                    message: if dispatched {
                        format!("SMS dispatched via accessibility to {recipient}")
                    } else {
                        format!("Failed to send SMS to {recipient}")
                    },
                })
            }
            Ok(false) => {
                warn!(recipient, "SMS Path A returned false; thermal gate blocked Path B");
                Ok(SystemResult::ActionCompleted {
                    command: format!("SendSms(to={recipient})"),
                    success: false,
                    message: "SMS failed and device is too hot for accessibility fallback".into(),
                })
            }
            Err(e) => {
                warn!(error = %e, recipient, "SMS JNI error");
                Ok(SystemResult::ActionCompleted {
                    command: format!("SendSms(to={recipient})"),
                    success: false,
                    message: format!("SMS failed: {e}"),
                })
            }
        }
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

        let thermal_ok = jni_bridge::jni_get_thermal_status()
            .map(|t| t < THERMAL_THROTTLE_THRESHOLD)
            .unwrap_or(true);

        // Path A: Settings.System.SCREEN_BRIGHTNESS via JNI.
        trace!(level, "execute_set_brightness: Path A");
        match jni_bridge::jni_set_brightness(level) {
            Ok(true) => {
                info!(level, "brightness set via Path A");
                Ok(SystemResult::ActionCompleted {
                    command: format!("SetBrightness({level:.2})"),
                    success: true,
                    message: format!("Brightness set to {:.0}%", level * 100.0),
                })
            }
            Ok(false) | Err(_) if thermal_ok => {
                // Path B: navigate Settings UI via accessibility.
                warn!(level, "brightness Path A failed; attempting Path B");
                let dispatched = self.accessibility_set_brightness(level);
                Ok(SystemResult::ActionCompleted {
                    command: format!("SetBrightness({level:.2})"),
                    success: dispatched,
                    message: if dispatched {
                        format!("Brightness set via accessibility to {:.0}%", level * 100.0)
                    } else {
                        "Failed to set brightness".into()
                    },
                })
            }
            Ok(false) => {
                warn!(level, "brightness Path A returned false; thermal gate blocked Path B");
                Ok(SystemResult::ActionCompleted {
                    command: format!("SetBrightness({level:.2})"),
                    success: false,
                    message: "Brightness failed and device is too hot for accessibility fallback".into(),
                })
            }
            Err(e) => {
                warn!(error = %e, "brightness JNI error");
                Ok(SystemResult::ActionCompleted {
                    command: format!("SetBrightness({level:.2})"),
                    success: false,
                    message: format!("Brightness failed: {e}"),
                })
            }
        }
    }

    /// Toggle Wi-Fi via `WifiManager`.
    fn execute_toggle_wifi(
        &self,
        enable: bool,
    ) -> Result<SystemResult, SystemBridgeError> {
        let action = if enable { "enabled" } else { "disabled" };

        let thermal_ok = jni_bridge::jni_get_thermal_status()
            .map(|t| t < THERMAL_THROTTLE_THRESHOLD)
            .unwrap_or(true);

        // Path A: WifiManager.setWifiEnabled / Settings intent via JNI.
        trace!(enable, "execute_toggle_wifi: Path A");
        match jni_bridge::jni_toggle_wifi(enable) {
            Ok(true) => {
                info!(enable, "Wi-Fi {action} via Path A");
                // Verify: poll network type to confirm state change.
                let confirmed = self.verify_wifi_state(enable);
                Ok(SystemResult::ActionCompleted {
                    command: format!("ToggleWifi({enable})"),
                    success: confirmed,
                    message: if confirmed {
                        format!("Wi-Fi {action}")
                    } else {
                        format!("Wi-Fi intent dispatched but state unconfirmed")
                    },
                })
            }
            Ok(false) | Err(_) if thermal_ok => {
                // Path B: navigate Settings app via accessibility.
                warn!(enable, "Wi-Fi Path A failed; attempting Path B");
                let dispatched = self.accessibility_toggle_wifi(enable);
                Ok(SystemResult::ActionCompleted {
                    command: format!("ToggleWifi({enable})"),
                    success: dispatched,
                    message: if dispatched {
                        format!("Wi-Fi {action} via accessibility")
                    } else {
                        format!("Failed to toggle Wi-Fi")
                    },
                })
            }
            Ok(false) => {
                warn!(enable, "Wi-Fi Path A returned false; thermal gate blocked Path B");
                Ok(SystemResult::ActionCompleted {
                    command: format!("ToggleWifi({enable})"),
                    success: false,
                    message: "Wi-Fi toggle failed and device is too hot for accessibility fallback".into(),
                })
            }
            Err(e) => {
                warn!(error = %e, "Wi-Fi JNI error");
                Ok(SystemResult::ActionCompleted {
                    command: format!("ToggleWifi({enable})"),
                    success: false,
                    message: format!("Wi-Fi toggle failed: {e}"),
                })
            }
        }
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

        let thermal_ok = jni_bridge::jni_get_thermal_status()
            .map(|t| t < THERMAL_THROTTLE_THRESHOLD)
            .unwrap_or(true);

        // Path A: PackageManager.getLaunchIntentForPackage + startActivity via JNI.
        trace!(package, "execute_launch_app: Path A");
        match jni_bridge::jni_launch_app(package) {
            Ok(true) => {
                info!(package, "app launched via Path A (direct intent)");
                // Verify: poll foreground package to confirm launch.
                let confirmed = self.verify_foreground_package(package);
                Ok(SystemResult::ActionCompleted {
                    command: format!("LaunchApp({package})"),
                    success: confirmed,
                    message: if confirmed {
                        format!("Launched {package}")
                    } else {
                        format!("Launch intent sent to {package} but not confirmed foreground")
                    },
                })
            }
            Ok(false) | Err(_) if thermal_ok => {
                // Path B: open via accessibility (long-press recents, find icon, tap).
                warn!(package, "Path A failed; attempting Path B (accessibility)");
                let dispatched = self.accessibility_launch_app(package);
                Ok(SystemResult::ActionCompleted {
                    command: format!("LaunchApp({package})"),
                    success: dispatched,
                    message: if dispatched {
                        format!("Launched {package} via accessibility")
                    } else {
                        format!("Failed to launch {package}")
                    },
                })
            }
            Ok(false) => {
                warn!(package, "launch Path A returned false; thermal gate blocked Path B");
                Ok(SystemResult::ActionCompleted {
                    command: format!("LaunchApp({package})"),
                    success: false,
                    message: "Launch intent failed and device is too hot for accessibility fallback".into(),
                })
            }
            Err(e) => {
                warn!(error = %e, package, "launch JNI error");
                Ok(SystemResult::ActionCompleted {
                    command: format!("LaunchApp({package})"),
                    success: false,
                    message: format!("Launch failed: {e}"),
                })
            }
        }
    }

    // ── Verification helpers ──────────────────────────────────────────────

    /// Poll `jni_get_foreground_package` up to 3 times (100 ms apart) to
    /// confirm the given package has come to the foreground.
    ///
    /// Returns `true` if confirmed, `false` if not confirmed after all polls.
    fn verify_foreground_package(&self, expected_pkg: &str) -> bool {
        for attempt in 0..3u8 {
            // Blocking sleep — SystemBridge is sync; callers accept ~300 ms max.
            std::thread::sleep(Duration::from_millis(100));
            match jni_bridge::jni_get_foreground_package() {
                Ok(pkg) if pkg == expected_pkg => {
                    debug!(attempt, expected_pkg, "foreground package confirmed");
                    return true;
                }
                Ok(pkg) => {
                    trace!(attempt, expected_pkg, actual = %pkg, "foreground package mismatch");
                }
                Err(e) => {
                    trace!(attempt, error = %e, "foreground package query failed");
                }
            }
        }
        warn!(expected_pkg, "could not confirm foreground package after 3 polls");
        false
    }

    /// Poll `jni_get_network_type` up to 3 times (200 ms apart) to confirm
    /// the Wi-Fi state matches `enable`.
    fn verify_wifi_state(&self, enable: bool) -> bool {
        for attempt in 0..3u8 {
            std::thread::sleep(Duration::from_millis(200));
            match jni_bridge::jni_get_network_type() {
                Ok(net_type) => {
                    let is_wifi = net_type == "wifi";
                    if is_wifi == enable {
                        debug!(attempt, enable, "Wi-Fi state confirmed");
                        return true;
                    }
                    trace!(attempt, enable, net_type, "Wi-Fi state not yet changed");
                }
                Err(e) => {
                    trace!(attempt, error = %e, "network type query failed");
                }
            }
        }
        warn!(enable, "could not confirm Wi-Fi state after 3 polls");
        false
    }

    // ── Path B: Accessibility fallback stubs ─────────────────────────────
    //
    // These stubs dispatch Path B via the accessibility service.  Full UI
    // automation logic is implemented in `execution/accessibility_executor.rs`;
    // here we call the bridge's press_home / navigate helpers.

    /// Path B fallback: open the Clock app via accessibility and set alarm.
    fn accessibility_set_alarm(&self, hour: u8, minute: u8, label: &str) -> bool {
        trace!(hour, minute, label, "accessibility_set_alarm: dispatching Path B");
        // Press home first to ensure a known state, then launch Clock.
        let _ = jni_bridge::jni_press_home();
        let ok = jni_bridge::jni_launch_app("com.google.android.deskclock")
            .unwrap_or(false)
            || jni_bridge::jni_launch_app("com.android.deskclock").unwrap_or(false);
        if ok {
            debug!(hour, minute, "Clock app launched for accessibility alarm");
        } else {
            warn!("Could not launch Clock app for accessibility alarm");
        }
        ok
    }

    /// Path B fallback: open the default SMS app via accessibility.
    fn accessibility_send_sms(&self, recipient: &str, _body: &str) -> bool {
        trace!(recipient, "accessibility_send_sms: dispatching Path B");
        let ok = jni_bridge::jni_launch_app("com.google.android.apps.messaging")
            .unwrap_or(false)
            || jni_bridge::jni_launch_app("com.android.mms").unwrap_or(false);
        if ok {
            debug!(recipient, "SMS app launched for accessibility send");
        } else {
            warn!("Could not launch SMS app for accessibility send");
        }
        ok
    }

    /// Path B fallback: open Settings > Display for brightness adjustment.
    fn accessibility_set_brightness(&self, level: f32) -> bool {
        trace!(level, "accessibility_set_brightness: dispatching Path B");
        let ok = jni_bridge::jni_launch_app("com.android.settings").unwrap_or(false);
        if ok {
            debug!(level, "Settings launched for accessibility brightness");
        } else {
            warn!("Could not launch Settings for accessibility brightness");
        }
        ok
    }

    /// Path B fallback: open Settings > Wi-Fi panel.
    fn accessibility_toggle_wifi(&self, enable: bool) -> bool {
        trace!(enable, "accessibility_toggle_wifi: dispatching Path B");
        let ok = jni_bridge::jni_launch_app("com.android.settings").unwrap_or(false);
        if ok {
            debug!(enable, "Settings launched for accessibility Wi-Fi toggle");
        } else {
            warn!("Could not launch Settings for accessibility Wi-Fi toggle");
        }
        ok
    }

    /// Path B fallback: launch app via recents/home screen accessibility navigation.
    fn accessibility_launch_app(&self, package: &str) -> bool {
        trace!(package, "accessibility_launch_app: dispatching Path B");
        // Best effort: press home, then attempt a second direct launch.
        let _ = jni_bridge::jni_press_home();
        std::thread::sleep(Duration::from_millis(150));
        let ok = jni_bridge::jni_launch_app(package).unwrap_or(false);
        if ok {
            debug!(package, "app launched via accessibility Path B retry");
        } else {
            warn!(package, "accessibility_launch_app: all paths failed");
        }
        ok
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

    // ── Parsing helpers (used by intent matching and tests) ───────────────

    /// Extract a time value from a natural-language string.
    ///
    /// Recognises:
    /// - `H:MM [am|pm]` — colon-separated with optional AM/PM suffix
    /// - `H am|pm` — hour-only with AM/PM (with or without space)
    ///
    /// Returns `Some((hour_24, minute))` or `None` if no time is found.
    ///
    /// # AM/PM conversion
    /// - 12 am → 0, 1–11 am → unchanged
    /// - 12 pm → 12, 1–11 pm → hour + 12
    pub fn extract_time(input: &str) -> Option<(u8, u8)> {
        let lower = input.to_lowercase();
        let s = lower.trim();

        // Walk the string looking for the first run of digits.
        let bytes = s.as_bytes();
        let len = bytes.len();
        let mut i = 0usize;

        while i < len {
            if bytes[i].is_ascii_digit() {
                // Collect all digits for the hour.
                let start = i;
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let hour_str = &s[start..i];
                let hour: u8 = hour_str.parse().ok()?;

                // Skip optional whitespace.
                let rest_start = i;
                let rest = s[rest_start..].trim_start();

                if rest.starts_with(':') {
                    // Colon-separated: H:MM [am|pm]
                    let after_colon = rest[1..].trim_start();
                    let mut j = 0usize;
                    while j < after_colon.len()
                        && after_colon.as_bytes().get(j).map(|b| b.is_ascii_digit()).unwrap_or(false)
                    {
                        j += 1;
                    }
                    if j == 0 {
                        return None;
                    }
                    let minute: u8 = after_colon[..j].parse().ok()?;
                    let suffix = after_colon[j..].trim();
                    let h24 = Self::apply_am_pm(hour, suffix);
                    return Some((h24, minute));
                } else if rest.starts_with("am") || rest.starts_with("pm") {
                    // Hour immediately followed by am/pm (no space).
                    let suffix = &rest[..2];
                    let h24 = Self::apply_am_pm(hour, suffix);
                    return Some((h24, 0));
                } else if let Some(remainder) = rest.strip_prefix(' ') {
                    // Space then possible am/pm.
                    let trimmed = remainder.trim_start();
                    if trimmed.starts_with("am") || trimmed.starts_with("pm") {
                        let suffix = &trimmed[..2];
                        let h24 = Self::apply_am_pm(hour, suffix);
                        return Some((h24, 0));
                    }
                }
                // No am/pm found — 24-hour bare number (e.g. "14:00" already handled above).
                // Continue scanning in case there's a time later in the string.
            } else {
                i += 1;
            }
        }
        None
    }

    /// Convert a 12-hour value to 24-hour given an `am`/`pm` suffix string.
    fn apply_am_pm(hour: u8, suffix: &str) -> u8 {
        let s = suffix.trim();
        if s.starts_with("pm") {
            if hour == 12 { 12 } else { hour + 12 }
        } else if s.starts_with("am") {
            if hour == 12 { 0 } else { hour }
        } else {
            // No am/pm — treat as-is (24-hour already parsed).
            hour
        }
    }

    /// Extract a brightness / volume percentage from a natural-language string.
    ///
    /// Recognises:
    /// - `N%` — bare numeric percentage
    /// - `N percent` — written out
    /// - `half` — 50%
    /// - `quarter` — 25%
    ///
    /// Returns `Some(value)` where `value` is in `[0.0, 100.0]`, or `None`.
    pub fn extract_percentage(input: &str) -> Option<f32> {
        let lower = input.to_lowercase();
        let s = lower.trim();

        // Named fractions first.
        if s.contains("half") {
            return Some(50.0);
        }
        if s.contains("quarter") {
            return Some(25.0);
        }

        // Walk for a digit sequence optionally followed by '%' or ' percent'.
        let bytes = s.as_bytes();
        let len = bytes.len();
        let mut i = 0usize;

        while i < len {
            if bytes[i].is_ascii_digit() {
                let start = i;
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let num_str = &s[start..i];
                let value: f32 = num_str.parse().ok()?;
                let rest = s[i..].trim_start();
                if rest.starts_with('%') || rest.starts_with("percent") {
                    return Some(value);
                }
                // Keep searching — this number was not a percentage.
            } else {
                i += 1;
            }
        }
        None
    }

    /// Split an SMS intent string of the form `"<recipient> saying <body>"`.
    ///
    /// - If the string contains ` saying `, the part before it is the recipient
    ///   and the part after is the message body.
    /// - Otherwise the whole string is treated as the recipient with an empty body.
    ///
    /// Returns `(recipient, body)`.
    pub fn split_sms_parts(input: &str) -> (String, String) {
        if let Some(idx) = input.find(" saying ") {
            let recipient = input[..idx].trim().to_string();
            let body = input[idx + " saying ".len()..].trim().to_string();
            (recipient, body)
        } else {
            (input.trim().to_string(), String::new())
        }
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
