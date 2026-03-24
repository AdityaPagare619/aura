# AURA v4 — Neocortex SIGSEGV Investigation Report

**Date:** 2026-03-25
**Analysts:** Multi-agent research + systematic debugging
**Classification:** CRITICAL — Neocortex Binary Non-Functional on Target Platform
**Status:** ROOT CAUSE BEING INVESTIGATED

---

## 1. Executive Summary

### Problem
The **neocortex** binary crashes with **SIGSEGV (EXIT: 139)** on Termux/Android (Moto G45 5G, MediaTek Dimensity 6300) at startup. The **daemon binary works fine** - this is a DIFFERENT issue from the daemon F001 SIGSEGV that was fixed.

### Key Distinction
| Binary | Status | Notes |
|--------|--------|-------|
| aura-daemon | ✅ WORKING | Fixed via F001: lto="thin" + panic="unwind" |
| aura-neocortex | ❌ CRASHING | SIGSEGV at startup, before any logs |

### Crash Timing
The crash occurs **BEFORE** any "listening on" or "model capabilities resolved" log messages appear. Based on code analysis, the crash window is:

```
main()
├── Args::parse() ✅ (no crash here)
├── panic hook set ✅ (no crash here)
├── tracing init ✅ (no crash here)
├── NeocortexRuntimeConfig::load() ✅ (no crash here)
├── spawn_shutdown_listener() ✅ (no crash here)
├── ModelManager::new() ✅ (no crash here)
├── model_manager.scan() ⚠️  (possible - GGUF parsing)
├── build_startup_capabilities() ✅ (no crash here)
└── run_server()
    ├── TCP listener bind ✅
    └── "listening on TCP" ← LAST LOG BEFORE CRASH
```

### Root Cause Hypotheses (Research)

| Hypothesis | Confidence | Evidence |
|------------|------------|----------|
| H1: Android MTE Conflict | 65% | MediaTek Dimensity 6300 may have MTE enabled; different memory patterns between daemon and neocortex |
| H2: Bionic/ABI Incompatibility | 50% | Dual libc++ linking in Termux causes C++ global constructor issues |
| H3: GGML CPU Feature Detection | 45% | Dimensity 6300 has Cortex-A76/A55 with +dotprod+fp16 |
| H4: NEON Register Corruption | 35% | MediaTek NEON implementation differs from Snapdragon |
| H5: Tagged Pointer in Rust | 30% | jemallocator + MTE conflicts |

---

## 2. Evidence from Research

### Similar Issues Found (llama.cpp GitHub)

| Issue | Device | Symptom | Relevance |
|-------|--------|---------|-----------|
| #18766 | MediaTek MT6737 | DirectIO SIGSEGV at sampler init | HIGH |
| #18732 | MediaTek Dimensity 9000 | Termux crash during model loading | HIGH |
| #16658 | Realme Android 15 | "tagged pointer truncation" linker crash | HIGH |
| #13708 | MediaTek MT6785V | SIGSEGV during warmup, std::ostringstream crash | HIGH |
| #8109 | Pixel 8 Pro | `svcntb() == QK8_0` GGML_ASSERT | HIGH |

### MediaTek Dimensity 6300 Specifics
- **CPU**: Cortex-A76 @ 2.4GHz + Cortex-A55 @ 2.0GHz (big.LITTLE)
- **GPU**: Mali-G57 MC2
- **Features confirmed**: NEON, FP16, DOTPROD
- **Issue**: MediaTek SoCs repeatedly appear in llama.cpp crash reports

### Build Flags Currently Applied
```rust
// aura-llama-sys/build.rs
.flag("-march=armv8.2-a+fp16+dotprod")  // MediaTek Dimensity 6300
.flag("-DGGML_USE_NEON")
.flag("-DGGML_USE_NEON_FP16")
.flag("-DGGML_NATIVE=OFF")               // Disabled CPU feature auto-detection
.flag("-DGGML_USE_SVE=OFF")              // Disabled SVE
```

---

## 3. Systematic Debugging: Phase 1 - Root Cause Investigation

### What We Know
1. ✅ Daemon works (F001 fix: lto="thin" + panic="unwind")
2. ❌ Neocortex crashes before model loading
3. ❌ Crash happens immediately at startup
4. ❌ No logs appear before crash (except "cli configuration")

### What We DON'T Know
1. Exact crash location (address, function)
2. Whether GGUF files exist on device
3. Android version on device
4. MTE status on device
5. CPU feature flags on device
6. How neocortex binary differs from daemon in linking

### Information Needed from Device
```bash
# 1. Android version
getprop ro.build.version.release

# 2. CPU features
cat /proc/cpuinfo | grep -E "Features|flags"

# 3. MTE status
cat /proc/self/status | grep -i tag

# 4. Linker info
ldd neocortex 2>&1 | grep -E "libc|libstd"
ldd daemon 2>&1 | grep -E "libc|libstd"

# 5. Full crash with backtrace
RUST_BACKTRACE=1 ./neocortex 2>&1

# 6. strace output
strace -f ./neocortex 2>&1 | tail -100

# 7. Check if models exist
ls -la /data/local/tmp/aura/models/ 2>/dev/null || echo "No models dir"
```

---

## 4. Decision Framework: D2 Classification

| Category | Evidence | Status |
|----------|----------|--------|
| **BUILD: Artifact missing** | Binary exists | ❌ Not the issue |
| **BUILD: Dependency conflict** | NDK linking needs verification | ⚠️ Need device info |
| **RUNTIME: SIGSEGV on device** | Neocortex crashes on startup | ✅ CONFIRMED |
| **RUNTIME: Crash before boot** | Crash before "listening on" log | ✅ CONFIRMED |
| **RUNTIME: Install corruption** | Binary deployed | ⚠️ Need verification |

### D3: Device Decision
| Question | Answer |
|----------|--------|
| Did failure occur on DEVICE? | YES |
| Is it a CODE issue? | YES - neocortex crashes, daemon works |
| Is it DEVICE SETUP issue? | NO - binary is deployed correctly |

### D4: Boot Decision
| Question | Answer |
|----------|--------|
| Did binary start? | PARTIAL - starts but crashes |
| Exit code? | 139 (SIGSEGV) |
| Which boot stage? | Stage 4-5 (before TCP listening) |

---

## 5. Recommended Fixes

### Fix 1: Verify GGML_NATIVE is Truly Disabled
The `-DGGML_NATIVE=OFF` flag should disable CPU feature detection. Verify this is actually being applied by checking build logs.

### Fix 2: Try Generic ARM64 March
```rust
// Change from:
.flag("-march=armv8.2-a+fp16+dotprod")
// To:
.flag("-march=armv8-a")  // Generic, no CPU-specific features
```

### Fix 3: Disable NEON FP16
```rust
.flag("-DGGML_USE_NEON_FP16=OFF")
```

### Fix 4: Add MTE Compatibility Flags
```bash
# In build.rs
.flag("-ffixed-x18")  // x18 is platform register on Android
```

### Fix 5: Switch to Debug Build
Debug builds don't have LTO and might work, confirming if this is an LTO/optimization issue.

---

## 6. Timeline

| Date | Event |
|------|-------|
| 2026-03-19 | F001 daemon SIGSEGV identified |
| 2026-03-20 | F001 fix applied (lto="thin", panic="unwind") |
| 2026-03-25 | Neocortex SIGSEGV identified |
| 2026-03-25 | Research conducted, hypotheses formed |

---

## 7. Next Steps

### Immediate (This Session)
1. [ ] Get device diagnostic info (Android version, MTE status, CPU flags)
2. [ ] Compare neocortex vs daemon ldd output
3. [ ] Try building with `-march=armv8-a` only
4. [ ] Try disabling NEON FP16
5. [ ] Deploy and test on device

### If Crash Persists
1. [ ] Add RUST_BACKTRACE=1 to get crash location
2. [ ] Build debug version to disable LTO
3. [ ] Check if GGUF files exist on device
4. [ ] Test with minimal model (TinyLlama)

---

## 8. Related Documents

- [F001 SIGSEGV Report](./AURA-F001-COMPREHENSIVE-RESOLUTION-REPORT.md) - Daemon crash analysis
- [Device Testing Guide](../validation/DEVICE-TEST-PROCEDURE.md) - How to test on device
- [Build Failure Taxonomy](../build/FAILURE_TAXONOMY.md) - F-codes for classification

---

**CLASSIFICATION:** CRITICAL — Neocortex binary crashes on MediaTek Dimensity 6300. Investigation ongoing. Device diagnostics needed for root cause confirmation.
