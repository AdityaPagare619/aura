# Team 3: ProactiveEngine + Dreaming Implementation Plan

## Current State Analysis

### ProactiveEngine
- ✅ `ProactiveEngine::tick()` is already wired into main_loop via `handle_cron_tick` 
- ✅ Morning briefings are triggered via `proactive_tick` cron job (every 5 min)
- ✅ Suggestion generation is working
- ❌ Suggestion acceptance → Hebbian learning NOT connected

### DreamingEngine  
- ✅ 4-stage consolidation already implemented: SENSORIMOTOR → CONSOLIDATION → REPLAY → AWAKE
- ✅ ETG trace management (replay successful, prune weak <10%)
- ✅ Session lifecycle (start, advance phases, abort, complete)
- ❌ NOT wired into main loop/cron
- ❌ No device conditions check from system

## Implementation Tasks

### 1. Connect Suggestion Acceptance → Hebbian Learning
**File**: `crates/aura-daemon/src/arc/learning/mod.rs`

Add method to LearningEngine:
```rust
/// Record suggestion feedback for Hebbian learning
pub fn record_suggestion_feedback(
    &mut self,
    suggestion_text: &str,
    category: DomainId,
    accepted: bool,
    now_ms: u64,
)
```

This creates Hebbian associations between:
- Suggestion category concept → "accepted" or "rejected" outcome
- Context from suggestion text → outcome

### 2. Wire DreamingEngine into Main Loop
**File**: `crates/aura-daemon/src/daemon_core/main_loop.rs`

Add handling for dreaming in `handle_cron_tick`:
- Check device conditions (charging, screen off, battery > 30%, thermal OK)
- Run dreaming session if conditions met
- Execute 4-stage consolidation cycle

### 3. Add Dreaming Cron Job  
**File**: `crates/aura-daemon/src/arc/cron.rs`

Add `CronJobId::DreamingTick` job that runs:
- Every 5 minutes when charging
- Only in P3Charging or P4DeepWork power tier

### 4. Add 25+ Tests for Dreaming Consolidation
**File**: `crates/aura-daemon/src/arc/learning/dreaming.rs`

Add tests for:
- Sensorimotor stage (loading ETG traces)
- Consolidation stage (strengthen/prune pathways)
- Replay stage (mental rehearsal)
- Awake stage (insight generation)
- Integration: full consolidation cycle

## Verification
- `cargo check` → 0 errors
- `cargo test` → 0 new failures

## Key Files to Modify
1. `crates/aura-daemon/src/arc/learning/mod.rs` - Add Hebbian feedback method
2. `crates/aura-daemon/src/daemon_core/main_loop.rs` - Wire Dreaming 
3. `crates/aura-daemon/src/arc/cron.rs` - Add DreamingTick job
4. `crates/aura-daemon/src/arc/learning/dreaming.rs` - Add tests
