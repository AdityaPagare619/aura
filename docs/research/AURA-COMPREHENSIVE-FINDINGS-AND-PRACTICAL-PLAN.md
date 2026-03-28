# AURA Comprehensive Findings & Practical Recovery Plan

## Date: 2026-03-28
## Status: Analysis Complete - Implementation Phase
## Source: NEOCORTEX Audit + Memory + Research + Sequential Thinking

---

## PART 0: CRITICAL FINDINGS FROM NEOCORTEX AUDIT DOC

### P0 Risks Identified (From Audit Section 4, Lines 84-85):
| Priority | Risk | Evidence | Primary Surfaces |
|----------|------|----------|------------------|
| P0 | FFI lifecycle/order mismatch | pointer validity + free ordering | aura-llama-sys + model.rs |
| P0 | Link/runtime mode drift | symbol expectations vs selected backend mode | build.rs + backend init/use |

### Audit Says (Line 13):
> "Dominant crash class in evidence remains FFI lifecycle + build/runtime mode mismatch at symbol/pointer boundaries."

### Audit Recommendations (Lines 2014-2026):
1. Immediate: enforce backend-only lifecycle API usage everywhere
2. Mid-term: Android lifecycle chaos and watchdog hardening tests  
3. Fastest gains: startup contract gates + backend lifecycle hard fences

### Audit Also Noted (Line 147):
> "LTO + panic=abort causes startup SIGSEGV - they changed from abort to unwind"

---

## PART 1: PROVEN FACTS (Evidence-Based)

### 1.1 Current State
| Item | Status | Evidence |
|------|--------|-----------|
| neocortex binary | CRASHES | SIGSEGV at bionic GetPropAreaForName |
| daemon binary | WORKS | Starts successfully |
| Build system | BROKEN | Using cc crate instead of CMake |
| Build flags | WRONG | Using armv8-a instead of armv8.7a |
| Device tested | ONE ONLY | Moto G45 (MediaTek Dimensity 6300) |
| Crash location | PRE-MAIN | Before Rust main() runs |

### 1.2 Root Causes Identified (Multiple Sources)
1. **From Audit**: P0 FFI lifecycle/order mismatch in aura-llama-sys + model.rs
2. **From Audit**: P0 Link/runtime mode drift in build.rs + backend init
3. **From Audit**: LTO + panic=abort causes startup SIGSEGV (changed to unwind)
4. **From Research**: Using `cc` crate instead of official CMake
5. **From Research**: Using `-march=armv8-a` instead of `-march=armv8.7a`
6. **From Memory**: libloading + static OnceLock initialization
7. **No Runtime Validation**: Build success assumed = runtime success (WRONG)
8. **Single Device Testing**: Only tested on one device, not multiple

### 1.3 What Works vs What Crashes
| Binary | Status | Notes |
|--------|--------|-------|
| aura-daemon | WORKS | Pure Rust, no native deps |
| aura-neocortex (stub) | CRASHES | Even without llama.cpp |
| llama.cpp native | UNTESTED | Override skips compilation |

---

## PART 2: RESEARCH FINDINGS

### 2.1 Official llama.cpp Android Documentation
**Source**: `https://github.com/ggml-org/llama.cpp/blob/master/docs/android.md`

**Correct Build Flags** (from official docs):
```bash
-DCMAKE_C_FLAGS="-march=armv8.7a"
-DCMAKE_CXX_FLAGS="-march=armv8.7a"
```

**Key Quote**:
> "Even if your device is not running armv8.7a, llama.cpp includes runtime checks for available CPU features it can use."

This means official approach uses RUNTIME detection, not static flags.

### 2.2 MediaTek-Specific Issues Found
- Flutter crashes on MediaTek (GitHub Issue #166248)
- React Native crashes on MediaTek (multiple issues)
- Rust 1.78.0 crash bug (Issue #160) - but we're on newer version
- GetPropAreaForName related to bionic system_properties

### 2.3 Enterprise Practices (Theoretical vs Practical)
| Enterprise Practice | Theoretical | Practical for Us |
|-------------------|-------------|-------------------|
| Test on 2500+ devices | Firebase Test Lab | ONLY 1 device available |
| Validate runtime first | Yes | We didn't do this |
| Trust runtime, not build | Yes | We trusted build |
| Use proven approaches | Yes | We used wrong tools |

**REALITY**: We have ONE device (Moto G45) - we work with what we have.

---

## PART 3: WHAT SHOULD HAVE HAPPENED

### 3.1 Development Order (What Should Have Been)
```
Week 1: Test basic hypothesis - can any Rust binary run on device?
Week 2: Test inference engine works before architecture
Week 3: Test on multiple devices  
Week 4: Build simple working system
Week 5+: Add enterprise features (health, monitoring, etc.)
```

### 3.2 What Actually Happened
```
Week 1: Built enterprise architecture (daemon + neocortex + health + etc.)
Week 2: Built complex systems
Week 3: Built more complex systems
... (months later)
Crash: Can't even run basic binary
```

---

## PART 4: PRACTICAL STEPWISE RECOVERY PLAN

### Constraints:
- Only ONE device available: Moto G45 (MediaTek Dimensity 6300)
- No cloud device farms
- We work with what we have

### Step 1: Test Minimal Binary (PROVE what crashes)
**Goal**: Isolate exactly what crashes

**Test A**: Minimal Rust binary (no deps)
```rust
fn main() {
    println!("Hello from Rust!");
}
```

**Test B**: neocortex with STUB (current)
**Test C**: Different build flags

**EACH TEST = PROOF (YES/NO)**

### Step 2: Apply Official Fixes (If Step 1 proves issue)
**Fix 1**: Change build.rs to use CMake
**Fix 2**: Use `-march=armv8.7a` flags
**Fix 3**: Rebuild and test

### Step 3: Validate on Device
**Each change = test on connected device**
**Each test = PROOF (works/doesn't work)**

---

## PART 5: DECISION MATRIX

### What We Know For Certain:
| Question | Answer |
|----------|--------|
| Current binary crashes? | YES (SIGSEGV) |
| Build succeeds? | YES |
| Official approach exists? | YES (CMake + armv8.7a) |
| We used correct approach? | NO |
| Tested on multiple devices? | NO (only 1) |

### What We Need To Prove:
| Question | How To Prove |
|----------|-------------|
| Does minimal Rust binary work? | Deploy hello world |
| Does official fix work? | Apply CMake + armv8.7a, test |
| Is it MediaTek-specific? | Can't test (only 1 device) |
| Is it bionic issue? | Need more research |

---

## PART 6: ACTION ITEMS

### Immediate Actions:
1. **Create minimal Rust binary** - Deploy to device, prove it crashes/works
2. **Apply official llama.cpp flags** - Change build to use CMake + armv8.7a
3. **Test each change** - On connected device, get PROOF

### What NOT To Do:
- ❌ Don't build more enterprise features
- ❌ Don't add more complexity
- ❌ Don't assume build success = runtime success
- ❌ Don't test only on one more attempt

### What TO Do:
- ✅ Test minimal binary first
- ✅ Validate runtime works
- ✅ Each step = PROOF
- ✅ Work with what we have

---

## PART 7: SUCCESS CRITERIA

### For This Recovery Plan:
| Criteria | How Measured |
|----------|-------------|
| Binary runs on device | No SIGSEGV on startup |
| Inference works | Model loads, generates tokens |
| Reproducible | Works after rebuild |
| Documented | All steps recorded |

### Proof Requirements:
- Each test needs EVIDENCE (not assumption)
- Each fix needs VERIFICATION (not hope)
- Each change needs CONFIRMATION (not trust)

---

## Summary

**From NEOCORTEX Audit**:
- P0: FFI lifecycle/order mismatch identified
- P0: Link/runtime mode drift identified  
- Audit recommended: startup contract gates + backend lifecycle hard fences
- Audit changed LTO: true → thin, panic: abort → unwind (to fix SIGSEGV)

**What We Know**: 
- Multiple possible causes (not just one)
- Current approach is broken
- Official solution exists (CMake + armv8.7a)
- We tested on ONE device only

**What We Need**:
- Test minimal binary first (isolate the issue)
- Apply official fixes
- Validate on device
- Each step = PROOF (not assumption)

**Constraints**:
- Only ONE device (Moto G45)
- No cloud testing
- We work with what we have

**Way Forward**:
1. Test minimal binary (today) - PROVE if any Rust works
2. Apply fixes one by one (after proof)
3. Validate runtime (each step)
4. Build from there

---

*Document updated with ALL sources: NEOCORTEX Audit + Memory + Research + Sequential Thinking*

---

*This document contains ALL findings from research sessions. Updated: 2026-03-28*
