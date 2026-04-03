//! Tests for platform abstraction layer submodules.
//!
//! Covers: path_resolver, cpu, battery, permissions, detect
//! All tests use host stubs — no Android device required.

#[cfg(test)]
mod path_resolver_tests {
    use aura_daemon::platform::path_resolver::{data_dir, db_path, home_dir, model_dir};

    #[test]
    fn test_data_dir_not_empty() {
        let path = data_dir();
        assert!(!path.as_os_str().is_empty(), "data path must not be empty");
    }

    #[test]
    fn test_data_dir_contains_aura_or_android() {
        let path = data_dir();
        #[cfg(not(target_os = "android"))]
        {
            let path_str = path.to_string_lossy();
            assert!(
                path_str.contains("aura"),
                "non-Android data path should contain 'aura', got: {path_str}"
            );
        }
    }

    #[test]
    fn test_model_dir_not_empty() {
        let path = model_dir();
        assert!(!path.as_os_str().is_empty(), "model path must not be empty");
    }

    #[test]
    fn test_model_dir_contains_models() {
        let path = model_dir();
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains("models"),
            "model path should reference models directory, got: {path_str}"
        );
    }

    #[test]
    fn test_db_path_not_empty() {
        let path = db_path();
        assert!(!path.as_os_str().is_empty(), "db path must not be empty");
    }

    #[test]
    fn test_db_path_contains_db() {
        let path = db_path();
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains(".db"),
            "db path should reference a database file, got: {path_str}"
        );
    }

    #[test]
    fn test_home_dir_not_empty() {
        let path = home_dir();
        assert!(!path.as_os_str().is_empty(), "home path must not be empty");
    }
}

#[cfg(test)]
mod cpu_tests {
    use aura_daemon::platform::cpu::PlatformCpuFeatures;

    #[test]
    fn test_cpu_features_detect_not_panic() {
        let features = PlatformCpuFeatures::detect();
        let _ = format!("{features:?}");
    }

    #[test]
    fn test_cpu_features_has_cores() {
        let features = PlatformCpuFeatures::detect();
        assert!(
            features.cpu_cores > 0,
            "cpu_cores must be positive, got: {}",
            features.cpu_cores
        );
    }

    #[test]
    fn test_cpu_features_has_architecture() {
        let features = PlatformCpuFeatures::detect();
        #[cfg(not(target_os = "android"))]
        assert!(
            !features.architecture.is_empty(),
            "architecture should be set on host"
        );
    }

    #[test]
    fn test_cpu_march_flag_not_empty() {
        let features = PlatformCpuFeatures::detect();
        let flag = features.get_march_flag();
        assert!(!flag.is_empty(), "march flag should not be empty");
    }

    #[test]
    fn test_cpu_llama_flags_has_minimum() {
        let features = PlatformCpuFeatures::detect();
        let flags = features.get_llama_flags();
        assert!(
            flags.len() >= 4,
            "should have at least 4 llama flags, got {}",
            flags.len()
        );
    }

    #[test]
    fn test_cpu_summary_not_empty() {
        let features = PlatformCpuFeatures::detect();
        let summary = features.summary();
        assert!(!summary.is_empty(), "summary should not be empty");
    }
}

#[cfg(test)]
mod battery_tests {
    use aura_daemon::platform::battery::BatteryPaths;

    #[test]
    fn test_battery_paths_detect_not_panic() {
        let paths = BatteryPaths::detect();
        let _ = format!("{paths:?}");
    }

    #[test]
    fn test_battery_read_capacity_none_on_host() {
        let paths = BatteryPaths::detect();
        #[cfg(not(target_os = "android"))]
        assert!(
            paths.read_capacity().is_none(),
            "host should not have real battery sysfs"
        );
    }
}

#[cfg(test)]
mod permissions_tests {
    use aura_daemon::platform::permissions::{
        AuraPermission, PermissionStatus, PlatformPermissions,
    };

    #[test]
    fn test_aura_permission_variants() {
        let perms = [
            AuraPermission::ReadExternalStorage,
            AuraPermission::WriteExternalStorage,
            AuraPermission::ReadMediaImages,
            AuraPermission::ReadMediaVideo,
            AuraPermission::ReadMediaAudio,
            AuraPermission::AccessFineLocation,
            AuraPermission::AccessCoarseLocation,
            AuraPermission::AccessBackgroundLocation,
            AuraPermission::Camera,
            AuraPermission::RecordAudio,
            AuraPermission::ReadContacts,
            AuraPermission::WriteContacts,
            AuraPermission::PostNotifications,
            AuraPermission::ForegroundService,
            AuraPermission::SystemAlertWindow,
            AuraPermission::WriteSettings,
        ];
        for p in &perms {
            let _ = format!("{p:?}");
        }
    }

    #[test]
    fn test_platform_permissions_get() {
        let perms = PlatformPermissions::get();
        // Should not panic — permissions manager initialized
        let groups = perms.get_required_groups();
        assert!(
            !groups.is_empty()
                || perms.check_permission(AuraPermission::Camera) == PermissionStatus::Granted
        );
    }

    #[test]
    fn test_permission_status_variants() {
        let statuses = [
            PermissionStatus::Granted,
            PermissionStatus::Denied,
            PermissionStatus::DeniedPermanently,
            PermissionStatus::Unknown,
        ];
        for s in &statuses {
            let _ = format!("{s:?}");
        }
    }

    #[test]
    fn test_android_permission_string() {
        let perms = PlatformPermissions::get();
        let perm = perms.to_android_permission(AuraPermission::Camera);
        assert_eq!(perm, "android.permission.CAMERA");
    }

    #[test]
    fn test_required_groups_nonempty() {
        let perms = PlatformPermissions::get();
        let groups = perms.get_required_groups();
        assert!(
            !groups.is_empty(),
            "should have at least one permission group"
        );
    }

    #[test]
    fn test_check_permission_granted_on_host() {
        let perms = PlatformPermissions::get();
        #[cfg(not(target_os = "android"))]
        {
            let status = perms.check_permission(AuraPermission::Camera);
            assert_eq!(status, PermissionStatus::Granted);
        }
    }
}

#[cfg(test)]
mod detect_tests {
    use aura_daemon::platform::detect::DeviceInfo;

    #[test]
    fn test_device_info_detect() {
        let info = DeviceInfo::detect();
        let _ = format!("{info:?}");
    }

    #[test]
    fn test_device_info_api_level_zero_on_host() {
        let info = DeviceInfo::detect();
        #[cfg(not(target_os = "android"))]
        assert_eq!(info.api_level, 0, "host should have api_level=0");
    }

    #[test]
    fn test_device_info_manufacturer() {
        let info = DeviceInfo::detect();
        #[cfg(not(target_os = "android"))]
        assert_eq!(info.manufacturer, "host");
    }

    #[test]
    fn test_device_info_model() {
        let info = DeviceInfo::detect();
        #[cfg(not(target_os = "android"))]
        assert_eq!(info.model, "development");
    }

    #[test]
    fn test_device_info_not_termux_on_host() {
        let info = DeviceInfo::detect();
        #[cfg(not(target_os = "android"))]
        {
            assert!(!info.is_termux);
            assert!(!info.is_apk);
            assert!(!info.is_rooted);
        }
    }

    #[test]
    fn test_device_info_clone() {
        let info = DeviceInfo::detect();
        let cloned = info.clone();
        assert_eq!(info.api_level, cloned.api_level);
    }

    #[test]
    fn test_device_info_scoped_storage() {
        let info = DeviceInfo::detect();
        #[cfg(not(target_os = "android"))]
        assert!(!info.supports_scoped_storage());
    }

    #[test]
    fn test_device_info_runtime_permissions() {
        let info = DeviceInfo::detect();
        #[cfg(not(target_os = "android"))]
        assert!(!info.supports_runtime_permissions());
    }
}
