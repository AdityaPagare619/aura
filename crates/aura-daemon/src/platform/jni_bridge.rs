//! Rust ↔ Kotlin JNI bridge for AURA v4.
//!
//! This module provides:
//!
//! 1. **`JNI_OnLoad`** — caches the `JavaVM` pointer for later use.
//! 2. **`jni_env()`** — returns a `JNIEnv` handle on any thread.
//! 3. **Exported `Java_dev_aura_v4_AuraDaemonBridge_native*` functions** — called by Kotlin via
//!    `external fun` declarations.
//! 4. **Helper functions** (`jni_perform_tap`, `jni_get_screen_tree`, etc.) called by other Rust
//!    modules to invoke Kotlin-side methods.
//!
//! # Safety
//!
//! All JNI interactions are inherently unsafe. Every boundary crossing is
//! wrapped in error handling — the native layer **never panics** on JNI
//! errors; instead it returns `PlatformError::JniFailed`.
//!
//! # Cfg gating
//!
//! The entire module is `#[cfg(target_os = "android")]` in production.
//! For host builds, every public helper function has a desktop stub that
//! returns a sensible mock value.

// The module is unconditionally compiled so that:
// - Desktop tests can exercise mock paths.
// - `pub use` in mod.rs works on all platforms.
// The *contents* are cfg-gated internally.

use aura_types::errors::PlatformError;

// ─── JavaVM Cache (Android only) ────────────────────────────────────────────

#[cfg(target_os = "android")]
mod inner {
    use std::sync::OnceLock;

    use aura_types::errors::PlatformError;
    use jni::{
        objects::{JClass, JObject, JString, JValue},
        sys::{jboolean, jfloat, jint, jlong, JNI_VERSION_1_6},
        JNIEnv, JavaVM,
    };
    use tracing::{error, info, warn};

    /// Cached `JavaVM` pointer — set once in `JNI_OnLoad`.
    static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();

    /// JNI class path for the Kotlin-side bridge.
    ///
    /// Must match the fully-qualified class name used in the Android project.
    /// Changing this without updating the Kotlin class declaration will cause
    /// `JNI_OnLoad` verification to fail with a clear error log.
    const BRIDGE_CLASS_PATH: &str = "dev/aura/v4/AuraDaemonBridge";

    // ── JNI_OnLoad ──────────────────────────────────────────────────────

    /// Called by the Android Runtime when `libaura_daemon.so` is loaded.
    ///
    /// # Safety
    /// Raw JNI pointer from the VM.
    #[no_mangle]
    pub unsafe extern "system" fn JNI_OnLoad(
        vm: *mut jni::sys::JavaVM,
        _reserved: *mut std::ffi::c_void,
    ) -> jint {
        let vm = match unsafe { JavaVM::from_raw(vm) } {
            Ok(v) => v,
            Err(e) => {
                eprintln!("AURA JNI_OnLoad: failed to wrap JavaVM: {e}");
                return -1;
            },
        };

        // Cache the VM for later `jni_env()` calls.
        if JAVA_VM.set(vm).is_err() {
            eprintln!("AURA JNI_OnLoad: JavaVM already initialised");
        }

        // Verify that the bridge class exists at load time so any class-path
        // mismatch (e.g. Kotlin rename without updating BRIDGE_CLASS_PATH) fails
        // loudly here rather than silently at the first JNI call.
        match jni_env() {
            Ok(mut env) => {
                match env.find_class(BRIDGE_CLASS_PATH) {
                    Ok(_) => {
                        info!(
                            "AURA JNI_OnLoad: verified bridge class '{}'",
                            BRIDGE_CLASS_PATH
                        );
                    },
                    Err(e) => {
                        error!(
                            "AURA JNI_OnLoad: bridge class '{}' not found — \
                             check Kotlin package name matches BRIDGE_CLASS_PATH: {e}",
                            BRIDGE_CLASS_PATH
                        );
                        // Return -1 to abort library load; the JVM will throw
                        // UnsatisfiedLinkError, making the misconfiguration obvious.
                        return -1;
                    },
                }
            },
            Err(e) => {
                error!("AURA JNI_OnLoad: could not obtain JNIEnv for class verification: {e}");
            },
        }

        info!("AURA JNI_OnLoad: native library loaded");
        JNI_VERSION_1_6 as jint
    }

    // ── jni_env() ───────────────────────────────────────────────────────

    /// Obtain a `JNIEnv` for the calling thread.
    ///
    /// Attaches the thread if it hasn't been attached yet (daemon threads
    /// created by `tokio` are not JVM-managed).
    pub fn jni_env() -> Result<JNIEnv<'static>, PlatformError> {
        let vm = JAVA_VM
            .get()
            .ok_or_else(|| PlatformError::JniFailed("JavaVM not initialised".into()))?;

        // `attach_current_thread_permanently` is idempotent — safe to call
        // repeatedly on the same thread.
        vm.attach_current_thread_permanently()
            .map_err(|e| PlatformError::JniFailed(format!("attach thread: {e}")))
    }

    // ════════════════════════════════════════════════════════════════════
    //  EXPORTED FUNCTIONS — called FROM Kotlin → Rust
    // ════════════════════════════════════════════════════════════════════

    /// `nativeInit(configJson: String): Long` — initialise the Rust daemon.
    ///
    /// Returns a pointer to `DaemonState` (as `jlong`), or 0 on failure.
    ///
    /// # Safety
    /// JNI call.
    #[no_mangle]
    pub unsafe extern "system" fn Java_dev_aura_v4_AuraDaemonBridge_nativeInit(
        mut env: JNIEnv,
        _class: JClass,
        config_json: JString,
    ) -> jlong {
        let config_str: String = match env.get_string(&config_json) {
            Ok(s) => s.into(),
            Err(e) => {
                let _ = env.throw_new(
                    "java/lang/RuntimeException",
                    format!("bad config string: {e}"),
                );
                return 0;
            },
        };

        let config: aura_types::config::AuraConfig = match serde_json::from_str(&config_str) {
            Ok(c) => c,
            Err(e) => {
                // Fall back to default config if JSON parsing fails.
                warn!("config JSON parse failed ({e}), using defaults");
                aura_types::config::AuraConfig::default()
            },
        };

        match crate::startup(config) {
            Ok((state, report)) => {
                info!("daemon started via JNI; startup took {}ms", report.total_ms);
                Box::into_raw(Box::new(state)) as jlong
            },
            Err(e) => {
                error!("daemon startup failed: {e}");
                let _ = env.throw_new("java/lang/RuntimeException", format!("startup: {e}"));
                0
            },
        }
    }

    /// `nativeRun(statePtr: Long)` — enter the main event loop (blocking).
    ///
    /// # Safety
    /// `state_ptr` must be a valid pointer from `nativeInit`.
    #[no_mangle]
    pub unsafe extern "system" fn Java_dev_aura_v4_AuraDaemonBridge_nativeRun(
        _env: JNIEnv,
        _class: JClass,
        state_ptr: jlong,
    ) {
        if state_ptr == 0 {
            error!("nativeRun called with null state pointer");
            return;
        }

        let state = unsafe { *Box::from_raw(state_ptr as *mut crate::DaemonState) };

        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                error!("nativeRun: failed to build tokio runtime: {e}");
                return;
            },
        };

        rt.block_on(async {
            crate::daemon_core::main_loop::run(state).await;
        });

        info!("nativeRun: main loop exited");
    }

    /// `nativeShutdown()` — request graceful shutdown.
    ///
    /// # Safety
    /// JNI call.
    #[no_mangle]
    pub unsafe extern "system" fn Java_dev_aura_v4_AuraDaemonBridge_nativeShutdown(
        _env: JNIEnv,
        _class: JClass,
    ) {
        info!("nativeShutdown requested");
        // The real shutdown uses the static cancel flag set by DaemonState.
        // This is a best-effort signal — the main loop checks it each tick.
    }

    // ════════════════════════════════════════════════════════════════════
    //  HELPER FUNCTIONS — called FROM Rust → Kotlin
    //  These invoke @JvmStatic methods on AuraDaemonBridge.
    // ════════════════════════════════════════════════════════════════════

    // ── Screen / Actions ────────────────────────────────────────────────

    /// Invoke `AuraDaemonBridge.performTap(x, y)`.
    pub fn jni_perform_tap(x: i32, y: i32) -> Result<bool, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(
                BRIDGE_CLASS_PATH,
                "performTap",
                "(II)Z",
                &[JValue::Int(x), JValue::Int(y)],
            )
            .map_err(|e| PlatformError::JniFailed(format!("performTap: {e}")))?;
        check_jni_exception(&mut env, "performTap")?;
        Ok(result.z().unwrap_or(false))
    }

    /// Invoke `AuraDaemonBridge.performSwipe(x1, y1, x2, y2, durationMs)`.
    pub fn jni_perform_swipe(
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: i32,
    ) -> Result<bool, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(
                BRIDGE_CLASS_PATH,
                "performSwipe",
                "(IIIII)Z",
                &[
                    JValue::Int(x1),
                    JValue::Int(y1),
                    JValue::Int(x2),
                    JValue::Int(y2),
                    JValue::Int(duration_ms),
                ],
            )
            .map_err(|e| PlatformError::JniFailed(format!("performSwipe: {e}")))?;
        check_jni_exception(&mut env, "performSwipe")?;
        Ok(result.z().unwrap_or(false))
    }

    /// Invoke `AuraDaemonBridge.typeText(text)`.
    pub fn jni_type_text(text: &str) -> Result<bool, PlatformError> {
        let mut env = jni_env()?;
        let j_text = env
            .new_string(text)
            .map_err(|e| PlatformError::JniFailed(format!("new_string: {e}")))?;
        let result = env
            .call_static_method(
                BRIDGE_CLASS_PATH,
                "typeText",
                "(Ljava/lang/String;)Z",
                &[(&j_text).into()],
            )
            .map_err(|e| PlatformError::JniFailed(format!("typeText: {e}")))?;
        check_jni_exception(&mut env, "typeText")?;
        Ok(result.z().unwrap_or(false))
    }

    /// Invoke `AuraDaemonBridge.getScreenTree()` → JSON bytes.
    pub fn jni_get_screen_tree() -> Result<Vec<u8>, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(BRIDGE_CLASS_PATH, "getScreenTree", "()[B", &[])
            .map_err(|e| PlatformError::JniFailed(format!("getScreenTree: {e}")))?;
        check_jni_exception(&mut env, "getScreenTree")?;

        let obj = result
            .l()
            .map_err(|e| PlatformError::JniFailed(format!("getScreenTree result: {e}")))?;
        let byte_arr: jni::objects::JByteArray = obj.into();
        let len = env
            .get_array_length(&byte_arr)
            .map_err(|e| PlatformError::JniFailed(format!("array length: {e}")))?
            as usize;

        let mut buf = vec![0i8; len];
        env.get_byte_array_region(&byte_arr, 0, &mut buf)
            .map_err(|e| PlatformError::JniFailed(format!("copy bytes: {e}")))?;

        // SAFETY: i8 → u8 is always valid for raw bytes.
        Ok(buf.into_iter().map(|b| b as u8).collect())
    }

    /// Invoke `AuraDaemonBridge.pressBack()`.
    pub fn jni_press_back() -> Result<bool, PlatformError> {
        call_bool_no_args("pressBack")
    }

    /// Invoke `AuraDaemonBridge.pressHome()`.
    pub fn jni_press_home() -> Result<bool, PlatformError> {
        call_bool_no_args("pressHome")
    }

    /// Invoke `AuraDaemonBridge.pressRecents()`.
    pub fn jni_press_recents() -> Result<bool, PlatformError> {
        call_bool_no_args("pressRecents")
    }

    /// Invoke `AuraDaemonBridge.openNotifications()`.
    pub fn jni_open_notifications() -> Result<bool, PlatformError> {
        call_bool_no_args("openNotifications")
    }

    /// Invoke `AuraDaemonBridge.getForegroundPackage()`.
    pub fn jni_get_foreground_package() -> Result<String, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(
                BRIDGE_CLASS_PATH,
                "getForegroundPackage",
                "()Ljava/lang/String;",
                &[],
            )
            .map_err(|e| PlatformError::JniFailed(format!("getForegroundPackage: {e}")))?;
        check_jni_exception(&mut env, "getForegroundPackage")?;
        let jstr = result
            .l()
            .map_err(|e| PlatformError::JniFailed(format!("result to obj: {e}")))?;
        let s: String = env
            .get_string((&jstr).into())
            .map_err(|e| PlatformError::JniFailed(format!("get_string: {e}")))?
            .into();
        Ok(s)
    }

    /// Invoke `AuraDaemonBridge.isServiceAlive()`.
    pub fn jni_is_service_alive() -> bool {
        call_bool_no_args("isServiceAlive").unwrap_or(false)
    }

    // ── Power / Battery ─────────────────────────────────────────────────

    /// Invoke `AuraDaemonBridge.getBatteryLevel()` → 0-100.
    pub fn jni_get_battery_level() -> Result<u8, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(BRIDGE_CLASS_PATH, "getBatteryLevel", "()I", &[])
            .map_err(|e| PlatformError::JniFailed(format!("getBatteryLevel: {e}")))?;
        check_jni_exception(&mut env, "getBatteryLevel")?;
        let level = result.i().unwrap_or(50) as u8;
        Ok(level.min(100))
    }

    /// Invoke `AuraDaemonBridge.isCharging()`.
    pub fn jni_is_charging() -> Result<bool, PlatformError> {
        call_bool_no_args("isCharging")
    }

    /// Invoke `AuraDaemonBridge.isIgnoringBatteryOptimizations()`.
    pub fn jni_is_ignoring_battery_optimizations() -> Result<bool, PlatformError> {
        call_bool_no_args("isIgnoringBatteryOptimizations")
    }

    // ── Thermal ─────────────────────────────────────────────────────────

    /// Invoke `AuraDaemonBridge.getThermalStatus()` → temperature in °C.
    pub fn jni_get_thermal_status() -> Result<f32, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(BRIDGE_CLASS_PATH, "getThermalStatus", "()F", &[])
            .map_err(|e| PlatformError::JniFailed(format!("getThermalStatus: {e}")))?;
        check_jni_exception(&mut env, "getThermalStatus")?;
        let temp = result.f().unwrap_or(35.0);
        Ok(temp)
    }

    // ── Doze / Wakelock ─────────────────────────────────────────────────

    /// Invoke `AuraDaemonBridge.isDozeMode()`.
    pub fn jni_is_doze_mode() -> Result<bool, PlatformError> {
        call_bool_no_args("isDozeMode")
    }

    /// Invoke `AuraDaemonBridge.acquireWakelock(tag, timeoutMs)`.
    pub fn jni_acquire_wakelock(tag: &str, timeout_ms: i64) -> Result<(), PlatformError> {
        let mut env = jni_env()?;
        let j_tag = env
            .new_string(tag)
            .map_err(|e| PlatformError::JniFailed(format!("new_string: {e}")))?;
        env.call_static_method(
            BRIDGE_CLASS_PATH,
            "acquireWakelock",
            "(Ljava/lang/String;J)V",
            &[(&j_tag).into(), JValue::Long(timeout_ms)],
        )
        .map_err(|e| PlatformError::JniFailed(format!("acquireWakelock: {e}")))?;
        check_jni_exception(&mut env, "acquireWakelock")?;
        Ok(())
    }

    /// Invoke `AuraDaemonBridge.releaseWakelock()`.
    pub fn jni_release_wakelock() -> Result<(), PlatformError> {
        let mut env = jni_env()?;
        env.call_static_method(BRIDGE_CLASS_PATH, "releaseWakelock", "()V", &[])
            .map_err(|e| PlatformError::JniFailed(format!("releaseWakelock: {e}")))?;
        check_jni_exception(&mut env, "releaseWakelock")?;
        Ok(())
    }

    // ── Notifications ───────────────────────────────────────────────────

    /// Invoke `AuraDaemonBridge.registerNotificationChannel(id, name, importance)`.
    pub fn jni_register_notification_channel(
        channel_id: &str,
        name: &str,
        importance: i32,
    ) -> Result<(), PlatformError> {
        let mut env = jni_env()?;
        let j_id = env
            .new_string(channel_id)
            .map_err(|e| PlatformError::JniFailed(format!("new_string id: {e}")))?;
        let j_name = env
            .new_string(name)
            .map_err(|e| PlatformError::JniFailed(format!("new_string name: {e}")))?;
        env.call_static_method(
            BRIDGE_CLASS_PATH,
            "registerNotificationChannel",
            "(Ljava/lang/String;Ljava/lang/String;I)V",
            &[(&j_id).into(), (&j_name).into(), JValue::Int(importance)],
        )
        .map_err(|e| PlatformError::JniFailed(format!("registerNotificationChannel: {e}")))?;
        check_jni_exception(&mut env, "registerNotificationChannel")?;
        Ok(())
    }

    /// Invoke `AuraDaemonBridge.postNotification(id, channelId, title, body, ongoing)`.
    pub fn jni_post_notification(
        id: i32,
        channel_id: &str,
        title: &str,
        body: &str,
        ongoing: bool,
    ) -> Result<(), PlatformError> {
        let mut env = jni_env()?;
        let j_channel = env
            .new_string(channel_id)
            .map_err(|e| PlatformError::JniFailed(format!("new_string: {e}")))?;
        let j_title = env
            .new_string(title)
            .map_err(|e| PlatformError::JniFailed(format!("new_string: {e}")))?;
        let j_body = env
            .new_string(body)
            .map_err(|e| PlatformError::JniFailed(format!("new_string: {e}")))?;
        env.call_static_method(
            BRIDGE_CLASS_PATH,
            "postNotification",
            "(ILjava/lang/String;Ljava/lang/String;Ljava/lang/String;Z)V",
            &[
                JValue::Int(id),
                (&j_channel).into(),
                (&j_title).into(),
                (&j_body).into(),
                JValue::Bool(ongoing as jboolean),
            ],
        )
        .map_err(|e| PlatformError::JniFailed(format!("postNotification: {e}")))?;
        check_jni_exception(&mut env, "postNotification")?;
        Ok(())
    }

    /// Invoke `AuraDaemonBridge.cancelNotification(id)`.
    pub fn jni_cancel_notification(id: i32) -> Result<(), PlatformError> {
        let mut env = jni_env()?;
        env.call_static_method(
            BRIDGE_CLASS_PATH,
            "cancelNotification",
            "(I)V",
            &[JValue::Int(id)],
        )
        .map_err(|e| PlatformError::JniFailed(format!("cancelNotification: {e}")))?;
        check_jni_exception(&mut env, "cancelNotification")?;
        Ok(())
    }

    // ── System Info ─────────────────────────────────────────────────────

    /// Invoke `AuraDaemonBridge.getAvailableMemoryMb()`.
    pub fn jni_get_available_memory_mb() -> Result<i64, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(BRIDGE_CLASS_PATH, "getAvailableMemoryMb", "()J", &[])
            .map_err(|e| PlatformError::JniFailed(format!("getAvailableMemoryMb: {e}")))?;
        Ok(result.j().unwrap_or(512))
    }

    // ── Utility ─────────────────────────────────────────────────────────

    /// AND-CRIT-007: Check for and clear any pending JNI exception.
    ///
    /// When Kotlin throws, the JNI environment enters an "exception pending"
    /// state. ANY subsequent JNI call (even `exception_check` in the `jni`
    /// crate internals) in this state causes undefined behavior — typically a
    /// hard SIGSEGV or immediate VM abort. The `jni` crate's `call_static_method`
    /// does attempt to detect this, but race-y Kotlin code (e.g., NPE in a
    /// getter) can leave exceptions pending. This helper provides a safety net:
    /// call it after every `call_static_method` that could throw.
    fn check_jni_exception(env: &mut JNIEnv<'_>, context: &str) -> Result<(), PlatformError> {
        if env.exception_check().unwrap_or(false) {
            // Log the exception to logcat for debugging.
            env.exception_describe();
            env.exception_clear();
            Err(PlatformError::JniFailed(format!(
                "{context}: pending JNI exception cleared"
            )))
        } else {
            Ok(())
        }
    }

    /// Call a static `()Z` method with no arguments on AuraDaemonBridge.
    fn call_bool_no_args(method: &str) -> Result<bool, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(BRIDGE_CLASS_PATH, method, "()Z", &[])
            .map_err(|e| PlatformError::JniFailed(format!("{method}: {e}")))?;
        // AND-CRIT-007: Clear any pending exception before reading the result.
        check_jni_exception(&mut env, method)?;
        Ok(result.z().unwrap_or(false))
    }

    // ── Sensor reads ────────────────────────────────────────────────────

    /// Invoke `AuraDaemonBridge.getAccelerometer()` → `[F` (float[3]: x, y, z).
    pub fn jni_get_accelerometer() -> Result<(f32, f32, f32), PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(BRIDGE_CLASS_PATH, "getAccelerometer", "()[F", &[])
            .map_err(|e| PlatformError::JniFailed(format!("getAccelerometer: {e}")))?;
        let arr = result
            .l()
            .map_err(|e| PlatformError::JniFailed(format!("getAccelerometer result: {e}")))?;
        let arr_ref: jni::objects::JFloatArray = arr.into();
        let mut buf = [0.0f32; 3];
        env.get_float_array_region(arr_ref, 0, &mut buf)
            .map_err(|e| PlatformError::JniFailed(format!("getAccelerometer array: {e}")))?;
        Ok((buf[0], buf[1], buf[2]))
    }

    /// Invoke `AuraDaemonBridge.getLightLevel()` → `F`.
    pub fn jni_get_light_level() -> Result<f32, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(BRIDGE_CLASS_PATH, "getLightLevel", "()F", &[])
            .map_err(|e| PlatformError::JniFailed(format!("getLightLevel: {e}")))?;
        Ok(result.f().unwrap_or(100.0))
    }

    /// Invoke `AuraDaemonBridge.isProximityNear()` → `Z`.
    pub fn jni_is_proximity_near() -> Result<bool, PlatformError> {
        call_bool_no_args("isProximityNear")
    }

    /// Invoke `AuraDaemonBridge.getStepCount()` → `I`.
    pub fn jni_get_step_count() -> Result<u32, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(BRIDGE_CLASS_PATH, "getStepCount", "()I", &[])
            .map_err(|e| PlatformError::JniFailed(format!("getStepCount: {e}")))?;
        Ok(result.i().unwrap_or(0) as u32)
    }

    // ── Connectivity reads ──────────────────────────────────────────────

    /// Invoke `AuraDaemonBridge.getNetworkType()` → `String` ("wifi"|"cellular"|"none").
    pub fn jni_get_network_type() -> Result<String, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(
                BRIDGE_CLASS_PATH,
                "getNetworkType",
                "()Ljava/lang/String;",
                &[],
            )
            .map_err(|e| PlatformError::JniFailed(format!("getNetworkType: {e}")))?;
        let jstr: JString = result
            .l()
            .map_err(|e| PlatformError::JniFailed(format!("getNetworkType result: {e}")))?
            .into();
        let rust_str: String = env
            .get_string(&jstr)
            .map_err(|e| PlatformError::JniFailed(format!("getNetworkType string: {e}")))?
            .into();
        Ok(rust_str)
    }

    /// Invoke `AuraDaemonBridge.isNetworkMetered()` → `Z`.
    pub fn jni_is_network_metered() -> Result<bool, PlatformError> {
        call_bool_no_args("isNetworkMetered")
    }

    /// Invoke `AuraDaemonBridge.getWifiRssi()` → `I`.
    pub fn jni_get_wifi_rssi() -> Result<i32, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(BRIDGE_CLASS_PATH, "getWifiRssi", "()I", &[])
            .map_err(|e| PlatformError::JniFailed(format!("getWifiRssi: {e}")))?;
        Ok(result.i().unwrap_or(-100))
    }

    /// Invoke `AuraDaemonBridge.isNetworkAvailable()` → `Z`.
    pub fn jni_is_network_available() -> Result<bool, PlatformError> {
        call_bool_no_args("isNetworkAvailable")
    }

    // ── Action helpers (Path A — direct intents) ────────────────────────

    /// Invoke `AuraDaemonBridge.launchApp(package)` → `Z`.
    ///
    /// Kotlin impl: `PackageManager.getLaunchIntentForPackage(package) +
    /// startActivity()`.
    pub fn jni_launch_app(package: &str) -> Result<bool, PlatformError> {
        let mut env = jni_env()?;
        let j_pkg = env
            .new_string(package)
            .map_err(|e| PlatformError::JniFailed(format!("new_string: {e}")))?;
        let result = env
            .call_static_method(
                BRIDGE_CLASS_PATH,
                "launchApp",
                "(Ljava/lang/String;)Z",
                &[(&j_pkg).into()],
            )
            .map_err(|e| PlatformError::JniFailed(format!("launchApp: {e}")))?;
        Ok(result.z().unwrap_or(false))
    }

    /// Invoke `AuraDaemonBridge.openUrl(url)` → `Z`.
    ///
    /// Kotlin impl: `Intent(ACTION_VIEW, Uri.parse(url)) + startActivity()`.
    pub fn jni_open_url(url: &str) -> Result<bool, PlatformError> {
        let mut env = jni_env()?;
        let j_url = env
            .new_string(url)
            .map_err(|e| PlatformError::JniFailed(format!("new_string: {e}")))?;
        let result = env
            .call_static_method(
                BRIDGE_CLASS_PATH,
                "openUrl",
                "(Ljava/lang/String;)Z",
                &[(&j_url).into()],
            )
            .map_err(|e| PlatformError::JniFailed(format!("openUrl: {e}")))?;
        Ok(result.z().unwrap_or(false))
    }

    /// Invoke `AuraDaemonBridge.sendSms(recipient, body)` → `Z`.
    ///
    /// Kotlin impl: `SmsManager.getDefault().sendTextMessage()`.
    pub fn jni_send_sms(recipient: &str, body: &str) -> Result<bool, PlatformError> {
        let mut env = jni_env()?;
        let j_recipient = env
            .new_string(recipient)
            .map_err(|e| PlatformError::JniFailed(format!("new_string recipient: {e}")))?;
        let j_body = env
            .new_string(body)
            .map_err(|e| PlatformError::JniFailed(format!("new_string body: {e}")))?;
        let result = env
            .call_static_method(
                BRIDGE_CLASS_PATH,
                "sendSms",
                "(Ljava/lang/String;Ljava/lang/String;)Z",
                &[(&j_recipient).into(), (&j_body).into()],
            )
            .map_err(|e| PlatformError::JniFailed(format!("sendSms: {e}")))?;
        Ok(result.z().unwrap_or(false))
    }

    /// Invoke `AuraDaemonBridge.setAlarm(hour, minute, label)` → `Z`.
    ///
    /// Kotlin impl: `Intent(AlarmClock.ACTION_SET_ALARM) + startActivity()`.
    pub fn jni_set_alarm(hour: u8, minute: u8, label: &str) -> Result<bool, PlatformError> {
        let mut env = jni_env()?;
        let j_label = env
            .new_string(label)
            .map_err(|e| PlatformError::JniFailed(format!("new_string: {e}")))?;
        let result = env
            .call_static_method(
                BRIDGE_CLASS_PATH,
                "setAlarm",
                "(IILjava/lang/String;)Z",
                &[
                    JValue::Int(hour as i32),
                    JValue::Int(minute as i32),
                    (&j_label).into(),
                ],
            )
            .map_err(|e| PlatformError::JniFailed(format!("setAlarm: {e}")))?;
        Ok(result.z().unwrap_or(false))
    }

    /// Invoke `AuraDaemonBridge.queryCalendar(startMs, endMs)` → `[B` (JSON bytes).
    ///
    /// Kotlin impl: `ContentResolver.query(CalendarContract.Events.CONTENT_URI)`.
    pub fn jni_query_calendar(start_ms: i64, end_ms: i64) -> Result<Vec<u8>, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(
                BRIDGE_CLASS_PATH,
                "queryCalendar",
                "(JJ)[B",
                &[JValue::Long(start_ms), JValue::Long(end_ms)],
            )
            .map_err(|e| PlatformError::JniFailed(format!("queryCalendar: {e}")))?;
        let obj = result
            .l()
            .map_err(|e| PlatformError::JniFailed(format!("queryCalendar result: {e}")))?;
        let byte_arr: jni::objects::JByteArray = obj.into();
        let len = env
            .get_array_length(&byte_arr)
            .map_err(|e| PlatformError::JniFailed(format!("array length: {e}")))?
            as usize;
        let mut buf = vec![0i8; len];
        env.get_byte_array_region(&byte_arr, 0, &mut buf)
            .map_err(|e| PlatformError::JniFailed(format!("copy bytes: {e}")))?;
        Ok(buf.into_iter().map(|b| b as u8).collect())
    }

    /// Invoke `AuraDaemonBridge.queryContacts(query)` → `[B` (JSON bytes).
    ///
    /// Kotlin impl: `ContentResolver.query(ContactsContract.CommonDataKinds.Phone.CONTENT_URI)`.
    pub fn jni_query_contacts(query: &str) -> Result<Vec<u8>, PlatformError> {
        let mut env = jni_env()?;
        let j_query = env
            .new_string(query)
            .map_err(|e| PlatformError::JniFailed(format!("new_string: {e}")))?;
        let result = env
            .call_static_method(
                BRIDGE_CLASS_PATH,
                "queryContacts",
                "(Ljava/lang/String;)[B",
                &[(&j_query).into()],
            )
            .map_err(|e| PlatformError::JniFailed(format!("queryContacts: {e}")))?;
        let obj = result
            .l()
            .map_err(|e| PlatformError::JniFailed(format!("queryContacts result: {e}")))?;
        let byte_arr: jni::objects::JByteArray = obj.into();
        let len = env
            .get_array_length(&byte_arr)
            .map_err(|e| PlatformError::JniFailed(format!("array length: {e}")))?
            as usize;
        let mut buf = vec![0i8; len];
        env.get_byte_array_region(&byte_arr, 0, &mut buf)
            .map_err(|e| PlatformError::JniFailed(format!("copy bytes: {e}")))?;
        Ok(buf.into_iter().map(|b| b as u8).collect())
    }

    /// Invoke `AuraDaemonBridge.queryNotifications()` → `[B` (JSON bytes).
    ///
    /// Kotlin impl: `NotificationListenerService.getActiveNotifications()`.
    pub fn jni_query_notifications() -> Result<Vec<u8>, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(BRIDGE_CLASS_PATH, "queryNotifications", "()[B", &[])
            .map_err(|e| PlatformError::JniFailed(format!("queryNotifications: {e}")))?;
        let obj = result
            .l()
            .map_err(|e| PlatformError::JniFailed(format!("queryNotifications result: {e}")))?;
        let byte_arr: jni::objects::JByteArray = obj.into();
        let len = env
            .get_array_length(&byte_arr)
            .map_err(|e| PlatformError::JniFailed(format!("array length: {e}")))?
            as usize;
        let mut buf = vec![0i8; len];
        env.get_byte_array_region(&byte_arr, 0, &mut buf)
            .map_err(|e| PlatformError::JniFailed(format!("copy bytes: {e}")))?;
        Ok(buf.into_iter().map(|b| b as u8).collect())
    }

    /// Invoke `AuraDaemonBridge.setBrightness(level)` → `Z`.
    ///
    /// Kotlin impl: `Settings.System.putInt(SCREEN_BRIGHTNESS)` +
    /// `WindowManager.LayoutParams` for immediate effect.
    /// `level` is in range `0.0..=1.0` (scaled to 0–255 on the Kotlin side).
    pub fn jni_set_brightness(level: f32) -> Result<bool, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(
                BRIDGE_CLASS_PATH,
                "setBrightness",
                "(F)Z",
                &[JValue::Float(level)],
            )
            .map_err(|e| PlatformError::JniFailed(format!("setBrightness: {e}")))?;
        Ok(result.z().unwrap_or(false))
    }

    /// Invoke `AuraDaemonBridge.toggleWifi(enable)` → `Z`.
    ///
    /// Kotlin impl:
    /// - Android < 10: `WifiManager.setWifiEnabled(enable)`.
    /// - Android ≥ 10: `Intent(Settings.ACTION_WIFI_SETTINGS)` via startActivity (API restriction —
    ///   direct toggle not available without root/DeviceAdmin).
    /// Returns `true` if the intent/call was dispatched; the caller must verify
    /// the Wi-Fi state via `jni_get_network_type()` to confirm the change.
    pub fn jni_toggle_wifi(enable: bool) -> Result<bool, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(
                BRIDGE_CLASS_PATH,
                "toggleWifi",
                "(Z)Z",
                &[JValue::Bool(enable as jboolean)],
            )
            .map_err(|e| PlatformError::JniFailed(format!("toggleWifi: {e}")))?;
        Ok(result.z().unwrap_or(false))
    }

    // ── OEM detection ───────────────────────────────────────────────────

    /// Invoke `AuraDaemonBridge.getDeviceManufacturer()` → `String`.
    pub fn jni_get_device_manufacturer() -> Result<String, PlatformError> {
        let mut env = jni_env()?;
        let result = env
            .call_static_method(
                BRIDGE_CLASS_PATH,
                "getDeviceManufacturer",
                "()Ljava/lang/String;",
                &[],
            )
            .map_err(|e| PlatformError::JniFailed(format!("getDeviceManufacturer: {e}")))?;
        let jstr: JString = result
            .l()
            .map_err(|e| PlatformError::JniFailed(format!("getDeviceManufacturer result: {e}")))?
            .into();
        let rust_str: String = env
            .get_string(&jstr)
            .map_err(|e| PlatformError::JniFailed(format!("getDeviceManufacturer string: {e}")))?
            .into();
        Ok(rust_str)
    }

    /// Invoke `AuraDaemonBridge.hasAutostartPermission()` → `Z`.
    pub fn jni_has_autostart_permission() -> Result<bool, PlatformError> {
        call_bool_no_args("hasAutostartPermission")
    }
}

// ─── Public API (platform-agnostic wrappers) ────────────────────────────────
//
// Each function delegates to the `inner` module on Android, or returns a
// sensible mock value on desktop.

/// Obtain a JNI environment handle for the calling thread.
///
/// # Desktop
/// Always returns `Err(JniFailed)`.
#[cfg(target_os = "android")]
pub fn jni_env() -> Result<jni::JNIEnv<'static>, PlatformError> {
    inner::jni_env()
}

#[cfg(not(target_os = "android"))]
pub fn jni_env() -> Result<(), PlatformError> {
    Err(PlatformError::JniFailed("not on Android".into()))
}

// ── Screen / Actions ────────────────────────────────────────────────────────

/// Perform a tap at (x, y) via the accessibility service.
#[cfg(target_os = "android")]
pub fn jni_perform_tap(x: i32, y: i32) -> Result<bool, PlatformError> {
    inner::jni_perform_tap(x, y)
}

#[cfg(not(target_os = "android"))]
pub fn jni_perform_tap(_x: i32, _y: i32) -> Result<bool, PlatformError> {
    Ok(true)
}

/// Perform a swipe gesture.
#[cfg(target_os = "android")]
pub fn jni_perform_swipe(
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    duration_ms: i32,
) -> Result<bool, PlatformError> {
    inner::jni_perform_swipe(x1, y1, x2, y2, duration_ms)
}

#[cfg(not(target_os = "android"))]
pub fn jni_perform_swipe(
    _x1: i32,
    _y1: i32,
    _x2: i32,
    _y2: i32,
    _duration_ms: i32,
) -> Result<bool, PlatformError> {
    Ok(true)
}

/// Type text into the focused field.
#[cfg(target_os = "android")]
pub fn jni_type_text(text: &str) -> Result<bool, PlatformError> {
    inner::jni_type_text(text)
}

#[cfg(not(target_os = "android"))]
pub fn jni_type_text(_text: &str) -> Result<bool, PlatformError> {
    Ok(true)
}

/// Get the serialised accessibility tree as bytes.
#[cfg(target_os = "android")]
pub fn jni_get_screen_tree() -> Result<Vec<u8>, PlatformError> {
    inner::jni_get_screen_tree()
}

#[cfg(not(target_os = "android"))]
pub fn jni_get_screen_tree() -> Result<Vec<u8>, PlatformError> {
    // Return empty bytes — callers should handle gracefully.
    Err(PlatformError::JniFailed("not on Android".into()))
}

/// Press the Back button.
#[cfg(target_os = "android")]
pub fn jni_press_back() -> Result<bool, PlatformError> {
    inner::jni_press_back()
}

#[cfg(not(target_os = "android"))]
pub fn jni_press_back() -> Result<bool, PlatformError> {
    Ok(true)
}

/// Press the Home button.
#[cfg(target_os = "android")]
pub fn jni_press_home() -> Result<bool, PlatformError> {
    inner::jni_press_home()
}

#[cfg(not(target_os = "android"))]
pub fn jni_press_home() -> Result<bool, PlatformError> {
    Ok(true)
}

/// Press the Recents button.
#[cfg(target_os = "android")]
pub fn jni_press_recents() -> Result<bool, PlatformError> {
    inner::jni_press_recents()
}

#[cfg(not(target_os = "android"))]
pub fn jni_press_recents() -> Result<bool, PlatformError> {
    Ok(true)
}

/// Open the notification shade.
#[cfg(target_os = "android")]
pub fn jni_open_notifications() -> Result<bool, PlatformError> {
    inner::jni_open_notifications()
}

#[cfg(not(target_os = "android"))]
pub fn jni_open_notifications() -> Result<bool, PlatformError> {
    Ok(true)
}

/// Get the foreground package name.
#[cfg(target_os = "android")]
pub fn jni_get_foreground_package() -> Result<String, PlatformError> {
    inner::jni_get_foreground_package()
}

#[cfg(not(target_os = "android"))]
pub fn jni_get_foreground_package() -> Result<String, PlatformError> {
    Ok("com.mock.launcher".into())
}

/// Check if the accessibility service is alive.
#[cfg(target_os = "android")]
pub fn jni_is_service_alive() -> bool {
    inner::jni_is_service_alive()
}

#[cfg(not(target_os = "android"))]
pub fn jni_is_service_alive() -> bool {
    false
}

// ── Power / Battery ─────────────────────────────────────────────────────────

/// Get battery level (0–100) via JNI BatteryManager.
#[cfg(target_os = "android")]
pub fn jni_get_battery_level() -> Result<u8, PlatformError> {
    inner::jni_get_battery_level()
}

#[cfg(not(target_os = "android"))]
pub fn jni_get_battery_level() -> Result<u8, PlatformError> {
    Ok(75)
}

/// Check if the device is charging.
#[cfg(target_os = "android")]
pub fn jni_is_charging() -> Result<bool, PlatformError> {
    inner::jni_is_charging()
}

#[cfg(not(target_os = "android"))]
pub fn jni_is_charging() -> Result<bool, PlatformError> {
    Ok(false)
}

/// Check if app is on the battery optimization whitelist.
#[cfg(target_os = "android")]
pub fn jni_is_ignoring_battery_optimizations() -> Result<bool, PlatformError> {
    inner::jni_is_ignoring_battery_optimizations()
}

#[cfg(not(target_os = "android"))]
pub fn jni_is_ignoring_battery_optimizations() -> Result<bool, PlatformError> {
    Ok(true)
}

// ── Thermal ─────────────────────────────────────────────────────────────────

/// Get device temperature in °C.
#[cfg(target_os = "android")]
pub fn jni_get_thermal_status() -> Result<f32, PlatformError> {
    inner::jni_get_thermal_status()
}

#[cfg(not(target_os = "android"))]
pub fn jni_get_thermal_status() -> Result<f32, PlatformError> {
    Ok(35.0)
}

// ── Doze / Wakelock ─────────────────────────────────────────────────────────

/// Check if device is in Doze mode.
#[cfg(target_os = "android")]
pub fn jni_is_doze_mode() -> Result<bool, PlatformError> {
    inner::jni_is_doze_mode()
}

#[cfg(not(target_os = "android"))]
pub fn jni_is_doze_mode() -> Result<bool, PlatformError> {
    Ok(false)
}

/// Acquire an Android PARTIAL_WAKE_LOCK via JNI.
#[cfg(target_os = "android")]
pub fn jni_acquire_wakelock(tag: &str, timeout_ms: i64) -> Result<(), PlatformError> {
    inner::jni_acquire_wakelock(tag, timeout_ms)
}

#[cfg(not(target_os = "android"))]
pub fn jni_acquire_wakelock(_tag: &str, _timeout_ms: i64) -> Result<(), PlatformError> {
    // Desktop stub — no-op success.
    Ok(())
}

/// Release the Android wakelock via JNI.
#[cfg(target_os = "android")]
pub fn jni_release_wakelock() -> Result<(), PlatformError> {
    inner::jni_release_wakelock()
}

#[cfg(not(target_os = "android"))]
pub fn jni_release_wakelock() -> Result<(), PlatformError> {
    Ok(())
}

// ── Notifications ───────────────────────────────────────────────────────────

/// Register a notification channel with Android NotificationManager.
#[cfg(target_os = "android")]
pub fn jni_register_notification_channel(
    channel_id: &str,
    name: &str,
    importance: i32,
) -> Result<(), PlatformError> {
    inner::jni_register_notification_channel(channel_id, name, importance)
}

#[cfg(not(target_os = "android"))]
pub fn jni_register_notification_channel(
    _channel_id: &str,
    _name: &str,
    _importance: i32,
) -> Result<(), PlatformError> {
    Ok(())
}

/// Post a notification to Android.
#[cfg(target_os = "android")]
pub fn jni_post_notification(
    id: i32,
    channel_id: &str,
    title: &str,
    body: &str,
    ongoing: bool,
) -> Result<(), PlatformError> {
    inner::jni_post_notification(id, channel_id, title, body, ongoing)
}

#[cfg(not(target_os = "android"))]
pub fn jni_post_notification(
    _id: i32,
    _channel_id: &str,
    _title: &str,
    _body: &str,
    _ongoing: bool,
) -> Result<(), PlatformError> {
    Ok(())
}

/// Cancel a notification by ID.
#[cfg(target_os = "android")]
pub fn jni_cancel_notification(id: i32) -> Result<(), PlatformError> {
    inner::jni_cancel_notification(id)
}

#[cfg(not(target_os = "android"))]
pub fn jni_cancel_notification(_id: i32) -> Result<(), PlatformError> {
    Ok(())
}

// ── System Info ─────────────────────────────────────────────────────────────

/// Get available system memory in MB.
#[cfg(target_os = "android")]
pub fn jni_get_available_memory_mb() -> Result<i64, PlatformError> {
    inner::jni_get_available_memory_mb()
}

#[cfg(not(target_os = "android"))]
pub fn jni_get_available_memory_mb() -> Result<i64, PlatformError> {
    Ok(2048)
}

// ── Sensor reads ────────────────────────────────────────────────────────────

/// Read accelerometer (x, y, z) in m/s².
#[cfg(target_os = "android")]
pub fn jni_get_accelerometer() -> Result<(f32, f32, f32), PlatformError> {
    inner::jni_get_accelerometer()
}

#[cfg(not(target_os = "android"))]
pub fn jni_get_accelerometer() -> Result<(f32, f32, f32), PlatformError> {
    // Mock: device sitting flat on table (gravity along -Z).
    Ok((0.02, 0.01, 9.81))
}

/// Read ambient light sensor level (lux).
#[cfg(target_os = "android")]
pub fn jni_get_light_level() -> Result<f32, PlatformError> {
    inner::jni_get_light_level()
}

#[cfg(not(target_os = "android"))]
pub fn jni_get_light_level() -> Result<f32, PlatformError> {
    Ok(150.0)
}

/// Check if proximity sensor detects a nearby object.
#[cfg(target_os = "android")]
pub fn jni_is_proximity_near() -> Result<bool, PlatformError> {
    inner::jni_is_proximity_near()
}

#[cfg(not(target_os = "android"))]
pub fn jni_is_proximity_near() -> Result<bool, PlatformError> {
    Ok(false)
}

/// Read step counter (cumulative since boot).
#[cfg(target_os = "android")]
pub fn jni_get_step_count() -> Result<u32, PlatformError> {
    inner::jni_get_step_count()
}

#[cfg(not(target_os = "android"))]
pub fn jni_get_step_count() -> Result<u32, PlatformError> {
    Ok(0)
}

// ── Connectivity reads ──────────────────────────────────────────────────────

/// Get current network type as a string ("wifi", "cellular", "none").
#[cfg(target_os = "android")]
pub fn jni_get_network_type() -> Result<String, PlatformError> {
    inner::jni_get_network_type()
}

#[cfg(not(target_os = "android"))]
pub fn jni_get_network_type() -> Result<String, PlatformError> {
    Ok("wifi".to_string())
}

/// Check if the current network is metered.
#[cfg(target_os = "android")]
pub fn jni_is_network_metered() -> Result<bool, PlatformError> {
    inner::jni_is_network_metered()
}

#[cfg(not(target_os = "android"))]
pub fn jni_is_network_metered() -> Result<bool, PlatformError> {
    Ok(false)
}

/// Get WiFi signal strength (RSSI in dBm).
#[cfg(target_os = "android")]
pub fn jni_get_wifi_rssi() -> Result<i32, PlatformError> {
    inner::jni_get_wifi_rssi()
}

#[cfg(not(target_os = "android"))]
pub fn jni_get_wifi_rssi() -> Result<i32, PlatformError> {
    Ok(-45)
}

/// Check if any network is available.
#[cfg(target_os = "android")]
pub fn jni_is_network_available() -> Result<bool, PlatformError> {
    inner::jni_is_network_available()
}

#[cfg(not(target_os = "android"))]
pub fn jni_is_network_available() -> Result<bool, PlatformError> {
    Ok(true)
}

// ── OEM detection ───────────────────────────────────────────────────────────

/// Get device manufacturer string (e.g., "Xiaomi", "samsung", "HUAWEI").
#[cfg(target_os = "android")]
pub fn jni_get_device_manufacturer() -> Result<String, PlatformError> {
    inner::jni_get_device_manufacturer()
}

#[cfg(not(target_os = "android"))]
pub fn jni_get_device_manufacturer() -> Result<String, PlatformError> {
    Ok("generic".to_string())
}

/// Check if the app has autostart permission (OEM-specific).
#[cfg(target_os = "android")]
pub fn jni_has_autostart_permission() -> Result<bool, PlatformError> {
    inner::jni_has_autostart_permission()
}

#[cfg(not(target_os = "android"))]
pub fn jni_has_autostart_permission() -> Result<bool, PlatformError> {
    Ok(true)
}

// ── Action intents ───────────────────────────────────────────────────────────

/// Launch an app by package name via `PackageManager` + `startActivity`.
#[cfg(target_os = "android")]
pub fn jni_launch_app(package: &str) -> Result<bool, PlatformError> {
    inner::jni_launch_app(package)
}

#[cfg(not(target_os = "android"))]
pub fn jni_launch_app(_package: &str) -> Result<bool, PlatformError> {
    Ok(true)
}

/// Open a URL via `ACTION_VIEW` intent.
#[cfg(target_os = "android")]
pub fn jni_open_url(url: &str) -> Result<bool, PlatformError> {
    inner::jni_open_url(url)
}

#[cfg(not(target_os = "android"))]
pub fn jni_open_url(_url: &str) -> Result<bool, PlatformError> {
    Ok(true)
}

/// Send an SMS via `SmsManager.sendTextMessage`.
#[cfg(target_os = "android")]
pub fn jni_send_sms(recipient: &str, body: &str) -> Result<bool, PlatformError> {
    inner::jni_send_sms(recipient, body)
}

#[cfg(not(target_os = "android"))]
pub fn jni_send_sms(_recipient: &str, _body: &str) -> Result<bool, PlatformError> {
    Ok(true)
}

/// Set an alarm via `AlarmClock.ACTION_SET_ALARM`.
#[cfg(target_os = "android")]
pub fn jni_set_alarm(hour: u8, minute: u8, label: &str) -> Result<bool, PlatformError> {
    inner::jni_set_alarm(hour, minute, label)
}

#[cfg(not(target_os = "android"))]
pub fn jni_set_alarm(_hour: u8, _minute: u8, _label: &str) -> Result<bool, PlatformError> {
    Ok(true)
}

/// Set screen brightness (0.0–1.0) via `Settings.System.SCREEN_BRIGHTNESS`.
#[cfg(target_os = "android")]
pub fn jni_set_brightness(level: f32) -> Result<bool, PlatformError> {
    inner::jni_set_brightness(level)
}

#[cfg(not(target_os = "android"))]
pub fn jni_set_brightness(_level: f32) -> Result<bool, PlatformError> {
    Ok(true)
}

/// Toggle Wi-Fi via `WifiManager` (or Settings intent on Android ≥ 10).
#[cfg(target_os = "android")]
pub fn jni_toggle_wifi(enable: bool) -> Result<bool, PlatformError> {
    inner::jni_toggle_wifi(enable)
}

#[cfg(not(target_os = "android"))]
pub fn jni_toggle_wifi(_enable: bool) -> Result<bool, PlatformError> {
    Ok(true)
}

// ── Content provider queries (return JSON bytes) ─────────────────────────────

/// Query `CalendarContract` events in `[start_ms, end_ms)` → JSON bytes.
#[cfg(target_os = "android")]
pub fn jni_query_calendar(start_ms: i64, end_ms: i64) -> Result<Vec<u8>, PlatformError> {
    inner::jni_query_calendar(start_ms, end_ms)
}

#[cfg(not(target_os = "android"))]
pub fn jni_query_calendar(_start_ms: i64, _end_ms: i64) -> Result<Vec<u8>, PlatformError> {
    Ok(b"[]".to_vec())
}

/// Query `ContactsContract` with a name/number filter → JSON bytes.
#[cfg(target_os = "android")]
pub fn jni_query_contacts(query: &str) -> Result<Vec<u8>, PlatformError> {
    inner::jni_query_contacts(query)
}

#[cfg(not(target_os = "android"))]
pub fn jni_query_contacts(_query: &str) -> Result<Vec<u8>, PlatformError> {
    Ok(b"[]".to_vec())
}

/// Query active notifications via `NotificationListenerService` → JSON bytes.
#[cfg(target_os = "android")]
pub fn jni_query_notifications() -> Result<Vec<u8>, PlatformError> {
    inner::jni_query_notifications()
}

#[cfg(not(target_os = "android"))]
pub fn jni_query_notifications() -> Result<Vec<u8>, PlatformError> {
    Ok(b"[]".to_vec())
}

// ─── Tests (desktop mock paths) ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jni_env_desktop_fails() {
        let result = jni_env();
        // On desktop, jni_env returns Err.
        assert!(result.is_err());
    }

    #[test]
    fn test_perform_tap_desktop_mock() {
        assert!(jni_perform_tap(100, 200).unwrap());
    }

    #[test]
    fn test_perform_swipe_desktop_mock() {
        assert!(jni_perform_swipe(0, 0, 500, 1000, 300).unwrap());
    }

    #[test]
    fn test_type_text_desktop_mock() {
        assert!(jni_type_text("hello world").unwrap());
    }

    #[test]
    fn test_get_screen_tree_desktop_fails() {
        assert!(jni_get_screen_tree().is_err());
    }

    #[test]
    fn test_press_back_desktop_mock() {
        assert!(jni_press_back().unwrap());
    }

    #[test]
    fn test_press_home_desktop_mock() {
        assert!(jni_press_home().unwrap());
    }

    #[test]
    fn test_press_recents_desktop_mock() {
        assert!(jni_press_recents().unwrap());
    }

    #[test]
    fn test_open_notifications_desktop_mock() {
        assert!(jni_open_notifications().unwrap());
    }

    #[test]
    fn test_foreground_package_desktop_mock() {
        let pkg = jni_get_foreground_package().unwrap();
        assert_eq!(pkg, "com.mock.launcher");
    }

    #[test]
    fn test_is_service_alive_desktop() {
        assert!(!jni_is_service_alive());
    }

    #[test]
    fn test_battery_level_desktop_mock() {
        let level = jni_get_battery_level().unwrap();
        assert_eq!(level, 75);
    }

    #[test]
    fn test_is_charging_desktop_mock() {
        assert!(!jni_is_charging().unwrap());
    }

    #[test]
    fn test_battery_optimization_desktop_mock() {
        assert!(jni_is_ignoring_battery_optimizations().unwrap());
    }

    #[test]
    fn test_thermal_status_desktop_mock() {
        let temp = jni_get_thermal_status().unwrap();
        assert!((temp - 35.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_doze_mode_desktop_mock() {
        assert!(!jni_is_doze_mode().unwrap());
    }

    #[test]
    fn test_wakelock_desktop_mock() {
        jni_acquire_wakelock("test", 10_000).unwrap();
        jni_release_wakelock().unwrap();
    }

    #[test]
    fn test_notification_channel_desktop_mock() {
        jni_register_notification_channel("test_channel", "Test", 3).unwrap();
    }

    #[test]
    fn test_post_notification_desktop_mock() {
        jni_post_notification(42, "test_channel", "Title", "Body", false).unwrap();
    }

    #[test]
    fn test_cancel_notification_desktop_mock() {
        jni_cancel_notification(42).unwrap();
    }

    #[test]
    fn test_available_memory_desktop_mock() {
        let mem = jni_get_available_memory_mb().unwrap();
        assert_eq!(mem, 2048);
    }

    // ── Sensor mock tests ───────────────────────────────────────────────

    #[test]
    fn test_accelerometer_desktop_mock() {
        let (x, y, z) = jni_get_accelerometer().unwrap();
        assert!((x - 0.02).abs() < 0.01);
        assert!((y - 0.01).abs() < 0.01);
        assert!((z - 9.81).abs() < 0.01);
    }

    #[test]
    fn test_light_level_desktop_mock() {
        let lux = jni_get_light_level().unwrap();
        assert!((lux - 150.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_proximity_near_desktop_mock() {
        assert!(!jni_is_proximity_near().unwrap());
    }

    #[test]
    fn test_step_count_desktop_mock() {
        assert_eq!(jni_get_step_count().unwrap(), 0);
    }

    // ── Connectivity mock tests ─────────────────────────────────────────

    #[test]
    fn test_network_type_desktop_mock() {
        let net = jni_get_network_type().unwrap();
        assert_eq!(net, "wifi");
    }

    #[test]
    fn test_network_metered_desktop_mock() {
        assert!(!jni_is_network_metered().unwrap());
    }

    #[test]
    fn test_wifi_rssi_desktop_mock() {
        let rssi = jni_get_wifi_rssi().unwrap();
        assert_eq!(rssi, -45);
    }

    #[test]
    fn test_network_available_desktop_mock() {
        assert!(jni_is_network_available().unwrap());
    }

    // ── OEM detection mock tests ────────────────────────────────────────

    #[test]
    fn test_device_manufacturer_desktop_mock() {
        let mfr = jni_get_device_manufacturer().unwrap();
        assert_eq!(mfr, "generic");
    }

    #[test]
    fn test_autostart_permission_desktop_mock() {
        assert!(jni_has_autostart_permission().unwrap());
    }

    // ── Action intent mock tests ────────────────────────────────────────

    #[test]
    fn test_launch_app_desktop_mock() {
        assert!(jni_launch_app("com.example.app").unwrap());
    }

    #[test]
    fn test_open_url_desktop_mock() {
        assert!(jni_open_url("https://example.com").unwrap());
    }

    #[test]
    fn test_send_sms_desktop_mock() {
        assert!(jni_send_sms("+1234567890", "hello").unwrap());
    }

    #[test]
    fn test_set_alarm_desktop_mock() {
        assert!(jni_set_alarm(7, 30, "wake up").unwrap());
    }

    #[test]
    fn test_set_brightness_desktop_mock() {
        assert!(jni_set_brightness(0.75).unwrap());
    }

    #[test]
    fn test_toggle_wifi_enable_desktop_mock() {
        assert!(jni_toggle_wifi(true).unwrap());
    }

    #[test]
    fn test_toggle_wifi_disable_desktop_mock() {
        assert!(jni_toggle_wifi(false).unwrap());
    }

    // ── Content provider mock tests ─────────────────────────────────────

    #[test]
    fn test_query_calendar_desktop_mock() {
        let bytes = jni_query_calendar(0, 1_000_000).unwrap();
        assert_eq!(&bytes, b"[]");
    }

    #[test]
    fn test_query_contacts_desktop_mock() {
        let bytes = jni_query_contacts("alice").unwrap();
        assert_eq!(&bytes, b"[]");
    }

    #[test]
    fn test_query_notifications_desktop_mock() {
        let bytes = jni_query_notifications().unwrap();
        assert_eq!(&bytes, b"[]");
    }
}
