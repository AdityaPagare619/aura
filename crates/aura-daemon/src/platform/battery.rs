//! Battery path resolver for cross-device compatibility.
//!
//! Different Android devices use different sysfs paths for battery information.
//! This module tries multiple path variants to find the correct one.
//! These constants are used at runtime on Android; they appear dead on host builds.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::OnceLock;

/// Battery information paths for different device types.
#[derive(Debug, Clone)]
pub struct BatteryPaths {
    /// Primary battery capacity path
    pub capacity: PathBuf,
    /// Primary battery status path
    pub status: PathBuf,
    /// Primary battery voltage path (optional)
    pub voltage: Option<PathBuf>,
    /// Primary battery current path (optional)
    pub current: Option<PathBuf>,
    /// Primary battery temperature path (optional)
    pub temperature: Option<PathBuf>,
}

/// Cached battery paths (detected once at startup).
static BATTERY_PATHS: OnceLock<BatteryPaths> = OnceLock::new();

/// Known battery path variants for different Android devices.
const BATTERY_CAPACITY_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/capacity", // Most devices
    "/sys/class/power_supply/Battery/capacity", // Samsung
    "/sys/class/power_supply/bms/capacity",     // Some Qualcomm
    "/sys/class/power_supply/main_battery/capacity", // Some devices
    "/sys/class/power_supply/dc/capacity",      // Some devices
];

const BATTERY_STATUS_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/status", // Most devices
    "/sys/class/power_supply/Battery/status", // Samsung
    "/sys/class/power_supply/bms/status",     // Some Qualcomm
    "/sys/class/power_supply/main_battery/status", // Some devices
    "/sys/class/power_supply/dc/status",      // Some devices
];

const BATTERY_VOLTAGE_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/voltage_now",
    "/sys/class/power_supply/Battery/voltage_now",
    "/sys/class/power_supply/bms/voltage_now",
    "/sys/class/power_supply/main_battery/voltage_now",
];

const BATTERY_CURRENT_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/current_now",
    "/sys/class/power_supply/Battery/current_now",
    "/sys/class/power_supply/bms/current_now",
    "/sys/class/power_supply/main_battery/current_now",
];

const BATTERY_TEMPERATURE_PATHS: &[&str] = &[
    "/sys/class/power_supply/battery/temp",
    "/sys/class/power_supply/Battery/temp",
    "/sys/class/power_supply/bms/temp",
    "/sys/class/power_supply/main_battery/temp",
    "/sys/class/thermal/thermal_zone0/temp", // Fallback to thermal zone
];

impl BatteryPaths {
    /// Detect battery paths at runtime.
    ///
    /// Tries multiple known path variants and returns the first one that exists.
    pub fn detect() -> &'static BatteryPaths {
        BATTERY_PATHS.get_or_init(Self::do_detect)
    }

    fn do_detect() -> Self {
        #[cfg(target_os = "android")]
        {
            Self::detect_android()
        }

        #[cfg(not(target_os = "android"))]
        {
            Self::detect_host()
        }
    }

    #[cfg(target_os = "android")]
    fn detect_android() -> Self {
        let capacity = Self::find_first_existing(BATTERY_CAPACITY_PATHS)
            .unwrap_or_else(|| PathBuf::from("/sys/class/power_supply/battery/capacity"));

        let status = Self::find_first_existing(BATTERY_STATUS_PATHS)
            .unwrap_or_else(|| PathBuf::from("/sys/class/power_supply/battery/status"));

        let voltage = Self::find_first_existing(BATTERY_VOLTAGE_PATHS);
        let current = Self::find_first_existing(BATTERY_CURRENT_PATHS);
        let temperature = Self::find_first_existing(BATTERY_TEMPERATURE_PATHS);

        Self {
            capacity,
            status,
            voltage,
            current,
            temperature,
        }
    }

    #[cfg(not(target_os = "android"))]
    fn detect_host() -> Self {
        // On host builds, return default paths (won't be used)
        Self {
            capacity: PathBuf::from("/sys/class/power_supply/battery/capacity"),
            status: PathBuf::from("/sys/class/power_supply/battery/status"),
            voltage: None,
            current: None,
            temperature: None,
        }
    }

    fn find_first_existing(paths: &[&str]) -> Option<PathBuf> {
        for path in paths {
            let path_buf = PathBuf::from(path);
            if path_buf.exists() {
                return Some(path_buf);
            }
        }
        None
    }

    /// Read battery capacity (0-100%).
    pub fn read_capacity(&self) -> Option<u8> {
        let content = std::fs::read_to_string(&self.capacity).ok()?;
        let trimmed = content.trim();
        trimmed.parse::<u8>().ok().map(|pct| pct.min(100))
    }

    /// Read battery status.
    ///
    /// Returns "Charging", "Discharging", "Full", "Not charging", or "Unknown".
    pub fn read_status(&self) -> String {
        std::fs::read_to_string(&self.status)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "Unknown".to_string())
    }

    /// Check if battery is charging.
    pub fn is_charging(&self) -> bool {
        let status = self.read_status();
        status == "Charging" || status == "Full"
    }

    /// Read battery voltage in microvolts (µV).
    pub fn read_voltage_uv(&self) -> Option<u64> {
        let path = self.voltage.as_ref()?;
        let content = std::fs::read_to_string(path).ok()?;
        let trimmed = content.trim();
        trimmed.parse::<u64>().ok()
    }

    /// Read battery current in microamps (µA).
    ///
    /// Positive = charging, negative = discharging.
    pub fn read_current_ua(&self) -> Option<i64> {
        let path = self.current.as_ref()?;
        let content = std::fs::read_to_string(path).ok()?;
        let trimmed = content.trim();
        trimmed.parse::<i64>().ok()
    }

    /// Read battery temperature in decidegrees Celsius.
    ///
    /// Divide by 10 to get degrees Celsius.
    pub fn read_temperature_decic(&self) -> Option<i64> {
        let path = self.temperature.as_ref()?;
        let content = std::fs::read_to_string(path).ok()?;
        let trimmed = content.trim();
        // Some devices report in millidegrees, some in decidegrees
        let val: i64 = trimmed.parse().ok()?;
        // If value is very large (> 1000), assume millidegrees
        if val.abs() > 1000 {
            Some(val / 100) // millidegrees → decidegrees
        } else {
            Some(val)
        }
    }

    /// Get a list of all supported battery paths.
    pub fn supported_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();

        paths.push(format!("capacity: {}", self.capacity.display()));
        paths.push(format!("status: {}", self.status.display()));

        if let Some(ref v) = self.voltage {
            paths.push(format!("voltage: {}", v.display()));
        }
        if let Some(ref c) = self.current {
            paths.push(format!("current: {}", c.display()));
        }
        if let Some(ref t) = self.temperature {
            paths.push(format!("temperature: {}", t.display()));
        }

        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_battery_paths_detect() {
        let paths = BatteryPaths::detect();
        // On host builds, paths should exist (even if they don't work)
        assert!(!paths.capacity.as_os_str().is_empty());
        assert!(!paths.status.as_os_str().is_empty());
    }

    #[test]
    fn test_supported_paths() {
        let paths = BatteryPaths::detect();
        let supported = paths.supported_paths();
        // Should have at least capacity and status
        assert!(supported.len() >= 2);
    }

    #[test]
    fn test_host_build() {
        #[cfg(not(target_os = "android"))]
        {
            let paths = BatteryPaths::detect();
            // On host, voltage/current/temperature should be None
            assert!(paths.voltage.is_none());
            assert!(paths.current.is_none());
            assert!(paths.temperature.is_none());
        }
    }
}
