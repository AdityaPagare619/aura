# COMPREHENSIVE EVIDENCE SUMMARY - NO VERDICTS
## Research findings for AURA Android Inference (Evidence Only)

**Date**: March 27, 2026  
**Purpose**: Evidence gathering only - NO decisions, NO verdicts  

---

## WHAT WE KNOW (VERIFIED FACTS)

### Our Situation
- Binary crashes at startup on Moto G45 5G (MediaTek Dimensity 6300)
- Crash is at bionic level: `GetPropAreaForName` in `contexts_serialized.cpp`
- This is during process initialization, NOT during inference
- We've tried 9+ fix attempts over 2+ weeks

### What We Tried (Chronologically)
1. Feature paradox fix (server=stub)
2. Build.rs diagnostics
3. Hard skip native compile
4. Drop to backend abstraction
5. Debug symbols + addr2line (found root cause)
6. -crt-static flag
7. Remove -crt-static
8. -C link-args=-lc
9. Custom target JSON

---

## EVIDENCE FROM OTHERS (NOT ASSUMPTIONS)

### Evidence 1: llama.cpp DOES Work on Android

**Source**: Multiple tutorials, GitHub repos, Reddit discussions

| Evidence | Source | Credibility |
|----------|--------|-------------|
| llama.cpp-android tutorials exist (154 stars) | GitHub JackZeng0208 | High |
| Official llama.cpp Android example in repo | `examples/llama.android` | High |
| PR #4926 merged - Android starter project | ggml-org/llama.cpp | High |
| Developer spent months on JNI integration | Reddit r/androiddev (16 days ago) | High |
| Successfully runs Gemma 3 1B, Qwen 2.5 3B | Same Reddit post | High |

**Quote from developer who actually did it**:
> "dont try to build llama.cpp with the default NDK cmake setup. use the llama.cpp cmake directly and just wire it into your gradle build. saves hours of debugging"

### Evidence 2: OEM-Specific Issues Are Known

**Source**: Same Reddit post - real experience

> "memory mapping behaves differently across OEMs. samsung and pixel handle mmap differently for large files (3GB+ model weights). test on both"

This explains why MediaTek might have issues while other chips work!

### Evidence 3: Other Challenges (Not Just Us)

**Source**: Same Reddit post

- Android kills background processes - need foreground service
- Thermal throttling (30% throughput loss after 30s on Tensor G3)
- JNI string handling expensive - batch tokens

---

## WHAT WE DON'T KNOW (QUESTIONS)

### Question 1: Build Approach
We use `cc` crate + `build.rs` to compile llama.cpp.  
The successful developer says: "use the llama.cpp cmake directly"  

**EVIDENCE NEEDED**: Is our build approach fundamentally wrong?

### Question 2: OEM Memory Mapping
The Reddit post mentions OEM-specific mmap behavior.  

**EVIDENCE NEEDED**: Is our MediaTek issue related to this known problem?

### Question 3: What Actually Crashes?
Our crash is at bionic initialization (`GetPropAreaForName`).  
The successful developers don't mention this issue.

**EVIDENCE NEEDED**: Is this a MediaTek-specific issue, or something in our integration?

### Question 4: Stub vs Native
When we use stub feature, we see "no dependencies" - we're not compiling llama.cpp at all!

**EVIDENCE NEEDED**: Have we actually tested native llama.cpp on this device, or only stub?

---

## ALTERNATIVES CONSIDERED (WITH ISSUES)

### Option A: Stay with llama.cpp

**Issues Found**:
- Our integration approach might be wrong (cc crate vs CMake)
- OEM-specific issues (known but not solved)
- bionic initialization crash (unique to our setup?)

**Evidence it CAN work**:
- Many tutorials exist
- Developers successfully run it
- Same model families work (Gemma 3, Qwen 2.5)

### Option B: Switch to mistral.rs

**Issues Found** (from research):
- Has segfaults (#2005, #689, #1203)
- Metal backend broken (#1029)
- CUDA+CPU panic open (#1114)
- No verified Android ARM64 deployment
- S24 Ultra crashes mentioned (Reddit)

**Evidence of issues**: Multiple GitHub issues found

### Option C: ONNX Runtime

**Evidence Found**:
- local-llms-on-android exists (114 stars)
- Uses ONNX instead of GGUF
- Requires model conversion

**Our status**: Never tested

### Option D: Pre-built Binaries

**Question**: Instead of building from source, can we use pre-built .so files?

---

## WHAT THE AUDIT SAYS (FROM NEOCORTEX-CODE-ONLY-FULL-AUDIT)

### P0 Risks Identified
1. **FFI lifecycle/order mismatch** in `aura-llama-sys`
2. **Link/runtime mode drift** - symbol expectations vs selected backend mode

### Risk Counters in Our Code
- `aura-llama-sys`: libloading = "0.8" (FFI)
- `model.rs`: unsafe=1, ffi=2

### Audit Recommendations
- Backend contract fences
- Startup readiness protocol
- Android lifecycle resilience

---

## QUESTIONS FOR DECISION (NOT ANSWERS)

Based on evidence, we need to ANSWER these before deciding:

1. **Build Approach**: Should we switch from cc crate to CMake integration?

2. **Testing**: Have we actually tested native llama.cpp, or only stub?

3. **OEM Issue**: Is our MediaTek crash the same as the known OEM mmap issues?

4. ** Alternatives**: Should we test ONNX before switching engines?

5. **Pre-built**: Can we use pre-compiled .so files instead of building?

---

## WHAT WE SHOULD DO NEXT (RESEARCH ONLY)

### Immediate Research Needed

1. **Try llama.cpp CMake approach** - The successful developer specifically said not to use default NDK cmake, use llama.cpp cmake directly

2. **Test with pre-built binaries** - Rule out build issues entirely

3. **Test ONNX export** - See if our models convert cleanly

4. **Device testing** - Test on different devices to isolate MediaTek-specific issue

### Evidence Still Needed

- [ ] Can we compile with llama.cpp official CMake?
- [ ] Do pre-built .so files work?
- [ ] Does ONNX export work for our models?
- [ ] Is this MediaTek-specific or general issue?

---

## SUMMARY

We have EVIDENCE that:
- llama.cpp CAN work on Android (many examples)
- There's a KNOWN approach that works (CMake, not cc crate)
- OEM-specific issues are REAL (not our imagination)
- Our approach (cc crate) might be the problem

We have EVIDENCE that:
- mistral.rs has REAL issues (segfaults, crashes)
- No verified Android deployment found
- We're trading known evil for unknown evil

We have QUESTIONS:
- Have we actually tested native llama.cpp?
- Is our build approach wrong?
- Can pre-built binaries work?

---

**NEXT STEP**: Research the CMake approach, test pre-built binaries, or test ONNX - before making ANY decision about switching engines.

**THIS IS NOT A RECOMMENDATION** - This is evidence summary for the team to review.
