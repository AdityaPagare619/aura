# AURA F001 Root Cause Resolution — Enterprise Execution Plan
**Plan ID:** AURA-PLAN-F001-001
**Version:** 1.0.0
**Date:** 2026-03-19
**Domain:** Platform Engineering + CI/CD
**Priority:** SEV-1 — Production Blocker
**Status:** EXECUTING

---

## Goal

Systematically identify and fix the root cause of F001_STARTUP_SEGFAULT — binary crashes on real Android/Termux devices despite passing CI.

**Hard Requirement:** AURA binary must run on real Termux/Android devices. No release until this is resolved.

---

## Current State (Post-termux-elf-cleaner)

| Test | Result | Interpretation |
|------|--------|----------------|
| H1: LD_PRELOAD bypass | SIGSEGV both with/without | NOT the cause |
| H4: termux-elf-cleaner | DF_1_* flags fixed, SIGSEGV persists | Partially confirmed, root cause deeper |
| H2: llvm-strip | NOT TESTED | Needs unstripped binary |
| H5: Rust nightly broken | NOT TESTED | Needs different Rust version |
| H6: panic=abort + NDK | NOT TESTED | Needs build config change |

**termux-elf-cleaner** found a real issue (DF_1_* flag = 0x08000001 unsupported by Bionic) and fixed it. But the binary **still crashes**. The root cause is NOT just ELF flags.

---

## Architecture

The CI/CD system for AURA Android builds consists of:

```
.github/workflows/
├── ci.yml                    # Unit tests, clippy, fmt, audit
├── build-android.yml         # Dev build (cdylib + neocortex only)
│   └── NOTE: Does NOT build aura-daemon binary!
└── release.yml               # Production build (daemon + neocortex)
    ├── Build: cargo build --release
    ├── Strip: llvm-strip (removes debug symbols)
    ├── Verify: ELF + dependency checks
    └── Release: GitHub Release with artifacts
```

**Key observation:** `aura-daemon` binary is only built in `release.yml`, not in `build-android.yml`. The dev workflow only builds `libaura_daemon.so` (cdylib).

---

## Hypothesis Test Matrix

| H | Hypothesis | Test Method | Confidence | Test Status |
|---|-----------|-------------|------------|-------------|
| H1 | LD_PRELOAD interference | `LD_PRELOAD="" ./binary` | 0.45 | ❌ RULED OUT |
| H2 | llvm-strip corrupts binary | Build WITHOUT llvm-strip | 0.35 | ⏳ PENDING |
| H3 | API level mismatch | Compare API 26 vs API 29 | 0.20 | ⏳ PENDING |
| H4 | TLS alignment mismatch | termux-elf-cleaner | 0.35 | ⚠️ PARTIAL (flags fixed, crash persists) |
| H5 | Rust nightly-2026-03-01 broken | Change to stable/older nightly | 0.30 | ⏳ PENDING |
| H6 | panic=abort + NDK r26b LTO crash | Disable panic=abort | 0.30 | ⏳ PENDING |

---

## Execution Phases

### Phase 1: Device Deep Diagnostics (IMMEDIATE)

**Purpose:** Gather more information from the device to narrow root cause.

**Device commands to run:**

```bash
cd ~/downloads

# 1. Check if termux-elf-cleaner actually changed the binary
ls -la aura-daemon
stat aura-daemon

# 2. Check library dependencies — does the binary load libraries successfully?
ldd aura-daemon 2>&1

# 3. Check if libraries are missing (critical for SIGSEGV)
ldd aura-daemon | grep "not found"

# 4. Inspect dynamic section
readelf -d aura-daemon

# 5. Check PT_TLS section
readelf -S aura-daemon | grep -i tls

# 6. Try running with strace (if available)
pkg install strace
strace ./aura-daemon --version 2>&1 | head -50
```

**Expected findings:**
- `ldd` output reveals missing libraries → H7 (missing dependency)
- `readelf -d` shows problematic dynamic entries → confirms H4
- `strace` shows where SIGSEGV happens → narrows to specific syscall

---

### Phase 2: Build Unstripped Binary (HIGH PRIORITY)

**Purpose:** Test H2 — does llvm-strip cause the crash?

**Approach:** Create a modified CI workflow that:
1. Builds the daemon binary WITHOUT the llvm-strip step
2. Uploads both stripped and unstripped versions as artifacts
3. User downloads unstripped version → tests on device

**Workflow modification:** Create `.github/workflows/release-test.yml`

```yaml
# AURA F001 Diagnostic Build — produces unstripped binary
name: F001 Diagnostic Build

on:
  workflow_dispatch:
    inputs:
      variant:
        description: 'Build variant'
        required: true
        default: 'unstripped'
        type: choice
        options:
          - unstripped
          - no-llvm-strip
          - unwind-panic

env:
  NDK_VERSION: r26b
  API_LEVEL: "26"
  TARGET: aarch64-linux-android

jobs:
  build-diagnostic:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          submodules: recursive

      - name: Install Rust nightly-2026-03-01
        uses: dtolnay/rust-toolchain@efa25f7f19611383d5b0ccf2d1c8914531636bf9
        with:
          toolchain: nightly-2026-03-01
          targets: aarch64-linux-android

      - name: Cache NDK
        uses: actions/cache@v4
        with:
          path: ~/android-ndk-r26b
          key: ndk-r26b-linux

      - name: Download NDK
        run: |
          if [ ! -d ~/android-ndk-r26b ]; then
            curl -fsSL https://dl.google.com/android/repository/android-ndk-r26b-linux.zip -o ndk.zip
            unzip -q ndk.zip -d ~
            rm ndk.zip
          fi

      - name: Configure cross-compilation
        run: |
          NDK_HOME=~/android-ndk-r26b
          TOOLCHAIN=$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin
          echo "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=${TOOLCHAIN}/aarch64-linux-android26-clang" >> $GITHUB_ENV
          echo "CC_aarch64_linux_android=${TOOLCHAIN}/aarch64-linux-android26-clang" >> $GITHUB_ENV
          echo "CXX_aarch64_linux_android=${TOOLCHAIN}/aarch64-linux-android26-clang++" >> $GITHUB_ENV
          echo "CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS=-L native=$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/aarch64-linux-android -L native=$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/aarch64-linux-android/26" >> $GITHUB_ENV

      - name: Build daemon binary
        run: |
          cargo build --release -p aura-daemon --target aarch64-linux-android
          # DO NOT STRIP — this is the diagnostic build

      - name: Upload unstripped binary
        uses: actions/upload-artifact@v4
        with:
          name: aura-daemon-unstripped
          path: target/aarch64-linux-android/release/aura-daemon
```

**Steps:**
1. Create this workflow file
2. Run it manually from GitHub Actions
3. Download artifact
4. Deploy to device → test
5. If unstripped works → H2 confirmed (llvm-strip is the issue)
6. If unstripped also crashes → H2 ruled out, move to H5/H6

---

### Phase 3: Rust Toolchain Variant (IF H2 NOT CONFIRMED)

**Purpose:** Test H5 — is `nightly-2026-03-01` broken for Android?

**Approach:** Build with `nightly-2025-06-01` (stable, pre-Rust 1.78 alignment checks)

```yaml
# In workflow_dispatch variant
toolchain: nightly-2025-06-01
```

**Steps:**
1. If unstripped binary also crashes → change Rust toolchain
2. Rebuild with nightly-2025-06-01
3. Test on device
4. If it works → Rust nightly-2026-03-01 is the root cause

---

### Phase 4: panic=unwind Variant (IF H2 + H5 NOT CONFIRMED)

**Purpose:** Test H6 — does `panic=abort` cause the startup crash?

**Approach:** Modify `Cargo.toml` to use `panic = "unwind"` for release builds

```toml
# In aura-daemon/Cargo.toml
[profile.release]
panic = "unwind"  # Instead of default abort
```

**Steps:**
1. If unstripped + old nightly also crashes → try panic=unwind
2. This changes crash behavior from abort to unwind (better for debugging too)

---

## Decision Matrix

```
                    ┌─────────────────────────────┐
                    │   UNSTRIPPED BINARY TEST    │
                    └──────────────┬──────────────┘
                                   │
                    ┌──────────────┴──────────────┐
                    ▼                              ▼
               WORKS (H2 confirmed)          STILL CRASHES
                    │                              │
         Remove llvm-strip              ┌─────────┴─────────┐
         from release.yml               ▼                   ▼
                                     OLD NIGHTLY        PANIC=UNWIND
                                       TEST               TEST
                                         │                   │
                                  WORKS (H5)       WORKS (H6)
                                    Change           Change panic
                                    Rust ver         setting
                                         │                   │
                                  ┌─────┴─────┐      ┌─────┴─────┐
                                  ▼           ▼      ▼           ▼
                               ALL FIX    PARTIAL   ALL FIX    PARTIAL
```

---

## Success Criteria

| Criterion | Definition |
|-----------|------------|
| F001 Resolved | Binary executes `--version` without SIGSEGV on device |
| H2 Resolved | Removing llvm-strip eliminates crash |
| H5 Resolved | Old nightly toolchain eliminates crash |
| H6 Resolved | panic=unwind eliminates crash |
| Regression Prevented | CI smoke test catches future runtime issues |
| Governance Complete | Binary contract gates in place (PR #17) |

---

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Cannot build locally | HIGH | HIGH | CI workflow approach (GitHub Actions) |
| GitHub Actions rate limit | MEDIUM | MEDIUM | Use workflow_dispatch (no rate limit for owner) |
| Unstripped binary also crashes | HIGH | LOW | Multiple hypothesis layers available |
| Root cause is NDK r26b itself | MEDIUM | HIGH | May need to upgrade NDK version |
| Fix requires different toolchain | MEDIUM | MEDIUM | Incremental testing of variants |

---

## Ownership

| Phase | Owner | Status |
|-------|-------|--------|
| Phase 1: Device Diagnostics | USER/DEVICE | PENDING |
| Phase 2: Build Unstripped Binary | PLATFORM ENG (AI) | READY TO EXECUTE |
| Phase 3: Rust Toolchain Variant | PLATFORM ENG | BACKLOG |
| Phase 4: panic=unwind | PLATFORM ENG | BACKLOG |
| Release Governance (PR #17) | RELEASE ENG | READY TO MERGE |

---

## Immediate Next Step

**For User (Device):** Run Phase 1 device diagnostic commands:
```bash
cd ~/downloads
ldd aura-daemon 2>&1
ls -la aura-daemon
```

**For AI (Platform Engineering):** Create `.github/workflows/release-test.yml` and execute.

---

*This plan follows enterprise-grade systematic failure resolution methodology. No speculation without test, no claims without evidence.*
