# AURA v4 — Enterprise Code Review

**Date:** March 2026  
**Codebase:** 147,441 lines of Rust | 208 source files | 4 crates  
**Target:** aarch64-linux-android (ARM64, on-device AI assistant)  
**Distribution:** Termux-based `git clone` + `bash install.sh`  
**Review Model:** 9-domain specialist panel, enterprise-grade methodology  
**Repository:** https://github.com/AdityaPagare619/aura

---

## Table of Contents

- [§1: Executive Summary](#1-executive-summary)
- [§2: Review Methodology](#2-review-methodology)
- [§3: Domain Scorecard](#3-domain-scorecard)
- [§4: Critical Findings Master List](#4-critical-findings-master-list)
- [§5: Overall Assessment](#5-overall-assessment)
- [§6: Architecture Review](#6-architecture-review)
- [§7: Rust Core Review](#7-rust-core-review)
- [§8: Security & Cryptography Review](#8-security--cryptography-review)
- [§9: CI/CD & DevOps Review](#9-cicd--devops-review)
- [§10: Test Quality Review](#10-test-quality-review)
- [§11: Documentation-vs-Code Consistency Review](#11-documentation-vs-code-consistency-review)
- [§12: Operational Capacity Review](#12-operational-capacity-review)
- [§13: Plugin/Extension Architecture Review](#13-pluginextension-architecture-review)
- [§14: Scalability & Resource Management Review](#14-scalability--resource-management-review)
- [§15: Cross-Domain Correlation Analysis](#15-cross-domain-correlation-analysis)
- [§16: External Standard Reconciliation](#16-external-standard-reconciliation)
- [§17: Prioritized Action Plan](#17-prioritized-action-plan)
- [§18: Appendices](#18-appendices)

---

## §1: Executive Summary

AURA v4 is an ambitious on-device Android AI assistant implementing a **bi-cameral cognitive architecture** with genuine reasoning capabilities. The codebase demonstrates sophisticated system design: an 11-stage execution pipeline, 4-tier memory hierarchy, 6-layer inference teacher stack, and privacy-first on-device operation.

**Key Statistics:**
- 147,441 lines of production Rust code
- 208 source files across 4 crates
- Two-process architecture: daemon (20-50MB) + neocortex (500MB-2GB)
- Default LLM: Qwen3-8B-Q4_K_M (32K context)

**Critical Assessment:**
- **14 critical findings** identified across 7 domains
- **7 of 9 domains** have ship-blocking issues
- Estimated remediation: **~33 engineer-hours** (Sprint 0)

**Verdict:** AURA v4 is **NOT ship-ready** in its current state. The architecture is sound and the core Rust quality is high, but critical gaps in security (timing attacks, missing zeroize), testing (hollow tests, zero ReAct coverage), CI/CD (broken pipelines), and Android integration (hardcoded stubs) must be resolved before production deployment.

---

## §2: Review Methodology

### 2.1 Review Panel Composition

This enterprise code review was conducted by a **9-domain specialist panel**, each domain expert reviewing independently before cross-correlation:

| Domain | Focus Area |
|--------|------------|
| Architecture & System Design | Bi-cameral design, pipeline flow, memory tiers |
| Rust Core | Language idioms, error handling, unsafe usage |
| Security & Cryptography | Vault implementation, key management, install security |
| Performance & Concurrency | Memory footprint, latency, parallelism |
| LLM/AI Integration | FFI bindings, inference stack, token management |
| Android/Mobile Platform | JNI, accessibility, power management |
| CI/CD & DevOps | Pipelines, toolchain, release process |
| Test Quality | Coverage, assertion quality, test architecture |
| Documentation-vs-Code | Consistency between specs and implementation |

### 2.2 Tools & Techniques

- **Static Analysis:** `cargo clippy`, `cargo check --workspace`, custom AST grep
- **Code Reads:** Manual line-by-line review of critical paths
- **Architecture Tracing:** Data flow analysis through all 11 executor stages
- **Cross-Reference:** Documentation vs. code constant matching
- **Metrics Collection:** .unwrap() count, unsafe blocks, .clone() patterns

### 2.3 Scope

- All 4 crates: `aura-daemon`, `aura-neocortex`, `aura-types`, `aura-llama-sys`
- CI/CD configuration: `.github/workflows/ci.yml`, `release.yml`
- Installation: `install.sh` (1004 lines)
- Documentation: `docs/` directory, inline comments

---

## §3: Domain Scorecard

| Domain | Grade | Critical Findings | High Findings | Ship-Blocker? |
|--------|-------|-------------------|---------------|---------------|
| Architecture | **B+** | 0 | 1 | NO |
| Rust Core | **B+** | 1 | 3 | YES |
| Security | **C-** | 4 | 2 | YES |
| Performance | **B** | 0 | 4 | NO |
| LLM/AI | **B+** | 2 | 1 | YES |
| Android | **D+** | 2 | 9 | YES |
| CI/CD | **C** | 4 | 2 | YES |
| Test Quality | **C+** | 2 | 3 | YES |
| Docs-vs-Code | **D+** | 2 | 4 | YES |

**Summary:** 7 of 9 domains have ship-blocking issues. Only Architecture and Performance pass without critical blockers.

---

## §4: Critical Findings Master List

### Security Domain (4 Critical)

| ID | Title | Location | Description |
|----|-------|----------|-------------|
| SEC-CRIT-001 | Timing Attack in PIN Verification | `vault.rs:811-812` | Uses `==` for PIN comparison instead of constant-time comparison. Enables timing side-channel attack (CWE-208). |
| SEC-CRIT-002 | Missing Zeroize on Key Material | `vault.rs` (multiple) | `zeroize` crate is transitive dependency but never imported. Key material persists in memory after use. |
| SEC-CRIT-003 | Placeholder SHA256 Checksums | `install.sh:39,44,49` | Checksums are placeholder strings, not real hashes. MITM attack possible during installation. |
| SEC-CRIT-004 | Unsalted SHA256 PIN Hash | `install.sh:884` | PIN stored as unsalted SHA256. Rainbow table attack trivial for 4-6 digit PINs. |

### LLM/AI Domain (2 Critical)

| ID | Title | Location | Description |
|----|-------|----------|-------------|
| LLM-CRIT-001 | Undefined Behavior in FFI | `lib.rs:1397` | `*const c_char` cast to `*mut c_char` for llama.cpp call. UB if llama.cpp writes to buffer. |
| LLM-CRIT-002 | GBNF Not Constraining Decode | `inference.rs` | GBNF grammar applied post-hoc for validation only, not during token generation. 5/6 teacher layers real, 1 partial. |

### Android Domain (2 Critical)

| ID | Title | Location | Description |
|----|-------|----------|-------------|
| AND-CRIT-001 | Hardcoded Action Success | `android/actions.rs` | `simulate_action_result()` returns hardcoded success for all actions. No real Android integration. |
| AND-CRIT-002 | Missing AccessibilityService | `android/accessibility.rs` | AccessibilityService is stub only. No real screen reading or UI automation capability. |

### CI/CD Domain (2 Critical, 2 Overlap with SEC)

| ID | Title | Location | Description |
|----|-------|----------|-------------|
| CI-CRIT-001 | Toolchain Conflict | `ci.yml` vs `rust-toolchain.toml` | CI uses stable, rust-toolchain.toml pins nightly-2026-03-01. Builds may diverge. |
| CI-CRIT-002 | Broken Release Pipeline | `release.yml` | Missing `--features stub` for cross-compile, missing submodule checkout for llama.cpp. Release builds fail. |

### Test Quality Domain (2 Critical)

| ID | Title | Location | Description |
|----|-------|----------|-------------|
| TEST-CRIT-001 | 45 Hollow Integration Tests | `integration_tests.rs` | Tests use tautological assertions (`assert!(true)`, `assert_eq!(x, x)`). Zero behavioral verification. |
| TEST-CRIT-002 | Zero ReAct Test Coverage | `react.rs` | 2,821 lines of core reasoning engine with 0 test functions. Most critical code path untested. |

### Documentation Domain (2 Critical)

| ID | Title | Location | Description |
|----|-------|----------|-------------|
| DOC-CRIT-001 | Trust Tier Inconsistency | Multiple docs | 5 different trust tier schemes across docs. Code has 5 tiers (Stranger→Soulmate), docs say 4 (STRANGER→INTIMATE). |
| DOC-CRIT-002 | Ethics Rule Count Mismatch | Multiple docs | Ground Truth says 15 rules, Security Model says 11, CLAUDE.md says 7+4. Code has 11. |

---

## §5: Overall Assessment

### 5.1 Weighted Grade

Applying domain importance weights for a production Android AI assistant:

| Domain | Weight | Grade Points | Weighted |
|--------|--------|--------------|----------|
| Architecture | 15% | 3.3 (B+) | 0.495 |
| Rust Core | 15% | 3.3 (B+) | 0.495 |
| Security | 20% | 1.7 (C-) | 0.340 |
| Performance | 10% | 3.0 (B) | 0.300 |
| LLM/AI | 15% | 3.3 (B+) | 0.495 |
| Android | 10% | 1.3 (D+) | 0.130 |
| CI/CD | 5% | 2.0 (C) | 0.100 |
| Test Quality | 5% | 2.3 (C+) | 0.115 |
| Docs-vs-Code | 5% | 1.3 (D+) | 0.065 |

**Weighted GPA: 2.535 (C+)**

### 5.2 Architecture Strengths

Despite critical gaps, AURA v4 demonstrates genuinely sophisticated architecture:

1. **Bi-Cameral Cognition:** System 1 (DGS fast templates) + System 2 (SemanticReact LLM reasoning) — correctly routes all real queries to LLM per Iron Laws
2. **11-Stage Executor:** Complete pipeline with PolicyGate (stage 2.5) and Sandbox (stage 2.6) for security
3. **4-Tier Memory:** Working (RAM, 1024 slots) → Episodic (SQLite+HNSW) → Semantic (FTS5+HNSW) → Archive (ZSTD/LZ4)
4. **6-Layer Teacher Stack:** CoT → Logprob → Cascade retry → Cross-model reflection → Best-of-N (GBNF is partial)
5. **Privacy-First:** All data on-device, no telemetry, GDPR export/delete support

### 5.3 Ship-Blocking Gaps

Seven domains require remediation before production:

1. **Security:** Timing attacks enable credential theft; missing zeroize leaks keys
2. **Testing:** Hollow tests provide false confidence; ReAct (core logic) completely untested
3. **CI/CD:** Broken pipelines mean releases cannot be verified
4. **Android:** Stubs mean actual device features don't work
5. **LLM/AI:** FFI UB could cause crashes or security vulnerabilities
6. **Documentation:** Inconsistencies will confuse maintainers and auditors

### 5.4 Remediation Estimate

**Sprint 0: ~33 Engineer-Hours**

| Fix Category | Estimated Hours |
|--------------|-----------------|
| CI-CRIT-001/002 (CI pipeline) | 4h |
| TEST-CRIT-001/002 (real tests) | 8h |
| SEC-CRIT-001/002 (vault hardening) | 4h |
| LLM-CRIT-001/002 (FFI + GBNF) | 6h |
| DOC-CRIT-001/002 (canonicalize) | 4h |
| AND-CRIT-001/002 (Android stubs) | 4h |
| SEC-CRIT-003/004 (install.sh) | 3h |

---

## §6: Architecture Review

**Grade: B+**  
**Verdict:** Sound architecture, no ship-blockers. Minor improvements recommended.

### 6.1 Architecture Claims Verification

All 6 major architecture claims from Ground Truth documentation were **VERIFIED**:

| Claim | Status | Evidence |
|-------|--------|----------|
| Bi-Cameral Cognition | ✅ VERIFIED | `classify_task()` in `react.rs` routes System 1 vs System 2. Always returns `SemanticReact` per Iron Laws. |
| 11-Stage Executor | ✅ VERIFIED | `executor.rs:1539` lines. All 11 stages confirmed with PolicyGate at 2.5, Sandbox at 2.6. |
| 3-Tier Planner | ✅ VERIFIED | ETG (≥0.6 confidence) → Template → LLM fallback. `planner.rs:1845` lines. |
| 6-Layer Teacher Stack | ✅ VERIFIED (5/6) | GBNF→CoT→Logprob→Cascade→Reflection→BoN. GBNF is post-hoc only (partial). |
| 4-Tier Memory | ✅ VERIFIED | Working (1024 slots) → Episodic (SQLite+HNSW) → Semantic (FTS5) → Archive (ZSTD/LZ4) |
| IPC Protocol | ✅ VERIFIED | Unix domain socket, bincode serialization, 32KB buffer |

### 6.2 Crate Dependency Graph

```
aura-daemon (main orchestrator)
    │
    ├──► aura-types (shared data structures)
    │         ▲
    │         │
    └──► aura-neocortex (LLM inference)
              │
              └──► aura-llama-sys (FFI to llama.cpp)
```

### 6.3 Two-Process Architecture

| Process | Role | Memory Footprint |
|---------|------|------------------|
| PID1: aura-daemon | Orchestration, memory, execution | 20-50MB baseline |
| PID2: aura-neocortex | LLM inference, token generation | 500MB-2GB (model dependent) |

### 6.4 11-Stage Executor Pipeline

```
Stage 1.0: Request Intake
Stage 2.0: Context Assembly
Stage 2.5: PolicyGate ← Security checkpoint
Stage 2.6: Sandbox ← Isolation layer
Stage 3.0: Intent Classification
Stage 4.0: Plan Generation
Stage 5.0: Plan Validation
Stage 6.0: Tool Selection
Stage 7.0: Tool Execution
Stage 8.0: Result Synthesis
Stage 9.0: Response Generation
Stage 10.0: Memory Commit
Stage 11.0: Response Delivery
```

### 6.5 Findings

| ID | Severity | Title | Description |
|----|----------|-------|-------------|
| F-01 | HIGH | .unwrap() Proliferation | 712 .unwrap() calls across workspace (640 in aura-daemon). ~95% in test code, but production paths have ~35-50 unwraps. |
| F-02 | MEDIUM | God File | `main_loop.rs` is 7,348 lines. Should be decomposed into focused modules. |
| F-03 | MEDIUM | DGS Dead Code | `classify_task()` always returns SemanticReact. DGS templates exist but are unreachable. |
| F-04 | LOW | Hardcoded score_plan() | `planner.rs` `score_plan()` returns 0.5 unconditionally. Plan scoring non-functional. |
| F-05 | LOW | Dual Token Limits | MAX_ITERATIONS=10 (daemon) vs MAX_REACT_ITERATIONS=5 (neocortex). Undocumented. |
| F-06 | LOW | Context Underutilization | DEFAULT_CONTEXT_BUDGET=2048 of 32768 available (6.25%). |
| F-07 | LOW | OCEAN Defaults Mismatch | ipc.rs has 0.85/0.75/0.50/0.70/0.25, Ground Truth says all 0.5. |

### 6.6 Architecture Review Criteria

| Criterion | Grade | Notes |
|-----------|-------|-------|
| SOLID Principles | B+ | Good separation, minor SRP violations in main_loop.rs |
| Coupling | A- | Crates well-isolated, clean dependency graph |
| Data Flow | A | Clear request→response path through all 11 stages |
| Module Boundaries | A | 4-crate structure enforces boundaries |
| Iron Laws Compliance | A- | Correctly routes all reasoning to LLM |
| Scalability | B+ | Memory tiers handle growth, but some O(n) operations |
| Error Propagation | B | Good thiserror usage, some unwrap leakage |

---

## §7: Rust Core Review

**Grade: B+**  
**Verdict:** High-quality Rust with 1 critical security finding.

### 7.1 Code Quality Metrics

| Metric | Count | Assessment |
|--------|-------|------------|
| Total .unwrap() | 925 | ~95% in test code. Production unwraps need audit. |
| Unsafe Blocks | 70 | All in FFI layer (aura-llama-sys). Necessary for llama.cpp. |
| .clone() Calls | 578 | 50-100 avoidable in hot paths. Optimization opportunity. |
| &String Anti-Pattern | 0 | Excellent. All functions use &str properly. |

### 7.2 Positive Patterns

1. **Excellent thiserror Usage:** Custom error types with proper From implementations
2. **Zero &String:** All string parameters use &str
3. **Poison-Safe Mutex:** Proper handling of poisoned mutexes
4. **Real Cryptography:** AES-256-GCM + Argon2id (not toy crypto)
5. **Proper Option/Result:** Minimal unwrap in critical paths

### 7.3 Findings

| ID | Severity | Title | Location | Description |
|----|----------|-------|----------|-------------|
| RC-01 | CRITICAL | Timing Attack | `vault.rs:811-812` | PIN comparison uses `==` instead of constant-time. SEC-CRIT-001. |
| RC-02 | HIGH | God File | `main_loop.rs` | 7,348 lines. Violates SRP. Needs decomposition. |
| RC-03 | HIGH | RC Dependency | `Cargo.toml` | `bincode = "2.0.0-rc.3"` is release candidate. Pin stable version. |
| RC-04 | HIGH | Missing SAFETY | All `unsafe impl` | 70 unsafe blocks lack //SAFETY: documentation. |
| RC-05 | MEDIUM | Clone in Hot Path | `react.rs`, `inference.rs` | ~50 unnecessary clones in token processing loop. |
| RC-06 | MEDIUM | Vec Shift O(n) | 4 locations | `Vec::remove(0)` causes O(n) shift. Use VecDeque. |
| RC-07 | MEDIUM | Mutex Contention | `embedding.rs` | Global embedding cache behind single Mutex. |
| RC-08 | LOW | Magic Numbers | Multiple | Hardcoded constants without named const. |
| RC-09 | LOW | Dead Code | `dgs.rs` | DGS templates unreachable due to classify_task(). |

### 7.4 Error Handling Assessment

```rust
// GOOD: Proper error propagation
pub fn process_request(&self, req: Request) -> Result<Response, DaemonError> {
    let context = self.build_context(&req)?;  // Propagates error
    let plan = self.generate_plan(&context)?;  // Propagates error
    // ...
}

// BAD: Unwrap in production path (found in ~35 locations)
let config = self.config.lock().unwrap();  // Panics on poison
```

### 7.5 Recommended Fixes

1. Replace `==` with `constant_time_eq` for all credential comparisons
2. Add `use zeroize::Zeroize` and derive on key material structs
3. Decompose `main_loop.rs` into focused modules (<500 lines each)
4. Pin bincode to stable release when available
5. Add //SAFETY: comments to all unsafe blocks

---

## §8: Security & Cryptography Review

**Grade: C-**  
**Verdict:** Genuine security architecture undermined by 4 critical implementation gaps.

### 8.1 Security Posture Summary

AURA v4 demonstrates **security-by-default design** with real cryptographic primitives:
- AES-256-GCM for encryption (not toy XOR)
- Argon2id for key derivation (not MD5/SHA1)
- 4-tier data classification
- 5-tier trust relationship model
- Anti-sycophancy controls (RING_SIZE=20, BLOCK_THRESHOLD=0.40)

However, **4 critical implementation gaps** undermine the design.

### 8.2 Critical Findings

#### SEC-CRIT-001: Timing Attack in PIN Verification

**Location:** `crates/aura-daemon/src/persistence/vault.rs:811-812`

```rust
// VULNERABLE CODE
if stored_pin == provided_pin {  // Line 811-812
    return Ok(true);
}
```

**Impact:** Attacker can determine PIN character-by-character by measuring response time. 4-digit PIN crackable in <10,000 timing measurements.

**Fix:**
```rust
use subtle::ConstantTimeEq;
if stored_pin.ct_eq(&provided_pin).into() {
    return Ok(true);
}
```

#### SEC-CRIT-002: Missing Zeroize on Key Material

**Location:** `vault.rs`, `crypto.rs` (multiple structs)

**Issue:** `zeroize` crate is a transitive dependency but never directly imported. Key material structs don't derive `Zeroize` or `ZeroizeOnDrop`.

```rust
// CURRENT (keys persist in memory)
struct VaultKey {
    key: [u8; 32],
    // No Drop impl, no zeroize
}

// REQUIRED
use zeroize::{Zeroize, ZeroizeOnDrop};
#[derive(Zeroize, ZeroizeOnDrop)]
struct VaultKey {
    key: [u8; 32],
}
```

#### SEC-CRIT-003: Placeholder SHA256 Checksums

**Location:** `install.sh:39,44,49`

```bash
DAEMON_SHA256="placeholder_checksum_replace_in_release"  # Line 39
NEOCORTEX_SHA256="placeholder_checksum_replace_in_release"  # Line 44
MODEL_SHA256="placeholder_checksum_replace_in_release"  # Line 49
```

**Impact:** No integrity verification during installation. MITM attacker can substitute malicious binaries.

#### SEC-CRIT-004: Unsalted SHA256 PIN Hash

**Location:** `install.sh:884`

```bash
echo -n "$PIN" | sha256sum | cut -d' ' -f1 > ~/.aura/pin_hash
```

**Impact:** Rainbow table attack trivial. 4-digit PIN (10,000 combinations) pre-computable.

**Fix:** Use Argon2id with random salt (consistent with vault.rs):
```bash
# Generate salt
SALT=$(head -c 16 /dev/urandom | base64)
# Hash with argon2
echo -n "${SALT}${PIN}" | argon2 "$SALT" -id -t 3 -m 16 -p 4 | tail -1 > ~/.aura/pin_hash
```

### 8.3 Configuration Mismatches

| Parameter | Documentation | Code | Risk |
|-----------|--------------|------|------|
| Trust Tiers | 4 (STRANGER→INTIMATE) | 5 (Stranger→Soulmate) | Confusion in access control |
| Data Tiers | Public/Internal/Confidential/Restricted | Ephemeral/Personal/Sensitive/Critical | Inconsistent classification |
| Argon2id p= | 1 | 4 | Documentation outdated |
| Ethics Rules | 15 (Ground Truth) | 11 (code) | Incomplete enforcement |

### 8.4 Positive Security Findings

1. **Real Crypto:** AES-256-GCM with proper IV generation
2. **Strong KDF:** Argon2id with 64MB memory, 3 iterations
3. **Anti-Sycophancy:** Active disagreement tracking prevents manipulation
4. **Sandbox Stage:** Executor stage 2.6 provides isolation
5. **PolicyGate:** Stage 2.5 enforces trust-based access control
6. **No Telemetry:** Zero cloud callbacks, all data on-device

### 8.5 Remediation Priority

| Priority | Finding | Effort | Impact |
|----------|---------|--------|--------|
| P0 | SEC-CRIT-001 (timing) | 1h | Prevents credential theft |
| P0 | SEC-CRIT-002 (zeroize) | 2h | Prevents key leakage |
| P1 | SEC-CRIT-003 (checksums) | 1h | Prevents MITM |
| P1 | SEC-CRIT-004 (PIN salt) | 1h | Prevents rainbow attack |

---

*[End of Part A — Sections §9-§18 continue in Part B and Part C]*
