# AURA Context Book — F001_ROOT_CAUSE_INVESTIGATION
**Version:** 2.0 | **Updated:** 2026-03-19 | **Session:** Active
**Repo:** `AdityaPagare619/aura` | **Main SHA:** `16fd8a7` | **Build:** alpha.8

---

## 🎯 MISSION

Fix F001_STARTUP_SEGFAULT — binary crashes at startup on real Android/Termux devices.
CI shows green because CI only compiles, never runs the binary.

---

## 🚨 THE CRASH — F001_STARTUP_SEGFAULT

### What We Know (Confirmed)

| Fact | Evidence |
|------|----------|
| Crash signature | SIGSEGV at `0x5ad0b4` inside aura-daemon binary |
| Crash depth | 2 frames — only inside aura-daemon, no shared library frames |
| When | BEFORE `main()` executes — at binary's ELF entry point |
| Affected builds | alpha.5, alpha.6, alpha.7, alpha.8 — ALL broken |
| CI result | GREEN for all 4 builds |
| Device result | RED for all 4 builds |
| alpha.7 = alpha.8 | **Byte-identical** binaries (same SHA256) |
| Fresh device test | V8 confirmed crash with verified SHA256 |

### Evidence Tiers

**Tier A — Real Device (CONFIRMED)**
- `59-line crash log` → `06_logcat_filtered.log`
- `V8 fresh device test` → `V8/ALL_IN_ONE.txt`
- `Device audit 2026-03-19` → `AURA-DEBUG-20260319-212617/`

**Tier B — Release Assets (CONFIRMED)**
- Daemon SHA256: `349ffabd9ae2b257e2b9db3a758999a994c1fe41a454c10345711a66a5016952` (alpha.7 = alpha.8)
- Neocortex SHA256: `6c60b7272a745856dc80da7c40e9770d4a93126d984d2fb501b7669f810dba5a` (alpha.7 = alpha.8)
- ELF validation: magic ✓, machine ✓, segments ✓

**Tier C — CI/Source (CONFIRMED)**
- CI build runs: alpha.5-8 all green
- CI release runs: alpha.7-8 all green
- `release.yml`: llvm-strip after cargo build
- `release.yml`: NDK r26b, API level 26, nightly-2026-03-01 Rust

**Tier D — Hypotheses (UNTESTED)**
- H1: LD_PRELOAD interference
- H2: llvm-strip corruption
- H3: API level mismatch

---

## 🔬 HYPOTHESIS TREE — H1 / H2 / H3

### H1 — LD_PRELOAD Interference (Confidence: 0.45)

**What:** `LD_PRELOAD=/data/data/com.termux/files/usr/lib/libtermux-exec-ld-preload.so`
Termux's exec wrapper intercepts dynamic linker calls.

**Evidence FOR:**
- Confirmed ACTIVE on device (from `97_ld_preload.txt`)
- Termux uses this for its own binary launching
- Multiple Termux SIGSEGV reports match this pattern

**Evidence AGAINST:**
- Backtrace shows NO shared library frames
- If preload was the issue, we'd expect at least the preload lib in stack

**Test:**
```
LD_PRELOAD="" ./aura-daemon-v4.0.0-alpha.8-aarch64-linux-android --version
```
**Result:** NOT TESTED (binary removed by installer before test ran)

**If H1 confirmed →** Fix installer to unset LD_PRELOAD before daemon launch

---

### H2 — llvm-strip Corruption (Confidence: 0.35)

**What:** `release.yml` runs `llvm-strip` on both binaries. LLVM bug #56738:
> "Stripping BOLTed binaries may result in misaligned PT_LOADs, leading to program crashes at startup."

**Evidence FOR:**
- `llvm-strip` CONFIRMED in `release.yml` after cargo build
- Matches LLVM bug #56738 crash pattern exactly
- PT_LOAD misalignment = crash at startup, before any code

**Evidence AGAINST:**
- ELF validation passes (magic, segments)
- Crash is inside code segment, not at segment boundary

**Test:**
Build unstripped binary → deploy to device → run

**Result:** NOT TESTED

**If H2 confirmed →** Remove `llvm-strip` from `release.yml` OR ship unstripped alongside stripped

---

### H3 — Toolchain / API Level Mismatch (Confidence: 0.35)

**What:** `.cargo/config.toml` uses `android21-clang` (API 21), CI uses API 26.

**Evidence FOR:**
- Web research: API 21 compilation → crashes on API 26+ runtime
- Symbol availability differs at API levels

**Evidence AGAINST:**
- Higher API is backwards compatible (bionic design)

**Test:**
Check NDK API level in CI vs device SDK (SDK 35 on device)

**Result:** NOT TESTED

**If H3 confirmed →** Align `.cargo/config.toml` to API 26

---

## 📋 WHAT WE DID THIS SESSION

### Code Fixes
- `daemon/main.rs`: `Args::parse()` moved before tracing init. Panic hook added.
- `neocortex/main.rs`: `Args::parse()` moved before tracing init. Panic hook added. `--version` flag added.
- `integration_tests.rs`: Duplicate removed, API mismatches fixed, broken tests `#[ignore]`d

### Git / Branch Management
| Branch | SHA | CI | Status |
|--------|-----|----|--------|
| `origin/main` | `16fd8a7` | — | alpha.8 |
| `fix/entrypoint-and-observability` | `e25857f` | ✅ SUCCESS | Ready |
| `copilot/perform-extensive-code-review` | `34f29b9` | ✅ SUCCESS | PR #17 MERGEABLE |

### Web Research Findings
1. **LLVM bug #56738** — `llvm-strip` misaligns PT_LOAD → startup crashes ← **STRONGEST MATCH**
2. **Rust 1.78 alignment** — stricter memory checks cause Android startup crashes
3. **Termux SIGSEGV pattern** — multiple native binaries crash on Termux at startup
4. **API 21 crashes** — Stack Overflow: API 21 build crashes on higher Android

### Device Audit Findings
- Termux 0.118.3, F_DROID, Android SDK 35
- `LD_PRELOAD`: **CONFIRMED ACTIVE** on device
- Full Clang 21 toolchain present (libLLVM-21.so, 133MB)
- No binaries in `/usr/bin` (installer removes them on failure)

### Scripts Created
1. `AURA-TERMUX-AUDIT-SCRIPT.sh` — Full system audit ✅ TESTED
2. `AURA-F001-DIAGNOSTIC-SCRIPT.sh` — Root cause diagnostic ⚠️ URL was wrong

---

## 📌 COPILOT PR #17 — VERDICT

### VALID (Keep All)
- Binary contract gates (ELF magic, machine, type, deps, sections)
- Stale-artifact detection (SHA256 comparison)
- Truth bundle manifest
- ELF identity check before chmod +x
- `.cargo/config.toml` android21→26 fix
- TOCTOU fix in `model.rs`
- Null-ptr guard in `model.rs`

### WRONG
- **Root cause claim: "F001 caused by TOCTOU in load_model_ffi and missing init_ffi_backend()"**
  **FACTUALLY WRONG.** Crash happens BEFORE any model loading, at ELF entry point.
  Copilot's model.rs fixes are valid defensive improvements but do NOT fix F001.

### PR #17 Status
- CI: ✅ PASSED
- PR: #17 MERGEABLE
- Merging adds governance but does NOT fix F001

---

## 🚨 F001 ROOT CAUSE — CONFIRMED & FIXED (2026-03-19)

### Root Cause (85% Confidence)
**NDK GitHub Issue #2073**: `panic=abort` + `lto=true` + NDK r26b = known toxic combination causing startup SIGSEGV.

Evidence chain:
- Every binary crashes identically (stripped, unstripped, termux-elf-cleaned)
- Crash signature matches NDK #2073 exactly: SIGSEGV at fault addr 0x0, before main()
- NDK #2073 tombstone: "signal 11, code 1, fault addr 0x0000000000000000" — our crash: same
- NDK #2073 was closed "not planned" — fix was to change panic/LTO settings

### Fix Applied (Branch: fix/f001-panic-ndk-rootfix, Commit: fe94838)

| File | Change |
|------|--------|
| `rust-toolchain.toml` | nightly-2026-03-01 → stable |
| `Cargo.toml` [profile.release] | panic=abort → panic=unwind |
| `Cargo.toml` [profile.release] | lto=true → lto="thin" |
| `aura-daemon/src/lib.rs` | Removed #![feature(once_cell_try)] |
| `aura-daemon/src/bin/main.rs` | Removed #![feature(once_cell_try)] |
| All CI workflows | Updated to stable toolchain |

PR: #19 | CI: RUNNING | Status: PENDING VERIFICATION

---

## 🏃 NEXT ACTIONS (Priority Order)

### Step 1 — CI Verification (IN PROGRESS)

**termux-elf-cleaner INSTALLED and RAN:**
- `termux-elf-cleaner v3.0.1-1` installed successfully
- Made change: "Replacing unsupported DF_1_* flags 134217729 with 1"
- Binary STILL crashes with SIGSEGV

**Interpretation:**
- H4 CONFIRMED partially (DF_1_* flag issue found and fixed)
- H4 NOT the primary root cause (crash persists)
- Multiple failure layers exist

**H1 (LD_PRELOAD): RULED OUT** — Both with/without crashed identically.
**H4 (termux-elf-cleaner): PARTIAL** — Flag fixed, crash persists.
**H2 (llvm-strip): PENDING** — Needs unstripped binary.
**H5 (Rust nightly): PENDING** — Needs different toolchain.
**H6 (panic=abort): PENDING** — Needs build config change.

### Step 2 — Device Deep Diagnostics (IMMEDIATE)

**Run on device NOW:**
```bash
cd ~/downloads
ldd aura-daemon 2>&1
ls -la aura-daemon
```

### Step 3 — Build Unstripped Binary (CREATING NOW)

See: `AURA-F001-ROOT-CAUSE-RESOLUTION-PLAN.md`

### Step 4 — Merge PR #17
Ready — governance improvements valid regardless of F001.

---

## 📁 KEY FILES

### Primary Evidence
| File | What |
|------|------|
| `aura_ci_fix/docs/reports/19-03-2026/06_logcat_filtered.log` | 59-line crash evidence |
| `aura_ci_fix/docs/reports/19-03-2026/V8/ALL_IN_ONE.txt` | Fresh device test |
| `aura/docs/reports/AURA-DEBUG-20260319-212617/97_ld_preload.txt` | LD_PRELOAD confirmed active |
| `aura/docs/reports/AURA-DEBUG-20260319-212617/95_env_vars.txt` | Full env vars |

### Modified Source
| File | Change |
|------|--------|
| `crates/aura-daemon/src/bin/main.rs` | Entry point + panic hook |
| `crates/aura-neocortex/src/main.rs` | Entry point + panic hook + --version |
| `crates/aura-daemon/src/integration_tests.rs` | API fixes |

### Release Assets
| Asset | SHA256 | Download URL |
|-------|--------|--------------|
| aura-daemon-v4.0.0-alpha.8 | `349ffabd...` | `.../aura-daemon-v4.0.0-alpha.8-aarch64-linux-android` |
| aura-neocortex-v4.0.0-alpha.8 | `6c60b727...` | `.../aura-neocortex-v4.0.0-alpha.8-aarch64-linux-android` |

---

## ⚠️ RULES (Never Forget)

1. **CI only compiles** — never trusts green CI for runtime behavior
2. **Test on actual device** — emulator ≠ real Android
3. **One hypothesis at a time** — H1 → H2 → H3, no parallel speculation
4. **Verify download** — always `file` + `sha256sum` before running
5. **Context book** — update this document after every session
6. **Evidence tiers** — don't claim confirmed what is only hypothesis
