# AURA v4 Operating Procedures

**Document**: `docs/operations/AURA-OPERATING-PROCEDURES.md`  
**Purpose**: Operating procedures for AURA v4.0.0-stable development  
**Scope**: All work on AURA codebase and infrastructure  
**Status**: MANDATORY — All work follows these procedures  
**Created**: 2026-03-22  
**Owner**: All Charters  

---

## Core Principle

> **"Build the system that prevents bugs, not the system that detects them."**

Every procedure in this document exists to enforce that principle. If a procedure does not contribute to bug prevention, it should be questioned.

---

## Team Structure (1-2 Person Edition)

### Role → Person Mapping

For a 1-2 person team, roles overlap. Here is the explicit mapping:

| Role | Primary | Secondary | Responsibilities |
|------|---------|-----------|------------------|
| **Architecture Charter** | You | — | System design, technical decisions, F015 prevention |
| **Build Infrastructure** | You | — | CI/CD, build systems, artifact management |
| **Runtime Platform** | You | — | Device execution, boot stages, observability |
| **QA Validation** | You | — | Testing strategy, device matrix, failure classification |
| **DevOps Release** | You | — | Release gates, rollback, governance |
| **Product Charter** | You | — | Scope control, v4.0.0-stable freeze |
| **Forensics** | You | — | Post-incident analysis, taxonomy updates |

**Key Insight**: In a 1-2 person team, ONE PERSON plays all roles. This is intentional. It means:
- You see the full system, not just your slice
- You make decisions with full context
- You are responsible for the entire stack

**Risk**: Cognitive overload. Mitigate with:
- Strict scope freeze (no additions to v4.0.0-stable)
- Checkpoint system (save state daily)
- Explicit timeboxing (don't work on two things at once)

---

## Weekly Cadence

### 5-Day Work Cycle

Each day has a structured purpose. Deviation requires explicit justification.

| Day | Focus | Primary Deliverable | Secondary |
|-----|-------|---------------------| ----------|
| **Monday** | Analysis + Planning | Review issue backlog, prioritize | Update risk register |
| **Tuesday** | Build System | CI Pipeline work | Device testing setup |
| **Wednesday** | Codebase | curl_backend/architecture work | Unit tests |
| **Thursday** | Runtime + Device | Boot stages, device testing | Observability |
| **Friday** | Documentation + Release | All 7 docs, release gate | Retrospective |

### Day Structure

#### Morning (First 30 minutes) — Checkpoint + Planning
```
1. Check CI runs from previous day
2. Review ISSUE-LOG.md for new failures
3. Update checkpoint file with yesterday's progress
4. Identify today's primary deliverable
5. Check for blockers from yesterday
```

#### Core Work (Next 6 hours) — Execution
```
1. Work on primary deliverable only
2. Follow decision trees when facing choices
3. Reference CONTRACT.md and FAILURE_TAXONOMY.md
4. Do not context-switch without reason
5. Checkpoint every 2 hours
```

#### Evening (Last 30 minutes) — Handoff
```
1. Update checkpoint file with today's progress
2. Document any new failures (F-codes)
3. Identify tomorrow's primary deliverable
4. Review CI status before shutdown
5. Save all work
```

---

## Decision Trees (D1-D6)

These decision trees MUST be followed for the corresponding decisions. They are MANDATORY.

### D1: Build Decision — Should I Build?

```
┌─────────────────────────────────────────────────┐
│                 D1: BUILD DECISION               │
├─────────────────────────────────────────────────┤
│                                                  │
│  Is this a new feature or bug fix?              │
│       │                                          │
│       ├──► Bug Fix ──► Continue                  │
│       │                                          │
│       └──► New Feature ──► Is v4.0.0-stable     │
│                              scope frozen?      │
│                                   │              │
│                       ┌───────────┴───────────┐ │
│                       │                       │ │
│                      YES                       NO│
│                       │                       │ │
│                       ▼                       ▼ │
│               STOP — NOT IN SCOPE        Continue│
│                                                  │
└─────────────────────────────────────────────────┘
```

**D1 Summary**: New features are STOPPED until v4.0.0-stable ships. Scope freeze is MANDATORY.

---

### D2: Structural Decision — Is This a Layer Problem?

```
┌─────────────────────────────────────────────────┐
│              D2: STRUCTURAL DECISION             │
├─────────────────────────────────────────────────┤
│                                                  │
│  Is failure in BUILD or RUNTIME?                │
│       │                                          │
│       ├──► BUILD                                 │
│       │     │                                    │
│       │     ├──► Artifact missing? ────► F002   │
│       │     ├──► Dependency conflict? ──► F003  │
│       │     ├──► ABI mismatch? ─────────► F004   │
│       │     ├──► Linker error? ─────────► F005  │
│       │     └──► Feature conflict? ──────► F006  │
│       │                                          │
│       └──► RUNTIME                              │
│             │                                    │
│             ├──► SIGSEGV on device? ───► F001   │
│             ├──► Crash before boot? ─────► F007  │
│             ├──► Install corruption? ─────► F008 │
│             ├──► Toolchain fails? ────────► F009 │
│             └──► Environment mismatch? ──► F010 │
│                                                  │
│  Is failure in OBSERVABILITY?                   │
│       │                                          │
│       ├──► No logs on failure? ─────────► F011  │
│       └──► Tests pass but fails? ────────► F012 │
│                                                  │
│  Is failure in PROCESS?                         │
│       │                                          │
│       ├──► Released without device test? ─► F013│
│       ├──► Bug came back after fix? ──────► F014 │
│       └──► Architecture causing bugs? ────► F015│
│                                                  │
└─────────────────────────────────────────────────┘
```

**D2 Summary**: Classify the failure FIRST. If you don't know the F-code, you don't know the root cause.

---

### D3: Device Decision — Is This a Device Issue?

```
┌─────────────────────────────────────────────────┐
│              D3: DEVICE DECISION                 │
├─────────────────────────────────────────────────┤
│                                                  │
│  Did failure occur on DEVICE?                   │
│       │                                          │
│       ├──► YES ──► Is it a CODE issue?          │
│       │             │                           │
│       │             ├──► YES ──► Code fix        │
│       │             │      (see D2)             │
│       │             │                           │
│       │             └──► NO ──► Is it DEVICE    │
│       │                  SETUP issue?           │
│       │                       │                 │
│       │              ┌─────────┴─────────┐       │
│       │              │                   │       │
│       │             YES                  NO     │
│       │              │                   │       │
│       │              ▼                   ▼       │
│       │        Device fix ONLY    Escalate to   │
│       │        (rustup, rust,      Architecture │
│       │         permissions)       Charter      │
│       │                                          │
│       └──► NO ──► Is CI showing BUILD           │
│             │     failure?                       │
│             │                                    │
│             ├──► YES ──► Build Infrastructure    │
│             │      Charter owns                 │
│             │                                    │
│             └──► NO ──► Unknown ──► D2          │
│                                                  │
└─────────────────────────────────────────────────┘
```

**D3 Summary**: Device issues (rustup, permissions, paths) are DEVICE SETUP issues, NOT CODE issues. Do NOT modify AURA code for device setup problems.

---

### D4: Boot Decision — Is Boot Complete?

```
┌─────────────────────────────────────────────────┐
│              D4: BOOT DECISION                  │
├─────────────────────────────────────────────────┤
│                                                  │
│  Did binary start?                               │
│       │                                          │
│       ├──► NO ──► Check exit code                │
│       │     │                                   │
│       │     ├──► SIGSEGV ──────────────► F001   │
│       │     ├──► Exit code != 0 ────────► F007  │
│       │     └──► Exit code 0 ──────────► Check  │
│       │                              logs        │
│       │                                          │
│       └──► YES ──► Is BOOT COMPLETE?           │
│             │                                   │
│             ├──► BOOT STAGE 1 ──► Check env     │
│             ├──► BOOT STAGE 2 ──► Check config  │
│             ├──► BOOT STAGE 3 ──► Check HTTP    │
│             ├──► BOOT STAGE 4 ──► Check Telegram│
│             └──► BOOT STAGE 5 ──► Ready         │
│                                                  │
│  Any BOOT stage failure?                         │
│       │                                          │
│       ├──► YES ──► Log shows which stage        │
│       │      (see F007)                         │
│       │                                          │
│       └──► NO ──► Binary running                 │
│                                                  │
└─────────────────────────────────────────────────┘
```

**D4 Summary**: Boot stages MUST be logged. If a stage fails, the log shows exactly which stage and what was expected.

---

### D5: Failure Decision — Is This a Known Failure?

```
┌─────────────────────────────────────────────────┐
│             D5: FAILURE DECISION                │
├─────────────────────────────────────────────────┤
│                                                  │
│  Is failure in FAILURE_TAXONOMY.md?             │
│       │                                          │
│       ├──► YES ──► Apply F-code                  │
│       │     │                                   │
│       │     ├──► Check Prevention column        │
│       │     ├──► Apply Resolution column        │
│       │     ├──► Add test that catches it       │
│       │     └──► Update taxonomy if needed      │
│       │                                          │
│       └──► NO ──► Is this a NEW failure?        │
│             │                                   │
│             ├──► YES ──► Create new F-code      │
│             │      (F016, F017, etc.)           │
│             │      ├──► Document in ISSUE-LOG   │
│             │      ├──► Add to TAXONOMY         │
│             │      ├──► Identify prevention     │
│             │      └──► Add test                │
│             │                                    │
│             └──► NO ──► Unknown failure         │
│                  ──► Escalate to Architecture   │
│                                                  │
└─────────────────────────────────────────────────┘
```

**D5 Summary**: If you encounter a failure not in the taxonomy, STOP and classify it before fixing. The taxonomy must be complete.

---

### D6: Release Decision — Should I Release?

```
┌─────────────────────────────────────────────────┐
│             D6: RELEASE DECISION               │
├─────────────────────────────────────────────────┤
│                                                  │
│  Are ALL release gates passing?                  │
│       │                                          │
│       ├──► BUILD gate                           │
│       │     └──► cargo build succeeds           │
│       │                                          │
│       ├──► INSPECT gate                        │
│       │     └──► file shows aarch64            │
│       │     └──► readelf shows android linker  │
│       │                                          │
│       ├──► TEST gate                           │
│       │     └──► cargo test --workspace passes │
│       │                                          │
│       ├──► LINT gate                           │
│       │     └──► cargo clippy passes           │
│       │     └──► cargo audit clean             │
│       │                                          │
│       ├──► DEVICE gate                        │
│       │     └──► Binary executes on device     │
│       │     └──► Exit code 0                   │
│       │     └──► All boot stages pass          │
│       │                                          │
│       ├──► CONTRACT gate                       │
│       │     └──► All CONTRACT.md terms met     │
│       │                                          │
│       ├──► TAXONOMY gate                       │
│       │     └──► No new failures unclassified │
│       │                                          │
│       └──► DOCS gate                           │
│             └──► All 7 required docs current   │
│                                                  │
│  ANY gate failing?                               │
│       │                                          │
│       ├──► YES ──► BLOCK RELEASE                │
│       │      Do NOT release until all gates    │
│       │      pass. This is MANDATORY.          │
│       │                                          │
│       └──► NO ──► All gates green               │
│             └──► APPROVED FOR RELEASE           │
│                                                  │
└─────────────────────────────────────────────────┘
```

**D6 Summary**: CI green is NOT a release gate. Device testing is a release gate. ALL gates must pass.

---

## Quality Metrics

### Daily Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| **Compilation** | Passes on all feature combinations | `cargo build --all-features` |
| **Tests** | 100% pass | `cargo test --workspace` |
| **Lint** | Zero warnings | `cargo clippy` |
| **Boot** | 5 stages logged | Device execution |

### Weekly Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| **CI Pass Rate** | 100% | GitHub Actions runs |
| **Device Pass Rate** | 100% | Manual device testing |
| **Failure Classification** | 100% | All failures have F-codes |
| **Documentation** | 100% current | All 7 docs updated |

### Transformation Metrics

| Metric | Baseline (alpha.8) | Target (stable) |
|--------|-------------------|-----------------|
| **Build → Device correlation** | 0% | 100% |
| **Failure classification rate** | 0% | 100% |
| **Boot stage coverage** | 0% | 100% |
| **Documentation coverage** | 25% | 100% |

---

## Operating Principles (7 Total)

### P1: System Over Symptom
**Every fix addresses root cause, not symptom.**
- If SIGSEGV occurs, fix ABI mismatch, not error handling
- If tests pass but device fails, fix test coverage, not test assertions
- If bug comes back, fix root cause, not just the recurrence

### P2: Contract Before Code
**CONTRACT.md is written before any code change.**
- All code must satisfy CONTRACT terms
- If code cannot satisfy contract, code is wrong, not contract
- CONTRACT defines success, not CI

### P3: Taxonomy Before Fix
**FAILURE_TAXONOMY.md is consulted before any fix.**
- If failure is not classified, classify first
- If F-code has prevention, prevention is implemented first
- Taxonomy grows with each new failure mode

### P4: Device Is Truth
**CI output is signal, not truth. Device execution is truth.**
- CI passing does not mean device works
- CI failing means device is unknown
- Only device execution proves device behavior

### P5: Layer Ownership
**Each failure belongs to one layer. Fix at the right layer.**
- Build failures → Build Infrastructure Charter
- Runtime failures → Runtime Platform Charter
- Process failures → DevOps Release Charter
- No layer fixes another layer's problem

### P6: Scope Freeze
**v4.0.0-stable scope is frozen. No additions.**
- Any proposed addition is logged for v4.1.0
- Scope creep is prevented by explicit freeze
- Stable must ship before features resume

### P7: Checkpoint Discipline
**Daily checkpoints prevent lost work.**
- End of each day: update checkpoint file
- Before risky operations: checkpoint
- Checkpoint includes: what was done, what's next, blockers

---

## Escalation Path

When stuck or facing conflict:

```
┌─────────────────────────────────────────────────┐
│                 ESCALATION PATH                  │
├─────────────────────────────────────────────────┤
│                                                  │
│  1. Checkpoint current state                     │
│                                                  │
│  2. Review relevant documents:                   │
│     - CONTRACT.md                               │
│     - FAILURE_TAXONOMY.md                       │
│     - This document (OPERATING-PROCEDURES)      │
│     - INFRA-WORK-QUALITY.txt                    │
│                                                  │
│  3. If decision tree (D1-D6) applies:           │
│     - Follow decision tree                      │
│     - Decision tree output IS the decision      │
│                                                  │
│  4. If no decision tree applies:                │
│     - Identify the layer (see D2)               │
│     - Own the decision within that layer         │
│                                                  │
│  5. If still stuck after 3 attempts:            │
│     - STOP                                      │
│     - Document hypothesis                       │
│     - Document what was tried                    │
│     - Request human review                       │
│     - Do NOT continue without resolution         │
│                                                  │
└─────────────────────────────────────────────────┘
```

---

## File Naming Conventions

| Type | Convention | Example |
|------|-----------|---------|
| Checkpoints | `checkpoints/aura-{date}.md` | `checkpoints/aura-2026-03-22.md` |
| Incident Reports | `docs/incidents/{date}-{summary}.md` | `docs/incidents/2026-03-22-sigsegv-f001.md` |
| Failure Logs | `ISSUE-LOG.md` (append only) | — |
| Postmortems | `docs/reports/postmortem-{date}.md` | `docs/reports/postmortem-2026-03-22.md` |

---

## Key Files Reference

| File | Purpose | When Referenced |
|------|---------|-----------------|
| `CONTRACT.md` | Platform promises | Before any release |
| `FAILURE_TAXONOMY.md` | F001-F015 codes | Before any fix |
| `OPERATING-PROCEDURES.md` | **THIS DOCUMENT** | Daily work |
| `TRANSFORMATION-PLAN.md` | Sprint plan | Weekly planning |
| `INFRA-WORK-QUALITY.txt` | Team charter, policies | When charter unclear |
| `ENTERPRISE-BASIC.txt` | Core truths, principles | When stuck |

---

## Forbidden Actions

These actions are FORBIDDEN without explicit Architecture Charter approval:

| Action | Why Forbidden | Alternative |
|--------|--------------|-------------|
| Bypass release gate | F013 (Release governance failure) | Fix gate, don't skip |
| Modify code without F-code | F014 (Regression from ad-hoc fix) | Classify first |
| Add features to v4.0.0 | Scope creep, delays stable | Log for v4.1.0 |
| Fix symptom without root cause | F014 (Regression) | Follow taxonomy |
| CI green → release | F013 (Release governance) | Follow D6 |
| Modify rustup on device | F009 (Toolchain failure) | pkg install rust |
| Use async_trait for flexibility | F006 (Feature complexity) | Sync-only traits |

---

## Document Evolution

This document is a LIVING DOCUMENT. Update when:

1. New failure mode requires new decision tree
2. Team structure changes
3. Process proves ineffective
4. New roles/charters created

**Update Protocol**: Changes require Architecture Charter approval.

---

## Summary

This document defines HOW work happens. The other documents define WHAT work happens:

| Document | Defines |
|----------|--------|
| `TRANSFORMATION-PLAN.md` | WHAT gets built (scope, timeline) |
| `CONTRACT.md` | WHAT is promised (success criteria) |
| `FAILURE_TAXONOMY.md` | WHAT can go wrong (failure modes) |
| `OPERATING-PROCEDURES.md` | **THIS DOCUMENT** — HOW work happens |

The WHAT documents are relatively static. This document (HOW) evolves as we learn.

---

## Final Statement

> **"The way we work is as important as what we build. These procedures are not bureaucracy. They are the system that prevents the failures that have plagued 8 alpha releases."**

Follow the decision trees. Classify every failure. Checkpoint daily. Device testing is mandatory. CI green is not release.

These are not suggestions. They are the operating procedures for v4.0.0-stable.
