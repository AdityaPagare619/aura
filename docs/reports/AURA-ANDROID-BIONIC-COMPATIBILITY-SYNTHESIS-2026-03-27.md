# AURA Android Bionic Compatibility Synthesis (2026-03-27)

## 1) Executive Summary (Honest Verdict)

Your failure timeline is technically coherent: most build-flag changes do not address the underlying startup crash class when the fault originates inside Android bionic initialization paths before meaningful app-level execution.

Based on the evidence provided (`GetPropAreaForName`, `android_fdsan_close_with_tag`) and the current architecture, this is best treated as a **platform/runtime compatibility incident** at the intersection of:

1. Android runtime/linker behavior
2. native C/C++ static initialization and loader interactions
3. SoC/vendor-specific behavior (MediaTek variance)

This means:
- **Build flags alone are unlikely to be sufficient**.
- The highest-confidence path is **architecture containment** (safe fallback + runtime gating) rather than attempting one more CPU-flag tweak.

---

## 2) What Was Tried (Consolidated)

| # | Attempt | Result | Interpretation |
|---|---|---|---|
| 1 | `armv8.2-a+fp16+dotprod` | SIGSEGV | ISA tuning did not remove fault class |
| 2 | `GGML_NATIVE=OFF`, `SVE=OFF` | SIGSEGV | Genericization did not remove early crash |
| 3 | `armv8-a`, `NEON_FP16=OFF` | SIGSEGV | Reduced vector assumptions still crashes |
| 4 | Stub build | blocked by direct FFI architecture | fallback path not fully isolated |
| 5 | addr2line forensic pass | root-cause indicators found | crash path inside bionic internals |
| 6 | `-crt-static` | no deps / worse | static strategy aggravated runtime mismatch |
| 7 | remove `-crt-static` | still no deps | toolchain/runtime behavior unchanged |
| 8 | `-C link-args=-lc` | no change | linker arg injection insufficient |
| 9 | custom target JSON | no change | target-spec forcing not enough |

---

## 3) Forensic Signals and Their Meaning

### Observed crash symbols
- `GetPropAreaForName` (`bionic/libc/system_properties/...`)
- `android_fdsan_close_with_tag` (`bionic/libc/bionic/fdsan.cpp`)

### Practical interpretation
- Crash manifests in system libc/runtime boundary, not typical Rust userland logic.
- This is consistent with a pre-main / early-runtime incompatibility class.
- Therefore, "fix code where crash appears" is not straightforward when call stacks terminate in bionic internals.

---

## 4) Architecture Findings in Current Repo

### Confirmed positives
- Neocortex now uses backend indirection for free paths (`aura_llama_sys::backend().free_model(...)`), which is correct for link flexibility.
- IPC split (daemon <-> neocortex) already provides a containment boundary for failover.

### Confirmed gap (now addressed in this change)
- Android stub fallback was not fully isolated from direct FFI compile/link paths.
- This prevented clean "no-llama-native" fallback in certain build/runtime combinations.

### Clarification on `build.rs` "hard override" concern

The current behavior is **feature-gated**, not a universal Android hard skip:

- `target_os=android && target_arch=aarch64 && feature(stub)=OFF`  
  -> native `llama.cpp` C/C++ compilation is executed.
- `target_os=android && target_arch=aarch64 && feature(stub)=ON`  
  -> native compilation is intentionally skipped (stub mode by design).
- non-Android targets  
  -> stub path by design.

So "no dependencies" is expected only when the `stub` feature is active (or when inspecting a host/stub artifact), not for all Android native builds.

---

## 5) Fix Implemented in This Patch

## Code change
`crates/aura-llama-sys/src/lib.rs`

### What changed
1. Android FFI extern block + FfiBackend implementation are now compiled only when:
   - `target_os = "android"`
   - and `feature = "stub"` is **not** enabled.
2. Added Android stub shim:
   - `init_ffi_backend(...)` under `target_os=android && feature=stub`
   - routes initialization to `init_stub_backend(...)`
   - keeps public API stable while avoiding native FFI binding requirements in stub mode.

### Why this matters
- Resolves the architecture blocker from attempt #4 ("stub build can't work due to direct FFI calls").
- Enables a safer compatibility fallback path for problematic devices/ROM combinations.

---

## 6) Why This Still May Not Fully Eliminate Device Crash

Even after this fix, if production path intentionally uses native llama backend on affected devices, bionic-level startup crashes may persist.

So this patch should be viewed as:
- **necessary compatibility hardening**, not guaranteed full remediation of every MediaTek/ROM startup fault.

### Root-cause status

- **Resolved:** architecture blocker that prevented Android stub fallback from being a clean path.
- **Not fully resolved:** native C++ FFI startup instability on diverse real devices/SoCs.
- **Practical truth:** IPC split (daemon <-> neocortex) is a correct architecture boundary, but native backend instability can still crash the neocortex side unless runtime gating/fallback policy is enforced.

---

## 7) Recommended No-Regret Path (Low Compatibility Risk)

## Priority P0 (do now)
1. **Device/runtime gating:** default to stub-safe mode on known-bad device signatures or failed warmup probes.
2. **Two-phase startup probe:** perform minimal backend init/health probe before enabling full inference path.
3. **Fast fallback:** if probe fails/crashes historically, daemon keeps service alive with degraded inference mode.

## Priority P1
4. **Artifact split:** maintain separate known-safe Android build profile (conservative ABI/runtime assumptions).
5. **Device matrix policy:** explicitly classify unsupported/experimental SoCs instead of universal best-effort.

## Priority P2
6. **Alternative backend option:** investigate ONNX/NNAPI/TFLite fallback path for problematic vendor stacks.

### Backend fit snapshot (system-level)

| Option | SoC Compatibility Risk | Feature Completeness | Performance | Integration Cost | Recommendation |
|---|---|---|---|---|---|
| llama.cpp native via Rust FFI (current) | Medium-High on heterogeneous Android devices | Full | High | Already integrated | Keep as Tier-1 where validated |
| Stub fallback (current) | Low | Minimal | N/A | Already integrated | Keep as safety mode / degraded path |
| ONNX Runtime Mobile + NNAPI | Lower (for broad Android) | Medium-High | Medium-High (device dependent) | Medium-High | Best candidate Tier-2 fallback |
| TFLite + XNNPACK/NNAPI | Low-Medium | Medium | Medium | Medium | Good candidate for constrained models |

Recommended strategy: **multi-backend policy** (native llama primary on validated devices, ONNX/TFLite fallback on unknown/problematic device classes, stub as fail-safe).

---

## 8) Evidence/Proof Plan You Can Trust

To move from "likely" to "provable," require these checks per failing device:

1. Startup marker logs proving whether crash occurs before/after backend initialization.
2. Tombstone collection (`/data/tombstones`) + symbolized stack traces.
3. Runtime report with:
   - ABI
   - Android version/API level
   - vendor/SoC identifiers
   - selected backend mode (native vs stub)
4. Controlled A/B:
   - same build + native backend
   - same build + forced stub backend

If native crashes and stub survives on same device profile, compatibility class is strongly confirmed.

---

## 9) Direct Answer to Your Ask

- You asked for a full, honest synthesis including architecture, all attempts, actions, and failures.
- The honest assessment is: **this is primarily a runtime compatibility problem, not a simple compiler-flag problem**.
- The fix included here removes a key architectural blocker so fallback mode can actually be used safely on Android.
- The next reliable outcome comes from runtime gating + probe-first startup + explicit device compatibility policy.

## 10) Scope Boundary (What This Document Does and Does Not Claim)

### Confirmed by repository evidence
- Android build/link behavior is feature-gated (native path when stub is OFF; stub-safe path when stub is ON).
- Backend abstraction can preserve linkability across native and stub-capable builds.
- Startup failures that trigger in bionic internals before normal stage flow are a real, separate class from inference-time defects.

### Not claimed as fully proven in this document
- A single universal root cause for all vendor/SoC crash variants.
- Guaranteed crash-free startup on every Android device class without runtime gating and device-matrix validation.

This synthesis should therefore be treated as an operational decision record with
clear evidence boundaries, not as universal proof of complete Android runtime stability.

## 11) Document Status

- Status: Complete synthesized assessment for the current incident window.
- Scope: Architecture, failed attempts, forensic interpretation, remediation direction, and proof plan.
- Next update trigger: new tombstone evidence, validated device A/B results, or backend strategy change.

**END OF SYNTHESIS**
