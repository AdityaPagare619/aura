# OutcomeBus Design — Synchronous Dispatch Hub

**Date:** 2026-03-09  
**Status:** Approved (autonomous decision)  
**Approach:** Synchronous Dispatch Hub (Approach A) using `std::mem::take` borrow-splitting

---

## Problem Statement

The main loop in `main_loop.rs` has 5 `react::execute_task()` call sites and 1 `System1::execute()` direct call. Only **site A** (TaskRequest handler, lines 1200-1310) has comprehensive post-processing (feedback loop, mood, goal tracking, episodic memory). Sites B-E have minimal to zero post-processing, meaning most execution outcomes are silently discarded.

This inconsistency means:
- The learning engine never sees outcomes from System1 plans, IPC plans, composed scripts, or proactive actions
- Memory consolidation only captures a fraction of what AURA does
- Identity/personality evolution only responds to user-facing task outcomes

## Design Decision: Why Synchronous Dispatch

The codebase has **zero `Arc<Mutex<>>`** patterns. All subsystems are owned directly on `LoopSubsystems` and passed via `&mut`. A broadcast-channel approach (Approach B) would require wrapping every subsystem in `Arc<Mutex<>>` — a massive refactor for the wrong abstraction.

The OutcomeBus solves a **consistency problem** (every outcome gets the same processing), not a decoupling problem. Synchronous dispatch fits perfectly.

## Architecture

### Module Structure

```
crates/aura-daemon/src/daemon_core/
├── outcome_bus.rs          # Bus + event types + subscriber trait
├── subscribers/
│   ├── mod.rs              # Re-exports + register_all()
│   ├── feedback_loop.rs    # Records errors to FeedbackLoop
│   ├── affective.rs        # Mood events on outcomes
│   ├── goal_tracker.rs     # Goal lifecycle updates
│   ├── episodic_memory.rs  # Stores outcomes as episodic memories
│   ├── learning.rs         # Feeds ARC learning engines (NEW)
│   └── audit.rs            # Audit trail logging
```

### Core Types

```rust
/// Where an execution outcome originated in the main loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeSource {
    TaskRequest,       // Site A — interactive user request
    System1Plan,       // Site B — System1 generated plan execution
    PlanReady,         // Site C — IPC inbound plan execution
    ComposedScript,    // Site D — composite script execution
    ProactiveAction,   // Site E — proactive/automation execution
    System1Direct,     // System1.execute() direct path (non-react)
}

/// Event published after every execution completion.
#[derive(Debug, Clone)]
pub struct OutcomeEvent {
    pub session_id: u64,
    pub task: String,
    pub outcome: TaskOutcome,
    pub mode: ExecutionMode,
    pub source: OutcomeSource,
    pub goal_id: Option<u64>,
    pub priority: Option<u32>,
    pub timestamp_ms: u64,
    pub disposition: f32,
    pub trust_score: f32,
}

/// Mutable context passed to subscribers during dispatch.
pub struct SubscriberContext<'a> {
    pub memory: &'a mut AuraMemory,
    pub identity: &'a mut IdentityEngine,
    pub arc_manager: &'a mut Option<ArcManager>,
    pub goal_tracker: &'a mut Option<GoalTracker>,
    pub audit_log: &'a mut AuditLog,
}

/// Trait for outcome subscribers.
pub trait OutcomeSubscriber: Send {
    fn on_outcome(&mut self, event: &OutcomeEvent, ctx: &mut SubscriberContext<'_>);
    fn name(&self) -> &str;
}

/// The bus itself.
#[derive(Default)]
pub struct OutcomeBus {
    subscribers: Vec<Box<dyn OutcomeSubscriber>>,
}

impl OutcomeBus {
    pub fn subscribe(&mut self, subscriber: Box<dyn OutcomeSubscriber>) { ... }
    pub fn publish(&mut self, event: &OutcomeEvent, ctx: &mut SubscriberContext<'_>) { ... }
    pub fn subscriber_count(&self) -> usize { ... }
}
```

### Borrow-Splitting Pattern (Critical)

Because `OutcomeBus` lives on `LoopSubsystems`, we cannot simultaneously borrow `&mut subs.outcome_bus` and `&mut subs.memory`. The solution is `std::mem::take`:

```rust
let mut bus = std::mem::take(&mut subs.outcome_bus);
let mut ctx = SubscriberContext {
    memory: &mut subs.memory,
    identity: &mut subs.identity,
    arc_manager: &mut subs.arc_manager,
    goal_tracker: &mut subs.goal_tracker,
    audit_log: &mut subs.audit_log,
};
bus.publish(&event, &mut ctx);
subs.outcome_bus = bus;
```

This is idiomatic Rust. `OutcomeBus` implements `Default` (empty subscriber list), so `take` leaves a valid empty bus. After publish, we restore the original.

**Helper function** to reduce boilerplate at each call site:

```rust
fn publish_outcome(subs: &mut LoopSubsystems, checkpoint: &Checkpoint, event: OutcomeEvent) {
    let mut bus = std::mem::take(&mut subs.outcome_bus);
    let mut ctx = SubscriberContext {
        memory: &mut subs.memory,
        identity: &mut subs.identity,
        arc_manager: &mut subs.arc_manager,
        goal_tracker: &mut subs.goal_tracker,
        audit_log: &mut subs.audit_log,
    };
    bus.publish(&event, &mut ctx);
    subs.outcome_bus = bus;
}
```

## Insertion Points

| # | Call Site | Line | Source Enum | Data Available |
|---|-----------|------|-------------|----------------|
| A | TaskRequest handler | ~1202 | TaskRequest | Full: goal_id, priority, session, outcome |
| B | System1 plan dispatch | ~1422 | System1Plan | Partial: session, outcome; no goal_id |
| C | PlanReady (IPC) | ~1879 | PlanReady | Partial: session, outcome; no goal_id |
| D | ComposedScript | ~2038 | ComposedScript | Partial: session, outcome; no goal_id |
| E | Proactive RunAutomation | ~2430 | ProactiveAction | Minimal: session, outcome |
| F | System1 direct execute | ~1391 | System1Direct | Minimal: outcome only |

## Subscribers

### 1. FeedbackLoopSubscriber
- **Triggers on:** Failed, CycleAborted
- **Action:** `memory.feedback_loop.record_error(task, reason)`
- **Currently exists at:** Site A only (lines 1226, 1250)

### 2. AffectiveSubscriber
- **Triggers on:** All outcomes
- **Action:** Maps outcome to mood event, calls `identity.affective.process_event_with_personality()`
- **Currently exists at:** Site A only (lines 1266-1274)

### 3. GoalTrackerSubscriber
- **Triggers on:** Success, Failed (when goal_id is Some)
- **Action:** `goal_tracker.complete(goal_id)` or `goal_tracker.fail(goal_id, reason)`
- **Currently exists at:** Site A only (lines 1284-1306)

### 4. EpisodicMemorySubscriber
- **Triggers on:** All outcomes
- **Action:** `memory.store_episodic(session_id, task, outcome_summary)`
- **Currently exists at:** Sites A and C (lines 1314, 1892)

### 5. LearningSubscriber (NEW)
- **Triggers on:** All outcomes
- **Action:** Feeds `arc_manager.learning` sub-engines with outcome data
- **Currently exists at:** NOWHERE — this is the primary new value

### 6. AuditSubscriber
- **Triggers on:** All outcomes
- **Action:** `audit_log.record_outcome(event)`

## Implementation Strategy

**Phase 1 (This PR):** ADDITIVE
- Add OutcomeBus with all 6 subscribers
- Wire `publish_outcome()` at all 6 call sites
- Do NOT remove existing post-processing at site A
- Sites B-E gain processing they never had
- Site A has some duplication (safe, verified by testing)

**Phase 2 (Follow-up):** MIGRATION
- Move site A's inline post-processing into subscribers
- Remove duplicate code from site A
- Add subscriber for metrics/telemetry

## Testing Plan

1. **Unit: OutcomeBus dispatch** — Register mock subscribers, publish events, verify all called in order
2. **Unit: Each subscriber** — Verify correct behavior for all 4 TaskOutcome variants
3. **Unit: SubscriberContext construction** — Verify borrow-splitting works
4. **Integration: Full loop** — Add a tracing subscriber, run through a TaskRequest, verify event published

## Verification Criteria

- [ ] `cargo build` compiles cleanly (no borrow errors)
- [ ] `cargo test` passes (existing + new)
- [ ] All 6 call sites have `publish_outcome()` calls
- [ ] Each subscriber handles all 4 TaskOutcome variants
- [ ] `std::mem::take` pattern is correct (bus restored after publish)
- [ ] No existing behavior is broken (site A still works identically)
