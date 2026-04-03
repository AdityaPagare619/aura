# AURA v4 Architecture Research Report

**Researcher:** Department 10: RESEARCH (@research)
**Date:** 2026-04-02
**Sources:** 15+ authoritative sources (2024-2026)

---

## Table of Contents

1. [Android Permission Handling Patterns](#1-android-permission-handling-patterns)
2. [Rust FFI Best Practices](#2-rust-ffi-best-practices)
3. [IPC Communication Patterns](#3-ipc-communication-patterns)
4. [Deployment Automation](#4-deployment-automation)

---

## 1. Android Permission Handling Patterns

### Current Best Practices (2024-2026)

Android's permission model has evolved significantly. Key changes relevant to AURA:

**Permission Categories:**
- **Normal permissions** (internet, network state) - granted automatically
- **Dangerous permissions** (camera, location, storage) - require runtime request
- **Signature permissions** - only for same-certificate apps
- **Special permissions** (overlay, VPN) - require explicit system settings navigation

**The Runtime Permission Workflow:**
```kotlin
// 1. Check permission status
if (ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA)
    != PackageManager.PERMISSION_GRANTED) {
    // Permission not granted
}

// 2. Show rationale if needed
if (ActivityCompat.shouldShowRequestPermissionRationale(this, Manifest.permission.CAMERA)) {
    // Show explanation dialog before requesting
    showPermissionRationaleDialog()
} else {
    // Request directly
    ActivityCompat.requestPermissions(this, arrayOf(Manifest.permission.CAMERA), CAMERA_REQUEST_CODE)
}

// 3. Handle callback
override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
    if (requestCode == CAMERA_REQUEST_CODE) {
        if (grantResults.isNotEmpty() && grantResults[0] == PackageManager.PERMISSION_GRANTED) {
            // Granted
        } else {
            // Denied - guide user to settings if critical
        }
    }
}
```

### Specific Patterns for AURA

**AURA-Specific Concerns:**
- AURA runs as a **system service** via JNI bridge to Rust daemon
- Native code (Rust) cannot directly request Android permissions
- Permissions must be requested from the **Kotlin Android app layer**, not the native layer

**Recommended Architecture:**
```
┌─────────────────────┐
│  Kotlin Android App │ ◄── Permission requests happen HERE
│  (UI + Permission   │
│   Management)       │
└────────┬────────────┘
         │ JNI Bridge
         ▼
┌─────────────────────┐
│  Rust Daemon        │ ◄── Checks permission status via JNI callback
│  (Native Logic)     │
└─────────────────────┘
```

**Permission Bridge Pattern (Kotlin → Rust):**
```kotlin
// Kotlin side: expose permission status to native code
class NativeBridge {
    companion object {
        init { System.loadLibrary("aura_daemon") }
    }
    
    // Called by Rust to check if a permission is granted
    @JvmStatic
    fun hasPermission(permission: String): Boolean {
        return ContextCompat.checkSelfPermission(context, permission) ==
            PackageManager.PERMISSION_GRANTED
    }
    
    // Called by Rust when it needs a permission
    @JvmStatic
    fun requestPermission(permission: String) {
        // Trigger UI permission flow
        // Result sent back to Rust via callback
    }
}
```

**Just-in-Time Permissions:**
- Don't request all permissions at app launch
- Request storage permission only when user tries to save
- Request network permission only when model download is initiated
- This reduces prompt fatigue and increases approval rates

### Common Pitfalls

1. **Assuming permanent permission** - Users can revoke at any time; always check before use
2. **Requesting at launch** - Prompt fatigue leads to uninstalls
3. **Ignoring `shouldShowRequestPermissionRationale`** - Results in poor UX
4. **Not handling "Don't ask again"** - Need graceful degradation to settings redirect
5. **Caching permission status** - Android expects fresh checks each time
6. **Permission group leaks** - Third-party libraries may add permissions you don't need

### Recommendations for AURA

| Recommendation | Priority | Effort |
|---|---|---|
| Implement permission bridge: Kotlin requests, status flows to Rust | HIGH | Medium |
| Just-in-time permission requests | HIGH | Low |
| Clear rationale dialogs for each permission | MEDIUM | Low |
| Audit manifest for permission bloat from dependencies | HIGH | Low |
| Handle permission revocation gracefully in daemon | MEDIUM | Medium |
| Test across Android 10-15 permission changes | HIGH | Medium |

---

## 2. Rust FFI Best Practices

### Current Best Practices (2024-2026)

AURA has **53 unsafe blocks** and **7 unsafe `impl Send`** types wrapping llama.cpp. The FFI boundary is the highest-risk area.

**Core Principle: Isolate unsafety in minimal wrapper types**

```rust
// Pattern 1: Opaque handle with Drop for cleanup
pub struct SafeHandle {
    ptr: *mut ffi::LlamaModel,
}

// Safety: llama.cpp documents thread-safe model loading
// MUST have verified this against llama.cpp docs
unsafe impl Send for SafeHandle {}
unsafe impl Sync for SafeHandle {}

impl SafeHandle {
    pub fn load(path: &str) -> Result<Self, LlamaError> {
        let c_path = CString::new(path)?;
        let ptr = unsafe { ffi::llama_load_model(c_path.as_ptr()) };
        if ptr.is_null() {
            return Err(LlamaError::from_last_error());
        }
        Ok(Self { ptr })
    }
}

impl Drop for SafeHandle {
    fn drop(&mut self) {
        unsafe { ffi::llama_free_model(self.ptr) };
    }
}
```

**Pattern 2: Lifetime-bounded references with PhantomData**
```rust
pub struct Context<'a> {
    ptr: *mut ffi::LlamaContext,
    _marker: PhantomData<&'a SafeHandle>, // Bound to model lifetime
}

impl<'a> Context<'a> {
    pub fn new(model: &'a SafeHandle) -> Result<Self, LlamaError> {
        let ptr = unsafe { ffi::llama_new_context(model.ptr) };
        // ...
    }
}
```

**Pattern 3: Thread-safe callbacks via Mutex + Arc**
```rust
struct CallbackRegistry {
    callbacks: Mutex<HashMap<EventType, Arc<dyn Fn(Event) + Send + Sync>>>,
}

extern "C" fn raw_callback(event_type: i32, data: *mut c_void) {
    let registry = unsafe { &*(data as *const CallbackRegistry) };
    let map = registry.callbacks.lock().unwrap();
    if let Some(cb) = map.get(&event_type.into()) {
        cb(/* ... */);
    }
}
```

### Send/Sync for FFI Wrappers - The Critical Pattern

AURA's 7 unsafe `impl Send` types need careful audit. The rules:

```rust
// CORRECT: C library explicitly documents thread safety
unsafe impl Send for LlamaModel {}  // llama.cpp: "Models can be shared across threads"

// DANGEROUS: C library uses thread-local state
unsafe impl Send for LlamaContext {}  // ⚠️ llama.cpp contexts may NOT be Send!

// SAFER: Use per-thread contexts
pub struct ThreadSafeContext {
    inner: Mutex<*mut ffi::LlamaContext>, // Serialize access
}
unsafe impl Send for ThreadSafeContext {}
```

**Audit Checklist for each `unsafe impl Send`:**
1. Does the C library documentation explicitly support cross-thread usage?
2. Does the C library use thread-local storage internally?
3. Is the wrapped pointer's lifetime managed correctly?
4. Are there any interior mutability concerns?

### Pointer Lifetime Management

```rust
// Pattern: Never expose raw pointers in safe API
pub struct InferenceResult {
    tokens: Vec<Token>,  // Copied out, no dangling pointer
    // NOT: *const ffi::TokenResult  ← dangling risk
}

impl InferenceResult {
    pub fn from_raw(raw: *const ffi::TokenResult, len: usize) -> Self {
        let tokens = unsafe {
            std::slice::from_raw_parts(raw, len)
                .iter()
                .map(|t| Token::from_raw(t))
                .collect()
        };
        // Free the C-side allocation AFTER copying
        unsafe { ffi::free_result(raw) };
        Self { tokens }
    }
}
```

### Reducing unsafe Block Count

**Strategy for AURA's 53 unsafe blocks:**

| Category | Count (est.) | Reduction Strategy |
|---|---|---|
| Model loading/freeing | ~6 | Wrapper struct with Drop |
| Context creation | ~4 | Lifetime-bound Context type |
| Inference calls | ~15 | Safe wrapper returning Vec |
| Tokenization | ~8 | CString handling in wrapper |
| Sampler operations | ~10 | State machine wrapper |
| Misc state access | ~10 | Encapsulate in safe methods |

**Target:** Reduce from 53 to ~15-20 by moving all unsafe into wrapper methods.

### Common Pitfalls

1. **Exposing `*mut c_void` in public API** - Always wrap in safe type
2. **Forgetting Drop** - Every `ffi::*_create` needs corresponding `ffi::*_free`
3. **Assuming Send without documentation** - Verify with C library docs
4. **Null pointer dereference** - Always check before use
5. **String lifetime mismatch** - CString must outlive the C call
6. **Double-free on error paths** - Use `ManuallyDrop` for partial initialization

### Recommendations for AURA

| Recommendation | Priority | Effort |
|---|---|---|
| Audit all 7 `unsafe impl Send` against llama.cpp thread safety docs | CRITICAL | High |
| Create `SafeModel`, `SafeContext`, `SafeSampler` wrapper types | HIGH | High |
| Reduce unsafe blocks from 53 to target <20 | HIGH | Medium |
| Use `bindgen` for automated FFI generation where possible | MEDIUM | Medium |
| Implement comprehensive FFI test suite (thread safety) | HIGH | Medium |
| Document every unsafe block with safety justification | HIGH | Low |

---

## 3. IPC Communication Patterns

### Current Best Practices (2024-2026)

AURA uses Unix domain sockets between `aura` daemon and `neocortex` binary. There are several IPC options in the Rust ecosystem:

**IPC Comparison for AURA:**

| Method | Latency | Complexity | Throughput | Cross-platform |
|---|---|---|---|---|
| Unix Domain Sockets | ~10μs | Low | High | Unix/Android only |
| Shared Memory (mmap) | ~100ns | High | Very High | Limited |
| Named Pipes | ~100μs | Low | Medium | Windows |
| tokio-unix-ipc | ~15μs | Low | High | Unix/Android |
| ipc_ring (ring buffer) | ~50ns | Medium | Very High | Unix |

**Recommended for AURA: Unix Domain Sockets with tokio-unix-ipc**

```rust
// Using tokio-unix-ipc for typed async IPC
use tokio_unix_ipc::{channel, Sender, Receiver};

// Shared message types
#[derive(Serialize, Deserialize)]
enum DaemonMessage {
    InferenceRequest { prompt: String, max_tokens: u32 },
    HealthCheck,
    Shutdown,
}

#[derive(Serialize, Deserialize)]
enum DaemonResponse {
    InferenceResult { tokens: Vec<String> },
    HealthOk,
    Error(String),
}

// Daemon side (server)
async fn handle_ipc(socket_path: &str) -> Result<()> {
    let (sender, receiver) = channel::<DaemonResponse, DaemonMessage>(socket_path)?;
    
    loop {
        let msg = receiver.recv().await?;
        match msg {
            DaemonMessage::InferenceRequest { prompt, max_tokens } => {
                let result = run_inference(&prompt, max_tokens).await;
                sender.send(DaemonResponse::InferenceResult { tokens: result }).await?;
            }
            // ...
        }
    }
}
```

**Advanced: Zero-Copy Shared Memory for High-Throughput**

For streaming inference results (token-by-token), shared memory ring buffers can be 100x faster:

```rust
// Using memory-mapped ring buffer for streaming
use memmap2::MmapMut;
use std::sync::atomic::{AtomicUsize, Ordering};

struct SharedRingBuffer {
    mmap: MmapMut,
    head: *const AtomicUsize,  // Writer position
    tail: *const AtomicUsize,  // Reader position
    data: *mut u8,             // Buffer data
    capacity: usize,
}

impl SharedRingBuffer {
    // SPSC (Single Producer, Single Consumer) lock-free
    pub fn push(&self, data: &[u8]) -> Result<(), TryPushError> {
        let head = (*self.head).load(Ordering::Relaxed);
        let tail = (*self.tail).load(Ordering::Acquire);
        // ... ring buffer logic with atomics
    }
}
```

### AURA-Specific IPC Architecture

```
┌──────────────────┐    Unix Socket    ┌──────────────────┐
│  aura daemon     │◄────────────────►│  neocortex       │
│  (Rust)          │                   │  (Rust binary)   │
│                  │   Control plane:  │                  │
│  - Model loading │   JSON/bincode    │  - Inference     │
│  - Resource mgmt │                   │  - Tokenization  │
│  - Health check  │   Data plane:     │  - Sampling      │
│                  │   Shared memory   │                  │
└──────────────────┘   (optional)      └──────────────────┘
        ▲
        │ JNI
        ▼
┌──────────────────┐
│  Android Kotlin  │
│  App             │
└──────────────────┘
```

**Recommended IPC Protocol:**
```
Control Plane (Unix socket with serde/bincode):
  - Commands: LoadModel, UnloadModel, HealthCheck, Shutdown
  - Responses: Ok, Error, Status

Data Plane (for streaming inference):
  - Option A: Unix socket with streaming (simpler)
  - Option B: Shared mmap ring buffer (faster, ~50ns overhead)
```

### Crates for AURA's IPC

| Crate | Use Case | Recommendation |
|---|---|---|
| `tokio-unix-ipc` | Typed async IPC with file handle passing | PRIMARY for control plane |
| `interprocess` | Cross-platform local sockets | Alternative if Linux-only becomes multi-platform |
| `ipc_ring` | Lock-free SPSC ring buffer | For streaming inference results |
| `bincode` / `rmp-serde` | Message serialization | Use with tokio-unix-ipc |
| `shared_memory` | mmap-based shared memory | For zero-copy data transfer |

### Common Pitfalls

1. **Socket path collision** - Use unique paths per daemon instance
2. **Stale socket files** - Clean up on startup
3. **Buffer overflow in streaming** - Implement backpressure
4. **No reconnection logic** - Handle daemon restart gracefully
5. **Large message blocking** - Chunk large payloads or use shared memory
6. **Missing timeouts** - IPC calls can hang forever without deadlines

### Recommendations for AURA

| Recommendation | Priority | Effort |
|---|---|---|
| Adopt `tokio-unix-ipc` for control plane IPC | HIGH | Medium |
| Define typed message protocol (Request/Response enums) | HIGH | Low |
| Implement reconnection logic for daemon restart | HIGH | Medium |
| Consider `ipc_ring` for streaming inference if latency-critical | MEDIUM | Medium |
| Add IPC health check with configurable timeout | HIGH | Low |
| Unique socket paths per daemon instance (PID-based) | MEDIUM | Low |

---

## 4. Deployment Automation

### Current Best Practices (2024-2026)

AURA targets Android (Termux), Linux desktop, with potential expansion. Deployment for Rust binaries on Android has specific challenges.

**Cross-Compilation Setup:**

```bash
# Install targets
rustup target add aarch64-linux-android
rustup target add armv7-linux-androideabi
rustup target add x86_64-linux-android

# Using cargo-ndk (recommended for Android)
cargo ndk --platform 30 -t aarch64-linux-android build --release

# Configure .cargo/config.toml
[target.aarch64-linux-android]
linker = "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android30-clang"
```

**Known NDK Issues:**
- NDK r25+: `libgcc.a` replaced by `libunwind.a` - copy and rename to fix `-lgcc not found`
- Rust 1.82+: Android API level minimum raised to 21, fixing backtrace support
- LTO + jemalloc: `pthread_atfork` duplicate symbol - use weak symbol version or update jemallocator

**Binary Size Optimization:**
```toml
# Cargo.toml - production profile
[profile.release]
opt-level = "z"       # Optimize for size
lto = true            # Link-time optimization
codegen-units = 1     # Single codegen unit for max optimization
strip = true          # Strip debug symbols (3-4x size reduction)
panic = "abort"       # Smaller binary, no unwinding
```

### Termux Deployment

For Termux specifically, Rust binaries can be deployed in two ways:

**Option A: Termux Package (Official)**
- Build using termux-packages build system
- Creates `.deb` package installable via `pkg install`
- Proper dependency management
- Slower release cycle (community-maintained)

**Option B: Self-Contained Binary**
- Cross-compile with NDK
- Push binary to device directly
- Faster deployment, but manual dependency management
- Need to handle shared library paths (LD_LIBRARY_PATH)

```bash
# Self-contained deployment script
cargo ndk --platform 28 -t aarch64-linux-android build --release
adb push target/aarch64-linux-android/release/aura /data/data/com.termux/files/usr/bin/
adb push target/aarch64-linux-android/release/neocortex /data/data/com.termux/files/usr/bin/
# Push model files
adb push models/ /data/data/com.termux/files/home/.aura/models/
```

### Update Mechanisms

**Pattern 1: Version-Tagged Binary with Checksum**
```rust
// Built into daemon
const VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_HASH: &str = env!("GIT_HASH");

struct UpdateInfo {
    version: String,
    download_url: String,
    sha256: String,
    min_android_api: u32,
}

async fn check_for_updates() -> Result<Option<UpdateInfo>> {
    let response = reqwest::get("https://api.github.com/repos/.../releases/latest").await?;
    // Compare versions, return update info if newer available
}

async fn apply_update(info: &UpdateInfo) -> Result<()> {
    let binary = download_binary(&info.download_url).await?;
    verify_checksum(&binary, &info.sha256)?;
    atomic_replace_binary(&binary)?;  // Write to temp, rename over old
    Ok(())
}
```

**Pattern 2: A/B Slot Updates (for critical deployments)**
```
/data/data/com.termux/files/usr/bin/aura        ← Active (Slot A)
/data/data/com.termux/files/usr/bin/aura.bak     ← Previous (Slot B)
/data/data/com.termux/files/usr/bin/aura.pending ← Update candidate

Flow:
1. Download to .pending
2. Verify checksum
3. Rename current → .bak
4. Rename .pending → active
5. Restart daemon
6. If crash, rollback: rename .bak → active
```

**Pattern 3: GitHub Releases with CI/CD**
```yaml
# .github/workflows/release.yml
name: Build & Release
on:
  push:
    tags: ['v*']

jobs:
  build-android:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-linux-android,armv7-linux-androideabi,x86_64-linux-android
      - name: Install NDK
        run: |
          echo "y" | $ANDROID_HOME/cmdline-tools/bin/sdkmanager --install "ndk;25.2.9519653"
      - name: Build
        run: |
          cargo install cargo-ndk
          cargo ndk --platform 28 -t aarch64-linux-android build --release
          strip target/aarch64-linux-android/release/aura
      - name: Upload Release
        uses: softprops/action-gh-release@v1
        with:
          files: target/aarch64-linux-android/release/aura
```

### Rollback Strategy

```rust
// Daemon implements health check after update
struct DeploymentManager {
    backup_path: PathBuf,
    active_path: PathBuf,
    health_check_timeout: Duration,
}

impl DeploymentManager {
    async fn deploy_with_rollback(&self, new_binary: &[u8]) -> Result<()> {
        // 1. Backup current
        std::fs::copy(&self.active_path, &self.backup_path)?;
        
        // 2. Write new binary
        let temp = self.active_path.with_extension("tmp");
        std::fs::write(&temp, new_binary)?;
        std::fs::rename(&temp, &self.active_path)?;
        
        // 3. Health check
        match self.verify_health().await {
            Ok(()) => {
                log::info!("Update successful");
                Ok(())
            }
            Err(e) => {
                log::error!("Health check failed: {}, rolling back", e);
                std::fs::copy(&self.backup_path, &self.active_path)?;
                Err(e)
            }
        }
    }
}
```

### Cross-Platform Build Matrix

| Target | Platform | Use Case |
|---|---|---|
| `aarch64-linux-android` | Android 64-bit ARM | Primary Android target |
| `armv7-linux-androideabi` | Android 32-bit ARM | Older devices |
| `x86_64-linux-android` | Android x86_64 | Emulator testing |
| `x86_64-unknown-linux-gnu` | Linux x86_64 | Desktop Linux |
| `aarch64-unknown-linux-gnu` | Linux ARM64 | Raspberry Pi, servers |

### Binary Size Strategies from GreptimeDB Edge

A production pattern for managing binary size vs debuggability:

1. **Build two versions:**
   - Stripped binary for production deployment (minimal size)
   - Unstripped binary with symbols stored in cloud/CI
   
2. **Stack trace recovery:**
   - Log base address of loaded objects at startup
   - On panic, log memory addresses
   - Offline: use `addr2line` with unstripped binary to recover symbols
   - `addr2line -e aura_unstripped 0x29ba4` → file:line info

### Common Pitfalls

1. **Wrong ABI folder names** - Must be `arm64-v8a` not `arm64`
2. **Missing strip** - Binary 3-4x larger than necessary
3. **No rollback on failed update** - Bricks the installation
4. **ProGuard stripping JNI classes** - Add keep rules for native-bridged classes
5. **NDK version mismatches** - Pin NDK version in CI
6. **Backtrace broken on Android** - Need Rust ≥1.82 for proper support

### Recommendations for AURA

| Recommendation | Priority | Effort |
|---|---|---|
| Set up cargo-ndk cross-compilation in CI | HIGH | Medium |
| Implement A/B slot update with rollback | HIGH | High |
| Dual binary strategy (stripped + symbol server) | MEDIUM | Medium |
| Automated GitHub Releases with Android targets | HIGH | Medium |
| Pin NDK version and document build setup | HIGH | Low |
| Build for all 3 Android ABIs + Linux x86_64 | MEDIUM | Low |
| Health check endpoint for post-update verification | HIGH | Low |

---

## Summary: Priority Matrix

| Area | Critical Items | Estimated Total Effort |
|---|---|---|
| **Android Permissions** | Permission bridge architecture, just-in-time requests | 2-3 weeks |
| **Rust FFI Safety** | Audit Send impls, reduce unsafe count, wrapper types | 3-4 weeks |
| **IPC Patterns** | Adopt tokio-unix-ipc, typed protocol, reconnection | 2-3 weeks |
| **Deployment** | Cross-compilation CI, A/B updates, GitHub Releases | 3-4 weeks |

**Recommended Implementation Order:**
1. **FFI Safety Audit** (highest risk - 53 unsafe blocks)
2. **IPC Protocol Design** (foundational for daemon↔neocortex)
3. **Deployment CI** (enables rapid iteration)
4. **Android Permissions** (needed before Play Store / Termux distribution)

---

## Sources

1. "Advanced Rust FFI Patterns: Safe Wrappers, Zero-Copy Transfers" - EliteDev.in (Jul 2025)
2. "How to Create Safe FFI Bindings in Rust" - OneUptime Blog (Jan 2026)
3. "Android Permissions Model: Secure Usage & Request Rationale" - SecureCodingPractices (Jul 2025)
4. "Mastering Android Permissions in 2025" - Stackademic (Jun 2025)
5. "Rust on Android - Lessons from the Edge" - Greptime Blog (Apr 2025)
6. "Compiling Rust libraries for Android apps: a deep dive" - Guillaume Endignoux (Oct 2022, updated)
7. "IPC with Rust on Unix, Linux, and macOS" - Medium (Jan 2026)
8. "Beyond FFI: Zero-Copy IPC with Rust and Lock-Free Ring-Buffers" - DEV.to (Dec 2025)
9. `tokio-unix-ipc` crate documentation - docs.rs
10. `interprocess` crate documentation - docs.rs
11. `ipc_ring` crate documentation - crates.io (Sep 2025)
12. Rust FFI Omnibus - jakegoulding.com
13. Rustonomicon: FFI chapter - doc.rust-lang.org
14. "Best practices: Send and Sync for interior mutability" - Rust Users Forum (Oct 2024)
15. `cross-rs` GitHub repository - cross-compilation toolchain
