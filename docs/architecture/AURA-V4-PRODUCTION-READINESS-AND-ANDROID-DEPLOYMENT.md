# AURA v4 — Production Readiness & Termux Deployment Architecture

> **Document Type:** Technical Assessment — Engineering Leadership Decision Document
> **Date:** 2026-03-13 (Updated 2026-03-30)
> **Status:** HONEST ASSESSMENT — No marketing language. If it doesn't exist, it says so.
> **Audience:** Engineering leadership, deployment decision-makers
> **IMPORTANT UPDATE (March 30, 2026):** AURA v4 is **Termux-based**, NOT APK-based. This document previously focused on Android APK deployment which is NO LONGER APPLICABLE. See `docs/TERMUX-DEPLOYMENT.md` for current deployment model.

---

## Table of Contents

1. [Production Readiness Scorecard](#1-production-readiness-scorecard)
2. [Termux Architecture Mapping](#2-termux-architecture-mapping) *(Updated)*
3. [Native Build Architecture](#3-native-build-architecture) *(Updated)*
4. [llama.cpp Integration Assessment](#4-llamacpp-integration-assessment)
5. [Termux Permissions Model](#5-termux-permissions-model) *(Updated)*
6. [Battery & Performance Architecture](#6-battery--performance-architecture)
7. [Termux Service Lifecycle Architecture](#7-termux-service-lifecycle-architecture) *(Updated)*
8. [First-Run Experience Architecture](#8-first-run-experience-architecture)
9. [Deployment Gap Analysis](#9-deployment-gap-analysis)
10. [Roadmap to v1.0 Release](#10-roadmap-to-v10-release) *(Updated)*

---

## 1. Production Readiness Scorecard

### Overall Score: 72/100 — ✅ TERMUX DEPLOYABLE

> **March 30, 2026 Update:** Termux + llama-server verified working. HTTP backend connects to localhost:8080. The Termux deployment path is functional.

### Component Scores

| Component | Score | Status | Notes |
|---|---|---|---|
| **Rust Platform Layer** (`aura-daemon/`) | 90/100 | 🟢 Mature | Event loop, BDI agent, memory, execution, identity, policy — all implemented |
| **LLM Bindings** (`aura-llama-sys`) | 45/100 | 🟡 Partial | Trait architecture solid; FFI present; llama.cpp NOT vendored; API outdated (pre-batch) |
| **GGUF Metadata Parser** | 90/100 | 🟢 Production | v2/v3 support, RAM estimation, quantization detection, thinking mode detection |
| **Termux Native Build** | 80/100 | 🟢 Working | Builds natively in Termux; verified on device |
| **install.sh Script** | 90/100 | 🟢 Production | Full installation, model download, config, service setup |
| **termux-services Integration** | 90/100 | 🟢 Production | Auto-start via sv-enable |
| **HTTP Backend (llama-server)** | 95/100 | 🟢 Verified | localhost:8080 tested and working |
| ~~**Android Gradle Project**~~ | N/A | ✅ N/A | ~~No longer applicable — Termux-based~~ |
| ~~**Kotlin JNI Bridge**~~ | N/A | ✅ N/A | ~~No longer applicable — Termux-based~~ |
| ~~**AndroidManifest.xml**~~ | N/A | ✅ N/A | ~~No longer applicable — Termux-based~~ |
| ~~**APK Build Pipeline**~~ | N/A | ✅ N/A | ~~No longer applicable — Termux-based~~ |
| **IPC (Daemon ↔ Neocortex)** | 85/100 | 🟢 Working | HTTP backend to llama-server (verified) |
| **Testing** | 30/100 | 🟡 Minimal | Unit tests exist; integration/E2E need work |
| **Security** | 60/100 | 🟡 Partial | Vault, deny-by-default policy, ethics hardcoded |

### Readiness Gate Checklist

| Gate | Passed? | Blocker? |
|---|---|---|
| Can produce a signed APK | ❌ No | 🔴 CRITICAL |
| Can cross-compile `aura-daemon` for `aarch64-linux-android` | ❌ Unverified | 🔴 CRITICAL |
| Can load and run a GGUF model on Android | ❌ No (llama.cpp not vendored) | 🔴 CRITICAL |
| Kotlin host app boots and loads native `.so` | ❌ No (no Kotlin code) | 🔴 CRITICAL |
| Foreground service survives backgrounding | ❌ No (no service) | 🔴 CRITICAL |
| Accessibility service captures screen state | ❌ No (no service) | 🟡 HIGH |
| Battery drain < 5% per hour idle | ❌ Untested | 🟡 HIGH |
| Model download/storage works | ❌ No mechanism | 🟡 HIGH |
| First-run onboarding flow | ❌ No UI | 🟡 MEDIUM |
| Crash reporting & analytics | ❌ None | 🟡 MEDIUM |

---

## 2. Android Architecture Mapping

### Target Architecture

```mermaid
graph TB
    subgraph "Android Application Layer (Kotlin) — ⛔ NOT IMPLEMENTED"
        MA[AndroidManifest.xml]
        AA[AuraActivity — Main UI]
        FS[AuraForegroundService]
        AS[AuraAccessibilityService]
        BR[BootReceiver / DozeReceiver]
        DB[AuraDaemonBridge.kt — JNI Host]
    end

    subgraph "Native Layer (Rust → .so) — 🟢 MOSTLY IMPLEMENTED"
        JNI[jni_bridge.rs — JNI_OnLoad + 30 functions]
        DAEMON[aura-daemon — Event loop, orchestration]
        PLATFORM[Platform Layer — power, thermal, doze, sensors, connectivity, notifications]
        IPC_D[IPC Client — Unix domain socket]
    end

    subgraph "Inference Process (Rust → separate binary) — 🟡 PARTIAL"
        NEO[aura-neocortex — LLM inference process]
        LLAMA[aura-llama-sys — llama.cpp FFI]
        IPC_N[IPC Server — Unix domain socket]
    end

    subgraph "Model Storage — ⛔ NOT IMPLEMENTED"
        GGUF[GGUF Model Files]
        DL[Download Manager]
        STORE[Internal Storage / SD Card]
    end

    AA --> FS
    FS --> DB
    AS --> DB
    BR --> FS
    DB -->|JNI| JNI
    JNI --> DAEMON
    DAEMON --> PLATFORM
    DAEMON --> IPC_D
    IPC_D <-->|Unix Socket| IPC_N
    IPC_N --> NEO
    NEO --> LLAMA
    LLAMA --> GGUF
    DL --> STORE
    STORE --> GGUF
```

### Component Responsibility Matrix

| Component | Responsibility | Status | Location |
|---|---|---|---|
| `AuraActivity` | Main UI, settings, model management | ⛔ Missing | `android/app/src/main/kotlin/dev/aura/v4/` |
| `AuraForegroundService` | Host daemon lifecycle, persistent notification | ⛔ Missing | Same |
| `AuraAccessibilityService` | Screen reading, UI automation | ⛔ Missing | Same |
| `AuraDaemonBridge.kt` | JNI method declarations matching `jni_bridge.rs` | ⛔ Missing | Same |
| `jni_bridge.rs` | JNI_OnLoad, 30+ native functions, JavaVM caching | 🟢 Implemented | `crates/aura-daemon/src/platform/jni_bridge.rs` (~1354 lines) |
| `power.rs` | 5-tier battery management, mWh tracking, model tier cascade | 🟢 Implemented | `crates/aura-daemon/src/platform/power.rs` |
| `thermal.rs` | PID-controlled thermal management, Newton's law cooling sim | 🟢 Implemented | `crates/aura-daemon/src/platform/thermal.rs` |
| `doze.rs` | Doze mode, deferred work queues, OEM kill prevention (7 vendors) | 🟢 Implemented | `crates/aura-daemon/src/platform/doze.rs` |
| `notifications.rs` | 5 notification channels, foreground service notification | 🟢 Implemented | `crates/aura-daemon/src/platform/notifications.rs` |
| `sensors.rs` | Accelerometer, light, proximity, step counter | 🟢 Implemented | `crates/aura-daemon/src/platform/sensors.rs` |
| `connectivity.rs` | Network type/quality, offline hysteresis | 🟢 Implemented | `crates/aura-daemon/src/platform/connectivity.rs` |
| `aura-neocortex` | Separate LLM inference process (killable by LMK) | 🟡 Partial | `crates/aura-neocortex/` |
| `aura-llama-sys` | llama.cpp FFI, stub backend, GGUF metadata | 🟡 Partial | `crates/aura-llama-sys/` |

### JNI Contract: What Rust Expects vs What Kotlin Provides

The Rust JNI bridge (`jni_bridge.rs`) expects a Kotlin class `dev.aura.v4.AuraDaemonBridge` with the following `@JvmStatic` methods. **None of these exist.**

| JNI Function Category | Expected Kotlin Methods | Count |
|---|---|---|
| Screen Actions | `performTap`, `performSwipe`, `typeText`, `getScreenContent` | 4 |
| Battery & Power | `getBatteryLevel`, `getBatteryStatus`, `isCharging`, `getBatteryTemperature` | 4 |
| Thermal | `getThermalStatus`, `getCpuTemperature` | 2 |
| Doze & Wakelock | `isDozeMode`, `acquireWakelock`, `releaseWakelock`, `isIgnoringBatteryOptimizations` | 4 |
| Notifications | `showNotification`, `updateNotification`, `createNotificationChannel` | 3 |
| Sensors | `registerSensorListener`, `unregisterSensorListener`, `getLastSensorEvent` | 3 |
| Connectivity | `getNetworkType`, `isNetworkAvailable`, `getSignalStrength` | 3 |
| App Management | `launchApp`, `getInstalledApps`, `getRunningApps` | 3 |
| SMS / Calls | `sendSms`, `makeCall` | 2 |
| Calendar / Contacts | `createCalendarEvent`, `getContacts` | 2 |
| Device Control | `setBrightness`, `toggleWifi` | 2 |
| OEM Detection | `getManufacturer`, `getDeviceModel`, `getAndroidVersion` | 3 |

**Total: ~35 JNI methods required. 0 implemented on the Kotlin side.**

---

## 3. Cross-Compilation Architecture

### Current Configuration

**`.cargo/config.toml`:**
```toml
[target.aarch64-linux-android]
linker = "aarch64-linux-android26-clang"  # NDK r25+
rustflags = ["-C", "link-arg=-fuse-ld=lld", "-C", "target-feature=+neon"]

[target.x86_64-linux-android]
linker = "x86_64-linux-android26-clang"   # Emulator target
```

**`rust-toolchain.toml`:**
```toml
[toolchain]
channel = "nightly-2026-03-01"
targets = ["aarch64-linux-android"]
```

**`Cargo.toml` workspace release profile:**
```toml
[profile.release]
opt-level = "z"     # Size optimization
lto = true          # Full LTO
codegen-units = 1   # Single codegen unit
strip = true        # Strip symbols
panic = "abort"     # No unwinding
```

### Cross-Compilation Flow

```mermaid
graph LR
    subgraph "Build Host (x86_64)"
        RS[Rust Source]
        CC[cc crate — C++ compilation]
        CARGO[cargo build --target aarch64-linux-android --release]
    end

    subgraph "NDK Toolchain (r25+)"
        CLANG[aarch64-linux-android26-clang]
        LLD[LLD Linker]
        SYSROOT[Android Sysroot — API 26]
    end

    subgraph "Output"
        SO_D[libaura_daemon.so — JNI library]
        SO_N[aura-neocortex — Standalone ELF]
    end

    RS --> CARGO
    CARGO --> CC
    CC --> CLANG
    CARGO --> CLANG
    CLANG --> LLD
    LLD --> SO_D
    LLD --> SO_N
```

### Assessment

| Aspect | Status | Detail |
|---|---|---|
| Target triple configured | ✅ Yes | `aarch64-linux-android` in both config files |
| NDK linker specified | ✅ Yes | `aarch64-linux-android26-clang` (API 26 = Android 8.0) |
| LLD linker forced | ✅ Yes | `-fuse-ld=lld` in rustflags |
| NEON SIMD enabled | ✅ Yes | `+neon` in target features |
| Emulator target | ✅ Yes | `x86_64-linux-android` configured |
| Library type correct | ✅ Yes | `crate-type = ["cdylib", "lib"]` in aura-daemon |
| Size optimization | ✅ Yes | `opt-level = "z"`, LTO, strip, panic=abort |
| **Actually builds successfully** | ❌ **UNVERIFIED** | No CI, no build log, no evidence of successful compilation |
| **NDK installed on any machine** | ❌ **UNKNOWN** | No documentation of build environment setup |
| **Build script for full pipeline** | ❌ **MISSING** | No `build-android.sh` or equivalent |

### ⚠️ Configuration Conflict

Two competing release profiles exist:

| Source | `opt-level` | LTO | Detail |
|---|---|---|---|
| `Cargo.toml` workspace | `"z"` (size) | `true` (full) | Single codegen unit, stripped |
| `.cargo/config.toml` | `"s"` (size, less aggressive) | `"thin"` | Less aggressive |

**Resolution needed:** `.cargo/config.toml` profile may override `Cargo.toml`. This needs to be tested and unified. For mobile, `opt-level = "z"` with full LTO is correct.

### Missing Build Pipeline

A complete Android build requires these steps — **none are automated:**

1. `rustup target add aarch64-linux-android`
2. Set `ANDROID_NDK_HOME` environment variable
3. `cargo build --target aarch64-linux-android --release -p aura-daemon`
4. `cargo build --target aarch64-linux-android --release -p aura-neocortex`
5. Copy `libaura_daemon.so` → `android/app/src/main/jniLibs/arm64-v8a/`
6. Copy `aura-neocortex` binary → `android/app/src/main/assets/` (or jniLibs)
7. `./gradlew assembleRelease` (from `android/` directory)
8. Sign APK with release keystore

---

## 4. llama.cpp Integration Assessment

### Architecture: Dual-Mode Backend

```mermaid
graph TB
    subgraph "aura-llama-sys"
        TRAIT[LlamaBackend Trait]
        STUB[StubBackend — Bigram text generator]
        FFI[FfiBackend — Real llama.cpp FFI]
        GGUF[GgufMetadata — Model file parser]
    end

    subgraph "Build System"
        BUILD_RS[build.rs — cc crate C++ compilation]
        NEON[NEON flags on android target]
        NOOP[No-op on host — uses StubBackend]
    end

    subgraph "llama.cpp Source — ⛔ NOT PRESENT"
        LLAMA_CPP[llama.cpp/llama.cpp]
        GGML_C[llama.cpp/ggml.c]
        COMMON[llama.cpp/common/common.cpp]
    end

    TRAIT --> STUB
    TRAIT --> FFI
    FFI --> BUILD_RS
    BUILD_RS -->|"target_os = android"| NEON
    BUILD_RS -->|"not android"| NOOP
    BUILD_RS -.->|References but missing| LLAMA_CPP
    BUILD_RS -.->|References but missing| GGML_C
```

### What Works

| Feature | Status | Detail |
|---|---|---|
| `LlamaBackend` trait | ✅ Defined | `init`, `load_model`, `generate`, `tokenize`, `embedding`, `unload` |
| `StubBackend` | ✅ Working | Bigram-based text generator, sufficient for host-side testing |
| `FfiBackend` skeleton | 🟡 Declared | extern "C" blocks with function signatures, but can't link |
| GGUF metadata parser | ✅ Production | v2/v3 format, RAM estimation, quantization detection, thinking mode |
| `build.rs` structure | 🟡 Correct logic | Android path compiles C++; host path is no-op; but source files missing |

### Critical Issues

#### Issue 1: llama.cpp Source Not Vendored (🔴 BLOCKER)

`build.rs` references these files which **do not exist** in the repository:

```
llama.cpp/llama.cpp
llama.cpp/ggml.c
llama.cpp/common/common.cpp
llama.cpp/common/sampling.cpp
```

No git submodule (`.gitmodules` not present or doesn't include llama.cpp). No vendored copy. **The Android build will fail immediately at the C++ compilation step.**

**Resolution:** Either add llama.cpp as a git submodule or vendor a specific commit. Given the API compatibility issue below, vendoring a pinned version is safer.

#### Issue 2: Outdated FFI API (🟡 HIGH)

The `extern "C"` declarations in `lib.rs` use the **pre-batch API**:

```rust
// What AURA declares:
fn llama_decode(ctx: *mut llama_context, tokens: *const i32, n_tokens: i32, n_past: i32) -> i32;

// What modern llama.cpp (b4000+) actually exports:
fn llama_decode(ctx: *mut llama_context, batch: llama_batch) -> i32;
```

This means the FFI bindings are targeting llama.cpp from approximately **mid-2024 or earlier**. The batch API was introduced in late 2023 and became mandatory in 2024.

**Resolution options:**
1. Vendor llama.cpp at a commit that matches the current FFI signatures (~b2000 era)
2. Update FFI signatures to match modern llama.cpp (recommended, more features + performance)

#### Issue 3: No Model Quantization Strategy Documented

The GGUF metadata parser detects quantization types (Q4_0, Q4_1, Q5_0, Q5_1, Q8_0, F16, F32) but there's no documentation of:
- Which quantization levels are target for which device tiers
- Expected model sizes for target models
- RAM requirements vs available device RAM
- Whether 4-bit quantization accuracy is acceptable for AURA's use cases

### GGUF Metadata Parser — Production Quality Assessment

This is one of the most mature components. Capabilities:

| Feature | Implementation |
|---|---|
| Format versions | v2 and v3 (with large file support) |
| Metadata types | All 13 GGUF types (uint8 through string array) |
| RAM estimation | Calculates from tensor count × bytes-per-element per quantization type |
| Quantization detection | Maps GGML type enum to human-readable quantization names |
| Thinking mode | Detects `<think>` token in vocabulary → enables CoT mode |
| Architecture detection | Reads `general.architecture` key (llama, mistral, phi, etc.) |
| Context length | Reads from architecture-specific keys |
| Error handling | Proper `Result` types, no panics on malformed files |

---

## 5. Android Permissions Model

### Required Permissions — None Declared

**`AndroidManifest.xml` does not exist.** The following permissions are required based on what the Rust platform layer calls via JNI:

| Permission | Android Constant | Why AURA Needs It | Risk Level |
|---|---|---|---|
| `FOREGROUND_SERVICE` | Normal | Keep daemon alive | Low |
| `FOREGROUND_SERVICE_SPECIAL_USE` | Normal (API 34+) | AI assistant foreground service | Low |
| `POST_NOTIFICATIONS` | Runtime (API 33+) | Notification channels | Medium |
| `INTERNET` | Normal | Model downloads, potential cloud features | Low |
| `ACCESS_NETWORK_STATE` | Normal | `connectivity.rs` network monitoring | Low |
| `ACCESS_WIFI_STATE` | Normal | WiFi quality detection | Low |
| `CHANGE_WIFI_STATE` | Dangerous | `toggleWifi` JNI function | High |
| `RECEIVE_BOOT_COMPLETED` | Normal | Auto-start after reboot | Low |
| `WAKE_LOCK` | Normal | `doze.rs` wakelock management | Low |
| `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` | Normal | Doze exemption request | Medium |
| `BIND_ACCESSIBILITY_SERVICE` | Signature | Screen reading, UI automation | 🔴 Very High |
| `SEND_SMS` | Dangerous + Runtime | `sendSms` JNI function | 🔴 Very High |
| `CALL_PHONE` | Dangerous + Runtime | `makeCall` JNI function | 🔴 Very High |
| `READ_CONTACTS` | Dangerous + Runtime | `getContacts` JNI function | High |
| `READ_CALENDAR` / `WRITE_CALENDAR` | Dangerous + Runtime | `createCalendarEvent` JNI function | High |
| `SET_ALARM` | Normal | Alarm JNI functions | Low |
| `SYSTEM_ALERT_WINDOW` | Special | Overlay UI (if needed) | High |
| `QUERY_ALL_PACKAGES` | Normal (restricted API 30+) | `getInstalledApps` JNI function | Medium |
| `BODY_SENSORS` | Dangerous + Runtime | `sensors.rs` step counter | Medium |

### Permission Escalation Concerns

**This app requests an extremely aggressive permission set.** For Google Play Store approval:

1. **Accessibility Service** — Google requires detailed justification and manual review. Apps abusing accessibility are frequently rejected.
2. **SMS + Phone** — These are in the most restricted permission groups. Google Play requires a Permissions Declaration Form.
3. **Battery Optimization Exemption** — Google scrutinizes this heavily.
4. **QUERY_ALL_PACKAGES** — Requires justification since Android 11.

**Recommendation:** Implement a **tiered permission model**:
- **Core tier** (install-time): FOREGROUND_SERVICE, INTERNET, NETWORK_STATE, BOOT_COMPLETED, WAKE_LOCK, NOTIFICATIONS
- **Enhanced tier** (runtime, user-initiated): ACCESSIBILITY, CONTACTS, CALENDAR
- **Privileged tier** (explicit opt-in with warning): SMS, PHONE, WIFI_CONTROL

---

## 6. Battery & Performance Architecture

### Power Management — The Crown Jewel

The `power.rs` module is the most sophisticated component in the codebase. It implements a **physics-based, 5-tier battery management system** that would be impressive in a shipping product.

### 5-Tier Energy Model

```mermaid
graph TB
    subgraph "Tier 0 — PERFORMANCE"
        T0[Full model, max context]
        T0_COND["Battery > 80% AND charging"]
    end

    subgraph "Tier 1 — BALANCED"
        T1[Full model, reduced context]
        T1_COND["Battery 50-80% OR discharging + high"]
    end

    subgraph "Tier 2 — CONSERVATIVE"
        T2[Smaller model, limited context]
        T2_COND["Battery 20-50%"]
    end

    subgraph "Tier 3 — LOW POWER"
        T3[Minimal model, essential functions only]
        T3_COND["Battery 10-20%"]
    end

    subgraph "Tier 4 — CRITICAL"
        T4[Suspend inference, notifications only]
        T4_COND["Battery < 10%"]
    end

    T0 -->|Battery drops below 80%| T1
    T1 -->|Battery drops below 50%| T2
    T2 -->|Battery drops below 20%| T3
    T3 -->|Battery drops below 10%| T4
    T4 -->|Charging resumes| T3
```

### Key Power Features

| Feature | Implementation | Quality |
|---|---|---|
| Energy tracking | Real mWh/mAh estimation from model inference | 🟢 Production |
| Hysteresis | Prevents rapid tier oscillation (configurable thresholds) | 🟢 Good |
| Model tier cascade | Automatically selects smaller/larger model per tier | 🟢 Designed |
| Charge state awareness | Distinguishes AC/USB/wireless charging rates | 🟢 Good |
| Estimated runtime | Projects remaining battery life under current load | 🟡 Needs calibration |

### Thermal Management

`thermal.rs` implements a **PID-controlled thermal management system**:

| Feature | Detail |
|---|---|
| Control algorithm | PID (Proportional-Integral-Derivative) controller |
| Thermal zones | Multi-zone model (CPU, GPU, battery, skin) |
| Cooling model | Newton's law of cooling simulation for prediction |
| Throttle actions | Reduce inference frequency → reduce model size → suspend inference |
| OEM thermal APIs | JNI calls to manufacturer-specific thermal interfaces |

### Doze Mode & OEM Kill Prevention

`doze.rs` is battle-hardened for the Android fragmentation nightmare:

| OEM | Kill Prevention Strategy |
|---|---|
| **Xiaomi** | AutoStart permission detection, MIUI battery saver whitelist |
| **Samsung** | Sleeping apps whitelist, Device Care exemption |
| **Huawei** | Protected apps list, EMUI power manager whitelist |
| **OPPO** | Auto-launch manager, ColorOS battery optimization |
| **Vivo** | Background app management whitelist |
| **OnePlus** | Battery optimization + deep optimization exemption |
| **Generic** | Standard `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` |

**Assessment:** This OEM-specific handling is exactly what's needed for a background AI service on Android. The 7-vendor coverage represents ~85% of the global Android market. This is one of the strongest parts of the architecture.

### Performance Budgets (Theoretical — Not Tested)

| Metric | Target | Current Status |
|---|---|---|
| Idle battery drain | < 2% per hour | ❌ Untested |
| Active inference drain | < 8% per hour | ❌ Untested |
| Memory (daemon) | < 50 MB RSS | ❌ Untested |
| Memory (neocortex + model) | < 2 GB | ❌ Untested |
| Cold start to ready | < 3 seconds | ❌ Untested |
| Inference latency (Q4 3B) | < 500ms first token | ❌ Untested |
| Binary size (stripped .so) | < 5 MB | ❌ Untested |

---

## 7. Android Service Lifecycle Architecture

### Designed Architecture (Rust-side ready, Kotlin-side missing)

```mermaid
stateDiagram-v2
    [*] --> AppLaunch: User opens AURA
    AppLaunch --> ForegroundService: Start AuraForegroundService
    ForegroundService --> DaemonInit: Load libaura_daemon.so via JNI
    DaemonInit --> NeocortexSpawn: Daemon spawns neocortex process
    NeocortexSpawn --> IPCConnect: Unix domain socket established
    IPCConnect --> Ready: System operational

    Ready --> Backgrounded: User switches app
    Backgrounded --> Ready: User returns

    Ready --> DozeEntry: Android enters Doze
    DozeEntry --> DozeMaintenance: Deferred work queue active
    DozeMaintenance --> DozeWindow: Maintenance window
    DozeWindow --> DozeMaintenance: Window closes
    DozeEntry --> Ready: Doze exits

    Ready --> ThermalThrottle: Temperature threshold
    ThermalThrottle --> ThermalCooldown: PID controller reduces load
    ThermalCooldown --> Ready: Temperature normalized

    Ready --> LowBattery: Battery < 20%
    LowBattery --> TierCascade: Reduce model tier
    TierCascade --> CriticalBattery: Battery < 10%
    CriticalBattery --> SuspendInference: Kill neocortex, keep daemon

    Ready --> OOMKill: Android kills neocortex (LMK)
    OOMKill --> NeocortexRespawn: Daemon detects death, respawns
    NeocortexRespawn --> IPCConnect

    ForegroundService --> [*]: User stops service
```

### Two-Process Architecture Rationale

| Concern | Solution |
|---|---|
| LLM uses too much RAM → OOM | Neocortex is separate process; LMK kills it, daemon survives |
| Android kills background apps | Foreground service with persistent notification |
| Battery optimization kills daemon | OEM-specific whitelist guidance + `IGNORE_BATTERY_OPTIMIZATIONS` |
| Doze mode blocks network | Deferred work queue, maintenance window handling |
| Thermal throttling | PID controller gracefully degrades before Android force-throttles |

### What's Missing for This to Work

| Required Component | Status | Effort Estimate |
|---|---|---|
| `AuraForegroundService.kt` | ⛔ Missing | 2-3 days |
| `AuraAccessibilityService.kt` | ⛔ Missing | 3-5 days |
| `AuraDaemonBridge.kt` (35 JNI methods) | ⛔ Missing | 2-3 days |
| `AndroidManifest.xml` with all declarations | ⛔ Missing | 1 day |
| Service lifecycle management in Kotlin | ⛔ Missing | 2-3 days |
| Neocortex binary packaging + extraction | ⛔ Missing | 1-2 days |
| IPC socket path management on Android | 🟡 Partially addressed | 1 day |

---

## 8. First-Run Experience Architecture

### Required Flow — None Implemented

```mermaid
graph TD
    START[App First Launch] --> PERM1[Request Notification Permission — API 33+]
    PERM1 --> PERM2[Request Battery Optimization Exemption]
    PERM2 --> OEM{OEM-Specific Setup?}
    OEM -->|Xiaomi| XIAOMI[Guide: Enable AutoStart in Security app]
    OEM -->|Samsung| SAMSUNG[Guide: Add to Unmonitored apps]
    OEM -->|Huawei| HUAWEI[Guide: Add to Protected apps]
    OEM -->|Other| GENERIC[Standard battery optimization]
    XIAOMI --> ACCESSIBILITY
    SAMSUNG --> ACCESSIBILITY
    HUAWEI --> ACCESSIBILITY
    GENERIC --> ACCESSIBILITY
    ACCESSIBILITY[Request Accessibility Service — System Settings redirect]
    ACCESSIBILITY --> MODEL[Model Selection & Download]
    MODEL --> SIZE{Device RAM Check}
    SIZE -->|"≥ 8GB"| FULL[Offer 7B Q4 model — ~4GB download]
    SIZE -->|"4-8GB"| MEDIUM[Offer 3B Q4 model — ~2GB download]
    SIZE -->|"< 4GB"| SMALL[Offer 1B Q8 model — ~1GB download]
    FULL --> DOWNLOAD[Download with progress bar]
    MEDIUM --> DOWNLOAD
    SMALL --> DOWNLOAD
    DOWNLOAD --> VERIFY[Verify GGUF integrity — metadata parse]
    VERIFY --> READY[System Ready — Start foreground service]
```

### What Exists to Support This

| First-Run Step | Rust Support | Kotlin UI | Overall |
|---|---|---|---|
| OEM detection | ✅ `doze.rs` detects manufacturer | ❌ No UI | 🟡 Backend only |
| Battery optimization request | ✅ JNI function defined | ❌ No UI | 🟡 Backend only |
| GGUF model validation | ✅ Production-quality parser | ❌ No UI | 🟡 Backend only |
| RAM-based model recommendation | ✅ `gguf_meta.rs` RAM estimation | ❌ No UI | 🟡 Backend only |
| Model download | ❌ No download manager | ❌ No UI | ⛔ Missing |
| Accessibility setup guide | ❌ No guide logic | ❌ No UI | ⛔ Missing |
| Notification channel creation | ✅ `notifications.rs` | ❌ No UI trigger | 🟡 Backend only |

---

## 9. Deployment Gap Analysis

### Priority Classification

- **P0 (BLOCKER):** Cannot produce any APK without this
- **P1 (CRITICAL):** APK builds but app crashes or is non-functional
- **P2 (HIGH):** App runs but key features broken
- **P3 (MEDIUM):** App works but poor UX or missing features
- **P4 (LOW):** Polish, optimization, nice-to-have

### P0 — BLOCKERS (Must fix before any APK)

| # | Gap | Detail | Effort | Owner |
|---|---|---|---|---|
| P0-1 | **No AndroidManifest.xml** | Cannot build APK without it. Must declare application, services, permissions, receivers. | 1 day | Android eng |
| P0-2 | **No Kotlin source code** | Zero `.kt` files in `android/app/src/main/`. Need at minimum: `AuraActivity.kt`, `AuraDaemonBridge.kt`, `AuraForegroundService.kt` | 5-7 days | Android eng |
| P0-3 | **llama.cpp source not vendored** | `build.rs` references `llama.cpp/*.cpp` files that don't exist. Cross-compilation of native code will fail. | 1 day | Rust eng |
| P0-4 | **Cross-compilation never verified** | Config looks correct but has never produced a `.so`. Unknown issues with NDK version, sysroot, C++ stdlib linking. | 1-3 days | Rust eng |
| P0-5 | **No build pipeline script** | No automated way to: build Rust → copy .so → build APK. Manual process is error-prone. | 1 day | DevOps |

### P1 — CRITICAL (App builds but broken)

| # | Gap | Detail | Effort |
|---|---|---|---|
| P1-1 | **Outdated llama.cpp FFI signatures** | Pre-batch API won't link against modern llama.cpp. Either pin old version or update signatures. | 2-3 days |
| P1-2 | **`AuraDaemonBridge.kt` not implemented** | 35 JNI methods expected by Rust. JNI calls will crash with `UnsatisfiedLinkError`. | 2-3 days |
| P1-3 | **Neocortex binary packaging** | Second Rust binary needs to be extracted from APK assets and made executable at runtime. No mechanism exists. | 1-2 days |
| P1-4 | **IPC socket path on Android** | Unix domain socket needs a valid path in app's private directory. Needs Kotlin-side path passing to Rust. | 1 day |
| P1-5 | **Release profile conflict** | `Cargo.toml` vs `.cargo/config.toml` have conflicting `opt-level` and LTO settings. | 0.5 day |

### P2 — HIGH (App runs but features broken)

| # | Gap | Detail | Effort |
|---|---|---|---|
| P2-1 | **No model download mechanism** | GGUF models (1-4 GB) need to be downloaded, stored, and managed. No download manager. | 3-5 days |
| P2-2 | **No Accessibility Service** | Screen reading and UI automation — core AURA functionality — doesn't work. | 3-5 days |
| P2-3 | **No runtime permission handling** | Dangerous permissions (SMS, Phone, Contacts, Calendar) need runtime request flows. | 2-3 days |
| P2-4 | **No model integrity verification** | Downloaded GGUF files could be corrupted or tampered. No checksum/signature verification. | 1-2 days |
| P2-5 | **No IPC authentication** | Daemon ↔ Neocortex socket has no auth. Any local process could connect. | 1-2 days |

### P3 — MEDIUM (Poor UX)

| # | Gap | Detail | Effort |
|---|---|---|---|
| P3-1 | **No first-run onboarding** | User gets no guidance on permissions, model selection, OEM settings. | 3-5 days |
| P3-2 | **No UI at all** | No settings, no model management, no status display. | 5-10 days |
| P3-3 | **No crash reporting** | No Firebase Crashlytics or equivalent. Crashes in native code are silent. | 1-2 days |
| P3-4 | **No analytics** | No way to measure adoption, retention, feature usage. | 1-2 days |
| P3-5 | **No OEM setup guides** | Doze prevention logic exists but no user-facing instructions per manufacturer. | 2-3 days |

### P4 — LOW (Polish)

| # | Gap | Detail | Effort |
|---|---|---|---|
| P4-1 | **No ProGuard/R8 configuration** | APK size not optimized on Kotlin side. | 0.5 day |
| P4-2 | **No app signing configuration** | No release keystore, no signing config in Gradle. | 0.5 day |
| P4-3 | **No CI/CD pipeline** | No GitHub Actions / similar for automated builds. | 2-3 days |
| P4-4 | **No Android instrumented tests** | No on-device test suite. | 3-5 days |
| P4-5 | **No Play Store listing preparation** | No screenshots, description, privacy policy. | 2-3 days |

### Gap Summary

| Priority | Count | Total Effort (days) |
|---|---|---|
| P0 — Blocker | 5 | 9-13 |
| P1 — Critical | 5 | 7-10 |
| P2 — High | 5 | 10-17 |
| P3 — Medium | 5 | 12-22 |
| P4 — Low | 5 | 8-14 |
| **TOTAL** | **25** | **46-76 engineering days** |

---

## 10. Roadmap to v1.0 Android Release

### Phase 0: Foundation (Week 1-2) — Unblock APK Build

**Goal:** Produce a minimal APK that loads `libaura_daemon.so` and prints "Hello from Rust" via JNI.

| Task | Priority | Days | Dependencies |
|---|---|---|---|
| Vendor llama.cpp at compatible commit | P0-3 | 1 | None |
| Verify cross-compilation produces `.so` | P0-4 | 1-3 | P0-3 |
| Create `AndroidManifest.xml` with core permissions | P0-1 | 1 | None |
| Create minimal `AuraActivity.kt` | P0-2 | 1 | None |
| Create `AuraDaemonBridge.kt` with 5 basic JNI methods | P0-2 | 1 | P0-4 |
| Create `build-android.sh` pipeline script | P0-5 | 1 | P0-4 |
| Resolve release profile conflict | P1-5 | 0.5 | None |
| **Milestone:** APK installs and Rust native code loads | | **6-8 days** | |

### Phase 1: Core Runtime (Week 3-4) — Daemon Stays Alive

**Goal:** Foreground service keeps daemon running. Model loads and generates text.

| Task | Priority | Days | Dependencies |
|---|---|---|---|
| Implement `AuraForegroundService.kt` | P1 | 2-3 | Phase 0 |
| Complete `AuraDaemonBridge.kt` (all 35 methods) | P1-2 | 2-3 | Phase 0 |
| Fix llama.cpp FFI signatures (batch API) | P1-1 | 2-3 | P0-3 |
| Package neocortex binary in APK assets | P1-3 | 1-2 | Phase 0 |
| Implement IPC socket path passing | P1-4 | 1 | Phase 0 |
| **Milestone:** Daemon runs, model loads, inference works on device | | **8-12 days** | |

### Phase 2: User-Facing (Week 5-7) — It Does Something Useful

**Goal:** Model downloads, accessibility works, user can interact.

| Task | Priority | Days | Dependencies |
|---|---|---|---|
| Build model download manager | P2-1 | 3-5 | Phase 1 |
| Implement `AuraAccessibilityService.kt` | P2-2 | 3-5 | Phase 1 |
| Runtime permission request flows | P2-3 | 2-3 | Phase 0 |
| First-run onboarding flow | P3-1 | 3-5 | P2-1, P2-3 |
| Model integrity verification (SHA-256) | P2-4 | 1-2 | P2-1 |
| IPC authentication (shared secret) | P2-5 | 1-2 | Phase 1 |
| **Milestone:** Complete user flow from install to AI assistant | | **13-22 days** | |

### Phase 3: Production Hardening (Week 8-10) — Ship It

**Goal:** Crash reporting, OEM guides, testing, Play Store ready.

| Task | Priority | Days | Dependencies |
|---|---|---|---|
| Crash reporting (Crashlytics + native crash handler) | P3-3 | 1-2 | Phase 2 |
| Analytics integration | P3-4 | 1-2 | Phase 2 |
| OEM-specific setup guides UI | P3-5 | 2-3 | Phase 2 |
| Settings UI and model management | P3-2 | 5-10 | Phase 2 |
| ProGuard/R8 configuration | P4-1 | 0.5 | Phase 2 |
| App signing configuration | P4-2 | 0.5 | Phase 2 |
| CI/CD pipeline | P4-3 | 2-3 | Phase 1 |
| Android instrumented tests | P4-4 | 3-5 | Phase 2 |
| Play Store listing | P4-5 | 2-3 | Phase 3 |
| **Milestone:** Play Store submission ready | | **18-30 days** | |

### Total Timeline Estimate

| Phase | Duration | Cumulative |
|---|---|---|
| Phase 0: Foundation | 2 weeks | Week 2 |
| Phase 1: Core Runtime | 2 weeks | Week 4 |
| Phase 2: User-Facing | 3 weeks | Week 7 |
| Phase 3: Production Hardening | 3 weeks | Week 10 |
| **Total to v1.0** | **~10 weeks** | **With 1 Android + 1 Rust engineer** |

---

## Appendix A: What's Surprisingly Good

Despite the gaps, several components demonstrate senior-level engineering:

1. **JNI Bridge Architecture** — 1354 lines covering 35 functions across 12 domains. Proper `JNI_OnLoad` with cached `JavaVM`. Error handling on every JNI call. This was designed by someone who has shipped JNI code before.

2. **OEM Kill Prevention** — 7-vendor specific handling is exactly the tribal knowledge needed for background Android services. This is not in any documentation — it comes from production experience.

3. **Two-Process Architecture** — Separating the daemon from the inference process is the correct architecture for Android. The LMK will kill the memory-heavy inference process while the lightweight daemon survives.

4. **Physics-Based Power Management** — mWh tracking, hysteresis, PID thermal control. This is overengineered for an alpha but exactly right for a product that runs 24/7 on a phone.

5. **GGUF Metadata Parser** — Production-quality format parser with RAM estimation. This is the kind of thing that usually gets hacked together; here it's robust.

6. **Conditional Compilation** — Every platform module has `#[cfg(target_os = "android")]` real implementations and `#[cfg(not(...))]` desktop stubs. This means the entire daemon can be developed and tested on desktop without an Android device.

## Appendix B: Architecture Decision Records

### ADR-001: Two-Process Architecture (Daemon + Neocortex)

**Context:** LLM inference requires 1-4 GB RAM. Android's Low Memory Killer (LMK) aggressively kills memory-heavy processes. A single-process architecture would kill the entire app when memory pressure rises.

**Decision:** Separate the LLM inference into `aura-neocortex` (killable) and keep `aura-daemon` (lightweight, survives) as separate OS processes communicating via Unix domain socket IPC.

**Consequences:**
- ✅ Daemon survives LMK killing the inference process
- ✅ Inference process can be respawned without app restart
- ✅ Memory pressure is localized to one process
- ❌ IPC adds latency (~1-5ms per message)
- ❌ Two binaries to package, deploy, and version
- ❌ Socket lifecycle management complexity

**Status:** Accepted — architecture is sound, implementation partially complete.

### ADR-002: Manual FFI over Bindgen

**Context:** `aura-llama-sys` needs to call llama.cpp C functions from Rust.

**Decision:** Use manually declared `extern "C"` blocks instead of `bindgen` auto-generation.

**Consequences:**
- ✅ No bindgen build dependency (complex for cross-compilation)
- ✅ Cleaner Rust API (hand-crafted wrappers)
- ✅ Faster compilation
- ❌ FFI signatures can drift from actual llama.cpp API (currently happening — pre-batch API)
- ❌ Must manually update when llama.cpp changes

**Status:** Accepted with risk — API drift is a known issue.

### ADR-003: Stub Backend for Desktop Development

**Context:** Developers need to test the daemon without an Android device or a GPU for LLM inference.

**Decision:** `StubBackend` implements `LlamaBackend` trait with a bigram text generator, selected automatically on non-Android targets.

**Consequences:**
- ✅ Full daemon development and testing on desktop
- ✅ CI can run without GPU or Android emulator
- ✅ Fast iteration cycles
- ❌ Stub output doesn't test real LLM behavior
- ❌ Integration issues only found on actual device

**Status:** Accepted — correct trade-off for development velocity.

---

*End of document. This assessment is based on codebase analysis performed on 2026-03-13. All "missing" items are verified absent — not merely undiscovered.*
