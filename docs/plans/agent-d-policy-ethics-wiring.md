# Agent D: Policy/Ethics Wiring Plan

> Wire PolicyGate, TRUTH framework, anti-sycophancy guard, manipulation check,
> and emergency stop into the AURA daemon's execution and response paths.

## Current State (What's Already Wired)

### ✅ PolicyGate in Execution Path (react.rs)
- `PolicyContext` struct (lines 93-175) bundles `PolicyGate` + `AuditLog`
- `execute_dgs()` checks policy gate before Executor (line 1173-1211)
- `execute_semantic_react()` checks policy gate per-action (line 1454-1482)
- `execute_task()` standalone accepts `Option<&mut PolicyContext>`

### ✅ Anti-Sycophancy in Response Path (main_loop.rs)
- `handle_ipc_inbound` → `ConversationReply` → `subs.identity.check_response()` (line 1213)
- Gate result: Pass / Nudge (append note) / Block (fallback message)

### ✅ Identity PolicyGate in User Command Path (main_loop.rs)
- `handle_user_command()` → `subs.identity.policy_gate.check_action()` (line 636)
- Block / Audit / Allow handling

## Gaps to Fill

### Gap 1: TRUTH Framework Not Called
`TruthFramework::validate_response()` (ethics.rs:363-452) exists but is NEVER
called. Must be called in the response path BEFORE anti-sycophancy gate.

**Action:** In `handle_ipc_inbound` → `ConversationReply` handler, add TRUTH
validation before the anti-sycophancy gate.

### Gap 2: Manipulation Check Not Called
`check_manipulation()` (ethics.rs:518-566) exists but is never called.

**Action:** In `handle_user_command()`, add manipulation check on user input
BEFORE the existing policy gate check. Log audit entries for suspicious/
manipulative verdicts.

### Gap 3: Emergency System Not Wired
`EmergencyStop` (emergency.rs) has full state machine, anomaly detector,
watchdog — but nothing calls it.

**Action:**
1. Add `EmergencyStop` to `LoopSubsystems`
2. Call `emergency.heartbeat()` on each main-loop tick
3. Call `emergency.check_and_trigger()` periodically (cron tick)
4. Check `emergency.actions_allowed()` before execution in `handle_ipc_inbound` PlanReady handler
5. Check `AnomalyDetector::check_user_stop_phrase()` in user command input

### Gap 4: TruthFramework Not Owned by Any Active Struct
`TruthFramework` exists in ethics.rs but isn't owned by `IdentityEngine` or
`LoopSubsystems`. Need to add it to `IdentityEngine`.

**Action:** Add `truth_framework: TruthFramework` to `IdentityEngine` and
expose a `validate_response(&self, text: &str) -> TruthValidation` method.

## Implementation Plan

### File: `identity/mod.rs` — Add TruthFramework to IdentityEngine
- Add `truth_framework: TruthFramework` field
- Add `validate_response()` method
- Add `check_manipulation()` delegation method
- Import `TruthFramework` and `TruthValidation`

### File: `daemon_core/main_loop.rs` — Wire into response + input paths
1. Add `EmergencyStop` field to `LoopSubsystems`
2. In `ConversationReply` handler: add TRUTH validation before anti-sycophancy
3. In `handle_user_command()`: add manipulation check before policy gate
4. In `PlanReady` handler: check `emergency.actions_allowed()` before execution
5. In the main `run()` loop: add heartbeat + periodic emergency check
6. Wire user stop phrase detection in `handle_user_command()`

### File: `policy/wiring.rs` — New bridge module (tests only)
- `SafetyPipeline` struct for ergonomic testing
- Unit tests for the wiring paths

### File: `policy/mod.rs` — Register wiring module
- Add `pub mod wiring;`

## Tests (15+ planned)

1. TRUTH validation blocks deceptive response
2. TRUTH validation passes clean response
3. TRUTH validation handles edge cases (empty, very long)
4. Anti-sycophancy + TRUTH combined pipeline
5. Manipulation check detects emotional manipulation
6. Manipulation check detects authority abuse
7. Manipulation check passes clean input
8. Emergency stop activates on manual trigger
9. Emergency stop blocks actions when activated
10. Emergency stop allows actions when normal
11. Emergency heartbeat + watchdog integration
12. Emergency user stop phrase detection
13. IdentityEngine.validate_response() delegates correctly
14. IdentityEngine.check_manipulation() delegates correctly
15. SafetyPipeline full flow test
16. TRUTH + sycophancy combined in response pipeline
17. Manipulation + policy gate combined in input pipeline

## Constraints
- No `.unwrap()` — use `Result<T, AuraError>` everywhere
- Use `tracing::{info, warn, error, debug}` for logging
- Doc comments on all public functions
- DO NOT modify internals of policy or identity modules
- Only add wiring/bridge code
