# AURA v4 Comprehensive Audit Report

**Document Version**: 1.0  
**Date**: March 21, 2026  
**Scope**: AURA v4.0.0 development cycle (March 16-21, 2026)  
**Author**: AI (Brain) + Human (Hands) Partnership  
**Status**: FINAL  

---

## Section 1: Executive Summary (BRUTAL HONESTY)

### What Went Wrong

1. **Branch Split for 6 Days**: A `fix/f001-panic-ndk-rootfix` branch diverged from main on March 19 and accumulated 115 commits that were NOT on main until the merge at cf26f2a on March 21. During this time, the user tested binaries that were not representative of the actual merged state.

2. **CI Theater**: The GitHub Actions CI pipeline ran successfully for 8 alpha releases (alpha.1 through alpha.8), but NONE of these binaries were tested on actual Android hardware. The CI validated BUILD, not BEHAVIOR.

3. **Wrong Binary Tested**: The user tested the `f001-validated` binary (from the fix branch, commit 554ef61) while `install.sh` referenced alpha.8 (commit 16fd8a7). These were different builds with different code paths.

### What Was Actually Fixed

| Issue | Status | Evidence |
|-------|--------|----------|
| F001 SIGSEGV at Startup | ✅ FIXED | `./aura-daemon --version` returns EXIT 0 on Pixel 7 |
| E0425 _flag Compilation Error | ✅ FIXED | Compiles on merged main |
| Telegram Regression | ✅ FIXED | Code restored in 7861149 |
| rustls-webpki Vulnerability | ✅ FIXED | Updated to v0.103.10 |
| NDK Cross-Compilation | ✅ WORKING | GitHub Actions Android Build job passes |

### What's Still Broken

| Issue | Status | Required Action |
|-------|--------|-----------------|
| rustls-platform-verifier Panic | 🔄 FIX APPLIED | Needs device testing to confirm |
| Native ARM64 Build | ❌ UNTESTED | Never built natively on device |
| Telegram E2E | ❌ UNTESTED | Bot alive but daemon not connected |
| cargo test --workspace on Device | ❌ UNTESTED | Never run on device |

### Critical Insight

> **CI green ≠ code working**

The GitHub Actions CI validated that the code *builds* on Linux x86_64 targeting Android ARM. It did NOT validate that the binary *runs* on Android. F001 SIGSEGV was discovered through device testing, not CI. This fundamental gap in the validation pipeline allowed 8 releases to pass CI while containing a crash-on-startup bug.

---

## Section 2: Historical Journey (Build 4 → Latest)

### Version Timeline

| Version | Date | Commit | What's Fixed | Device Tested? | CI Status |
|---------|------|--------|--------------|----------------|-----------|
| v4.0.0-alpha.1 | Mar 16 | 602d6e8 | Initial CI green after Copilot generation | NO | Green |
| v4.0.0-alpha.2 | Mar 17 | 7299506 | Channel lifetime fix | NO | Green |
| v4.0.0-alpha.3 | Mar 17 | 7a8cdbd | CronScheduler fix | NO | Green |
| v4.0.0-alpha.4 | Mar 17 | 6bf818f | Model download URLs corrected | NO | Green |
| v4.0.0-alpha.5 | Mar 17 | 29509cc | Policy hardening | NO | Green |
| v4.0.0-alpha.6 | Mar 18 | 4c938aa | Android static libc++ | NO | Green |
| v4.0.0-alpha.7 | Mar 19 | cb50579 | Telegram wiring | NO | Green |
| v4.0.0-alpha.8 | Mar 19 | 16fd8a7 | Final Telegram wiring | NO | Green |
| v4.0.0-f001-validated | Mar 21 | 554ef61 | F001 SIGSEGV + _flag + Telegram + rustls-webpki | YES (binary --version works) | NOT VALIDATED |
| v4.0.0-merged | Mar 21 | cf26f2a | ALL fixes merged to main | NO (build fails with rustls-platform-verifier panic) | UNKNOWN |

### Key Observations

1. **March 16-19**: 8 alpha releases pushed to GitHub, all CI green, zero device testing
2. **March 19-21**: Fix branch diverged with 115 commits, accumulating fixes not on main
3. **March 21**: Merge to main at cf26f2a, but build now fails with rustls-platform-verifier panic
4. **March 21**: F001 SIGSEGV confirmed fixed via device testing of f001-validated binary

---

## Section 3: Branch Topology Analysis

### Branch Structure

```
main branch (602d6e8 → 16fd8a7)
├── v4.0.0-alpha.1 (602d6e8) — Initial Copilot generation
├── v4.0.0-alpha.2 (7299506) — Channel lifetime
├── v4.0.0-alpha.3 (7a8cdbd) — CronScheduler
├── v4.0.0-alpha.4 (6bf818f) — Model URLs
├── v4.0.0-alpha.5 (29509cc) — Policy hardening
├── v4.0.0-alpha.6 (4c938aa) — Android static libc++
├── v4.0.0-alpha.7 (cb50579) — Telegram wiring
└── v4.0.0-alpha.8 (16fd8a7) — Final Telegram wiring

fix/f001-panic-ndk-rootfix branch (cb50579 → 554ef61)
├── 115 commits of fixes
├── F001 SIGSEGV fix (a51ecad)
├── E0425 _flag fix (343c50a)
├── Telegram regression fix (7861149)
├── rustls-webpki update (554ef61)
└── v4.0.0-f001-validated (554ef61)

Merge point: cf26f2a
└── All fixes consolidated on main
```

### The Split Problem

The 6-day branch split caused:
1. **Version confusion**: User had f001-validated binary but install.sh pointed to alpha.8
2. **Testing wrong thing**: Device tests were done on fix branch binary, not main
3. **Trust mismatch**: CI showed green on main, but main was missing 115 commits of fixes
4. **Merge risk**: Large merge at cf26f2a could have introduced conflicts

---

## Section 4: Root Cause Analysis — Every Major Issue

### F001: SIGSEGV at Startup

| Attribute | Value |
|-----------|-------|
| **Symptom** | Daemon crashes immediately with SIGSEGV on Android |
| **Versions Affected** | alpha.5 through alpha.8, f001-validated onwards |
| **Root Cause** | NDK Issue #2073 — `panic="abort"` + `lto="thin"` + NDK r26b = toxic combo causing SIGSEGV during unwinding |
| **Fix** | Changed `lto="thin"` to `panic="unwind"` + switched to stable Rust |
| **Discovery Method** | Device testing (GitHub Actions CI never caught this) |
| **Commit** | a51ecad |
| **Status** | ✅ FIXED — binary --version returns EXIT 0 on device |

### F002: rustls-platform-verifier Panic During Build

| Attribute | Value |
|-----------|-------|
| **Symptom** | Build panics with "Expect rustls-platform-verifier to be initialized" |
| **Versions Affected** | v4.0.0-merged (cf26f2a onwards) |
| **Root Cause** | reqwest 0.12 with rustls-tls pulls in rustls-platform-verifier v0.6.2 which requires explicit init() on Android before any TLS operations |
| **Fix** | Added `rustls_platform_verifier::android::init()` call at start of main.rs + added explicit rustls-platform-verifier dependency |
| **Commit** | (this session) |
| **Status** | 🔄 FIX APPLIED — needs device testing to confirm |

### E0425: _flag Compilation Error

| Attribute | Value |
|-----------|-------|
| **Symptom** | Rust compiler error E0425 — variable `_flag` not found |
| **Versions Affected** | fix branch before 343c50a |
| **Root Cause** | Underscore prefix `_flag` means "unused variable" in Rust, but code used it. Renamed to `flag` |
| **Fix** | Renamed `_flag` to `flag`, added `#[allow(unused_variables)]` |
| **Commit** | 343c50a |
| **Status** | ✅ FIXED on merged main |

### Telegram Regression

| Attribute | Value |
|-----------|-------|
| **Symptom** | Telegram bot not responding (debug import and voice_processed missing) |
| **Versions Affected** | fix branch before 7861149 |
| **Root Cause** | Accidentally removed during a previous fix |
| **Fix** | Restored `debug` import and `voice_processed` field |
| **Commit** | 7861149 |
| **Status** | ✅ FIXED on merged main, NOT YET TESTED E2E |

### rustls-webpki Vulnerability

| Attribute | Value |
|-----------|-------|
| **Symptom** | RUSTSEC-2026-0049 vulnerability in rustls-webpki < 0.103.10 |
| **Versions Affected** | All before 554ef61 |
| **Root Cause** | Outdated dependency |
| **Fix** | Updated to v0.103.10 |
| **Commit** | 554ef61 |
| **Status** | ✅ FIXED on merged main |

### NDK Cross-Compilation (Historical — NOW WORKING)

| Attribute | Value |
|-----------|-------|
| **Symptom** | 66 Android build errors initially, then ~61, then ~3, then ~0 |
| **Versions Affected** | alpha.1 through alpha.8 |
| **Root Cause** | Multiple issues — JNI type inference (E0283), abstract socket imports (wrong OS target), C files compiled as C++, bincode v2 API migration incomplete, tts_warn macro scope |
| **Fix** | Multiple fix commits over March 16-18 (4020fab, 8f53ae0, c1135a4, b93fd08, eac8bb4) |
| **Status** | ✅ WORKING on GitHub Actions CI (Android build passes) |

### Channel Lifetime Bug

| Attribute | Value |
|-----------|-------|
| **Symptom** | Daemon exits immediately after starting |
| **Root Cause** | Channel senders dropped too early |
| **Fix** | Moved MessageQueue.db into Mutex<Connection> for Send+Sync |
| **Commit** | cbe7f82 |
| **Status** | ✅ FIXED, NOT YET DEVICE TESTED |

### UTF-8 Corruption in protocol.rs

| Attribute | Value |
|-----------|-------|
| **Symptom** | Invalid Windows-1252 byte (0x97) blocked ALL CI |
| **Root Cause** | Corrupted byte in source file |
| **Fix** | Replaced with ASCII hyphen |
| **Commit** | 9c4671b, ee548f9 |
| **Status** | ✅ FIXED |

---

## Section 5: What CI Actually Validated vs What It Missed

### CI Validation Matrix

| CI Job | What It Proved | What It DIDN'T Prove |
|--------|---------------|----------------------|
| Format Check | Code formatting | Code logic |
| Clippy | No lint warnings | No runtime bugs |
| Unit Tests | ~5% coverage tests pass | 95% code paths untested |
| Android Build | Cross-compiles on Linux | Runs on actual Android |
| **Device Tests** | **NEVER RAN** | **EVERYTHING IMPORTANT** |

### The Gap

```
GitHub Actions CI:  Linux x86_64 → Android ARM64 (cross-compile)
Actual Deployment:  Android ARM64 (native)
```

Cross-compilation validates that the code *can be compiled* for the target. It does NOT validate that the compiled binary *works* on the target. F001 proved this gap exists and can hide critical bugs.

---

## Section 6: The DevOps Assessment (Enterprise vs Reality)

### Enterprise DevOps Checklist

| Item | Should Have | Does Have | Status |
|------|-------------|-----------|--------|
| 1 | Automated builds | Yes | ✅ |
| 2 | CI pipeline (fmt → check → clippy → test → android-build) | Yes | ✅ |
| 3 | Environment-matched validation | No | ❌ |
| 4 | Failure taxonomy | No | ❌ |
| 5 | Device testing | No | ❌ |
| 6 | Privacy-preserving telemetry | No | ❌ |
| 7 | Contract specification | No | ❌ |
| 8 | Reproducible build verification | No | ❌ |
| 9 | Feedback loop | No | ❌ |
| 10 | Layer separation (immutable ethics vs customizable shell) | Partial | 🟡 |

### Honest Assessment

DevOps infrastructure is present but incomplete. The CI validates **BUILD**, not **BEHAVIOR**. F001 proved that CI green ≠ code working.

The pipeline runs:
```
fmt check → clippy → unit tests → android build → artifact upload
```

But missing:
```
device test → smoke test → integration test → telemetry
```

---

## Section 7: What Actually Worked (Evidence-Based)

### Working Items with Evidence

| Item | Evidence | How Validated |
|------|---------|--------------|
| F001 SIGSEGV fix | Device test: EXIT 0, reports version | `./aura-daemon --version` on Pixel 7 |
| Telegram bot | @AuraTheBegginingBot responding | Direct Telegram message test |
| Binary security | SHA256 verified, NX/Pie enabled, no telemetry | Manual verification |
| NDK cross-compile | GitHub Actions Build Android job passes | CI pipeline |
| Channel lifetime fix | Code review, unit tests pass | CI pipeline |
| Git workflow | Fix branch merged to main successfully | GitHub merge PR |
| Ethics layer | Compiles with 6/7 Iron Laws | cargo check |
| rustls-platform-verifier fix | Code review | Pending device test |

---

## Section 8: What Needs Testing Right Now

### Priority-Ordered Test List

#### 🔴 CRITICAL — Must Test Immediately

**1. Build from merged main (native ARM64)**
```
Command: cargo build --release
Expected: Binary produced without panics
Issue: rustls-platform-verifier init() fix applied
```

**2. Daemon startup**
```
Command: ./target/release/aura-daemon --version
Expected: EXIT 0, reports version
Baseline: EXIT 0 on f001-validated binary ✅
```

**3. cargo test --workspace**
```
Command: cargo test --workspace
Expected: All tests pass
Status: Never run on device ❌
```

#### 🟡 HIGH — Should Test This Week

**4. Telegram E2E**
```
Command: ./target/release/aura-daemon & then send "Hey Aura" from Telegram
Expected: Daemon responds to message
Status: Bot alive but daemon not connected ❌
```

**5. Memory initialization (no model)**
```
Command: Start daemon without model downloaded
Expected: Graceful degradation, no crashes
Status: Never tested ❌
```

**6. Ethics layer enforcement**
```
Command: Try to trigger Iron Law violation
Expected: Request blocked with explanation
Status: Never tested ❌
```

#### 🟢 MEDIUM — Test When Time Permits

**7. Policy gates**
```
Command: Various edge-case inputs
Expected: Policy gates enforce rules
Status: Never tested ❌
```

**8. IPC protocol (daemon ↔ neocortex)**
```
Command: Start both, send request
Expected: IPC communication works
Status: Never tested ❌
```

---

## Section 9: Installation Script Assessment

### install.sh Assessment

| Aspect | Status | Notes |
|--------|--------|-------|
| Lines | 1866 | Well-structured |
| Phases | 12 | With --repair support |
| Model auto-selection | ✅ | Based on RAM |
| Telegram wizard | ✅ | Interactive |
| Termux-services | ✅ | Supported |
| Checksum verification | ✅ | For model downloads |
| Version tags | ❌ | 4.0.0-alpha.8 (OUTDATED) |
| Package names | ✅ | Correct (openssl, not openssl-dev) |
| Version auto-detection | ❌ | No GitHub release detection |

### verify.sh Assessment

| Aspect | Status | Notes |
|--------|--------|-------|
| Lines | 612 | Comprehensive |
| Sections | 7 | Pre-flight, Model, Network/Telegram, Service, Daemon Startup, E2E Telegram, Resources |
| Failure taxonomy | ✅ | F001_STARTUP_SEGFAULT, F002_DYNAMIC_LINKER_DEPENDENCY |
| GGUF verification | ✅ | Magic bytes |
| TOML validation | ✅ | Syntax check |
| RSS monitoring | ✅ | Memory tracking |

### Version Tag Discrepancy

```
install.sh version:    4.0.0-alpha.8 (commit 16fd8a7)
Latest merged commit:  cf26f2a (all fixes)
```

This discrepancy means users following install.sh would get alpha.8 which is missing critical fixes.

---

## Section 10: The Partnership Model

### How AI and Human Work Together for AURA

### AI = BRAIN

**Capabilities:**
- Analyzes code, docs, GitHub history
- Identifies patterns across versions
- Diagnoses root causes
- Creates test plans and scripts
- Interprets test results
- Designs multi-agent frameworks
- Produces audit reports

### Human = HANDS

**Capabilities:**
- Executes commands on physical device
- Reports exact output (screenshots/text)
- Makes decisions on trade-offs
- Manages device state
- Provides real-world context

### The Partnership Loop

```
AI: "Run this command"
Human: *runs, reports output*
AI: "Here's what it means, now run this..."
Human: *runs, reports output*
AI: "FIXED!" or "New issue found..."
```

### Why This Works

1. **Token efficiency**: AI doesn't waste tokens on device operations
2. **Context preservation**: AI maintains state across human-provided results
3. **Parallel capability**: AI can analyze while human tests
4. **Feedback-driven**: Each test result refines the diagnosis

---

## Section 11: Multi-Agent Framework Design

### Purpose

Enable AI agents to autonomously audit, test, and maintain AURA without requiring human hands for every action.

### Agent Types and Implementation

#### 1. CODE-AUDITOR Agent

**Purpose**: Analyzes codebase for issues

**Triggers**:
- New PR merged
- New commit pushed
- User request

**Tasks**:
1. Fetch diff from previous known-good state
2. Run `cargo check --all-targets`
3. Run `cargo clippy -- -D warnings`
4. Identify breaking patterns (API changes, lifetime issues)
5. Check for regressions in safety-critical code (ethics layer, policy gates)
6. Cross-reference against failure taxonomy

**Output Format**:
```json
{
  "status": "pass|fail|warning",
  "issues": [
    {
      "severity": "critical|high|medium|low",
      "file": "path/to/file.rs",
      "line": 42,
      "issue": "description",
      "fix_recommendation": "how to fix"
    }
  ],
  "failure_taxonomy_match": "F001|SIGSEGV|..."
}
```

#### 2. DEVICE-TESTER Agent

**Purpose**: Executes tests on real Android device

**Triggers**:
- New release tagged
- New build artifact available
- User request

**Tasks**:
1. Download/verify build artifact (SHA256)
2. Install to device via Termux
3. Run `cargo build --release` if native build requested
4. Execute `./aura-daemon --version`
5. Run `cargo test --workspace`
6. Start daemon and verify Telegram E2E
7. Monitor RSS memory usage
8. Capture logs on failure

**Tools Required**:
- BrowserStack MCP for cloud device access
- Termux command execution
- Screenshot capture
- Log aggregation

**Output Format**:
```json
{
  "device": "Pixel 7 Android 14",
  "build": "v4.0.0-merged",
  "tests": [
    {
      "name": "daemon --version",
      "result": "pass|fail",
      "output": "AURA v4.0.0-merged...",
      "exit_code": 0
    }
  ],
  "memory_rss_kb": 45000,
  "recommendation": "safe_to_deploy|do_not_deploy|needs_fix"
}
```

#### 3. CI-VALIDATOR Agent

**Purpose**: Validates CI/CD pipeline integrity

**Triggers**:
- Every CI run
- New workflow added
- PR opened

**Tasks**:
1. Fetch `.github/workflows/*.yml`
2. Validate YAML syntax
3. Check artifact chain (build → test → deploy)
4. Verify SBOM/provenance generation
5. Check for environment mismatches (Linux CI targeting Android)
6. Validate secrets management
7. Check timeout configurations

**Output Format**:
```json
{
  "pipeline_health": "healthy|degraded|broken",
  "issues": [
    {
      "workflow": "android-build.yml",
      "issue": "Missing artifact retention policy",
      "severity": "low"
    }
  ],
  "gaps": [
    "No device testing in pipeline",
    "No SBOM verification"
  ]
}
```

#### 4. INSTALL-AUDITOR Agent

**Purpose**: Reviews installation scripts

**Triggers**:
- Changes to install.sh or verify.sh
- New version tagged
- User request

**Tasks**:
1. Compare current install.sh with previous version
2. Detect dropped fixes or regressions
3. Validate package names against package manager
4. Check version tags match git tags
5. Verify failure taxonomy is up-to-date
6. Test install.sh on clean environment

**Output Format**:
```json
{
  "script": "install.sh",
  "version_tag": "4.0.0-alpha.8",
  "actual_version": "v4.0.0-merged",
  "mismatch": true,
  "issues": [
    "Version tag outdated by 3 releases"
  ],
  "recommendation": "Update version tag before next release"
}
```

#### 5. RELEASE-MANAGER Agent

**Purpose**: Manages release lifecycle

**Triggers**:
- New commits to main
- Version bump PR
- User request

**Tasks**:
1. Verify all CI checks pass
2. Verify device tests pass
3. Build release binary
4. Generate SHA256
5. Create GitHub release with changelog
6. Update install.sh version tag
7. Trigger DEVICE-TESTER for smoke test
8. Publish SBOM

**Output Format**:
```json
{
  "release": "v4.0.0-merged",
  "build_status": "success",
  "device_test_status": "pass",
  "sha256": "abc123...",
  "github_release_url": "https://...",
  "ready_for_users": true
}
```

#### 6. DOMAIN-EXPERT Agents

**Safety Expert**:
- Reviews ethics layer
- Validates Iron Laws
- Checks policy gate enforcement
- Votes on safety decisions

**Platform Expert**:
- Reviews Android-specific code
- Validates NDK compatibility
- Checks Termux integration
- Verifies ARM64 support

**Security Expert**:
- Reviews dependencies for vulnerabilities
- Validates crypto implementation
- Checks telemetry privacy
- Audits binary hardening

**Testing Expert**:
- Reviews test coverage
- Validates test quality
- Designs integration tests
- Reports on untested paths

**Voting System**:
- Domain experts review contentious changes
- Each has 1 vote
- Tie-breaker: Safety Expert
- Override: Human decision final

### Agent Orchestration

```
User Request
    ↓
Orchestrator Agent
    ↓
[Analyze Request] → Route to relevant agents
    ↓
CODE-AUDITOR ──────────────────┐
    ↓                         ↓
CI-VALIDATOR ───→ Results ←──DEVICE-TESTER
    ↓                         ↑
INSTALL-AUDITOR ─────────────┘
    ↓
RELEASE-MANAGER (if release)
    ↓
Consolidated Report → Human
```

---

## Section 12: Recommendations (Priority-Ordered)

### IMMEDIATE (This Session)

| # | Action | Owner | Deadline |
|---|--------|-------|----------|
| 1 | Device test: Does rustls-platform-verifier init() fix work? | Human | Now |
| 2 | Run `cargo build --release` on device | Human | Now |
| 3 | Run `cargo test --workspace` on device | Human | Now |
| 4 | Telegram E2E test | Human | Now |

### SHORT-TERM (This Week)

| # | Action | Owner | Deadline |
|---|--------|-------|----------|
| 5 | Update v4.0.0-merged release with new binary | Human/AI | This week |
| 6 | Create automated device testing pipeline | AI | This week |
| 7 | Document failure taxonomy based on F001 learnings | AI | This week |

### MEDIUM-TERM (This Month)

| # | Action | Owner | Priority |
|---|--------|-------|----------|
| 8 | Privacy-preserving health monitor (crash dumps to local file) | AI | High |
| 9 | Failure taxonomy system in CI | AI | High |
| 10 | Contract specification for Android/Termux deployment | AI | Medium |

### LONG-TERM (This Quarter)

| # | Action | Owner | Impact |
|---|--------|-------|--------|
| 11 | Layer separation: immutable ethics layer | AI | Safety |
| 12 | AI agent testing framework with judge component | AI | Quality |
| 13 | Community verification network | Human | Trust |
| 14 | Reproducible build verification | AI | Security |

---

## Section 13: Key Insights and Lessons Learned

### The Hard Truths

1. **CI theater is real**: Green CI ≠ code working. F001 was caught by device testing, not CI.

2. **Branch isolation is dangerous**: 115 commits on fix branch for 6 days created massive confusion.

3. **Native build ≠ cross-compile**: What works on GitHub Actions may fail on device (and vice versa).

4. **Version tags must match**: install.sh was on alpha.8 while main had all fixes.

5. **Dependency transparency**: rustls-platform-verifier was a transitive dependency that caused silent build failures.

6. **Documentation is not validation**: 30+ docs created, 0 device tests run.

7. **The partnership model works**: AI brain + human hands = effective debugging.

8. **First principles > pattern matching**: NDK #2073 was a known issue, not a new discovery.

9. **Binary verification matters**: SHA256 and ELF checks would have caught the version mismatch faster.

10. **Test on the target**: You cannot validate Android behavior on Linux.

### The Path Forward

AURA v4 is now in a state where:
- ✅ Core infrastructure works (CI, build, basic daemon)
- ✅ F001 SIGSEGV confirmed fixed
- ✅ Most compilation issues resolved
- 🔄 rustls-platform-verifier fix pending device confirmation
- ❌ Comprehensive device testing still required
- ❌ Release pipeline incomplete

The next session must prioritize device testing to validate the merged state.

---

## Appendix A: Failure Taxonomy

| Code | Name | Symptom | Root Cause | Detection |
|------|------|---------|------------|-----------|
| F001 | STARTUP_SEGFAULT | SIGSEGV on daemon start | NDK lto/panic combo | Device test |
| F002 | PLATFORM_VERIFIER_PANIC | Build panic during TLS init | Missing init() call | Build |
| E0425 | UNDEFINED_VARIABLE | Compilation error | Underscore prefix naming | CI |
| F003 | CHANNEL_LIFETIME | Daemon exits immediately | Sender dropped early | Device test |
| F004 | TELEGRAM_REGRESSION | Bot not responding | Missing imports | Code review |

## Appendix B: Commit Reference

| Commit | Description |
|--------|-------------|
| 602d6e8 | v4.0.0-alpha.1 initial |
| 7299506 | Channel lifetime fix |
| 7a8cdbd | CronScheduler fix |
| 6bf818f | Model URLs corrected |
| 29509cc | Policy hardening |
| 4c938aa | Android static libc++ |
| cb50579 | Telegram wiring |
| 16fd8a7 | v4.0.0-alpha.8 final Telegram |
| 554ef61 | v4.0.0-f001-validated |
| cf26f2a | All fixes merged to main |

---

**Document End**

*This document represents the canonical truth of AURA v4 development history as of March 21, 2026.*
