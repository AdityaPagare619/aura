# AURA v4 Failure Taxonomy

**Document**: `docs/build/FAILURE_TAXONOMY.md`  
**Purpose**: Complete failure classification F001-F015 with signatures, diagnostics, and resolution paths  
**Status**: FOUNDATIONAL — All future failures MUST be classified against this taxonomy  
**Created**: 2026-03-22  
**Owner**: DevOps Release Charter  

---

## Philosophy

> **"A failure without a taxonomy code is a failure that will happen again."**

Every failure in AURA's lifecycle (build, test, deploy, runtime) is classified by its ROOT CAUSE layer, not its SYMPTOM. This taxonomy enables:
- **Instant recognition** — F001 SIGSEGV means one specific thing, not "crash"
- **Correct assignment** — Device failures go to Runtime Platform, not Code changes
- **Prevention planning** — Each F-code has an explicit prevention mechanism
- **Historical correlation** — "F001 again? Check F001 fixes before doing anything new"

This taxonomy was built from real failures documented in `ISSUE-LOG.md` and `AURA-v4-COMPREHENSIVE-AUDIT.md`. Every F-code represents a failure that was encountered, fixed ad-hoc, then re-encountered. The taxonomy exists so we stop repeating the cycle.

---

## Taxonomy Structure

Each failure class follows this template:

```
F### — Failure Name
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        [Build | CI | Runtime | Device | Process]
Symptom:      [What was observed]
Signature:    [How to detect it in logs/output]
Root Cause:   [Why it happened — THE LAYER IT BELONGS TO]
Environment:  [Which environments trigger this]
Prevention:   [What prevents this from recurring]
Detection:   [How CI/monitoring detects it]
Resolution:  [Immediate fix]
Evidence:    [Prior occurrences]
```

---

## Build System Failures (F001-F006)

These failures occur during the compilation, linking, and artifact production phases. They are the RESPONSIBILITY of the Build Infrastructure Charter.

---

### F001 — SEGMENTATION FAULT AT RUNTIME

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        Runtime
Symptom:      Binary crashes with SIGSEGV immediately on launch
Signature:    SIGSEGV signal, address in 0x0 range, crash in early main
Root Cause:   ABI mismatch between build environment (glibc Linux) and 
              runtime environment (bionic Android). Binary was compiled
              correctly but linked against wrong C library.
Environment:  Builds on Linux x86_64 (CI), runs on Android ARM64 (device)
Prevention:   Target-aware cross-compilation with NDK sysroot
              Static linking where possible
              Device testing in CI pipeline (NOT optional)
Detection:    CI must execute binary on target device before release
              CI running binary on Linux IS NOT VALID DETECTION
Evidence:     v4.0.0-alpha.1 through alpha.5 — CI green, device crashed
              Root cause identified in COMPREHENSIVE-AUDIT.md
Resolution:   Cross-compile with correct NDK toolchain. Verify with:
              file aura-daemon (shows correct architecture)
              readelf -d aura-daemon (shows correct dynamic linker)
              ldd aura-daemon (on device — shows bionic, not glibc)
              Run binary on actual device. If SIGSEGV, F001.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

**Decision Path**:  
```
Is binary crashing on device? → Is architecture correct? → Is linker correct? → F001
```

---

### F002 — ARTIFACT NOT FOUND

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        CI / Build
Symptom:      CI workflow cannot find expected artifact (binary, library, tool)
Signature:    "file not found", "no such file", "artifact does not exist"
Root Cause:   Build step failed silently, or artifact path misconfigured,
              or artifact not uploaded to expected location
Environment:  Any CI environment
Prevention:   Explicit artifact validation in build stage
              All artifacts checked for existence before proceeding
              Build logs show artifact creation confirmation
Detection:    CI workflow fails at artifact-dependent step
              ls or test -f commands fail
Evidence:     Multiple CI runs where binary step succeeded but 
              subsequent steps could not find binary
Resolution:   Validate build output. Check WORKFLOW_DIRECTORY. 
              Ensure artifact upload step succeeded.
              Verify artifact path matches expected path in next step.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

### F003 — DEPENDENCY MISMATCH

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        Build
Symptom:      Compilation fails with version conflicts, missing features,
              or incompatible crate versions
Signature:    "incompatible versions", "feature not found", 
              "package not found", version conflict messages
Root Cause:   Cargo.lock was not updated, or workspace dependencies
              have conflicting version requirements, or features
              were added/removed without lockfile update
Environment:  Local builds, CI builds
Prevention:   cargo update before significant changes
              Lockfile committed with all changes
              Feature flags documented and tested together
Detection:    cargo build fails with dependency resolution errors
Evidence:     curl_backend feature work — reqwest vs curl feature
              conflicts required workspace-wide feature coordination
Resolution:   Run cargo update to resolve versions.
              Check Cargo.lock is committed.
              Verify feature flags are coherent across workspace.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

### F004 — ABI MISMATCH

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        Build / Runtime
Symptom:      Linker errors, symbol not found, undefined reference,
              or runtime errors about missing symbols
Signature:    "undefined symbol", "ABI version mismatch", 
              "incompatible ELF"
Root Cause:   Code compiled against different ABI than runtime library.
              Typically: C library version mismatch (glibc vs bionic),
              or Rust crate compiled for different target than used.
Environment:  Cross-compilation environments, environments with
              multiple C library implementations
Prevention:   NDK sysroot used consistently for all compilation
              Target triple verified before build
              Static linking for critical dependencies
Detection:    Linker errors during build, or runtime library errors
Evidence:     F001 was root-caused to ABI mismatch (glibc compiled
              for bionic runtime). F004 is the general case.
Resolution:   Rebuild with correct sysroot. Verify target triple.
              Use armv8a-linux-android from NDK, not generic aarch64.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

### F005 — LINKER OR ENTRYPOINT FAILURE

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        Build
Symptom:      Binary will not start, "cannot execute binary file",
              wrong interpreter, wrong ELF class
Signature:    "Exec format error", "wrong ELF class",
              "cannot find entry point"
Root Cause:   Cross-compilation produced wrong ELF format, or
              entry point not correctly set, or shebang wrong
Environment:  Cross-compilation to different architecture or OS
Prevention:   Target triple explicitly set in all build commands
              Build artifacts validated with file and readelf
              Binary tested on target before release
Detection:    Binary cannot be executed on target system
Evidence:     Cross-compilation to Android without proper target
Resolution:   Set correct target triple: 
              cargo build --target aarch64-linux-android
              Verify with: file aura-daemon
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

### F006 — FEATURE GATE CONFLICT

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        Build
Symptom:      Compilation fails with feature-related errors, 
              cfg attributes in invalid positions, 
              unresolved imports when features switched
Signature:    "invalid `cfg` attribute", "unresolved import",
              "feature not found", "trait not satisfied"
Root Cause:   Feature flags used incorrectly — cfg on use statements,
              cfg inside functions, mutually exclusive features
              not properly gated, async_trait issues when features
              switch compilation paths
Environment:  Builds with non-default feature flags
Prevention:   Features only on actual feature-gated code
              No cfg inside use blocks
              No cfg on items inside impl blocks without feature
              All feature combinations tested in CI
Detection:    cargo build --features X fails
Evidence:     curl_backend work — async_trait dependency issues
              when switching between reqwest and curl backends
Resolution:   Redesign feature architecture. Sync-only traits
              avoid async_trait complexity. Test all feature
              combinations: cargo build --all-features
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

## Runtime Platform Failures (F007-F010)

These failures occur when the binary is running but encounters environmental issues. They are the RESPONSIBILITY of the Runtime Platform Charter.

---

### F007 — RUNTIME CRASH BEFORE BOOT

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        Runtime
Symptom:      Binary starts but crashes during initialization,
              before any user-facing functionality
Signature:    Crash, panic, or exit with code != 0 in first 5 seconds
Root Cause:   Config file missing/corrupt, permission denied on data
              directory, required environment variables not set,
              or panic in early initialization code
Environment:  Device running binary for first time, or after update
Prevention:   Boot stage logging (5 stages documented in build contract)
              Config file validation on startup
              Clear error messages for missing prerequisites
Detection:    Binary exit code checked immediately after launch
              Boot stage logs captured to file
Evidence:     Generic panic messages with no context about which
              stage failed or what was missing
Resolution:   Run with RUST_BACKTRACE=full. Check logs from each
              boot stage. Validate config file exists and is valid.
              Check file permissions on data directory.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

### F008 — INSTALLER LOGIC DRIFT

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        Runtime / Device
Symptom:      Installation succeeds but binary does not work,
              or installation corrupts existing working binary,
              or installation overwrites incompatible version
Signature:    Binary present but crashes, version mismatch in logs,
              working binary replaced by broken one
Root Cause:   Installation script does not verify prerequisites,
              or does not check existing version compatibility,
              or does not backup before overwrite
Environment:  Device with existing installation, automated updates
Prevention:   Pre-install checklist validated
              Version compatibility checked before overwrite
              Backup of existing binary before install
              Rollback path if install fails
Detection:    Binary crashes after install, or version mismatch reported
Evidence:     Multiple instances of "install worked but nothing works"
Resolution:   Do NOT install over broken binary. First fix the broken
              state, then install. Use --force only after validation.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

### F009 — TOOLCHAIN INSTALLATION FAILURE

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        Device / Setup
Symptom:      Cannot install or configure Rust toolchain on device,
              rustup panics, cargo fails, wrong rustc version
Signature:    "rustup default stable" panics with rustls error,
              "toolchain not installed", version mismatch
Root Cause:   aarch64-unknown-linux-android is Tier 3 target —
              Rust Project does NOT publish rustc/cargo for Android.
              Native toolchain builds NOT supported officially.
              Third-party Rust installations conflict.
Environment:  Termux on Android ARM64
Prevention:   Do NOT use rustup on Android/Termux. Use pkg install rust.
              Do NOT install rustup alongside pkg-installed Rust.
              Document that Android native compilation is NOT supported.
Detection:    rustup commands fail or panic
Evidence:     rustup 1.29.0 panics with rustls-platform-verifier
              error on Termux. pkg-installed Rust 1.93.1 works.
Resolution:   Remove rustup: rm -rf ~/.rustup ~/.cargo
              Use only: pkg install rust
              For cross-compilation: build on Linux, deploy to Android
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

### F010 — RUNTIME ENVIRONMENT MISMATCH

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        Runtime
Symptom:      Binary runs but fails when accessing system resources,
              network requests fail, file operations fail
Signature:    Permission denied, connection refused, timeout,
              resource not found at runtime but present in build env
Root Cause:   Runtime environment does not provide expected resources.
              Paths differ, permissions differ, network topology differs.
              Build assumed Linux paths but runtime is Android/Termux.
Environment:  Cross-environment deployment (Linux build → Android run)
Prevention:   Runtime environment contract defined before development
              All paths configurable, not hardcoded
              Graceful degradation when resources unavailable
Detection:    Binary works on build machine, fails on device
Evidence:     Paths like /usr/local vs /data/data/com.termux/files
Resolution:   Use Termux-specific paths. Check $PREFIX environment.
              All file paths should be relative to $PREFIX on Termux.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

## Observability Failures (F011-F012)

These failures are about the ABSENCE of information when failures occur. They are the RESPONSIBILITY of the QA Validation Charter.

---

### F011 — OBSERVABILITY GAP

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        Process / QA
Symptom:      Failure occurs but no logs, no crash dump, no way to
              diagnose what happened
Signature:    Silent failure, exit code 0 but nothing happened,
              "it just doesn't work"
Root Cause:   No logging implemented in critical code paths,
              no panic handler, no crash signal handler,
              no structured error propagation
Environment:  Any environment, especially production
Prevention:   Boot stage logging mandatory in all binaries
              All errors logged with context before propagation
              Panic handler that captures backtrace
              Crash dump written to local file on failure
Detection:    User reports "it crashed with no error message"
Evidence:     Multiple reports of "SIGSEGV with no logs"
Resolution:   Add logging. Implement boot stage checkpoints.
              Capture RUST_BACKTRACE on panic.
              Write crash dump to /sdcard/AURA/crash-DATE.txt
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

### F012 — TEST COVERAGE GAP

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        QA / Process
Symptom:      Tests pass but critical failure still occurs in production
Signature:    "tests passed but bug still happened", coverage shows
              gaps in critical paths, edge cases not tested
Root Cause:   Tests only cover happy path, or tests run in wrong
              environment, or tests don't verify actual behavior
Environment:  Local/CI testing that doesn't match device reality
Prevention:   Device-based testing mandatory for all releases
              Tests must run on target environment, not simulated
              Edge case catalog maintained and tested
Detection:    Bug found in production that was "covered by tests"
Evidence:     F001 SIGSEGV — unit tests passed, integration tests
              passed, but device crashed
Resolution:   Tests must be run on target device. CI tests on Linux
              are BUILD verification only, not VALIDATION.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

## Process/Governance Failures (F013-F015)

These failures are about team process, not technical systems. They are the RESPONSIBILITY of all charters.

---

### F013 — RELEASE GOVERNANCE FAILURE

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        Process
Symptom:      Release shipped without required validation,
              CI green but device broken, hotfixes bypass review
Signature:    "it was just a small fix", "CI passed so it's fine",
              "we need this out now"
Root Cause:   Pressure to ship overrides process. CI seen as 
              source of truth. Device testing skipped for speed.
              Not following release gates.
Environment:  Any environment with time pressure
Prevention:   Release gates are MANDATORY, not optional
              CI green is BUILD signal, not RELEASE signal
              Device testing is part of definition of done
Detection:    Bug in released version that passed CI but failed device
Evidence:     8 alpha releases shipped with CI green and device broken
Resolution:   STOP the release. Follow release gate checklist.
              Device testing is not optional. CI != validation.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

### F014 — REGRESSION FROM AD-HOC FIX

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        Process
Symptom:      Bug fixed, later reappears, or fix causes new bug
Signature:    "we fixed this before", "regression", "this worked before"
Root Cause:   Fix was not understood, was temporary, or affected
              shared code without proper analysis. Fix addressed
              symptom not root cause.
Environment:  Any code change without proper failure analysis
Prevention:   Every fix must identify failure code (F001-F015)
              Every fix must have test that would have caught it
              Root cause analysis before any code change
Detection:    Same failure occurs again after "fix"
Evidence:     F001 SIGSEGV fixed in alpha.6, appeared again in later
              versions due to different root cause (Rust runtime)
Resolution:   Do NOT fix without failure taxonomy code.
              If you don't know F-code, you don't know root cause.
              Without root cause, any "fix" is guessing.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

### F015 — ARCHITECTURAL DECAY

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Layer:        Process / Architecture
Symptom:      Code works but cannot be maintained, features take
              increasingly long to implement, bugs increasingly
              frequent in specific subsystems
Signature:    "this module is a mess", circular dependencies,
              feature flags everywhere, async/sync confusion
Root Cause:   Architectural decisions made under pressure without
              understanding long-term consequences. Technical debt
              accumulated without acknowledgment.
Environment:  All long-lived codebases
Prevention:   Architecture review before major changes
              Technical debt tracked and scheduled for resolution
              Architectural decision records (ADRs) for significant choices
Detection:    Velocity decreasing, bug rate increasing, fear of changes
Evidence:     curl_backend work — feature-gated async trait architecture
              created more problems than it solved
Resolution:   STOP adding features. Fix architecture first.
              Redesign curl_backend as sync-only trait.
              Technical debt is a debt — it must be paid.
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

## Failure Decision Tree

When a failure occurs, follow this tree to classify it:

```
FAILURE ENCOUNTERED
        │
        ▼
┌─────────────────────────────────────────┐
│ What layer does this failure belong to? │
└─────────────────────────────────────────┘
        │
        ├─► Build System ──────► F002-F006
        │                              │
        │   Artifact missing? ───────► F002
        │   Dependency conflict? ────► F003
        │   ABI mismatch? ─────────► F004
        │   Linker error? ─────────► F005
        │   Feature conflict? ─────► F006
        │
        ├─► Runtime Platform ───► F001, F007-F010
        │                              │
        │   SIGSEGV on device? ─────► F001
        │   Crash before boot? ─────► F007
        │   Install corrupts? ───────► F008
        │   Toolchain fails? ───────► F009
        │   Environment mismatch? ──► F010
        │
        ├─► Observability ────────► F011-F012
        │                              │
        │   No logs on failure? ─────► F011
        │   Tests passed but fails? ─► F012
        │
        └─► Process/Governance ──► F013-F015
                                   │
        │   Released without device test? ─► F013
        │   Bug came back after fix? ──────► F014
        │   Architecture causing bugs? ────► F015
```

---

## Failure Code Registry

| Code | Name | Layer | Last Occurrence | Status |
|------|------|-------|-----------------|--------|
| F001 | SEGMENTATION FAULT | Runtime | alpha.5 | ROOT CAUSE: ABI mismatch |
| F002 | ARTIFACT NOT FOUND | Build | Ongoing | Prevented by artifact validation |
| F003 | DEPENDENCY MISMATCH | Build | Ongoing | Prevented by cargo update discipline |
| F004 | ABI MISMATCH | Build | alpha.5 | Same root cause as F001 |
| F005 | LINKER/ENTRYPOINT | Build | alpha.3 | Prevented by target triple verification |
| F006 | FEATURE GATE CONFLICT | Build | Ongoing | curl_backend redesign in progress |
| F007 | RUNTIME CRASH BEFORE BOOT | Runtime | Ongoing | Boot stage logging planned |
| F008 | INSTALLER LOGIC DRIFT | Runtime | Ongoing | Pre-install checklist planned |
| F009 | TOOLCHAIN INSTALLATION | Device | 2026-03 | Documented: rustup broken on Termux |
| F010 | RUNTIME ENVIRONMENT MISMATCH | Runtime | Ongoing | Path configuration work in progress |
| F011 | OBSERVABILITY GAP | QA | Ongoing | Privacy-preserving telemetry planned |
| F012 | TEST COVERAGE GAP | QA | alpha.5 | Device testing mandated |
| F013 | RELEASE GOVERNANCE FAILURE | Process | alpha.8 | CI green ≠ release ready |
| F014 | REGRESSION FROM AD-HOC FIX | Process | Ongoing | Taxonomy prevents this |
| F015 | ARCHITECTURAL DECAY | Architecture | Ongoing | curl_backend redesign in progress |

---

## How to Use This Taxonomy

### When Filing a Bug Report
1. Identify the failure code (F001-F015)
2. If no code fits, the failure is not yet classified — STOP and classify first
3. Include the F-code in the issue title: "[F001] Binary crashes on device"

### When Fixing a Bug
1. Verify the failure code
2. Check Prevention column — implement prevention first
3. Fix root cause, not symptom
4. Add test that would have caught this failure
5. Update this document if new failure mode discovered

### When Reviewing a Release
1. For each failure in the release, verify F-code
2. Check Prevention column was implemented
3. If Prevention not implemented, release is NOT COMPLETE

### When Deciding What to Fix
1. Check failure codes in order of impact
2. F001-F008 (systemic) before F009-F015 (process)
3. If fix doesn't match F-code, you're fixing wrong thing

---

## Anti-Patterns This Taxonomy Prevents

| Anti-Pattern | Why It's Wrong | Correct Approach |
|-------------|---------------|-------------------|
| "CI is green, ship it" | CI = build, not validation | Device test required |
| "It works on my machine" | Machine != target environment | Test on actual device |
| "Quick fix, don't need tests" | Quick fixes = regressions | Every fix needs F-code + test |
| "We know this bug" | Knowing ≠ prevented | F-code + prevention implemented |
| "Let's add a feature" | Architecture already decaying | Fix F015 first |
| "rustup install rust" | rustup broken on Android | pkg install rust only |
| "Use async_trait for flexibility" | Complexity cost > flexibility benefit | Sync-only traits |
| "Feature flag everything" | Flags create F006 | Feature boundaries = module boundaries |

---

## Evolution of This Document

This taxonomy is a LIVING DOCUMENT. When a new failure mode is discovered:

1. Document the failure in ISSUE-LOG.md with full context
2. Classify it against this taxonomy
3. If no code fits, create new F-code (F016, F017, etc.)
4. Add prevention mechanism
5. Add test that detects it
6. Update this document

**The goal is zero unclassified failures.**

---

## Relationship to Other Documents

| Document | Purpose |
|----------|---------|
| `CONTRACT.md` | What the system promises to deliver |
| `FAILURE_TAXONOMY.md` | **THIS DOCUMENT** — What can go wrong |
| `INFRA-WORK-QUALITY.txt` | Team charters, build policies |
| `ISSUE-LOG.md` | Historical failures with context |
| `AURA-v4-COMPREHENSIVE-AUDIT.md` | All versions, all changes, all failures |

**CONTRACT defines success. TAXONOMY defines failure modes. Both are required.**
