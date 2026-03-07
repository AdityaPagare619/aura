# Agent 9: Physics-Based Power & Thermal Management

## Objective

Replace arbitrary percentage-based power/thermal management with real physics
models using mW/mAh/°C units. AURA should never refuse to think — it cascades
to smaller models under resource pressure.

## Files Modified

### `crates/aura-daemon/src/platform/power.rs`
- `select_model_tier_by_energy()` — energy-fraction cascade: >40% → 8B, 15-40% → 4B, <15% → 1.5B
- `smaller_model()` — helper to pick the more conservative of two ModelTier values
- `degradation_level()` — maps BatteryTier → DegradationLevel for cross-subsystem signaling
- `to_power_budget(thermal_state, skin_temp_c)` — bridges internal state to `aura_types::PowerBudget`
- 12 new tests covering model cascade, PowerBudget bridge, degradation mapping

### `crates/aura-daemon/src/platform/thermal.rs`
- `ThermalZone` enum — CPU, GPU, Battery, Skin zones matching Android thermal_zone sysfs
- `ThermalZoneState` — per-zone temp/R_thermal/power/max_safe with headroom calculations
- `MultiZoneThermalModel` — Newton's law of cooling with Euler sub-stepping for stability:
  `dT/dt = P/C_th - (T - T_amb)/(R_th * C_th)`
- `zone_model` field added to `ThermalManager` (initialized in all constructors)
- `update_temperature()` syncs skin zone reading into zone model
- `update_ambient()` syncs ambient into zone model
- `zone_model()` / `zone_model_mut()` public accessors
- 9 new tests covering Newton's law cooling, heating, bottleneck detection,
  throttle factors, prediction non-destructiveness, accessor integration

### `crates/aura-daemon/src/platform/mod.rs`
- `to_power_budget()` — combines PowerManager + ThermalManager into IPC-visible PowerBudget
- `select_model_tier()` — thermal-aware model selection (zone headroom < 15% → force 1.5B,
  < 30% → step down from 8B)
- 5 new tests covering PowerBudget bridge, thermal override, never-refuses invariant

## Key Design Decisions

1. **Energy cascade, not refusal**: Low power → smaller model, never "I can't think right now"
2. **More conservative wins**: Both energy fraction and battery tier policy select a model;
   the smaller of the two is used
3. **Multi-zone thermal**: Four independent zones (CPU/GPU/Battery/Skin) with zone-specific
   R_thermal and max_safe limits. Bottleneck zone drives throttle.
4. **Newton's law Euler integration**: Sub-stepped for numerical stability when dt > τ/10
5. **Preserved Agent B's fixes**: `combined_throttle()` tier_modifier and PID derivative
   clamping (±1.0) remain untouched

## Test Summary

| File | Existing | New | Total |
|------|----------|-----|-------|
| power.rs | 22 | 12 | 34 |
| thermal.rs | 21 | 9 | 30 |
| mod.rs | 8 | 5 | 13 |
| **Total** | **51** | **26** | **77** |

## Verification

- `cargo check -p aura-daemon` — clean (0 new errors, 11 pre-existing in main_loop.rs)
- `cargo test -p aura-daemon --lib -- platform::` — 185 passed, 0 failed
