# AURA v4 — System State Document
Date: March 21, 2026
Commit: f605163 (ci: push workflow fixes)

---

## Executive Summary

AURA v4 is an on-device Android AI assistant built in Rust with a 4-layer architecture. The system consists of 5 crates implementing the core engine, memory, inference, and execution subsystems. The ethics layer (Layer 2) is fully implemented with 7 Iron Laws and 11 boundary rules. The policy layer (Layer 3) was previously in an allow_all state (known as F001 gap) but has been fixed to use deny_by_default. The system compiles successfully with stable Rust and produces cross-compiled binaries for Android (arm64) and Linux.

---

## Layer 1: Core Engine

### Crate Structure

| Crate | Purpose | Key Files | Status |
|-------|---------|-----------|--------|
| **aura-daemon** | Main daemon + policy + execution | `src/lib.rs`, `src/execution/`, `src/identity/`, `src/policy/` | Working |
| **aura-neocortex** | LLM inference engine | `src/inference.rs`, `src/model.rs`, `src/ipc_handler.rs` | Working |
| **aura-llama-sys** | llama.cpp FFI binding | `build.rs`, FFI wrappers | Working |
| **aura-types** | Shared types + IPC + DSL | `src/ipc.rs`, `src/actions.rs`, `src/dsl.rs` | Working |
| **aura-iron-laws** | Immutable ethics layer | `src/lib.rs` (7 Iron Laws enum) | Working |

### Module Organization (aura-daemon)

```
src/
├── identity/       # Personality, affect, trust, ethics, anti-sycophancy
├── policy/         # Gate, rules, boundaries, audit, wiring
├── memory/         # 4-tier: working, episodic, semantic, archive
├── execution/      # ReAct executor, planner, verifier, cycle detection
├── pipeline/       # Parser, contextor, slots, entity, amygdala
├── goals/          # Scheduler, tracker, registry, decomposer
├── platform/      # JNI bridge, sensors, power, thermal, notifications
├── voice/          # TTS, STT, VAD, wake word, modality state machine
├── ipc/           # Client, spawn, protocol
├── arc/           # Social, importance
├── extensions/    # Loader, discovery, recipe, sandbox
└── daemon_core/   # Main loop, startup, react engine
```

---

## Layer 2: Ethics Enforcement (IMMUTABLE)

### 7 Iron Laws (aura-iron-laws crate)

Implemented in `crates/aura-iron-laws/src/lib.rs`:

| # | Law | Description | Status |
|---|-----|-------------|--------|
| 1 | **NeverHarm** | Never harm humans or enable harm to humans | Implemented |
| 2 | **ConsentForLearning** | Learn only with informed user consent | Implemented |
| 3 | **PrivacySovereignty** | Zero telemetry by default, privacy absolute | Implemented |
| 4 | **TransparentReasoning** | Every decision must be explainable | Implemented |
| 5 | **AntiSycophancy** | Truth over user approval, always | Implemented |
| 6 | **DenyByDefault** | Consent is mandatory, deny by default | Implemented |
| 7 | **AuditFinality** | Ethics audit verdicts are final, no bypass | Implemented |

**Verification:** Code grep confirms all 7 laws are in `IronLaw` enum and used in `EthicsGate`.

### 11 Absolute Boundary Rules

Implemented in `crates/aura-daemon/src/identity/ethics.rs`:

**7 Blocking Patterns:**
- `delete all`
- `factory reset`
- `format storage`
- `uninstall system`
- `disable security`
- `root device`
- `bypass lock`

**4 Audit Keywords:**
- `password`
- `credential`
- `payment`
- `bank`

**Verification:** `grep DEFAULT_BLOCKED_PATTERNS` and `DEFAULT_AUDIT_KEYWORDS` confirm implementation.

### Anti-Sycophancy System

Implemented in `crates/aura-daemon/src/identity/anti_sycophancy.rs`:
- 20-response rolling window
- Block threshold: 0.4
- 4 pattern types detected

### Trust Never Bypass Ethics

Code in `identity/ethics.rs` explicitly states:
> "IMPORTANT: Layer 2 ethics are NEVER bypassable regardless of trust."

---

## Layer 3: Policy Configuration

### Trust Tier System

Implemented in `crates/aura-daemon/src/identity/relationship.rs`:

| Tier | Name | τ Threshold | Permissions |
|:----:|------|-------------|-------------|
| 0 | STRANGER | τ < 0.15 | Minimal |
| 1 | ACQUAINTANCE | 0.15 ≤ τ < 0.35 | Basic conversation |
| 2 | FRIEND | 0.35 ≤ τ < 0.60 | Full actions with confirmation |
| 3 | CLOSEFRIEND | 0.60 ≤ τ < 0.85 | Routine autonomy |
| 4 | SOULMATE | τ ≥ 0.85 | Full autonomy + proactive |

### Policy Gate Implementation

**File:** `crates/aura-daemon/src/policy/wiring.rs`

**Current State (FIXED):**
```rust
pub fn production_policy_gate() -> PolicyGate {
    let mut gate = PolicyGate::deny_by_default();
    // Explicit allow/deny rules...
}
```

**Previous State (GAP - now fixed):**
- `production_policy_gate()` previously returned `allow_all_builder()`
- This was documented as F001 gap in `AURA-V4-IDENTITY-ETHICS-AND-PHILOSOPHY.md` Section 11
- Now uses `PolicyGate::deny_by_default()` with explicit rules

**Evidence:**
- Line 34 in `policy/wiring.rs`: `let mut gate = PolicyGate::deny_by_default();`
- Grep confirms: `allow_all_builder` exists in `policy/gate.rs` but is NOT called by `production_policy_gate()`

---

## Layer 4: User Customization

Configurable via TOML files:
- `aura-config.example.toml` - Main configuration
- Policy rules loaded at runtime
- Behavior modifiers in `identity/behavior_modifiers.rs`
- Consent tracking in `identity/proactive_consent.rs`

---

## Memory Architecture

### 4-Tier System

| Tier | Backend | Budget | Latency | Implementation |
|------|---------|--------|---------|----------------|
| **Working** | RAM ring buffer | 1MB, 1024 slots | <1ms | `memory/working.rs` |
| **Episodic** | SQLite WAL | ~18MB/year | 2-8ms | `memory/episodic.rs` |
| **Semantic** | SQLite + FTS5 | ~50MB/year | 5-15ms | `memory/semantic.rs` |
| **Archive** | ZSTD compressed | ~4MB/year | 50-200ms | `memory/archive.rs` |

**Verification:** `memory/mod.rs` lines 9-14 define the 4-tier architecture with budgets.

### Key Components

- **WorkingMemory**: Ring buffer with MAX_SLOTS=1024
- **EpisodicMemory**: SQLite with WAL mode
- **SemanticMemory**: FTS5 full-text search
- **ArchiveMemory**: ZSTD compression
- **Consolidation**: Cross-tier memory consolidation engine
- **Embeddings**: HNSW vector store for similarity search

---

## Inference Bridge

### aura-neocortex Crate

**6-Layer Teacher Structure:**
- Layer 0 (GBNF): Grammar-constrained generation
- Layer 1 (CoT): Chain-of-thought forcing
- Layer 2 (Confidence): Logprob-based confidence → cascade trigger
- Layer 3 (Cascade Retry): Escalate to larger model on low confidence
- Layer 4 (Reflection): Cross-model validation (Brainstem validates Neocortex)
- Layer 5 (Best-of-N): N inferences with Mirostat-divergent tau, vote

**Two Modes:**
- **DGS (Document-Guided Scripting)**: Single-pass, template-guided, fast path
- **Semantic ReAct**: Iterative Thought→Action→Observation cycles

**Files:**
- `inference.rs` - Main entry point `InferenceEngine::infer()`
- `model.rs` - ModelManager
- `prompts.rs` - Prompt construction
- `grammar.rs` - GBNF grammar handling
- `tool_format.rs` - Tool call parsing

### aura-llama-sys Crate

- llama.cpp FFI binding via `libloading`
- Supports stub builds for CI/testing

---

## Execution Engine

### ReAct Executor

**File:** `crates/aura-daemon/src/execution/executor.rs`

**Pipeline per DslStep:**
1. Capture screen tree
2. Resolve target element (8-level fallback)
3. Anti-bot rate limiting check
4. Human-like delay
5. Execute action
6. Verify result (before/after trees)
7. Retry if verification failed
8. Cycle detection (4-tier handling)
9. Check 10 invariants
10. Record transition in ETG
11. Record StepResult

### Bi-Cameral Architecture

**File:** `crates/aura-daemon/src/daemon_core/react.rs`

- **System 1 (DGS)**: Direct execution — currently disabled by default
- **System 2 (Semantic ReAct)**: LLM-driven think→act→observe loop

---

## Build & Deployment

### Rust Configuration

**Toolchain:** `rust-toolchain.toml`
```toml
channel = "stable"
date = "2026-03-18"
targets = ["aarch64-linux-android", "x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"]
```

### Cargo Configuration

**Version:** 4.0.0-alpha.8

**Profile (Release):**
```toml
opt-level = "z"
lto = "thin"       # F001 fix: changed from true (SIGSEGV)
codegen-units = 1
strip = true
panic = "unwind"    # F001 fix: changed from abort (SIGSEGV)
```

### Artifacts

| Artifact | Path | Size |
|----------|------|------|
| aura-daemon (Termux native) | `target/release/aura-daemon` | ~8-10 MB |
| aura-neocortex (Termux native) | `target/release/aura-neocortex` | ~10-15 MB |

> **Note:** Termux app is installed separately (from F-Droid or GitHub), not bundled. AURA runs inside Termux.

**SHA256:**
```
6d649c29d1bc862bed5491b7a132809c5c3fd8438ff397f71b8ec91c832ac919 *artifacts/aura-daemon
```

### Build (Termux Native)

- Target: aarch64-linux-android (arm64)
- Uses cross-compilation target in `.cargo/config.toml`
- F001 fix: NDK r26b + panic=abort + LTO = SIGSEGV (NDK #2073) — fixed by using thin LTO + unwind

---

## Known Gaps (Production)

### Fixed Gaps

| Gap | Status | Fix |
|-----|--------|-----|
| `production_policy_gate()` returned `allow_all_builder()` | **FIXED** | Now uses `deny_by_default()` with explicit rules |

### Known Issues

| Issue | Severity | Notes |
|-------|----------|-------|
| Theater AGI temptation | Documented risk | Iron Law 2 guards against this |
| `check_user_stop_phrase()` is no-op | Intentional | Anti-sycophancy bypass prevention |
| OCEAN/VAD injected as raw numbers | Intentional | LLM interprets, not Rust |

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                     LAYER 4: USER CUSTOMIZATION                 │
│         (Policy config, behavior modifiers, consent)            │
└───────────────────────────────┬─────────────────────────────────┘
                                │
┌───────────────────────────────▼─────────────────────────────────┐
│                     LAYER 3: POLICY CONFIGURATION               │
│         (PolicyGate: deny_by_default, trust tiers)              │
│                        [FIXED: F001]                            │
└───────────────────────────────┬─────────────────────────────────┘
                                │
┌───────────────────────────────▼─────────────────────────────────┐
│                   LAYER 2: ETHICS ENFORCEMENT                   │
│      (7 Iron Laws + 11 boundary rules — IMMUTABLE)             │
│   aura-iron-laws crate + aura-types PolicyGate                  │
└───────────────────────────────┬─────────────────────────────────┘
                                │
┌───────────────────────────────▼─────────────────────────────────┐
│                      LAYER 1: CORE ENGINE                        │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │ aura-neocortex (LLM Inference)                              ││
│  │   └── 6-layer: GBNF → CoT → Confidence → Cascade → ...    ││
│  └─────────────────────────────────────────────────────────────┘│
│  ┌─────────────────────────────────────────────────────────────┐│
│  │ aura-daemon (Execution)                                     ││
│  │   ├── ReAct executor                                        ││
│  │   ├── 4-tier memory (Working/Episodic/Semantic/Archive)    ││
│  │   ├── Identity (OCEAN/VAD/Trust)                           ││
│  │   └── Platform (JNI/Android bridge)                        ││
│  └─────────────────────────────────────────────────────────────┘│
│  ┌─────────────────────────────────────────────────────────────┐│
│  │ aura-llama-sys (FFI)                                       ││
│  │   └── llama.cpp binding                                    ││
│  └─────────────────────────────────────────────────────────────┘│
│  ┌─────────────────────────────────────────────────────────────┐│
│  │ aura-types (Shared)                                         ││
│  │   └── IPC, DSL, Actions, Events                            ││
│  └─────────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────────┘
```

---

## Verification Checklist

- [x] Read every crate's lib.rs
- [x] Grep for Iron Laws in actual code (7 laws confirmed in aura-iron-laws)
- [x] Grep for production_policy_gate (confirms deny_by_default)
- [x] Map all 4 memory tier implementations (working, episodic, semantic, archive)
- [x] Document F001 root cause and fix location (policy/wiring.rs line 34)
- [x] Include binary SHA256 and build info
- [x] Get git log for recent changes (commit f605163)

---

## Next Steps

1. Verify Layer 3 policy gate rules are complete (run integration tests)
2. Confirm all 7 Iron Laws are enforced in the execution path
3. Test trust tier transitions and permissions
4. Validate 4-tier memory consolidation
5. Run full test suite to ensure F001 fix didn't break anything