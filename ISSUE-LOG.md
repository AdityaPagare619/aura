# AURA Issue Log — Root Cause Analysis

**Purpose:** Every removed/changed file, dependency, or code has a documented reason here.  
**Created:** 2026-03-21  
**Pattern:** GAP-P3 (Platform-Specific) issues on Termux/Android  

---

## Issue: RUSTLS-PLATFORM-VERIFIER PANIC on Termux (FIXED)

### What Was Changed

| Change | Before | After | Date |
|--------|--------|-------|------|
| `rustls-platform-verifier` dep | NOT explicit (transitive via reqwest) | Added explicit `rustls-platform-verifier = "0.6"` | 2026-03-21 |
| `rustls-platform-verifier` dep | Explicit `= "0.6"` | **REMOVED** (transitive only) | 2026-03-21 |
| `main.rs` | No platform verifier init | Added `rustls_platform_verifier::android::init()` | 2026-03-21 |
| `main.rs` | With init call | **REMOVED** init call (wrong hypothesis) | 2026-03-21 |
| `reqwest` features | `rustls-tls` | Feature-gated (`optional = true`) | 2026-03-21 |
| `rustls-platform-verifier` dep | Explicit dep | **REMOVED** entirely | 2026-03-21 |

### What Was Created

| File | Purpose | Date |
|------|---------|------|
| `curl_backend.rs` | HTTP backend using curl subprocess (Termux-compatible) | 2026-03-21 |
| Feature: `curl-backend` | Feature flag to switch between reqwest and curl | 2026-03-21 |

### The Panic Message

```
thread 'main' (30255) panicked at /cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rustls-platform-verifier-0.6.2/src/android.rs:94:10:
Expect rustls-platform-verifier to be initialized
```

### Root Cause

| Aspect | Details |
|--------|---------|
| **Root cause** | `rustup` (not cargo build) panics when syncing toolchain channels over HTTPS |
| **Trigger** | Running `rustup default stable-aarch64-linux-android` on Termux |
| **Why** | Termux reports `target_os = "android"` but has NO JVM (Zygote). rustls-platform-verifier sees Android → tries Android TrustManager → PANIC because no JVM |
| **Affected versions** | All tools using rustls-platform-verifier on Termux (rustup, cargo, AURA reqwest) |
| **Evidence** | GitHub issue #219, users.rust-lang.org thread, Reddit Rust/termux |

### Evidence Links

- **GitHub Issue #219**: https://github.com/rustls/rustls-platform-verifier/issues/219
- **users.rust-lang.org**: "rustup isn't supported on Termux. Use pkg install rust instead"
- **reqwest #2968**: rust-webpki-roots causes crash on Android

### What I Did Wrong (Lessons)

| Mistake | What I Should Have Done |
|---------|----------------------|
| Added explicit `rustls-platform-verifier = "0.6"` | Read the panic path — `/cargo/registry/` = rustup, not my code |
| Added `rustls_platform_verifier::android::init()` | Panic was in RUSTUP, not AURA code — init() was meaningless |
| Didn't search before fixing | Should have searched "rustls-platform-verifier termux panic" first |
| Didn't read the F001 plan | The plan already mentioned "H5: Rust nightly broken" hypothesis |

### The Fix (Two Approaches)

**Approach A: Use curl (Implemented)**
- curl uses OpenSSL on Termux (works perfectly, no platform-verifier)
- `curl_backend.rs` replaces reqwest for Telegram API calls
- Feature flag: `--features curl-backend` for Termux builds

**Approach B: Use pkg-installed rust (Alternative)**
- `pkg install rust` on Termux has patches for this exact issue
- Don't use rustup on Termux
- But pkg rust targets native Termux, not necessarily Android cross-compilation

### System Architecture Impact

AURA is a 5-crate workspace. Changing the HTTP backend has SYSTEM-WIDE implications:

```
aura (workspace)
├── aura-daemon (HTTP = Telegram only)
│   ├── telegram/
│   │   ├── polling.rs    (HttpBackend trait — unchanged)
│   │   ├── reqwest_backend.rs (feature-gated)
│   │   └── curl_backend.rs  (NEW — feature-gated)
│   └── daemon_core/main_loop.rs (wires backend — feature-gated)
├── aura-types    (NO HTTP) → unaffected
├── aura-neocortex (NO HTTP) → unaffected
├── aura-llama-sys (FFI only) → unaffected
└── aura-iron-laws (compile-time) → unaffected
```

**Risk Assessment:**
| Component | Impact | Why |
|----------|--------|-----|
| aura-daemon/telegram | MEDIUM | HTTP backend changed |
| daemon_core/main_loop.rs | LOW | Feature-gated import |
| polling.rs trait | NONE | Trait unchanged, backends implement it |
| Voice pipeline | LOW | Uses same HttpBackend trait |
| Other crates | NONE | No HTTP dependencies |

### Build Configuration Matrix

| Build Context | Target | HTTP Backend | Command |
|--------------|--------|-------------|---------|
| GitHub Actions CI | aarch64-linux-android | reqwest ✅ | `cargo build --release` |
| Termux (rustup) | aarch64-linux-android | **PANICS** ❌ | Use below |
| Termux (pkg rust) | aarch64-linux-android | curl ✅ | `cargo build --features curl-backend --release` |
| Host Linux dev | x86_64-unknown-linux-gnu | reqwest ✅ | `cargo build --release` |
| Android APK (JNI) | aarch64-linux-android | JNI ✅ | Separate build |

### What Needs Testing

1. **CI build (reqwest)** — GitHub Actions, no changes, should work
2. **Termux + curl-backend** — Native build, `cargo build --features curl-backend --release`
3. **Telegram E2E** — Send "Hey Aura" via Telegram app
4. **Voice pipeline** — Record voice, verify multipart upload works
5. **Long polling** — Verify getUpdates polling works with curl

### Why curl_backend.rs Fits

| Factor | Assessment |
|--------|-----------|
| API Compatibility | ✅ Matches `HttpBackend` trait exactly |
| multipart format | ✅ Correct Telegram multipart/form-data |
| GET /getUpdates | ✅ curl -s -X GET |
| POST /sendMessage | ✅ curl -s -X POST -d '{"chat_id":...,"text":...}' |
| POST /sendDocument | ✅ curl multipart with file |
| Performance | ✅ Acceptable (spawns curl subprocess per request) |
| Security | ✅ No extra TLS deps, uses system OpenSSL |
| Termux compatible | ✅ No JVM needed, curl is installed |

### Files Involved

| File | Action | Reason |
|------|--------|--------|
| `reqwest_backend.rs` | KEPT (feature-gated) | Works on CI/Linux, used when `curl-backend` NOT enabled |
| `curl_backend.rs` | CREATED | Uses curl subprocess, works on Termux, no TLS library deps |
| `reqwest` in Cargo.toml | Made optional | Only included when `curl-backend` NOT enabled |
| `rustls-platform-verifier` dep | REMOVED | Was explicitly added (wrong), now transitive only |
| `main.rs` init() call | REMOVED | Was added based on wrong hypothesis |
| `telegram/mod.rs` | Added `curl_backend` module | Exports curl backend |
| `daemon_core/main_loop.rs` | Feature-gated imports | Switches between backends |

---

## Issue: F001 SIGSEGV at Startup (PREVIOUS — Already Fixed)

### Summary

| Aspect | Details |
|--------|---------|
| **Root cause** | NDK Issue #2073 — `panic="abort"` + `lto="thin"` + NDK r26b = toxic combo |
| **Versions affected** | alpha.5 through alpha.8 |
| **Fix** | Changed `lto="thin"` + `panic="unwind"` in Cargo.toml |
| **Commit** | a51ecad |
| **Device test** | EXIT 0 ✅ (binary works on Pixel 7) |
| **Pattern** | GAP-P1: SIGSEGV/panic at startup |

### Files Changed

| File | Change | Date |
|------|--------|------|
| `Cargo.toml` profile.release | `lto = "thin"` + `panic = "unwind"` | 2026-03-19 |
| `rust-toolchain.toml` | `nightly` → `stable` | 2026-03-19 |

---

## Issue: EXIT CODE MISLEADING

### What Happened

```
cargo build --release 2>&1 | tee build.log
# PANIC in output
echo "Build exit code: $?"
Build exit code: 0  # WRONG!
```

### Root Cause

| Aspect | Details |
|--------|---------|
| **Problem** | The `tee` pipe captures stdout/stderr correctly, but `$?` captures the exit code of the LAST command in the pipeline |
| **Last command** | `tee build.log` (exit 0) |
| **Real cargo exit** | Non-zero (panic = failure) |
| **Binary** | NEVER built |

### Lesson

Never trust `echo $?` in a pipeline. Use:
```bash
cargo build --release
echo "Exit code: $?"  # Run separately
```

---

## Pattern: CI ≠ Device

Every issue in AURA so far follows the same pattern:

| Issue | CI (Linux) | Device (Termux/Android) |
|-------|-----------|------------------------|
| F001 SIGSEGV | ✅ Works | ❌ Crashes |
| rustls-platform-verifier | ✅ Works | ❌ Panics |
| Cross-compilation | ✅ Works | ❌ (untested) |

**Rule:** Always test on the actual target environment.

---

## Taxonomy Reference

| Code | Category | Description |
|------|---------|-------------|
| GAP-P1 | SIGSEGV/Panic | Startup crashes, unwinding failures |
| GAP-P2 | Linker | Missing libs, dynamic linking errors |
| GAP-P3 | Platform | NDK, Termux, bionic, JVM issues |
| GAP-P4 | Logic | Wrong output, bypassed policy |
| GAP-P5 | Performance | OOM, startup time |
| GAP-P6 | Network/TLS | HTTPS issues, certificate problems |
| GAP-P7 | Configuration | User input, wrong settings |

---

*Last updated: 2026-03-21*
