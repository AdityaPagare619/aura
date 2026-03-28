# 04 — DevOps and Infrastructure Architecture (Current Codebase)

## 1) Pipeline topology

Active workflows include:

- `ci.yml`
- `build-android.yml`
- `device-validate.yml`
- plus release/audit/supporting workflows in `.github/workflows/`.

This creates a multi-stage validation/deployment envelope rather than a single monolithic build.

## 2) CI Pipeline v2 architecture (`ci.yml`)

`ci.yml` includes jobs for:

- Source checks
- Android build
- cargo check
- cargo test
- clippy with `-D warnings`
- rustfmt check
- security audit
- version checks (contextual)

Representative references:
- workflow name and jobs: `.github/workflows/ci.yml:15`, `:70`, `:161`, `:185`, `:210`, `:234`, `:250`
- feature set in check/test/clippy: `.github/workflows/ci.yml:182`, `:205`, `:231`

## 3) Android build path architecture

Two Android-related strategies appear in workflows:

### 3.1 CI v2 Android build (`ci.yml`)

- builds `aura-daemon` Android binary with features `stub,curl-backend`
- uploads artifact name `aura-daemon-android-v2`

References:
- build command: `.github/workflows/ci.yml:116-123`
- artifact upload: `.github/workflows/ci.yml:148-155`

### 3.2 Dedicated android build (`build-android.yml`)

- uses NDK download and checksum verification
- builds `aura-daemon` cdylib and `aura-neocortex` binary for aarch64
- verifies runtime deps and uploads binaries

References:
- NDK integrity flow: `.github/workflows/build-android.yml:78-98`
- build commands: `.github/workflows/build-android.yml:154-158`
- artifact upload: `.github/workflows/build-android.yml:217-223`

## 4) Device validation architecture

`device-validate.yml` is triggered on successful `CI Pipeline v2` runs and attempts to fetch named artifact for validation (arch/size/simulated execution checks and optional release draft operations).

References:
- trigger and job: `.github/workflows/device-validate.yml:16`, `:26-29`
- artifact expectation: `.github/workflows/device-validate.yml:31-35`
- validation checks: `.github/workflows/device-validate.yml:47-60`

## 5) Observed operational infra issue

Recent failure (`23677813385`) failed in Device Validation on artifact download (`Artifact not found for name: aura-daemon-android-v2`).

Implication: infra coupling risk between producer/consumer workflow contexts remains a critical operational concern independent of application correctness.

## 6) Security and supply-chain controls

- NDK SHA verification in dedicated Android build workflow.
- CI security audit step (`cargo audit` path) in `ci.yml`.

References:
- `.github/workflows/build-android.yml:78-92`
- `.github/workflows/ci.yml:250-263`

## 7) DevOps architecture conclusions

1. The project has enterprise-style staged CI controls, not a simplistic single job.
2. Artifact and workflow coupling is the key reliability lever.
3. Android path is explicitly treated as special-case infra due to toolchain + ABI realities.
4. Build and validation workflow consistency is as important as code correctness for production readiness.
