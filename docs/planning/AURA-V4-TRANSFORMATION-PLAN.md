# AURA v4 Transformation Plan

**Document**: `docs/planning/AURA-V4-TRANSFORMATION-PLAN.md`  
**Purpose**: Complete transformation roadmap from alpha chaos to production platform  
**Scope**: v4.0.0-stable release  
**Status**: FOUNDATIONAL — This plan is MANDATORY for all work  
**Created**: 2026-03-22  
**Owner**: Architecture Charter + DevOps Release Charter  

---

## Executive Summary

**Current State**: AURA v4 has 8 failed alpha releases. CI passes green. Device crashes. Architecture is decaying (F015). curl_backend design is broken (F006). No device testing in CI (F012). Release governance nonexistent (F013). The codebase is untestable and unmaintainable.

**Target State**: AURA v4.0.0-stable is a production-grade platform. CI validates builds AND runtime behavior on device. Every release follows the contract defined in `CONTRACT.md`. Every failure is classified against `FAILURE_TAXONOMY.md`. The system prevents bugs, not just detects them.

**Transformation Approach**: Build the FOUNDATION first (CONTRACT, TAXONOMY, OPERATING-PROCEDURES). Then fix the BUILD SYSTEM (CI Pipeline v2, device testing). Then fix the CODEBASE (curl_backend redesign, boot stages). Then validate everything. Then ship.

**This is NOT a feature sprint. This is a foundation sprint.**

---

## Critical Insight: Why Previous Fixes Failed

Every alpha release (4.0.0-alpha.1 through alpha.8) followed the same pattern:

```
Bug discovered → Quick fix → CI passes → Ship → Bug reappears in different form
```

**Root cause**: Fixes addressed SYMPTOMS, not SYSTEMS. Each fix was a patch on a broken foundation. The F001 SIGSEGV was "fixed" in alpha.6 by adding error handling. It reappeared in alpha.7 because the real issue (ABI mismatch) was never addressed.

**The transformation breaks this cycle by:**
1. Building FOUNDATION documents that PREVENT the bugs (CONTRACT, TAXONOMY)
2. Building CI that VALIDATES on device, not just Linux (F012 prevention)
3. Building architecture that is TESTABLE (curl_backend redesign)
4. Building RELEASE GOVERNANCE that enforces quality (F013 prevention)

**If CONTRACT, TAXONOMY, OPERATING-PROCEDURES did not exist before — nothing else matters.**

---

## Phase 0: Foundation (COMPLETED ✅)

These documents are the PREREQUISITE for all other work. No code is changed until these exist.

| Document | Purpose | Status |
|----------|---------|--------|
| `CONTRACT.md` | Platform contract — what v4.0.0-stable promises | ✅ DONE |
| `FAILURE_TAXONOMY.md` | F001-F015 failure classification | ✅ DONE |
| `OPERATING-PROCEDURES.md` | Team roles, cadence, D1-D6 decision trees | 🔄 IN PROGRESS |
| `CI-PIPELINE-V2-DESIGN.md` | CI architecture with device testing | 📋 PLANNED |

**Why this matters**: The 47 hard decisions made in sequential thinking were IMPOSSIBLE without CONTRACT and TAXONOMY. Future decisions will reference these documents. They are the constitution; everything else is law.

---

## Phase 1: Build System Transformation (Day 2)

**Goal**: CI validates BUILD AND RUNTIME, not just build.

### CI Pipeline v2 Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        CI PIPELINE v2                            │
├─────────────────────────────────────────────────────────────────┤
│  STAGE 1: SOURCE                                                  │
│  └─ Checkout, dependency cache, environment detection           │
│                                                                   │
│  STAGE 2: BUILD                                                   │
│  └─ cargo build --all-features --target aarch64-linux-android   │
│                                                                   │
│  STAGE 3: INSPECT                                                 │
│  └─ file aura-daemon (verify architecture)                       │
│  └─ readelf -d aura-daemon (verify dynamic linker)                │
│  └─ ls -la aura-daemon (verify size, permissions)                │
│                                                                   │
│  STAGE 4: TEST (Linux host)                                      │
│  └─ cargo test --workspace                                        │
│  └─ cargo clippy                                                  │
│  └─ cargo audit                                                    │
│                                                                   │
│  STAGE 5: DEVICE DEPLOY (Cross-compile artifact)                 │
│  └─ Upload android-artifact to GitHub Actions cache              │
│  └─ Download to Termux device via curl                           │
│                                                                   │
│  STAGE 6: DEVICE VALIDATE (The REAL test)                        │
│  └─ Execute binary on actual Android device                      │
│  └─ Check exit code (0 = pass)                                   │
│  └─ Capture boot stage logs                                       │
│  └─ Report: BUILD=green, DEVICE=green, READY FOR RELEASE         │
└─────────────────────────────────────────────────────────────────┘
```

**Key Principle**: STAGE 4 (Linux tests) ≠ VALIDATION. STAGE 6 (device execution) = VALIDATION.

**F001 Prevention**: If binary cannot be built for Android target, pipeline fails at STAGE 2. If binary builds but crashes on device, pipeline fails at STAGE 6. CI NEVER passes with device crash.

**F012 Prevention**: Device testing is MANDATORY. No release proceeds without STAGE 6 green.

### CI Workflow Files

| File | Purpose | Status |
|------|---------|--------|
| `.github/workflows/ci.yml` | Stages 1-4 (Linux) | Needs v2 redesign |
| `.github/workflows/build-android.yml` | Stage 2 (cross-compile) | Needs v2 redesign |
| `.github/workflows/device-validate.yml` | Stages 5-6 (device) | NEW — not yet created |

**Deliverable**: All CI workflows passing. STAGE 6 green on Termux device.

---

## Phase 2: Codebase Architecture Transformation (Day 3)

**Goal**: curl_backend is redesigned, testable, and maintainable.

### curl_backend Sync-Only Redesign

**Problem (F006)**: Current feature-gated HttpBackend trait is broken:
- `#[cfg]` on use statements (invalid Rust)
- `async_trait` proc-macro issues when features switch
- Sized trait issues with async return types
- Trait object safety violations

**Solution (Option A — Chosen)**: Sync-only trait design.

```rust
// Simple, synchronous trait — both backends implement this
pub trait HttpBackend: Send + Sync {
    fn get(&self, url: &str) -> Result<Response, HttpError>;
    fn post(&self, url: &str, body: &[u8]) -> Result<Response, HttpError>;
}

// reqwest backend: wraps in spawn_blocking for async contexts
pub struct ReqwestBackend { ... }

impl HttpBackend for ReqwestBackend {
    fn get(&self, url: &str) -> Result<Response, HttpError> {
        // Sync call, no async_trait needed
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async { self.client.get(url).send().await })
    }
}

// curl backend: uses tokio::process::Command (already async)
pub struct CurlBackend { ... }

impl HttpBackend for CurlBackend {
    fn get(&self, url: &str) -> Result<Response, HttpError> {
        // Uses tokio::process::Command which is async-native
        // but exposed through sync interface
        self.execute_curl(url)
    }
}
```

**Why Sync-Only Works**:
1. Telegram API is inherently synchronous (long-polling, not streaming)
2. No need for async trait complexity
3. reqwest already uses blocking internally — we just make it explicit
4. curl backend already async via tokio::process::Command
5. Both backends can be swapped without changing calling code
6. No feature gate conflicts — both features compile independently

**F006 Prevention**: Feature boundaries = module boundaries, not cfg flags.

### Cargo Workspace Redesign

**Problem**: Current workspace structure causes unnecessary dependencies.

**Solution**: Explicit dependency management per binary.

```
aura/
├── Cargo.toml              # Workspace root
├── aura-daemon/
│   ├── Cargo.toml          # Depends only on needed crates
│   └── src/main.rs
├── aura-core/              # Core logic, no backend dependency
│   ├── Cargo.toml
│   └── src/lib.rs
├── aura-telegram/          # Telegram protocol, no HTTP dependency
│   ├── Cargo.toml
│   └── src/lib.rs
└── aura-http/              # HTTP abstraction layer (NEW)
    ├── Cargo.toml
    ├── src/lib.rs           # HttpBackend trait
    ├── src/reqwest_backend.rs
    └── src/curl_backend.rs
```

**Benefits**:
- `aura-core` can be tested without any HTTP backend
- `aura-telegram` can be unit tested with mock backend
- `aura-daemon` chooses backend at compile time
- No circular dependencies

**Deliverable**: `cargo build --workspace --all-features` succeeds. curl_backend works on both backends.

---

## Phase 3: Runtime Platform (Day 4)

**Goal**: Binary has 5 boot stages with logging, failure classification, and observability.

### Boot Stage Implementation

```rust
fn main() {
    // STAGE 1: Pre-flight checks
    log::info!("[BOOT:1/5] Pre-flight: Environment");
    validate_environment();
    
    // STAGE 2: Config loading
    log::info!("[BOOT:2/5] Config: Loading configuration");
    let config = load_config()?;
    
    // STAGE 3: HTTP backend initialization
    log::info!("[BOOT:3/5] HTTP: Initializing backend");
    let http_backend = init_http_backend(&config)?;
    
    // STAGE 4: Telegram session setup
    log::info!("[BOOT:4/5] Telegram: Connecting to MTProto");
    let telegram = init_telegram(&config, http_backend)?;
    
    // STAGE 5: Main event loop
    log::info!("[BOOT:5/5] Ready: Entering main loop");
    main_loop(telegram)?;
}
```

**F007 Prevention**: If any boot stage fails, log shows exactly which stage failed and what was expected.

### Privacy-Preserving Observability

**Problem**: We need to know when failures occur, but we cannot collect PII.

**Solution**: Local crash dumps only, no network telemetry.

```
/sdcard/AURA/
├── logs/
│   ├── aura-boot-2026-03-22.log
│   └── aura-crash-2026-03-22-143022.log
└── dumps/
    └── crash-2026-03-22-143022.tar.gz  # Backtrace + config (no PII)
```

**Deliverable**: Binary runs 5 boot stages. Logs written to /sdcard/AURA/logs/. Crash dumps on failure.

---

## Phase 4: Documentation (Day 5)

**Goal**: All 7 required docs exist, tested, and maintained.

### 7 Required Documents

| # | Document | Purpose | Status |
|---|----------|---------|--------|
| 1 | `architecture/overview.md` | System architecture | 📋 PLANNED |
| 2 | `build/contract.md` | Build contract | ✅ DONE |
| 3 | `runtime/boot-stages.md` | Boot stage documentation | 📋 PLANNED |
| 4 | `validation/device-matrix.md` | Test matrix OS×RAM×Vendor | 📋 PLANNED |
| 5 | `release/rollback.md` | Rollback procedures | 📋 PLANNED |
| 6 | `failure-db/signatures.md` | Failure code database | ✅ DONE (TAXONOMY) |
| 7 | `incident/postmortems.md` | Post-incident reviews | 📋 PLANNED |

**Deliverable**: All 7 docs exist with current content.

---

## Phase 5: Release Gate (Day 5)

**Goal**: Release follows mandatory gates, CI ≠ release signal.

### Release Gate Checklist

```
┌─────────────────────────────────────────────────────────────────┐
│                    RELEASE GATE CHECKLIST                        │
├─────────────────────────────────────────────────────────────────┤
│ □ BUILD: cargo build --all-features succeeds                    │
│ □ INSPECT: file shows aarch64, readelf shows android linker    │
│ □ TEST: cargo test --workspace passes                          │
│ □ LINT: cargo clippy passes, cargo audit clean                 │
│ □ DEVICE: Binary executes on Termux device, exit code 0         │
│ □ BOOT: All 5 boot stages log successfully                      │
│ □ REGRESSION: Previous failures (F001-F015) verified fixed      │
│ □ CONTRACT: All contract terms verified                        │
│ □ DOCS: All 7 required docs updated                            │
│ □ RISK: Risk register updated, no new risks introduced          │
│ □ APPROVAL: Architecture + DevOps charters approve             │
└─────────────────────────────────────────────────────────────────┘
```

**Key Principle**: CI green = BUILD gate passed. Device test green = VALIDATION gate passed. Both required for release.

**F013 Prevention**: If any gate check fails, release is BLOCKED. No exceptions.

---

## 5-Day Sprint Breakdown

### Day 1: Foundation (COMPLETED ✅)
- [x] CONTRACT.md written
- [x] FAILURE_TAXONOMY.md written
- [x] All 47 hard decisions documented
- [x] Skills loaded and ready

### Day 2: CI Pipeline v2
- [ ] Design CI Pipeline v2 architecture
- [ ] Implement `.github/workflows/device-validate.yml`
- [ ] Update `.github/workflows/ci.yml` for v2
- [ ] Add STAGE 6 (device validation) to pipeline
- [ ] Verify pipeline runs on GitHub Actions
- [ ] **Checkpoint**: Pipeline green on Linux, device test stage exists

### Day 3: curl_backend Redesign
- [ ] Create `aura-http/` crate with sync-only HttpBackend trait
- [ ] Implement `ReqwestBackend` wrapping reqwest in spawn_blocking
- [ ] Implement `CurlBackend` using tokio::process::Command
- [ ] Remove broken `curl_backend.rs` feature-gated code
- [ ] Verify both backends compile and work
- [ ] **Checkpoint**: `cargo build --all-features` succeeds

### Day 4: Boot Stages + Observability
- [ ] Implement 5 boot stages in `aura-daemon`
- [ ] Add boot stage logging to file
- [ ] Implement privacy-preserving crash dumps
- [ ] Create `/sdcard/AURA/` directory structure
- [ ] Test boot stages on device
- [ ] **Checkpoint**: Binary logs 5 boot stages on device

### Day 5: Documentation + Release Gate
- [ ] Write all 7 required documents
- [ ] Implement `release-gate.sh` script
- [ ] Create device matrix (OS×RAM×Vendor)
- [ ] Document rollback procedures
- [ ] Final validation on device
- [ ] **Checkpoint**: All gates pass, release ready

---

## Subagent Assignment Matrix

| Agent | Day 2 | Day 3 | Day 4 | Day 5 |
|-------|-------|-------|-------|-------|
| **CI Engineer** | CI Pipeline v2 | device-validate.yml | Monitor CI health | Final validation |
| **Rust Engineer** | Review curl_backend design | Implement sync-only trait | Boot stage logging | Code review |
| **Device Tester** | Test device workflow | Test curl_backend on device | Boot stage test | Final device test |
| **Document Writer** | Write CI-PIPELINE-V2-DESIGN.md | Write architecture docs | Write boot-stages.md | Write all 7 docs |

### Agent Handoff Protocol

Every agent returns structured output:

```json
{
  "status": "ok" | "partial" | "error",
  "skill_loaded": ["skillA"],
  "result_summary": "1-2 sentence summary",
  "artifacts": ["path/to/file"],
  "tests_run": {"unit": 1, "integration": 0, "passed": 1},
  "token_cost_estimate": 500,
  "time_spent_secs": 60,
  "next_steps": ["action1", "action2"],
  "proposal_for_change": null
}
```

### Skill → Task Mapping

| Task | Primary Skill | Secondary Skill |
|------|--------------|-----------------|
| CI Pipeline v2 | `infrastructure-as-code` | `system-architecture-patterns` |
| curl_backend redesign | `production-grade-coding` | `context-aware-implementation` |
| Boot stage implementation | `test-driven-development` | `systematic-debugging` |
| Documentation | `writing-plans` | `executing-plans` |
| Device testing | `verification-before-completion` | `autonomous-research` |

---

## Success Criteria

### Definition of Done for v4.0.0-stable

| Criterion | Measurement | Target |
|-----------|-------------|--------|
| **Build** | `cargo build --all-features` | ✅ Passes |
| **Lint** | `cargo clippy` | ✅ Zero warnings |
| **Test** | `cargo test --workspace` | ✅ 100% pass |
| **Device** | Binary on Termux device | ✅ Exit code 0 |
| **Boot** | 5 boot stages logged | ✅ All stages pass |
| **Regression** | F001-F015 failures | ✅ Zero regressions |
| **Contract** | CONTRACT.md terms | ✅ 100% compliant |
| **Taxonomy** | FAILURE_TAXONOMY.md | ✅ Up to date |
| **Docs** | 7 required documents | ✅ All exist |
| **Release Gate** | `release-gate.sh` | ✅ All gates pass |

### Transformation Metrics

| Metric | Baseline (alpha.8) | Target (stable) |
|--------|-------------------|------------------|
| CI → Device correlation | 0% (CI green, device red) | 100% (CI green = device green) |
| Failure classification | 0% (symptoms not codes) | 100% (all failures classified) |
| Boot stage logging | 0% (no observability) | 100% (5 stages logged) |
| Required docs | 2/7 (25%) | 7/7 (100%) |
| Architecture decay (F015) | High | None |
| curl_backend reliability | Broken (F006) | Working (both backends) |

---

## Risk Register

| Risk | Impact | Probability | Mitigation | Owner |
|------|--------|-------------|------------|-------|
| Device CI fails due to Termux limitations | HIGH | MEDIUM | Use Termux CI (Jorin's approach) or fallback to binary inspection | CI Engineer |
| curl_backend redesign reveals deeper issues | MEDIUM | LOW | Timebox to 4 hours, escalate if not resolved | Rust Engineer |
| Boot stage implementation slows other work | LOW | LOW | Implement incrementally, test on device each stage | Rust Engineer |
| Documentation rot (docs become outdated) | MEDIUM | MEDIUM | Docs are part of definition of done | Document Writer |
| Feature scope creep (adding to v4.0.0) | HIGH | HIGH | Explicit scope freeze, no additions | Architecture Charter |

---

## Open Questions (TODOs)

| Question | Owner | Due |
|----------|-------|-----|
| How to test on 3x3x3 device matrix (9+ devices)? | Device Tester | Day 4 |
| How to handle custom ROM users? | Architecture Charter | Day 1 |
| What is exact rollback procedure for Termux? | DevOps Release | Day 5 |
| How to validate CI on device without manual steps? | CI Engineer | Day 2 |
| What is exact format for privacy-preserving crash dumps? | Runtime Platform | Day 4 |

---

## Checkpoint System

**Daily Checkpoint** (End of each day):
1. Save current state to `checkpoints/aura-{date}.md`
2. Verify all Phase deliverables for the day
3. If checkpoint fails, STOP and resolve before continuing
4. Update this document with completed items

**Gate Checkpoint** (Before each phase):
1. Verify previous phase deliverables complete
2. If not complete, do NOT proceed to next phase
3. Document blockers before escalating

---

## What This Plan Does NOT Cover

This plan covers FOUNDATION only. v4.0.0-stable scope is LOCKED:

### IN SCOPE (v4.0.0-stable)
- CI Pipeline v2 with device testing
- curl_backend sync-only redesign
- Boot stage logging (5 stages)
- Privacy-preserving observability
- All 7 required documents
- Release gate checklist

### NOT IN SCOPE (Future versions)
- Additional features beyond current alpha.8
- Custom ROM support
- Multiple device testing matrix (future)
- Performance optimization
- Battery life improvements
- UI/UX changes

**Rule**: No scope additions until v4.0.0-stable ships. Every addition delays stable.

---

## Final Statement

> **"A release is not finished when CI is green. A release is finished when the system has proven itself under the contract it claims to support."**

This plan is the path from chaos (8 failed alphas) to stability (v4.0.0-stable). It is not a feature roadmap. It is not a sprint for new capabilities. It is the systematic elimination of failure modes through foundation, process, and architecture.

The work is hard. The deadlines are tight. The standards are uncompromising.

But the alternative is another 8 failed releases.

---

## Document History

| Date | Version | Changes |
|------|---------|---------|
| 2026-03-22 | 1.0 | Initial version — Phase 0 complete |
