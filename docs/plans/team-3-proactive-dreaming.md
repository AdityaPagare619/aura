# TEAM 3: ProactiveEngine + Dreaming Implementation Plan

## Overview
This plan addresses wiring ProactiveEngine::tick() into the main loop and implementing real Dreaming consolidation for the AURA daemon.

## Phase 1: ProactiveEngine Wiring

### Current State
- `ProactiveEngine::tick()` exists in `proactive/mod.rs:161-277`
- Main loop already has code to call proactive tick (line 2267-2362 in main_loop.rs)
- Code checks for "proactive_tick" job name but no cron job with that name exists

### Tasks
1. **Add ProactiveTick cron job** to `cron.rs`:
   - Job ID: `CronJobId::ProactiveTick = 32`
   - Interval: 300 seconds (5 minutes)
   - Power tier: P1IdlePlus
   - Name: "proactive_tick"

2. **Verify suggestion feedback → Hebbian learning wiring**:
   - When suggestion is accepted/rejected, call `suggestions.record_feedback()`
   - This is already implemented in suggestions.rs

### Changes Required
- `crates/aura-daemon/src/arc/cron.rs`: Add CronJobId::ProactiveTick and job definition

## Phase 2: Dreaming Implementation

### Current State
- `DreamingEngine` exists in `learning/dreaming.rs` with phases defined
- 5 phases: Maintenance, EtgVerification, Exploration, Annotation, Cleanup
- Conditions check: charging + screen off + battery > 30% + thermal nominal

### Tasks
1. **Add 4-stage consolidation cycle within Dreaming phases**:
   - SENSORIMOTOR: Load ETG traces into working memory
   - CONSOLIDATION: Strengthen successful pathways, prune weak ones
   - REPLAY: Replay successful traces to strengthen pathways
   - AWAKE: Generate insights from pattern analysis

2. **Implement pathway pruning logic**:
   - Track success rate per ETG trace
   - Delete traces with <10% success rate

3. **Implement memory consolidation in dreaming**:
   - Move old episodic memories → archive
   - Generate insights from patterns

4. **Wire Dreaming into main loop**:
   - Check dreaming conditions on each cron tick
   - Start dreaming session when conditions met
   - Run through all phases

### Changes Required
- `crates/aura-daemon/src/arc/learning/dreaming.rs`: Add consolidation logic
- `crates/aura-daemon/src/daemon_core/main_loop.rs`: Wire dreaming trigger

## Phase 3: Tests

### Required Tests (25+)
1. ProactiveEngine tick produces actions when triggers registered
2. Morning briefing triggered at correct hour
3. Suggestion evaluation with feedback learning
4. Dreaming conditions detection
5. Dreaming phase transitions
6. Pathway pruning (success rate <10%)
7. Memory consolidation during dreaming
8. Cron job registration
9. Integration tests for wiring

## Files to Modify
1. `crates/aura-daemon/src/arc/cron.rs` - Add proactive tick job
2. `crates/aura-daemon/src/arc/learning/dreaming.rs` - Add consolidation logic
3. `crates/aura-daemon/src/daemon_core/main_loop.rs` - Wire dreaming

## Verification
- `cargo check` → 0 errors
- `cargo test -p aura-daemon --lib` → no new failures
