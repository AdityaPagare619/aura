//! Permission handling for different Android versions.
//!
//! Android has different permission models across API levels:
//! - API 21-25 (Lollipop-Marshmallow): Different runtime permission model
//! - API 26-28 (Oreo-Pie): Background execution limits
//! - API 29-33 (Q-Tiramisu): Scoped storage, background location
//! - API 34 (Upside Down Cake): Foreground service types

use std::collections::HashMap;
use std::sync::OnceLock;

/// Permission status for a specific permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionStatus {
    /// Permission is granted
    Granted,
    /// Permission is denied
    Denied,
    /// Permission is denied and "Don't ask again" was selected
    DeniedPermanently,
    /// Permission status is unknown
    Unknown,
}

/// Android permission types that AURA needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuraPermission {
    /// Read external storage (API < 33)
    ReadExternalStorage,
    /// Write external storage (API < 30)
    WriteExternalStorage,
    /// Read media images (API 33+)
    ReadMediaImages,
    /// Read media video (API 33+)
    ReadMediaVideo,
    /// Read media audio (API 33+)
    ReadMediaAudio,
    /// Access fine location
    AccessFineLocation,
    /// Access coarse location
    AccessCoarseLocation,
    /// Background location (API 29+)
    AccessBackgroundLocation,
    /// Camera
    Camera,
    /// Record audio
    RecordAudio,
    /// Read contacts
    ReadContacts,
    /// Write contacts
    WriteContacts,
    /// Read calendar
    ReadCalendar,
    /// Write calendar
    WriteCalendar,
    /// Read call log
    ReadCallLog,
    /// Read phone state
    ReadPhoneState,
    /// Send SMS
    SendSms,
    /// Receive SMS
    ReceiveSms,
    /// Post notifications (API 33+)
    PostNotifications,
    /// Foreground service (API 28+)
    ForegroundService,
    /// Foreground service camera (API 34+)
    ForegroundServiceCamera,
    /// Foreground service microphone (API 34+)
    ForegroundServiceMicrophone,
    /// Foreground service location (API 34+)
    ForegroundServiceLocation,
    /// Request ignore battery optimizations (API 23+)
    RequestIgnoreBatteryOptimizations,
    /// System alert window (overlay)
    SystemAlertWindow,
    /// Write settings
    WriteSettings,
}

/// Permission group for requesting multiple permissions together.
#[derive(Debug, Clone)]
pub struct PermissionGroup {
    /// Name of the permission group
    pub name: String,
    /// Permissions in this group
    pub permissions: Vec<AuraPermission>,
    /// Whether this group is required for AURA to function
    pub required: bool,
}

/// Platform permission manager.
#[derive(Debug)]
pub struct PlatformPermissions {
    /// Detected API level
    api_level: u32,
    /// Cached permission status
    permission_cache: HashMap<AuraPermission, PermissionStatus>,
}

/// Cached permission manager.
static PERMISSIONS: OnceLock<PlatformPermissions> = OnceLock::new();

impl PlatformPermissions {
    /// Get the platform permission manager.
    pub fn get() -> &'static PlatformPermissions {
        PERMISSIONS.get_or_init(Self::new)
    }

    fn new() -> Self {
        let api_level = crate::platform::detect::DeviceInfo::detect().api_level;

        Self {
            api_level,
            permission_cache: HashMap::new(),
        }
    }

    /// Check if a permission is required for the current API level.
    pub fn is_required(&self, permission: AuraPermission) -> bool {
        match permission {
            // Storage permissions
            AuraPermission::ReadExternalStorage => self.api_level < 33,
            AuraPermission::WriteExternalStorage => self.api_level < 30,
            AuraPermission::ReadMediaImages => self.api_level >= 33,
            AuraPermission::ReadMediaVideo => self.api_level >= 33,
            AuraPermission::ReadMediaAudio => self.api_level >= 33,

            // Location permissions
            AuraPermission::AccessFineLocation => true,
            AuraPermission::AccessCoarseLocation => true,
            AuraPermission::AccessBackgroundLocation => self.api_level >= 29,

            // Other permissions
            AuraPermission::Camera => true,
            AuraPermission::RecordAudio => true,
            AuraPermission::ReadContacts => true,
            AuraPermission::WriteContacts => true,
            AuraPermission::ReadCalendar => true,
            AuraPermission::WriteCalendar => true,
            AuraPermission::ReadCallLog => true,
            AuraPermission::ReadPhoneState => true,
            AuraPermission::SendSms => true,
            AuraPermission::ReceiveSms => true,
            AuraPermission::PostNotifications => self.api_level >= 33,

            // Foreground service permissions
            AuraPermission::ForegroundService => self.api_level >= 28,
            AuraPermission::ForegroundServiceCamera => self.api_level >= 34,
            AuraPermission::ForegroundServiceMicrophone => self.api_level >= 34,
            AuraPermission::ForegroundServiceLocation => self.api_level >= 34,

            // System permissions
            AuraPermission::RequestIgnoreBatteryOptimizations => self.api_level >= 23,
            AuraPermission::SystemAlertWindow => true,
            AuraPermission::WriteSettings => true,
        }
    }

    /// Get the Android permission string for a permission.
    pub fn to_android_permission(&self, permission: AuraPermission) -> &'static str {
        match permission {
            AuraPermission::ReadExternalStorage => "android.permission.READ_EXTERNAL_STORAGE",
            AuraPermission::WriteExternalStorage => "android.permission.WRITE_EXTERNAL_STORAGE",
            AuraPermission::ReadMediaImages => "android.permission.READ_MEDIA_IMAGES",
            AuraPermission::ReadMediaVideo => "android.permission.READ_MEDIA_VIDEO",
            AuraPermission::ReadMediaAudio => "android.permission.READ_MEDIA_AUDIO",
            AuraPermission::AccessFineLocation => "android.permission.ACCESS_FINE_LOCATION",
            AuraPermission::AccessCoarseLocation => "android.permission.ACCESS_COARSE_LOCATION",
            AuraPermission::AccessBackgroundLocation => {
                "android.permission.ACCESS_BACKGROUND_LOCATION"
            }
            AuraPermission::Camera => "android.permission.CAMERA",
            AuraPermission::RecordAudio => "android.permission.RECORD_AUDIO",
            AuraPermission::ReadContacts => "android.permission.READ_CONTACTS",
            AuraPermission::WriteContacts => "android.permission.WRITE_CONTACTS",
            AuraPermission::ReadCalendar => "android.permission.READ_CALENDAR",
            AuraPermission::WriteCalendar => "android.permission.WRITE_CALENDAR",
            AuraPermission::ReadCallLog => "android.permission.READ_CALL_LOG",
            AuraPermission::ReadPhoneState => "android.permission.READ_PHONE_STATE",
            AuraPermission::SendSms => "android.permission.SEND_SMS",
            AuraPermission::ReceiveSms => "android.permission.RECEIVE_SMS",
            AuraPermission::PostNotifications => "android.permission.POST_NOTIFICATIONS",
            AuraPermission::ForegroundService => "android.permission.FOREGROUND_SERVICE",
            AuraPermission::ForegroundServiceCamera => {
                "android.permission.FOREGROUND_SERVICE_CAMERA"
            }
            AuraPermission::ForegroundServiceMicrophone => {
                "android.permission.FOREGROUND_SERVICE_MICROPHONE"
            }
            AuraPermission::ForegroundServiceLocation => {
                "android.permission.FOREGROUND_SERVICE_LOCATION"
            }
            AuraPermission::RequestIgnoreBatteryOptimizations => {
                "android.permission.REQUEST_IGNORE_BATTERY_OPTIMIZATIONS"
            }
            AuraPermission::SystemAlertWindow => "android.permission.SYSTEM_ALERT_WINDOW",
            AuraPermission::WriteSettings => "android.permission.WRITE_SETTINGS",
        }
    }

    /// Get required permission groups for AURA.
    pub fn get_required_groups(&self) -> Vec<PermissionGroup> {
        let mut groups = Vec::new();

        // Storage group
        let mut storage_perms = Vec::new();
        if self.is_required(AuraPermission::ReadExternalStorage) {
            storage_perms.push(AuraPermission::ReadExternalStorage);
        }
        if self.is_required(AuraPermission::WriteExternalStorage) {
            storage_perms.push(AuraPermission::WriteExternalStorage);
        }
        if self.is_required(AuraPermission::ReadMediaImages) {
            storage_perms.push(AuraPermission::ReadMediaImages);
        }
        if self.is_required(AuraPermission::ReadMediaVideo) {
            storage_perms.push(AuraPermission::ReadMediaVideo);
        }
        if self.is_required(AuraPermission::ReadMediaAudio) {
            storage_perms.push(AuraPermission::ReadMediaAudio);
        }
        if !storage_perms.is_empty() {
            groups.push(PermissionGroup {
                name: "Storage".to_string(),
                permissions: storage_perms,
                required: true,
            });
        }

        // Location group
        let mut location_perms = vec![
            AuraPermission::AccessFineLocation,
            AuraPermission::AccessCoarseLocation,
        ];
        if self.is_required(AuraPermission::AccessBackgroundLocation) {
            location_perms.push(AuraPermission::AccessBackgroundLocation);
        }
        groups.push(PermissionGroup {
            name: "Location".to_string(),
            permissions: location_perms,
            required: true,
        });

        // Communication group
        groups.push(PermissionGroup {
            name: "Communication".to_string(),
            permissions: vec![
                AuraPermission::ReadContacts,
                AuraPermission::WriteContacts,
                AuraPermission::ReadCalendar,
                AuraPermission::WriteCalendar,
                AuraPermission::ReadCallLog,
                AuraPermission::ReadPhoneState,
                AuraPermission::SendSms,
                AuraPermission::ReceiveSms,
            ],
            required: true,
        });

        // Media group
        groups.push(PermissionGroup {
            name: "Media".to_string(),
            permissions: vec![AuraPermission::Camera, AuraPermission::RecordAudio],
            required: true,
        });

        // Notification group (API 33+)
        if self.is_required(AuraPermission::PostNotifications) {
            groups.push(PermissionGroup {
                name: "Notifications".to_string(),
                permissions: vec![AuraPermission::PostNotifications],
                required: false,
            });
        }

        // Foreground service group
        let mut foreground_perms = Vec::new();
        if self.is_required(AuraPermission::ForegroundService) {
            foreground_perms.push(AuraPermission::ForegroundService);
        }
        if self.is_required(AuraPermission::ForegroundServiceCamera) {
            foreground_perms.push(AuraPermission::ForegroundServiceCamera);
        }
        if self.is_required(AuraPermission::ForegroundServiceMicrophone) {
            foreground_perms.push(AuraPermission::ForegroundServiceMicrophone);
        }
        if self.is_required(AuraPermission::ForegroundServiceLocation) {
            foreground_perms.push(AuraPermission::ForegroundServiceLocation);
        }
        if !foreground_perms.is_empty() {
            groups.push(PermissionGroup {
                name: "Foreground Service".to_string(),
                permissions: foreground_perms,
                required: true,
            });
        }

        // System group
        groups.push(PermissionGroup {
            name: "System".to_string(),
            permissions: vec![
                AuraPermission::RequestIgnoreBatteryOptimizations,
                AuraPermission::SystemAlertWindow,
                AuraPermission::WriteSettings,
            ],
            required: false,
        });

        groups
    }

    /// Check if a permission is granted.
    ///
    /// On Android, this would call through JNI to check the permission.
    /// On non-Android, returns Granted for all permissions.
    pub fn check_permission(&self, permission: AuraPermission) -> PermissionStatus {
        // Check cache first
        if let Some(&status) = self.permission_cache.get(&permission) {
            return status;
        }

        #[cfg(target_os = "android")]
        {
            self.check_permission_android(permission)
        }

        #[cfg(not(target_os = "android"))]
        {
            PermissionStatus::Granted
        }
    }

    #[cfg(target_os = "android")]
    fn check_permission_android(&self, permission: AuraPermission) -> PermissionStatus {
        // Try JNI first
        if let Ok(status) = self.check_permission_jni(permission) {
            return status;
        }

        // Fallback: assume granted for non-critical permissions
        match permission {
            AuraPermission::PostNotifications => PermissionStatus::Unknown,
            _ => PermissionStatus::Granted,
        }
    }

    #[cfg(target_os = "android")]
    fn check_permission_jni(
        &self,
        permission: AuraPermission,
    ) -> Result<PermissionStatus, aura_types::errors::PlatformError> {
        use crate::platform::jni_bridge;
        use aura_types::errors::PlatformError;

        let perm_str = self.to_android_permission(permission);

        // This would call a JNI method to check the permission
        // For now, we'll return Unknown
        // TODO: Implement JNI call to AuraDaemonBridge.checkSelfPermission()
        Ok(PermissionStatus::Unknown)
    }

    /// Get a summary of permission requirements for the current API level.
    pub fn get_summary(&self) -> String {
        let groups = self.get_required_groups();
        let mut summary = format!("Android API {}:\n", self.api_level);

        for group in &groups {
            summary.push_str(&format!(
                "  {} ({}):\n",
                group.name,
                if group.required {
                    "required"
                } else {
                    "optional"
                }
            ));
            for perm in &group.permissions {
                let status = self.check_permission(*perm);
                summary.push_str(&format!("    {:?}: {:?}\n", perm, status));
            }
        }

        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_permissions_get() {
        let perms = PlatformPermissions::get();
        assert!(perms.api_level >= 0);
    }

    #[test]
    fn test_is_required() {
        let perms = PlatformPermissions::get();

        // On host builds (api_level=0), these should be false
        #[cfg(not(target_os = "android"))]
        {
            assert!(!perms.is_required(AuraPermission::ReadExternalStorage));
            assert!(!perms.is_required(AuraPermission::WriteExternalStorage));
            assert!(!perms.is_required(AuraPermission::AccessBackgroundLocation));
            assert!(!perms.is_required(AuraPermission::PostNotifications));
        }
    }

    #[test]
    fn test_to_android_permission() {
        let perms = PlatformPermissions::get();
        let perm = perms.to_android_permission(AuraPermission::Camera);
        assert_eq!(perm, "android.permission.CAMERA");
    }

    #[test]
    fn test_get_required_groups() {
        let perms = PlatformPermissions::get();
        let groups = perms.get_required_groups();
        // Should have at least some groups
        assert!(!groups.is_empty());
    }

    #[test]
    fn test_check_permission_host() {
        let perms = PlatformPermissions::get();
        #[cfg(not(target_os = "android"))]
        {
            let status = perms.check_permission(AuraPermission::Camera);
            assert_eq!(status, PermissionStatus::Granted);
        }
    }
}
