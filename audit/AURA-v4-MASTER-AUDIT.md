# AURA v4 — Enterprise Code Review: Master Audit Document
## Document Control: AURA-v4-ECR-2026-001 · v1.0
**Date:** 2026-03-14  
**Codebase:** AURA v4 — On-Device Android AI Assistant  
**Lines of Code:** 147,441 (Rust) + ~3,200 (Kotlin/Android)  
**Review Type:** Multi-Domain Enterprise Code Review  
**Reviewers:** 9 specialist domains  
**Status:** FINAL — NOT READY FOR PRODUCTION

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Codebase Overview](#2-codebase-overview)
3. [Domain Review Grades](#3-domain-review-grades)
4. [Critical Findings Register (All 16)](#4-critical-findings-register)
5. [§1 — Rust Code Quality](#5-rust-code-quality)
6. [§2 — Architecture & System Design](#6-architecture--system-design)
7. [§3 — Security & Cryptography](#7-security--cryptography)
8. [§4 — Performance & Concurrency](#8-performance--concurrency)
9. [§5 — LLM / AI Integration](#9-llm--ai-integration)
10. [§6 — Android / Mobile Platform](#10-android--mobile-platform)
11. [§7 — CI/CD & DevOps](#11-cicd--devops)
12. [§8 — Test Quality](#12-test-quality)
13. [§9 — Documentation vs Code Consistency](#13-documentation-vs-code-consistency)
14. [§10 — Cross-Domain Synthesis](#14-cross-domain-synthesis)
15. [§11 — Prioritized Remediation Plan](#15-prioritized-remediation-plan)
16. [§12 — Appendices](#16-appendices)

---

## 1. Executive Summary

AURA v4 is a 147,441-line Rust on-device Android AI assistant implementing a bi-cameral cognitive architecture, AES-256-GCM encrypted memory vault, 11-stage executor with deny-by-default PolicyGate, 4-tier memory hierarchy, and a 6-layer LLM teacher stack. It is the most architecturally ambitious on-device assistant reviewed to date.

**The architecture is sound. The implementation has critical gaps.**

Nine specialist reviewers identified **16 critical findings, 30 high findings, and 51 medium findings**. Six of the nine domains are ship-blockers. The project is approximately 2–3 focused engineering days away from a minimally viable ship state ("Sprint 0"), and 5–7 weeks from production-grade quality.

### Overall Verdict

| Rating | Meaning |
|--------|---------|
| ⛔ NOT READY | Sprint 0 required before any public distribution |

### Strength Summary
- Genuine cryptographic primitives (AES-256-GCM, Argon2id, fresh CSPRNG nonces)
- Deny-by-default PolicyGate with principled capability enforcement
- Physics-based battery model and ISO 13732-1 thermal management
- Zero `&String` anti-patterns across 147K lines
- Principled error handling with `thiserror` + `?` operator
- Anti-sycophancy system (RING_SIZE=20, BLOCK_THRESHOLD=0.40) — rare and well-implemented
- 4-tier memory with graduated confidentiality enforcement
- OEM kill-prevention for 6 Android manufacturers

### Critical Gap Summary
- 4 security critical defects forming a complete key extraction attack chain
- 7 Android Kotlin defects causing guaranteed crashes on production hardware
- 2 LLM/FFI defects including undefined behavior in the C FFI layer
- CI pipeline cannot produce a working release artifact
- 45 hollow integration tests with tautological assertions
- 5-way inconsistency in trust tier definitions across code and documentation

---

## 2. Codebase Overview

### Repository Structure
```
aura-v4/
├── crates/
│   ├── aura-daemon/          # Main service process (~90K lines)
│   │   ├── daemon_core/      # ReAct loop, main_loop, token budget
│   │   ├── execution/        # 11-stage executor, planner (3-tier)
│   │   ├── persistence/      # Vault (AES-256-GCM + Argon2id)
│   │   ├── policy/           # PolicyGate, audit, boundaries, ethics
│   │   ├── identity/         # Personality (OCEAN+VAD), relationships, ethics
│   │   ├── memory/           # 4-tier memory, HNSW, embeddings, consolidation
│   │   ├── platform/         # Android JNI bridge, power, thermal, doze
│   │   ├── health/           # System monitor, watchdog
│   │   ├── bridge/           # System API bridge (~12 stubs)
│   │   └── extensions/       # Extension scaffold (~450 lines)
│   ├── aura-neocortex/       # LLM inference subprocess (~30K lines)
│   │   ├── inference.rs      # 6-layer teacher stack, 2286 lines
│   │   ├── context.rs        # Prompt assembly, token truncation
│   │   ├── model.rs          # GGUF model management
│   │   └── grammar.rs        # GBNF grammar (dead_code suppressed)
│   ├── aura-llama-sys/       # llama.cpp FFI bindings (~8K lines)
│   └── aura-types/           # Shared types, IPC protocol
├── android/                  # Kotlin Android layer (~3.2K lines)
│   └── app/src/main/kotlin/
│       ├── AuraDaemonBridge.kt
│       ├── AuraForegroundService.kt
│       ├── AuraAccessibilityService.kt
│       └── BootReceiver.kt
├── .github/workflows/        # CI/CD (3 workflows, all defective)
└── install.sh                # Termux install script (1004 lines)
```

### Key Metrics

| Metric | Value | Assessment |
|--------|-------|-----------|
| Total lines (Rust) | 147,441 | Large |
| Unsafe blocks | ~70 | Moderate (none with SAFETY comments) |
| `.unwrap()` calls | ~925 (~50 non-test) | Acceptable with fixes |
| `.clone()` calls | ~578 (50–100 avoidable) | Good |
| `&String` parameters | 0 | Excellent |
| Files >800 lines | 8 | Needs refactor |
| Test coverage (effective) | ~23% (~540/2376 tests) | Critical gap |
| Hollow tests | 45 | Critical gap |
| Documentation fidelity | ~55% | Critical gap |

### Architecture at a Glance

```
User Input
    ↓
[Daemon — main_loop.rs]
    ↓
[PolicyGate — deny-by-default]
    ↓
[11-Stage Executor]
  Stage 1: Context assembly
  Stage 2: Memory retrieval
  Stage 2.5: PolicyGate check
  Stage 2.6: Sandbox validation
  Stage 3-11: Execution + response
    ↓
[3-Tier Planner: ETG → Template → LLM]
    ↓
[ReAct Loop — classify_task() → SemanticReact (always)]
    ↓
[Neocortex IPC]
    ↓
[6-Layer Teacher Stack]
  Layer 0: GBNF Grammar (post-hoc only ⚠️)
  Layer 1: Chain-of-Thought
  Layer 2: Logprob Confidence (4-channel Bayesian)
  Layer 3: Cascade Retry (max 3)
  Layer 4: Cross-model Reflection (Brainstem1_5B)
  Layer 5: Best-of-N (BON_SAMPLES=3)
    ↓
[Response → Vault + Memory → User]
```

---

## 3. Domain Review Grades

| # | Domain | Grade | Score | Ship-Blocker | Critical Count |
|---|--------|-------|-------|-------------|---------------|
| §1 | Rust Code Quality | B+ | 83/100 | YES (timing attack) | 1 |
| §2 | Architecture & System Design | B+ | 85/100 | NO | 0 |
| §3 | Security & Cryptography | C+ | 67/100 | YES | 4 |
| §4 | Performance & Concurrency | B | 78/100 | PARTIAL | 4 |
| §5 | LLM / AI Integration | B+ | 80/100 | YES | 2 |
| §6 | Android / Mobile Platform | B- | 65/100 | YES | 7 |
| §7 | CI/CD & DevOps | C | 60/100 | YES | 4 |
| §8 | Test Quality | C+ | 55/100 | YES | 2 |
| §9 | Docs vs Code Consistency | D+ | 45/100 | YES | 2 |
| — | **Composite** | **B-** | **69/100** | **YES** | **16** |

### Ship-Blocker Summary (6 of 9 domains)

```
§3 Security ──── 4 criticals ──── key extraction attack chain
§6 Android ──── 7 criticals ──── guaranteed launch crashes
§7 CI/CD ────── 4 criticals ──── cannot produce release artifacts
§5 LLM/AI ───── 2 criticals ──── FFI undefined behavior + broken grammars
§8 Tests ──────  2 criticals ──── 45 hollow tests, ReAct untested
§9 Docs ────────2 criticals ──── trust tier chaos, ethics count mismatch
```

---

## 4. Critical Findings Register

All 16 critical findings across all domains. Each is independently verifiable in source code.

| ID | Domain | File | Issue | CWE/Risk |
|----|--------|------|-------|---------|
| **CRIT-01** | Security | `vault.rs:811-812` | `==` comparison on HMAC output is not constant-time despite comment | CWE-208 Timing Attack |
| **CRIT-02** | Security | `vault.rs:~690` | AES-256 key stored as `Option<[u8;32]>`, no `Zeroize` on drop | CWE-316 Key in Memory |
| **CRIT-03** | Security/CI | `install.sh:39,44,49` | All 3 model SHA256 checksums are literal placeholder strings | CWE-494 No Integrity |
| **CRIT-04** | Security/CI | `install.sh:884` | PIN stored as `echo -n | sha256sum` — no salt, no KDF | CWE-916 Weak Hash |
| **CRIT-05** | LLM/FFI | `llama-sys/lib.rs:1397` | `tokens.as_ptr() as *mut LlamaToken` — const-to-mut cast is UB | CWE-119 Memory Safety |
| **CRIT-06** | LLM | `inference.rs:368-385` | GBNF grammar validated post-generation, not at decode time | Wasted compute + invalid JSON |
| **CRIT-07** | Android | `AuraForegroundService.kt` | Missing Android 14 `foregroundServiceType` — crashes on 40% of devices | API 34 `MissingForegroundServiceTypeException` |
| **CRIT-08** | Android | `AndroidManifest.xml` | `ACCESS_NETWORK_STATE` + `ACCESS_WIFI_STATE` undeclared | `SecurityException` crash on all devices |
| **CRIT-09** | Android | `AuraDaemonBridge.kt` | Sensor listeners registered, never unregistered | Memory leak + battery drain → OOM |
| **CRIT-10** | Android | `AuraForegroundService.kt` | WakeLock acquired with 10-min timeout, never renewed | Daemon freezes after 10 minutes |
| **CRIT-11** | Android | `AuraDaemonBridge.kt` | WakeLock compound read/release not atomic (`@Volatile` insufficient) | Race condition → double-release crash |
| **CRIT-12** | Android | `AuraAccessibilityService.kt` | `AccessibilityNodeInfo` objects not recycled in `findNodeByContentDesc` | Pool exhaustion → crash |
| **CRIT-13** | Android | `jni_bridge.rs` | No JNI exception check after Kotlin callbacks | Pending exception → UB on next JNI call |
| **CRIT-14** | CI/CD | `ci.yml` vs `rust-toolchain.toml` | CI uses `stable`, project requires `nightly-2026-03-01` | All CI builds use wrong toolchain |
| **CRIT-15** | CI/CD | `release.yml` | Missing `--features stub` and `submodules: recursive` | Every tagged release build fails |
| **CRIT-16** | Tests | `integration_tests.rs` | 45 hollow tests with tautological assertions; `react.rs` (2821 lines) has 0 tests | ~23% real coverage |

> **Note on deduplication:** CRIT-03 and CRIT-04 appear in both Security (§3) and CI/CD (§7) domain reviews. They are listed once here.

---

## 5. §1 — Rust Code Quality

**Grade: B+ (83/100) · Reviewer: Rust Core Specialist**

### Summary
The codebase demonstrates strong Rust idioms: zero `&String` parameters, principled `thiserror` error handling, and a poison-safe mutex pattern throughout. The single critical finding (timing attack) is a one-line fix. The primary technical debt is architectural (7,348-line god file) rather than systemic.

### Critical Finding
**CRIT-RUST-1 — Variable-Time HMAC Comparison**  
`vault.rs:811-812`: Comment says "Constant-time comparison" but uses standard `==`. Fix: `subtle` crate `ConstantTimeEq`. (See CRIT-01)

### High Findings

| ID | File | Issue | Fix |
|----|------|-------|-----|
| HIGH-RUST-1 | `main_loop.rs` | 7,348-line god file violating SRP | Split into logical modules |
| HIGH-RUST-2 | `Cargo.toml` | `bincode = "2.0.0-rc.3"` RC in production | Upgrade to stable or pin with justification |
| HIGH-RUST-3 | ~8 files | `unsafe impl Send/Sync` with no `// SAFETY:` comments | Add safety invariant documentation |

### Medium Findings

| ID | File | Issue |
|----|------|-------|
| MED-RUST-1 | `llama-sys/lib.rs:919` | `ctx_ptr = 0x2 as *mut LlamaContext` sentinel pointer — fragile, UB risk |
| MED-RUST-2 | `user_profile.rs:309` | `partial_cmp().unwrap()` on `f32` — panics on NaN |
| MED-RUST-3 | `policy/audit.rs` | Hash chain uses SipHash, not SHA-256 (not forensic-grade) |
| MED-RUST-4 | `main_loop.rs` | 30–50 avoidable `String::clone()` in hot event-dispatch paths |
| MED-RUST-5 | `model.rs:303-304` | `path.clone()` and `meta.clone()` called 3× in loop |
| MED-RUST-6 | `aura-types/Cargo.toml` | Pure types crate depends on `async-trait` |

### Positive Findings
- **Zero** `&String` parameters (excellent — `&str` used correctly throughout)
- Principled `classify_task()` always returns `SemanticReact` (by design)
- Poison-safe mutex: `lock().unwrap_or_else(|e| e.into_inner())`
- Real AES-256-GCM + Argon2id cryptography
- `thiserror` + `?` operator error handling throughout

### Metrics

| Metric | Count | Non-Test |
|--------|-------|---------|
| `.unwrap()` | ~925 | ~50 |
| `unsafe` blocks | ~70 | all 70 |
| `.clone()` | ~578 | 50–100 avoidable |
| `&String` | 0 | — |
| Files >800 lines | 8 | — |

---

## 6. §2 — Architecture & System Design

**Grade: B+ (85/100) · Reviewer: Architecture & System Design Specialist**

### Summary
All 6 major architecture claims are confirmed in source code. The bi-cameral design, 11-stage executor, 3-tier planner, 6-layer teacher stack, 4-tier memory, and IPC protocol are all implemented as documented. The primary architectural concern is the 712 panickable `.unwrap()` calls — on Android, a panic = daemon crash with no recovery.

### Architecture Claims — All Confirmed

| Claim | Location | Status |
|-------|----------|--------|
| Bi-cameral: System 1 DGS + System 2 SemanticReact | `react.rs:626` always returns `SemanticReact` | ✅ |
| 11-stage executor pipeline | `executor.rs:575-791` stages 1, 2, 2.5, 2.6, 3–11 | ✅ |
| 3-tier planner: ETG → Template → LLM | `planner.rs`, `MIN_ETG_CONFIDENCE=0.6` | ✅ |
| 6-layer inference teacher stack | All 6 layers in `inference.rs` | ✅ |
| 4-tier memory: Working/Episodic/Semantic/Archive | Full implementation confirmed | ✅ |
| IPC protocol: typed enums, 14+13 variants, 64KB max | `ipc.rs` | ✅ |

### Key Findings

| ID | Severity | Issue |
|----|----------|-------|
| F-01 | HIGH | 712 `.unwrap()` calls — on Android, any panic = daemon crash |
| F-02 | MEDIUM | `main_loop.rs` 2,786 lines; `handle_cron_tick` 202 lines with string-matching dispatch |
| F-03 | MEDIUM | ETG confidence threshold divergence: `System1=0.70` vs `Planner=0.60` |
| F-04 | MEDIUM | OCEAN defaults mismatch: `ipc.rs` O=0.85/C=0.75/E=0.50/A=0.70/N=0.25 vs Ground Truth docs all 0.5 |
| F-05 | LOW | Dual execution paths in `react.rs`: wired `ReactEngine` vs legacy standalone shim |

### Dependency Graph
```
aura-daemon ──depends on──▶ aura-types
aura-neocortex ──depends on──▶ aura-types
aura-neocortex ──depends on──▶ aura-llama-sys
(daemon and neocortex share zero compile-time dependencies — correct isolation)
```

### Operational Limits
- **Single-task sequential processor** — `&mut self` exclusive borrow prevents true concurrency
- **IPC buffer**: 64 messages max; 65th dropped silently (no backpressure)
- **Token budget**: 2048 = 6.25% of 32K model capacity
- **In-flight tasks**: NOT persisted — lost on crash

---

## 7. §3 — Security & Cryptography

**Grade: C+ (67/100) · Reviewer: Security & Cryptography Specialist**

### Summary
Cryptographic primitives are correctly selected and implemented (AES-256-GCM, Argon2id). The PolicyGate architecture is sound. However, four critical defects in the key lifecycle and distribution pipeline create a complete attack chain from network access to vault key extraction. All four criticals are fixable in under 6 engineer-hours.

### Critical Findings

| ID | File | Issue | Fix Effort |
|----|------|-------|-----------|
| CRIT-01 | `vault.rs:811-812` | Non-constant-time HMAC comparison (timing attack) | 30 min — `subtle::ConstantTimeEq` |
| CRIT-02 | `vault.rs:~690` | AES key not zeroed on drop — no `Zeroize` | 1 hr — `#[derive(Zeroize, ZeroizeOnDrop)]` |
| CRIT-03 | `install.sh:39,44,49` | All model SHA256 checksums are placeholders | 2 hr — compute and embed real checksums |
| CRIT-04 | `install.sh:884` | PIN stored as unsalted `sha256sum` | 2 hr — use Argon2id CLI or salted hash |

### Complete Attack Chain
```
MITM during install
  → substitute malicious GGUF [CRIT-03]
  → model executes adversarial tool calls
  → time vault unlock attempts [CRIT-01]
  → recover PIN hash → rainbow table attack [CRIT-04]
  → derive vault key → extract from memory [CRIT-02]
```
Each step is independently exploitable; combined they form a complete key extraction chain.

### High Findings

| ID | Issue |
|----|-------|
| HIGH-SEC-1 | `allow_all_builder()` not `#[cfg(test)]` gated — internal PolicyGate bypass path |
| HIGH-SEC-2 | Checksum failure asks user to confirm — allows bypassing verification |
| HIGH-SEC-3 | `sed -i "s/%%USERNAME%%/$user_name/g"` — shell injection via username |
| HIGH-SEC-4 | NDK (~1GB) downloaded without SHA256 verification |
| HIGH-SEC-5 | No authentication tokens in IPC protocol |
| HIGH-SEC-6 | `curl \| sh` Rust toolchain install |

### Medium Findings

| ID | Issue |
|----|-------|
| MED-SEC-1 | Argon2id `p=4` in code vs `p=1` in documentation |
| MED-SEC-2 | Data tier naming inconsistency (code vs docs use different names) |
| MED-SEC-3 | Trust tier count: code has 5 tiers, all docs show 4 |
| MED-SEC-4 | `absolute_rules` stored as `Vec` — theoretically mutable at runtime |
| MED-SEC-5 | Trust level float injected into every LLM prompt (prompt injection surface) |
| MED-SEC-6 | Ethics rule count: code=11, docs=15, 9 undocumented |
| MED-SEC-7 | No Android Hardware Keystore integration |
| MED-SEC-8 | GGUF metadata parse failure silently falls back to 1024-token context |

### Confirmed Correct
- AES-256-GCM with 12-byte CSPRNG nonce, fresh per encrypt ✅
- Argon2id: 64MB memory, 3 iterations ✅
- Deny-by-default PolicyGate ✅
- `allow_all()` is `#[cfg(test)]` only ✅
- Anti-sycophancy: RING_SIZE=20, BLOCK_THRESHOLD=0.40 ✅
- Tier 3 (Critical) data never returned in search results ✅
- `is_safe_for_llm()` returns false for Tier 2+ ✅
- Each `ContextPackage` built fresh per request ✅

---

## 8. §4 — Performance & Concurrency

**Grade: B (78/100) · Reviewer: Performance & Concurrency Specialist**

### Summary
The worst performance defect is behavioral, not implementational: `classify_task()` always returns `SemanticReact`, permanently disabling the fast DGS path. The most dangerous concurrency issue is the global `EMBEDDING_CACHE Mutex` serializing all threads. Four O(n) Vec shifts in hot paths are easily fixed with `VecDeque`.

### Critical Performance Findings

| ID | File | Issue | Impact |
|----|------|-------|--------|
| C1 | `react.rs` | `NeocortexClient::connect()` called fresh every ReAct iteration | New TCP/socket connection per LLM call |
| C2 | `react.rs` | `classify_task()` hardcoded `SemanticReact` — DGS fast path permanently disabled | Every request goes through full LLM |
| C3 | `context.rs:385,398` | `history.remove(0)` O(n) Vec shift inside truncation loop | O(n²) for 50-turn history |
| C4 | `context.rs` | Full prompt re-assembly + token estimate on every truncation iteration | ~47 full rebuilds for 50-turn history |

### High Performance Findings

| ID | Issue |
|----|-------|
| H1 | `hnsw.rs:600`: `vec![false; self.nodes.len()]` allocated O(n) per `search_layer()` call |
| H2 | `hnsw.rs:661`: Redundant `.sort_by()` after max-heap drain |
| H3 | `embeddings.rs`: Global `Mutex<Option<EmbeddingCache>>` serializes all threads |
| H4 | `embeddings.rs`: O(n) LRU eviction scanning all 1024 entries |
| H5 | `embeddings.rs`: 1536-byte clone per cache hit (384-dim `Vec<f32>`) |
| H6 | `consolidation.rs:543`: 100 sequential `embed()` calls per Deep pass, all hitting global Mutex |
| H7 | `consolidation.rs:569`: k-means uses scalar dot products — no NEON SIMD |
| H8 | `react.rs` + `context.rs`: `Vec::remove(0)` pattern at 4 confirmed sites |

### Memory Budget

| State | RSS | Notes |
|-------|-----|-------|
| Normal operation | 80–130MB | Daemon + embeddings |
| With KV cache | +100–200MB | Inference context |
| Normal total | **180–330MB** | Within 400MB ceiling |
| Peak (Best-of-N 3×) | **300–450MB** | Can breach critical ceiling |
| GGUF via mmap | +4.5GB | Not in RSS but competes for page cache |

### Concurrency Risks

| Risk | Location | Severity |
|------|----------|---------|
| Global embedding Mutex | `embeddings.rs` | HIGH — serializes all threads |
| `block_on()` in async context | `monitor.rs:ping_neocortex()` | HIGH — deadlock risk |
| Dual Mutex held simultaneously | `episodic.rs` (conn + hnsw) | MEDIUM — consistent order but thread pool risk |

### Top 5 Performance Fixes (by ROI)
1. Fix `classify_task()` to enable DGS routing for simple requests
2. Pool IPC connection across ReAct iterations
3. Replace `Vec::remove(0)` with `VecDeque::pop_front()` at 4 sites
4. Replace global embedding Mutex with `Arc<RwLock<_>>` + lock-free miss path
5. Pre-allocate `visited` scratch buffer in HNSW `search_layer()` (pass via `&mut`)

---

## 9. §5 — LLM / AI Integration

**Grade: B+ (80/100) · Reviewer: LLM/AI Integration Specialist**

### Summary
Five of six teacher stack layers are genuinely implemented. The GBNF grammar layer is real but applied post-hoc only. One FFI call has undefined behavior. The 6.25% context utilization represents a significant capability gap.

### Critical Findings

| ID | File | Issue |
|----|------|-------|
| CRIT-05 | `llama-sys/lib.rs:1397` | `tokens.as_ptr() as *mut LlamaToken` — const-to-mut cast is undefined behavior if llama.cpp writes to the buffer |
| CRIT-06 | `inference.rs:368-385` | GBNF grammar applied post-generation only; invalid JSON can be generated and only penalized after wasting full inference compute |

### Teacher Stack Status

| Layer | Name | Status | Notes |
|-------|------|--------|-------|
| 0 | GBNF Grammar | ⚠️ PARTIAL | Post-hoc validation only, not decode-time constraint |
| 1 | Chain-of-Thought | ✅ REAL | `inference.rs:1331-1340` |
| 2 | Logprob Confidence | ✅ REAL | 4-channel Bayesian fusion `inference.rs:1449-1543` |
| 3 | Cascade Retry | ✅ REAL | `infer_with_cascade()` max 3 retries |
| 4 | Cross-model Reflection | ✅ REAL | `maybe_reflect()` switches to Brainstem1_5B |
| 5 | Best-of-N | ✅ REAL | `infer_bon()` BON_SAMPLES=3 |

### High Findings

| ID | Issue |
|----|-------|
| HIGH-LLM-1 | `classify_task()` always returns `SemanticReact` — entire 441-line `RouteClassifier` is dead code |
| HIGH-LLM-2 | `DEFAULT_CONTEXT_BUDGET=2048` uses only 6.25% of 32K context window |
| HIGH-LLM-3 | Dual token tracking: daemon's `TokenBudgetManager` vs neocortex's `TokenTracker` — no synchronization, can drift |
| HIGH-LLM-4 | `model.rs:634-645` — Android `Drop` implementation does not free llama.cpp allocations |
| HIGH-LLM-5 | `lib.rs:1344-1351` — RNG seeded with `SystemTime::now()` nanos (can repeat within same millisecond) |

### Key Numbers

| Parameter | Value | Assessment |
|-----------|-------|-----------|
| Default model | `qwen3-8b-q4_k_m.gguf` | Good choice |
| Context window | 32,768 tokens | Excellent |
| Context budget used | 2,048 tokens | Only 6.25% utilized |
| Max daemon iterations | 10 | |
| Max neocortex iterations | 5 | Mismatch (not synchronized) |
| Worst-case LLM calls | 10 × 5 × 3 = 150 | Per user request |
| BON samples | 3 | Only in Strategist mode |

---

## 10. §6 — Android / Mobile Platform

**Grade: B- (65/100) · Reviewer: Android/Mobile Platform Specialist**

### Summary
The Rust platform layer (power physics model, ISO 13732-1 thermal management, OEM kill-prevention) is production-quality. The Kotlin integration layer has 7 critical defects causing guaranteed crashes on real hardware. The gap between the two layers is the defining characteristic of this domain.

### Critical Findings (7)

| ID | File | Issue | Devices Affected |
|----|------|-------|-----------------|
| CRIT-07 | `AuraForegroundService.kt` | Missing Android 14 `foregroundServiceType` | ~40% (API 34+) |
| CRIT-08 | `AndroidManifest.xml` | Missing `ACCESS_NETWORK_STATE` + `ACCESS_WIFI_STATE` | All devices |
| CRIT-09 | `AuraDaemonBridge.kt` | Sensor listeners never unregistered | All devices (leak) |
| CRIT-10 | `AuraForegroundService.kt` | WakeLock expires at 10 min, never renewed | All devices |
| CRIT-11 | `AuraDaemonBridge.kt` | WakeLock race condition (`@Volatile` insufficient) | Multi-core devices |
| CRIT-12 | `AuraAccessibilityService.kt` | `AccessibilityNodeInfo` not recycled | All devices (pool exhaustion) |
| CRIT-13 | `jni_bridge.rs` | No JNI exception checking after Kotlin callbacks | All devices |

### High Findings (9)

| ID | Issue |
|----|-------|
| HIGH-AND-1 | Battery temperature used as thermal proxy instead of `PowerManager.getCurrentThermalStatus()` |
| HIGH-AND-2 | Deprecated `WifiManager.connectionInfo` (broken on API 31+) |
| HIGH-AND-3 | `Thread.sleep(500)` on accessibility main thread → ANR |
| HIGH-AND-4 | Compound sensor reads across 3 `@Volatile` fields — torn reads |
| HIGH-AND-5 | `build.gradle.kts` lists 3 ABIs but only `arm64-v8a` is built in Cargo |
| HIGH-AND-6 | `nativeShutdown()` called on main thread in `onDestroy()` → ANR if Rust blocks |
| HIGH-AND-7 | CI Android pipeline cannot produce a working APK |
| HIGH-AND-8 | Termux native build takes 10–30 min (thermal throttle risk) |
| HIGH-AND-9 | ~12 `system_api.rs` methods are stubs returning placeholder values |

### Rust Platform Layer Strengths

| Component | Notable Implementation |
|-----------|----------------------|
| `power.rs` | Physics model: 5000mAh × 3.85V × 0.85η; 5-tier degradation with 3% hysteresis |
| `thermal.rs` | ISO 13732-1 skin temp thresholds; PID controller; Newton's law cooling simulation |
| `doze.rs` | OEM kill prevention: Xiaomi/Samsung/Huawei/OPPO/Vivo/OnePlus |
| `sensors.rs` | Power-aware sampling rate adjustment |

---

## 11. §7 — CI/CD & DevOps

**Grade: C (60/100) · Reviewer: CI/CD & DevOps Specialist**

### Summary
The CI pipeline has a fundamental toolchain mismatch: `rust-toolchain.toml` pins `nightly-2026-03-01` but all 3 CI workflows use `dtolnay/rust-toolchain@stable`. Every CI build runs on the wrong toolchain. The release pipeline cannot produce release artifacts. The install script has a confirmed supply-chain attack vector.

### Critical Findings

| ID | File | Issue |
|----|------|-------|
| CRIT-14 | `ci.yml` vs `rust-toolchain.toml` | Toolchain mismatch: CI stable vs project nightly |
| CRIT-15 | `release.yml` | Missing `--features stub` + `submodules: recursive` — every release build fails |
| CRIT-03 | `install.sh` | Placeholder SHA256 checksums (cross-ref Security CRIT-03) |
| CRIT-04 | `install.sh` | Unsalted SHA256 PIN (cross-ref Security CRIT-04) |

### High Findings

| ID | Issue |
|----|-------|
| HIGH-CI-1 | `sed -i` with unsanitized `$user_name` — shell injection |
| HIGH-CI-2 | NDK (~1GB) downloaded without SHA256 check |
| HIGH-CI-3 | `softprops/action-gh-release@v2` not pinned to commit SHA (supply-chain risk) |

### Medium Findings

| ID | Issue |
|----|-------|
| MED-CI-1 | Cache key excludes `Cargo.lock` |
| MED-CI-2 | `aura-neocortex` never tested in CI |
| MED-CI-3 | No `concurrency:` group — stale runs not cancelled |
| MED-CI-4 | `.gitmodules` tracks `branch = master` — non-reproducible builds |
| MED-CI-5 | Missing: `shellcheck`, `cargo-audit`, MSRV enforcement, NDK caching, SBOM generation |

### Fix Checklist (Sprint 0)
- [ ] `ci.yml`: Change `dtolnay/rust-toolchain@stable` → `dtolnay/rust-toolchain@nightly-2026-03-01`
- [ ] `release.yml`: Add `--features stub` to `cargo check`
- [ ] `release.yml`: Add `submodules: recursive` to checkout step
- [ ] `install.sh`: Replace all 3 placeholder checksums with real SHA256 values
- [ ] `install.sh`: Replace unsalted PIN hash with Argon2id

---

## 12. §8 — Test Quality

**Grade: C+ (55/100) · Reviewer: Test Quality Specialist**

### Summary
AURA has 2,376 test functions but approximately 23% (~540) provide genuine regression detection. 45 integration tests assert tautologically. The 2,821-line ReAct engine — the system's primary execution path — has zero tests.

### Critical Findings

| ID | File | Issue |
|----|------|-------|
| CRIT-16 | `integration_tests.rs` (~37KB) | 45 hollow tests with tautological assertions (e.g., `assert!(result.is_ok())` on a function that always returns `Ok`) |
| CRIT-16b | `daemon_core/react.rs` | 2,821-line ReAct engine has **zero** test functions |

### High Findings

| ID | Issue |
|----|-------|
| HIGH-TEST-1 | `planner.score_plan()` hardcoded to return `0.5` — tests passing against stub behavior |
| HIGH-TEST-2 | All executor tests use `Executor::for_testing()`, bypassing PolicyGate — not testing real execution |
| HIGH-TEST-3 | No property-based testing for any cryptographic operations |
| HIGH-TEST-4 | No integration tests for IPC protocol (message encoding, 64KB limit, dropped messages) |

### Coverage Analysis

| Category | Test Functions | Genuinely Detecting Regressions |
|----------|---------------|--------------------------------|
| Unit tests | ~1,800 | ~450 (~25%) |
| Integration tests | ~576 | ~90 (~16%) |
| Hollow tautological | 45 | 0 (0%) |
| **Total** | **~2,376** | **~540 (~23%)** |

### Critical Untested Paths
- ReAct loop (`react.rs` — 2,821 lines)
- PolicyGate decision logic (all tests bypass it)
- Vault encryption/decryption under concurrent access
- IPC message handling under backpressure (64-message limit)
- Memory consolidation ("dreaming") correctness

### Minimum Test Coverage for MVS Gate
Replace 15 of 45 hollow integration tests with tests that assert behavioral correctness. Write at minimum 5 tests for the ReAct engine covering: single-turn success, iteration limit exceeded, tool call failure, and context truncation.

---

## 13. §9 — Documentation vs Code Consistency

**Grade: D+ (45/100) · Reviewer: Docs-vs-Code Consistency Specialist**

### Summary
Documentation consistency is the weakest domain. Five separate sources describe AURA's trust tier system with five incompatible definitions. Ethics rule counts conflict across three independent files. The install documentation claims `bcrypt` while the code uses `Argon2id` and `bcrypt` does not appear in `Cargo.toml`.

### Critical Findings

| ID | Issue | Files Affected |
|----|-------|---------------|
| CRIT-DOC-1 | Trust tiers — 5-way inconsistency: code has 5 tiers, 4 docs each list 4 with different names | `relationship.rs`, 4 doc files |
| CRIT-DOC-2 | Ethics rule count conflict: code has 11, docs claim 15, 9 in code are undocumented | `ethics.rs`, architecture docs |

### High Documentation Findings

| ID | Issue |
|----|-------|
| HIGH-DOC-1 | Install doc claims `bcrypt` for PIN; code uses `Argon2id`; `bcrypt` not in `Cargo.toml` |
| HIGH-DOC-2 | `MAX_REACT_ITERATIONS` mismatch: daemon says 10, neocortex caps at 5 |
| HIGH-DOC-3 | OCEAN personality defaults: `ipc.rs` has O=0.85/C=0.75, docs say all 0.5 |
| HIGH-DOC-4 | ETG confidence threshold: `System1=0.70` vs `Planner=0.60` |
| HIGH-DOC-5 | Argon2id `p=4` in code vs `p=1` in docs |
| HIGH-DOC-6 | Phantom `aura-gguf` crate in architecture docs — does not exist in workspace |

### Trust Tier Inconsistency Detail

| Source | Tiers |
|--------|-------|
| `relationship.rs` (code) | Stranger / Acquaintance / Friend / CloseFriend / Soulmate (5) |
| Architecture docs | STRANGER / ACQUAINTANCE / TRUSTED / INTIMATE (4) |
| Security docs | STRANGER / ACQUAINTANCE / TRUSTED / INTIMATE (4) |
| User docs | STRANGER / ACQUAINTANCE / TRUSTED / INTIMATE (4) |
| API docs | STRANGER / ACQUAINTANCE / TRUSTED / INTIMATE (4) |

The code has an extra tier (`CloseFriend`) with no documented permission boundaries.

---

## 14. §10 — Cross-Domain Synthesis

### Three Root Causes

All 16 critical and 30 high findings trace to one of three root causes:

**Root Cause 1: Prototype Code Shipped as Production**
- `score_plan()` hardcoded to return `0.5`
- `classify_task()` always returns `SemanticReact` (disables entire fast path)
- ~12 `system_api.rs` stubs returning placeholders
- GBNF applied post-hoc only
- Placeholder SHA256 checksums

**Root Cause 2: Single-Developer Consistency Drift**
- Trust tier naming diverged across 5 sources
- Ethics rule counts diverged across 3 sources
- OCEAN defaults diverged
- Battery threshold diverged between two Rust files
- Token budget tracking diverged between two subsystems

**Root Cause 3: Missing Safety Infrastructure**
- No `zeroize` for key material
- No `subtle` for constant-time comparison
- No JNI exception checking
- No `#[cfg(test)]` gate on `allow_all_builder()`
- No test coverage for the primary execution path

### Three Critical Attack Chains

**Chain A — Key Extraction (Security)**
```
Timing attack [CRIT-01] → No zeroize [CRIT-02] → FFI UB [CRIT-05] → Zero tests → Undetectable
```

**Chain B — Installation Compromise (CI/Security)**
```
Broken CI [CRIT-14/15] → Placeholder checksums [CRIT-03] → Rainbow-table PIN [CRIT-04] → allow_all_builder → Full device
```

**Chain C — Trust Boundary Erosion (Docs/Security)**
```
Trust tier drift [CRIT-DOC-1] → Rule gaps [CRIT-DOC-2] → Mutable absolute_rules Vec → Trust float in LLM → Feedback loop
```

### Cross-Domain Dependencies

| Finding A | Finding B | Combined Risk |
|-----------|-----------|--------------|
| CRIT-03 (no checksums) | CRIT-05 (FFI UB) | Malicious model triggers UB |
| HIGH-AND-9 (stubs) | CRIT-16 (hollow tests) | System under test is neither real nor tested |
| MED-SEC-5 (trust float in prompt) | CRIT-DOC-1 (undocumented tier) | Undocumented capabilities exploitable via trust manipulation |

---

## 15. §11 — Prioritized Remediation Plan

### Sprint 0: Critical Fixes (1–2 Days, ~33 Engineer-Hours)
**Gate: Must complete before any public distribution**

#### Security (6 hr)
- [ ] `vault.rs:811-812`: Replace `==` with `subtle::ConstantTimeEq` *(30 min)*
- [ ] `vault.rs`: Add `#[derive(Zeroize, ZeroizeOnDrop)]` to `VaultKey` *(1 hr)*
- [ ] `install.sh`: Replace all 3 GGUF placeholder checksums with real SHA256 values *(2 hr)*
- [ ] `install.sh:884`: Replace unsalted PIN hash with Argon2id *(2 hr)*

#### Android (5 hr)
- [ ] `AndroidManifest.xml`: Add `ACCESS_NETWORK_STATE` + `ACCESS_WIFI_STATE` *(15 min)*
- [ ] `AuraForegroundService.kt`: Add `foregroundServiceType` for API 34+ *(2 hr)*
- [ ] `AuraDaemonBridge.kt`: Add `sensorManager.unregisterListener(this)` in cleanup *(30 min)*
- [ ] `AuraForegroundService.kt`: Fix WakeLock — acquire with no timeout or renew in heartbeat *(1 hr)*

#### CI/CD (2 hr)
- [ ] `ci.yml`: Change to `dtolnay/rust-toolchain@nightly-2026-03-01` *(30 min)*
- [ ] `release.yml`: Add `--features stub` + `submodules: recursive` *(1 hr)*

#### LLM/FFI (4 hr)
- [ ] `llama-sys/lib.rs:1397`: Fix const-to-mut cast UB *(2 hr)*

#### Tests (8 hr)
- [ ] Replace 15 of 45 hollow integration tests with behavioral assertions *(6 hr)*
- [ ] Write 5 minimum ReAct engine tests *(2 hr)*

#### Documentation (8 hr)
- [ ] Reconcile trust tier naming (pick code's 5-tier or docs' 4-tier, update everywhere) *(4 hr)*
- [ ] Reconcile ethics rule count (audit code vs docs, align) *(4 hr)*

**Sprint 0 Definition of Done:**
- CI green on `nightly-2026-03-01`
- Real checksums in `install.sh`
- `subtle::ConstantTimeEq` in vault
- `Zeroize` on key material
- No FFI const-to-mut UB
- Android starts without crash on API 34+
- Foreground service permissions declared
- 15+ hollow tests replaced
- Trust tier definition consistent across all files
- `KNOWN-ISSUES.md` published

### Sprint 1: High Findings (3–5 Days, ~40 Engineer-Hours)

**Security:**
- `gate.rs`: Add `#[cfg(test)]` to `allow_all_builder()`
- `install.sh`: Remove checksum-bypass confirmation prompt
- `install.sh`: Fix shell injection in `sed` substitution
- `install.sh`: Add SHA256 verification for NDK archive
- `ipc.rs`: Add session authentication token to IPC handshake

**Android:**
- `jni_bridge.rs`: Add `env.exception_check()` after every Kotlin callback
- `AuraDaemonBridge.kt`: Fix WakeLock race condition with `@Synchronized`
- `AuraAccessibilityService.kt`: Fix node recycling in `findNodeByContentDesc`
- `AuraDaemonBridge.kt`: Fix thermal API (use `PowerManager.getCurrentThermalStatus()`)
- `AuraDaemonBridge.kt`: Fix WiFi RSSI (use `NetworkCapabilities` API)
- `AuraAccessibilityService.kt`: Move `waitForElement` to coroutine

**Performance:**
- `react.rs`: Pool IPC connection across ReAct iterations
- `context.rs`: Replace `Vec::remove(0)` with `VecDeque::pop_front()` (4 sites)

**Rust:**
- `main_loop.rs`: Add `// SAFETY:` comments to all 8 `unsafe impl Send/Sync` blocks
- Upgrade `bincode` from `2.0.0-rc.3` to stable release

### Sprint 2: Medium Findings (1–2 Weeks, ~77 Engineer-Hours)

- Test coverage: grow from ~23% to ~60% effective coverage
- Fix `score_plan()` stub (hardcoded 0.5)
- Enable DGS routing in `classify_task()`
- Fix GBNF to apply at decode time
- Split `main_loop.rs` 7,348-line god file
- Reconcile all documentation inconsistencies (all HIGH-DOC items)
- Fix `MED-AND-*` findings (battery threshold, notification channel, proguard, etc.)

### Sprint 3: Excellence (2–4 Weeks, ~132 Engineer-Hours)

- Test coverage: grow to ~80%
- Hardware Keystore integration
- Extension system: sandboxing, API versioning, documentation
- Pre-built binary distribution (eliminate 30-min Termux build)
- Full SBOM generation
- Fuzz testing for vault, IPC protocol, GGUF parser
- Streaming inference response

### Total Effort Estimate

| Sprint | Duration | Engineer-Hours | Outcome |
|--------|----------|---------------|---------|
| Sprint 0 | 1–2 days | ~33 hr | Minimally viable ship state |
| Sprint 1 | 3–5 days | ~40 hr | Production security baseline |
| Sprint 2 | 1–2 weeks | ~77 hr | 60% test coverage, perf improvements |
| Sprint 3 | 2–4 weeks | ~132 hr | Production-grade quality |
| **Total** | **5–7 weeks** | **~282 hr** | **Production-grade** |

---

## 16. §12 — Appendices

### Appendix A: File Risk Register

| File | Lines | Risk Level | Primary Concern |
|------|-------|-----------|----------------|
| `vault.rs` | 1,722 | 🔴 CRITICAL | Timing attack + no zeroize |
| `install.sh` | 1,004 | 🔴 CRITICAL | Placeholder checksums + unsalted PIN |
| `llama-sys/lib.rs` | 1,590 | 🔴 CRITICAL | FFI const-to-mut UB |
| `jni_bridge.rs` | 1,627 | 🔴 CRITICAL | No JNI exception checking |
| `AuraForegroundService.kt` | 171 | 🔴 CRITICAL | WakeLock expires + API 34 crash |
| `AndroidManifest.xml` | 59 | 🔴 CRITICAL | Missing permissions |
| `react.rs` | 2,867 | 🟠 HIGH | Zero tests + IPC reconnect per call |
| `main_loop.rs` | 7,348 | 🟠 HIGH | God file + 712 unwraps |
| `inference.rs` | 2,286 | 🟠 HIGH | GBNF post-hoc + 6.25% context use |
| `integration_tests.rs` | ~37KB | 🟠 HIGH | 45 hollow tests |
| `context.rs` | 1,561 | 🟠 HIGH | O(n²) truncation |
| `embeddings.rs` | 948 | 🟡 MEDIUM | Global Mutex + O(n) eviction |
| `executor.rs` | 1,539 | 🟡 MEDIUM | Tests bypass PolicyGate |
| `ethics.rs` | 1,109 | 🟡 MEDIUM | 11 vs 15 rules inconsistency |
| `relationship.rs` | 485 | 🟡 MEDIUM | 5 vs 4 tier inconsistency |

### Appendix B: Confirmed Architecture Properties

The following architecture properties are verified correct in source code and should be preserved in all refactoring:

1. **Bi-cameral routing** — System 1 / System 2 design is sound even though the routing is currently hardwired
2. **11-stage executor** — All stages present and wired correctly
3. **Deny-by-default PolicyGate** — Correct implementation, must not be weakened
4. **4-tier memory with confidentiality enforcement** — Critical: Tier 3 data correctly excluded from LLM context
5. **Fresh ContextPackage per request** — No cross-conversation data leakage confirmed
6. **AES-256-GCM implementation** — Correct (nonce generation, AAD, authentication tag)
7. **Argon2id implementation** — Correct primitives (fix zeroize and salt alignment with docs)
8. **Anti-sycophancy system** — Rare feature, correctly implemented, preserve

### Appendix C: Deferred Findings (Post-Sprint 3)

Items noted but deferred as outside the production-gate scope:

- Multi-user support (currently single-user only by design)
- Encrypted SQLite (using plaintext SQLite with vault-layer encryption)
- Remote attestation for Termux distribution
- Formal verification of PolicyGate rules
- Hardware security module (HSM) integration beyond Keystore

### Appendix D: External Review Reconciliation

An external minimal-depth review identified the following additional findings not covered in the 9-domain review:

| External Finding | Assessment | Action |
|-----------------|-----------|--------|
| Codebase fails to compile (missing `tokens_used` field, TcpStream/UnixStream mismatch) | Plausible — type changes may have diverged; CI toolchain mismatch makes this undetectable | Verify compilation in Sprint 0 CI fix |
| No Android Hardware Keystore | Confirmed — tracked as MED-SEC-7 | Sprint 3 |
| Memory consolidation LLM hallucination drift | Valid architectural concern | Add to Sprint 2 test coverage |
| GGUF metadata failure falls back to 1024 context | Confirmed — tracked as MED-SEC-8 | Fix in Sprint 1 |

### Appendix E: Glossary

| Term | Definition |
|------|-----------|
| DGS | Deterministic Grammar System — System 1 fast path for pattern-matched requests |
| SemanticReact | System 2 LLM-based ReAct loop for complex reasoning |
| ETG | Execution Template Graph — first tier of 3-tier planner |
| OCEAN+VAD | Personality model: Openness/Conscientiousness/Extraversion/Agreeableness/Neuroticism + Valence/Arousal/Dominance |
| BON | Best-of-N — Layer 5 of teacher stack, samples 3 responses |
| GBNF | GGUF BNF — grammar format for constrained LLM output |
| HNSW | Hierarchical Navigable Small World — approximate nearest-neighbor index for episodic memory |
| PolicyGate | Deny-by-default capability enforcement layer in Stage 2.5 of executor |
| ContextPackage | Per-request IPC payload from daemon to neocortex (max 64KB) |
| MVS Gate | Minimum Viable Ship gate — Sprint 0 completion criteria |

---

*Document generated from 9-domain enterprise code review of AURA v4.*  
*Document Control ID: AURA-v4-ECR-2026-001 · v1.0 · 2026-03-14*  
*Classification: Internal — Engineering*
