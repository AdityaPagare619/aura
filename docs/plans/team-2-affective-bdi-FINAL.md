# TEAM 2: AffectiveEngine + BDI Scheduler Wiring Plan

## Current State Analysis

### AffectiveEngine
- **Status**: ALREADY WIRED in main_loop.rs
- **Usage**: `process_event_with_personality()` called on:
  - A11y events (line 653)
  - Chat events (line 719)
  - User interactions (line 788)
  - Task outcomes (line 1217)
- **Mood modifiers**: Used via `PersonalityEngine.compute_influence()` in System2 path

### BDI Scheduler  
- **Status**: PARTIALLY WIRED
- **Current flow**: Goals created → stored in `checkpoint.goals` → mirrored to `bdi.base.enqueue()`
- **Issue**: `checkpoint.goals` Vec is still the source of truth, BDI is secondary

### HTN Decomposer
- **Status**: EXISTS but not wired to BDI scheduler
- **Location**: `crates/aura-daemon/src/goals/decomposer.rs`

## Missing Wiring (What Needs Implementation)

### 1. Response Style Modulation (Priority: HIGH)
**Current**: PersonalityEngine computes influence but doesn't explicitly modulate response length/format
**Need**: 
- When `stress > 0.7` or `arousal > 0.6` → shorten responses
- When `valence > 0.3` and `arousal > 0.2` → add emojis/warmth
- When `emotion == Fear` or `emotion == Sadness` → more empathetic responses

### 2. BDI Scheduler as Primary (Priority: HIGH)
**Current**: Goals stored in both checkpoint.goals (Vec) and BDI scheduler
**Need**: 
- Use BDI scheduler's `next_decision()` for scheduling instead of raw Vec
- Update checkpoint to store BDI state instead of Vec<Goal>
- Wire HTN decomposer to handle complex goals

### 3. HTN Decomposer Integration (Priority: MEDIUM)
**Current**: Decomposer exists but not called during goal execution
**Need**:
- When goal has no steps, call `decomposer.decompose()`
- Store decomposed sub-goals in BDI scheduler

## Implementation Plan

### Phase 1: Response Style Modulation
1. Add `ResponseStyleModifier` struct in `affective.rs`
2. Implement methods: `should_shorten()`, `should_add_emoji()`, `empathy_level()`
3. Wire into message generation path in `main_loop.rs`

### Phase 2: BDI Scheduler Integration  
1. Modify `checkpoint.rs` to use `BdiScheduler` instead of `Vec<Goal>`
2. Add `schedule()` and `next_goal()` calls to main loop scheduling
3. Wire HTN decomposer into BDI desire generation

### Phase 3: Tests
- 20+ new tests covering:
  - Response style modifiers
  - BDI scheduler decision logic
  - HTN decomposition integration
  - AffectiveEngine → response style pipeline

## Files to Modify
1. `crates/aura-daemon/src/identity/affective.rs` - Add ResponseStyleModifier
2. `crates/aura-daemon/src/daemon_core/checkpoint.rs` - Change goals type
3. `crates/aura-daemon/src/daemon_core/main_loop.rs` - Wire response style, BDI primary
4. `crates/aura-daemon/src/goals/mod.rs` - Export decomposer wiring

## Test Targets
- ResponseStyleModifier: 6 tests
- BDI scheduling decisions: 8 tests  
- HTN decomposition: 4 tests
- Integration: 6 tests
