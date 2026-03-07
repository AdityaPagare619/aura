# AURA Execution Flow: User Input to Response

Complete trace of how AURA processes a user command from receipt to completion.

## Overview

```
User Input → CommandParser → Amygdala → PolicyGate → Contextor → RouteClassifier
  → System1 (ETG cache) or System2 (Neocortex LLM)
  → ReAct Engine (execute actions via AccessibilityService)
  → Response to User
```

## Step-by-Step Trace

### Phase 1: Event Ingestion

**File:** `daemon_core/main_loop.rs`

The main loop runs a `tokio::select!` over 8 channels:

| Channel | Source | Example Events |
|---------|--------|---------------|
| `a11y_rx` | AccessibilityService | Screen changed, element focused |
| `notification_rx` | NotificationListener | New notification arrived |
| `user_command_rx` | UI / voice input | "Open WhatsApp and message John" |
| `ipc_outbound_rx` | Internal subsystems | Neocortex request ready to send |
| `ipc_inbound_rx` | Neocortex process | Plan ready, conversation reply |
| `db_write_rx` | Memory subsystem | Batch write to SQLite |
| `cron_tick_rx` | Scheduler | Timed task trigger |
| `response_rx` | Execution engine | Action completed, result ready |

Plus:
- Periodic checkpoint timer (saves DaemonState)
- 100ms cancel-flag polling (user cancellation)

When `user_command_rx` fires, the pipeline begins.

### Phase 2: Parsing

**File:** `daemon_core/main_loop.rs`

```
Raw input string
    │
    ▼
CommandParser.parse()
    │
    ▼
ParsedEvent {
    intent: String,
    entities: Vec<Entity>,
    raw: String,
    timestamp: Instant,
}
```

The CommandParser extracts structured intent and entities from natural language input.

### Phase 3: Safety Evaluation

**Files:** `policy/gate.rs`, `identity/ethics.rs`

```
ParsedEvent
    │
    ▼
Amygdala.score(event)          // Urgency/threat assessment
    │
    ▼
PolicyGate.check_action(event) // Layer 1: configurable rules
    │                            //   glob match, first-match-wins
    │                            //   → Allow / Deny / Audit / Confirm
    ▼
Identity Ethics check           // Layer 2: hardcoded blocks
    │                            //   blocked_patterns[] → absolute deny
    │                            //   audit_keywords[] → always log
    ▼
PolicyDecision { effect, rule_id, reason }
```

If `Deny`: respond with explanation, pipeline stops.  
If `Confirm`: pause execution, ask user to approve, resume on confirmation.  
If `Audit`: log the action, continue.  
If `Allow`: continue.

### Phase 4: Context Enrichment

**File:** `daemon_core/main_loop.rs`

```
Contextor.enrich(event, memory)
    │
    ├── Working memory: current task context, recent screens
    ├── Episodic memory: recent similar interactions  
    ├── Semantic memory: user preferences, app knowledge
    └── HNSW vector search: semantically similar past events
    
    ▼
EnrichedEvent {
    parsed: ParsedEvent,
    context: Vec<MemoryItem>,
    user_prefs: UserPreferences,
    screen_state: ScreenSnapshot,
}
```

### Phase 5: Routing

**File:** `routing/classifier.rs`

```
RouteClassifier.classify(enriched_event)
    │
    ├── System1    → ETG cache hit, confidence ≥ 0.70
    ├── System2    → Novel task, needs LLM planning
    ├── DaemonOnly → Internal bookkeeping (checkpoint, cron)
    └── Hybrid     → Partial cache hit, may need LLM assist
```

### Phase 6a: System1 Fast Path

**File:** `routing/system1.rs`

```
System1 dispatch
    │
    ▼
Plan cache lookup (max 256 entries, threshold 0.70)
    │
    ├── Cache hit ──► ActionPlan { source: EtgPath, steps: [...] }
    │                    │
    │                    ▼
    │               ReAct Engine (Phase 7)
    │
    └── Cache miss ─► ETG BFS pathfinding (max depth 20)
         │
         ├── Path found (reliability > 0.70) ──► Cache + Execute
         │
         └── No path ──► Escalate to System2
```

Also handles simple ack patterns (confirmations, dismissals) without ETG lookup.

### Phase 6b: System2 Slow Path

**File:** `routing/system2.rs`

```
System2 dispatch
    │
    ▼
Create pending request (tracked for timeout/cancellation)
    │
    ▼
IPC message to Neocortex ──► Abstract Unix socket
    │
    ▼
Neocortex receives context + enriched event
    │
    ▼
LLM inference (llama.cpp, 1-30s)
    │
    ▼
IPC response: PlanReady { steps: [...] } or ConversationReply { text }
    │
    ▼
Main loop ipc_inbound_rx handler
    │
    ├── PlanReady ──► ActionPlan { source: LlmGenerated } ──► ReAct Engine
    │
    └── ConversationReply ──► Anti-sycophancy gate (Pass/Nudge/Block)
                                  │
                                  └──► Response to user
```

IPC inbound also handles: `Loaded`, `LoadFailed`, `Error`, `MemoryWarning`, `TokenBudgetExhausted`.

### Phase 7: Execution (ReAct Engine)

**File:** `daemon_core/react.rs`

```
ActionPlan received
    │
    ▼
Determine ExecutionMode:
    ├── Dgs (Document-Guided Scripting) ── template-matched, deterministic
    └── SemanticReact ── observation-action loop, adaptive
    
    │
    ▼
AgenticSession {
    max_iterations: 10,
    max_consecutive_failures: 5,
    history_bound: 32,
}
    │
    ▼
┌─► Execute next step
│       │
│       ▼
│   Selector cascade (L0-L7) to find target element
│       │
│       ▼
│   Perform action (tap, type, scroll, swipe, etc.)
│       │
│       ▼
│   Observe result (screen changed? element found?)
│       │
│       ▼
│   Reflection scoring:
│       score = base_confidence
│             + screen_change_bonus
│             + element_bonus
│             - strategy_penalty
│       │
│       ├── Success ──► Record in ETG, reinforce Hebbian pattern (+0.10)
│       │               Next step (or complete if last step)
│       │
│       └── Failure ──► Weaken Hebbian pattern (-0.15)
│                       Strategy escalation (monotonic):
│                       Direct → Exploratory → Cautious → Recovery
│                       │
│                       Mid-exec escalation:
│                       DgsSuccess → RetryAdjusted → Brainstem → FullNeocortex
│                       │
└───────────────────────┘ (loop until complete, max iters, or max failures)

    │
    ▼
Execution Result → response_rx → Main Loop → User
```

### Phase 8: Post-Execution

After successful execution:
1. ETG edges reinforced (reliability increases)
2. Successful plan cached in System1 plan cache (if from System2)
3. Episodic memory entry created (what was done, when, result)
4. Pattern engine updated (Hebbian +0.10 for each successful step)

After failed execution:
1. ETG edges weakened (reliability decreases)
2. Pattern engine updated (Hebbian -0.15)
3. Error→learning feedback via MemoryIntelligence
4. Episodic memory entry created (what failed, why)

## Timing Budget (Typical)

| Phase | System1 | System2 |
|-------|---------|---------|
| Parse | <5ms | <5ms |
| Safety check | <1ms | <1ms |
| Context enrichment | 5-15ms | 5-15ms |
| Routing | <2ms | <2ms |
| Plan retrieval | <10ms (cache) | 1-30s (LLM) |
| Execution per step | 50-500ms | 50-500ms |
| **Total (single step)** | **~70ms** | **~1-30s** |
