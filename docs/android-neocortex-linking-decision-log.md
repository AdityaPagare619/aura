# Android Neocortex Linking Decision Log

Owner: AURA core/runtime + mobile systems + DevOps

Purpose: Prevent repeated speculative fixes and keep cross-domain traceability.

## Governance Rules (Active)

1. No multi-change speculative patches.
2. Every change must declare:
   - hypothesis
   - expected outcome
   - rollback criteria
3. Max 1 in-progress experiment at a time.
4. Every failed run updates this log.
5. No completion claim without evidence bundle.

---

## Baseline Problem Statement

- Real device (Termux) runtime failure seen previously:
  - `aura-neocortex` fails with missing `libc++_shared.so`.
- After static-link attempts, CI `Build Android` fails linking `aura-neocortex`.
- Current objective:
  1. CI Android build succeeds,
  2. neocortex artifact avoids runtime dependence on `libc++_shared.so`,
  3. installer/runtime probes remain strict.

---

## Experiment E1

Date: 2026-03-18

### Hypothesis

If rustc cannot find `c++_static`, then explicit NDK library search roots in both:

- build script (`cargo:rustc-link-search=native=...`), and
- workflow rustflags (`CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS=-L native=...`)

will resolve static libc++ archive location and unblock Android neocortex link.

### Change Set

- `crates/aura-llama-sys/build.rs`
  - emit Android NDK link-search paths
  - disable cc-rs automatic C++ stdlib linkage
  - emit `rustc-link-lib=static=c++_static`
  - archive-discovery warning logs
- `.github/workflows/build-android.yml`
  - export `ANDROID_NDK_HOST_TAG`
  - set explicit target rustflags `-L native=...`
- `.github/workflows/release.yml`
  - mirror target rustflags and host tag

Branch/PR:
- `fix/android-static-libcpp-resolution`
- PR #16

### Expected Outcome

- Build no longer fails with `could not find native static library c++_static`.

### Result

- `Build Android` run `23249758573` progressed past `c++_static` discovery.
- New link failure surfaced with unresolved C++ exception ABI symbols:
  - `__cxa_begin_catch`
  - `__cxa_allocate_exception`
  - `__cxa_throw`
  - `__gxx_personality_v0`
  - `std::terminate()`

Interpretation:
- Search-path issue is mostly resolved.
- Now missing static C++ ABI runtime linkage completeness.

### Rollback Criteria

- If fix introduces no progress (same error) -> rollback.
- Not triggered, because error class changed and gave new root signal.

---

## Experiment E2 (In Progress)

Date: 2026-03-18

### Hypothesis

Android NDK static C++ runtime requires explicit `c++abi` linkage in addition to `c++_static` for exception/runtime symbols used by llama.cpp C++ objects.

### Change Set

- `crates/aura-llama-sys/build.rs`
  - add: `println!("cargo:rustc-link-lib=static=c++abi");`

### Expected Outcome

- Linker resolves `__cxa*`, `__gxx_personality_v0`, `std::terminate`, and related symbols.
- `Build aura-neocortex binary` succeeds.

### Rollback Criteria

- If unresolved symbols persist with same ABI set, revert E2 and test alternate ABI runtime strategy.

### Result

- Build Android run `23250189302` (workflow_dispatch on branch `fix/android-static-libcpp-resolution`) passed end-to-end.
- Key step outcomes:
  - `Build aura-daemon cdylib (libaura_daemon.so)`: success
  - `Build aura-neocortex binary`: success
  - `Verify Android runtime dependencies`: success
  - `Upload binaries`: success

### Root Cause Confirmed

- E1 solved archive discovery (`libc++_static.a` found) but linker still failed due to unresolved C++ exception ABI symbols.
- Root issue: static libc++ linkage alone was insufficient; Android NDK C++ ABI runtime symbols required explicit `c++abi` linkage.

### Final Status

- E2 hypothesis confirmed.
- Android cross-build is now green for neocortex with runtime dependency gate preserved.
