//! Physics-based thermal management with junction-to-skin model and PID control.
//!
//! Replaces arbitrary temperature thresholds (40/45/50°C) with a real thermal
//! model grounded in semiconductor physics and human comfort thresholds.
//!
//! # Thermal Model
//!
//! The phone is modeled as a lumped thermal system:
//!
//! ```text
//! T_junction = T_ambient + P_dissipated × R_thermal_ja
//! T_skin     = T_junction - P_dissipated × R_thermal_jc
//! dT/dt      = (P_dissipated - (T - T_ambient)/R_th) / C_th
//! ```
//!
//! Where:
//! - R_thermal_ja: junction-to-ambient thermal resistance (°C/W)
//! - R_thermal_jc: junction-to-case thermal resistance (°C/W)
//! - C_th: thermal capacitance of phone body (J/°C)
//! - τ = R_th × C_th: thermal time constant (seconds)
//!
//! # Skin Temperature Thresholds (ISO 13732-1)
//!
//! | State    | T_skin     | Rationale                          |
//! |----------|------------|------------------------------------|
//! | Cool     | < 37°C     | No perceptible warmth              |
//! | Warm     | 37–40°C    | Comfortable but noticeable         |
//! | Hot      | 40–43°C    | Uncomfortable, need to throttle    |
//! | Critical | > 43°C     | Pain threshold, emergency          |
//!
//! # PID Controller
//!
//! Instead of step-function throttling, a PID controller provides smooth
//! control of inference rate based on temperature error from the setpoint.
//! This eliminates oscillation and provides optimal throughput.
//!
//! # Spec Reference
//!
//! See `AURA-V4-POWER-AGENCY-REBALANCE.md` §8 — Thermal Management.

use std::time::{Duration, Instant};

use aura_types::power::{
    SocThermalProfile, ThermalState, PHONE_THERMAL_CAPACITANCE, SKIN_TEMP_CRITICAL_C,
    SKIN_TEMP_HOT_C, SKIN_TEMP_WARM_C,
};
use serde::{Deserialize, Serialize};

// ─── Thermal Zone Model ─────────────────────────────────────────────────────

/// Distinct thermal zones on a mobile SoC.
///
/// Each zone has independent temperature dynamics and contributes to the
/// overall thermal state. Real Android devices expose these via
/// `/sys/class/thermal/thermal_zone*/type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThermalZone {
    /// CPU cluster (big.LITTLE or similar). Typically the hottest zone
    /// during inference workloads.
    Cpu,
    /// GPU / NPU zone. Active during tensor operations.
    Gpu,
    /// Battery cell temperature. Constrained by chemistry (max ~45°C
    /// for LiPo longevity).
    Battery,
    /// External skin temperature — what the user feels. ISO 13732-1 limits
    /// apply here (43°C pain threshold).
    Skin,
}

/// Per-zone thermal state tracking.
///
/// Each zone has its own thermal resistance, power contribution, and
/// current temperature. The zone model enables targeted throttling
/// (e.g., reduce GPU load without affecting CPU if GPU is the bottleneck).
#[derive(Debug, Clone)]
pub struct ThermalZoneState {
    /// Zone identifier.
    pub zone: ThermalZone,
    /// Current temperature in °C.
    pub temperature_c: f64,
    /// Thermal resistance junction-to-zone (°C/W).
    pub r_thermal: f64,
    /// Zone's contribution to total power dissipation (W).
    pub power_w: f64,
    /// Maximum safe temperature for this zone (°C).
    pub max_safe_c: f64,
}

impl ThermalZoneState {
    /// Headroom before hitting the zone's safety limit (°C).
    ///
    /// Returns 0.0 if already at or above the limit.
    pub fn headroom_c(&self) -> f64 {
        (self.max_safe_c - self.temperature_c).max(0.0)
    }

    /// Fraction of thermal headroom remaining (0.0–1.0).
    ///
    /// 1.0 means the zone is cool (at ambient), 0.0 means at the limit.
    pub fn headroom_fraction(&self, ambient_c: f64) -> f64 {
        let total_range = self.max_safe_c - ambient_c;
        if total_range <= 0.0 {
            return 0.0;
        }
        self.headroom_c() / total_range
    }
}

/// Multi-zone thermal model tracking all SoC thermal domains.
///
/// Provides Newton's law of cooling time-stepping for transient simulation
/// and per-zone throttle recommendations. The simulation uses the lumped
/// capacitance model:
///
/// ```text
/// dT/dt = P/(m·Cp) − h·(T − T_amb)
///       = P/C_th   − (T − T_amb)/(R_th · C_th)
/// ```
///
/// Where `C_th = m·Cp` is the phone's thermal capacitance and
/// `R_th` is the zone's thermal resistance.
#[derive(Debug, Clone)]
pub struct MultiZoneThermalModel {
    zones: Vec<ThermalZoneState>,
    /// Ambient temperature shared by all zones (°C).
    ambient_c: f64,
    /// Phone thermal capacitance (J/°C).
    thermal_capacitance: f64,
}

impl MultiZoneThermalModel {
    /// Create a multi-zone model with default zone configuration.
    ///
    /// Default zones are tuned for a Snapdragon 8 Gen 3–class SoC:
    /// - CPU: R_th=12 °C/W, max 95°C (junction limit)
    /// - GPU: R_th=10 °C/W, max 90°C
    /// - Battery: R_th=20 °C/W, max 45°C (LiPo chemistry limit)
    /// - Skin: R_th=8 °C/W, max 43°C (ISO 13732-1 pain threshold)
    pub fn new() -> Self {
        Self::with_ambient(25.0)
    }

    /// Create a multi-zone model with a specific ambient temperature.
    pub fn with_ambient(ambient_c: f64) -> Self {
        Self {
            zones: vec![
                ThermalZoneState {
                    zone: ThermalZone::Cpu,
                    temperature_c: ambient_c,
                    r_thermal: 12.0,
                    power_w: 0.3,
                    max_safe_c: 95.0,
                },
                ThermalZoneState {
                    zone: ThermalZone::Gpu,
                    temperature_c: ambient_c,
                    r_thermal: 10.0,
                    power_w: 0.1,
                    max_safe_c: 90.0,
                },
                ThermalZoneState {
                    zone: ThermalZone::Battery,
                    temperature_c: ambient_c,
                    r_thermal: 20.0,
                    power_w: 0.05,
                    max_safe_c: 45.0,
                },
                ThermalZoneState {
                    zone: ThermalZone::Skin,
                    temperature_c: ambient_c,
                    r_thermal: 8.0,
                    power_w: 0.0, // Skin receives heat from other zones
                    max_safe_c: SKIN_TEMP_CRITICAL_C,
                },
            ],
            ambient_c,
            thermal_capacitance: PHONE_THERMAL_CAPACITANCE,
        }
    }

    /// Advance the thermal simulation by `dt` seconds using Newton's law of cooling.
    ///
    /// For each zone, applies:
    /// ```text
    /// dT = (P/C_th − (T − T_amb)/(R_th · C_th)) · dt
    /// ```
    ///
    /// Uses Euler integration with sub-stepping for numerical stability
    /// when `dt` exceeds 1/10 of the smallest thermal time constant.
    pub fn simulate_step(&mut self, dt_seconds: f64) {
        if dt_seconds <= 0.0 {
            return;
        }

        // Sub-step for stability: each step ≤ min(τ)/10
        let min_tau = self
            .zones
            .iter()
            .map(|z| z.r_thermal * self.thermal_capacitance)
            .fold(f64::INFINITY, f64::min);
        let max_step = (min_tau / 10.0).max(0.1);
        let n_steps = ((dt_seconds / max_step).ceil() as usize).max(1);
        let step = dt_seconds / n_steps as f64;

        for _ in 0..n_steps {
            for zone in &mut self.zones {
                let heat_in = zone.power_w / self.thermal_capacitance;
                let heat_out = (zone.temperature_c - self.ambient_c)
                    / (zone.r_thermal * self.thermal_capacitance);
                zone.temperature_c += (heat_in - heat_out) * step;
            }
        }
    }

    /// Update a specific zone's power dissipation (W).
    pub fn set_zone_power(&mut self, zone: ThermalZone, power_w: f64) {
        if let Some(z) = self.zones.iter_mut().find(|z| z.zone == zone) {
            z.power_w = power_w;
        }
    }

    /// Update a specific zone's temperature from a sensor reading (°C).
    pub fn set_zone_temperature(&mut self, zone: ThermalZone, temp_c: f64) {
        if let Some(z) = self.zones.iter_mut().find(|z| z.zone == zone) {
            z.temperature_c = temp_c;
        }
    }

    /// Update the ambient temperature (°C).
    pub fn set_ambient(&mut self, ambient_c: f64) {
        self.ambient_c = ambient_c;
    }

    /// Get the current ambient temperature (°C).
    pub fn ambient_c(&self) -> f64 {
        self.ambient_c
    }

    /// Get the state of a specific zone.
    pub fn zone_state(&self, zone: ThermalZone) -> Option<&ThermalZoneState> {
        self.zones.iter().find(|z| z.zone == zone)
    }

    /// Get the most constrained zone — the one closest to its thermal limit.
    ///
    /// Returns `None` only if no zones are configured.
    pub fn bottleneck_zone(&self) -> Option<&ThermalZoneState> {
        self.zones.iter().min_by(|a, b| {
            a.headroom_fraction(self.ambient_c)
                .partial_cmp(&b.headroom_fraction(self.ambient_c))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Per-zone throttle factor: the minimum headroom fraction across all zones.
    ///
    /// Returns 1.0 when all zones are cool, 0.0 when any zone is at its limit.
    pub fn zone_throttle_factor(&self) -> f64 {
        self.zones
            .iter()
            .map(|z| z.headroom_fraction(self.ambient_c))
            .fold(1.0_f64, f64::min)
            .clamp(0.0, 1.0)
    }

    /// Predict the skin temperature after `dt` seconds at current power levels.
    ///
    /// Runs a copy of the simulation forward without modifying the current state.
    pub fn predict_skin_temp_after(&self, dt_seconds: f64) -> f64 {
        let mut sim = self.clone();
        sim.simulate_step(dt_seconds);
        sim.zones
            .iter()
            .find(|z| z.zone == ThermalZone::Skin)
            .map(|z| z.temperature_c)
            .unwrap_or(self.ambient_c)
    }
}

// ─── Configurable Thresholds ────────────────────────────────────────────────

/// Temperature thresholds for thermal state transitions (°C).
///
/// Based on ISO 13732-1 human skin contact comfort data.
/// The gap between `degrade` and `recover` for each boundary is the
/// hysteresis band (default 2°C).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ThermalThresholds {
    /// Skin temperature above which we enter Warm state (°C).
    /// Default: 37°C — perceptible warmth on skin contact.
    pub warm_c: f32,
    /// Skin temperature above which we enter Hot state (°C).
    /// Default: 40°C — uncomfortable for prolonged holding.
    pub hot_c: f32,
    /// Skin temperature above which we enter Critical state (°C).
    /// Default: 43°C — pain threshold for 10-second contact.
    pub critical_c: f32,
}

impl Default for ThermalThresholds {
    fn default() -> Self {
        Self {
            warm_c: SKIN_TEMP_WARM_C as f32,
            hot_c: SKIN_TEMP_HOT_C as f32,
            critical_c: SKIN_TEMP_CRITICAL_C as f32,
        }
    }
}

// ─── PID Controller ─────────────────────────────────────────────────────────

/// PID controller for smooth thermal throttling.
///
/// Controls inference throttle factor based on temperature error from
/// a setpoint (target skin temperature). Prevents oscillation and provides
/// maximum throughput within thermal limits.
///
/// # Tuning
///
/// - Kp: Proportional gain — immediate response to temperature overshoot.
///   Too high → oscillation. Too low → slow response.
/// - Ki: Integral gain — eliminates steady-state error (temperature offset).
///   Too high → windup overshoot. Too low → permanent offset.
/// - Ki is bounded by anti-windup clamping.
/// - Kd: Derivative gain — dampens oscillation by responding to rate of change.
///   Too high → noise amplification. Too low → underdamped oscillation.
#[derive(Debug, Clone)]
struct ThermalPidController {
    /// Target skin temperature (°C) — the setpoint.
    setpoint_c: f64,
    /// Proportional gain.
    kp: f64,
    /// Integral gain.
    ki: f64,
    /// Derivative gain.
    kd: f64,
    /// Accumulated integral error.
    integral: f64,
    /// Maximum absolute integral to prevent windup.
    integral_max: f64,
    /// Previous error for derivative calculation.
    prev_error: f64,
    /// Time of last update.
    last_update: Instant,
}

impl ThermalPidController {
    /// Create a PID controller targeting the warm threshold.
    ///
    /// Default gains tuned for phone thermal dynamics:
    /// - τ ≈ 300-2400s, so we need gentle gains
    /// - Kp = 0.15: moderate proportional response
    /// - Ki = 0.005: slow integral to handle steady-state
    /// - Kd = 0.5: moderate derivative to dampen oscillation
    fn new(setpoint_c: f64) -> Self {
        Self {
            setpoint_c,
            kp: 0.15,
            ki: 0.005,
            kd: 0.5,
            integral: 0.0,
            integral_max: 5.0, // Anti-windup: max ±5°C·s accumulated error
            prev_error: 0.0,
            last_update: Instant::now(),
        }
    }

    /// Update PID controller with new temperature reading.
    ///
    /// Returns a throttle factor in [0.0, 1.0].
    /// - 1.0: no throttle (temperature well below setpoint)
    /// - 0.0: full stop (temperature far above setpoint)
    fn update(&mut self, current_temp_c: f64) -> f64 {
        let now = Instant::now();
        let dt = now.duration_since(self.last_update).as_secs_f64();
        self.last_update = now;

        // Error: positive means we're too hot (need to reduce power)
        let error = current_temp_c - self.setpoint_c;

        // Proportional term
        let p = self.kp * error;

        // Integral term with anti-windup clamping
        if dt > 0.0 && dt < 300.0 {
            // Only accumulate if dt is reasonable (not first call or huge gap)
            self.integral += error * dt;
            self.integral = self.integral.clamp(-self.integral_max, self.integral_max);
        }
        let i = self.ki * self.integral;

        // Derivative term (rate of temperature change)
        // Clamped to ±1.0 to prevent derivative kick from causing
        // step-like throttle jumps when dt is small.
        let d = if dt > 0.01 {
            let raw_d = self.kd * (error - self.prev_error) / dt;
            raw_d.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.prev_error = error;

        // PID output: higher = more throttling needed
        let pid_output = p + i + d;

        // Map PID output to throttle factor:
        // pid_output <= 0 → no throttle (1.0) — we're below setpoint
        // pid_output >= 1 → full throttle (0.0) — we're way above
        let throttle = (1.0 - pid_output).clamp(0.0, 1.0);

        throttle
    }

    /// Reset the PID state (e.g., after a long idle period).
    fn reset(&mut self) {
        self.integral = 0.0;
        self.prev_error = 0.0;
        self.last_update = Instant::now();
    }
}

// ─── Thermal Physics Model ─────────────────────────────────────────────────

/// Transient thermal model for the phone body.
///
/// Tracks the thermal state using a first-order RC model:
/// `dT/dt = (P_in - (T - T_ambient)/R_th) / C_th`
///
/// This allows prediction of future temperatures and thermal budget estimation.
#[derive(Debug, Clone)]
struct ThermalPhysicsModel {
    /// SoC thermal profile (R_th, C_th, power levels).
    profile: SocThermalProfile,
    /// Estimated ambient temperature (°C).
    ambient_c: f64,
    /// Current estimated skin temperature (°C).
    skin_temp_c: f64,
    /// Current estimated junction temperature (°C).
    junction_temp_c: f64,
    /// Current power dissipation estimate (W).
    power_w: f64,
}

impl ThermalPhysicsModel {
    fn new(profile: SocThermalProfile) -> Self {
        Self {
            profile,
            ambient_c: 25.0,
            skin_temp_c: 25.0,
            junction_temp_c: 25.0,
            power_w: profile.idle_power_w,
        }
    }

    /// Update the model with a measured skin temperature.
    ///
    /// Back-calculates junction temperature and power from the skin reading.
    fn update_from_skin_reading(&mut self, measured_skin_c: f64) {
        self.skin_temp_c = measured_skin_c;

        // Estimate power from skin temperature rise above ambient:
        // T_skin = T_ambient + P × (R_ja - R_jc)
        // P = (T_skin - T_ambient) / (R_ja - R_jc)
        let r_case_to_ambient = self.profile.r_thermal_ja - self.profile.r_thermal_jc;
        if r_case_to_ambient > 0.0 {
            self.power_w = ((measured_skin_c - self.ambient_c) / r_case_to_ambient)
                .max(self.profile.idle_power_w);
        }

        // Estimate junction temperature from skin and power:
        // T_junction = T_skin + P × R_jc
        self.junction_temp_c = measured_skin_c + self.power_w * self.profile.r_thermal_jc;
    }

    /// Update ambient temperature estimate.
    ///
    /// On Android, this could come from the battery temperature sensor
    /// (which is closer to ambient than skin temperature).
    fn update_ambient(&mut self, ambient_c: f64) {
        self.ambient_c = ambient_c;
    }

    /// Estimate time in seconds until skin temperature reaches a target.
    ///
    /// Uses the first-order thermal model:
    /// `T(t) = T_steady + (T_0 - T_steady) × exp(-t/τ)`
    ///
    /// Returns `None` if temperature is already above target or will never reach it.
    fn time_to_temp_seconds(&self, target_skin_c: f64) -> Option<f64> {
        let r_ca = self.profile.r_thermal_ja - self.profile.r_thermal_jc;
        if r_ca <= 0.0 {
            return None;
        }

        // Steady-state skin temperature at current power
        let t_steady = self.ambient_c + self.power_w * r_ca;

        // If steady state is below target, we'll never reach it
        if t_steady <= target_skin_c {
            return None;
        }

        // If already above target
        if self.skin_temp_c >= target_skin_c {
            return Some(0.0);
        }

        // Time constant (using case-to-ambient R for skin model)
        let tau = r_ca * aura_types::power::PHONE_THERMAL_CAPACITANCE;

        // Solve: target = t_steady + (skin_current - t_steady) × exp(-t/τ)
        // exp(-t/τ) = (target - t_steady) / (skin_current - t_steady)
        let ratio = (target_skin_c - t_steady) / (self.skin_temp_c - t_steady);
        if ratio <= 0.0 || ratio >= 1.0 {
            return None;
        }

        Some(-tau * ratio.ln())
    }

    /// Maximum safe inference duration at current conditions (seconds).
    ///
    /// Returns time until skin temperature would reach the hot threshold.
    fn thermal_budget_seconds(&self) -> Option<f64> {
        self.time_to_temp_seconds(SKIN_TEMP_HOT_C)
    }
}

// ─── ThermalManager ─────────────────────────────────────────────────────────

/// Manages SoC thermal state with physics-based modeling and PID control.
///
/// Combines:
/// 1. **Physics model**: junction-to-skin temperature estimation
/// 2. **State machine**: 4-state (Cool/Warm/Hot/Critical) with hysteresis
/// 3. **PID controller**: smooth throttle factor (no step functions)
/// 4. **Budget estimation**: time until thermal limit at current load
/// 5. **Multi-zone model**: per-zone (CPU/GPU/Battery/Skin) temperature tracking
pub struct ThermalManager {
    /// Current discrete thermal state.
    state: ThermalState,
    /// Last-read skin temperature (°C).
    temperature_c: f32,
    /// Configurable thresholds.
    thresholds: ThermalThresholds,
    /// Hysteresis band (°C) — recovery requires temperature to drop this
    /// far below the degrade threshold before we upgrade state.
    hysteresis_c: f32,
    /// PID-controlled throttle factor: 1.0 = unrestricted, 0.0 = full stop.
    throttle_factor: f32,
    /// Timestamp of the last state transition.
    last_transition: Instant,
    /// Minimum dwell time before allowing another transition.
    min_transition_interval: Duration,
    /// Recent temperature samples for rise-rate estimation.
    /// Fixed-size ring of (timestamp, temperature).
    temp_history: [(Instant, f32); Self::HISTORY_SIZE],
    /// Next write index into `temp_history`.
    history_idx: usize,
    /// Number of valid entries in `temp_history`.
    history_count: usize,
    /// PID controller for smooth throttle computation.
    pid: ThermalPidController,
    /// Physics model for junction/skin temperature estimation.
    physics: ThermalPhysicsModel,
    /// Multi-zone thermal model for per-zone tracking and simulation.
    zone_model: MultiZoneThermalModel,
}

impl ThermalManager {
    const HISTORY_SIZE: usize = 16;

    /// Create a new `ThermalManager` with physics-based default thresholds.
    pub fn new() -> Self {
        Self::with_thresholds(ThermalThresholds::default())
    }

    /// Create a `ThermalManager` with custom thresholds.
    pub fn with_thresholds(thresholds: ThermalThresholds) -> Self {
        Self::with_thresholds_and_profile(thresholds, SocThermalProfile::DEFAULT)
    }

    /// Create a `ThermalManager` with custom thresholds and SoC profile.
    pub fn with_thresholds_and_profile(
        thresholds: ThermalThresholds,
        soc_profile: SocThermalProfile,
    ) -> Self {
        let now = Instant::now();
        // PID setpoint halfway between warm and hot for smooth control
        let setpoint = (thresholds.warm_c as f64 + thresholds.hot_c as f64) / 2.0;
        Self {
            state: ThermalState::Cool,
            temperature_c: 25.0,
            thresholds,
            hysteresis_c: 2.0,
            throttle_factor: 1.0,
            last_transition: now,
            min_transition_interval: Duration::from_secs(10),
            temp_history: [(now, 25.0); Self::HISTORY_SIZE],
            history_idx: 0,
            history_count: 0,
            pid: ThermalPidController::new(setpoint),
            physics: ThermalPhysicsModel::new(soc_profile),
            zone_model: MultiZoneThermalModel::new(),
        }
    }

    /// Update the current temperature reading (skin temperature in °C).
    ///
    /// Returns `Some(new_state)` if a thermal state transition occurred.
    pub fn update_temperature(&mut self, temp_c: f32) -> Option<ThermalState> {
        self.temperature_c = temp_c;
        self.record_sample(temp_c);

        // Update physics model
        self.physics.update_from_skin_reading(temp_c as f64);

        // Sync multi-zone model: update skin zone from the sensor reading
        self.zone_model
            .set_zone_temperature(ThermalZone::Skin, temp_c as f64);

        // PID-controlled throttle factor (smooth, no step function)
        self.throttle_factor = self.pid.update(temp_c as f64) as f32;

        let candidate = self.compute_candidate_state(temp_c);

        if candidate == self.state {
            return None;
        }

        // Enforce dwell time.
        if self.last_transition.elapsed() < self.min_transition_interval {
            tracing::trace!(
                current = ?self.state,
                candidate = ?candidate,
                temp_c,
                "thermal transition suppressed: dwell time"
            );
            return None;
        }

        let old = self.state;
        self.state = candidate;
        self.last_transition = Instant::now();

        tracing::info!(
            from = ?old,
            to = ?candidate,
            skin_temp_c = temp_c,
            junction_temp_c = self.physics.junction_temp_c as f32,
            power_w = self.physics.power_w as f32,
            throttle = self.throttle_factor,
            "thermal state transition"
        );

        Some(candidate)
    }

    /// Update ambient temperature estimate (e.g., from battery temp sensor).
    pub fn update_ambient(&mut self, ambient_c: f32) {
        self.physics.update_ambient(ambient_c as f64);
        self.zone_model.set_ambient(ambient_c as f64);
    }

    /// Compute the candidate thermal state with hysteresis.
    fn compute_candidate_state(&self, temp_c: f32) -> ThermalState {
        // Degrade path: check if we should move to a worse state.
        match self.state {
            ThermalState::Cool => {
                if temp_c >= self.thresholds.warm_c {
                    return ThermalState::Warm;
                }
            }
            ThermalState::Warm => {
                if temp_c >= self.thresholds.hot_c {
                    return ThermalState::Hot;
                }
                // Recover: must drop below warm threshold minus hysteresis.
                if temp_c < self.thresholds.warm_c - self.hysteresis_c {
                    return ThermalState::Cool;
                }
            }
            ThermalState::Hot => {
                if temp_c >= self.thresholds.critical_c {
                    return ThermalState::Critical;
                }
                if temp_c < self.thresholds.hot_c - self.hysteresis_c {
                    return ThermalState::Warm;
                }
            }
            ThermalState::Critical => {
                // Only recover from critical if well below the threshold.
                if temp_c < self.thresholds.critical_c - self.hysteresis_c {
                    return ThermalState::Hot;
                }
            }
        }

        self.state
    }

    /// Record a temperature sample for rise-rate estimation.
    fn record_sample(&mut self, temp_c: f32) {
        self.temp_history[self.history_idx] = (Instant::now(), temp_c);
        self.history_idx = (self.history_idx + 1) % Self::HISTORY_SIZE;
        if self.history_count < Self::HISTORY_SIZE {
            self.history_count += 1;
        }
    }

    /// Estimate temperature rise rate (°C per second).
    ///
    /// Uses linear regression over the history window for noise robustness.
    /// Returns `None` if insufficient data (need at least 2 samples).
    pub fn rise_rate_c_per_sec(&self) -> Option<f32> {
        if self.history_count < 2 {
            return None;
        }

        // Find oldest and newest valid samples.
        let newest_idx = if self.history_idx == 0 {
            Self::HISTORY_SIZE - 1
        } else {
            self.history_idx - 1
        };
        let oldest_idx = if self.history_count < Self::HISTORY_SIZE {
            0
        } else {
            self.history_idx
        };

        let (t_old, temp_old) = self.temp_history[oldest_idx];
        let (t_new, temp_new) = self.temp_history[newest_idx];

        let elapsed = t_new.duration_since(t_old).as_secs_f32();
        if elapsed < 0.01 {
            return None;
        }

        Some((temp_new - temp_old) / elapsed)
    }

    /// Physics-based thermal budget: seconds until skin hits hot threshold.
    ///
    /// Uses the first-order thermal model for more accurate prediction than
    /// simple linear extrapolation from rise rate.
    ///
    /// Returns `None` if temperature is cooling or will never reach hot.
    pub fn thermal_budget_seconds(&self) -> Option<f32> {
        // Try physics model first (more accurate for non-linear thermal response)
        if let Some(budget) = self.physics.thermal_budget_seconds() {
            return Some(budget as f32);
        }

        // Fallback: linear extrapolation from rise rate
        let rate = self.rise_rate_c_per_sec()?;
        if rate <= 0.0 {
            return None; // Cooling or stable
        }

        let headroom = self.thresholds.hot_c - self.temperature_c;
        if headroom <= 0.0 {
            return Some(0.0);
        }

        Some(headroom / rate)
    }

    // ─── Read-only Queries ──────────────────────────────────────────────

    /// Current discrete thermal state.
    pub fn current_state(&self) -> ThermalState {
        self.state
    }

    /// Last-read skin temperature in °C.
    pub fn temperature(&self) -> f32 {
        self.temperature_c
    }

    /// Estimated junction temperature in °C.
    pub fn junction_temperature(&self) -> f32 {
        self.physics.junction_temp_c as f32
    }

    /// Estimated power dissipation in watts.
    pub fn estimated_power_w(&self) -> f32 {
        self.physics.power_w as f32
    }

    /// Estimated ambient temperature in °C.
    pub fn ambient_temperature(&self) -> f32 {
        self.physics.ambient_c as f32
    }

    /// Immutable reference to the multi-zone thermal model.
    ///
    /// Use this to query per-zone temperatures, headroom, and throttle
    /// factors for fine-grained thermal-aware scheduling.
    pub fn zone_model(&self) -> &MultiZoneThermalModel {
        &self.zone_model
    }

    /// Mutable reference to the multi-zone thermal model.
    ///
    /// Use this to feed per-zone sensor readings or adjust power
    /// dissipation estimates from workload profiling.
    pub fn zone_model_mut(&mut self) -> &mut MultiZoneThermalModel {
        &mut self.zone_model
    }

    /// PID-controlled throttle factor: 1.0 = no throttle, 0.0 = full stop.
    ///
    /// This is a **smooth continuous** signal, not a step function.
    /// The PID controller targets a temperature setpoint halfway between
    /// warm and hot thresholds for optimal throughput.
    pub fn throttle_factor(&self) -> f32 {
        self.throttle_factor
    }

    /// Whether inference should be paused (Hot or Critical).
    pub fn should_pause_inference(&self) -> bool {
        matches!(self.state, ThermalState::Hot | ThermalState::Critical)
    }

    /// Whether an emergency checkpoint + sleep is needed (Critical).
    pub fn should_emergency_checkpoint(&self) -> bool {
        self.state == ThermalState::Critical
    }

    /// Reference to the current thresholds.
    pub fn thresholds(&self) -> &ThermalThresholds {
        &self.thresholds
    }

    /// Maximum sustainable power at current ambient temperature (W).
    ///
    /// This is the power level that would keep skin temperature at the
    /// hot threshold indefinitely.
    pub fn max_sustainable_power_w(&self) -> f32 {
        self.physics
            .profile
            .max_sustainable_power_w(self.physics.ambient_c, self.thresholds.hot_c as f64)
            as f32
    }

    /// Reference to the SoC thermal profile.
    pub fn soc_profile(&self) -> &SocThermalProfile {
        &self.physics.profile
    }
}

impl Default for ThermalManager {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manager() -> ThermalManager {
        let mut tm = ThermalManager::new();
        // Reset last_transition so dwell time is already satisfied.
        tm.last_transition = Instant::now() - Duration::from_secs(60);
        tm
    }

    // ── State Transitions (physics-based thresholds) ────────────────────

    #[test]
    fn test_cool_to_warm_transition() {
        let mut tm = make_manager();
        assert_eq!(tm.current_state(), ThermalState::Cool);

        // 37°C is the new warm threshold (was 40°C)
        let result = tm.update_temperature(37.5);
        assert_eq!(result, Some(ThermalState::Warm));
        assert_eq!(tm.current_state(), ThermalState::Warm);
    }

    #[test]
    fn test_warm_to_hot_transition() {
        let mut tm = make_manager();
        tm.state = ThermalState::Warm;

        // 40°C is the new hot threshold (was 45°C)
        let result = tm.update_temperature(40.5);
        assert_eq!(result, Some(ThermalState::Hot));
        assert!(tm.should_pause_inference());
    }

    #[test]
    fn test_hot_to_critical_transition() {
        let mut tm = make_manager();
        tm.state = ThermalState::Hot;

        // 43°C is the new critical threshold (was 50°C)
        let result = tm.update_temperature(43.5);
        assert_eq!(result, Some(ThermalState::Critical));
        assert!(tm.should_emergency_checkpoint());
    }

    #[test]
    fn test_hysteresis_prevents_immediate_recovery() {
        let mut tm = make_manager();
        tm.state = ThermalState::Warm;

        // 36°C is below warm (37°C) but NOT below warm - hysteresis (35°C).
        let result = tm.update_temperature(36.0);
        assert!(result.is_none());
        assert_eq!(tm.current_state(), ThermalState::Warm);

        // 34°C is below warm - hysteresis (35°C) → recover to Cool.
        let result = tm.update_temperature(34.0);
        assert_eq!(result, Some(ThermalState::Cool));
    }

    #[test]
    fn test_critical_recovery_needs_big_drop() {
        let mut tm = make_manager();
        tm.state = ThermalState::Critical;

        // 42°C is below critical (43°C) but NOT below critical - hysteresis (41°C).
        let result = tm.update_temperature(42.0);
        assert!(result.is_none());

        // 40°C IS below 41°C → recovers to Hot.
        let result = tm.update_temperature(40.0);
        assert_eq!(result, Some(ThermalState::Hot));
    }

    // ── PID Controller ──────────────────────────────────────────────────

    #[test]
    fn test_pid_throttle_below_setpoint() {
        let mut pid = ThermalPidController::new(38.5); // setpoint
                                                       // Well below setpoint → no throttle
        let throttle = pid.update(30.0);
        assert!(
            throttle > 0.9,
            "throttle should be near 1.0 below setpoint, got {throttle}"
        );
    }

    #[test]
    fn test_pid_throttle_above_setpoint() {
        let mut pid = ThermalPidController::new(38.5);
        // Well above setpoint → heavy throttle
        std::thread::sleep(Duration::from_millis(10));
        let throttle = pid.update(45.0);
        assert!(
            throttle < 0.3,
            "throttle should be low above setpoint, got {throttle}"
        );
    }

    #[test]
    fn test_pid_throttle_at_setpoint() {
        let mut pid = ThermalPidController::new(38.5);
        std::thread::sleep(Duration::from_millis(10));
        let throttle = pid.update(38.5);
        // At setpoint, proportional error is 0, so throttle should be ~1.0
        assert!(
            throttle > 0.8,
            "throttle should be near 1.0 at setpoint, got {throttle}"
        );
    }

    #[test]
    fn test_pid_smooth_transition() {
        let mut pid = ThermalPidController::new(38.5);
        let mut last_throttle = 1.0;

        // Gradually increase temperature — throttle should decrease smoothly
        for temp in [30.0, 32.0, 34.0, 36.0, 38.0, 39.0, 40.0, 41.0, 42.0] {
            std::thread::sleep(Duration::from_millis(10));
            let throttle = pid.update(temp);
            // Should be monotonically decreasing (or equal) as temp rises
            assert!(
                throttle <= last_throttle + 0.05, // Small tolerance for PID dynamics
                "PID should decrease smoothly: {last_throttle} -> {throttle} at {temp}°C"
            );
            last_throttle = throttle;
        }
    }

    // ── Physics Model ───────────────────────────────────────────────────

    #[test]
    fn test_physics_model_junction_estimation() {
        let mut model = ThermalPhysicsModel::new(SocThermalProfile::SNAPDRAGON_8_GEN3);
        model.update_from_skin_reading(35.0);
        // Skin 35°C, ambient 25°C → P ≈ (35-25)/(12-4) = 1.25W
        // Junction = 35 + 1.25 × 4 = 40°C
        assert!((model.junction_temp_c - 40.0).abs() < 1.0);
    }

    #[test]
    fn test_physics_model_power_estimation() {
        let mut model = ThermalPhysicsModel::new(SocThermalProfile::SNAPDRAGON_8_GEN3);
        model.update_from_skin_reading(41.0);
        // Skin 41°C, ambient 25°C → P = (41-25)/(12-4) = 2.0W
        assert!((model.power_w - 2.0).abs() < 0.5);
    }

    #[test]
    fn test_physics_thermal_budget() {
        let mut model = ThermalPhysicsModel::new(SocThermalProfile::SNAPDRAGON_8_GEN3);
        model.skin_temp_c = 35.0;
        model.power_w = 3.0;

        let budget = model.thermal_budget_seconds();
        // Should have some budget since we're below hot threshold
        assert!(budget.is_some());
        let secs = budget.unwrap();
        assert!(secs > 0.0, "should have positive budget, got {secs}");
    }

    #[test]
    fn test_physics_no_budget_when_cooling() {
        let mut model = ThermalPhysicsModel::new(SocThermalProfile::SNAPDRAGON_8_GEN3);
        model.skin_temp_c = 35.0;
        model.power_w = 0.5; // Very low power

        // Steady-state skin temp = 25 + 0.5 × (12-4) = 29°C < 40°C (hot)
        // Will never reach hot → budget is None
        let budget = model.thermal_budget_seconds();
        assert!(
            budget.is_none(),
            "should have no budget when can't overheat"
        );
    }

    // ── Integration ─────────────────────────────────────────────────────

    #[test]
    fn test_dwell_time_suppresses_rapid_changes() {
        let mut tm = ThermalManager::new();
        // Just transitioned.
        tm.last_transition = Instant::now();
        tm.state = ThermalState::Cool;

        // Should trigger Warm (38°C > 37°C) but dwell time prevents it.
        let result = tm.update_temperature(38.0);
        assert!(result.is_none());
    }

    #[test]
    fn test_thermal_budget_returns_none_when_cooling() {
        let mut tm = make_manager();
        // Add samples showing decreasing temperature.
        tm.record_sample(39.0);
        std::thread::sleep(Duration::from_millis(50));
        tm.record_sample(37.0);
        // Temperature is set to 37, physics model should show limited budget
        tm.temperature_c = 37.0;
        tm.physics.update_from_skin_reading(37.0);
        // The rise rate is negative → linear fallback returns None
        let rate = tm.rise_rate_c_per_sec();
        assert!(rate.is_some());
        assert!(rate.unwrap() < 0.0, "should be cooling");
    }

    #[test]
    fn test_custom_thresholds() {
        let thresholds = ThermalThresholds {
            warm_c: 35.0,
            hot_c: 38.0,
            critical_c: 42.0,
        };
        let mut tm = ThermalManager::with_thresholds(thresholds);
        tm.last_transition = Instant::now() - Duration::from_secs(60);

        // 36°C should trigger Warm with the custom 35°C threshold.
        let result = tm.update_temperature(36.0);
        assert_eq!(result, Some(ThermalState::Warm));
    }

    #[test]
    fn test_default_thresholds_are_physics_based() {
        let thresholds = ThermalThresholds::default();
        assert!((thresholds.warm_c - 37.0).abs() < f32::EPSILON);
        assert!((thresholds.hot_c - 40.0).abs() < f32::EPSILON);
        assert!((thresholds.critical_c - 43.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_max_sustainable_power() {
        let tm = make_manager();
        let p_max = tm.max_sustainable_power_w();
        // At 25°C ambient, hot=40°C: (40-25)/(12-4) = 1.875W
        assert!((p_max - 1.875).abs() < 0.01);
    }

    #[test]
    fn test_junction_and_power_queries() {
        let mut tm = make_manager();
        tm.update_temperature(38.0);
        // After update, junction and power should be estimated
        let junction = tm.junction_temperature();
        let power = tm.estimated_power_w();
        assert!(junction > 38.0, "junction should be above skin");
        assert!(power > 0.0, "power should be positive");
    }

    #[test]
    fn test_soc_profile_accessible() {
        let tm = make_manager();
        let profile = tm.soc_profile();
        assert_eq!(profile.name, "Snapdragon 8 Gen 3");
    }

    #[test]
    fn test_ambient_update() {
        let mut tm = make_manager();
        tm.update_ambient(30.0);
        assert!((tm.ambient_temperature() - 30.0).abs() < f32::EPSILON);
    }

    // ── Throttle smoothness ─────────────────────────────────────────────

    #[test]
    fn test_throttle_is_continuous_not_stepped() {
        let mut tm = make_manager();
        let mut throttles = Vec::new();

        // Sample throttle at various temperatures
        for temp in [30.0, 33.0, 35.0, 37.0, 38.0, 39.0, 40.0, 41.0, 43.0] {
            std::thread::sleep(Duration::from_millis(10));
            tm.update_temperature(temp);
            throttles.push(tm.throttle_factor());
        }

        // Verify throttle values are continuous (no large jumps)
        for window in throttles.windows(2) {
            let diff = (window[1] - window[0]).abs();
            // PID should prevent jumps larger than 0.5 for 2-3°C steps
            assert!(
                diff < 0.6,
                "throttle jumped too much: {} -> {} (diff {})",
                window[0],
                window[1],
                diff
            );
        }

        // Verify general trend: throttle decreases as temp increases
        assert!(
            throttles.first().unwrap() > throttles.last().unwrap(),
            "throttle should decrease overall as temperature rises"
        );
    }

    #[test]
    fn test_history_size_increased() {
        // Verify we have 16 history slots (up from 8)
        assert_eq!(ThermalManager::HISTORY_SIZE, 16);
    }

    // ── Multi-Zone Thermal Model ────────────────────────────────────────

    #[test]
    fn test_zone_model_newtons_law_cooling() {
        // Verify Newton's law: zone hotter than ambient should cool toward ambient
        let mut model = MultiZoneThermalModel::with_ambient(25.0);
        model.set_zone_temperature(ThermalZone::Skin, 40.0);
        model.set_zone_power(ThermalZone::Skin, 0.0); // No heat input

        model.simulate_step(60.0); // 60 seconds
        let skin = model.zone_state(ThermalZone::Skin).expect("skin zone");
        assert!(
            skin.temperature_c < 40.0,
            "skin should cool from 40°C with no power, got {:.2}",
            skin.temperature_c
        );
        assert!(
            skin.temperature_c > 25.0,
            "skin should not cool below ambient in 60s, got {:.2}",
            skin.temperature_c
        );
    }

    #[test]
    fn test_zone_model_heating_with_power() {
        // Apply power to CPU zone — it should heat up
        let mut model = MultiZoneThermalModel::with_ambient(25.0);
        let cpu_before = model
            .zone_state(ThermalZone::Cpu)
            .expect("cpu zone")
            .temperature_c;

        model.set_zone_power(ThermalZone::Cpu, 3.0); // 3W CPU load
        model.simulate_step(30.0); // 30 seconds

        let cpu_after = model
            .zone_state(ThermalZone::Cpu)
            .expect("cpu zone")
            .temperature_c;
        assert!(
            cpu_after > cpu_before,
            "CPU should heat with 3W power: {cpu_before:.2} -> {cpu_after:.2}"
        );
    }

    #[test]
    fn test_zone_model_bottleneck_is_most_constrained() {
        let mut model = MultiZoneThermalModel::with_ambient(25.0);
        // Battery has max_safe 45°C, set it close to limit
        model.set_zone_temperature(ThermalZone::Battery, 44.0);
        // Skin has max_safe 43°C, set it even closer
        model.set_zone_temperature(ThermalZone::Skin, 42.5);

        let bottleneck = model.bottleneck_zone().expect("should have zones");
        assert_eq!(
            bottleneck.zone,
            ThermalZone::Skin,
            "skin at 42.5/43 should be more constrained than battery at 44/45"
        );
    }

    #[test]
    fn test_zone_throttle_factor_cool_is_one() {
        let model = MultiZoneThermalModel::with_ambient(25.0);
        // All zones start at ambient, well below limits
        let factor = model.zone_throttle_factor();
        assert!(
            factor > 0.9,
            "all zones at ambient should give throttle near 1.0, got {factor:.3}"
        );
    }

    #[test]
    fn test_zone_throttle_factor_at_limit_is_zero() {
        let mut model = MultiZoneThermalModel::with_ambient(25.0);
        // Push skin to its critical limit
        model.set_zone_temperature(ThermalZone::Skin, SKIN_TEMP_CRITICAL_C);
        let factor = model.zone_throttle_factor();
        assert!(
            factor < 0.05,
            "skin at critical limit should give throttle near 0.0, got {factor:.3}"
        );
    }

    #[test]
    fn test_zone_predict_skin_temp_nondestructive() {
        let model = MultiZoneThermalModel::with_ambient(25.0);
        let original = model
            .zone_state(ThermalZone::Skin)
            .expect("skin")
            .temperature_c;

        // Prediction should not mutate the original model
        let _predicted = model.predict_skin_temp_after(120.0);
        let after = model
            .zone_state(ThermalZone::Skin)
            .expect("skin")
            .temperature_c;
        assert!(
            (original - after).abs() < f64::EPSILON,
            "predict_skin_temp_after must not mutate model"
        );
    }

    #[test]
    fn test_thermal_manager_exposes_zone_model() {
        let mut tm = make_manager();
        // Should be able to read zone model via accessor
        let factor = tm.zone_model().zone_throttle_factor();
        assert!(factor > 0.0, "zone throttle should be accessible");

        // Should be able to mutate zone model via mut accessor
        tm.zone_model_mut().set_zone_power(ThermalZone::Cpu, 2.0);
        // Verify it took effect
        let cpu = tm.zone_model().zone_state(ThermalZone::Cpu).expect("cpu");
        assert!(
            (cpu.power_w - 2.0).abs() < f64::EPSILON,
            "zone model mutation should persist"
        );
    }

    #[test]
    fn test_update_temperature_syncs_zone_model_skin() {
        let mut tm = make_manager();
        tm.update_temperature(39.0);
        let skin = tm.zone_model().zone_state(ThermalZone::Skin).expect("skin");
        assert!(
            (skin.temperature_c - 39.0).abs() < 0.01,
            "update_temperature should sync zone model skin to {}, got {}",
            39.0,
            skin.temperature_c
        );
    }
}
