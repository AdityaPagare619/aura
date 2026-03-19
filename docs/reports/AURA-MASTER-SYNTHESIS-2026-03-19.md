# AURA Engineering — Comprehensive Master Synthesis
## Session: All Evidence · All Verdicts · Honest Judgments

> **Created:** 2026-03-19 | **Ultimate Goal:** AURA must install, run, and function on real Android/Termux devices
> **Local HEAD:** `16fd8a7` (origin/main, alpha.8) | **Status:** F001 UNRESOLVED
> **CRITICAL RULE:** Every claim must be verified. Do NOT trust any agent (including Copilot) blindly.

---

## ═══════════════════════════════════════════════════════
## 🎯 THE ONE GOAL THAT MATTERS
## ═══════════════════════════════════════════════════════

**AURA must install, run, and function on real Android/Termux devices. Pre-built release must work without user-side burden.**

Everything else — governance, testing, architecture, Copilot PRs — is in service of this goal.

**Current Reality:** alpha.8 binaries crash on device with `F001_STARTUP_SEGFAULT`. Confirmed on TWO devices.

---

## ═══════════════════════════════════════════════════════
## 🔴 F001 — THE ONE BLOCKER
## ═══════════════════════════════════════════════════════

### What Is F001

- **Name:** `F001_STARTUP_SEGFAULT`
- **Symptom:** `SIGSEGV`, `SEGV_MAPERR`, fault addr `0x0000000000000000` (null pointer dereference)
- **Timing:** At binary entry point/startup code, BEFORE `main()` business logic
- **Scope:** Both `aura-daemon` AND `aura-neocortex` crash identically
- **Reproducible:** On TWO different physical devices (first device + V8 fresh device)
- **Evidence:** logcat shows `pc 0x5ad0b4` and `pc 0x5a72cc` — both inside the daemon binary
- **Clean binary:** SHA256 matches release assets exactly, ELF magic valid, all CI checks passed

### What F001 Is NOT

| Ruled Out | Evidence |
|-----------|----------|
| Download corruption | SHA256 matched published sidecars |
| Stale binary on device | Fresh device (V8) reproduced crash |
| Rust panic | SIGSEGV in native startup code, not panic |
| Missing library | ELF NEEDED deps all resolved (readelf passed) |
| File permission issue | Binary has execute permissions |
| FFI/init_ffi_backend | That would crash on inference, not startup |
| Model loading | Crash happens before any model loading |
| Tracing issue | Tracing init happens AFTER crash point |

### H1/H2/H3 Hypothesis Tree — NEVER COMPLETED

The forensics doc (correctly) designed a hypothesis tree with decision gates. **NONE were completed.**

#### H1 — Termux LD_PRELOAD Interaction (Confidence: 0.45)
**Test:** `LD_PRELOAD= ./aura-daemon --version`
**Evidence:** `LD_PRELOAD=/data/data/com.termux/files/usr/lib/libtermux-exec-ld-preload.so` set in environment. Preload path opens before crash (visible in strace). Both binaries fail identically.
**Status:** ❌ **NEVER TESTED** — Installer cleaned up artifacts before test could run
**Blocker:** Logs show `"No such file or directory"` because installer removed binary after probe failure

#### H2 — llvm-strip Invalidation (Confidence: 0.20)
**Test:** Compare stripped (release) vs unstripped (debug) binary runtime
**Evidence:** `release.yml` does `llvm-strip` on binaries (plus `strip=true` in Cargo.toml — double-strip). If strip removes something critical (program header, TLS template), startup could break.
**Status:** ❌ **NEVER TESTED**

#### H3 — Toolchain/ABI/Linker Mismatch (Confidence: 0.35)
**Test:** Build unstripped debug binary, deploy to device, capture full backtrace
**Evidence:** `.cargo/config.toml` uses `android21-clang`, CI uses `API_LEVEL="26"`. API level affects libc/libm/libpthread headers.
**Status:** ❌ **NEVER TESTED**

### The Critical Failure

The forensics doc (Section 6) explicitly describes these tests:
> "Run differential execution tests with and without Termux preload... If runtime passes when preload is removed..."

**But these tests were NEVER completed because:**
1. Installer removes binary after probe failure
2. Differential test scripts ran AFTER binary was gone
3. `04_daemon_no_preload.log`: "No such file or directory" — binary already removed
4. `05_daemon_min_env.log`: Same — binary already removed

### The Correct Path Forward

1. Build **unstripped debug binaries** from `16fd8a7` (same commit as alpha.8)
2. Deploy to device **WITHOUT install.sh** (manual deployment, bypass cleanup)
3. Run H1: `LD_PRELOAD= ./aura-daemon --version`
4. Run H2: Compare stripped vs unstripped runtime
5. Run H3: Build fresh debug binary and test
6. If H1 passes → patch install.sh with LD_PRELOAD sanitization wrapper
7. If H2 passes → remove llvm-strip step from release.yml
8. If H3 passes → identify and fix toolchain/ABI issue

---

## ═══════════════════════════════════════════════════════
## 📊 EVIDENCE TIERS
## ═══════════════════════════════════════════════════════

### Tier A — Real Device Evidence (Highest Priority)

| ID | Source | Key Fact |
|----|--------|----------|
| E-A1 | `06_logcat_filtered.log` | 59 lines, 13 crash events, identical SIGSEGV across daemon+neocortex |
| E-A2 | `V8/ALL_IN_ONE.txt` | Fresh device test: SHA256 verified, runtime crashed, binary then removed |
| E-A3 | `04_daemon_no_preload.log` | H1 test blocked — binary removed before test ran |
| E-A4 | `05_daemon_min_env.log` | H1 test blocked — binary removed before test ran |
| E-A5 | Postmortem doc | alpha.5→alpha.8 timeline, SEV-1 severity confirmed |

### Tier B — Release Artifact Evidence

| ID | Source | Key Fact |
|----|--------|----------|
| E-B1 | GitHub Release API | alpha.7 and alpha.8 daemon have IDENTICAL SHA256 (stale artifact) |
| E-B2 | GitHub Release API | alpha.7 and alpha.8 neocortex have IDENTICAL SHA256 (stale artifact) |
| E-B3 | CI Build history | All builds passed (alpha.5→alpha.8) — build was correct, runtime failed |

### Tier C — Source Code Evidence

| ID | Location | Key Fact |
|----|----------|----------|
| E-C1 | `Cargo.toml:36` | `panic = "abort"` — makes RUST_BACKTRACE useless |
| E-C2 | `.cargo/config.toml:9` | API 21 linker vs CI API 26 — mismatch |
| E-C3 | `release.yml` | No real-device execution step — CI never runs binary |
| E-C4 | `integration_tests.rs` | Not in lib.rs, duplicate test_helpers, 119 errors |
| E-C5 | `daemon/main.rs:88-107` | tracing init before Args::parse() |
| E-C6 | `neocortex/main.rs:133-155` | tracing init before Args::parse(), no --version flag |
| E-C7 | `model.rs` | `init_ffi_backend()` never called on Android |
| E-C8 | `daemon_core/shutdown.rs` | `graceful_shutdown()` implemented but never called |
| E-C9 | `model.rs` | headroom check compares available memory to full model size |
| E-C10 | `neocortex/main.rs` | shutdown listener `.expect()` — non-graceful on channel close |

---

## ═══════════════════════════════════════════════════════
## 🤖 COPILOT PR #17 — HONEST VERDICT
## ═══════════════════════════════════════════════════════

### ✅ What Copilot Got RIGHT

| Change | Evidence | Verdict |
|--------|----------|---------|
| Binary Contract gates | ELF magic/machine/type/deps/sections validation | ✅ CORRECT — good release gate |
| Stale-artifact detection | Compares SHA256 vs previous release | ✅ CORRECT — addresses duplicate alpha.7/alpha.8 |
| Truth bundle manifest | JSON provenance record | ✅ CORRECT — useful for forensics |
| ELF identity check | Magic + AArch64 validation in install.sh | ✅ CORRECT — catches wrong-arch artifacts |
| .cargo/config.toml android21→26 | Local matches CI API level | ✅ CORRECT — eliminates API skew |
| F001/F002/F099 taxonomy | Consistent failure codes | ✅ CORRECT — aligned with postmortem |
| model.rs TOCTOU fix | Re-checks backend after init failure | ✅ VALID defensive improvement |
| model.rs null-ptr guard | Guards model_ptr and ctx_ptr | ✅ VALID defensive improvement |

### ❌ What Copilot Got WRONG

| Claim | Reality |
|-------|---------|
| F001 caused by TOCTOU in load_model_ffi | ❌ WRONG — crash at binary entry, before model loading |
| init_ffi_backend() missing causes F001 | ❌ WRONG — that would crash on inference, not startup |
| "CI ALL PASS" | ❌ MISLEADING — true for old commit `d76b1fb`, false for current HEAD `535645e` |
| Drew causal conclusions about F001 | ❌ WRONG — never tested H1/H2/H3, just speculated |
| --version ordering issue found | ✅ Identified but NOT fixed |
| integration_tests.rs broken | ✅ Identified but NOT fixed |

### ⚠️ Critical Gap in PR #17

| Issue | Fact |
|-------|------|
| CI on current HEAD | `total_count: 0` — NO checks registered on `535645e` |
| Branch relationship | Copilot branch DIVERGENT from main — 4 commits behind, separate history |
| Mergeable state | `clean` (GitHub says mergeable), but needs rebase |

### Verdict on PR #17

**VALUABLE governance improvements. MUST NOT merge before CI validates HEAD commit. Must rebase onto current main first.**

---

## ═══════════════════════════════════════════════════════
## 📋 PR PORTFOLIO — HONEST VERDICTS
## ═══════════════════════════════════════════════════════

| PR | State | Size | CI | Base vs Main | Verdict |
|----|-------|------|----|--------------|---------|
| #17 | OPEN | +321/-28 | ❌ 0 checks on HEAD | DIVERGENT | ⚠️ Valuable, needs rebase+CI |
| #1 | OPEN | +5/-4 | ❌ 0 checks | 13 behind, DIRTY | ⚠️ Fix correct, needs rebase |
| #13 | OPEN | 0 files | N/A | 11 behind | ❌ DEAD — no progress |
| #4 | MERGED | 0 files | N/A | on main | ✅ Verified |
| #10 | MERGED | +17/-12 | N/A | on main | ✅ Verified |
| #12 | MERGED | various | N/A | on main | ✅ Verified |
| #14 | MERGED | various | N/A | on main | ✅ Verified |
| #15 | MERGED | various | N/A | on main | ✅ Verified |
| #16 | MERGED | various | N/A | on main | ✅ Verified |
| #11 | MERGED | +677/-5906 | N/A | on main | ⚠️ Valid fix, massive deletion |
| #9 | CLOSED | +39/-40 | N/A | superseded | Content reached main via other paths |

---

## ═══════════════════════════════════════════════════════
## 🔍 SYSTEMIC ARCHITECTURAL ISSUES
## ═══════════════════════════════════════════════════════

### Root Cause of F001 Shipping Undetected

The ENTIRE CI/CD pipeline (build, test, release) only:
- ✅ Compiles the code
- ✅ Links the binaries
- ✅ Validates ELF format
- ✅ Checks dependencies
- ❌ **NEVER executes the binary on any device**

This is why F001 passed CI: CI never ran the binary.

### Integration Tests Are Dead

- `integration_tests.rs` is NOT in `lib.rs` — never compiled
- Has duplicate `mod test_helpers` — won't compile even if included
- 119 compilation errors (wrong API paths, wrong field names)
- AURA has NEVER had working integration tests

### Panic=abort Makes Forensics Impossible

- `Cargo.toml:36`: `panic = "abort"`
- No global panic hook in either binary
- `catch_unwind` Err arms discard panic payload
- `RUST_BACKTRACE=1` completely useless in release mode
- Any future crash will produce zero diagnostic output

### Governance Architecture Is Correct But Incomplete

- State machine `dev → freeze → rc → stable` — good
- Hard gates G1-G4 — good design
- But RC gate (G3) has NEVER passed for any alpha release
- Stop-ship triggers defined but F001 still shipped

---

## ═══════════════════════════════════════════════════════
## ✅ WHAT'S GOOD (Keep These)
## ═══════════════════════════════════════════════════════

1. **Binary contract gates** (in PR #17) — prevent wrong-arch/corrupt artifacts
2. **Stale-artifact detection** (in PR #17) — catches duplicate releases
3. **Truth bundle manifest** (in PR #17) — forensic provenance
4. **ELF identity check** (in PR #17) — validates before chmod +x
5. **Install.sh fail-fast** (in main) — protects users from bad installs
6. **Failure taxonomy F001/F002/F099** — consistent error codes
7. **Graceful shutdown architecture** — code exists, just not wired
8. **Policy gates and observability** — architectural foundation solid
9. **Proactive and memory subsystems** — fully wired, just need device
10. **IPC and Telegram integration** — architecture correct, needs F001 fixed

---

## ═══════════════════════════════════════════════════════
## 📝 EXECUTION PLAN (Priority Order)
## ═══════════════════════════════════════════════════════

### P0 BLOCKER — F001 Resolution

**Step 0:** Build unstripped debug binaries from `16fd8a7`
```
# This requires NDK/cargo-ndk — most likely needs CI/GitHub Actions
git checkout 16fd8a7
cargo build --release -p aura-daemon -p aura-neocortex --target aarch64-linux-android
# Keep symbols: DON'T strip, or use --split-debuginfo
```

**Step 1:** Deploy to device WITHOUT install.sh
```
adb push aura-daemon /data/local/tmp/aura-daemon
adb push aura-neocortex /data/local/tmp/aura-neocortex
adb shell chmod +x /data/local/tmp/aura-daemon
adb shell chmod +x /data/local/tmp/aura-neocortex
```

**Step 2:** H1 Test — LD_PRELOAD differential
```
adb shell "LD_PRELOAD= /data/local/tmp/aura-daemon --version"
adb shell "LD_PRELOAD= /data/local/tmp/aura-neocortex --help"
```
- If PASS (version prints) → H1 confirmed → fix install.sh with LD_PRELOAD wrapper
- If FAIL → proceed to H2

**Step 3:** H2 Test — stripped vs unstripped
```
# Already running unstripped from Step 2
# If H1 failed, test stripped:
adb shell "/data/local/tmp/aura-daemon --version"
```
- If unstripped PASS and stripped FAIL → H2 confirmed → remove llvm-strip step
- If both FAIL → proceed to H3

**Step 4:** H3 Test — fresh toolchain build
```
# Build fresh debug binary with current toolchain
# Deploy and test
```
- If H3 passes → identify specific toolchain/ABI difference
- If H3 fails → deeper investigation needed

### P1 — PR #17 Merge (Can Parallel with P0)

**Already created worktree:** `../aura-pr17-rebased` on branch `copilot/pr17-rebased`

Steps:
1. Rebase copilot branch onto current main (in worktree)
2. Push to update PR head
3. Wait for GitHub Actions to run on new commit
4. Verify CI passes
5. Merge PR #17

### P1 — Panic Hook for Forensics

1. Change `Cargo.toml:36`: `panic = "abort"` → `panic = "unwind"`
2. Add `std::panic::set_hook()` to daemon `main.rs`
3. Add `std::panic::set_hook()` to neocortex `main.rs`
4. Fix `catch_unwind` Err arms to log payload
5. This won't fix F001 but makes future crashes visible

### P2 — Entry Point Fixes

1. Move `Args::parse()` BEFORE `tracing_subscriber::init()` in daemon
2. Move `Args::parse()` BEFORE `tracing_subscriber::init()` in neocortex
3. Add `--version` flag to neocortex
4. Commit as `fix(entrypoint): parse CLI before tracing init`

### P2 — Integration Tests Compilation

1. Remove duplicate `mod test_helpers;` from `integration_tests.rs`
2. Add `#[cfg(test)] mod integration_tests;` to `lib.rs`
3. Fix `SqliteConfig.path` → `db_path`
4. Fix `Episode.id` type
5. Fix remaining API errors
6. Mark Android-specific tests as `#[ignore]`

### P3 — Real-Device Smoke Test in CI

1. Add Android emulator step to `release.yml`
2. Run `aura-daemon --version` and `aura-neocortex --help`
3. Fail release if either crashes

---

## ═══════════════════════════════════════════════════════
## 🔑 KEY INSIGHTS (Things Easy to Miss)
## ═══════════════════════════════════════════════════════

1. **Copilot governance improvements are GOOD but don't fix F001** — binary contracts prevent future bad releases, but don't fix the current one

2. **The H1/H2/H3 hypothesis tree was designed correctly but NEVER completed** — the blocker was the installer's artifact cleanup, which was noted but not resolved

3. **CI green ≠ device green** — the entire CI pipeline never executes the binary, which is why F001 shipped undetected

4. **Copilot drew wrong causal conclusions** — TOCTOU/init_ffi_backend would affect inference, not startup

5. **panic=abort makes EVERYTHING invisible** — future crashes will produce zero diagnostics unless fixed

6. **integration_tests.rs is completely dead** — 119 errors, duplicate module, not compiled

7. **The neocortex config from prior session was lost** during repo sync to main

8. **PR #17 has NO CI on current HEAD** — the "CI passes" claim was true for an old commit only

9. **The forensics logs (04/05) prove H1 test was blocked** — binary removed before test could run

10. **All evidence points to binary construction issue** — toolchain/ABI/linker/strip, not application code

---

## ═══════════════════════════════════════════════════════
## 📁 EVIDENCE FILE LOCATIONS
## ═══════════════════════════════════════════════════════

### Real Device Evidence
- `C:\Users\Lenovo\aura_ci_fix\docs\reports\19-03-2026\06_logcat_filtered.log` — 59-line crash log (PRIMARY)
- `C:\Users\Lenovo\aura_ci_fix\docs\reports\19-03-2026\V8\ALL_IN_ONE.txt` — Fresh device test
- `C:\Users\Lenovo\aura_ci_fix\docs\reports\19-03-2026\04_daemon_no_preload.log` — H1 blocked
- `C:\Users\Lenovo\aura_ci_fix\docs\reports\19-03-2026\05_daemon_min_env.log` — H1 blocked

### Governance Docs
- `C:\Users\Lenovo\aura_ci_fix\docs\reports\AURA-ANDROID-REALDEVICE-FORENSICS-2026-03.md` — Hypothesis tree
- `C:\Users\Lenovo\aura_ci_fix\docs\reports\AURA-ANDROID-INCIDENT-POSTMORTEM-alpha5-alpha8-2026-03-19.md` — Full timeline
- `C:\Users\Lenovo\aura_ci_fix\docs\plans\AURA-RELEASE-GOVERNANCE-v2.md` — Governance design
- `C:\Users\Lenovo\aura_ci_fix\docs\plans\AURA-SYSTEM-TRANSFORMATION-ROADMAP-2026Q2.md` — Roadmap

### Source Code (origin/main at 16fd8a7)
- `C:\Users\Lenovo\aura\Cargo.toml` — panic=abort at line 36
- `C:\Users\Lenovo\aura\.cargo\config.toml` — API 21 at line 9
- `C:\Users\Lenovo\aura\crates\aura-daemon\src\bin\main.rs` — tracing before args at 88-107
- `C:\Users\Lenovo\aura\crates\aura-neocortex\src\main.rs` — tracing before args at 133-155, no --version
- `C:\Users\Lenovo\aura\crates\aura-daemon\src\integration_tests.rs` — broken, 119 errors
- `C:\Users\Lenovo\aura\.github\workflows\release.yml` — no device execution step

---

## ═══════════════════════════════════════════════════════
## 📌 THIS SESSION'S ACTIONS
## ═══════════════════════════════════════════════════════

### Done
1. ✅ Synced local repo to `origin/main` (`16fd8a7`, alpha.8)
2. ✅ Ran 5 parallel audit agents covering PR #17, entry points, integration tests, branch diff, panic handling
3. ✅ Verified PR #17 CI status via GitHub MCP — `total_count: 0` on HEAD
4. ✅ Verified branch divergence via git
5. ✅ Verified logcat evidence — 59 lines, 13 crash events, identical SIGSEGV
6. ✅ Verified H1 test was blocked — "No such file or directory" in 04/05 logs
7. ✅ Created comprehensive tracking doc
8. ✅ Created PR #17 rebase worktree (`../aura-pr17-rebased`)
9. ✅ Created this master synthesis doc

### In Progress
1. ⏳ PR #17 rebase (worktree ready, needs execution)
2. ⏳ Entry point fixes (daemon + neocortex)
3. ⏳ Panic hook addition
4. ⏳ Integration tests fix

### Pending
1. 🔴 F001 resolution — build unstripped binaries, complete H1/H2/H3 tree
2. 🔴 PR #17 rebase and merge
3. 🔴 Real-device smoke test in CI
4. 🟡 PR #1 resolution
5. 🟡 Neocortex config re-apply

---

*This document is the source of truth. Updated after each session.*
*Verdicts are based on evidence, not trust. Verify everything.*
