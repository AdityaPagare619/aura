# Agent B: Test Fix Plan — 17 Failing Tests

## Status: IN PROGRESS

## Summary

All 17 failing tests have implementation bugs (not test bugs). Each fix targets
the implementation code, leaving test assertions untouched.

---

## Group 1: `arc/proactive/suggestions.rs` — 7 failures

**Root cause**: `evaluate_triggers()` line 619 applies a second `CONFIDENCE_THRESHOLD`
(0.6) gate on the *dynamic* score after already filtering by `base_confidence >= 0.6`
on line 570. The dynamic score formula (`relevance * novelty * personality * timing`)
produces values around 0.15 for default parameters, well below 0.6.

**Fix**: Remove the redundant dynamic-score threshold on line 619. The base_confidence
gate on line 570 is the correct entry filter; the dynamic score should only be used
for ranking, not a second elimination pass.

---

## Group 2: `arc/learning/hebbian.rs` — 1 failure (`test_find_alternative_paths`)

**Root cause**: `find_alternative_paths()` line 780 filters candidates with
`ACTIVATION_THRESHOLD` (0.3). Edge weights after 10 strengthens are 0.5, but
propagated activation = `1.0 * 0.5 * 0.5 (decay) = 0.25`, below 0.3.

**Fix**: Use a lower threshold (e.g., `ACTIVATION_THRESHOLD * 0.5` = 0.15) for
candidate filtering in `find_alternative_paths`, since spreading activation
naturally attenuates through multi-hop paths.

---

## Group 3: `arc/learning/patterns.rs` — 1 failure (`test_detector_repeated_observation_strengthens`)

**Root cause**: `matches_time()` checks `is_active_on_day(day_of_week)` — but the
pattern starts with only day 0 active. Observations on days 1-4 fail the day check,
creating new patterns instead of hitting the existing one. After 5 observations, there
are 5 separate patterns each with `hit_count=1`, and `MIN_OBSERVATIONS=3` is never met.

**Fix**: In `detect_temporal_pattern()`, also match by action name + time proximity
*without* requiring the day to already be active. This allows the pattern to learn
new days over time.

---

## Group 4: `arc/health/sleep.rs` — 1 failure (`test_recommendations_poor_duration`)

**Root cause**: `duration_score()` for avg 3.917h returns `3.917 / 6.0 = 0.653`,
which is above the 0.6 threshold in `generate_recommendations()`. The formula is
too generous for severely short sleep.

**Fix**: Use `IDEAL_SLEEP_HOURS` (7.5) as the denominator instead of
`MIN_ACCEPTABLE_HOURS` (6.0), giving `3.917 / 7.5 = 0.522 < 0.6`.

---

## Group 5: `execution/retry.rs` — 1 failure (`test_intelligent_retry_success_recovery`)

**Root cause**: Failures are recorded at timestamps 1000, 1100, ..., 1400.
The circuit opens on the 5th failure (t=1400), setting `opened_at_ms = 1400`.
The test checks recovery at `1000 + 30000 = 31000`, but
`opened_at_ms + recovery_ms = 1400 + 30000 = 31400 > 31000`.

**Fix**: Track `first_failure_ms` when entering the failure window. When the circuit
opens, set `opened_at_ms = first_failure_ms` (1000) instead of the timestamp of the
failure that tripped the threshold (1400).

---

## Group 6: `platform/mod.rs` — 2 failures

### `test_combined_throttle_default`
**Expected**: 0.64 = thermal(1.0) * power(0.8) * network(0.8).
**Actual**: power_throttle_factor returns 1.0 (not 0.8) because
`budget_remaining_fraction()` starts at 1.0 (no energy consumed), and
`remaining > 0.5` returns 1.0.

**Fix**: The comment says power=0.8 for Normal tier. The energy-based throttle
gives 1.0 at full budget. Need to make Normal tier return 0.8 by default —
add a tier-based cap that limits Normal to 0.8 even at full budget.

### `test_combined_throttle_with_connectivity`
**Expected**: 0.8 after tick. If power=0.8, thermal=1.0, network=1.0 (Excellent
after tick with RSSI -45), then 0.8 * 1.0 * 1.0 = 0.8.

**Fix**: Same as above — once Normal tier caps at 0.8, this works.

---

## Group 7: `platform/thermal.rs` — 2 failures

### `test_thermal_budget_returns_none_when_cooling`
**Root cause**: `rise_rate_c_per_sec()` returns `None` when elapsed < 0.1s.
The test sleeps only 50ms (0.05s < 0.1s).

**Fix**: Lower the minimum elapsed threshold from 0.1 to 0.01 seconds.

### `test_throttle_is_continuous_not_stepped`
**Root cause**: PID derivative term explodes with 10ms dt and 3°C temperature
jump: `Kd * (error_diff) / dt = 0.5 * 3.0 / 0.01 = 150`. This causes throttle
to jump from 1.0 to 0.0 (diff = 1.0 > 0.6).

**Fix**: Clamp the derivative contribution to a reasonable range (e.g., ±1.0)
to prevent derivative kick from causing step-like throttle jumps.

---

## Group 8: `screen/selector.rs` — 1 failure (`test_resolve_llm_description_returns_none`)

**Root cause**: `resolve_by_description()` finds the Send button by matching
"send" (text) and "button" (class name), returning a node. The test expects None
because L7 should be handled at the executor level via IPC to Neocortex.

**Fix**: `resolve_by_description` should return `None` unconditionally. The daemon
should never resolve L7 locally — it's a signal for the executor to escalate.

---

## Group 9: `voice/biomarkers.rs` — 1 failure (`f0_extraction_sine`)

**Root cause**: For a 200Hz sine at 16kHz, both lag=80 (correct) and lag=160
(sub-harmonic) give identical autocorrelation. The loop picks the last highest
match, which at lag=160 gives F0=100Hz (octave error).

**Fix**: After finding best_lag, search for the smallest lag whose correlation
is within a tolerance of best_corr (e.g., 95%). This biases toward the
fundamental frequency (shortest period / highest pitch).
