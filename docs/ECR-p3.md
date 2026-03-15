## §12 CI/CD & DevOps

### 12.1 Overview

The CI/CD pipeline is in a state of complete dysfunction. Three GitHub Actions workflows exist; none produce a valid artifact. The root cause is not complex — a toolchain mismatch and two missing flags — but the consequence is that AURA v4 has never had a working automated build. Every release has been manually produced, meaning release integrity guarantees are zero.

### 12.2 Critical CI Issues

#### CI-CRIT-001 — Toolchain Mismatch: CI uses `stable`, project requires `nightly`
**Files:** `.github/workflows/ci.yml` vs `rust-toolchain.toml`

```yaml
# ci.yml — what CI uses:
- uses: dtolnay/rust-toolchain@stable
```

```toml
# rust-toolchain.toml — what the project actually needs:
[toolchain]
channel = "nightly-2026-03-01"
components = ["rustfmt", "clippy", "rust-src"]
```

The project uses `nightly`-only features (const generics extensions, `std::simd`, proc-macro features). The `stable` toolchain cannot compile this codebase. Every CI run fails at the compile step. This has been true since the project was created.

**Fix:**
```yaml
- uses: dtolnay/rust-toolchain@master
  with:
    toolchain: nightly-2026-03-01
    components: rustfmt, clippy, rust-src
```

#### CI-CRIT-002 — Release Workflow Missing Critical Flags
**File:** `.github/workflows/release.yml`

Two independent failures in the release workflow:

**Missing `--features stub`:**
```yaml
# release.yml — current (broken):
- run: cargo build --release
# Missing required feature flag:
- run: cargo build --release --features stub
```

The release build requires `--features stub` to compile without the hardware components that aren't available in CI. Without it, compilation fails on missing platform bindings.

**Missing `submodules: recursive`:**
```yaml
# release.yml — current (broken):
- uses: actions/checkout@v3
# Missing:
- uses: actions/checkout@v3
  with:
    submodules: recursive
```

llama.cpp is a git submodule. Without recursive submodule checkout, the `aura-neocortex` build fails with missing source files.

**Combined fix:**
```yaml
- uses: actions/checkout@v3
  with:
    submodules: recursive
    fetch-depth: 0

- uses: dtolnay/rust-toolchain@master
  with:
    toolchain: nightly-2026-03-01

- run: cargo build --release --features stub
```

### 12.3 High CI Issues

#### CI-HIGH-001 — Android CI Has Never Produced a Working APK

The Android CI workflow attempts to build the APK but fails because:
1. No NDK version is pinned — uses whatever GitHub Actions provides
2. No `ANDROID_NDK_HOME` environment variable set for Cargo cross-compilation
3. Gradle build depends on pre-compiled Rust `.so` files that don't exist in CI
4. `aarch64-linux-android` Rust target not installed in CI toolchain step

No fix has been applied to any of these four issues. The Android CI pipeline is entirely decorative.

#### CI-HIGH-002 — No Secret Scanning or Dependency Audit

The CI pipeline has no:
- `cargo audit` step (checks for known CVEs in dependencies)
- Secret scanning (no check for accidentally committed API keys, credentials)
- License compliance check

Given the security findings elsewhere (SEC-CRIT-003: placeholder checksums, SEC-HIGH-001: Telegram API key), these omissions are material risks.

#### CI-HIGH-003 — No Integration Test Stage

CI runs unit tests only (`cargo test --lib`). The integration test suite (`integration_tests.rs`) — which contains the 45 tautological tests — is not run in CI. Even the tautological tests passing would provide signal; they don't run at all.

### 12.4 CI/CD Findings Summary

| ID | Severity | Location | Issue |
|----|----------|----------|-------|
| CI-CRIT-001 | Critical | `ci.yml` | Wrong toolchain — every build fails |
| CI-CRIT-002 | Critical | `release.yml` | Missing `--features stub` + submodules |
| CI-HIGH-001 | High | Android workflow | Never produced working APK |
| CI-HIGH-002 | High | All workflows | No `cargo audit`, no secret scan |
| CI-HIGH-003 | High | All workflows | Integration tests not run in CI |
| CI-MED-001 | Medium | `release.yml` | No code signing for release artifacts |
| CI-MED-002 | Medium | All workflows | No caching for Cargo registry/build |
| CI-MED-003 | Medium | All workflows | No matrix testing across Android API levels |
| CI-LOW-001 | Low | All workflows | No build status badge in README |

---

## §13 Test Quality

### 13.1 Overview

The AURA v4 test suite presents a dangerous illusion: `cargo test` passes, giving false confidence. Examination of the test bodies reveals systematic failure — 45 tautological assertions that test nothing, and the most critical module in the codebase (the 2821-line ReAct engine) has zero test functions. The project has a test suite in the same way a movie set has buildings — the facade is there, the structure is not.

### 13.2 Critical Test Issues

#### TEST-CRIT-001 — 45 Tautological Tests
**File:** `integration_tests.rs` (and scattered unit test modules)

45 test functions contain assertions that are always true regardless of system behavior:

```rust
// Pattern 1 — Pure tautology:
#[test]
fn test_memory_storage_works() {
    assert!(true);  // Always passes. Tests nothing.
}

// Pattern 2 — Disjunction tautology:
#[test]
fn test_vault_encrypt_decrypt() {
    let result = vault.encrypt(b"test");
    assert!(result.is_ok() || result.is_err());  // Always true by definition
}

// Pattern 3 — Existence check only:
#[test]
fn test_react_engine_creates() {
    let engine = ReactEngine::new(config);
    assert!(engine.is_ok());  // Only tests construction, not behavior
}
```

These tests consume CI time, appear in coverage reports, and provide zero detection capability for regressions.

**Most egregious example — the vault test that misses SEC-CRIT-001:**
```rust
#[test]
fn test_vault_hmac_verification() {
    let vault = Vault::new(test_config());
    let data = b"sensitive data";
    let stored = vault.store(data).unwrap();
    let retrieved = vault.retrieve(&stored).unwrap();
    assert_eq!(data, &retrieved[..]);
    // NEVER TESTS: timing behavior of HMAC comparison
    // SEC-CRIT-001 would have been caught here if the test existed
}
```

#### TEST-CRIT-002 — ReAct Engine: Zero Test Functions
**File:** `react.rs` — 2821 lines, 0 test functions

The ReAct engine is the core orchestration loop of AURA. It manages:
- LLM prompt construction
- Tool selection and dispatch
- Iteration control (up to 10 iterations)
- Context accumulation
- Error recovery

It has **no tests**. Not one. The 2821 lines execute in production but have never been exercised by automated verification. This means:
- The LLM-HIGH-001 bug (hardcoded `SemanticReact`) was never caught by tests
- The PERF-CRIT-C1 bug (new TCP socket per iteration) was never caught
- Any regression in the ReAct loop goes undetected until a user notices

### 13.3 Coverage Analysis

| Module | Lines | Test Functions | Meaningful Assertions | Coverage |
|--------|-------|---------------|----------------------|----------|
| `react.rs` | 2821 | 0 | 0 | 0% |
| `vault.rs` | 1240 | 8 | 3 real, 5 tautological | ~12% |
| `episodic.rs` | 890 | 4 | 4 real | ~18% |
| `semantic.rs` | 760 | 3 | 2 real, 1 tautological | ~14% |
| `consolidation.rs` | 680 | 2 | 0 real (both tautological) | 0% |
| `hnsw.rs` | 540 | 5 | 5 real | ~45% |
| `inference.rs` | 420 | 3 | 1 real, 2 tautological | ~8% |
| `context.rs` | 310 | 4 | 3 real | ~35% |
| **Total (key modules)** | **7661** | **29** | **~18 real** | **~15%** |

### 13.4 Missing Test Categories

**Security tests (none exist):**
- Constant-time comparison verification
- Key zeroization on drop
- Vault tamper detection
- PIN hash resistance

**Concurrency tests (none exist):**
- Lock ordering under concurrent access
- WakeLock race conditions
- Token budget under parallel requests

**Regression tests for known bugs (none exist):**
Every finding in this document should have a corresponding regression test added BEFORE the fix is implemented (TDD-style).

### 13.5 Test Quality Findings Summary

| ID | Severity | Location | Issue |
|----|----------|----------|-------|
| TEST-CRIT-001 | Critical | `integration_tests.rs` + scattered | 45 tautological assertions |
| TEST-CRIT-002 | Critical | `react.rs` | 2821 lines, zero tests |
| TEST-HIGH-001 | High | `consolidation.rs` | 0% meaningful coverage |
| TEST-HIGH-002 | High | `vault.rs` | SEC-CRIT-001 would be caught by proper test |
| TEST-MED-001 | Medium | Multiple | No property-based tests for crypto operations |
| TEST-MED-002 | Medium | Multiple | No concurrency/race condition tests |
| TEST-LOW-001 | Low | Multiple | Test names don't describe what is tested |

---

## §14 Documentation vs Code Consistency

### 14.1 Overview

AURA v4 has ~32,000 lines of documentation — more documentation than most projects have code. The documentation is genuinely thoughtful and architecturally rich. However, systematic inconsistencies between documentation and implementation have accumulated, creating a situation where documentation is a liability: it describes a system that doesn't exist, while the system that does exist is not fully documented.

### 14.2 Critical Documentation Issues

#### DOC-CRIT-001 — Five-Way Trust Tier Inconsistency

AURA's trust tier system (governing what actions the agent can take autonomously) is defined differently in five separate documents:

| Document | Trust Tiers Defined |
|----------|---------------------|
| `GROUND-TRUTH-ARCHITECTURE.md` | 3 tiers: Sandboxed, Supervised, Autonomous |
| `SECURITY-MODEL.md` | 4 tiers: Sandboxed, Restricted, Supervised, Autonomous |
| `PLUGIN-SPEC.md` | 5 tiers: Untrusted, Sandboxed, Restricted, Supervised, Autonomous |
| `API-REFERENCE.md` | 3 tiers (different names): Read-only, Confirmation-required, Unrestricted |
| Code (`trust_tier.rs`) | 4 variants in enum (matches SECURITY-MODEL.md partially) |

The code implements 4 tiers but none of the 5 documents agrees with each other or the code. A developer reading any one document will have incorrect assumptions about the trust model.

#### DOC-CRIT-002 — Ethics Rules: Three Different Counts

| Document | Ethics Rules Count |
|----------|--------------------|
| Code (`ethics.rs`) | 11 rules |
| `ETHICS-FRAMEWORK.md` | 15 rules |
| `PLUGIN-SPEC.md` (references ethics) | 10 rules |

The canonical ethics rule set exists in code with 11 rules. Two documents describe different numbers. A plugin developer reading `ETHICS-FRAMEWORK.md` will expect 15 rules to be enforced; only 11 are.

### 14.3 Implementation Gaps (Documented but not Built)

| Feature | Documentation State | Code State |
|---------|---------------------|------------|
| DGS fast path | Fully documented with routing logic | `classify_task()` hardcoded to bypass |
| GBNF decode-time enforcement | Documented as decode-time | Implemented post-generation |
| Plugin sandboxing | Documented with capability model | `simulate_action_result()` placeholder |
| Multi-user context | Architecture doc describes it | Not implemented |
| Remote model serving | Documented as feature | Not implemented; would violate Iron Laws |

### 14.4 Documentation Findings Summary

| ID | Severity | Issue |
|----|----------|-------|
| DOC-CRIT-001 | Critical | 5-way trust tier inconsistency |
| DOC-CRIT-002 | Critical | Ethics rules: 3 different counts across 3 sources |
| DOC-HIGH-001 | High | `simulate_action_result()` documented as "implement later" — shipped as stub |
| DOC-MED-001 | Medium | DGS routing documented correctly but not implemented |
| DOC-MED-002 | Medium | Iron Laws document doesn't list Telegram as approved exception |
| DOC-MED-003 | Medium | Unsafe count in docs: 23 (actual: 70) |
| DOC-LOW-001 | Low | Crate names in docs: `aura-core`/`aura-android` (actual: `aura-neocortex`/`aura-types`) |

---

# PART III — OPERATIONAL ANALYSIS

---

## §15 Operational Resilience

### 15.1 Daemon Lifecycle

**Startup:**
- No liveness check before accepting requests — daemon may partially start and accept requests while subsystems are still initializing
- PolicyGate reads fabricated system state at startup (AND-HIGH-003) — initial decisions are based on fake data

**Runtime:**
- No watchdog: if the daemon's async runtime deadlocks (PERF-HIGH-006 risk), there is no external process to detect and restart it
- No circuit breaker on LLM calls: if neocortex becomes unresponsive, the daemon blocks indefinitely waiting for LLM response
- Memory consolidation runs as a background task with no budget limit — can starve foreground request processing during Deep consolidation passes

**Shutdown:**
- WakeLock released properly (if service is destroyed cleanly)
- SQLite connections not explicitly closed — rely on Drop
- HNSW index not persisted on shutdown — index must be rebuilt from SQLite on next start (potentially slow for large memory stores)

### 15.2 Recovery Scenarios

| Failure | Current Behavior | Expected Behavior |
|---------|-----------------|-------------------|
| LLM timeout | Blocks indefinitely | Timeout + error response |
| OOM kill | Silent crash | Graceful restart with state recovery |
| SQLite corruption | Panic | Corruption detection + recovery |
| HNSW desync | Silent wrong results | Checksum validation |
| Kotlin layer crash | JNI UB / silent hang | Supervised restart |

### 15.3 Operational Findings Summary

| ID | Severity | Issue |
|----|----------|-------|
| OPS-HIGH-001 | High | No timeout on LLM calls — indefinite block risk |
| OPS-HIGH-002 | High | No watchdog / self-healing for async deadlock |
| OPS-HIGH-003 | High | HNSW index not persisted — cold start rebuild |
| OPS-MED-001 | Medium | No circuit breaker for neocortex connection |
| OPS-MED-002 | Medium | Consolidation can starve foreground tasks |
| OPS-MED-003 | Medium | No structured startup health check sequence |

---

## §16 Plugin Architecture

### 16.1 Current State

The plugin architecture is documented at high sophistication (capability model, trust tiers, sandboxing specification) but the implementation is a placeholder:

```rust
pub fn simulate_action_result(action: &Action) -> ActionResult {
    // Iron Law violation: stub in production
    ActionResult::Success { output: "simulated".to_string() }
}
```

**Iron Law violation:** `simulate_action_result()` is explicitly flagged in the Iron Laws as "implement later." It shipped. This means all plugin action execution returns fabricated success responses. The agent can "complete tasks" that never actually executed.

### 16.2 Plugin Architecture Findings

| ID | Severity | Issue |
|----|----------|-------|
| PLUG-CRIT-001 | Critical | `simulate_action_result()` in production — all actions fabricated |
| PLUG-HIGH-001 | High | Trust tier model inconsistency (= DOC-CRIT-001) |
| PLUG-HIGH-002 | High | No capability enforcement at runtime |
| PLUG-MED-001 | Medium | Plugin loading has no signature verification |
| PLUG-MED-002 | Medium | No resource quota per plugin |

---

## §17 Scalability Analysis

### 17.1 Single-Device Architecture (Appropriate)

AURA is intentionally single-device, anti-cloud. Scalability concerns are therefore about vertical scaling (handling more complex tasks on one device) rather than horizontal scaling.

### 17.2 Vertical Scaling Constraints

| Resource | Current Limit | Bottleneck |
|----------|-------------|-----------|
| LLM inference | Sequential (1 request at a time) | llama.cpp single-context |
| Embedding | Global Mutex (single-threaded) | `embeddings.rs` |
| HNSW search | Sequential | No batching |
| Context budget | 2048 tokens (6.25% of model) | Config value |
| Memory (RAM) | 400MB ceiling | Android process limit |

### 17.3 Scalability Findings

| ID | Severity | Issue |
|----|----------|-------|
| SCALE-HIGH-001 | High | Context budget 16× below model capability |
| SCALE-HIGH-002 | High | Embedding serialized by global Mutex |
| SCALE-MED-001 | Medium | No request queuing — concurrent requests race |
| SCALE-MED-002 | Medium | HNSW index size unbounded — no pruning policy |
