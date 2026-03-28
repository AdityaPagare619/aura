# 06 — Binary and Build Architecture (Code-First)

## 1) Build graph overview

Binary/build architecture spans:

- Rust workspace build via cargo
- platform-conditioned native compilation in `aura-llama-sys/build.rs`
- Android NDK cross-compile settings in CI workflows
- artifact packaging and downstream validation workflows

## 2) `aura-llama-sys` build behavior

### 2.1 Target-conditioned native compile

- On Android aarch64: compile C and C++ llama sources and link static libs.
- In stub mode on Android: skip native compilation and emit stub cfg markers.
- On non-Android: always stub path (pure Rust side).

Reference: `crates/aura-llama-sys/build.rs:16-87`.

### 2.2 Android C++ runtime link strategy

Build script emits link search paths and static libs:

- `c++_static`
- `c++abi`

with NDK root discovery and warnings if archives are not found.

Reference: `crates/aura-llama-sys/build.rs:90-183`.

## 3) Binary targets and runtime intent

- `aura-daemon` has binary and shared-library use cases.
- In APK mode, shared library path is primary; standalone daemon binary path is for Termux/host ops.

Reference: `crates/aura-daemon/src/bin/main.rs:4-10`.

## 4) CI build architecture details

### 4.1 CI pipeline Android job (`ci.yml`)

- Builds Android daemon binary with `stub,curl-backend`.
- Uploads `aura-daemon-android-v2` artifacts.

Reference: `.github/workflows/ci.yml:116-123`, `:148-155`.

### 4.2 Dedicated Android build workflow (`build-android.yml`)

- Builds daemon cdylib (`--lib -p aura-daemon --features reqwest`) and neocortex binary.
- Performs runtime dependency inspections and artifact upload.

Reference: `.github/workflows/build-android.yml:154-158`, `:181-223`.

## 5) Known build/ops fragility classes from evidence

1. **Feature skew risk**: `stub/curl-backend` path vs `reqwest` path across workflows can produce different runtime assumptions.
2. **Artifact naming/coupling risk**: downstream workflow failure if expected artifact name or run context mismatches.
3. **Path hygiene risk**: minor path typos (`targets` vs `target`) can break verification confidence if untested.

References:
- `.github/workflows/ci.yml:140-143`
- `.github/workflows/device-validate.yml:31-35`
- failed run `23677813385` logs

## 6) Build architecture conclusions

- The repository currently contains both source-native and stub build pathways; architecture is not single-path.
- Reliability depends on enforcing consistent feature/target assumptions across CI workflows and runtime initialization contracts.
- Any migration to prebuilt native `.so` strategy should be treated as an architecture decision with explicit end-to-end changes (build, packaging, init, validation), not only a `build.rs` edit.
