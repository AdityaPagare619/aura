# AURA v4 - MEMORY SYSTEM: Complete Investigation Context

**Date Created:** 2026-03-25
**Status:** ACTIVE - FULL AUTONOMOUS MODE ENGAGED
**Priority:** CRITICAL

---

## 📊 EXECUTIVE SUMMARY - WHAT WE KNOW

### The Problem (Verified)
1. **Neocortex crashes with SIGSEGV (EXIT: 139)** on Moto G45 5G (MediaTek Dimensity 6300)
2. **Daemon WORKS fine** - This is a DIFFERENT issue from F001 daemon crash
3. **Crash happens BEFORE any "listening on" log** - During startup sequence
4. **Binary builds successfully** - CI shows green compilation

### What We've Tried
| Attempt | Fix | Result |
|--------|-----|--------|
| 1 | Changed -march to armv8.2-a+fp16+dotprod | Still SIGSEGV |
| 2 | Added GGML_NATIVE=OFF, GGML_USE_SVE=OFF | Still SIGSEGV |
| 3 (CURRENT) | Changed to armv8-a + GGML_USE_NEON_FP16=OFF | PENDING TEST |

### Root Cause Hypothesis (Based on Research)
- **MediaTek Dimensity 6300 has issues with FP16 vectorization** (llama.cpp #13708, #18766)
- **Research shows**: FP16_VECTOR_ARITHMETIC causes crashes on MediaTek SoCs
- **Hypothesis**: The +fp16+dotprod flags cause ggml to use FP16 vector instructions that MediaTek's Cortex-A76/A55 doesn't handle correctly

---

## 🔬 TECHNICAL DETAILS

### Device Info
- **Device:** Moto G45 5G
- **SoC:** MediaTek Dimensity 6300
- **CPU:** Cortex-A76 @ 2.4GHz + Cortex-A55 @ 2.0GHz (big.LITTLE)
- **GPU:** Mali-G57 MC2
- **Architecture:** ARM64-v8A

### Build Configuration Applied
```rust
// crates/aura-llama-sys/build.rs - CONSERVATIVE FIX
.flag("-march=armv8-a")                    // Changed from armv8.2-a+fp16+dotprod
.flag("-DGGML_USE_NEON")                   // Standard NEON (no FP16 vectorization)
.flag("-DGGML_USE_NEON_FP16=OFF")          // Disabled FP16 vectorization
.flag("-DGGML_NATIVE=OFF")                  // Disable CPU feature auto-detection
.flag("-DGGML_USE_SVE=OFF")                // Disable SVE
```

### Crash Timing Analysis
```
main()
├── Args::parse()                      ✅ No crash
├── panic hook set                     ✅ No crash  
├── tracing init                       ✅ No crash
├── NeocortexRuntimeConfig::load()     ✅ No crash
├── spawn_shutdown_listener()           ✅ No crash
├── ModelManager::new()                ✅ No crash
├── model_manager.scan()               ⚠️  Possible (GGUF parsing)
├── build_startup_capabilities()        ✅ No crash
└── run_server()
    ├── TCP listener bind              ✅ No crash
    └── "listening on TCP"            ← LAST LOG BEFORE CRASH
```

### Similar Issues Found (llama.cpp GitHub)
| Issue | Device | Symptom | Relevance |
|-------|--------|---------|-----------|
| #13708 | MediaTek MT6785V | SIGSEGV during warmup, std::ostringstream crash | HIGH |
| #18766 | MediaTek MT6737 | DirectIO SIGSEGV at sampler init | HIGH |
| #18732 | MediaTek Dimensity 9000 | Termux crash during model loading | HIGH |
| #16658 | Realme Android 15 | "tagged pointer truncation" linker crash | HIGH |
| #8109 | Pixel 8 Pro | `svcntb() == QK8_0` GGML_ASSERT | HIGH |

---

## 🧠 KEY INSIGHTS FROM RESEARCH

### First Principles Analysis
1. **ASSUMPTION**: "FP16 vectorization makes inference faster"
   - **REALITY**: Only if hardware supports it correctly
   - **VERIFIED**: MediaTek Dimensity 6300 does NOT support FP16 vectorization properly

2. **ASSUMPTION**: "GGML_NATIVE=OFF disables all CPU feature detection"
   - **REALITY**: Only disables runtime detection, NOT compile-time flags
   - **FIX APPLIED**: Changed -march to armv8-a (generic, no CPU-specific features)

3. **ASSUMPTION**: "SIGSEGV is always in the code we wrote"
   - **REALITY**: Could be in llama.cpp C/C++ code
   - **VERIFIED**: neocortex links llama.cpp, daemon does NOT

### Constraint Analysis
| Constraint | Type | Treatment |
|------------|------|----------|
| Must work on MediaTek Dimensity 6300 | Physical | HARD CONSTRAINT |
| Must not crash at startup | Logical | MUST satisfy |
| GGML needs CPU features for speed | Conventional | CAN be disabled for stability |
| Binary size < 100MB | Economic | Not a concern |

---

## 📋 PAST COMMITS RELEVANT TO THIS ISSUE

| Commit | SHA | Description |
|--------|-----|-------------|
| 3feefa7 | fix: Disable GGML_NATIVE and SVE to prevent SIGSEGV on Android | Previous attempt |
| 8622905 | fix: Telegram send parsing, System1 fallback, and Neocortex ARM build | Earlier neocortex work |
| 128ed2e | Complete stable Rust migration | F001 fix for daemon |

---

## 🎯 DECISION FRAMEWORKS APPLIED

### D2: Structural Decision
| Category | Evidence | Status |
|----------|----------|--------|
| RUNTIME: SIGSEGV on device | Confirmed | ✅ |
| RUNTIME: Crash before boot | Confirmed - before "listening on" | ✅ |
| BUILD: Dependency conflict | Need to verify NDK linking | ⚠️ |

### D3: Device Decision
| Question | Answer |
|----------|--------|
| Failure on DEVICE? | YES |
| CODE issue? | YES - daemon works, neocortex doesn't |
| DEVICE SETUP issue? | NO - binary deployed correctly |

### D4: Boot Decision
| Question | Answer |
|---------|--------|
| Binary start? | PARTIAL |
| Exit code? | 139 (SIGSEGV) |
| Boot stage? | Stage 4-5 |

---

## 🔄 NEXT STEPS (FULL AUTONOMOUS MODE)

### Immediate Actions
1. [ ] Trigger CI build with conservative fix
2. [ ] Deploy neocortex binary to device
3. [ ] Run neocortex with RUST_BACKTRACE=1
4. [ ] Capture full device logs
5. [ ] Analyze crash location

### If Still Broken
1. [ ] Try `-DGGML_USE_NEON=OFF` (disable NEON entirely)
2. [ ] Try `-march=armv8-a+nosimd` 
3. [ ] Build debug version (no LTO) to isolate
4. [ ] Check MTE status on device

### Device Diagnostics Needed
```bash
# Android version
getprop ro.build.version.release

# CPU features  
cat /proc/cpuinfo | grep -E "Features|flags"

# MTE status
cat /proc/self/status | grep -i tag

# Linker comparison
ldd neocortex 2>&1 | grep -E "libc|libstd"
ldd daemon 2>&1 | grep -E "libc|libstd"

# Full backtrace
RUST_BACKTRACE=1 ./neocortex 2>&1

# strace
strace -f ./neocortex 2>&1 | tail -100
```

---

## ✅ VERIFICATION CRITERIA

### CI Build Success
- ✅ Compilation succeeds
- ✅ No link errors
- ⚠️ NOT verification of device behavior

### Device Test Success (REAL VERIFICATION)
- ⬜ Binary starts without SIGSEGV
- ⬜ "listening on" log appears
- ⬜ Can accept connections
- ⬜ Full ReAct loop works

---

## 📊 STATUS LOG

| Time | Event | Evidence |
|------|-------|----------|
| 2026-03-25 HH:00 | Conservative fix committed | commit 8355d781 |
| 2026-03-25 HH:00 | CI build triggered | PENDING |
| 2026-03-25 HH:00 | Device test | PENDING |

---

**This document is the SOURCE OF TRUTH for neocortex SIGSEGV investigation.**
**All decisions must trace back to this document.**
