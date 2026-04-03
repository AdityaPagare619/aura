# AURA v4 — Deep Architecture Review

**Reviewer:** 🏗️ Architect Agent  
**Date:** 2026-04-02  
**Scope:** Full codebase at `C:\Users\Lenovo\aura-hotfix-link2`  
**Model:** mimo-v2-pro-free (Architect persona)

---

## Executive Summary

AURA v4 is a well-structured Rust workspace with 5 crates implementing an on-device AI agent for Android. The architecture demonstrates strong security awareness (authenticated IPC envelopes, rate limiting, policy gates), a sophisticated 4-tier memory system, and clean separation between daemon and neocortex (LLM) processes. However, several architectural issues limit scalability, cross-device compatibility, and maintainability.

**Severity Distribution:**
| Severity | Count | Description |
|----------|-------|-------------|
| 🔴 HIGH | 4 | Blocks multi-device/multi-platform deployment |
| 🟡 MEDIUM | 6 | Limits scalability or maintainability |
| 🟢 LOW | 4 | Technical debt, polish items |

---

## 1. Module Dependencies and Coupling

### 1.1 Dependency Graph

```
aura-types (shared types, IPC protocol, config)
    ├── aura-daemon (main process: memory, policy, platform, IPC client)
    ├── aura-neocortex (LLM process: inference, IPC handler)
    ├── aura-llama-sys (FFI bindings to llama.cpp)
    └── aura-iron-laws (ethics layer, independent)
```

### 🔴 ISSUE 1.1: Circular Coupling Between Daemon and Neocortex via Shared Constants

**File:** `crates/aura-daemon/src/ipc/protocol.rs:38-45` and `crates/aura-neocortex/src/ipc_handler.rs:46-56`

**Problem:** Both crates independently define `MAX_MESSAGE_SIZE` (256KB), `LENGTH_PREFIX_SIZE` (4 bytes), and `REQUEST_TIMEOUT` (30s). If these drift, protocol corruption occurs silently.

```
// daemon/protocol.rs:48
pub const MAX_MESSAGE_SIZE: usize = 256 * 1024;

// neocortex/ipc_handler.rs:47
const MAX_MESSAGE_SIZE: usize = 256 * 1024;
```

**Impact:** Any change to wire protocol constants requires synchronized updates across two crates. A mismatch causes silent data corruption or connection drops.

**Fix:** Move all wire protocol constants to `aura-types/src/ipc.rs` as a single source of truth. Both crates import from `aura_types::ipc::*`.

### 🟡 ISSUE 1.2: aura-daemon Has No Dependency on aura-llama-sys (Intentional) But Implicit Coupling Exists

**File:** `crates/aura-daemon/Cargo.toml:23`

**Problem:** The daemon correctly doesn't depend on `aura-llama-sys`, but the `NeocortexClient` sends `DaemonToNeocortex::Load` messages containing `model_path` strings that are resolved by `aura-llama-sys::ModelScanner`. The daemon has no type-safe way to validate model paths before sending them.

**Impact:** Invalid model paths cause IPC round-trip failures (30s timeout) instead of early rejection.

**Fix:** Add a `ModelDiscovery` trait to `aura-types` that both crates can implement/use. The daemon validates paths locally before IPC.

### 🟢 ISSUE 1.3: Workspace-Level vs Crate-Level SBOM Configuration Mismatch

**File:** `Cargo.toml:11-13, 43-44`

**Problem:** Comments state "SBOM is configured at crate level" but no crate actually defines `[package.metadata.bom]`. The workspace comment is misleading.

**Impact:** Security audit tooling may miss SBOM configuration entirely.

**Fix:** Either implement SBOM metadata in each crate or remove the misleading comments.

---

## 2. Hardcoded Paths and Device Assumptions

### 🔴 ISSUE 2.1: Android-Specific Hardcoded Paths in Default Config

**File:** `crates/aura-types/src/config.rs:64, 75, 178, 280`

```rust
// Line 64: Default data directory
fn default_daemon_data_dir() -> String {
    "/data/data/com.aura/files".to_string()  // Android-only path
}

// Line 178: Default model directory
model_dir: "/data/local/tmp/aura/models".to_string(),  // Android-only

// Line 280: Default SQLite path
db_path: "/data/data/com.aura/databases/aura.db".to_string(),  // Android-only
```

**Impact:** 
- On non-Android hosts, defaults fail silently or require explicit config override
- On Android, `/data/data/com.aura/` requires app-private permissions; `/data/local/tmp/` is world-readable (security risk for model files)
- Different Android ROMs may not have `/data/local/tmp/` writable

**Fix:** Introduce a `PlatformPaths` trait:
```rust
pub trait PlatformPaths {
    fn data_dir(&self) -> PathBuf;
    fn model_dir(&self) -> PathBuf;
    fn db_path(&self) -> PathBuf;
}
```
Implement per-platform with runtime detection. Use `dirs` crate for cross-platform home directory resolution.

### 🔴 ISSUE 2.2: Hardcoded Neocortex Binary Path

**File:** `crates/aura-daemon/src/ipc/spawn.rs:34`

```rust
const ANDROID_NEOCORTEX_PATH: &str = "/data/local/tmp/aura-neocortex";
```

**Impact:** 
- `/data/local/tmp/` permissions vary by OEM (Samsung Knox, Xiaomi MIUI restrict this)
- No fallback if this path is inaccessible
- The path resolution in `resolve_neocortex_path()` (lines 48-105) is good but the constant is still used as last resort

**Fix:** Remove the hardcoded constant. Make `resolve_neocortex_path()` the only source, with proper error reporting if all paths fail.

### 🟡 ISSUE 2.3: sysfs Paths for Battery/Thermal Are ARM-Linux Specific

**File:** `crates/aura-daemon/src/platform/mod.rs:257, 290`

```rust
// Line 257
let capacity = fs::read_to_string("/sys/class/power_supply/battery/capacity");

// Line 290
let raw = fs::read_to_string("/sys/class/thermal/thermal_zone0/temp");
```

**Impact:** 
- Some devices expose battery at `/sys/class/power_supply/battery/` while others use `/sys/class/power_supply/BAT0/` or `/sys/class/power_supply/main-battery/`
- `thermal_zone0` may not be the skin temperature sensor on all SoCs
- x86 Android (Intel-based tablets) may have different sysfs layout

**Fix:** Probe multiple sysfs paths at startup, cache the working one. Add a `ThermalZone` enum that maps to discovered paths.

### 🟡 ISSUE 2.4: config.toml Contains Production Secrets

**File:** `config.toml:75`

```toml
bot_token = "8764736044:AAEuSHrnfzvrEbp9txWFrgSeC6R_daT6304"
```

**Impact:** Token is committed to version control. Any clone has full Telegram bot access.

**Fix:** Remove from config.toml. Use `AURA_TELEGRAM_TOKEN` env var exclusively (already supported in code at `aura-types/src/config.rs:536-537`). Add `config.toml` to `.gitignore` and provide `config.example.toml` only.

---

## 3. Platform Abstraction Layer

### 🟡 ISSUE 3.1: No Formal Platform Trait — Just cfg-Gated Functions

**File:** `crates/aura-daemon/src/platform/mod.rs:241-313`

**Problem:** Platform reads (`read_battery_info`, `read_temperature`, `read_doze_state`) are free functions gated by `#[cfg(target_os = "android")]`. There's no trait interface, making it impossible to:
- Mock platform behavior in tests without `#[cfg]` duplication
- Add new platforms (iOS, embedded Linux) without modifying every `#[cfg]` block
- Inject platform behavior for dependency injection

**Impact:** Adding iOS support would require touching 174+ `#[cfg]` annotations across the codebase.

**Fix:** Define a `PlatformProvider` trait:
```rust
pub trait PlatformProvider: Send + Sync {
    fn read_battery(&self) -> Result<(u8, bool), PlatformError>;
    fn read_temperature(&self) -> Result<f32, PlatformError>;
    fn read_doze_state(&self) -> Result<bool, PlatformError>;
    fn read_sensors(&self) -> Result<SensorSnapshot, PlatformError>;
}
```
Implement `AndroidPlatformProvider` and `HostStubProvider`. Inject via `PlatformState::new_with_provider()`.

### 🟡 ISSUE 3.2: Voice Module Has Deep Platform Coupling

**Files:** `crates/aura-daemon/src/voice/*.rs` (7 files, 50+ `#[cfg]` blocks)

**Problem:** Every voice submodule (wake_word, vad, tts, stt, signal_processing, audio_io, call_handler) has its own `#[cfg(target_os = "android")]` blocks with unsafe FFI calls. The pattern is:
```rust
#[cfg(target_os = "android")]
unsafe impl Send for SomeFfiHandle {}

#[cfg(target_os = "android")]
impl SomeVoiceComponent {
    pub fn new() -> Result<Self, VoiceError> {
        let state = unsafe { ffi_init() };
        // ...
    }
}
```

**Impact:** 
- Each FFI boundary is a memory safety risk
- No abstraction over audio backends (Android AudioRecord vs PulseAudio vs ALSA)
- Testing voice on host requires full `#[cfg]` stubbing

**Fix:** Create an `AudioBackend` trait with `AndroidAudioBackend` and `HostStubBackend` implementations. Isolate all `unsafe` FFI behind a single `ffi` module.

### 🟢 ISSUE 3.3: LTO + panic=abort Incompatibility (Already Fixed)

**File:** `Cargo.toml:38-41`

**Status:** ✅ Already addressed with comments referencing F001 fix. Changed from `lto = true` + `panic = "abort"` to `lto = "thin"` + `panic = "unwind"` due to NDK #2073.

**Note:** This is documented correctly. No action needed.

---

## 4. IPC Design (Unix Sockets vs TCP)

### Architecture Assessment: ✅ WELL DESIGNED

The IPC layer uses a clean platform abstraction:

```
Android:  UnixStream (abstract namespace @aura_ipc_v4)
Non-Android: TcpStream (127.0.0.1:19400)
```

**Protocol:** Length-prefixed bincode frames (4-byte LE header + payload)

### Strengths:
- Abstract Unix socket on Android avoids filesystem permissions
- TCP fallback for host development is clean
- Authenticated envelope with session tokens (CSPRNG, inherited FD)
- Rate limiting (100 req/s, 20 burst)
- Protocol versioning (v3, checked on every message)
- Exponential backoff reconnection
- Message size limits (256KB normal, 16KB under memory pressure)

### 🟡 ISSUE 4.1: Asymmetric Transport — Daemon Uses tokio, Neocortex Uses std

**File:** `crates/aura-daemon/src/ipc/protocol.rs:26-29` vs `crates/aura-neocortex/src/ipc_handler.rs:23-27`

```rust
// Daemon side (tokio async)
#[cfg(target_os = "android")]
pub type IpcStream = tokio::net::UnixStream;
#[cfg(not(target_os = "android"))]
pub type IpcStream = tokio::net::TcpStream;

// Neocortex side (std blocking)
#[cfg(target_os = "android")]
type IpcStreamInner = std::os::unix::net::UnixStream;
#[cfg(not(target_os = "android"))]
type IpcStreamInner = std::net::TcpStream;
```

**Impact:** The daemon uses async I/O (tokio) while the neocortex uses blocking I/O (std). This is intentional (neocortex inference is CPU-bound), but creates an asymmetry where:
- The neocortex can only handle ONE connection at a time
- Progress messages during inference block the write path
- If the daemon disconnects mid-inference, the neocortex may hang until write timeout

**Fix:** This is acceptable for the current use case (single daemon↔neocortex pair). Document the intentional asymmetry. Add a comment block explaining why blocking I/O is correct for the neocortex.

### 🟡 ISSUE 4.2: No Multiplexing — Single Request/Response Channel

**File:** `crates/aura-daemon/src/ipc/client.rs:83-92`

**Problem:** `NeocortexClient` holds a single `IpcStream`. All requests are serialized — the daemon cannot send a Cancel while a Plan request is in flight without queuing.

**Impact:** Cancellation during long inference requires waiting for the current request to complete or timeout (30s).

**Fix:** Consider multiplexed framing with request IDs, or use a separate control channel (e.g., a second socket for Cancel/Ping). For now, document the limitation.

### 🟢 ISSUE 4.3: TCP Port 19400 Is Hardcoded

**File:** `crates/aura-daemon/src/ipc/protocol.rs:45`

```rust
pub const TCP_FALLBACK_PORT: u16 = 19400;
```

**Impact:** Port conflict if another service uses 19400. Only affects host development.

**Fix:** Move to config or use port 0 (OS-assigned) with a handshake mechanism. Low priority since this is development-only.

---

## 5. Memory Management (4-Tier System)

### Architecture Assessment: ✅ EXCELLENT DESIGN

```
┌─────────┐    ┌──────────┐    ┌──────────┐    ┌─────────┐
│ Working  │───▶│ Episodic │───▶│ Semantic │───▶│ Archive │
│ (RAM)    │    │ (SQLite) │    │ (SQLite) │    │ (ZSTD)  │
│ 1MB/1024 │    │ ~18MB/yr │    │ ~50MB/yr │    │ ~4MB/yr │
│ <1ms     │    │ 2-8ms    │    │ 5-15ms   │    │ 50-200ms│
└─────────┘    └──────────┘    └──────────┘    └─────────┘
```

### Strengths:
- Clear tier boundaries with defined budgets and latencies
- WAL-mode SQLite for durability (never loses data)
- Cross-tier queries with relevance-based merging and deduplication
- GDPR export/erasure support
- Pattern discovery engine with Hebbian learning
- Feedback loop for error→resolution learning

### 🟡 ISSUE 5.1: Working Memory Is Not Thread-Safe

**File:** `crates/aura-daemon/src/memory/mod.rs:143-146`

```rust
/// `WorkingMemory`, `PatternEngine`, and `FeedbackLoop` are owned directly
/// (single-threaded access via `&mut self`).
/// `EpisodicMemory`, `SemanticMemory`, and `ArchiveMemory` each hold
/// `Arc<Mutex<Connection>>` internally, so they are `Send + Sync`.
```

**Problem:** `AuraMemory` is `!Sync` because `WorkingMemory` uses `&mut self`. This prevents concurrent access from multiple daemon subsystems (e.g., the pipeline and the consolidation engine cannot query working memory simultaneously).

**Impact:** All working memory access is serialized through the single `AuraMemory` owner. If consolidation is running, the pipeline blocks.

**Fix:** Wrap `WorkingMemory` in `RwLock<WorkingMemory>` to allow concurrent reads (queries) while serializing writes (push/sweep).

### 🟡 ISSUE 5.2: No Memory Pressure Feedback Loop to Model Selection

**File:** `crates/aura-daemon/src/memory/mod.rs:616-647` and `crates/aura-daemon/src/platform/mod.rs:210-226`

**Problem:** `memory_usage()` reports per-tier statistics, but there's no connection between memory pressure and model tier selection. The platform layer selects model tiers based on battery/thermal only.

**Impact:** Under memory pressure (e.g., 28MB RSS warning), the system could still load the 8B model, causing OOM.

**Fix:** Add memory pressure to `PlatformState::select_model_tier()`:
```rust
pub fn select_model_tier(&self, memory_report: &MemoryUsageReport) -> ModelTier {
    let energy_tier = self.power.select_model_tier_by_energy();
    let memory_tier = if memory_report.total_bytes > HIGH_MEMORY_THRESHOLD {
        ModelTier::Brainstem1_5B
    } else { energy_tier };
    // Take the more conservative of energy and memory
    min(energy_tier, memory_tier)
}
```

### 🟢 ISSUE 5.3: Archive Memory Uses Passthrough ZSTD (No Chunking)

**File:** `crates/aura-daemon/src/memory/mod.rs:14`

```
| Archive  | Passthrough (ZSTD)| ~4MB/year  | 50-200ms  | Old memories |
```

**Problem:** Archive blobs are compressed as monolithic ZSTD streams. To query a single archived memory, the entire blob must be decompressed.

**Impact:** Archive queries are O(n) in blob size. As archive grows, query latency increases linearly.

**Fix:** Implement chunked archive storage with a lightweight index (concept→offset mapping). Each chunk is independently decompressible.

---

## 6. Extension/Plugin Architecture

### Architecture Assessment: ✅ WELL DESIGNED (Trait-Based)

```
Extension (base trait)
    ├── Skill (active capability: execute, usage_schema)
    ├── Ability (passive background: start/stop/get_state)
    └── Lens (perceptual filter: process_context)
```

**Plus:** Recipe (chained workflow of Skills/Tools)

### Strengths:
- Clear trait hierarchy with `async_trait` for lifecycle
- Permission-based sandbox (`CapabilityManifest`, `PolicyGate`)
- Resource limits and timeouts per extension
- Extension discovery via filesystem scanning

### 🔴 ISSUE 6.1: No Dynamic Loading — Extensions Must Be Compiled In

**File:** `crates/aura-types/src/extensions.rs:109-162`

**Problem:** The `Extension` trait uses `async_trait` which requires monomorphization. Extensions cannot be loaded at runtime from shared libraries. All extensions must be compiled into the daemon binary.

**Impact:** 
- Adding a new extension requires recompiling and redeploying the entire daemon
- No third-party extension ecosystem possible
- Android users cannot install extensions without a full APK update

**Fix:** Define a stable C ABI for extensions:
```rust
#[repr(C)]
pub struct ExtensionVtable {
    pub name: extern "C" fn() -> *const c_char,
    pub execute: extern "C" fn(input: *const c_char) -> *const c_char,
    pub free_string: extern "C" fn(*mut c_char),
}
```
Use `libloading` to load `.so` files at runtime. Wrap in safe Rust API.

### 🟡 ISSUE 6.2: Extension Sandbox Is Incomplete

**File:** `crates/aura-daemon/src/extensions/sandbox.rs` (referenced but not fully analyzed)

**Problem:** The `ExtensionSandbox` is referenced in `mod.rs` but the actual permission enforcement is unclear. The `Extension` trait declares `required_permissions()` but there's no visible enforcement of resource limits (memory, CPU) at the trait level.

**Impact:** A buggy or malicious extension could consume unbounded resources.

**Fix:** Implement actual resource metering:
```rust
pub struct ResourceMeter {
    pub memory_used: AtomicU64,
    pub cpu_time_ms: AtomicU64,
    pub execution_start: Instant,
    pub limits: ResourceLimits,
}
```
Check limits on every extension callback.

### 🟢 ISSUE 6.3: Recipe Steps Are Unvalidated String Templates

**File:** `crates/aura-types/src/extensions.rs:236-240`

```rust
pub struct RecipeStep {
    pub tool_or_skill_id: String,
    pub parameters_template: Value,  // Unvalidated JSON
}
```

**Impact:** Recipe steps reference tool/skill IDs as strings with no compile-time or load-time validation. A typo causes runtime failure.

**Fix:** Validate all recipe steps at load time against the capability registry. Reject recipes with unknown tool IDs.

---

## 7. Build System (NDK, Features, Targets)

### Architecture Assessment: ✅ SOLID with Minor Issues

### Strengths:
- Clean feature flags: `stub`, `voice`, `curl-backend`, `reqwest`, `server`
- Proper `CARGO_CFG_TARGET_OS` usage in build.rs (not `#[cfg]` which reflects host)
- NDK path detection via multiple env vars (`NDK_HOME`, `ANDROID_NDK_HOME`, `ANDROID_NDK_ROOT`)
- `cc` crate for native compilation with proper NEON flags
- LTO set to `thin` (not `true`) to avoid NDK #2073

### 🟡 ISSUE 7.1: llama.cpp Compilation Has Redundant Flags

**File:** `crates/aura-llama-sys/build.rs:85-98`

```rust
c_build
    .flag("-march=armv8.7a+fp16+dotprod")
    .flag("-DGGML_USE_NEON")
    .flag("-DGGML_USE_NEON_FP16=ON")  // Redundant with -DGGML_USE_NEON
    .flag("-DGGML_NATIVE=ON")
    .flag("-DGGML_USE_SVE=OFF")
    .flag("-DGGML_USE_NEON")  // DUPLICATE of line 86
```

**Impact:** `-DGGML_USE_NEON` appears twice. `-DGGML_USE_NEON_FP16=ON` may conflict with the C define approach. Minor: compiles correctly but shows lack of review.

**Fix:** Remove duplicate flag. Verify CMake-equivalent defines are correct.

### 🟡 ISSUE 7.2: No Multi-Architecture Build Support

**File:** `Makefile:23`

```makefile
ANDROID_TARGET:= aarch64-linux-android
```

**Impact:** Only ARM64 Android is supported. ARM32 (`armv7a-linux-androideabi`) and x86_64 Android emulators are not buildable.

**Fix:** Add `ANDROID_TARGETS` list and iterate:
```makefile
ANDROID_TARGETS := aarch64-linux-android armv7a-linux-androideabi x86_64-linux-android
```

### 🟡 ISSUE 7.3: Server Backend Feature Is Hidden

**File:** `crates/aura-llama-sys/Cargo.toml:21`

```toml
server = ["stub"]  # Server implies stub — delegates to llama-server via HTTP
```

**Impact:** Users don't know about the `server` feature for using external llama-server. Not documented in Makefile or README.

**Fix:** Add `server` target to Makefile and document in README.

### 🟢 ISSUE 7.4: Host Build Strip Path Is Linux-Only

**File:** `Makefile:68`

```makefile
"$$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip"
```

**Impact:** macOS developers cannot run `make android-strip` (should be `darwin-x86_64` or `darwin-arm64`).

**Fix:** Detect host OS and select appropriate prebuilt directory.

---

## Summary of Recommended Fixes

### Priority 1 (Blocking Multi-Device Deployment)
| # | Issue | Files | Effort |
|---|-------|-------|--------|
| 🔴 1.1 | Duplicated IPC constants | `protocol.rs`, `ipc_handler.rs` | 2h |
| 🔴 2.1 | Hardcoded Android paths in defaults | `config.rs` | 4h |
| 🔴 2.2 | Hardcoded neocortex binary path | `spawn.rs` | 2h |
| 🔴 6.1 | No dynamic extension loading | `extensions.rs` | 2-3 days |

### Priority 2 (Scalability & Maintainability)
| # | Issue | Files | Effort |
|---|-------|-------|--------|
| 🟡 3.1 | No platform trait abstraction | `platform/mod.rs` | 1 day |
| 🟡 4.1 | Asymmetric async/sync IPC | `protocol.rs`, `ipc_handler.rs` | Document (1h) |
| 🟡 5.1 | Working memory not thread-safe | `memory/mod.rs` | 4h |
| 🟡 5.2 | No memory→model tier feedback | `mod.rs`, `platform/mod.rs` | 4h |
| 🟡 7.1 | Redundant build flags | `build.rs` | 1h |
| 🟡 7.2 | Single-arch build | `Makefile` | 2h |

### Priority 3 (Technical Debt)
| # | Issue | Files | Effort |
|---|-------|-------|--------|
| 🟢 1.3 | SBOM comments misleading | `Cargo.toml` | 30min |
| 🟢 4.3 | Hardcoded TCP port | `protocol.rs` | 1h |
| 🟢 6.3 | Unvalidated recipe steps | `extensions.rs` | 2h |
| 🟢 7.4 | Linux-only strip path | `Makefile` | 1h |

---

## Architecture Strengths (What's Done Right)

1. **Security-First IPC:** Authenticated envelopes with CSPRNG tokens, sequence numbers, rate limiting, protocol versioning
2. **4-Tier Memory System:** Clean separation, WAL-mode durability, cross-tier queries, GDPR compliance
3. **Platform Abstraction (Partial):** Good use of `#[cfg]` for Android-specific code with host stubs
4. **Feature Flags:** Clean feature separation (`stub`, `voice`, `curl-backend`, `reqwest`, `server`)
5. **Teacher Stack:** Sophisticated prompt assembly with chain-of-thought forcing, grammar constraints, importance-based high-stakes gating
6. **Extension Traits:** Well-designed `Extension`/`Skill`/`Ability`/`Lens` hierarchy
7. **Power Management:** Real physics-based model selection (mWh, mA, °C) with thermal zone awareness
8. **ReAct Loop:** Proper agent loop with observation→reasoning→action cycles

---

## Recommended Architecture Evolution

```
Current:                    Recommended:
┌─────────┐                ┌─────────────────────────────────┐
│ Daemon  │                │ Daemon                          │
│ (monolith)│              │ ┌─────────────────────────────┐ │
│         │                │ │ PlatformProvider (trait)     │ │
│         │                │ │ ├── AndroidProvider          │ │
│         │                │ │ ├── HostProvider             │ │
│         │                │ │ └── IosProvider (future)     │ │
│         │                │ └─────────────────────────────┘ │
│         │                │ ┌─────────────────────────────┐ │
│         │                │ │ ExtensionLoader (dynamic)    │ │
│         │                │ │ ├── libloading (.so)         │ │
│         │                │ │ ├── Built-in (compile-time)  │ │
│         │                │ │ └── Remote (future: gRPC)    │ │
│         │                │ └─────────────────────────────┘ │
│         │                │ ┌─────────────────────────────┐ │
│         │                │ │ Memory (thread-safe)         │ │
│         │                │ │ ├── Working: RwLock          │ │
│         │                │ │ ├── Episodic: Arc<Mutex>     │ │
│         │                │ │ └── MemoryPressure→ModelTier │ │
│         │                │ └─────────────────────────────┘ │
└─────────┘                └─────────────────────────────────┘
```

---

*Report generated by 🏗️ Architect Agent — AURA v4 Deep Architecture Review*
