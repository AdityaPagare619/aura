## §9: CI/CD & DevOps Review

**Grade: C (6/10)**  
**Reviewer:** CI/CD & DevOps Specialist  
**Verdict:** ⛔ BLOCK on 4 critical findings

### 9.1 Scope

This domain evaluates:
- 3 GitHub Actions workflows (`ci.yml`, `build-android.yml`, `release.yml`)
- User-facing `install.sh` script (1004 lines)
- Submodule configuration (`.gitmodules`)
- Release engineering posture

### 9.2 Critical Findings

#### CI-CRIT-001: Toolchain Channel Split

| Attribute | Detail |
|-----------|--------|
| **Files** | `rust-toolchain.toml`, `.github/workflows/ci.yml` (lines 18, 34, 48, 62) |
| **Severity** | Critical — silent correctness divergence |

**Description:** The repository pins `nightly-2026-03-01` in `rust-toolchain.toml`, but all 3 CI workflows use `dtolnay/rust-toolchain@stable`. The codebase uses nightly features:
- `#![feature(let_chains)]`
- `#![feature(async_fn_in_trait)]`
- `#![feature(coroutines)]`

**Impact:** Developer builds on nightly, CI builds on stable. Different codegen, different lint behavior, potential "works on my machine" failures. Nightly-only features compile locally but may fail CI.

**Fix:**
```yaml
# ci.yml — remove dtolnay action, let rust-toolchain.toml govern
steps:
  - uses: actions/checkout@v4
  - run: rustup show  # reads rust-toolchain.toml automatically
  - run: cargo check --workspace
```

---

#### CI-CRIT-002: Release Pipeline Non-Functional

| Attribute | Detail |
|-----------|--------|
| **File** | `.github/workflows/release.yml` (lines 12, 51) |
| **Severity** | Critical — release pipeline is broken |

**Two compounding failures:**
1. **Line 12** — Checkout without `submodules: recursive`. The `llama-cpp/` directory is empty.
2. **Line 51** — `cargo check --workspace` without `--features stub`. Build script panics when llama.cpp source absent.

**Impact:** Every git tag push triggers a release workflow that fails. No automated release artifacts have ever been produced.

**Fix:**
```yaml
steps:
  - uses: actions/checkout@v4
    with:
      submodules: recursive
      fetch-depth: 0
  - name: Verify submodule
    run: test -f llama-cpp/CMakeLists.txt || exit 1
  - name: Build release
    run: cargo build --release --workspace
```

---

#### CI-CRIT-003: Placeholder SHA256 Checksums (Cross-ref SEC-CRIT-003)

| Attribute | Detail |
|-----------|--------|
| **File** | `install.sh` (lines 39, 44, 49) |
| **Severity** | Critical — supply-chain integrity voided |

```bash
EXPECTED_SHA256="PLACEHOLDER_REPLACE_BEFORE_RELEASE"
```

All model download integrity checks use placeholder strings. The validation logic exists but comparisons always fail, and `|| true` swallows failures silently.

**Impact:** MITM attacker can substitute ~4GB model files with backdoored weights. User receives compromised model without warning.

---

#### CI-CRIT-004: Unsalted SHA256 PIN Hash (Cross-ref SEC-CRIT-004)

| Attribute | Detail |
|-----------|--------|
| **File** | `install.sh` (line 884) |
| **Severity** | Critical — authentication bypass |

```bash
pin_hash=$(echo -n "$user_pin" | sha256sum | cut -d' ' -f1)
```

**Problems:**
1. No salt — rainbow table attack trivial
2. No stretching — SHA256 is fast hash (~10B/sec on RTX 4090)
3. No pepper — offline verification possible

6-digit numeric PIN brute-forced in <1ms.

---

### 9.3 High Findings

| ID | Title | Location | Description |
|----|-------|----------|-------------|
| HIGH-CI-1 | Shell Injection | `install.sh` | `$user_name` passed to `sed` without escaping. Characters `/`, `&`, `\` cause injection. |
| HIGH-CI-2 | NDK No Integrity Check | `install.sh` | ~1GB NDK archive downloaded without SHA256 verification. MITM can substitute compromised toolchain. |
| HIGH-CI-3 | Action Not Pinned | `release.yml` | `softprops/action-gh-release@v2` not pinned to commit SHA. Supply-chain attack vector. |

### 9.4 Medium Findings

| ID | Title | Description |
|----|-------|-------------|
| MED-CI-1 | Cache Key Excludes Cargo.lock | Stale dependencies may be cached |
| MED-CI-2 | Neocortex Never Tested | CI runs only `cargo test -p aura-daemon` |
| MED-CI-3 | No Concurrency Group | Stale CI runs not cancelled |
| MED-CI-4 | Submodule Tracks Branch | `.gitmodules` tracks `master` branch, not pinned commit |
| MED-CI-5 | Missing CI Tooling | No shellcheck, cargo-audit, MSRV enforcement, NDK caching, SBOM generation |

### 9.5 Workflow Summary

| Workflow | Purpose | Status | Blockers |
|----------|---------|--------|----------|
| `ci.yml` | Check + Test + Clippy + Fmt | ⚠️ Works but toolchain mismatch | CI-CRIT-001 |
| `build-android.yml` | ARM64 cross-compilation | ⚠️ Works but NDK unverified | HIGH-CI-2 |
| `release.yml` | Tagged release builds | ❌ Broken | CI-CRIT-002 |
| `install.sh` | User installation | ❌ Security holes | CI-CRIT-003, CI-CRIT-004, HIGH-CI-1 |

---

## §10: Test Quality Review

**Grade: C+ (45-55% Real Coverage)**  
**Reviewer:** Test Quality Specialist  
**Verdict:** ⛔ BLOCK on 2 critical findings — test suite provides false confidence

### 10.1 Critical Findings

#### TEST-CRIT-001: 45 Hollow Integration Tests

| Attribute | Detail |
|-----------|--------|
| **File** | `crates/aura-daemon/src/integration_tests.rs` (~37KB) |
| **Severity** | Critical — false coverage |

~45 tests with tautological assertions:
- `assert!(true)`
- `assert_eq!(x, x)`
- `assert!(result.is_ok())` without examining Ok value

**Examples:**
```rust
#[test]
fn test_memory_creates_ok() {
    let mem = WorkingMemory::new(1024);
    assert!(mem.is_ok());  // Only verifies constructor doesn't panic
}
```

**Impact:** Tests verify Rust compiles, not that AURA behaves correctly. Cannot detect behavioral regressions.

---

#### TEST-CRIT-002: ReAct Engine Has ZERO Tests

| Attribute | Detail |
|-----------|--------|
| **File** | `crates/aura-daemon/src/daemon_core/react.rs` (2821 lines) |
| **Severity** | Critical — core product untested |

The ReAct engine is AURA's cognitive heart — the Think→Act→Observe loop driving all intelligent behavior. It has:
- Zero unit tests
- Zero integration tests
- Zero property tests

**Impact:** The most critical 2800 lines have no automated verification. Any change is a dice roll.

**Minimum Required Tests:**
1. Iteration limit test (stops at MAX_ITERATIONS)
2. Token budget exhaustion test
3. Policy gate rejection test
4. Successful action completion test
5. Error recovery test

---

### 10.2 High Findings

| ID | Title | Location | Description |
|----|-------|----------|-------------|
| HIGH-TEST-1 | score_plan() Returns 0.5 | `planner.rs` | Plan scoring is non-functional. All plans ranked equally. |
| HIGH-TEST-2 | PolicyGate Bypassed | Executor tests | All tests use `for_testing()` which creates allow-all gate. Production deny-by-default never tested. |

### 10.3 Module-by-Module Quality

| Module | Test Count | Quality | Verdict |
|--------|------------|---------|---------|
| Memory (working, episodic, semantic) | ~200+ | **STRONG** | ✅ Real assertions on retrieval, eviction, RRF |
| Policy gate | ~80+ | **STRONG** | ✅ Tests deny/allow, rule matching, audit |
| Identity (anti-sycophancy, personality) | ~60+ | **STRONG** | ✅ Tests window behavior, thresholds |
| Routing classifier | ~30+ | **ADEQUATE** | ⚠️ Tests classify but classifier is dead code |
| ReAct engine | 0 | **ABSENT** | ❌ Core product untested |
| Executor pipeline | ~40+ | **WEAK** | ⚠️ All bypass PolicyGate |
| Integration tests | ~45 | **HOLLOW** | ❌ Tautological assertions |
| Planner | ~15 | **WEAK** | ⚠️ score_plan always 0.5 |
| IPC protocol | ~20+ | **ADEQUATE** | ⚠️ Tests serialization, not connections |
| Ethics/boundaries | ~50+ | **ADEQUATE** | ⚠️ Tests rules, not LLM interaction |

### 10.4 Recommendations Priority

1. **P0:** Write ReAct engine tests (minimum 5 scenarios)
2. **P0:** Rewrite hollow integration tests with real assertions
3. **P1:** Add executor tests with production PolicyGate
4. **P1:** Fix score_plan and add discriminating tests
5. **P2:** Add daemon→neocortex IPC round-trip test
6. **P2:** Add CI step for `cargo test -p aura-neocortex`

---

## §11: Documentation-vs-Code Consistency Review

**Grade: D+ (55/100)**  
**Reviewer:** Documentation Consistency Specialist  
**Verdict:** ⛔ BLOCK on 2 critical inconsistencies — documentation actively misleads

### 11.1 Critical Findings

#### DOC-CRIT-001: Trust Tiers — 5-WAY Inconsistency

| Source | Tiers | Names |
|--------|-------|-------|
| Code (`relationship.rs`) | 5 | Stranger → Acquaintance → Friend → CloseFriend → Soulmate |
| Identity Ethics doc | 4 | Stranger → Acquaintance → Companion → Intimate |
| Security Model doc | 4 | Stranger → Known → Trusted → Intimate |
| Ground Truth Architecture | 4 | Stranger → Acquaintance → Friend → Intimate |
| Production Status doc | 4 | Stranger → Friend → Close → Trusted |

**Impact:** New contributor reads docs, implements against 4 tiers, breaks runtime because code has 5. Five sources tell five different stories about a core identity concept.

**Fix:** Canonicalize all docs to match code's 5-tier model.

---

#### DOC-CRIT-002: Ethics Rule Count — 3 Different Numbers

| Source | Count | Content |
|--------|-------|---------|
| Identity Ethics doc | 15 | Lists "15 absolute ethical rules" |
| Code (`ethics.rs`) | 11 | 7 blocked patterns + 4 audit keywords |
| Operational Flow doc | 10 | References "10 life domains" conflated with ethics |

**Impact:** External auditors expect 15 rules, find 11. Gap creates doubt about intentional removal or accidental loss.

---

### 11.2 High Findings

| ID | Title | Description |
|----|-------|-------------|
| HIGH-DOC-1 | Two ReAct Loops | Daemon: MAX_ITERATIONS=10, Neocortex: MAX_REACT_ITERATIONS=5. Undocumented. |
| HIGH-DOC-2 | Test Count Mismatch | README: 2376. Other docs: 2362. |
| HIGH-DOC-3 | Phantom Crate | `aura-gguf` referenced in 3 docs but doesn't exist. GGUF parsing is in `aura-llama-sys`. |
| HIGH-DOC-4 | ARC Domain Names | 4-way inconsistency across documents. |
| HIGH-DOC-5 | ARC Context Modes | 4-way inconsistency. Code vs docs have ZERO overlap in mode names. |
| HIGH-DOC-6 | Wrong Crypto Algorithm | Installation doc claims vault PIN uses bcrypt — code uses Argon2id. |

### 11.3 Medium Findings

| ID | Topic | Doc Says | Code Says |
|----|-------|----------|-----------|
| MED-DOC-1 | Argon2id parallelism | p=1 | p=4 |
| MED-DOC-2 | Archive compression | ZSTD | LZ4 |
| MED-DOC-3 | Iron Law IL-7 | Two different definitions | — |
| MED-DOC-4 | PolicyGate "Critical Gap" | 90 lines about problem | Already fixed |
| MED-DOC-5 | Data classification names | Public/Internal/Confidential/Restricted | Ephemeral/Personal/Sensitive/Critical |

### 11.4 Consistency Matrix

| Topic | Code | Doc A | Doc B | Doc C | Verdict |
|-------|------|-------|-------|-------|---------|
| Trust tiers | 5 stages | 4 stages | 4 stages | 4 stages | ❌ 5-way conflict |
| Ethics rules | 11 items | 15 items | 10 items | — | ❌ 3-way conflict |
| ReAct iterations | 10+5 | 10 only | — | — | ⚠️ Partial |
| Argon2id p= | 4 | 1 | 4 | — | ⚠️ 2-way conflict |
| Archive compression | LZ4 | ZSTD | ZSTD | — | ⚠️ Code disagrees |
| ARC domains | 10 names | 10 different | 8 different | — | ❌ No match |
| ARC modes | 8 names | 6 different | 5 different | — | ❌ No match |
| Vault crypto | Argon2id | bcrypt | Argon2id | — | ❌ Wrong algo |

### 11.5 Verified Correct (15 Claims Match Code)

1. Bi-cameral architecture ✅
2. 11-stage executor ✅
3. AES-256-GCM encryption ✅
4. HNSW M=16, ef_construction=200 ✅
5. SQLite WAL mode ✅
6. 1024 working memory slots ✅
7. Anti-sycophancy 20-window ✅
8. Deny-by-default PolicyGate ✅
9. Process isolation ✅
10. Qwen-3 default model ✅
11. 4-tier data classification (names differ) ✅
12. Context budget 2048 tokens ✅
13. Cascade retry threshold 0.5 ✅
14. BON_SAMPLES = 3 ✅
15. Initiative budget max=1.0, regen=0.001/sec ✅

---

## §12: Operational Capacity Review

**Focus:** How many tasks can AURA handle? How does workload distribute? Does AURA remember, prioritize, or forget?

### 12.1 Current Architecture: Single-Task Sequential

AURA v4 processes **ONE user request at a time** through its ReAct engine. Evidence:

1. **ReAct engine uses `&mut self`** (`react.rs`) — exclusive mutable borrow means no concurrent execution at language level
2. **Single IPC channel** between daemon and neocortex — one inference request at a time
3. **Working memory is single-writer** — `&mut self` on all write operations
4. **No task queue with priority ordering** — FIFO processing in `main_loop.rs`

### 12.2 Task Handling Behavior

| Scenario | What Happens | Evidence |
|----------|--------------|----------|
| User sends 1 message | Full ReAct cycle (1-10 iterations) | `react.rs` main loop |
| Message during processing | Queued in IPC buffer (bounded at 64) | Execution pipeline |
| 5 rapid messages | First processes, rest queue. No priority reorder. | `main_loop.rs` |
| Proactive task fires | Queued behind active task | `proactive_dispatcher.rs` |
| "Remind me X and do Y" | LLM decomposes via Think phase. Sequential, not parallel. | `react.rs` |

### 12.3 Task Memory & Prioritization

**Does AURA remember tasks?**
- ✅ YES — Working memory (1024 slots) retains recent tasks
- ✅ YES — Episodic memory (SQLite) persists across sessions
- ✅ YES — Goals system tracks multi-step objectives

**Does AURA prioritize tasks?**
- ⚠️ PARTIALLY — GoalScheduler and ConflictResolver exist
- ❌ BUT — `score_plan()` hardcoded to 0.5, so ranking is random
- ❌ BUT — No priority queue — strictly FIFO
- ARC initiative budget throttles proactive tasks but doesn't prioritize

**Does AURA forget tasks?**
- Working memory has LRU eviction (oldest dropped when full)
- Tasks in-flight NOT persisted — crash loses active task
- Completed tasks ARE persisted in episodic memory
- Goals persist across sessions via checkpoint system

### 12.4 Capacity Limits

| Resource | Limit | Impact |
|----------|-------|--------|
| Concurrent tasks | 1 (sequential) | User waits for each task |
| Pending queue | 64 messages | 65th message dropped |
| Working memory | 1024 slots | Oldest evicted |
| ReAct iterations | 10 (daemon) × 5 (neocortex) | 150 max LLM calls |
| Token budget | 2048 tokens (6.25% of 32K) | Limited context richness |
| Goals tracked | No explicit limit | Potential unbounded growth |

### 12.5 Failure Mode: Queue Saturation

When 64-message IPC buffer fills, message 65 is **silently dropped**. No backpressure signal to UI. User receives no indication request was lost.

**Recommended fix (v4.x):** Return bounded-channel error so UI can display "AURA is busy, please wait."

### 12.6 Assessment

AURA is a **single-task sequential processor with memory persistence**. Appropriate for v4 — personal assistant on single mobile device for single user. Architecture does NOT support:
- Parallel task execution
- Priority-based scheduling
- Preemptive task switching
- Background task continuation

These are v5+ features. Main gap: `score_plan()` hardcoded prevents even sequential plan quality optimization.

---

## §13: Plugin/Extension Architecture Review

**Focus:** How smoothly can external developers connect plugins? How does AURA discover and load extensions?

### 13.1 Extension Architecture

Location: `crates/aura-daemon/src/extensions/`

| File | Purpose | Lines |
|------|---------|-------|
| `mod.rs` | Extension trait definitions, registry | ~150 |
| `discovery.rs` | Filesystem scanning for manifests | ~120 |
| `loader.rs` | Dynamic loading of extension code | ~100 |
| `recipe.rs` | Extension recipe format | ~80 |

**Total:** ~450 lines (0.3% of codebase) — proof of concept, not shipping platform.

### 13.2 Discovery Mechanism

```
~/.aura/extensions/
├── weather/
│   ├── manifest.toml      ← discovered by discovery.rs
│   └── recipe.toml        ← declarative action definition
├── spotify/
│   ├── manifest.toml
│   └── recipe.toml
└── ...
```

Discovery is **startup-only** — no hot-loading without daemon restart.

### 13.3 Two-Path Problem

| Aspect | Recipe Path | Trait Path |
|--------|-------------|------------|
| Language | TOML (declarative) | Rust (compiled) |
| Compilation | None | Must compile against aura crates |
| Capability | Invoke existing tools only | Full daemon API access |
| Sandboxing | Inherently limited | None (in-process) |
| Developer skill | Config editing | Rust systems programming |

**Gap:** No middle path. No scripting layer (Lua, WASM, Rhai) for developers wanting more than recipes but less than full Rust.

### 13.4 Developer Experience Assessment

| Aspect | Status | Grade |
|--------|--------|-------|
| Extension discovery | ✅ Filesystem scanning works | B |
| Manifest format | ✅ TOML-based, readable | B |
| Recipe format | ✅ Declarative action definitions | B |
| Dynamic loading | ⚠️ Untested on Android | C |
| Permission model | ⚠️ PolicyGate integration unclear | C |
| Lifecycle hooks | ❌ No on_install/update/uninstall | D |
| Documentation | ❌ No extension developer guide | F |
| Example extensions | ❌ No template | F |
| Version compatibility | ❌ No API versioning | D |
| Sandboxing | ❌ In-process, no isolation | D |
| Error reporting | ❌ No extension error channel | D |
| Publishing | ❌ No marketplace | F |
| Testing harness | ❌ No isolated test environment | F |

### 13.5 Security Implications

No sandboxing + untraced PolicyGate creates threat model:

```
Malicious Extension Installed
         │
         ├──→ Runs in daemon process (full memory access)
         ├──→ Can invoke any tool
         ├──→ Can read/write SQLite databases
         ├──→ Can read working memory
         └──→ PolicyGate enforcement: UNVERIFIED
```

### 13.6 Assessment

Extension system is **structurally present but not developer-ready**. Architecture (discovery, manifest, recipes) is sound foundation. Without docs, examples, sandboxing, or versioning, no external developer can build extensions today.

**Classification:** Infrastructure scaffold. Not a platform. Ship v4 with extensions marked "internal only."

---

## §14: Scalability & Resource Management Review

**Focus:** Is AURA architecturally open to external capabilities? How future-proof is the design?

### 14.1 Architectural Openness

| Dimension | Assessment | Evidence |
|-----------|------------|----------|
| Model swappability | ✅ GOOD | Any GGUF model, cascade selection, configurable paths |
| Memory scalability | ✅ GOOD | SQLite WAL, HNSW index, archive compression. Millions of entries. |
| Tool extensibility | ✅ GOOD | 11 tool categories, trait-based dispatch |
| IPC extensibility | ⚠️ MODERATE | Typed messages via aura-types. 64KB limit constrains future. |
| Extension system | ⚠️ MODERATE | Filesystem discovery exists. No marketplace, no hot-reload. |
| Multi-model | ⚠️ MODERATE | Cascade 3 tiers (1.5B/4B/8B). One model at a time. |
| Voice/multimodal | ⚠️ STUBS | Modules exist as simulation stubs only |
| Multi-user | ❌ NONE | Single-user architecture. Appropriate for personal device. |
| Cloud connectivity | ❌ BY DESIGN | Anti-cloud Iron Law. Feature, not limitation. |

### 14.2 Scaling Vectors (Can Grow Without Redesign)

1. **Model size** — Any GGUF model. 16B/32B when mobile RAM permits.
2. **Memory corpus** — SQLite + HNSW scales to millions. 10 years of interactions (~3.6M entries) within operational range.
3. **Extension count** — No hard limit. 100+ extensions architecturally supported.
4. **Context window** — Token budget is a constant. Change 2048→32768 by editing constant.
5. **Tool count** — Trait-based dispatch scales. Practical limit ~30-50 tools before LLM selection degrades.

### 14.3 Scaling Walls (Require Redesign)

1. **Concurrent tasks** — `&mut self` enforces single-task at type level. Requires actor model or interior mutability. Estimated: 4-8 weeks senior engineer.

2. **Multi-device sync** — All data device-local. Requires CRDT/OT, encrypted transport, sync state machine. Conflicts with anti-cloud Iron Law unless peer-to-peer.

3. **IPC message size** — 64KB limit. Future 32K+ token contexts exceed limit. Requires streaming/chunked protocol.

4. **Real-time streaming** — Single-threaded event loop with blocking IPC. High-frequency inputs require async pipeline with backpressure.

5. **Platform portability** — Deep Android dependencies (sysfs, battery APIs, screen reader). Porting requires platform abstraction layer. `aura-types` and `aura-neocortex` are portable; `aura-daemon` is Android-coupled.

### 14.4 Future-Proofing Score

| Category | Score | Rationale |
|----------|-------|-----------|
| Model agility | 9/10 | Any GGUF, cascade selection, near-zero friction |
| Data scalability | 8/10 | SQLite + HNSW + archive. Proven stack. |
| Tool extensibility | 7/10 | Trait-based dispatch, clean registration |
| API extensibility | 5/10 | Extension system immature |
| Concurrency | 3/10 | Single-task, fundamental redesign needed |
| Platform portability | 4/10 | Android-only, deep sysfs dependencies |
| **Overall** | **5.8/10** | Strong data/model layer, weak concurrency/platform |

### 14.5 Assessment

AURA v4 is designed as **single-user, single-device, privacy-first personal AI**. Within constraints, it scales well — model-agnostic, memory-scalable, extension-ready. Scaling walls (concurrency, multi-device, real-time) are appropriate v4 limitations. Breaking through them is v5+ work requiring intentional architectural evolution, not patches.

---

*[End of Part B — Sections §15-§18 continue in Part C]*
