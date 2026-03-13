//! Network connectivity monitoring for AURA v4.
//!
//! Tracks network state (WiFi, cellular, none), quality estimation, metered
//! detection, and offline/online transitions. All reads go through JNI on
//! Android with desktop mock implementations for testing.
//!
//! # Architecture
//!
//! On Android, connectivity data is fetched from `AuraDaemonBridge` static
//! methods that wrap `ConnectivityManager` / `WifiManager` APIs.  On desktop,
//! mock values simulate a normal WiFi connection.
//!
//! # Usage
//!
//! ```ignore
//! let mut cm = ConnectivityManager::new();
//! cm.poll()?;
//! if cm.is_offline() {
//!     // Queue work for later
//! }
//! ```

use std::time::{Duration, Instant};

use aura_types::errors::PlatformError;
use serde::{Deserialize, Serialize};

// ─── Constants ──────────────────────────────────────────────────────────────

/// Maximum number of entries in the network history ring.
const HISTORY_SIZE: usize = 32;

/// Number of consecutive offline samples before declaring truly offline.
const OFFLINE_CONFIRM_COUNT: usize = 3;

/// WiFi RSSI threshold for "good" signal (dBm).
const RSSI_GOOD: i32 = -55;

/// WiFi RSSI threshold for "acceptable" signal (dBm).
const RSSI_FAIR: i32 = -70;

/// WiFi RSSI threshold for "poor" signal — below this is unusable (dBm).
const RSSI_POOR: i32 = -85;

// ─── Network Types ──────────────────────────────────────────────────────────

/// Type of network connection currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NetworkType {
    /// Connected via WiFi.
    Wifi,
    /// Connected via cellular (mobile data).
    Cellular,
    /// Connected via Ethernet (rare on phones, possible via USB-C adapter).
    Ethernet,
    /// No network connection.
    None,
}

impl std::fmt::Display for NetworkType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wifi => write!(f, "WiFi"),
            Self::Cellular => write!(f, "Cellular"),
            Self::Ethernet => write!(f, "Ethernet"),
            Self::None => write!(f, "None"),
        }
    }
}

/// Estimated network quality based on signal strength and type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NetworkQuality {
    /// Excellent connectivity — fast, unmetered.
    Excellent,
    /// Good connectivity — usable for most operations.
    Good,
    /// Fair — may be slow, possibly metered.
    Fair,
    /// Poor — minimal connectivity, high latency expected.
    Poor,
    /// No connectivity.
    Offline,
}

impl std::fmt::Display for NetworkQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Excellent => write!(f, "Excellent"),
            Self::Good => write!(f, "Good"),
            Self::Fair => write!(f, "Fair"),
            Self::Poor => write!(f, "Poor"),
            Self::Offline => write!(f, "Offline"),
        }
    }
}

/// A single connectivity snapshot record for history tracking.
// Phase 8 wire point: fields read by connectivity trend analysis in JNI path.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
struct ConnectivityRecord {
    network_type: NetworkType,
    quality: NetworkQuality,
    timestamp: Instant,
}

/// Aggregated snapshot of current network state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectivitySnapshot {
    pub network_type: NetworkType,
    pub quality: NetworkQuality,
    pub is_metered: bool,
    pub wifi_rssi: Option<i32>,
    pub is_offline: bool,
    pub offline_duration: Option<Duration>,
}

// ─── ConnectivityManager ────────────────────────────────────────────────────

/// Monitors network state and provides connectivity-aware decisions.
pub struct ConnectivityManager {
    /// Current network type.
    network_type: NetworkType,
    /// Estimated network quality.
    quality: NetworkQuality,
    /// Whether the current connection is metered.
    is_metered: bool,
    /// WiFi RSSI (only valid when on WiFi).
    wifi_rssi: Option<i32>,
    /// Whether the device is confirmed offline.
    is_offline: bool,
    /// When the device went offline (if offline).
    offline_since: Option<Instant>,
    /// Consecutive offline poll count.
    offline_streak: usize,
    /// History ring of recent connectivity states.
    history: Vec<ConnectivityRecord>,
    /// Timestamp of last poll.
    last_poll: Instant,
    /// Registered callback for network changes.
    on_change: Option<Box<dyn Fn(NetworkType, NetworkQuality) + Send>>,
}

impl ConnectivityManager {
    /// Create a new `ConnectivityManager` assuming WiFi connectivity.
    pub fn new() -> Self {
        Self {
            network_type: NetworkType::Wifi,
            quality: NetworkQuality::Good,
            is_metered: false,
            wifi_rssi: None,
            is_offline: false,
            offline_since: None,
            offline_streak: 0,
            history: Vec::with_capacity(HISTORY_SIZE),
            last_poll: Instant::now(),
            on_change: None,
        }
    }

    /// Poll the platform for current connectivity state.
    pub fn poll(&mut self) -> Result<ConnectivitySnapshot, PlatformError> {
        let net_type = read_network_type()?;
        let metered = read_is_metered()?;
        let rssi = if net_type == NetworkType::Wifi {
            read_wifi_rssi().ok()
        } else {
            None
        };

        let old_type = self.network_type;
        let old_quality = self.quality;

        // Update internal state.
        self.network_type = net_type;
        self.is_metered = metered;
        self.wifi_rssi = rssi;
        self.quality = self.estimate_quality();

        // Update offline tracking.
        self.update_offline_state(net_type);

        // Record history.
        self.record_state();

        self.last_poll = Instant::now();

        // Fire change callback if state changed.
        if net_type != old_type || self.quality != old_quality {
            if let Some(cb) = &self.on_change {
                cb(self.network_type, self.quality);
            }
        }

        Ok(self.snapshot())
    }

    /// Get a snapshot of the current connectivity state without polling.
    pub fn snapshot(&self) -> ConnectivitySnapshot {
        ConnectivitySnapshot {
            network_type: self.network_type,
            quality: self.quality,
            is_metered: self.is_metered,
            wifi_rssi: self.wifi_rssi,
            is_offline: self.is_offline,
            offline_duration: self.offline_since.map(|t| t.elapsed()),
        }
    }

    /// Whether the device is currently offline.
    pub fn is_offline(&self) -> bool {
        self.is_offline
    }

    /// Whether the current connection is metered (e.g., cellular).
    pub fn is_metered(&self) -> bool {
        self.is_metered
    }

    /// Current network type.
    pub fn network_type(&self) -> NetworkType {
        self.network_type
    }

    /// Current estimated network quality.
    pub fn quality(&self) -> NetworkQuality {
        self.quality
    }

    /// WiFi signal strength (RSSI) if on WiFi.
    pub fn wifi_rssi(&self) -> Option<i32> {
        self.wifi_rssi
    }

    /// How long the device has been offline (if offline).
    pub fn offline_duration(&self) -> Option<Duration> {
        self.offline_since.map(|t| t.elapsed())
    }

    /// Time since the last connectivity poll.
    pub fn time_since_last_poll(&self) -> Duration {
        self.last_poll.elapsed()
    }

    /// Register a callback for network state changes.
    ///
    /// Only one callback is supported; calling this again replaces the previous one.
    pub fn set_on_change<F>(&mut self, callback: F)
    where
        F: Fn(NetworkType, NetworkQuality) + Send + 'static,
    {
        self.on_change = Some(Box::new(callback));
    }

    /// Whether AURA should allow large data transfers (model downloads, etc.).
    ///
    /// Returns `true` on unmetered connections with at least Good quality.
    pub fn should_allow_large_transfer(&self) -> bool {
        !self.is_metered
            && !self.is_offline
            && matches!(
                self.quality,
                NetworkQuality::Excellent | NetworkQuality::Good
            )
    }

    /// Whether AURA should allow any network activity.
    ///
    /// Returns `false` only when truly offline.
    pub fn should_allow_network(&self) -> bool {
        !self.is_offline
    }

    /// Throttle factor for network-dependent operations.
    ///
    /// Returns 0.0 (offline) to 1.0 (excellent connectivity).
    pub fn network_throttle_factor(&self) -> f32 {
        match self.quality {
            NetworkQuality::Excellent => 1.0,
            NetworkQuality::Good => 0.8,
            NetworkQuality::Fair => 0.5,
            NetworkQuality::Poor => 0.2,
            NetworkQuality::Offline => 0.0,
        }
    }

    /// Number of state change records in the history.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    // ─── Internal ───────────────────────────────────────────────────────

    /// Estimate network quality from type and signal strength.
    fn estimate_quality(&self) -> NetworkQuality {
        if self.network_type == NetworkType::None {
            return NetworkQuality::Offline;
        }

        match self.network_type {
            NetworkType::Wifi => {
                if let Some(rssi) = self.wifi_rssi {
                    if rssi >= RSSI_GOOD {
                        NetworkQuality::Excellent
                    } else if rssi >= RSSI_FAIR {
                        NetworkQuality::Good
                    } else if rssi >= RSSI_POOR {
                        NetworkQuality::Fair
                    } else {
                        NetworkQuality::Poor
                    }
                } else {
                    // WiFi without RSSI data — assume Good.
                    NetworkQuality::Good
                }
            }
            NetworkType::Cellular => {
                // Cellular is always at most Fair if metered, Good if unmetered.
                if self.is_metered {
                    NetworkQuality::Fair
                } else {
                    NetworkQuality::Good
                }
            }
            NetworkType::Ethernet => NetworkQuality::Excellent,
            NetworkType::None => NetworkQuality::Offline,
        }
    }

    /// Update offline detection with hysteresis.
    fn update_offline_state(&mut self, net_type: NetworkType) {
        if net_type == NetworkType::None {
            self.offline_streak += 1;
            if self.offline_streak >= OFFLINE_CONFIRM_COUNT && !self.is_offline {
                self.is_offline = true;
                self.offline_since = Some(Instant::now());
                tracing::warn!(
                    "device confirmed offline after {} polls",
                    self.offline_streak
                );
            }
        } else {
            if self.is_offline {
                if let Some(since) = self.offline_since {
                    tracing::info!(
                        duration_secs = since.elapsed().as_secs(),
                        new_type = %net_type,
                        "device back online"
                    );
                }
            }
            self.offline_streak = 0;
            self.is_offline = false;
            self.offline_since = None;
        }
    }

    /// Record current state in the history ring.
    fn record_state(&mut self) {
        if self.history.len() >= HISTORY_SIZE {
            self.history.remove(0);
        }
        self.history.push(ConnectivityRecord {
            network_type: self.network_type,
            quality: self.quality,
            timestamp: Instant::now(),
        });
    }
}

impl Default for ConnectivityManager {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Platform Reads (cfg-gated) ─────────────────────────────────────────────

/// Read current network type from the platform.
#[cfg(target_os = "android")]
fn read_network_type() -> Result<NetworkType, PlatformError> {
    let type_str = super::jni_bridge::jni_get_network_type()?;
    match type_str.as_str() {
        "wifi" | "WIFI" => Ok(NetworkType::Wifi),
        "cellular" | "CELLULAR" | "mobile" => Ok(NetworkType::Cellular),
        "ethernet" | "ETHERNET" => Ok(NetworkType::Ethernet),
        _ => Ok(NetworkType::None),
    }
}

#[cfg(not(target_os = "android"))]
fn read_network_type() -> Result<NetworkType, PlatformError> {
    // Mock: WiFi connected.
    Ok(NetworkType::Wifi)
}

/// Read whether the current connection is metered.
#[cfg(target_os = "android")]
fn read_is_metered() -> Result<bool, PlatformError> {
    super::jni_bridge::jni_is_network_metered()
}

#[cfg(not(target_os = "android"))]
fn read_is_metered() -> Result<bool, PlatformError> {
    // Mock: WiFi is not metered.
    Ok(false)
}

/// Read WiFi signal strength (RSSI in dBm).
#[cfg(target_os = "android")]
fn read_wifi_rssi() -> Result<i32, PlatformError> {
    super::jni_bridge::jni_get_wifi_rssi()
}

#[cfg(not(target_os = "android"))]
fn read_wifi_rssi() -> Result<i32, PlatformError> {
    // Mock: good WiFi signal.
    Ok(-45)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connectivity_manager_default() {
        let cm = ConnectivityManager::new();
        assert_eq!(cm.network_type(), NetworkType::Wifi);
        assert_eq!(cm.quality(), NetworkQuality::Good);
        assert!(!cm.is_metered());
        assert!(!cm.is_offline());
    }

    #[test]
    fn test_poll_returns_snapshot() {
        let mut cm = ConnectivityManager::new();
        let snap = cm.poll().expect("poll should succeed on host");
        assert_eq!(snap.network_type, NetworkType::Wifi);
        assert!(!snap.is_metered);
        assert!(!snap.is_offline);
    }

    #[test]
    fn test_wifi_quality_excellent() {
        let mut cm = ConnectivityManager::new();
        cm.poll().expect("poll");
        // Mock RSSI is -45 → Excellent.
        assert_eq!(cm.quality(), NetworkQuality::Excellent);
    }

    #[test]
    fn test_network_type_display() {
        assert_eq!(NetworkType::Wifi.to_string(), "WiFi");
        assert_eq!(NetworkType::Cellular.to_string(), "Cellular");
        assert_eq!(NetworkType::Ethernet.to_string(), "Ethernet");
        assert_eq!(NetworkType::None.to_string(), "None");
    }

    #[test]
    fn test_network_quality_display() {
        assert_eq!(NetworkQuality::Excellent.to_string(), "Excellent");
        assert_eq!(NetworkQuality::Good.to_string(), "Good");
        assert_eq!(NetworkQuality::Fair.to_string(), "Fair");
        assert_eq!(NetworkQuality::Poor.to_string(), "Poor");
        assert_eq!(NetworkQuality::Offline.to_string(), "Offline");
    }

    #[test]
    fn test_estimate_quality_no_network() {
        let mut cm = ConnectivityManager::new();
        cm.network_type = NetworkType::None;
        cm.quality = cm.estimate_quality();
        assert_eq!(cm.quality(), NetworkQuality::Offline);
    }

    #[test]
    fn test_estimate_quality_wifi_rssi_tiers() {
        let mut cm = ConnectivityManager::new();
        cm.network_type = NetworkType::Wifi;

        cm.wifi_rssi = Some(-40);
        assert_eq!(cm.estimate_quality(), NetworkQuality::Excellent);

        cm.wifi_rssi = Some(-60);
        assert_eq!(cm.estimate_quality(), NetworkQuality::Good);

        cm.wifi_rssi = Some(-75);
        assert_eq!(cm.estimate_quality(), NetworkQuality::Fair);

        cm.wifi_rssi = Some(-90);
        assert_eq!(cm.estimate_quality(), NetworkQuality::Poor);
    }

    #[test]
    fn test_estimate_quality_cellular_metered() {
        let mut cm = ConnectivityManager::new();
        cm.network_type = NetworkType::Cellular;
        cm.is_metered = true;
        assert_eq!(cm.estimate_quality(), NetworkQuality::Fair);

        cm.is_metered = false;
        assert_eq!(cm.estimate_quality(), NetworkQuality::Good);
    }

    #[test]
    fn test_estimate_quality_ethernet() {
        let mut cm = ConnectivityManager::new();
        cm.network_type = NetworkType::Ethernet;
        assert_eq!(cm.estimate_quality(), NetworkQuality::Excellent);
    }

    #[test]
    fn test_offline_detection_hysteresis() {
        let mut cm = ConnectivityManager::new();
        // Simulate going offline.
        cm.update_offline_state(NetworkType::None);
        assert!(!cm.is_offline()); // Not confirmed yet (streak = 1).

        cm.update_offline_state(NetworkType::None);
        assert!(!cm.is_offline()); // streak = 2.

        cm.update_offline_state(NetworkType::None);
        assert!(cm.is_offline()); // streak = 3 → confirmed.
        assert!(cm.offline_since.is_some());
    }

    #[test]
    fn test_offline_recovery() {
        let mut cm = ConnectivityManager::new();
        // Go offline.
        for _ in 0..OFFLINE_CONFIRM_COUNT {
            cm.update_offline_state(NetworkType::None);
        }
        assert!(cm.is_offline());

        // Come back online.
        cm.update_offline_state(NetworkType::Wifi);
        assert!(!cm.is_offline());
        assert!(cm.offline_since.is_none());
        assert_eq!(cm.offline_streak, 0);
    }

    #[test]
    fn test_should_allow_large_transfer() {
        let mut cm = ConnectivityManager::new();
        cm.poll().expect("poll");
        // WiFi, not metered, Excellent → should allow.
        assert!(cm.should_allow_large_transfer());

        // Metered → should not allow.
        cm.is_metered = true;
        assert!(!cm.should_allow_large_transfer());

        // Offline → should not allow.
        cm.is_metered = false;
        cm.is_offline = true;
        assert!(!cm.should_allow_large_transfer());
    }

    #[test]
    fn test_should_allow_network() {
        let cm = ConnectivityManager::new();
        assert!(cm.should_allow_network());

        let mut cm2 = ConnectivityManager::new();
        cm2.is_offline = true;
        assert!(!cm2.should_allow_network());
    }

    #[test]
    fn test_network_throttle_factor() {
        let mut cm = ConnectivityManager::new();

        cm.quality = NetworkQuality::Excellent;
        assert!((cm.network_throttle_factor() - 1.0).abs() < f32::EPSILON);

        cm.quality = NetworkQuality::Good;
        assert!((cm.network_throttle_factor() - 0.8).abs() < f32::EPSILON);

        cm.quality = NetworkQuality::Fair;
        assert!((cm.network_throttle_factor() - 0.5).abs() < f32::EPSILON);

        cm.quality = NetworkQuality::Poor;
        assert!((cm.network_throttle_factor() - 0.2).abs() < f32::EPSILON);

        cm.quality = NetworkQuality::Offline;
        assert!((cm.network_throttle_factor() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_history_bounded() {
        let mut cm = ConnectivityManager::new();
        for _ in 0..(HISTORY_SIZE + 10) {
            cm.record_state();
        }
        assert_eq!(cm.history.len(), HISTORY_SIZE);
    }

    #[test]
    fn test_time_since_last_poll() {
        let cm = ConnectivityManager::new();
        assert!(cm.time_since_last_poll() < Duration::from_secs(1));
    }

    #[test]
    fn test_change_callback_fires() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let fired = Arc::new(AtomicBool::new(false));
        let fired_clone = fired.clone();

        let mut cm = ConnectivityManager::new();
        // Set initial state to None so the first poll (which returns WiFi) triggers a change.
        cm.network_type = NetworkType::None;
        cm.quality = NetworkQuality::Offline;

        cm.set_on_change(move |_net_type, _quality| {
            fired_clone.store(true, Ordering::SeqCst);
        });

        cm.poll().expect("poll");
        assert!(fired.load(Ordering::SeqCst));
    }

    #[test]
    fn test_desktop_mock_network_type() {
        let net = read_network_type().expect("mock");
        assert_eq!(net, NetworkType::Wifi);
    }

    #[test]
    fn test_desktop_mock_metered() {
        let metered = read_is_metered().expect("mock");
        assert!(!metered);
    }

    #[test]
    fn test_desktop_mock_wifi_rssi() {
        let rssi = read_wifi_rssi().expect("mock");
        assert_eq!(rssi, -45);
    }

    #[test]
    fn test_snapshot_without_poll() {
        let cm = ConnectivityManager::new();
        let snap = cm.snapshot();
        assert_eq!(snap.network_type, NetworkType::Wifi);
        assert!(!snap.is_offline);
    }
}
