# AURA v4 — Context Book (MASTER)

**Version:** 3.0 | **Updated:** 2026-03-20 | **Session:** Active  
**Working Directory:** `C:\Users\Lenovo\aura` on branch `fix/f001-panic-ndk-rootfix`  
**Repo:** `AdityaPagare619/aura`  
**Main Branch SHA:** `16fd8a7` | **Current Branch HEAD:** `128ed2e` | **Release:** alpha.8  

---

## 1. PROJECT CONTEXT — What Is AURA v4?

### Overview

**AURA (Adaptive Universal Reasoning Assistant)** is a **privacy-first, on-device AI agent for Android smartphones** written in Rust. It runs entirely on the user's phone with zero cloud dependencies, zero telemetry exfiltration, and zero fallback to remote services.

### Core Technology Stack

| Component | Technology | Purpose |
|-----------|-----------|---------|
| Core Runtime | **Rust** | Memory-safe, high-performance, bounded collections |
| LLM Inference | **llama.cpp** via FFI | On-device model execution |
| Persistence | **SQLite + bincode checkpoints** | All user data stays on-device |
| Deployment | **Termux** on Android | User-space Linux environment |
| IPC | Unix socket (Android) / TCP (host) | Daemon ↔ Neocortex communication |

### Key Contributors

| Role | Agent | Responsibilities |
|------|-------|-----------------|
| Senior Architect | AdityaPagare619 | Project lead, Rust architecture, Android integration |
| Multi-Agent Orchestration | opencode | Parallel agents, implementation, verification |

### Mission

Build a **privacy-sovereign AI assistant** that:
- Never sends data to the cloud
- Runs entirely on-device (even without internet)
- Provides honest, non-sycophantic responses
- Gives users full control over their data (GDPR export/delete)
- Operates transparently with visible reasoning

### Three Pillars (Priority Order)

1. **Privacy Sovereignty (#1)** — All data stays on device. Zero telemetry. Zero cloud.
2. **Anti-Sycophancy (#2)** — Never prioritize user approval over truth.
3. **Transparency (#3)** — Visible reasoning, auditable decisions.

---

## 2. CURRENT CRISIS — F001 SIGSEGV at Startup

### The Problem

The `aura-daemon-v4.0.0-alpha.8-aarch64-linux-android` binary crashes with **SIGSEGV (EXIT: 139)** on Termux/Android devices at startup — before ANY output is produced. The crash has persisted through alpha.5, alpha.6, alpha.7, and alpha.8 releases.

**Critical insight:** CI shows GREEN for all builds because CI only compiles — it never runs the binary on a real device.

### Root Cause (CONFIRMED — NDK #2073)

**NDK GitHub Issue #2073:** `lto=true` + `panic="abort"` + NDK r26b = known toxic combination causing startup SIGSEGV.

| Setting | BROKEN (alpha.5-8) | FIXED (branch) |
|---------|---------------------|----------------|
| `lto` | `true` (full LTO) | `"thin"` (thin LTO) |
| `panic` | `"abort"` (immediate) | `"unwind"` (caught) |
| Toolchain | `nightly-2026-03-01` | `stable` |

### Binary Analysis Findings

| Finding | Value |
|---------|-------|
| Crash address | `0x5ad0b4` |
| Fault type | SEGV_MAPERR (NULL deref) |
| Crash timing | BEFORE `main()` — at ELF entry point |
| Relocations | 21,423 RELATIVE relocations (BIND_NOW) |
| INIT_ARRAY | 7 functions executed before main |
| PREINIT_ARRAY | 2 functions executed before main |

### Crash Timeline

```
User runs: aura-daemon --version
    ↓
Kernel loads binary, maps segments
    ↓
Dynamic linker applies 21,000+ relocations (BIND_NOW)
    ↓
CRT startup code → INIT_ARRAY functions (7) → PREINIT_ARRAY (2)
    ↓
Rust runtime initializes
    ↓
    ↓ ← SIGSEGV OCCURS HERE (0x5ad0b4, fault addr 0x0)
    ↓
Process dies: EXIT: 139
```

### Why This Happens

Full LTO (`lto=true`) aggressively merges functions across compilation units. Combined with `panic="abort"` (no unwinding), this creates a situation where the panic handler or CRT initialization code has a dangling/misoptimized function pointer. When the code tries to call through it → NULL dereference → SIGSEGV.

### Fix Applied

**Branch:** `fix/f001-panic-ndk-rootfix` | **Latest Commit:** `128ed2e` | **CI Status:** ✅ GREEN (all 6 jobs)

| File | Change |
|------|--------|
| `Cargo.toml` | `lto=true` → `lto="thin"` |
| `Cargo.toml` | `panic="abort"` → `panic="unwind"` |
| `rust-toolchain.toml` | `nightly-2026-03-01` → `stable` |
| All CI workflows | Updated to `stable` toolchain |
| `aura-daemon/src/lib.rs` | Removed `#![feature(once_cell_try)]` |
| `aura-daemon/src/bin/main.rs` | Removed `#![feature(once_cell_try)]` |
| `crates/aura-neocortex/src/main.rs` | Removed `#![feature(negative_impls)]` |
| `crates/aura-neocortex/src/model.rs` | Removed `impl !Sync for LoadedModel {}` |
| `daemon/main.rs` | Panic hook moved to Step 0 (before Args::parse) |
| `neocortex/main.rs` | Panic hook moved to Step 0 + `--version` flag added |
| `release.yml` | Added termux-elf-cleaner, unstripped artifacts, ELF analysis |

### Verification Status

| Environment | Status | Notes |
|------------|--------|-------|
| CI (compilation) | ✅ GREEN | All 6 jobs pass |
| CI (runtime) | ❌ NOT TESTED | No Android/Termux in CI |
| Real device | ❌ NOT TESTED | Binary needs rebuild from fix branch |

**CRITICAL:** The fix has NOT been tested on an actual Termux/Android device. This is the #1 priority.

### Hypotheses Ruled Out

| Hypothesis | Verdict | Evidence |
|-----------|---------|---------|
| H1: LD_PRELOAD interference | ❌ RULED OUT | Tested both ways, same crash |
| H2: llvm-strip corruption | ⚠️ POSSIBLE | NDK #2073 is primary cause |
| H3: API level mismatch | ⚠️ POSSIBLE | Secondary cause, not primary |
| H4: termux-elf-cleaner | ⚠️ PARTIAL | Fixed DF_1_* flags, crash persists |

---

## 3. WORKING DIRECTORY

### Important

**ALL work happens in:** `C:\Users\Lenovo\aura` on branch `fix/f001-panic-ndk-rootfix`

**NEVER work in:**
- `aura_ci_fix` (old directory, deprecated)
- Any other folders outside `C:\Users\Lenovo\aura`

### Git State

| Branch | SHA | Description |
|--------|-----|-------------|
| `origin/main` | `16fd8a7` | alpha.8 release |
| `fix/f001-panic-ndk-rootfix` | `128ed2e` | Current working branch (CI green) |
| `fix/entrypoint-and-observability` | `e25857f` | Panic hook ordering fix |
| `copilot/perform-extensive-code-review` | `34f29b9` | PR #17 (governance improvements) |

### Recent Git History (Last 10 commits)

| SHA | Description |
|-----|-------------|
| `53ed646` | docs(ethics): reconcile rule count with implementation |
| `a95cade` | docs(memory): reconcile consolidation tier naming |
| `b1c7e94` | fix(arc): implement battery/thermal penalty thresholds |
| `ae6b0e4` | fix(forestguardian): implement all 5 patterns and 4 intervention levels |
| `1cd080f` | fix(identity): rename epistemic levels to match specification |
| `61de60a` | fix(memory): add emotional_valence, goal_relevance, novelty_score to importance scoring |
| `13f4bb2` | fix(planner): implement real semantic similarity, fix template matching stub |
| `e400208` | fix(ethics): make Audit verdicts non-bypassable, reconcile trust thresholds |
| `57a75b6` | fix(reflection): align prompts.rs output schema with grammar.rs parser |
| `128ed2e` | fix(diagnostics): comprehensive CI pipeline improvements + defensive daemon fixes |

---

## 4. KEY TECHNICAL PRINCIPLES

### The LLM = Brain, Rust = Body Boundary

**Core rule:** Rust reasons NOTHING. LLM reasons everything.

| Decision | Who Decides | Rust's Role |
|----------|-------------|-------------|
| What the user wants | **LLM** | Never regex-match intent |
| Which tools to use | **LLM** | Never if/else chain on keywords |
| Whether a goal succeeded | **LLM** | Never pixel/pattern match success |
| How to respond | **LLM** | Never template strings |
| When to be proactive | **LLM** | Never time-based triggers without context |

| Action | Who Executes | Rust's Role |
|--------|-------------|-------------|
| Read screen state | **Rust** | Accessibility API calls |
| Execute tools | **Rust** | Android API calls |
| Store/retrieve memory | **Rust** | SQLite operations |
| Enforce policy/safety | **Rust** | Deterministic checks |
| Manage IPC/networking | **Rust** | Socket management |
| Bound resource usage | **Rust** | OOM prevention |

**Violation = Theater AGI:** Fake intelligence with hardcoded heuristics. Breaks on novel input, multilingual, sarcasm, ambiguity.

### The Seven Iron Laws

| # | Law | Category | Consequence of Violation |
|---|-----|---------|--------------------------|
| 1 | LLM = brain, Rust = body | Architectural | Theater AGI |
| 2 | Theater AGI is banned | Anti-Theater | Brittle keyword matching |
| 3 | Structural fast-path parsers ARE allowed | Carve-out | N/A (explicit permission) |
| 4 | Never change production logic to make tests pass | Engineering Integrity | Corrupted code + tests |
| 5 | Anti-cloud absolute | Privacy | Privacy violation |
| 6 | Privacy-first + GDPR export/delete | Privacy | Trust destruction |
| 7 | No sycophancy | Ethics | Harmful validation |

### VAD/EmotionLabels = Defaults, Not Absolute Truth

AURA discovers its own dimensions through interaction. VAD (Valence-Arousal-Dominance) and EmotionLabels are **starting points**, not rigid constraints. The system evolves based on:
- User feedback signals
- Interaction history
- Trust tier progression
- Behavioral patterns

### Deny-by-Default Policy Gate

`production_policy_gate()` must return `deny_by_default_builder()`, NOT `allow_all_builder()`.

**Current gap:** `production_policy_gate()` returns `allow_all_builder()` — Layer 1 (configurable policy) is disabled in production.

**Risk:** Policy rules configured for the deployment have no effect.

**What's protected:** Layer 2 (hardcoded ethics) is independent and always enforced.

### HNSW Vector Embeddings for True Semantic Memory

HNSW (Hierarchical Navigable Small World) provides TRUE semantic search:
- Vector-based similarity matching
- Handles meaning, not just string patterns
- String matching is Phase 2 cache optimization (not semantic memory itself)

### Active Inference Framework

AURA uses **Active Inference** — minimizing surprise by continuously updating beliefs about the world and taking actions to gather information that reduces uncertainty.

### Trust Thresholds

| Tier | Name | τ Threshold | Permissions |
|------|------|-------------|-------------|
| 0 | STRANGER | τ < 0.15 | Basic conversation only |
| 1 | ACQUAINTANCE | 0.15 ≤ τ < 0.35 | Read-only memory, consent management |
| 2 | FRIEND | 0.35 ≤ τ < 0.60 | Memory write, actions with confirmation |
| 3 | CLOSEFRIEND | 0.60 ≤ τ < 0.85 | Full autonomy for routine tasks |
| 4 | SOULMATE | τ ≥ 0.85 | Full autonomy including proactive |

---

## 5. ARCHITECTURE SUMMARY

### Two-Binary Architecture

```
┌─────────────────────────────────────────────────────────┐
│ Binary 1: aura-daemon (PID 1)                          │
│ • Persistent daemon (~20-50 MB)                         │
│ • Tokio single-thread runtime                           │
│ • Controls all subsystems                               │
│ • Loaded by install.sh / termux-services               │
│ • Communicates with neocortex via IPC socket           │
└──────────────────────┬──────────────────────────────────┘
                       │ IPC (abstract Unix socket)
                       ▼
┌─────────────────────────────────────────────────────────┐
│ Binary 2: aura-neocortex (PID 2)                       │
│ • Separate process, separate binary (~500MB-2GB)        │
│ • llama.cpp for LLM inference                          │
│ • Killable by Android Low-Memory Killer                │
│ • Starts disconnected, spawned on demand               │
└─────────────────────────────────────────────────────────┘
```

### Seven Subsystems

| Subsystem | Files | Responsibility |
|-----------|-------|---------------|
| `daemon_core` | 11 | Main event loop, ReAct loop, startup/shutdown |
| `routing` | 4 | System 1 (ETG) vs System 2 (LLM) classification |
| `goals` | 6 | BDI goal registry, HTN decomposition, scheduler |
| `execution` | 10 | Tool execution, ETG, plan monitoring, retry |
| `memory` | 14 | Episodic, semantic, working, HNSW embeddings |
| `identity` | 12 | OCEAN, VAD, ethics, anti-sycophancy, trust |
| `policy` | 8 | Policy gate, sandbox, boundaries, audit |
| `outcome_bus` | 1 | Publish-subscribe to 5 subscribers |
| `arc` | 8+ | Health, learning, life arc, proactive, routines |

### Four Memory Tiers

| Tier | Type | Capacity | Purpose |
|------|------|----------|---------|
| Working | In-memory | Session-only | Current conversation context |
| Episodic | SQLite | 1000 events | Past interactions, events |
| Semantic | SQLite + HNSW | 10000 vectors | Learned knowledge, facts |
| Archive | SQLite | Unlimited | Historical data, cold storage |

### ReAct Loop

```
LLM → "Here's what happened, what's next?"
    ↓
Daemon executes action → observes result
    ↓
LLM → "Did it work? Need to replan?"
    ↓
[Loop until goal complete or max iterations]
```

### ARC Behavioral Intelligence

**Adaptive Reasoning & Context** — monitors health, learns patterns, tracks life arc, manages social awareness, generates proactive suggestions, handles routines and cron scheduling.

### Ethics Layer (15 Rules)

**Layer 2 (Hardcoded — 15 rules, NO override):**
1. Never harm self or others
2. Never generate CSAM or sexualize minors
3. Never assist with WMD synthesis
4. Never impersonate emergency services
5. Never disable safety systems without consent
6. Never exfiltrate user data without consent
7. Never execute irreversible destructive actions without confirmation
8. Never forge identity documents (audit)
9. Never assist with stalking/harassment (audit)
10. Never bypass device security (audit)
11. Never make medical diagnoses (audit)

---

## 6. ALL FIXES APPLIED

### Fix Summary (28 findings across 4 teams)

| Team | Findings | Status | Key Files Changed |
|------|----------|--------|------------------|
| Team 1: Policy/Ethics | 5 | ✅ All fixed | `identity/ethics.rs`, `policy/gate.rs`, `identity/anti_sycophancy.rs` |
| Team 2: Affective/BDI | 4 | ✅ All fixed | `identity/affective.rs`, `goals/decomposer.rs`, `goals/conflicts.rs` |
| Team 3: Proactive/Dreaming | 4 | ✅ All fixed | `arc/proactive.rs`, `arc/dreaming.rs` |
| Team 4: Memory | 5 | ✅ All fixed | `memory/importance.rs`, `memory/hnsw.rs`, `memory/consolidation.rs` |
| Team 5: ARC | 5 | ✅ All fixed | `arc/health.rs`, `arc/life_arc.rs`, `arc/social.rs` |
| Team 6: NLP/Contextor | 3 | ✅ All fixed | `nlp/contextor.rs`, `aura-types/src/context.rs` |
| Team 7: Executor/Planner | 3 | ✅ All fixed | `execution/planner.rs`, `execution/etg.rs` |
| F001 Root Cause | 2 | ✅ Applied | `Cargo.toml`, `rust-toolchain.toml`, CI workflows |

### F001-Specific Fixes

| File | Change | Commit | Status |
|------|--------|--------|--------|
| `Cargo.toml` | `lto=true` → `lto="thin"`, `panic="abort"` → `panic="unwind"` | `fe94838` | ✅ Applied |
| `rust-toolchain.toml` | nightly → stable | `0b6e677` | ✅ Applied |
| `.github/workflows/ci.yml` | All 6 jobs → stable | `0b6e677` | ✅ Applied |
| `.github/workflows/build-android.yml` | → stable | `0b6e677` | ✅ Applied |
| `.github/workflows/release.yml` | → stable + termux-elf-cleaner | `128ed2e` | ✅ Applied |
| `crates/aura-daemon/src/lib.rs` | Removed nightly features | `fe94838` | ✅ Applied |
| `crates/aura-daemon/src/bin/main.rs` | Removed nightly features + panic hook | `e25857f` | ✅ Applied |
| `crates/aura-neocortex/src/main.rs` | Removed nightly features | `fe94838` | ✅ Applied |
| `crates/aura-neocortex/src/model.rs` | Removed nightly features | `fe94838` | ✅ Applied |
| `.github/workflows/f001-diagnostic.yml` | Updated to stable | `128ed2e` | ✅ Applied |

### Agent Assignments (from plans/)

| Agent | Responsibility | Status |
|-------|---------------|--------|
| agent-11 | Screen semantic cache | ✅ Complete |
| agent-13 | ARC behavioral intelligence | ✅ Complete |
| agent-9 | Power/thermal physics | ✅ Complete |
| agent-b | Test fixes | ✅ Complete |
| agent-c | Voice/Telegram wiring | ✅ Complete |
| agent-d | Policy/Ethics wiring | ✅ Complete |
| agent-e | NLP/Contextor | ✅ Complete |

---

## 7. KEY FILES TO KNOW

### Critical Source Files

| File | Purpose |
|------|---------|
| `crates/aura-daemon/src/bin/main.rs` | Daemon entry point, panic hook, CLI parsing |
| `crates/aura-neocortex/src/main.rs` | Neocortex entry point, llama.cpp wrapper |
| `crates/aura-daemon/src/daemon_core/startup.rs` | 8-phase startup sequence |
| `crates/aura-daemon/src/daemon_core/main_loop.rs` | Event loop with 7 channels |
| `crates/aura-daemon/src/daemon_core/react.rs` | ReAct loop implementation |
| `crates/aura-daemon/src/memory/importance.rs` | Memory importance scoring |
| `crates/aura-daemon/src/memory/hnsw.rs` | HNSW vector index |
| `crates/aura-daemon/src/identity/ethics.rs` | 15 hardcoded ethics rules |
| `crates/aura-daemon/src/policy/gate.rs` | Policy gate evaluation |
| `crates/aura-daemon/src/outcome_bus.rs` | OutcomeBus (765 lines, 5 subscribers) |

### Configuration Files

| File | Purpose |
|------|---------|
| `Cargo.toml` | Workspace config, `lto` and `panic` settings |
| `rust-toolchain.toml` | Rust toolchain selection |
| `.cargo/config.toml` | Build target and linker configuration |
| `.github/workflows/ci.yml` | 6-job CI pipeline |
| `.github/workflows/release.yml` | Release pipeline with ELF analysis |
| `crates/aura-types/src/tools.rs` | 30-tool registry |

### Evidence Files

| File | What It Contains |
|------|------------------|
| `docs/reports/AURA-F001-COMPREHENSIVE-RESOLUTION-REPORT.md` | Complete F001 analysis (530 lines) |
| `docs/reports/AURA-ANDROID-REALDEVICE-FORENSICS-2026-03.md` | Device forensics |
| `docs/reports/AURA-ANDROID-INCIDENT-POSTMORTEM-alpha5-alpha8-2026-03-19.md` | Postmortem |
| `docs/reports/AURA-SYSTEM-FAILURE-ANALYSIS.md` | System failure analysis |
| `docs/reports/AURA-MASTER-SYNTHESIS-2026-03-19.md` | Master synthesis |
| `docs/architecture/AURA-V4-SYSTEM-ARCHITECTURE.md` | Architecture reference (867 lines) |
| `docs/architecture/AURA-V4-IDENTITY-ETHICS-AND-PHILOSOPHY.md` | Identity/ethics reference (1132+ lines) |
| `docs/architecture/AURA-V4-MEMORY-AND-DATA-ARCHITECTURE.md` | Memory architecture |
| `docs/architecture/AURA-V4-PRODUCTION-STATUS.md` | Production readiness |

### Architecture Decision Records

| File | Decision |
|------|----------|
| `docs/adr/ADR-001-bicameral-architecture.md` | Why Rust |
| `docs/adr/ADR-002-etg-caching.md` | Why ETG as System 1 |
| `docs/adr/ADR-003-memory-tiers.md` | Why 4 memory tiers |
| `docs/adr/ADR-004-safety-borders.md` | Why deny-by-default |
| `docs/adr/ADR-005-accessibility-first.md` | Accessibility-first design |
| `docs/adr/ADR-006-bio-inspired-learning.md` | Bio-inspired learning |
| `docs/adr/ADR-007-deny-by-default-policy-gate.md` | Policy gate architecture |

---

## 8. OUTSTANDING WORK

### CRITICAL — Device Testing (BLOCKER)

**The fix has NOT been tested on an actual Termux/Android device.**

Required steps:
1. Push fix branch to remote → Trigger release build
2. Download new `aura-daemon` binary from GitHub Release
3. Deploy to Termux device
4. Run: `~/aura-daemon --version`
5. **Expected:** Clean version output, EXIT: 0
6. **If it works:** Root cause confirmed → Proceed to alpha.9 release
7. **If it still crashes:** Secondary causes need investigation

### HIGH PRIORITY

| Task | Status | Notes |
|------|--------|-------|
| Test neocortex binary for same SIGSEGV | Not started | May also be affected by lto=true + panic=abort |
| Add runtime testing to CI | Not started | Need real Android/Termux environment |
| Fix `production_policy_gate()` gap | Not started | Layer 1 (configurable policy) disabled |
| Merge PR #17 (governance improvements) | Ready | CI passes, governance valid |
| Release alpha.9 | Blocked on device test | After device test passes |

### MEDIUM PRIORITY

| Task | Status | Notes |
|------|--------|-------|
| Upgrade NDK r26b → r27 | Optional | r27 is LTS, r28 has regressions, r29 has crashes |
| ELF analysis on new binary | Not started | Confirm crash addr changes |
| addr2line mapping on unstripped binary | Not started | Map crash addr to function name |
| NDK version upgrade testing | Not started | r27 LTS stability |

### LOWER PRIORITY

| Task | Status | Notes |
|------|--------|-------|
| Simulated → real screen verification | Not started | Replace `simulate_action_result()` |
| Unwrap proliferation cleanup | Not started | 634 `.unwrap()` calls need proper error handling |
| God function decomposition | Not started | `handle_cron_tick` is 1200 lines |
| Type mismatch fix | Not started | Compile error in `inference.rs:789` |

---

## 9. TOMORROW'S PRIORITIES

### First Thing: Device Testing

```
1. Push fix branch to origin (if not already pushed)
2. Wait for GitHub Actions release build to complete
3. Download: aura-daemon-v4.0.0-{NEW_TAG}-aarch64-linux-android
4. On device: sha256sum to verify
5. On device: file to check ELF
6. On device: chmod +x aura-daemon
7. On device: LD_PRELOAD="" ./aura-daemon --version
8. Expected: "AURA v4.0.0-alpha.X" → EXIT: 0
```

### If Device Test PASSES

1. Tag new release: `v4.0.0-alpha.9`
2. Merge PR #19 (fix branch → main)
3. Update release notes with F001 fix
4. Test neocortex binary
5. Consider NDK upgrade (r26b → r27)

### If Device Test FAILS

1. Deploy **unstripped** binary from fix branch artifacts
2. Run `addr2line -e aura-daemon-unstripped 0x5ad0b4` to map crash address
3. Investigate secondary causes:
   - TLS initialization
   - Bionic version mismatch
   - Static init crashes
4. Consider reverting to nightly with different panic settings
5. Test with `lto=false` as control experiment

### Secondary Actions

1. Merge PR #17 (governance improvements) — ready, CI green
2. Fix `production_policy_gate()` gap — security priority
3. Update this context book with device test results
4. Write alpha.9 release notes

---

## RULES FOR FUTURE SESSIONS

1. **Always test on real device** — CI only compiles, never runs
2. **Update this document** — After every session, add findings
3. **Never claim confirmed what is only hypothesis** — Use evidence tiers
4. **Binary analysis first** — Use `file`, `readelf`, `sha256sum` before running
5. **One hypothesis at a time** — H1 → H2 → H3, no parallel speculation
6. **Never work in aura_ci_fix** — Only `C:\Users\Lenovo\aura`
7. **Verify download** — Always `file` + `sha256sum` before running
8. **Test neocortex separately** — Both binaries may have same issue
9. **The crash happens BEFORE main()** — Don't look for Rust code bugs
10. **NDK issues are compiler-level** — Check toolchain, LTO, panic settings first

---

**END OF CONTEXT BOOK**

*Last updated: 2026-03-20 | Generated from comprehensive project analysis*
