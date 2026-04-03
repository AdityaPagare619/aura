//! Runtime device detection for Android compatibility.
//!
//! Provides runtime detection of Android version, device capabilities,
//! and platform features. This allows AURA to adapt its behavior based
//! on the actual device it's running on.

use std::sync::OnceLock;

/// Runtime device information detected at startup.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Android API level (21-34+)
    pub api_level: u32,
    /// Device manufacturer (e.g., "samsung", "xiaomi", "google")
    pub manufacturer: String,
    /// Device model (e.g., "SM-G998B", "Pixel 7")
    pub model: String,
    /// Android version string (e.g., "13", "14")
    pub android_version: String,
    /// Whether device is rooted
    pub is_rooted: bool,
    /// Whether device supports 64-bit
    pub is_64bit: bool,
    /// Whether running in Termux environment
    pub is_termux: bool,
    /// Whether running in Android app context (APK)
    pub is_apk: bool,
}

/// Cached device info (computed once at startup).
static DEVICE_INFO: OnceLock<DeviceInfo> = OnceLock::new();

impl DeviceInfo {
    /// Detect device information at runtime.
    ///
    /// On Android, reads from `/system/build.prop` and JNI.
    /// On non-Android, returns sensible defaults.
    pub fn detect() -> &'static DeviceInfo {
        DEVICE_INFO.get_or_init(Self::do_detect)
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
        let api_level = Self::read_api_level();
        let manufacturer = Self::read_manufacturer();
        let model = Self::read_model();
        let android_version = Self::read_android_version();
        let is_rooted = Self::check_rooted();
        let is_64bit = Self::check_64bit();
        let is_termux = Self::check_termux();
        let is_apk = !is_termux;

        Self {
            api_level,
            manufacturer,
            model,
            android_version,
            is_rooted,
            is_64bit,
            is_termux,
            is_apk,
        }
    }

    #[cfg(not(target_os = "android"))]
    fn detect_host() -> Self {
        Self {
            api_level: 0,
            manufacturer: "host".to_string(),
            model: "development".to_string(),
            android_version: "N/A".to_string(),
            is_rooted: false,
            is_64bit: cfg!(target_pointer_width = "64"),
            is_termux: false,
            is_apk: false,
        }
    }

    #[cfg(target_os = "android")]
    fn read_api_level() -> u32 {
        // Try JNI first
        if let Ok(level) = crate::platform::jni_bridge::jni_get_device_api_level() {
            return level;
        }

        // Fallback: read from build.prop
        if let Ok(content) = std::fs::read_to_string("/system/build.prop") {
            for line in content.lines() {
                if line.starts_with("ro.build.version.sdk=") {
                    if let Some(value) = line.split('=').nth(1) {
                        if let Ok(level) = value.trim().parse::<u32>() {
                            return level;
                        }
                    }
                }
            }
        }

        // Default to Android 7.0 (API 24) if detection fails
        24
    }

    #[cfg(target_os = "android")]
    fn read_manufacturer() -> String {
        // Try JNI first
        if let Ok(mfr) = crate::platform::jni_bridge::jni_get_device_manufacturer() {
            return mfr;
        }

        // Fallback: read from build.prop
        if let Ok(content) = std::fs::read_to_string("/system/build.prop") {
            for line in content.lines() {
                if line.starts_with("ro.product.manufacturer=") {
                    if let Some(value) = line.split('=').nth(1) {
                        return value.trim().to_lowercase();
                    }
                }
            }
        }

        "unknown".to_string()
    }

    #[cfg(target_os = "android")]
    fn read_model() -> String {
        if let Ok(content) = std::fs::read_to_string("/system/build.prop") {
            for line in content.lines() {
                if line.starts_with("ro.product.model=") {
                    if let Some(value) = line.split('=').nth(1) {
                        return value.trim().to_string();
                    }
                }
            }
        }

        "unknown".to_string()
    }

    #[cfg(target_os = "android")]
    fn read_android_version() -> String {
        if let Ok(content) = std::fs::read_to_string("/system/build.prop") {
            for line in content.lines() {
                if line.starts_with("ro.build.version.release=") {
                    if let Some(value) = line.split('=').nth(1) {
                        return value.trim().to_string();
                    }
                }
            }
        }

        "unknown".to_string()
    }

    #[cfg(target_os = "android")]
    fn check_rooted() -> bool {
        // Check for common root indicators
        let root_indicators = [
            "/system/xbin/su",
            "/system/bin/su",
            "/sbin/su",
            "/data/local/su",
            "/data/local/bin/su",
            "/data/local/xbin/su",
        ];

        for path in root_indicators {
            if std::path::Path::new(path).exists() {
                return true;
            }
        }

        // Check for Magisk
        if std::path::Path::new("/sbin/.magisk").exists() {
            return true;
        }

        false
    }

    #[cfg(target_os = "android")]
    fn check_64bit() -> bool {
        // Check if running on 64-bit kernel
        if let Ok(arch) = std::env::var("TERMUX_ARCH") {
            return arch.contains("64") || arch == "aarch64";
        }

        // Check uname
        if let Ok(output) = std::process::Command::new("uname").arg("-m").output() {
            let arch = String::from_utf8_lossy(&output.stdout);
            return arch.contains("64") || arch == "aarch64";
        }

        // Default to checking target pointer width
        cfg!(target_pointer_width = "64")
    }

    #[cfg(target_os = "android")]
    fn check_termux() -> bool {
        // Check for Termux environment variables
        if std::env::var("TERMUX_VERSION").is_ok() {
            return true;
        }

        if std::env::var("PREFIX")
            .map(|p| p.contains("termux"))
            .unwrap_or(false)
        {
            return true;
        }

        // Check for Termux-specific paths
        if std::path::Path::new("/data/data/com.termux").exists() {
            return true;
        }

        false
    }

    /// Check if the device is a Samsung device.
    pub fn is_samsung(&self) -> bool {
        self.manufacturer.contains("samsung")
    }

    /// Check if the device is a Xiaomi device.
    pub fn is_xiaomi(&self) -> bool {
        self.manufacturer.contains("xiaomi")
            || self.manufacturer.contains("redmi")
            || self.manufacturer.contains("poco")
    }

    /// Check if the device is a Huawei device.
    pub fn is_huawei(&self) -> bool {
        self.manufacturer.contains("huawei") || self.manufacturer.contains("honor")
    }

    /// Check if the device is a Google Pixel.
    pub fn is_pixel(&self) -> bool {
        self.model.to_lowercase().contains("pixel")
    }

    /// Check if the device supports scoped storage (API 29+).
    pub fn supports_scoped_storage(&self) -> bool {
        self.api_level >= 29
    }

    /// Check if the device requires foreground service types (API 34+).
    pub fn requires_foreground_service_types(&self) -> bool {
        self.api_level >= 34
    }

    /// Check if the device supports runtime permissions (API 23+).
    pub fn supports_runtime_permissions(&self) -> bool {
        self.api_level >= 23
    }

    /// Check if the device has background execution limits (API 26+).
    pub fn has_background_execution_limits(&self) -> bool {
        self.api_level >= 26
    }

    /// Check if the device supports battery optimization exemptions (API 23+).
    pub fn supports_battery_optimization(&self) -> bool {
        self.api_level >= 23
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_info_detect() {
        let info = DeviceInfo::detect();
        // On host builds, api_level should be 0
        #[cfg(not(target_os = "android"))]
        assert_eq!(info.api_level, 0);
    }

    #[test]
    fn test_device_info_clone() {
        let info = DeviceInfo::detect();
        let cloned = info.clone();
        assert_eq!(info.api_level, cloned.api_level);
    }

    #[test]
    fn test_scoped_storage_check() {
        let info = DeviceInfo::detect();
        #[cfg(not(target_os = "android"))]
        assert!(!info.supports_scoped_storage());
    }
}
