# Agent 13 — ARC Subsystem Wiring Plan

## Goal
Wire the ARC subsystems (AffectiveEngine, BDI GoalScheduler, ProactiveEngine, PersonalityEngine) into the main event loop (`main_loop.rs`). These subsystems are fully built but NOT plugged into the loop.

## Constraints
- NO `.unwrap()` — use `Result<T, AuraError>`
- Use `tracing` for all logging
- Doc comments on public functions
- Don't break Agent E's Contextor wiring
- Do NOT modify ARC module internals — only add wiring in `main_loop.rs`
- Currently 1931 tests passing, 0 failing — must maintain

## Architecture
- `DaemonState.subsystems: SubSystems` holds `bdi_scheduler`, `goal_tracker`, `goal_decomposer`, `arc` (all `Option<T>`)
- `LoopSubsystems` is a **separate** struct constructed in `main_loop.rs` — does NOT use `DaemonState.subsystems`
- Key gap: `LoopSubsystems` has no ARC fields. Must bridge them.

## Changes

### 1. Wire AffectiveEngine into Mood Updates
**Replace inline EWMA** in 3 handlers with `AffectiveEngine` calls:

- `handle_a11y_event` (lines 454-475): Replace manual arousal EWMA with `affective.process_event(MoodEvent, now_ms)`
- `handle_notification_event` (lines 524-538): Replace manual valence/arousal nudge
- `handle_user_command` (lines 582-594): Replace manual arousal/valence nudge for Chat

Map existing events → `MoodEvent` variants:
- A11y EmergencyBypass/InstantWake → `MoodEvent::UserFrustrated` (high urgency)
- A11y SlowAccumulate → no mood event (low salience)
- A11y Suppress → `MoodEvent::Silence { duration_ms: 0 }` (decay)
- Notification high-importance → `MoodEvent::UserHappy` or contextual
- Chat with positive voice biomarker → `MoodEvent::UserHappy`
- Chat with negative voice biomarker → `MoodEvent::UserFrustrated`
- TaskSucceeded/Failed → from react outcome in task request handler

### 2. Wire BDI Scheduler + GoalTracker
**Replace** `state.checkpoint.goals.push(goal)` in `TaskRequest` handler with:
1. Create `ScoredGoal` from the goal
2. `bdi_scheduler.base.enqueue(scored_goal)`
3. `goal_tracker.track(goal)`
4. `goal_tracker.activate(goal_id, now_ms)` when execution starts
5. `goal_tracker.complete(goal_id, now_ms)` or `fail(goal_id, reason, now_ms)` on outcome

Keep `state.checkpoint.goals` as the source of truth for backward compat, but mirror into BDI/tracker.

### 3. Wire ProactiveEngine::tick()
In `handle_cron_tick`, add a new branch for `proactive_tick` job name:
- Call `proactive_engine.tick(now_ms, power_tier, context_mode)`
- Process returned `ProactiveAction` list (suggestions → response channel, automations → react engine)

### 4. Wire Personality into Response Generation
In `dispatch_system2`, compute `PersonalityInfluence` and thread into the context:
- `personality.compute_influence(mood, relationship_stage, trust)`
- Apply `routing_bias` to classifier
- Apply `response_params` to System2 context

### 5. Write 15+ Tests
- AffectiveEngine wiring tests (mood updates via events)
- BDI scheduler wiring tests (goal lifecycle)
- ProactiveEngine tick wiring tests
- Personality influence tests
- Degraded-mode tests (all Option<T> = None)

## File Changes
- `crates/aura-daemon/src/daemon_core/main_loop.rs` — ALL wiring changes
- `docs/plans/agent-13-arc-wiring.md` — this plan
