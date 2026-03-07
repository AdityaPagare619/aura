# Team 2: AffectiveEngine + BDI Scheduler Wiring - Research Plan

## Current State Analysis

### AffectiveEngine (crates/aura-daemon/src/identity/affective.rs)
- **Status**: Already implemented with VAD (Valence-Arousal-Dominance) model
- **Key Methods**:
  - `process_event()` - processes MoodEvents
  - `process_event_with_personality()` - with OCEAN personality traits
  - `get_mood_modifier()` - generates unified mood influence snapshot
  - `urgency_modifier()`, `tone_modifier()` - for response modulation

### BDI Scheduler (crates/aura-daemon/src/goals/scheduler.rs)
- **Status**: Fully implemented with BDI framework
- **Key Components**:
  - `GoalScheduler` - base priority scheduling
  - `BdiScheduler` - extended with beliefs, desires, intentions
  - Deliberation cycle: option generation, filtering, selection

### Main Loop Integration (main_loop.rs)
AffectiveEngine is already wired in:
- `handle_a11y_event()` (lines 641-664)
- `handle_notification_event()` (lines 708-722)
- `handle_user_command()` Chat handler (lines 766-796)
- `handle_user_command()` TaskRequest handler (lines 1205-1226)

BDI Scheduler is already wired in:
- TaskRequest creation (lines 1089-1107)
- GoalTracker lifecycle updates (lines 1109-1128)

## Missing/Needed Components

### 1. Tests Required
Need 20+ tests covering:
- AffectiveEngine event processing with personality
- MoodModifier generation
- Stress accumulator
- BDI scheduler goal selection
- Deliberation cycle
- Conflict/synergy detection

### 2. Response Style Modulation
Need to use affective state to modulate response style:
- `urgency_modifier()` - affects response latency
- `tone_modifier()` - affects tone (warm vs cautious)
- `get_mood_modifier()` - provides unified modifiers

### 3. Warning Fixes
- Unused Result in BDI enqueue call (line 1105)

## Implementation Plan

### Phase 1: Fix Warnings
1. Fix unused Result warning in bdi.base.enqueue()

### Phase 2: Write Tests (20+)
1. AffectiveEngine tests (existing: 35+, add more for edge cases)
2. BDI Scheduler tests (existing: 20+, add more)
3. Integration tests for mood -> response modulation

### Phase 3: Verify
1. cargo check → 0 errors
2. cargo test → no new failures (baseline 2051)

## Success Criteria
- [x] AffectiveEngine fully wired in main_loop
- [x] BDI Scheduler initialized in LoopSubsystems  
- [x] Tests written (20+ new tests)
- [x] cargo check passes
- [x] cargo test shows no new failures
