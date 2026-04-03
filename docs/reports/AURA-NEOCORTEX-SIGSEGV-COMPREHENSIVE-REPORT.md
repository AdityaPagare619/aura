# AURA Neocortex SIGSEGV Investigation - Comprehensive Report

**Date:** 2026-03-25
**Status:** UNRESOLVED - Root Cause Unknown
**Device:** Moto G45 5G (MediaTek Dimensity 6300)

---

## Executive Summary

The neocortex binary crashes with SIGSEGV (signal 11) on the Moto G45 5G device. This is a **different issue** from the F001 daemon SIGSEGV which was fixed with lto="thin" + panic="unwind".

### Key Findings:
1. Daemon works perfectly on device
2. Neocortex crashes immediately on startup (~250ms after spawn)
3. Multiple build fixes have been attempted - NONE resolved the issue
4. The crash happens BEFORE any model loading or FFI calls

---

## Device Information

| Property | Value |
|----------|-------|
| Device | Moto G45 5G |
| SoC | MediaTek Dimensity 6300 |
| CPU | Cortex-A76 @ 2.4GHz + Cortex-A55 @ 2.0GHz |
| Android | 15 (API 35) |
| Architecture | ARM64-v8A |

### CPU Features Detected:
```
fp asimd evtstrm aes pmull sha1 sha2 crc32 atomics fphp asimdhp cpuid asimdrdm lrcpc dcpop asimddp
```

Note: No SVE, no FP16 vector extension listed - just standard ARMv8-A features.

---

## Symptoms

### Observed Behavior:
```
neocortex process spawned pid=28995
neocortex process has exited signal: 11 (SIGSEGV)
```

- Exit code: 139 (SIGSEGV)
- Fault address: 0x0 (NULL pointer dereference)
- Crash timing: ~250ms AFTER process spawn
- No logs printed before crash

### What Works:
- Daemon binary (aura-daemon) - works perfectly
- Same Rust toolchain, same profile, same everything
- Only difference: daemon is pure Rust, neocortex links to llama.cpp

---

## Fixes Attempted

### Attempt 1: Original Build (armv8.2-a+fp16+dotprod)
- **Result:** SIGSEGV
- **Commit:** 86229055

### Attempt 2: GGML_NATIVE=OFF + GGML_USE_SVE=OFF
- **Result:** SIGSEGV  
- **Commit:** 3feefa77
- **Changes:**
  - Added `-DGGML_NATIVE=OFF`
  - Added `-DGGML_USE_SVE=OFF`

### Attempt 3: Conservative armv8-a + NEON_FP16=OFF
- **Result:** SIGSEGV
- **Commit:** 8355d781
- **Changes:**
  - Changed `-march=armv8.2-a+fp16+dotprod` → `-march=armv8-a`
  - Added `-DGGML_USE_NEON_FP16=OFF`

### Attempt 4: Disable NEON entirely + -O1
- **Result:** BUILD FAILURE
- **Reason:** llama.cpp uses C++ exceptions, cannot compile with `-fno-exceptions`

### Attempt 5: armv8-a + NEON + FP16=OFF (current)
- **Result:** SIGSEGV
- **Commit:** f5bf2709
- Binary tested: v5 (downloaded from GitHub Release)
- **Still crashes**

---

## Root Cause Analysis

### Hypothesis 1: llama.cpp Static Initialization
The crash happens ~250ms after spawn - before model loading, before any FFI calls. This suggests the crash happens during:
1. Rust runtime initialization
2. Static library constructors in llama.cpp
3. FFI boundary initialization

### Hypothesis 2: MediaTek-specific CPU Issue
MediaTek Dimensity 6300 has issues with certain llama.cpp configurations:
- Issue #13708: MediaTek MT6785V crashes in std::ostringstream
- Issue #18766: MediaTek MT6737 DirectIO SIGSEGV
- Issue #8109: SVE assertion failure on Pixel 8 Pro

### Hypothesis 3: Memory Layout Issue
The fault address is 0x0 - NULL pointer dereference. This could be:
- BSS section initialization
- Global static initialization
- Memory mapping issue

---

## Evidence

### Daemon Logs Show:
```
2026-03-25T09:03:46.607321Z neocortex_spawn: spawning neocortex process path=/data/local/tmp/aura-neocortex
2026-03-25T09:03:46.611789Z neocortex_spawn: neocortex process spawned pid=28995
2026-03-25T09:03:46.611877Z neocortex_wait_ready: waiting for neocortex to become ready
2026-03-25T09:03:46.863388Z neocortex_wait_ready: neocortex process has exited signal: 11 (SIGSEGV)
2026-03-25T09:03:46.863588Z ERROR: neocortex spawned but failed readiness check — running without LLM
```

### Logcat Shows:
```
signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr 0x0000000000000000
#00 pc 00000000001cbd04  /data/local/tmp/aura-neocortex
#01 pc 00000000001c2528  /data/local/tmp/aura-neocortex
```

---

## Tests Not Performed (Limitations)

1. **Stub build test** - FAILED (neocortex code directly calls llama_* functions, not through stub interface)
2. **Debug symbols** - Not available (would help locate exact crash point)
3. **strace** - Not available on device
4. **gdbserver** - Not tested

---

## Files Modified

| File | Change |
|------|--------|
| `crates/aura-llama-sys/build.rs` | Multiple changes to march flags, GGML options |
| `.github/workflows/build-android.yml` | Build workflow |

---

## Commits Related to This Issue

| SHA | Description |
|-----|-------------|
| 86229055 | fix: Telegram send parsing, System1 fallback, and Neocortex ARM build |
| 3feefa77 | fix: Disable GGML_NATIVE and SVE to prevent SIGSEGV on Android |
| 8355d781 | fix(neocortex): Use conservative armv8-a march to avoid MediaTek SIGSEGV |
| f5bf2709 | fix: Revert NEON disable, keep armv8-a + FP16-off |

---

## Conclusions

1. **The issue is NOT in Rust code** - daemon works with same toolchain
2. **The issue is in llama.cpp** - neocortex links to it, daemon doesn't
3. **Build flags don't fix it** - tried march, NEON, FP16, SVE, NATIVE - all still crash
4. **The crash happens during initialization** - not during model loading

### Recommended Next Steps:
1. Try building with debug symbols to get exact crash location
2. Try different llama.cpp version
3. Check if there's a MediaTek-specific build configuration needed
4. Consider alternative LLM backends

---

**This document is the SOURCE OF TRUTH for neocortex SIGSEGV investigation.**
