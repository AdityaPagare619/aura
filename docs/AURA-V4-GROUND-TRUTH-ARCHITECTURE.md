# AURA v4 — Ground-Truth Architecture

> **Canonical reference. All prior architecture documents, audit reports, and inline comments are superseded by this file.**
> Last updated: 2026-03-11

---

## 0. Purpose and Status of This Document

This document is the definitive specification for AURA v4. Developers read this before reading code. When code contradicts this document, the code is wrong. When this document contradicts an audit report, this document is correct — the audits informed it, but this is the synthesis.

AURA v4 is a **privacy-first, on-device Android AI assistant written in Rust**. All inference runs locally via llama.cpp. No telemetry. No cloud fallback. No exceptions.

---

## 1. The Core Principle — Brain / Body Separation

The entire architecture flows from one principle:

> **Intelligence lives in the LLM. Rust is the body.**

| Layer | Process | Responsibility |
|---|---|---|
| Body | `aura-daemon` (PID 1) | Executes actions, manages screen, reads sensors, writes memory, enforces safety policy, stores all state |
| Brain | `aura-neocortex` (PID 2) | Reasons, plans, generates responses, expresses personality, applies emotional intelligence |

**What this means in practice:**

- Rust code **never** computes what the LLM should think or feel
- Rust code **never** orchestrates multi-step reasoning
- Rust code **never** generates behavioral directives ("be more formal", "show empathy")
- The LLM **always** receives raw state (numbers, facts, context) and decides what to do with it
- The LLM **always** owns the ReAct loop: Thought → Action → Observation → next Thought

Any code that violates this principle is architecturally wrong regardless of how well it is implemented.

---

## 2. Process Architecture

### Two Processes, One Mind

```
┌──────────────────────────────────────┐
│           aura-daemon (PID 1)        │
│  - Persistent background process     │
│  - Screen interaction (a11y layer)   │
│  - Memory subsystem (SQLite, HNSW)   │
│  - Identity state storage            │
│  - Tool execution                    │
│  - Safety policy enforcement         │
│  - ETG cache (fast-path flows)       │
│  - OutcomeBus dispatcher             │
└─────────────────┬────────────────────┘
                  │ Unix socket IPC
                  │ Length-prefixed bincode
                  │
┌─────────────────▼────────────────────┐
│        aura-neocortex (PID 2)        │
│  - LLM process (llama.cpp)           │
│  - ReAct loop owner                  │
│  - All reasoning and planning        │
│  - Personality expression            │
│  - Emotional intelligence            │
│  - Response generation               │
└──────────────────────────────────────┘
```

### IPC Contract

Transport: Unix socket, length-prefixed bincode frames.

**Daemon → Neocortex:**

| Message | When | Payload |
|---|---|---|
| `ContextPackage` | S2 invocation | Full context: user request, episodic memories, semantic facts, personality/mood/relationship state (raw values), tool definitions, current screen state |
| `ReActObservation` | After each action step | Real screen state captured from device |
| `Embed` | Semantic memory write | Text to embed (handler must be implemented — currently dead code) |

**Neocortex → Daemon:**

| Message | When | Payload |
|---|---|---|
| `ReActStep` | Each reasoning step | `{ thought, action, action_input }` |
| `FinalResponse` | Loop complete | Response text + any side-effect instructions |
| `EmbeddingResult` | After embed request | Vector |

**Critical fix required:** `ReActObservation` and `ReActStep` IPC message types do not yet exist. The loop is currently open — daemon uses `simulate_action_result()` in `daemon_core/react.rs` at lines ~1662 and ~1779. These IPC types must be added and the simulation calls removed before the ReAct loop functions correctly.

---

## 3. The Routing Truth

### S1: Fast-Path (Cache Lookup Only)

S1 is not a Bayesian cascade. S1 is not a decision tree. S1 is a cache check.

```
Incoming request
      │
      ▼
Does ETG cache contain a matching flow for this request?
      │
   Yes │ No
      │  └──→ Build ContextPackage → IPC to neocortex (S2)
      ▼
Execute cached flow directly (~50–200ms)
      │
      ▼
If execution fails mid-flow → fall back to S2
```

The ETG (Execution Template Graph) cache holds successful multi-step action flows. Each entry is freshness-decayed (recent successes have higher weight). LRU eviction. A flow must be fresh and confidence-scored above threshold to execute from cache.

**Routing classifier (`routing/classifier.rs`):** Retained but simplified. Its only job is to score a request as "complex/important → LLM" or "simple/cached → check ETG." The current multi-stage Bayesian cascade in `routing/system1.rs` is replaced by this binary decision plus an ETG lookup.

### S2: LLM ReAct Loop

```
ContextPackage arrives at neocortex
      │
      ▼
[Neocortex] Thought: what do I need to do?
      │
      ▼
[Neocortex] Action: <tool_name> with <args>
      │
      ▼ IPC: ReActStep
[Daemon] Execute one action on device
      │
      ▼ IPC: ReActObservation
[Neocortex] Observe real screen state
      │
      ▼
Next thought... (loop)
      │
      ▼
[Neocortex] Final response
      │
      ▼ IPC: FinalResponse
[Daemon] Safety gate → execute + deliver
```

The neocortex owns the loop. The daemon executes exactly one step per `ReActStep` message and returns the real observation. The daemon does not orchestrate, does not decide when the loop ends, does not generate intermediate thoughts.

---

## 4. The Memory Truth

### Memory Stores the User — Not the Screen

Memory is about who the user is: their preferences, habits, goals, relationship history, emotional patterns, beliefs, and values. Memory is not a log of UI interactions. Screen navigation patterns belong in the ETG cache.

### Memory Subsystem (all in daemon)

| Store | Implementation | Content |
|---|---|---|
| Episodic | SQLite | Time-stamped events: conversations, actions taken, outcomes observed |
| Semantic | HNSW index | Distilled facts about the user: preferences, habits, beliefs, goals |
| Working | Ring buffer | Active context window — last N turns fed into every ContextPackage |
| ETG cache | LRU map | Successful action flows with freshness decay — **not** part of memory proper |

### What Goes in Each Store

**Episodic** receives raw events: "User asked to set a timer at 14:32. Timer set. User confirmed." Not screen coordinates. Not element IDs.

**Semantic** receives distillations: "User prefers metric units." "User exercises on weekday mornings." "User dislikes being interrupted during focus time." These facts are written during the dreaming/consolidation phase (see Section 6).

**ETG cache** receives successful execution traces: sequences of tool calls that completed a goal. These are extracted by pattern matching on episodic outcomes, not by LLM reasoning.

### The `memory/patterns.rs` Status

`memory/patterns.rs` (1231L) stores **user behavioral patterns** — action→outcome Hebbian associations and temporal co-occurrence. It does not store screen coordinates or UI element references. This is correct placement and the file is **kept**. There is potential redundancy with `arc/learning/patterns.rs`, but both serve different consumers: `memory/patterns.rs` serves memory consolidation; `arc/learning/patterns.rs` serves proactive prediction. Audit for overlap before any future consolidation.

---

## 5. The Identity and Soul Truth

### These Subsystems Are Strong — The Wiring Is Wrong

The identity subsystems contain real mathematics grounded in personality psychology literature. They are among the strongest parts of the codebase. They are **kept in full**. What changes is how their output connects to the LLM.

### The Wiring Problem

**Current (wrong):**
```
Rust computes identity state
      │
      ▼
Rust generates directive string: "Be more formal. Show warmth."
      │
      ▼ injected into prompt
LLM reads opaque instruction it cannot reason about
```

**Correct:**
```
Rust stores identity state (the numbers)
      │
      ▼
Rust packages raw values into ContextPackage
      │
      ▼ IPC
LLM receives: OCEAN scores, VAD values, relationship stage, trust level
      │
      ▼
LLM expresses personality naturally from first principles
```

The LLM understands what OCEAN scores mean. The LLM understands valence and arousal. Give it the numbers — let it reason about them. Don't pre-digest them into instructions.

### Subsystem Status

**`personality.rs` (1151L) — Grade A. KEEP ALL MATH. TRANSFORM wiring.**

- OCEAN model with exposure-attenuated trait evolution
- Differential malleability weights (McCrae & Costa grounding)
- Micro-drift with mean regression
- **Change:** OCEAN scores passed to LLM as structured context values. Rust no longer computes behavioral directives from trait scores.

**`affective.rs` (1630L) — Grade A. KEEP ALL MATH. TRANSFORM wiring.**

- VAD model (Valence-Arousal-Dominance)
- Exponential decay (300s half-life)
- EMA smoothing (15min τ)
- Stability gating, 30-min cooldown
- **Change:** Raw VAD values + human-readable description passed to LLM. Rust no longer generates mood-based prompt phrases. Note: bug at `affective.rs:211-218` — incorrect conditional ordering in mood context string. Fix before wiring transformation.

**`relationship.rs` (420L) — Grade A-. KEEP ALL MATH. TRANSFORM wiring.**

- Trust with diminishing returns: `1/√(1+n/10)`
- Negativity bias at 1.5× (literature supports 2–3×; recalibrate)
- 4-tier autonomy gating
- Capacity eviction
- **Change:** Relationship stage and trust level passed to LLM as context. LLM decides tone, formality, openness. **Autonomy gating stays in Rust** — it is safety-critical and must not be delegated.

**`prompt_personality.rs` (584L) — Grade B+. TRANSFORM.**

Currently generates imperative directive strings. Becomes a serializer: takes raw OCEAN/VAD/relationship values and produces a structured context block (e.g., JSON or TOML section) for inclusion in ContextPackage. No directives. No imperatives. Raw values only.

**`thinking_partner.rs` (123L) — Grade B. TRANSFORM.**

Currently implements Socratic dialogue via Rust string templates. This is the LLM's job. Transform to: a `thinking_partner_mode: bool` field in ContextPackage. When true, the neocortex's system prompt activates Socratic framing. The Rust file is reduced to a flag setter.

**`anti_sycophancy.rs` — TRANSFORM.**

Becomes two things:
1. Anti-sycophancy instructions are **always** present in the system prompt. Unconditional. Not toggled by any state.
2. A thin Rust post-filter that checks if the LLM response unconditionally agrees with everything the user stated — pattern matching, not semantic analysis. If flagged, the response is sent back to the neocortex with a "reconsider" instruction.

**`user_profile.rs` (694L) — Grade A-. KEEP AS-IS.**

Granular privacy with 5 independent toggles, GDPR export/delete, SQLite persistence. No wiring problems. Minor bug: `effective_ocean()` at lines 325–341 is a no-op at default calibration — it returns unadjusted values. Fix the calibration logic or remove the pretense.

### ContextPackage Identity Block (canonical shape)

```
identity:
  personality:
    openness: 0.52
    conscientiousness: 0.61
    extraversion: 0.44
    agreeableness: 0.58
    neuroticism: 0.38
    confidence: 0.71          # how settled these scores are
  affect:
    valence: 0.62             # positive/negative
    arousal: 0.41             # activated/calm
    dominance: 0.55           # in-control/overwhelmed
    description: "calm and mildly positive"
  relationship:
    stage: "Friend"           # Stranger|Acquaintance|Friend|CloseFriend|Soulmate
    trust: 0.74
    interaction_count: 847
  thinking_partner_mode: false
```

---

## 6. The Learning Truth

### LLM-Assisted Consolidation, Not Hebbian Learning

Hebbian learning applied to screen interaction patterns in Rust is the wrong layer computing the wrong thing. It approximates what the LLM already understands natively. Replace it.

### Correct Learning Architecture

**Episodic observation (keep):**
OutcomeBus dispatches every execution outcome to the episodic store. Raw events accumulate during the day. This is correct and kept unchanged.

**Dreaming / consolidation phase (transform):**
When device is charging and idle, the dreaming process:
1. Pulls recent episodic memories (since last consolidation)
2. Sends them to neocortex with prompt: *"Distill key facts about this user from these recent interactions. Be specific. Each fact should be independently useful."*
3. Writes LLM output to semantic memory (HNSW index)
4. Marks episodic entries as consolidated

This is the correct learning loop. The LLM does the semantic compression. Rust stores the result.

**ETG learning (keep, no LLM needed):**
Successful multi-step flows are extracted from episodic outcomes by pattern matching on execution traces. When a sequence of actions completes a recognized goal with no errors, it is written to the ETG cache. This is pure Rust pattern matching — no LLM required.

**OutcomeBus (keep architecture, transform subscribers):**
The OutcomeBus correctly dispatches execution outcomes to 5 subscribers. The architecture is right. What each subscriber does needs the transformation described above and in the Surgery Map below.

```
Execution completes
      │
      ▼
OutcomeBus dispatches to:
  ├── Episodic memory (write raw event) ✓
  ├── ETG (check if flow qualifies for caching) ✓
  ├── Goals (update progress tracking) ✓
  ├── Identity (update personality/relationship/mood state) ✓
  └── Anti-sycophancy (track agreement pattern) ✓
```

---

## 7. Surgery Map

### DELETE — Wrong Concepts, Not Just Wrong Implementation

These files implement ideas that are architecturally incorrect. Rewriting them would still be wrong. They are removed.

| File | Lines | Why Deleted | Replacement |
|---|---|---|---|
| *(no files currently in this category — see audit corrections below)* | — | — | — |

### TRANSFORM — Correct Concepts, Wrong Wiring

These files are kept, but their output connections to the LLM change fundamentally.

| File | Current | Target |
|---|---|---|
| `identity/personality.rs` | Computes OCEAN → generates behavioral directives | Computes OCEAN → serializes values to ContextPackage |
| `identity/affective.rs` | Computes VAD → generates mood phrases | Computes VAD → serializes raw values + description to ContextPackage |
| `identity/relationship.rs` | Computes trust → generates tone directives | Computes trust → serializes stage/trust to ContextPackage; autonomy gating stays in Rust |
| `identity/prompt_personality.rs` | Generates imperative strings | Serializes structured identity context block |
| `identity/thinking_partner.rs` | Rust Socratic templates | Sets `thinking_partner_mode: bool` flag in ContextPackage |
| `identity/anti_sycophancy.rs` | Conditional sycophancy detection | Always-on system prompt directive + thin post-filter |
| `routing/classifier.rs` | Complex multi-stage Bayesian scoring | Binary: complex/important → LLM; simple/known → ETG check |
| `learning/hebbian.rs` | Rust-computed association weights on screen patterns | Deleted; replaced by LLM-assisted episodic consolidation in dreaming phase |
| `dreaming.rs` | Offline processing with Rust algorithms | Offline LLM-summarization of episodic memories → semantic memory writes |

### FIX — Genuine Bugs in Otherwise Correct Code

| Location | Bug | Fix |
|---|---|---|
| `inference.rs:789` | Type mismatch: uses `aura_types::ipc::GenerationConfig` where `ModeConfig` required — **compile error** | Replace with `ModeConfig` |
| `inference.rs:503` | ReAct goal set to `base_prompt.system_prompt.clone()` instead of original user request | Add `original_goal: String` field to `AssembledPrompt`; use it here |
| `daemon_core/react.rs:~1662,~1779` | `simulate_action_result()` — loop is open, no real screen observation. **Note:** The real ReAct loop is in `daemon_core/react.rs` (2585 lines). This file has `simulate_action_result()` and needs the closed-loop IPC wiring. | Implement `ReActObservation` IPC message; wire closed-loop IPC in `daemon_core/react.rs`; remove simulation calls |
| `main_loop.rs:handle_cron_tick` | 1,200-line string-matching God Function | Refactor to event-dispatch table |
| Codebase-wide | 634 `.unwrap()` calls — will panic on device | Systematic elimination; replace with `?`, `unwrap_or`, or logged error paths |
| IPC definition | `DaemonToNeocortex::Embed` variant has no handler | Implement handler or remove variant; currently dead code or masked compile error |
| `user_profile.rs:325-341` | `effective_ocean()` is no-op at default calibration | Fix calibration logic or remove the function |
| `affective.rs:211-218` | Incorrect conditional ordering in mood context string | Reorder conditionals to match intended semantic |

### KEEP — Correct Architecture, No Changes Needed

| File/System | Why Kept |
|---|---|
| `user_profile.rs` | Granular privacy, GDPR, SQLite persistence — correct as-is |
| OutcomeBus architecture | 5-subscriber dispatch is correct; what subscribers do changes |
| Episodic memory (SQLite) | Correct store, correct content |
| Semantic memory (HNSW) | Correct store; content written by LLM consolidation |
| Working memory (ring buffer) | Correct; feeds ContextPackage |
| ETG cache (LRU + freshness decay) | Correct architecture |
| Autonomy gating in `relationship.rs` | Safety-critical; must stay in Rust |
| All OCEAN math in `personality.rs` | Mathematically sound; only wiring changes |
| All VAD math in `affective.rs` | Mathematically sound; only wiring changes |
| All trust math in `relationship.rs` | Sound; negativity bias recalibration recommended (1.5× → 2–3×) |
| Privacy architecture | Non-negotiable; nothing touches this |
| `routing/system1.rs` | IS the ETG cache-lookup (256-entry LRU, 14-day half-life freshness decay). Not a Bayesian cascade. Already exactly what the architecture calls for. |
| `routing/system2.rs` | IS the daemon-side IPC lifecycle manager: monotonic request ID allocation, pending request tracking (64-entry bounded HashMap), 30s stale sweep, cancellation. Daemon correctly owns this. |
| `execution/react.rs` | IS a 165-line escalation threshold evaluator (CognitiveState, EscalationContext, adaptive threshold with urgency/battery/thermal). Correctly placed. Not the ReAct loop. The real ReAct loop is in `daemon_core/react.rs` (2585 lines) — that file has `simulate_action_result()` and needs the closed-loop IPC wiring. |
| `arc/` directory | Does domain-specific DATA MANAGEMENT: health metric tracking, social relationship scoring, behavioral pattern detection, proactive suggestion generation. Uses deterministic algorithms (Bayesian confidence, Hebbian associations) that correctly belong in Rust. Not doing LLM-level domain reasoning. |
| `memory/patterns.rs` | Stores USER BEHAVIORAL PATTERNS (action→outcome Hebbian associations, temporal co-occurrence). No screen coordinates, no UI element references. Note: potential redundancy with `arc/learning/patterns.rs`; both serve different consumers — memory consolidation vs proactive prediction. |

---

## 8. Day Zero Guarantee

Every feature works correctly on first launch with zero learned data. No cold-start failure modes.

| Subsystem | Day Zero State | Behavior |
|---|---|---|
| Personality | OCEAN: O=0.85, C=0.75, E=0.50, A=0.70, N=0.25 | Curious, reliable, warm, stable — correct baseline |
| Relationship | Stranger stage, trust: 0.0 | Polite, measured tone — appropriate for a stranger |
| Affect | Neutral VAD: (0.0, 0.0, 0.5) | Neutral valence, calm arousal, balanced dominance — appropriate default |
| ETG cache | Empty | Every request routes to S2 (LLM) — correct fallback |
| Episodic memory | Empty | LLM reasons from request context only — works |
| Semantic memory | Empty | LLM reasons without prior facts — works |
| Working memory | Empty | No prior turns in context — works |
| Anti-sycophancy | Always active | Unconditional system prompt directive, always present |
| Privacy toggles | All enabled | Maximum privacy by default |

The system degrades gracefully as data accumulates: more learned facts → better context → better responses. It never requires learned data to function.

---

## 9. The Privacy Guarantee

These constraints are absolute. No architectural change may compromise them.

| Guarantee | Implementation |
|---|---|
| No telemetry | No outbound network calls except user-initiated actions |
| No cloud inference | llama.cpp runs entirely on-device |
| No cloud memory | SQLite and HNSW index are on-device only |
| No cloud fallback | If on-device inference fails, the system returns an error — it does not fall back to a remote model |
| User data export | GDPR-compliant export in `user_profile.rs` — all episodic, semantic, and profile data |
| User data deletion | Full delete in `user_profile.rs` — all stores wiped on request |
| Granular privacy | 5 independent toggles in `user_profile.rs` — users control what each subsystem stores |

The privacy architecture is not a feature. It is the product's foundational promise. `user_profile.rs` is kept exactly as-is.

---

## Appendix A: ContextPackage Canonical Shape

The ContextPackage is the sole input to the neocortex. It contains everything the LLM needs to reason. Nothing is pre-digested into directives.

```
ContextPackage {
    request: String,                    // original user request, verbatim
    working_memory: Vec<Turn>,          // last N conversation turns
    episodic_context: Vec<Episode>,     // relevant recent events
    semantic_facts: Vec<Fact>,          // distilled user facts from HNSW
    identity: IdentityContext {         // raw values, no directives
        personality: OceanScores,
        affect: VadState,
        relationship: RelationshipState,
        thinking_partner_mode: bool,
    },
    tools: Vec<ToolDefinition>,         // available actions daemon can execute
    screen_state: Option<ScreenState>,  // current device screen (if relevant)
    safety_constraints: SafetyContext,  // active constraints daemon enforces
}
```

Nothing in this package is an instruction to the LLM about how to behave. Everything is raw state for the LLM to reason about.

---

## Appendix B: What This Architecture Is Not

These are common misreadings to avoid:

- **AURA is not a chatbot with a plugin system.** It is an AI assistant that operates the device. The LLM issues actions; the daemon executes them.
- **S1 is not an AI.** It is a cache. There is no intelligence in S1 — it is a lookup table of previously successful flows.
- **The daemon is not a co-reasoner.** It does not think. It executes, stores, and observes. All reasoning happens in the neocortex.
- **Personality is not a prompt injection system.** OCEAN scores are context. The LLM is trusted to express them — we do not need to tell it "be conscientious."
- **Memory is not a screen scraper.** It records what the user said, wanted, felt, and accomplished — not which buttons were pressed.
- **The dreaming phase is not a batch job.** It is a consolidation ritual: the system reflects on the day, extracts durable knowledge, and forgets the ephemeral details.

---

*This document reflects the architecture as designed. Files not yet updated to match this spec are tracked in the Surgery Map above. When in doubt: the LLM reasons, Rust executes.*
