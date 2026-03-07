# Close V4 Intelligence Gap — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Wire the existing planner + executor + IPC types together so V4's daemon core
has a functional intelligence pipeline: user intent → neocortex → plan → execute → learn → respond.

**Architecture:** The infrastructure already exists in isolation. The planner, executor, IPC types
(`DaemonToNeocortex`, `NeocortexToDaemon`, `ContextPackage`, `FailureContext`), and channels are
all well-built. The gap is purely **wiring** — 13 stub handlers in `main_loop.rs` that log and
discard instead of routing to the execution pipeline. We add an `Executor` and `ActionPlanner`
to `DaemonState`, wire the PlanReady handler to invoke the executor, and add IPC send paths for
chat and replan requests.

**Tech Stack:** Rust (tokio async), aura-types (shared IPC/ETG types), rusqlite, bincode, tracing

**Key constraint:** `DaemonState.db` is `rusqlite::Connection` which is `!Sync`, so the executor
must run on the same task as the main loop (no `tokio::spawn` with state borrows). We use
`tokio::select!` biased branching with an internal execution channel.

---

## Task 1: Add Executor + Planner to DaemonState

**Files:**
- Modify: `crates/aura-daemon/src/daemon_core/startup.rs:33-42` (DaemonState struct)
- Modify: `crates/aura-daemon/src/daemon_core/startup.rs` (startup phases)
- Modify: `crates/aura-daemon/src/daemon_core/main_loop.rs:19-21` (imports)

**What:** Add `executor: Executor` and `planner: ActionPlanner` fields to `DaemonState`.
Initialize them during startup phase 4 (state restore) with the configured safety level.

### Step 1: Add fields to DaemonState

In `startup.rs:33-42`, add two new fields:

```rust
// After existing fields in DaemonState:
use crate::execution::executor::Executor;
use crate::execution::planner::ActionPlanner;

pub struct DaemonState {
    pub channels: DaemonChannels,
    pub db: Connection,
    pub checkpoint: DaemonCheckpoint,
    pub config: AuraConfig,
    pub startup_time_ms: u64,
    pub cancel_flag: Arc<AtomicBool>,
    // NEW: Execution pipeline
    pub executor: Executor,
    pub planner: ActionPlanner,
}
```

### Step 2: Initialize in startup

In the startup function, after checkpoint restore (phase 4), initialize:

```rust
// Phase 4b: Initialize execution pipeline
let executor = Executor::normal();  // TODO: read safety level from config
let planner = ActionPlanner::new();
```

And pass them into the DaemonState constructor.

### Step 3: Fix any compilation errors

Run: `cargo check -p aura-daemon`

The `DaemonState` is constructed in startup.rs and used in test helpers.
Ensure all construction sites include the new fields.

### Step 4: Commit

```bash
git add -A && git commit -m "feat(daemon): add Executor + ActionPlanner to DaemonState"
```

---

## Task 2: Wire PlanReady → Executor

**Files:**
- Modify: `crates/aura-daemon/src/daemon_core/main_loop.rs:519-530` (PlanReady handler)
- Modify: `crates/aura-daemon/src/daemon_core/main_loop.rs:19-21` (imports)

**What:** When `NeocortexToDaemon::PlanReady { plan }` arrives, validate the plan
using `planner.validate_plan()`, then execute it via `executor.execute()`.
On completion, log the outcome and record it. The executor needs a `ScreenProvider` —
for now, use a `MockScreenProvider` that captures a real screen tree via JNI
(or a test stub).

### Step 1: Add imports to main_loop.rs

```rust
use crate::execution::executor::ExecutionOutcome;
use crate::screen::actions::MockScreenProvider;
use aura_types::etg::ActionPlan;
```

### Step 2: Implement PlanReady handler

Replace the stub at lines 519-530 with:

```rust
NeocortexToDaemon::PlanReady { plan } => {
    let step_count = plan.steps.len();
    tracing::info!(steps = step_count, "action plan received from neocortex");

    // Guard: reject absurdly large plans
    if step_count > 100 {
        tracing::warn!(steps = step_count, "plan too large — rejecting");
        return Ok(());
    }

    // Guard: reject empty plans (LLM placeholder)
    if plan.steps.is_empty() {
        tracing::warn!("received empty plan from neocortex — ignoring");
        return Ok(());
    }

    // Validate plan against active goal (if any)
    if let Some(goal) = state.checkpoint.goals.first() {
        if let Err(errors) = state.planner.validate_plan(&plan, goal) {
            tracing::warn!(?errors, "plan validation failed — rejecting");
            return Ok(());
        }
    }

    // Execute the plan
    // NOTE: MockScreenProvider is a temporary stand-in.
    // Real implementation will use the JNI-backed ScreenProvider.
    let screen = MockScreenProvider::new();
    match state.executor.execute(&plan, &screen).await {
        Ok(ExecutionOutcome::Success { steps_executed, elapsed_ms }) => {
            tracing::info!(
                steps_executed,
                elapsed_ms,
                "plan executed successfully"
            );
            // TODO Task 3: record_outcome for template learning
        }
        Ok(ExecutionOutcome::Failed { step, reason, elapsed_ms }) => {
            tracing::warn!(
                step,
                reason = %reason,
                elapsed_ms,
                "plan execution failed"
            );
            // TODO Task 2b: trigger replan via LLM
        }
        Ok(ExecutionOutcome::CycleDetected { step, tier }) => {
            tracing::warn!(step, tier, "cycle detected during execution");
            // TODO Task 5: replan on strategic cycle
        }
        Ok(ExecutionOutcome::Cancelled { step, elapsed_ms }) => {
            tracing::info!(step, elapsed_ms, "plan execution cancelled");
        }
        Err(e) => {
            tracing::error!(error = %e, "executor error");
        }
    }
}
```

### Step 3: Verify compilation

Run: `cargo check -p aura-daemon`

### Step 4: Commit

```bash
git add -A && git commit -m "feat(daemon): wire PlanReady handler to executor pipeline"
```

---

## Task 3: Call planner.record_outcome() from executor results

**Files:**
- Modify: `crates/aura-daemon/src/daemon_core/main_loop.rs` (PlanReady handler, continuation from Task 2)

**What:** After `executor.execute()` returns, if the plan came from a template
(`plan.source == PlanSource::Hybrid`), find the template index and call
`planner.record_outcome(idx, success)`. This closes the learning loop so
template confidence scores evolve with real outcomes.

### Step 1: Add template outcome recording

In the `PlanReady` handler, after the match on `ExecutionOutcome`, add:

```rust
// Record outcome for template learning
use aura_types::etg::PlanSource;

let success = matches!(&outcome, ExecutionOutcome::Success { .. });
if plan.source == PlanSource::Hybrid {
    // Find the template that generated this plan by matching goal description.
    // This is a heuristic — in the future, ActionPlan should carry template_idx.
    if let Some(idx) = state.planner.find_template_for_goal(&plan.goal_description) {
        state.planner.record_outcome(idx, success);
    }
}
```

### Step 2: Add helper method to ActionPlanner

In `planner.rs`, add:

```rust
/// Find the index of the best-matching template for a goal description.
/// Returns `None` if no template matches.
pub fn find_template_for_goal(&self, description: &str) -> Option<usize> {
    let desc_lower = description.to_lowercase();
    self.templates
        .iter()
        .enumerate()
        .filter(|(_, t)| desc_lower.contains(&t.trigger_pattern.to_lowercase()))
        .max_by(|a, b| a.1.confidence.partial_cmp(&b.1.confidence).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(idx, _)| idx)
}
```

### Step 3: Verify compilation

Run: `cargo check -p aura-daemon`

### Step 4: Run tests

Run: `cargo test -p aura-daemon -- planner`

### Step 5: Commit

```bash
git add -A && git commit -m "feat(planner): close template learning loop with record_outcome"
```

---

## Task 4: Implement UserCommand::Chat → Neocortex IPC

**Files:**
- Modify: `crates/aura-daemon/src/daemon_core/main_loop.rs:352-375` (Chat handler)
- Modify: `crates/aura-daemon/src/daemon_core/channels.rs` (need IPC outbound sender access)

**What:** When a user sends a Chat message, build a `ContextPackage` with the conversation
text and send `DaemonToNeocortex::Converse` over the IPC outbound channel. The reply
comes back asynchronously via `NeocortexToDaemon::ConversationReply` (already handled at line 532).

### Problem: Channel access

The main loop drops senders at line 54: `drop(senders)`. This means we can't send IPC outbound
from within handlers. **Solution:** Keep the `ipc_outbound_tx` sender alive by extracting it
before the drop, or add it to `DaemonState`.

### Step 1: Preserve IPC outbound sender

In `main_loop.rs:run()`, before `drop(senders)`, extract the IPC outbound sender:

```rust
let channels = std::mem::take(&mut state.channels);
let (senders, mut rxs) = channels.split();
// Keep the IPC outbound sender for handlers that need to send to neocortex
let ipc_tx = senders.ipc_outbound_tx.clone();
drop(senders);
```

Or better: add `ipc_outbound_tx: Option<IpcOutboundTx>` to `DaemonState` and populate it
during the split, so all handlers have access.

### Step 2: Build ContextPackage and send

Replace the Chat stub at lines 352-375:

```rust
UserCommand::Chat { text } => {
    if text.trim().is_empty() {
        tracing::warn!("ignoring empty chat message");
        return Ok(());
    }

    let text = if text.len() > 4096 {
        tracing::warn!(len = text.len(), "truncating oversized chat message");
        text[..4096].to_string()
    } else {
        text
    };

    // Boost disposition arousal slightly
    state.checkpoint.disposition.mood.arousal =
        (state.checkpoint.disposition.mood.arousal + 0.1).clamp(-1.0, 1.0);

    // Build context package for neocortex
    let mut ctx = ContextPackage::default();
    ctx.conversation_history.push(ConversationTurn {
        role: Role::User,
        content: text.clone(),
        timestamp_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    });
    ctx.inference_mode = InferenceMode::Conversational;
    ctx.personality = PersonalitySnapshot::from(&state.checkpoint.personality);

    // Set active goal context if any
    if let Some(goal) = state.checkpoint.goals.first() {
        ctx.active_goal = Some(GoalSummary {
            description: goal.description.clone(),
            current_step: format!("step {}", goal.steps.len()),
            blockers: Vec::new(),
        });
    }

    // Serialize and send over IPC
    let msg = DaemonToNeocortex::Converse { context: ctx };
    let payload = bincode::serde::encode_to_vec(&msg, bincode::config::standard())
        .map_err(|e| format!("bincode encode error: {e}"))?;

    // Send via IPC outbound channel
    // (requires ipc_outbound_tx to be available — see Step 1)
    if let Some(ref tx) = state.ipc_outbound_tx {
        let _ = tx.send(IpcOutbound { payload }).await;
        tracing::info!(len = text.len(), "chat message sent to neocortex");
    } else {
        tracing::warn!("no IPC outbound channel — cannot send chat to neocortex");
    }
}
```

### Step 3: Verify compilation

Run: `cargo check -p aura-daemon`

### Step 4: Commit

```bash
git add -A && git commit -m "feat(daemon): wire Chat command to neocortex IPC"
```

---

## Task 5: Add Replan Path on Execution Failure + Strategic Cycle

**Files:**
- Modify: `crates/aura-daemon/src/daemon_core/main_loop.rs` (PlanReady handler failure arms)
- Modify: `crates/aura-daemon/src/execution/executor.rs:488-493` (Strategic cycle handler)

**What:** When the executor fails or detects a strategic cycle, send
`DaemonToNeocortex::Replan` with a `FailureContext` describing what went wrong.
The neocortex will respond with a new `PlanReady` message, creating a feedback loop.

### Step 1: Build FailureContext from execution failure

In the `PlanReady` handler's `Failed` arm:

```rust
Ok(ExecutionOutcome::Failed { step, reason, elapsed_ms }) => {
    tracing::warn!(step, reason = %reason, elapsed_ms, "plan execution failed");

    // Build failure context for replan
    let failure = FailureContext {
        task_goal_hash: hash_goal_description(&plan.goal_description),
        current_step: step,
        failing_action: 0, // TODO: extract from step
        target_id: 0,
        expected_state_hash: 0,
        actual_state_hash: 0,
        tried_approaches: 1,
        last_3_transitions: [TransitionPair::default(); 3],
        error_class: 1,  // Generic failure
    };

    // Send replan request to neocortex
    let mut ctx = ContextPackage::default();
    ctx.inference_mode = InferenceMode::Planner;
    if let Some(goal) = state.checkpoint.goals.first() {
        ctx.active_goal = Some(GoalSummary {
            description: goal.description.clone(),
            current_step: format!("step {step} failed: {reason}"),
            blockers: vec![reason.clone()],
        });
    }

    let msg = DaemonToNeocortex::Replan {
        context: ctx,
        failure,
    };
    send_to_neocortex(state, msg).await;
}
```

### Step 2: Similarly for CycleDetected arm

```rust
Ok(ExecutionOutcome::CycleDetected { step, tier }) => {
    tracing::warn!(step, tier, "cycle detected during execution");

    if tier >= 2 {  // Strategic or higher
        let failure = FailureContext {
            task_goal_hash: hash_goal_description(&plan.goal_description),
            current_step: step,
            failing_action: 0,
            target_id: 0,
            expected_state_hash: 0,
            actual_state_hash: 0,
            tried_approaches: tier as u64,
            last_3_transitions: [TransitionPair::default(); 3],
            error_class: 2,  // Cycle
        };

        let mut ctx = ContextPackage::default();
        ctx.inference_mode = InferenceMode::Strategist;
        let msg = DaemonToNeocortex::Replan { context: ctx, failure };
        send_to_neocortex(state, msg).await;
    }
}
```

### Step 3: Add helper function `send_to_neocortex`

```rust
/// Serialize and send a message to the neocortex over IPC outbound.
async fn send_to_neocortex(state: &DaemonState, msg: DaemonToNeocortex) {
    let payload = match bincode::serde::encode_to_vec(&msg, bincode::config::standard()) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "failed to encode neocortex message");
            return;
        }
    };

    if let Some(ref tx) = state.ipc_outbound_tx {
        if let Err(e) = tx.send(IpcOutbound { payload }).await {
            tracing::error!(error = %e, "failed to send to neocortex IPC channel");
        }
    } else {
        tracing::warn!("no IPC outbound channel available");
    }
}
```

### Step 4: Add `hash_goal_description` helper

```rust
/// Simple FNV-1a hash of a goal description string for FailureContext.
fn hash_goal_description(desc: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in desc.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
```

### Step 5: Verify compilation

Run: `cargo check -p aura-daemon`

### Step 6: Run full test suite

Run: `cargo test -p aura-daemon`

### Step 7: Commit

```bash
git add -A && git commit -m "feat(daemon): add replan loop on execution failure and strategic cycle"
```

---

## Dependency Graph

```
Task 1 (DaemonState fields) ─┬─→ Task 2 (PlanReady→Executor) ─→ Task 3 (record_outcome)
                              │                                  ↓
                              └─→ Task 4 (Chat→IPC) ────────→ Task 5 (Replan loop)
```

Tasks 2 and 4 can be done in parallel after Task 1.
Task 3 depends on Task 2.
Task 5 depends on Tasks 2 and 4 (needs both executor wiring and IPC send capability).

## V3 Parity Checklist After All 5 Tasks

| V3 Feature | V4 Status After | Notes |
|---|---|---|
| ReAct loop (LLM decides next) | ~40% | Have replan-on-failure, but not LLM-between-every-step |
| LLM-decides-when-to-stop | 20% | Executor still runs all steps; LLM only consulted on failure |
| Self-reflection after tool result | 30% | Replan sends failure context, but no per-step reflection |
| Meta-cognition / uncertainty | 10% | Monitor has invariants, but no uncertainty reasoning |
| Fallback chains | 70% | Executor has 5-strategy failure handling + replan path |
| Hebbian self-correction | 50% | record_outcome closes template learning loop |
| Neural-validated planning | 40% | Planner validates, LLM generates, but no revision loops |
| Tool orchestrator with risk | 80% | AntiBot + Monitor + CycleDetector + safety levels |

**Overall parity estimate after these 5 tasks: ~45%** (up from ~15%)

## Future Work (Not in This Plan)

1. **LLM-between-every-step** — After each step verification, send screen state to neocortex
   for "what should I do next?" instead of blindly following the plan
2. **Per-step reflection** — After verification, have LLM evaluate "did this achieve what I expected?"
3. **Uncertainty detection** — LLM outputs confidence scores; executor pauses if confidence drops
4. **Real ScreenProvider** — Replace MockScreenProvider with JNI-backed Android screen access
5. **IPC socket implementation** — Replace the stub `handle_ipc_outbound` with real socket writes
6. **Conversation memory** — Persist conversation history across restarts via checkpoint
