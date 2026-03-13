# AURA v4 — Master System Architecture

**Status:** Authoritative reference. Every agent reads this before touching any file.  
**Conflicts:** This doc defers to `HONEST-REFLECTION-AND-PHILOSOPHY-SHIFT.md` on philosophy.  
**Canonical types:** `AURA-V4-GROUND-TRUTH-ARCHITECTURE.md` for module-level layout.

---

## 1. The One Law

> **LLM = brain. Daemon = body.**

The daemon packages raw structured state and executes typed actions.  
The LLM reasons about state and decides what to do.  
Any Rust code that reasons, decides, or generates user-facing text **is wrong by default.**

---

## 2. The Core Loop (how AURA actually works)

```
User message (Direct | Voice | Telegram)
    ↓
Daemon builds ContextPackage
  - raw OCEAN floats [5]
  - raw VAD floats [3]
  - relationship_stage (enum)
  - relevant_memories (Vec<MemoryItem>)
  - screen_state (Option<ScreenDescription>)
  - tools (Vec<ToolDefinition>)
  - conversation_history (last N turns)
  - inference_mode (Conversational | Planner | Strategist | Composer)
    ↓
Daemon sends DaemonToNeocortex::{Converse | Plan | Replan | Compose}
    ↓
Neocortex (LLM) reasons → returns NeocortexToDaemon::{ConversationReply | PlanReady | ...}
    ↓
Daemon delivers reply OR executes plan step 1
    ↓ (if executing)
Daemon captures real screen state after action
    ↓
Daemon sends DaemonToNeocortex::ReActStep { tool_name, observation, screen_description, goal, step_index, max_steps }
    ↓
Neocortex reasons → returns NeocortexToDaemon::ReActDecision { done, reasoning, next_action }
    ↓ (repeat until done=true or max_steps reached)
```

**The ReAct loop is closed.** Daemon executes ONE action, captures REAL screen state, sends observation back. LLM decides next step. No `simulate_action_result()`.

---

## 3. IPC Typed Variants — Daemon → Neocortex

| Variant | When sent | What neocortex must do |
|---------|-----------|----------------------|
| `Load { model_path, params }` | Startup | Load model, return `Loaded` or `LoadFailed` |
| `Converse { context }` | User message (conversational) | Generate reply → `ConversationReply` |
| `Plan { context, failure }` | New goal / task | Generate action plan → `PlanReady` |
| `Replan { context, failure }` | Step failed | Generate new plan → `PlanReady` |
| `Compose { context, template }` | DSL composition needed | Generate DSL steps → `ComposedScript` |
| `ReActStep { ... }` | After executing each action | Decide done/next → `ReActDecision` |
| `Embed { text }` | Memory indexing | Generate vector → `Embedding` |
| `ProactiveContext { trigger, data }` | Proactive opportunity detected | Generate natural message → `ConversationReply` |
| `Ping` | Health check | Return `Pong` |
| `Cancel` | User interrupts | Abort inference, no response |
| `Unload` / `UnloadImmediate` | Shutdown / thermal | Unload model |

**`ProactiveContext` is not yet in `aura-types/ipc.rs` — Agent 1 must add it.**

---

## 4. IPC Typed Variants — Neocortex → Daemon

| Variant | Meaning |
|---------|---------|
| `ConversationReply { text, mood_hint }` | Send text to user |
| `PlanReady { plan }` | Execute the action plan |
| `ComposedScript { steps }` | Execute DSL steps |
| `ReActDecision { done, reasoning, next_action }` | Proceed or finish ReAct loop |
| `Embedding { vector }` | Store in HNSW memory index |
| `Loaded { model_name, memory_used_mb }` | Model ready |
| `Error { code, message }` | Inference failed |
| `Pong { uptime_ms }` | Health check response |
| `MemoryWarning { used_mb, available_mb }` | Reduce context size |
| `TokenBudgetExhausted` | Prompt too long |

---

## 5. Real-World Device State Model

AURA must function across 4 device states. Every subsystem must handle all 4.

### State A — Normal (battery >30%, no thermal)
- Full inference pipeline active
- Proactive triggers fire on schedule
- Background embedding runs after conversations
- Dreaming phase runs at night

### State B — Low Battery (15–30%)
- Disable background embedding
- Disable proactive triggers (no unsolicited messages)
- S1 ETG cache preferred; S2 only if no cache hit
- Inference max_tokens halved

### State C — Critical Battery (<15%) or Thermal Throttle
- S1 ETG cache ONLY — no LLM calls unless user actively messages
- Return "I'll be more helpful when you're charging" on new goals
- No memory consolidation
- No background tasks

### State D — User Idle (screen off, overnight, charging)
- Dreaming phase: LLM summarizes episodic → semantic memories
- ETG cache warming: pre-compute responses for common patterns
- Goal review: flag overdue/stalled goals for next wakeup
- No user-facing messages

**Daemon reads `EnvironmentSnapshot` before every LLM call and enforces these limits.**

---

## 6. Day-Zero Guarantee

AURA must function on day 1 with zero learned data. No subsystem may require prior learning to operate.

| Subsystem | Day-zero behavior |
|-----------|------------------|
| Personality | OCEAN defaults (0.5,0.5,0.5,0.5,0.5) → LLM infers neutral-warm tone |
| Goals | Empty list → LLM asks "What are you working on?" |
| Memory | Empty → no recall, LLM says "I don't have context on that yet" |
| Proactive | Zero triggers → silent (correct behavior) |
| ETG cache | Empty → S2 always, builds cache from first interactions |
| Relationships | `Acquaintance` stage → LLM uses formal-friendly register |

**Learning makes AURA faster and more personal. It never makes AURA functional.**

---

## 7. Personality Expression Pipeline

```
OCEAN f32[5]  ←── from PersonalityEngine::personality()
VAD f32[3]    ←── from AffectiveState::current_vad()
RelationshipStage ←── from RelationshipTracker::stage()
    ↓
Serialized as raw numbers into ContextPackage.identity_block
    ↓ (no template strings, no directive sentences)
LLM receives: { "ocean": [0.72, 0.31, 0.65, 0.88, 0.41], "vad": [0.6, 0.3, 0.7], "relationship": "Companion" }
    ↓
LLM expresses these naturally in its response
```

**NEVER generate strings like "I am feeling ENERGETIC and CURIOUS today" in Rust. The LLM expresses numbers as language — that's literally what it exists to do.**

---

## 8. Proactive Delivery Architecture

```
ProactiveDetector fires trigger (e.g. goal stalled 3 days)
    ↓
Daemon sends DaemonToNeocortex::ProactiveContext {
    trigger: ProactiveTrigger::GoalStalled { goal_id, title, stalled_days },
    user_context: ContextPackage,  // OCEAN, VAD, relationship, recent memories
}
    ↓
Neocortex generates: NeocortexToDaemon::ConversationReply { text: "Hey, noticed you haven't worked on X in 3 days. Want to pick it back up?" }
    ↓
Daemon routes via Telegram sender
```

**The format of the message is entirely LLM-generated. Daemon only sends typed structured data.**

---

## 9. 1.5B Model Capability Boundaries

Design every feature around these hard limits:

| CAN do reliably | CANNOT do reliably |
|----------------|-------------------|
| Single-turn reasoning with structured context | Multi-step causal chains >5 steps |
| Choose between 2-4 typed options | Nuanced sarcasm/dark humor detection |
| Summarize episodic → semantic with clear format | Long-horizon planning >10 steps |
| Detect tone shift in <200 token window | Reliable JSON without GBNF grammar |
| Generate natural reply from OCEAN/VAD numbers | Parallel tracking of >3 active goals |

**Consequence:** Every complex task is decomposed into single-turn LLM decisions. The ReAct loop is mandatory for multi-step execution. Not optional. Not an escalation path — the default path.

---

## 10. Architectural Prohibitions (never do these)

1. **No format string theater.** `format!("PROACTIVE_TRIGGER:goal_stalled | goal_id:{}", id)` stuffed into `Converse.context.user_text` is forbidden. Use typed IPC variants.

2. **No stubs as handlers.** `tracing::debug!("embedding received")` as the entire match arm for `NeocortexToDaemon::Embedding` is dead code. Either implement it or `todo!()` with an architectural comment.

3. **No LLM bypass.** Any Rust function that produces user-facing text without calling the LLM is forbidden (except ETG cache hits, which are LLM-generated responses stored verbatim).

4. **No allow_all() in production.** `PolicyGate::allow_all()` is `#[cfg(test)]` only. Every production policy decision is real.

5. **No silent proactive.** All 6 proactive systems must send typed `ProactiveContext` to neocortex. None of them generates text in Rust.

---

## 11. File Ownership Map (for parallel agent dispatch)

| Domain | Primary files | Notes |
|--------|-------------|-------|
| IPC types | `aura-types/src/ipc.rs` | Add ProactiveContext variant here |
| Main loop + proactive | `daemon_core/main_loop.rs`, `daemon_core/proactive_dispatcher.rs` | Agent 1 ONLY |
| Personality + identity | `identity/prompt_personality.rs`, `identity/affective.rs`, `identity/user_profile.rs` | Agent 2 ONLY |
| ReAct execution | `daemon_core/react.rs` | Agent 3 ONLY |
| Error handling | `daemon_core/` (except main_loop.rs) | Agent 4 ONLY |
| Cron / integrations | `daemon_core/main_loop.rs` (handle_cron_tick) | Agent 1, phase 2 |

**Never assign same file to two agents. Conflicts corrupt the codebase.**

---

*Last updated: 2026-03-12. Every future agent dispatched on AURA must read sections 1–10 before writing a single line.*
