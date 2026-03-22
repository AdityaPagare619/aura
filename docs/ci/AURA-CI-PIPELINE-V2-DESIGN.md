# AURA v4 CI Pipeline v2 Design

**Document**: `docs/ci/AURA-CI-PIPELINE-V2-DESIGN.md`  
**Purpose**: Complete CI/CD architecture with 6-stage pipeline and device validation  
**Status**: FOUNDATIONAL — This design is MANDATORY for all CI work  
**Created**: 2026-03-22  
**Owner**: Build Infrastructure Charter  

---

## Executive Summary

**Current Problem**: AURA has 3 failing CI workflows. 8 alpha releases shipped with CI green and device red. CI validates BUILD, not BEHAVIOR. F001 SIGSEGV was never caught by CI because CI never executed the binary on the target device.

**Solution**: CI Pipeline v2 with 6 stages. STAGE 4 (Linux tests) ≠ VALIDATION. STAGE 6 (device execution) = VALIDATION.

**Key Principle**: `CI green = BUILD gate passed. Device test green = VALIDATION gate passed. Both required for release.`

---

## Pipeline Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          CI PIPELINE v2                                  │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐                 │
│  │  STAGE 1    │───▶│  STAGE 2    │───▶│  STAGE 3    │                 │
│  │  SOURCE     │    │  BUILD      │    │  INSPECT    │                 │
│  └─────────────┘    └─────────────┘    └─────────────┘                 │
│        │                  │                  │                          │
│        ▼                  ▼                  ▼                          │
│  Checkout, cache     Cross-compile      Verify artifact                 │
│  Env detection      Target: ARM64       Architecture, linker            │
│                                                                          │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐                 │
│  │  STAGE 4    │───▶│  STAGE 5    │───▶│  STAGE 6    │                 │
│  │  TEST       │    │  DEPLOY     │    │  VALIDATE   │                 │
│  └─────────────┘    └─────────────┘    └─────────────┘                 │
│        │                  │                  │                          │
│        ▼                  ▼                  ▼                          │
│  Unit + lint         Upload artifact    EXECUTE ON DEVICE              │
│  Linux only          Download to device  THE REAL TEST                  │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

**CRITICAL**: STAGE 4 runs on Linux x86_64. STAGE 6 runs on Android ARM64. These are DIFFERENT ENVIRONMENTS. A binary that passes STAGE 4 may fail STAGE 6 (F001 SIGSEGV).

---

## Stage Specifications

### Stage 1: SOURCE

**Purpose**: Checkout code, setup environment, prepare for build.

**Actions**:
```yaml
- name: Checkout code
  uses: actions/checkout@v4
  
- name: Setup Rust
  uses: dtolnay/rust-toolchain@stable
  with:
    targets: aarch64-linux-android
    
- name: Cache cargo
  uses: Swatinem/rust-cache@v2
  with:
    cache-on-failure: true
    
- name: Detect environment
  run: |
    echo "HOST_ARCH=$(uname -m)"
    echo "TARGET_ARCH=aarch64-linux-android"
    echo "CARGO_TARGET_DIR=${{ env.CARGO_TARGET_DIR }}"
```

**Failure Modes**:
- F002 (Artifact not found): Not applicable at this stage
- F003 (Dependency mismatch): Handled by cargo resolver

**Gate**: Must complete successfully before STAGE 2.

---

### Stage 2: BUILD

**Purpose**: Cross-compile AURA for Android ARM64.

**Prerequisites**:
1. Rust toolchain with `aarch64-linux-android` target installed
2. Android NDK r26b with clang for cross-compilation
3. CC/AR environment variables set for the target

**Actions**:
```yaml
- name: Setup Rust
  uses: dtolnay/rust-toolchain@master
  with:
    toolchain: stable
    targets: aarch64-linux-android

- name: Setup Android NDK
  uses: android-ndk-org/setup-ndk@v1
  with:
    ndk-version: r26b

- name: Build aura-daemon
  env:
    CC_aarch64_linux_android: ${{ env.ANDROID_NDK_HOME }}/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android34-clang
    AR_aarch64_linux_android: ${{ env.ANDROID_NDK_HOME }}/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-ar
    CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER: ${{ env.ANDROID_NDK_HOME }}/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android34-clang
  run: |
    cargo build \
      --release \
      --features "stub,reqwest" \
      --target aarch64-linux-android \
      --manifest-path crates/aura-daemon/Cargo.toml
      
- name: Build aura-cli
  env:
    CC_aarch64_linux_android: ${{ env.ANDROID_NDK_HOME }}/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android34-clang
    AR_aarch64_linux_android: ${{ env.ANDROID_NDK_HOME }}/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-ar
    CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER: ${{ env.ANDROID_NDK_HOME }}/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android34-clang
  run: |
    cargo build \
      --release \
      --target aarch64-linux-android \
      --manifest-path crates/aura-cli/Cargo.toml
```

**Target Triple**: `aarch64-unknown-linux-android` (API 21+, Android 5.0+)

**NDK Requirement**: 
- Use `aarch64-linux-android` from Android NDK r26b. NOT `aarch64-unknown-linux-gnu`.
- NDK is REQUIRED for C compilation of dependencies like `ring` (cryptography)
- WITHOUT NDK: Build fails with "failed to find tool aarch64-linux-android-clang"

**Feature Syntax**:
- When using `--manifest-path crates/aura-daemon/Cargo.toml`, use crate-level features: `stub,reqwest`
- When using `--workspace`, use workspace features: `aura-llama-sys/stub,aura-daemon/reqwest`

**Failure Modes**:
- F001 (SIGSEGV): May not be caught here — build succeeds even if runtime will fail
- F003 (Dependency mismatch): cargo resolver fails
- F005 (Linker error): Wrong target or missing NDK
- F006 (Feature gate conflict): `#[cfg]` issues in code
- **NEW**: NDK not installed → "failed to find tool aarch64-linux-android-clang"

**Gate**: `cargo build` must exit code 0. Artifact must be produced.

---

### Stage 3: INSPECT

**Purpose**: Verify artifact is correct for target platform BEFORE proceeding to tests.

**Actions**:
```yaml
- name: Get artifact info
  id: artifact
  run: |
    ARTIFACT_PATH="targets/aarch64-linux-android/release/aura-daemon"
    echo "path=$ARTIFACT_PATH" >> $GITHUB_OUTPUT
    echo "size=$(stat -c%s $ARTIFACT_PATH)"
    
- name: Verify architecture
  run: |
    file targets/aarch64-linux-android/release/aura-daemon
    # Expected: ELF 64-bit LSB executable, ARM aarch64
    
- name: Verify dynamic linker
  run: |
    readelf -d targets/aarch64-linux-android/release/aura-daemon | grep NEEDED
    # Expected: libc.so, libm.so, libdl.so (bionic, NOT glibc)
    
- name: Verify ELF class
  run: |
    readelf -h targets/aarch64-linux-android/release/aura-daemon
    # Expected: Machine: AArch64 (EM_AARCH64)
```

**Expected Output**:
```
ELF 64-bit LSB executable, ARM aarch64, EABI5 version 1 (SYSV)
Machine: AARCH64
```

**NOT Expected**:
```
ELF 64-bit LSB executable, x86-64 (glibc)  ← WRONG ARCHITECTURE
```

**Failure Modes**:
- F001 (SIGSEGV): If architecture is wrong, runtime will fail
- F004 (ABI mismatch): If glibc instead of bionic, runtime will fail
- F005 (Linker/entrypoint): If ELF class wrong, cannot execute

**Gate**: Artifact must pass all inspections. If inspection fails, pipeline BLOCKS.

---

### Stage 4: TEST (Linux Host)

**Purpose**: Run unit tests, lint, and security checks on Linux host.

**Actions**:
```yaml
- name: Run unit tests (Linux)
  run: |
    cargo test --workspace --lib
    # Runs on HOST machine (Linux x86_64)
    # Tests core logic without device-specific code
    
- name: Run integration tests (Linux)
  run: |
    cargo test --workspace --test '*'
    # Only tests that can run on Linux
    
- name: Clippy lint
  run: |
    cargo clippy --workspace --all-targets -- -D warnings
    
- name: Security audit
  run: |
    cargo audit
```

**WARNING**: These tests run on LINUX x86_64, NOT Android ARM64.

**What This Stage Validates**:
- Core logic correctness
- Compilation without warnings
- No known security vulnerabilities (cargo audit)
- Unit test coverage

**What This Stage Does NOT Validate**:
- Runtime behavior on Android
- Device-specific issues (F001-F010)
- ABI compatibility with bionic
- Permission issues on Android

**Failure Modes**:
- F012 (Test coverage gap): Tests pass but device fails

**Gate**: All tests must pass. But this gate alone is NOT SUFFICIENT for release.

---

### Stage 5: DEPLOY

**Purpose**: Upload artifact to accessible location for device download.

**Options**:

**Option A: GitHub Actions Artifacts** (Recommended for small teams)
```yaml
- name: Upload artifact
  uses: actions/upload-artifact@v4
  with:
    name: aura-daemon-android
    path: targets/aarch64-linux-android/release/aura-daemon
    retention-days: 30
```

**Option B: GitHub Release**
```yaml
- name: Create Release Draft
  uses: softprops/action-gh-release@v1
  with:
    draft: true
    files: targets/aarch64-linux-android/release/aura-daemon
```

**Option C: Direct Device Upload** (For Termux CI)
```yaml
- name: Upload to device via curl
  run: |
    curl -T targets/aarch64-linux-android/release/aura-daemon \
      "$DEVICE_UPLOAD_URL"
```

**Failure Modes**:
- F002 (Artifact not found): Upload failed
- Network issues: Upload interrupted

**Gate**: Artifact must be available for device download.

---

### Stage 6: DEVICE VALIDATE (THE REAL TEST)

**Purpose**: Execute binary on actual Android device. THIS IS THE VALIDATION.

**Workflow**: `device-validate.yml` (separate workflow, triggers on workflow_run)

```yaml
name: Device Validate
on:
  workflow_run:
    workflows: ["CI Pipeline v2"]
    types: [completed]
    branches: [main]

jobs:
  device-test:
    runs-on: ubuntu-latest
    if: ${{ github.event.workflow_run.conclusion == 'success' }}
    
    steps:
      - name: Download artifact
        uses: actions/download-artifact@v4
        with:
          name: aura-daemon-android
          
      - name: Setup SSH to device
        uses: webfactory/ssh-access@v1
        with:
          host: ${{ secrets.DEVICE_HOST }}
          user: ${{ secrets.DEVICE_USER }}
          key: ${{ secrets.DEVICE_SSH_KEY }}
          
      - name: Deploy to device
        run: |
          scp aura-daemon ${{ secrets.DEVICE_USER }}@${{ secrets.DEVICE_HOST }}:/tmp/
          ssh ${{ secrets.DEVICE_USER }}@${{ secrets.DEVICE_HOST }} \
            'chmod +x /tmp/aura-daemon'
            
      - name: Execute on device
        id: execute
        run: |
          ssh ${{ secrets.DEVICE_USER }}@${{ secrets.DEVICE_HOST }} \
            '/tmp/aura-daemon --version 2>&1 || echo "EXIT_CODE=$?"'
            
      - name: Check exit code
        run: |
          if [ "${{ steps.execute.outputs.exit_code }}" != "0" ]; then
            echo "DEVICE_TEST_FAILED"
            exit 1
          fi
          
      - name: Cleanup
        if: always()
        run: |
          ssh ${{ secrets.DEVICE_USER }}@${{ secrets.DEVICE_HOST }} \
            'rm -f /tmp/aura-daemon'
```

**Alternative: Termux CI (No SSH)**
```yaml
# For teams without cloud device access
# Uses GitHub Actions to download artifact and create release
# Human manually tests on device and reports result

- name: Create Release for Manual Testing
  uses: softprops/action-gh-release@v1
  if: github.event_name == 'push'
  with:
    draft: true
    files: targets/aarch64-linux-android/release/aura-daemon
    body: |
      ## Manual Testing Required
      
      This release requires manual validation on:
      - Moto G45 5G (Termux)
      - Android 15 (API 35)
      
      Test steps:
      1. Download artifact
      2. Deploy to device
      3. Execute: `/tmp/aura-daemon --version`
      4. Check exit code is 0
      5. Check 5 boot stages logged
      
      Report results in CI run.
```

**Failure Modes**:
- F001 (SIGSEGV): Binary crashes on device (ABI mismatch)
- F007 (Runtime crash): Binary starts but fails during boot
- F009 (Toolchain failure): Device environment issues
- F010 (Environment mismatch): Path/permission issues

**Gate**: Binary MUST execute on device with exit code 0. If this fails, release is BLOCKED.

---

## Workflow Files

### ci.yml (Stages 1-4)

```yaml
name: CI Pipeline v2

on:
  push:
    branches: [main, develop]
  pull_request:
    branches: [main]

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: full

jobs:
  # STAGES 1-4: SOURCE → BUILD → INSPECT → TEST
  build-and-test:
    runs-on: ubuntu-latest
    
    steps:
      # STAGE 1: SOURCE
      - name: Checkout
        uses: actions/checkout@v4
        
      - name: Setup Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-linux-android
          
      - name: Cache cargo
        uses: Swatinem/rust-cache@v2
        with:
          cache-on-failure: true
          
      - name: Detect environment
        run: |
          echo "HOST=$(uname -s)"
          echo "ARCH=$(uname -m)"
          echo "TARGET=aarch64-linux-android"
          
      # STAGE 2: BUILD
      - name: Build aura-daemon
        run: |
          cargo build \
            --release \
            --features "aura-llama-sys/stub,aura-daemon/reqwest" \
            --target aarch64-linux-android \
            --manifest-path crates/aura-daemon/Cargo.toml
            
      - name: Build aura-cli
        run: |
          cargo build \
            --release \
            --target aarch64-linux-android \
            --manifest-path crates/aura-cli/Cargo.toml
            
      # STAGE 3: INSPECT
      - name: Verify architecture
        run: |
          file targets/aarch64-linux-android/release/aura-daemon
          # Must show: ELF 64-bit LSB executable, ARM aarch64
          
      - name: Verify dynamic linker
        run: |
          readelf -d targets/aarch64-linux-android/release/aura-daemon | grep NEEDED
          # Must show bionic libraries, NOT glibc
          
      # STAGE 4: TEST (Linux)
      - name: Run unit tests
        run: |
          cargo test --workspace --lib
          
      - name: Clippy
        run: |
          cargo clippy --workspace --all-targets -- -D warnings
          
      - name: Audit
        run: |
          cargo audit
          
      # STAGE 5: DEPLOY (part of ci.yml)
      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: aura-daemon-android
          path: targets/aarch64-linux-android/release/aura-daemon
          retention-days: 30
```

### device-validate.yml (Stages 5-6)

```yaml
name: Device Validate

on:
  workflow_run:
    workflows: ["CI Pipeline v2"]
    types: [completed]
    branches: [main]

jobs:
  device-test:
    runs-on: ubuntu-latest
    if: ${{ github.event.workflow_run.conclusion == 'success' }}
    
    steps:
      - name: Download artifact
        uses: actions/download-artifact@v4
        with:
          name: aura-daemon-android
          path: .
          
      - name: Make executable
        run: chmod +x aura-daemon
        
      - name: Setup SSH
        uses: webfactory/ssh-access@v1
        with:
          host: ${{ secrets.DEVICE_HOST }}
          user: ${{ secrets.DEVICE_USER }}
          key: ${{ secrets.DEVICE_SSH_KEY }}
          
      - name: Deploy to device
        run: |
          scp aura-daemon ${{ secrets.DEVICE_USER }}@${{ secrets.DEVICE_HOST }}:/tmp/
          
      - name: Execute on device
        run: |
          ssh ${{ secrets.DEVICE_USER }}@${{ secrets.DEVICE_HOST }} \
            'chmod +x /tmp/aura-daemon && /tmp/aura-daemon --version'
            
      - name: Report success
        if: success()
        run: |
          echo "DEVICE_VALIDATION_PASSED"
          echo "Binary executed successfully on Android device"
          
      - name: Report failure
        if: failure()
        run: |
          echo "DEVICE_VALIDATION_FAILED"
          echo "Binary did not execute correctly on device"
          exit 1
          
      - name: Cleanup
        if: always()
        run: |
          ssh ${{ secrets.DEVICE_USER }}@${{ secrets.DEVICE_HOST }} \
            'rm -f /tmp/aura-daemon'
```

---

## Device Matrix

### Current Device (Moto G45 5G)

| Property | Value |
|----------|-------|
| Model | Moto G45 5G |
| OS | Android 15 (API 35) |
| Architecture | aarch64 (ARM64-v8a) |
| Termux | Installed |
| Curl | 8.19.0 |
| Rust | 1.93.1 (pkg) |

### 3x3x3 Device Matrix (Future)

| Class | Low-End | Mid-Range | High-End |
|-------|---------|-----------|----------|
| **OS** | Android 10 (API 29) | Android 13 (API 33) | Android 15 (API 35) |
| **RAM** | 2GB | 4GB | 8GB+ |
| **Vendor** | Samsung A-series | Pixel A-series | Pixel flagship |

**Priority**: Get v4.0.0-stable working on Moto G45 first. Expand matrix after stable ships.

---

## Failure Handling in CI

### CI Fails at Stage 2 (BUILD)

**Action**: Check F003 (Dependency), F005 (Linker), F006 (Feature gate)

**Command**:
```bash
cargo build --all-features --target aarch64-linux-android 2>&1
```

**Common Fixes**:
- F003: `cargo update` to resolve dependency conflicts
- F005: Verify NDK toolchain is installed: `rustup target list | grep android`
- F006: Redesign feature architecture (sync-only traits)

### CI Fails at Stage 3 (INSPECT)

**Action**: Check F004 (ABI mismatch), F005 (Wrong ELF)

**Command**:
```bash
file aura-daemon
readelf -h aura-daemon | grep Machine
```

**Common Fixes**:
- F004: Use correct NDK sysroot
- F005: Set correct target triple

### CI Fails at Stage 4 (TEST)

**Action**: Fix test or code. This is standard unit test failure.

**Commands**:
```bash
cargo test --workspace 2>&1
cargo clippy --workspace 2>&1
```

### CI Fails at Stage 6 (DEVICE)

**Action**: This is the REAL failure. Do NOT ignore.

**Diagnosis**:
```bash
# On device
/tmp/aura-daemon 2>&1
echo "Exit code: $?"

# Check boot logs
cat /sdcard/AURA/logs/aura-boot-*.log

# Check crash dumps
ls -la /sdcard/AURA/dumps/
```

**Common Fixes**:
- F001: Cross-compile with correct NDK (F004 prevention)
- F007: Check config, permissions, environment variables
- F009: Reset device Rust setup (F009 prevention)
- F010: Use correct Termux paths

---

## Termux CI Feasibility

Research confirms Termux CI IS possible:
- Jorin's Go coding agent uses GitHub Actions + Termux cross-compilation successfully
- termux-packages repo has 44,000+ workflow runs
- Approach: Cross-compile on Linux (CI), deploy artifact to Termux for validation

**Current Limitation**: No automated device access in GitHub Actions (device is physical).

**Workaround Options**:

| Option | Pros | Cons |
|--------|------|------|
| Manual release + manual testing | Simple | Slow, error-prone |
| SSH to always-on device | Automated | Security, availability |
| Cloud Android devices (Firebase Test Lab) | Scalable | Cost |
| Release draft + human approval | Human validation | Manual gate |

**Recommended**: Release draft + human approval for v4.0.0-stable. Automate SSH after stable ships.

---

## Release Gate Integration

This CI pipeline feeds into the release gate defined in `OPERATING-PROCEDURES.md`:

```
┌─────────────────────────────────────────────────────────────────┐
│                    RELEASE GATE CHECKLIST                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  □ BUILD gate (Stage 2)                                        │
│     cargo build --all-features --target aarch64-linux-android   │
│     EXIT CODE: 0                                                 │
│                                                                  │
│  □ INSPECT gate (Stage 3)                                       │
│     file shows aarch64, readelf shows android linker            │
│     VERIFIED: correct architecture + ABI                        │
│                                                                  │
│  □ TEST gate (Stage 4)                                         │
│     cargo test --workspace passes                               │
│     cargo clippy passes                                         │
│     EXIT CODE: 0                                                 │
│                                                                  │
│  □ DEVICE gate (Stage 6)                                       │
│     Binary executes on device                                   │
│     Exit code: 0                                                │
│     5 boot stages logged                                        │
│     VERIFIED: device works                                      │
│                                                                  │
│  ALL GATES PASS?                                                 │
│     ├──► YES ──► APPROVED FOR RELEASE                          │
│     └──► NO ──► BLOCKED — fix gate failures                    │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## CI Workflow Comparison

| Workflow | Current State | v2 State | Change |
|----------|--------------|----------|--------|
| `ci.yml` | Failing (compile errors) | 6 stages | Full redesign |
| `build-android.yml` | Failing | Merged into ci.yml | Simplified |
| `device-validate.yml` | Not exists | 2 stages (5+6) | NEW |

---

## Implementation Roadmap

| Phase | Action | Status |
|-------|--------|--------|
| 1 | Create `device-validate.yml` | 📋 TODO |
| 2 | Update `ci.yml` to Stage 3 | 📋 TODO |
| 3 | Add Stage 4 (lint + audit) | 📋 TODO |
| 4 | Add artifact upload (Stage 5) | 📋 TODO |
| 5 | Test full pipeline | 📋 TODO |
| 6 | Add device SSH access | 📋 TODO |
| 7 | Automate device testing (Stage 6) | 📋 TODO |

---

## Key Insights

1. **CI validates BUILD, not BEHAVIOR** — Stage 4 passing ≠ device works
2. **Device testing is MANDATORY** — Stage 6 is the REAL test
3. **CI green ≠ release ready** — All 6 stages must pass
4. **Inspection is cheap** — Stage 3 catches ABI issues before runtime
5. **F001 SIGSEGV was never caught** — because CI never ran binary on device

---

## Final Statement

> **"A CI pipeline that cannot execute the binary on the target device is not a CI pipeline. It is a build script that lies about quality."**

CI Pipeline v2 fixes this. Stage 6 is not optional. Device testing is not optional. The binary must run on the device, or the release is not complete.

---

## References

| Document | Purpose |
|----------|---------|
| `CONTRACT.md` | Platform contract — what v4.0.0-stable promises |
| `FAILURE_TAXONOMY.md` | F001-F015 failure classification |
| `OPERATING-PROCEDURES.md` | Release gate checklist (D6) |
| `TRANSFORMATION-PLAN.md` | Sprint plan for CI implementation |
