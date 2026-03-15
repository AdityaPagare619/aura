# §6 — Android / Mobile Platform Specialist Review
## AURA v4 · Document Control: AURA-v4-AND-2026-006 · v1.0
**Date:** 2026-03-14  
**Reviewer:** Android / Mobile Platform Domain Specialist  
**Scope:** Kotlin Android layer, JNI bridge, AndroidManifest, foreground service, accessibility service, Rust platform crates (power, thermal, doze, sensors, connectivity, jni_bridge), CI Android build  
**Status:** FINAL

---

## Executive Summary

AURA's Android platform layer is architecturally sound and contains several impressive implementations — a physics-based battery model, ISO 13732-1 compliant thermal management, and a comprehensive OEM kill-prevention system. However, the Kotlin integration layer contains 7 critical defects that would cause measurable crashes, data loss, or OS-level termination in production on a real Android device. The CI pipeline cannot build a working Android APK. The overall layer is **NOT READY FOR PRODUCTION**.

**Overall Grade: B- (65/100)**  
Rust platform crates: A- | Kotlin integration: C+ | JNI bridge safety: C | CI Android build: D | API compatibility: C+

| Severity | Count |
|----------|-------|
| CRITICAL  | 7 |
| HIGH      | 9 |
| MEDIUM    | 11 |
| LOW       | 5 |
| **Total** | **32** |

---

## 1. Architecture Claims Verification

| Claim | Location | Status | Notes |
|-------|----------|--------|-------|
| JNI model: `System.loadLibrary("aura_daemon")` | `AuraDaemonBridge.kt` | ✅ CONFIRMED | |
| ARM64-only target | `build.gradle.kts` | ⚠️ PARTIAL | gradle lists arm64-v8a **+ armeabi-v7a + x86_64**; `.cargo/config.toml` only configures `aarch64-linux-android` |
| Foreground service: `START_STICKY` | `AuraForegroundService.kt` | ✅ CONFIRMED | |
| Boot receiver: `BOOT_COMPLETED` + `QUICKBOOT_POWERON` | `BootReceiver.kt` | ✅ CONFIRMED | |
| Heartbeat: 30s normal, 60s low-power | heartbeat loop | ✅ CONFIRMED | |
| Battery thresholds: 5% critical, 20% low | heartbeat loop | ⚠️ MISMATCH | Code: 5%/20%; `monitor.rs:LOW_POWER_BATTERY_THRESHOLD=0.10` (10%) — two different thresholds in use |
| Thermal: 85°C shutdown | `thermal.rs` sysfs junction temp | ✅ CONFIRMED | Skin temp threshold is 43°C — different measurement point, both correct |
| Memory: 300MB warning, 400MB critical | `monitor.rs` | ✅ CONFIRMED | |
| Distribution: Termux `git clone` + `bash install.sh` | `install.sh` | ✅ CONFIRMED | |

---

## 2. Critical Findings

### CRIT-AND-1 — Sensor Listeners Never Unregistered (Resource Leak)
**File:** `AuraDaemonBridge.kt`  
**API Level impact:** All versions  
**Consequence:** Memory leak + battery drain; escalates to `ANR` or `OOM` kill on long sessions

```kotlin
// AuraDaemonBridge.kt
sensorManager.registerListener(this, accelerometer, SensorManager.SENSOR_DELAY_NORMAL)
sensorManager.registerListener(this, gyroscope, SensorManager.SENSOR_DELAY_NORMAL)
// ← No unregisterListener() in onDestroy(), onPause(), or cleanup()
```

Sensor listeners remain registered for the process lifetime. On Android, `SensorManager` holds a strong reference to the listener, preventing GC. Over hours/days of daemon operation this accumulates. More critically, hardware sensor polling at `SENSOR_DELAY_NORMAL` (200ms) continuously drains battery even when AURA is idle.

**Fix:**
```kotlin
override fun onDestroy() {
    super.onDestroy()
    sensorManager.unregisterListener(this)
}
```

---

### CRIT-AND-2 — WakeLock Race Condition
**File:** `AuraDaemonBridge.kt`

```kotlin
// AuraDaemonBridge.kt
@Volatile var managedWakeLock: PowerManager.WakeLock? = null

fun releaseWakelock() {
    if (managedWakeLock?.isHeld == true) {  // ← Check
        managedWakeLock?.release()           // ← Use (not atomic)
    }
}
```

`@Volatile` ensures visibility but not atomicity. Between the `isHeld` check and `release()` call, another thread can null out `managedWakeLock` or call `release()` independently. The result is either a double-release crash (`RuntimeException: WakeLock under-locked`) or a leaked wakelock that holds the CPU awake indefinitely.

**Fix:**
```kotlin
@Synchronized fun releaseWakelock() {
    managedWakeLock?.let { lock ->
        if (lock.isHeld) lock.release()
    }
    managedWakeLock = null
}
```

---

### CRIT-AND-3 — Foreground Service WakeLock Expires (10-minute Timeout Not Renewed)
**File:** `AuraForegroundService.kt`

```kotlin
// AuraForegroundService.kt
wakeLock = powerManager.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "AuraService::WakeLock")
wakeLock.acquire(10 * 60 * 1000L)  // 10 minutes — then expires
// ← No wakeLock.acquire() renewal in the service loop
```

After 10 minutes, the WakeLock expires. Android can then put the CPU to sleep while the daemon is mid-inference. The Rust process does not pause cleanly — it freezes at an arbitrary point (inside `llama_decode`, a syscall, or a Tokio await point). On wake, the daemon may resume in a corrupted state or the IPC connection may be dead.

**Fix:** Either acquire with `0L` (no timeout) for a service that must run indefinitely, or implement WakeLock renewal in the heartbeat loop:
```kotlin
if (!wakeLock.isHeld) wakeLock.acquire(10 * 60 * 1000L)
```

---

### CRIT-AND-4 — Missing Android 14 Foreground Service Type
**File:** `AuraForegroundService.kt`, `AndroidManifest.xml`  
**API Level:** 34+ (Android 14, ~40% of active Android devices as of 2026)

```kotlin
// AuraForegroundService.kt
startForeground(NOTIFICATION_ID, notification)
// ← Missing foregroundServiceType parameter
```

Android 14 (API 34) requires `startForeground()` to specify a `foregroundServiceType` matching the manifest declaration. Without it, `startForeground()` throws `MissingForegroundServiceTypeException` on Android 14+, which crashes the service immediately on launch on ~40% of current Android devices.

**Fix:**
```kotlin
// In AndroidManifest.xml:
<service android:foregroundServiceType="specialUse" ... />

// In AuraForegroundService.kt:
if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
    startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
} else {
    startForeground(NOTIFICATION_ID, notification)
}
```

---

### CRIT-AND-5 — Accessibility Node Recycling Bug (Memory Leak + Crash)
**File:** `AuraAccessibilityService.kt`

```kotlin
// AuraAccessibilityService.kt
fun findNodeByContentDesc(desc: String): AccessibilityNodeInfo? {
    val root = rootInActiveWindow ?: return null
    // ... traversal finds matching nodes ...
    return matchingNode  // ← Intermediate nodes NOT recycled
    // root.recycle() never called; all traversed non-matching nodes leaked
}
```

`AccessibilityNodeInfo` objects are pooled by the Android framework. Callers are required to call `.recycle()` on every node they no longer need. Leaking nodes exhausts the accessibility node pool, causing subsequent queries to fail with null returns (silent degradation) or throwing `IllegalStateException` (crash).

**Fix:**
```kotlin
fun findNodeByContentDesc(desc: String): AccessibilityNodeInfo? {
    val root = rootInActiveWindow ?: return null
    return try {
        root.findAccessibilityNodeInfosByText(desc).firstOrNull()
    } finally {
        root.recycle()
    }
}
```

---

### CRIT-AND-6 — Missing Permissions in AndroidManifest.xml
**File:** `AndroidManifest.xml`

Two permissions used by application code are not declared in the manifest:
- `ACCESS_NETWORK_STATE` — used by `connectivity.rs` / `getWifiRssi()` in `AuraDaemonBridge.kt`
- `ACCESS_WIFI_STATE` — used by `AuraDaemonBridge.kt:getWifiRssi()`

On Android, accessing `WifiManager` or `ConnectivityManager` APIs without the corresponding declared permission throws `SecurityException` at runtime. This crash occurs on every device.

**Fix:** Add to `AndroidManifest.xml`:
```xml
<uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
<uses-permission android:name="android.permission.ACCESS_WIFI_STATE" />
```

---

### CRIT-AND-7 — No JNI Exception Checking After Kotlin Callbacks
**File:** `jni_bridge.rs`

```rust
// jni_bridge.rs — pattern repeated throughout
let result = env.call_method(&callback, "onUpdate", "(Ljava/lang/String;)V", &[...])?;
// ← No env.exception_check() or env.exception_describe() after call
```

When a JNI call into Kotlin throws a Java exception, the JNI environment enters a "pending exception" state. Any subsequent JNI call in this state has undefined behavior — it may crash, silently discard data, or corrupt the JNI frame. The Rust `?` operator propagates `jni::Error` for JNI-level errors but does NOT catch pending Java exceptions.

**Fix:** After every JNI callback:
```rust
if env.exception_check()? {
    env.exception_describe()?;
    env.exception_clear()?;
    return Err(AuraError::JniException("callback threw".into()));
}
```

---

## 3. High Findings

### HIGH-AND-1 — `getThermalStatus()` Uses Battery Intent as Thermal Proxy
**File:** `AuraDaemonBridge.kt`

```kotlin
fun getThermalStatus(): ThermalStatus {
    val batteryStatus = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
    val temp = batteryStatus?.getIntExtra(BatteryManager.EXTRA_TEMPERATURE, 0)?.div(10f)
    // ← Battery temperature, NOT CPU/SoC temperature
```

`BatteryManager.EXTRA_TEMPERATURE` reports **battery cell temperature**, which lags SoC temperature by 5–15 minutes and is typically 10–20°C lower than junction temperature under load. AURA's actual thermal emergency (85°C junction) cannot be reliably detected via battery temperature alone.

**Fix:** Use `PowerManager.getCurrentThermalStatus()` (API 29+) or read `/sys/class/thermal/thermal_zone*/temp` sysfs entries (which `thermal.rs` already does correctly in the Rust layer). The Kotlin layer should delegate to the Rust thermal monitor rather than reimplement with worse data.

---

### HIGH-AND-2 — Deprecated `WifiManager.connectionInfo` API
**File:** `AuraDaemonBridge.kt`

```kotlin
val wifiInfo = wifiManager.connectionInfo  // ← Deprecated API 31, removed API 33+
val rssi = wifiInfo.rssi
```

`WifiManager.getConnectionInfo()` was deprecated in API 31 (Android 12) and its RSSI data was restricted in API 30 (requires `ACCESS_FINE_LOCATION` or returns -1). On API 33+ devices, the method returns stale/empty data. AURA's connectivity-aware behavior (adjusting heartbeat, offloading decisions) will silently receive wrong RSSI values.

**Fix:** Use `WifiManager.registerActiveNetworkCallback()` with `NetworkCapabilities.getSignalStrength()` (API 29+).

---

### HIGH-AND-3 — `waitForElement` Blocks Calling Thread with `Thread.sleep(500)`
**File:** `AuraAccessibilityService.kt`

```kotlin
fun waitForElement(desc: String, timeoutMs: Long = 5000): AccessibilityNodeInfo? {
    val deadline = System.currentTimeMillis() + timeoutMs
    while (System.currentTimeMillis() < deadline) {
        findNodeByContentDesc(desc)?.let { return it }
        Thread.sleep(500)  // ← Blocks Accessibility callback thread
    }
    return null
}
```

The Android Accessibility Service callbacks run on the main thread. `Thread.sleep()` on the main thread blocks all UI interaction and other accessibility events during the 5-second timeout window. This will trigger ANR (Application Not Responding) dialog if the main thread is blocked for more than 5 seconds, which is exactly the configured timeout.

**Fix:** Move the polling to a coroutine:
```kotlin
suspend fun waitForElement(desc: String, timeoutMs: Long = 5000): AccessibilityNodeInfo? {
    val deadline = System.currentTimeMillis() + timeoutMs
    while (System.currentTimeMillis() < deadline) {
        findNodeByContentDesc(desc)?.let { return it }
        delay(500)
    }
    return null
}
```

---

### HIGH-AND-4 — `@Volatile` on Sensor State Without Compound-Read Synchronization
**File:** `AuraDaemonBridge.kt`

```kotlin
@Volatile var accelerometerX: Float = 0f
@Volatile var accelerometerY: Float = 0f
@Volatile var accelerometerZ: Float = 0f
```

Individual fields are `@Volatile` (atomic reads/writes per field). However, any code reading `(x, y, z)` as a unit is reading three separate atomic operations — between the `x` read and the `z` read, a sensor update can arrive. The resulting 3-vector is a mix of two different sensor samples (torn read). For motion detection, this produces phantom motion vectors.

**Fix:** Use a single `@Volatile` reference to an immutable data class:
```kotlin
data class AccelerometerReading(val x: Float, val y: Float, val z: Float)
@Volatile var accelerometerReading: AccelerometerReading = AccelerometerReading(0f, 0f, 0f)
```

---

### HIGH-AND-5 — `armeabi-v7a` and `x86_64` ABIs Listed but Not Built
**File:** `build.gradle.kts` vs `.cargo/config.toml`

`build.gradle.kts` lists three ABIs: `arm64-v8a`, `armeabi-v7a`, `x86_64`. The `.cargo/config.toml` only configures the `aarch64-linux-android` Rust target. The result: the Gradle build will attempt to package a multi-ABI APK, but only `arm64-v8a` will have the native `.so`. The APK will either fail to build (missing native libs) or install on a 32-bit device and crash immediately with `UnsatisfiedLinkError`.

**Fix (path A — ARM64-only):** Remove `armeabi-v7a` and `x86_64` from `abiFilters` in `build.gradle.kts`.
**Fix (path B — multi-ABI):** Add `armv7-linux-androideabi` and `x86_64-linux-android` Rust targets to `.cargo/config.toml` and the CI build matrix.

---

### HIGH-AND-6 — `nativeShutdown()` Called from `onDestroy()` on Main Thread
**File:** `AuraForegroundService.kt`

```kotlin
override fun onDestroy() {
    nativeShutdown()  // ← JNI call; Rust may block on mutex/IO
    super.onDestroy()
}
```

`onDestroy()` runs on the main thread. `nativeShutdown()` calls into Rust, which may need to flush SQLite, cancel async tasks, or join tokio threads. Any of these operations that block for more than ~5 seconds will trigger ANR. The Rust daemon is explicitly designed for long-running operations (inference, consolidation) — requesting shutdown during one of these is likely to block.

**Fix:** Dispatch `nativeShutdown()` to a background thread in `onDestroy()`, with a 3-second timeout:
```kotlin
override fun onDestroy() {
    lifecycleScope.launch(Dispatchers.IO) {
        withTimeout(3000) { nativeShutdown() }
    }
    super.onDestroy()
}
```

---

### HIGH-AND-7 — CI Android Build Pipeline Cannot Produce Working APK
**File:** `.github/workflows/build-android.yml`

The Android CI workflow:
1. Uses `dtolnay/rust-toolchain@stable` but `rust-toolchain.toml` pins `nightly-2026-03-01` — toolchain mismatch causes compile errors
2. Does not run `cargo ndk` with the correct `--target aarch64-linux-android` flag
3. Does not strip debug symbols from the native `.so` before APK packaging (>200MB `.so` in APK)
4. Does not sign the APK (unsigned APK cannot be sideloaded or submitted to Play Store)

The CI pipeline has never produced a functional APK in its current state.

---

### HIGH-AND-8 — Termux Build Takes 10–30 Minutes (Thermal Throttling Risk)
**File:** `install.sh` (noted in external review)

Building AURA natively on Android via Termux requires compiling ~147K lines of Rust plus llama.cpp (C++) on mobile hardware. At sustained 100% CPU, the device will thermal-throttle (AURA itself triggers shutdown at 85°C). The build process may take 10–30 minutes, during which:
- The device must stay plugged in (can't use normally)
- A thermal event mid-build produces a corrupted partial build
- No progress indicator; user sees no output for minutes at a time

**Fix:** Provide pre-built `.so` binaries as GitHub release artifacts. The install script should download the pre-built binary, verify checksum, and only fall back to source build if explicitly requested.

---

### HIGH-AND-9 — Many `system_api.rs` Methods Are Stubs
**File:** `bridge/system_api.rs`

Numerous `execute_*` methods in the system API bridge return hardcoded placeholder values instead of making real Android API calls:
- `execute_get_wifi_networks()` → returns empty `Vec`
- `execute_get_running_apps()` → returns placeholder list
- `execute_get_calendar_events()` → returns stub data
- Others (exact count: ~12 stub methods confirmed)

PolicyGate rules that evaluate system state (e.g., "is the user in a meeting?") will make decisions based on fabricated data. Any integration test that passes using these stubs is not testing real behavior.

---

## 4. Medium Findings

### MED-AND-1 — Battery Threshold Mismatch
`heartbeat.rs`: `LOW_POWER_THRESHOLD = 0.20` (20%)  
`monitor.rs`: `LOW_POWER_BATTERY_THRESHOLD = 0.10` (10%)

Two separate components apply different low-power triggers. The behavior when battery is between 10% and 20% is undefined — one component throttles, the other does not.

---

### MED-AND-2 — Thermal Measurement Source Inconsistency
The Kotlin layer reads battery temperature (`BatteryManager.EXTRA_TEMPERATURE`); the Rust `thermal.rs` reads sysfs junction temperature. These measure different physical locations with different lag characteristics. The thermal coordination between layers is not synchronized.

---

### MED-AND-3 — `BOOT_COMPLETED` Receiver Not Exported in Manifest
Modern Android (API 26+) requires broadcast receivers to be explicitly exported or declared as unexported:
```xml
<receiver android:exported="false" android:name=".BootReceiver">
```
Without `android:exported`, the manifest declaration generates a lint warning and on some OEM ROMs (particularly Xiaomi MIUI) is silently ignored, preventing boot auto-start.

---

### MED-AND-4 — No Notification Channel for Android 8+
**File:** `AuraForegroundService.kt`

Android 8.0 (API 26) requires notification channels. If `createNotificationChannel()` is not called before the first `startForeground()`, the notification is silently dropped. On Android 8+ (99%+ of active devices), the foreground service notification will not appear, and some Android versions will immediately kill a foreground service with no visible notification.

---

### MED-AND-5 — Accessibility Service Requires Manual Enable
`AuraAccessibilityService` requires the user to manually enable it in Settings → Accessibility. There is no setup flow, onboarding screen, or deep link to the accessibility settings page. Users who don't enable it will see no error; accessibility-dependent features will silently not work.

---

### MED-AND-6 — No Graceful Degradation When JNI Library Fails to Load
**File:** `AuraDaemonBridge.kt`

```kotlin
// Static block or init
System.loadLibrary("aura_daemon")
// ← No try/catch; UnsatisfiedLinkError crashes the whole app
```

If the native library fails to load (wrong ABI, missing `.so`, corrupted install), the app crashes immediately with an uncaught `UnsatisfiedLinkError`. No error message is shown to the user.

**Fix:** Wrap in try/catch and show a user-facing error with recovery instructions.

---

### MED-AND-7 — No Foreground Service Restart Logic on Process Kill
`START_STICKY` causes Android to restart the service after process death, but it restarts with a null intent. If the daemon Rust process crashes (e.g., on a panic), Android will attempt to restart the service, but any in-progress task will be lost and the daemon will start in a cold state with no indication to the user.

---

### MED-AND-8 — Doze Mode May Kill Service Before OEM Mitigation Activates
`doze.rs` implements OEM-specific kill prevention for Xiaomi, Samsung, Huawei, OPPO, Vivo, and OnePlus. However, standard Android Doze mode can kill foreground services before the OEM mitigation has a chance to run (OEM mitigation requires a `BroadcastReceiver` that itself may be deferred).

---

### MED-AND-9 — No Battery Optimization Exemption Request
AURA does not request exemption from battery optimization (`ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`). Without exemption, Android will aggressively throttle the foreground service in Doze mode even with `START_STICKY`. The exemption is recommended for apps that need reliable background execution (medical, accessibility, security apps).

---

### MED-AND-10 — Native Build on Termux Requires Root for Some Operations
`install.sh` calls `pkg install` and modifies system paths that may require elevated permissions on stock Android + Termux. The script does not check for permission issues and will fail non-obviously.

---

### MED-AND-11 — No Minimum API Level Declared in Manifest
`AndroidManifest.xml` does not declare `android:minSdkVersion`. Without this, the app appears compatible with all Android versions including pre-API 23 devices that lack required security APIs (Keystore, AES-GCM hardware acceleration, etc.).

---

## 5. Low Findings

### LOW-AND-1 — Hardcoded Package Name
Package name `dev.aura.v4` is hardcoded in multiple Kotlin files. Renaming for distribution requires a find-and-replace across all files.

### LOW-AND-2 — No ProGuard / R8 Configuration
No `proguard-rules.pro` file exists. JNI-called methods may be stripped by R8 in release builds, causing `NoSuchMethodError` in production.

### LOW-AND-3 — Logging in Production Build
`AuraDaemonBridge.kt` and `AuraForegroundService.kt` contain `Log.d()` calls that will appear in `adb logcat` in release builds, potentially leaking internal state.

### LOW-AND-4 — No Network Security Config
`AndroidManifest.xml` does not declare a `network_security_config`. Cleartext HTTP is blocked by default on API 28+ but there is no explicit config to prevent accidental cleartext downgrade.

### LOW-AND-5 — `targetSdkVersion` Not Visible in Review
The `build.gradle.kts` `targetSdkVersion` was not confirmed in the reviewed files. If `targetSdkVersion < 34`, Android 14 foreground service restrictions (CRIT-AND-4) may not apply immediately but will once the target is bumped.

---

## 6. Platform Layer Highlights (Strengths)

The following Rust platform components are notably well-implemented and should be preserved:

| Component | Highlights |
|-----------|-----------|
| `power.rs` | Physics-based energy model: 5000mAh × 3.85V × 0.85η efficiency; 5-tier performance degradation with 3% hysteresis to prevent oscillation |
| `thermal.rs` | ISO 13732-1 skin temperature thresholds (43°C); full PID controller with derivative term; Newton's law cooling simulation; multi-zone monitoring (junction + skin) |
| `doze.rs` | OEM-specific kill prevention for 6 manufacturers (Xiaomi MIUI, Samsung Knox, Huawei HMS, OPPO ColorOS, Vivo FuntouchOS, OnePlus OxygenOS) with version-specific intent handling |
| `sensors.rs` | Reasonable sensor abstraction with power-aware sampling rate adjustment |
| `connectivity.rs` | Network quality scoring with exponential moving average |

The quality gap between the Rust platform layer and the Kotlin integration layer is significant. The Rust code reflects engineering care; the Kotlin code reflects rapid prototyping.

---

## 7. Remediation Priority

| Priority | Finding | Effort | Impact |
|----------|---------|--------|--------|
| P0 (Sprint 0) | CRIT-AND-4: Android 14 crash on 40% of devices | 2 hr | Prevents launch crash |
| P0 (Sprint 0) | CRIT-AND-6: Missing manifest permissions | 30 min | Prevents SecurityException crash |
| P0 (Sprint 0) | CRIT-AND-1: Sensor listener leak | 1 hr | Prevents battery drain + OOM |
| P0 (Sprint 0) | CRIT-AND-3: WakeLock expires at 10 min | 1 hr | Prevents daemon freeze |
| P1 (Sprint 1) | CRIT-AND-7: JNI no exception checking | 4 hr | Prevents silent corruption |
| P1 (Sprint 1) | CRIT-AND-2: WakeLock race condition | 1 hr | Prevents crash/leak |
| P1 (Sprint 1) | CRIT-AND-5: Node recycling bug | 2 hr | Prevents accessibility crash |
| P1 (Sprint 1) | HIGH-AND-1: Wrong thermal API | 2 hr | Correct thermal detection |
| P1 (Sprint 1) | HIGH-AND-2: Deprecated WiFi API | 2 hr | Correct RSSI on API 31+ |
| P1 (Sprint 1) | HIGH-AND-3: Thread.sleep on main thread | 1 hr | Prevents ANR |
| P1 (Sprint 1) | HIGH-AND-5: ABI mismatch | 1 hr | Prevents install failure |
| P2 (Sprint 2) | HIGH-AND-7: CI cannot build APK | 1 day | Enables automated testing |
| P2 (Sprint 2) | HIGH-AND-8: 30-min Termux build | 1 wk | Pre-built binaries |
| P2 (Sprint 2) | HIGH-AND-9: system_api stubs | 2 wks | Real Android integration |

---

## 8. Verdict

**⛔ NOT READY FOR PRODUCTION**

Four of the seven critical defects are day-one crashes on real hardware: Android 14 foreground service type (40% of devices), missing manifest permissions (all devices), sensor leak (all devices over time), and WakeLock expiry (all devices after 10 minutes). None of these are architectural — they are implementation gaps fixable in a combined ~5 engineer-hours.

The Rust platform layer (power, thermal, doze) is production-quality. The Kotlin integration layer needs a focused remediation sprint before any public distribution.

Minimum viable Android state: CRIT-AND-1, CRIT-AND-3, CRIT-AND-4, and CRIT-AND-6 fixed. These four fixes prevent all guaranteed launch-day crashes.
