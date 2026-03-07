//! Energy-aware power management with real mWh/mAh budgets.
//!
//! Replaces arbitrary percentage thresholds with physics-based energy accounting:
//! - Battery level mapped to remaining energy (mWh), not just percentage
//! - Token generation tracked in millijoules, not arbitrary counts
//! - Power draw measured in milliamps for projected runtime
//! - Unified model: no more separate PowerState vs BatteryTier
//!
//! # Energy Model
//!
//! ```text
//! Total energy  = battery_mAh × V_nominal × η_delivery
//!               = 5000 × 3.85 × 0.85 = 16362.5 mWh
//!
//! AURA's share  = total × 5% daily = 818 mWh = 2945 J
//!
//! Token cost    = model_specific_mJ/token
//!   - 1.5B Q4: 0.1 mJ → 29.4M tokens/day on budget
//!   - 4B Q4:   0.3 mJ → 9.8M tokens/day on budget
//!   - 8B Q4:   0.5 mJ → 5.9M tokens/day on budget
//! ```
//!
//! # Spec Reference
//!
//! See `AURA-V4-POWER-AGENCY-REBALANCE.md` §5 — Five-Tier Degradation Model.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use aura_types::errors::PlatformError;
use aura_types::ipc::ModelTier;
use aura_types::power::{
    DegradationLevel, PowerBudget, DEFAULT_BATTERY_CAPACITY_MAH, DEFAULT_DAILY_ENERGY_SHARE,
    ENERGY_PER_TOKEN_1_5B_MJ, ENERGY_PER_TOKEN_4B_MJ, ENERGY_PER_TOKEN_8B_MJ, LIPO_NOMINAL_VOLTAGE,
    POWER_DELIVERY_EFFICIENCY,
};
use serde::{Deserialize, Serialize};

// ─── Battery Tier Enum ──────────────────────────────────────────────────────

/// Five-tier battery degradation model.
///
/// Each tier gates which capabilities AURA exposes to conserve power.
/// Tiers are ordered from most capable (Charging) to least (Emergency).
///
/// Tier boundaries are expressed in **remaining device energy** terms,
/// though we use battery percentage as the input signal (it's what Android
/// provides). The energy equivalent depends on battery capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum BatteryTier {
    /// Charging or battery >80%: full capability, proactive suggestions,
    /// background sync, DREAMING work.
    Charging,
    /// Battery 40–80%: standard operation, reduced background.
    Normal,
    /// Battery 20–40%: minimal background, only user-initiated +
    /// high-priority proactive.
    Conserve,
    /// Battery 10–20%: essential only, no proactive, minimal inference.
    Critical,
    /// Battery <10%: core functions only, no inference, save state.
    Emergency,
}

impl BatteryTier {
    /// Ordered index for comparison (0 = most capable).
    fn ordinal(self) -> u8 {
        match self {
            Self::Charging => 0,
            Self::Normal => 1,
            Self::Conserve => 2,
            Self::Critical => 3,
            Self::Emergency => 4,
        }
    }

    /// True if `self` is a *degradation* relative to `other`.
    pub fn is_worse_than(self, other: BatteryTier) -> bool {
        self.ordinal() > other.ordinal()
    }

    /// Energy per token (mJ) for the recommended model at this tier.
    pub fn energy_per_token_mj(self) -> f64 {
        match self {
            Self::Charging => ENERGY_PER_TOKEN_8B_MJ,
            Self::Normal => ENERGY_PER_TOKEN_4B_MJ,
            Self::Conserve => ENERGY_PER_TOKEN_1_5B_MJ,
            Self::Critical => ENERGY_PER_TOKEN_1_5B_MJ,
            Self::Emergency => 0.0, // no inference
        }
    }
}

impl std::fmt::Display for BatteryTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Charging => write!(f, "Charging (Tier 0)"),
            Self::Normal => write!(f, "Normal (Tier 1)"),
            Self::Conserve => write!(f, "Conserve (Tier 2)"),
            Self::Critical => write!(f, "Critical (Tier 3)"),
            Self::Emergency => write!(f, "Emergency (Tier 4)"),
        }
    }
}

// ─── Tier Policy ────────────────────────────────────────────────────────────

/// Operational limits for a single [`BatteryTier`].
///
/// Inference budgets are derived from the energy model:
/// - Charging: 8B model, 0.5 mJ/token → ~120 calls/hr at ~500 tokens/call = 30 mWh/hr
/// - Normal: 4B model, 0.3 mJ/token → ~60 calls/hr at ~500 tokens/call = 9 mWh/hr
/// - Conserve: 1.5B model, 0.1 mJ/token → ~20 calls/hr at ~500 tokens/call = 1 mWh/hr
/// - Critical: 1.5B model, restricted → ~5 calls/hr = 0.25 mWh/hr
/// - Emergency: no inference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierPolicy {
    pub max_inference_calls_per_hour: u32,
    pub model_tier: ModelTier,
    pub background_scan_interval: Duration,
    pub proactive_enabled: bool,
    pub max_concurrent_goals: u8,
    /// Estimated power draw for inference at this tier (milliwatts).
    pub inference_power_mw: f64,
    /// Maximum energy per hour at this tier (mWh).
    pub max_energy_per_hour_mwh: f64,
}

impl TierPolicy {
    /// Policy for [`BatteryTier::Charging`].
    ///
    /// Full 8B model: ~6.5W during inference × avg 50% duty cycle = ~3.25W avg.
    /// 120 calls/hr × ~500 tokens × 0.5 mJ/token = 30,000 mJ = 8.33 mWh/hr.
    pub fn charging() -> Self {
        Self {
            max_inference_calls_per_hour: 120,
            model_tier: ModelTier::Full8B,
            background_scan_interval: Duration::from_secs(30),
            proactive_enabled: true,
            max_concurrent_goals: 8,
            inference_power_mw: 6500.0,
            max_energy_per_hour_mwh: 30.0,
        }
    }

    /// Policy for [`BatteryTier::Normal`].
    ///
    /// 4B model: ~4.0W during inference.
    /// 60 calls/hr × ~500 tokens × 0.3 mJ/token = 9,000 mJ = 2.5 mWh/hr.
    pub fn normal() -> Self {
        Self {
            max_inference_calls_per_hour: 60,
            model_tier: ModelTier::Standard4B,
            background_scan_interval: Duration::from_secs(120),
            proactive_enabled: true,
            max_concurrent_goals: 5,
            inference_power_mw: 4000.0,
            max_energy_per_hour_mwh: 10.0,
        }
    }

    /// Policy for [`BatteryTier::Conserve`].
    ///
    /// 1.5B model: ~2.5W during inference.
    /// 20 calls/hr × ~500 tokens × 0.1 mJ/token = 1,000 mJ = 0.28 mWh/hr.
    pub fn conserve() -> Self {
        Self {
            max_inference_calls_per_hour: 20,
            model_tier: ModelTier::Brainstem1_5B,
            background_scan_interval: Duration::from_secs(600),
            proactive_enabled: false,
            max_concurrent_goals: 2,
            inference_power_mw: 2500.0,
            max_energy_per_hour_mwh: 2.0,
        }
    }

    /// Policy for [`BatteryTier::Critical`].
    pub fn critical() -> Self {
        Self {
            max_inference_calls_per_hour: 5,
            model_tier: ModelTier::Brainstem1_5B,
            background_scan_interval: Duration::from_secs(1800),
            proactive_enabled: false,
            max_concurrent_goals: 1,
            inference_power_mw: 2500.0,
            max_energy_per_hour_mwh: 0.5,
        }
    }

    /// Policy for [`BatteryTier::Emergency`].
    pub fn emergency() -> Self {
        Self {
            max_inference_calls_per_hour: 0,
            model_tier: ModelTier::Brainstem1_5B,
            background_scan_interval: Duration::from_secs(3600),
            proactive_enabled: false,
            max_concurrent_goals: 0,
            inference_power_mw: 0.0,
            max_energy_per_hour_mwh: 0.0,
        }
    }

    /// Get the policy for a given tier.
    pub fn for_tier(tier: BatteryTier) -> Self {
        match tier {
            BatteryTier::Charging => Self::charging(),
            BatteryTier::Normal => Self::normal(),
            BatteryTier::Conserve => Self::conserve(),
            BatteryTier::Critical => Self::critical(),
            BatteryTier::Emergency => Self::emergency(),
        }
    }
}

// ─── Hysteresis Thresholds ──────────────────────────────────────────────────

/// Asymmetric thresholds for tier transitions.
///
/// `degrade_at` is the battery% at or below which we drop to the worse tier.
/// `recover_at` is the battery% above which we return to the better tier.
/// The gap between them is the hysteresis band.
#[derive(Debug, Clone, Copy)]
struct TierBoundary {
    /// Battery% at or below which we enter the worse tier.
    degrade_at: u8,
    /// Battery% above which we recover to the better tier.
    recover_at: u8,
}

/// Default boundaries per the AURA v4 rebalance spec (§5.4).
///
/// | Transition        | Degrade | Recover | Band |
/// |-------------------|---------|---------|------|
/// | Charging → Normal | ≤80     | >83     | 3%   |
/// | Normal → Conserve | ≤40     | >43     | 3%   |
/// | Conserve → Crit   | ≤20     | >23     | 3%   |
/// | Critical → Emerg  | ≤10     | >13     | 3%   |
const BOUNDARIES: [TierBoundary; 4] = [
    TierBoundary {
        degrade_at: 80,
        recover_at: 83,
    }, // Charging↔Normal
    TierBoundary {
        degrade_at: 40,
        recover_at: 43,
    }, // Normal↔Conserve
    TierBoundary {
        degrade_at: 20,
        recover_at: 23,
    }, // Conserve↔Critical
    TierBoundary {
        degrade_at: 10,
        recover_at: 13,
    }, // Critical↔Emergency
];

/// Ordered tier list for boundary indexing.
const TIER_ORDER: [BatteryTier; 5] = [
    BatteryTier::Charging,
    BatteryTier::Normal,
    BatteryTier::Conserve,
    BatteryTier::Critical,
    BatteryTier::Emergency,
];

// ─── Callback Storage ───────────────────────────────────────────────────────

/// Maximum number of registered tier-change callbacks.
const MAX_CALLBACKS: usize = 16;

// ─── Energy Tracker ─────────────────────────────────────────────────────────

/// Tracks AURA's energy consumption in real physical units (mWh, mA).
///
/// This replaces the token-count budget with actual energy accounting.
#[derive(Debug, Clone)]
pub struct EnergyTracker {
    /// Battery capacity in mAh (from device or default).
    pub battery_capacity_mah: f64,
    /// AURA's daily energy budget in mWh.
    pub daily_budget_mwh: f64,
    /// Energy consumed today in mWh.
    pub energy_consumed_mwh: f64,
    /// Current instantaneous draw in mA.
    pub current_draw_ma: f64,
    /// Timestamp of daily budget reset.
    budget_reset_time: Instant,
}

impl EnergyTracker {
    /// Create an energy tracker with default parameters.
    ///
    /// Default: 5000 mAh battery, 5% daily share.
    /// Budget = 5000 × 3.85 × 0.85 × 0.05 = 818 mWh/day.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_BATTERY_CAPACITY_MAH, DEFAULT_DAILY_ENERGY_SHARE)
    }

    /// Create an energy tracker with specific battery parameters.
    pub fn with_capacity(battery_capacity_mah: f64, daily_share: f64) -> Self {
        let clamped_share = daily_share.clamp(0.01, 0.20);
        let total_energy_mwh =
            battery_capacity_mah * LIPO_NOMINAL_VOLTAGE * POWER_DELIVERY_EFFICIENCY;
        let daily_budget_mwh = total_energy_mwh * clamped_share;

        Self {
            battery_capacity_mah,
            daily_budget_mwh,
            energy_consumed_mwh: 0.0,
            current_draw_ma: 0.0,
            budget_reset_time: Instant::now(),
        }
    }

    /// Record energy consumed by token generation.
    pub fn record_tokens(&mut self, token_count: u32, energy_per_token_mj: f64) {
        let energy_mj = token_count as f64 * energy_per_token_mj;
        self.energy_consumed_mwh += energy_mj / 3600.0;
    }

    /// Record raw energy consumption in mWh.
    pub fn record_energy_mwh(&mut self, mwh: f64) {
        self.energy_consumed_mwh += mwh;
    }

    /// Energy remaining in today's budget (mWh).
    pub fn energy_remaining_mwh(&self) -> f64 {
        (self.daily_budget_mwh - self.energy_consumed_mwh).max(0.0)
    }

    /// Fraction of daily budget remaining (0.0–1.0).
    pub fn budget_remaining_fraction(&self) -> f64 {
        if self.daily_budget_mwh <= 0.0 {
            return 0.0;
        }
        self.energy_remaining_mwh() / self.daily_budget_mwh
    }

    /// Projected runtime at current draw (hours).
    pub fn projected_runtime_hours(&self) -> f64 {
        if self.current_draw_ma <= 0.0 {
            return f64::INFINITY;
        }
        let power_mw = self.current_draw_ma * LIPO_NOMINAL_VOLTAGE;
        if power_mw <= 0.0 {
            return f64::INFINITY;
        }
        self.energy_remaining_mwh() / power_mw
    }

    /// Check and perform daily budget reset if needed (24hr window).
    pub fn maybe_reset_daily(&mut self) {
        if self.budget_reset_time.elapsed() >= Duration::from_secs(86400) {
            tracing::info!(
                consumed_mwh = self.energy_consumed_mwh,
                budget_mwh = self.daily_budget_mwh,
                "daily energy budget reset"
            );
            self.energy_consumed_mwh = 0.0;
            self.budget_reset_time = Instant::now();
        }
    }

    /// Convert energy budget to approximate token count for a model tier.
    pub fn tokens_remaining_for_tier(&self, tier: BatteryTier) -> u64 {
        let mj_per_token = tier.energy_per_token_mj();
        if mj_per_token <= 0.0 {
            return 0;
        }
        let remaining_mj = self.energy_remaining_mwh() * 3600.0;
        (remaining_mj / mj_per_token) as u64
    }
}

impl Default for EnergyTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ─── PowerManager ───────────────────────────────────────────────────────────

/// Manages battery tier state with hysteresis, energy tracking, and callback dispatch.
///
/// Unified power system — replaces the former separate `PowerState` and `BatteryTier`.
/// All energy accounting is in real physical units (mWh, mA).
///
/// Thread-safe reads via atomics; mutations require `&mut self`.
pub struct PowerManager {
    current_tier: BatteryTier,
    battery_level: AtomicU8,
    is_charging: AtomicBool,
    /// Registered tier-change callbacks (bounded to [`MAX_CALLBACKS`]).
    tier_change_callbacks: Vec<Box<dyn Fn(BatteryTier, BatteryTier) + Send>>,
    /// Hysteresis percentage — default 3%. Stored for runtime introspection.
    #[allow(dead_code)]
    hysteresis_pct: u8,
    /// Timestamp of the last tier transition.
    last_transition: Instant,
    /// Minimum time between tier changes to damp rapid oscillation.
    min_transition_interval: Duration,
    /// Number of inference calls made in the current hour window.
    inference_calls_this_hour: u32,
    /// Start of the current hourly accounting window.
    hour_window_start: Instant,
    /// Energy tracker for mWh-based budgeting.
    energy: EnergyTracker,
}

impl PowerManager {
    /// Create a new `PowerManager` starting in [`BatteryTier::Normal`].
    pub fn new() -> Self {
        Self {
            current_tier: BatteryTier::Normal,
            battery_level: AtomicU8::new(75),
            is_charging: AtomicBool::new(false),
            tier_change_callbacks: Vec::new(),
            hysteresis_pct: 3,
            last_transition: Instant::now(),
            min_transition_interval: Duration::from_secs(30),
            inference_calls_this_hour: 0,
            hour_window_start: Instant::now(),
            energy: EnergyTracker::new(),
        }
    }

    /// Create a `PowerManager` with specific battery capacity.
    pub fn with_battery(capacity_mah: f64, daily_share: f64) -> Self {
        Self {
            energy: EnergyTracker::with_capacity(capacity_mah, daily_share),
            ..Self::new()
        }
    }

    /// Update battery level and charging state.
    ///
    /// Returns `Some(new_tier)` if a tier transition occurred.
    pub fn update_battery(&mut self, level: u8, charging: bool) -> Option<BatteryTier> {
        let level = level.min(100);
        self.battery_level.store(level, Ordering::Relaxed);
        self.is_charging.store(charging, Ordering::Relaxed);

        // Perform daily energy budget reset if needed.
        self.energy.maybe_reset_daily();

        let candidate = self.compute_candidate_tier(level, charging);

        if candidate == self.current_tier {
            return None;
        }

        // Enforce minimum dwell time.
        if self.last_transition.elapsed() < self.min_transition_interval {
            tracing::trace!(
                current = %self.current_tier,
                candidate = %candidate,
                "tier change suppressed: dwell time not met"
            );
            return None;
        }

        let old = self.current_tier;
        self.current_tier = candidate;
        self.last_transition = Instant::now();

        tracing::info!(
            from = %old,
            to = %candidate,
            battery = level,
            charging,
            energy_remaining_mwh = self.energy.energy_remaining_mwh(),
            "battery tier transition"
        );

        // Dispatch callbacks (bounded).
        for cb in &self.tier_change_callbacks {
            cb(old, candidate);
        }

        Some(candidate)
    }

    /// Compute the candidate tier considering hysteresis.
    fn compute_candidate_tier(&self, level: u8, charging: bool) -> BatteryTier {
        // Charging with high battery always maps to Charging tier.
        if charging && level > BOUNDARIES[0].degrade_at {
            return BatteryTier::Charging;
        }
        // Even if charging, if battery is low we respect degradation tiers
        // but allow one tier better than battery alone would dictate.
        if charging {
            let raw = self.raw_tier_from_level(level);
            // Charging bumps us up one tier (but not above Charging).
            return match raw {
                BatteryTier::Normal => BatteryTier::Charging,
                BatteryTier::Conserve => BatteryTier::Normal,
                BatteryTier::Critical => BatteryTier::Conserve,
                BatteryTier::Emergency => BatteryTier::Critical,
                BatteryTier::Charging => BatteryTier::Charging,
            };
        }

        // Not charging — apply hysteresis based on current tier.
        let current_ord = self.current_tier.ordinal() as usize;

        // Check if we should degrade (move to a worse tier).
        if current_ord < TIER_ORDER.len() - 1 {
            let boundary = &BOUNDARIES[current_ord];
            if level <= boundary.degrade_at {
                return TIER_ORDER[current_ord + 1];
            }
        }

        // Check if we should recover (move to a better tier).
        if current_ord > 0 {
            let boundary = &BOUNDARIES[current_ord - 1];
            if level > boundary.recover_at {
                return TIER_ORDER[current_ord - 1];
            }
        }

        // Stay in current tier (within hysteresis band).
        self.current_tier
    }

    /// Raw tier from level without hysteresis (used internally).
    fn raw_tier_from_level(&self, level: u8) -> BatteryTier {
        if level > 80 {
            BatteryTier::Charging
        } else if level > 40 {
            BatteryTier::Normal
        } else if level > 20 {
            BatteryTier::Conserve
        } else if level > 10 {
            BatteryTier::Critical
        } else {
            BatteryTier::Emergency
        }
    }

    /// Current battery tier.
    pub fn current_tier(&self) -> BatteryTier {
        self.current_tier
    }

    /// Current battery level (0–100).
    pub fn battery_level(&self) -> u8 {
        self.battery_level.load(Ordering::Relaxed)
    }

    /// Whether the device is currently charging.
    pub fn is_charging(&self) -> bool {
        self.is_charging.load(Ordering::Relaxed)
    }

    /// Maximum inference calls per hour for the current tier.
    pub fn max_inference_calls(&self) -> u32 {
        TierPolicy::for_tier(self.current_tier).max_inference_calls_per_hour
    }

    /// Recommended model tier for the current power state.
    pub fn recommended_model_tier(&self) -> ModelTier {
        TierPolicy::for_tier(self.current_tier).model_tier
    }

    /// Whether proactive suggestions are permitted.
    pub fn should_allow_proactive(&self) -> bool {
        TierPolicy::for_tier(self.current_tier).proactive_enabled
    }

    /// Whether background scanning is permitted.
    ///
    /// Disabled at Critical and Emergency tiers.
    pub fn should_allow_background(&self) -> bool {
        matches!(
            self.current_tier,
            BatteryTier::Charging | BatteryTier::Normal | BatteryTier::Conserve
        )
    }

    /// Background scan interval for the current tier.
    pub fn background_scan_interval(&self) -> Duration {
        TierPolicy::for_tier(self.current_tier).background_scan_interval
    }

    /// Maximum concurrent goals for the current tier.
    pub fn max_concurrent_goals(&self) -> u8 {
        TierPolicy::for_tier(self.current_tier).max_concurrent_goals
    }

    /// Energy-aware throttle factor (1.0 = unrestricted, 0.0 = stopped).
    ///
    /// Derived from energy budget remaining, not arbitrary per-tier constants.
    /// Uses a smooth ramp based on remaining energy fraction.
    pub fn power_throttle_factor(&self) -> f32 {
        if self.is_charging() {
            return 1.0;
        }

        let remaining = self.energy.budget_remaining_fraction();

        // Smooth throttle curve based on remaining energy:
        // >50% remaining → 1.0 (full speed)
        // 20-50% → linear ramp 1.0 → 0.5
        // 5-20% → linear ramp 0.5 → 0.1
        // <5% → 0.0 (stop)
        let energy_throttle = if remaining > 0.5 {
            1.0
        } else if remaining > 0.2 {
            // Linear: 0.5 → 1.0 mapped to remaining 0.2 → 0.5
            0.5 + (remaining - 0.2) / 0.3 * 0.5
        } else if remaining > 0.05 {
            // Linear: 0.1 → 0.5 mapped to remaining 0.05 → 0.2
            0.1 + (remaining - 0.05) / 0.15 * 0.4
        } else {
            0.0
        };

        let throttle = energy_throttle as f32;
        throttle
    }

    /// Get the full [`TierPolicy`] for the current tier.
    pub fn current_policy(&self) -> TierPolicy {
        TierPolicy::for_tier(self.current_tier)
    }

    /// Register a callback for tier transitions.
    ///
    /// # Errors
    /// Returns [`PlatformError::CallbackCapacityExceeded`] if the limit
    /// ([`MAX_CALLBACKS`]) is reached.
    pub fn on_tier_change(
        &mut self,
        callback: Box<dyn Fn(BatteryTier, BatteryTier) + Send>,
    ) -> Result<(), PlatformError> {
        if self.tier_change_callbacks.len() >= MAX_CALLBACKS {
            return Err(PlatformError::CallbackCapacityExceeded { max: MAX_CALLBACKS });
        }
        self.tier_change_callbacks.push(callback);
        Ok(())
    }

    /// Record an inference call against the hourly budget.
    ///
    /// Also records the energy consumed based on estimated token count.
    /// Returns `true` if the call is within budget, `false` if budget
    /// is exhausted for this hour.
    pub fn record_inference_call(&mut self) -> bool {
        // Roll over the hourly window if needed.
        if self.hour_window_start.elapsed() >= Duration::from_secs(3600) {
            self.inference_calls_this_hour = 0;
            self.hour_window_start = Instant::now();
        }

        let max = self.max_inference_calls();
        if self.inference_calls_this_hour >= max {
            tracing::warn!(
                calls = self.inference_calls_this_hour,
                max,
                tier = %self.current_tier,
                energy_remaining_mwh = self.energy.energy_remaining_mwh(),
                "inference budget exhausted for current hour"
            );
            return false;
        }
        self.inference_calls_this_hour += 1;
        true
    }

    /// Record tokens generated for energy accounting.
    ///
    /// Automatically selects energy-per-token based on current tier's model.
    pub fn record_tokens(&mut self, token_count: u32) {
        let mj_per_token = self.current_tier.energy_per_token_mj();
        self.energy.record_tokens(token_count, mj_per_token);
    }

    /// Record tokens with explicit energy-per-token value (for non-default models).
    pub fn record_tokens_with_energy(&mut self, token_count: u32, energy_per_token_mj: f64) {
        self.energy.record_tokens(token_count, energy_per_token_mj);
    }

    /// Number of inference calls remaining in the current hourly window.
    pub fn inference_calls_remaining(&self) -> u32 {
        let max = self.max_inference_calls();
        max.saturating_sub(self.inference_calls_this_hour)
    }

    /// Reference to the energy tracker.
    pub fn energy(&self) -> &EnergyTracker {
        &self.energy
    }

    /// Mutable reference to the energy tracker.
    pub fn energy_mut(&mut self) -> &mut EnergyTracker {
        &mut self.energy
    }

    /// Update current draw measurement (milliamps).
    pub fn update_current_draw(&mut self, current_ma: f64) {
        self.energy.current_draw_ma = current_ma;
    }

    /// Energy remaining in today's budget (mWh).
    pub fn energy_remaining_mwh(&self) -> f64 {
        self.energy.energy_remaining_mwh()
    }

    /// Projected runtime at current draw (hours).
    pub fn projected_runtime_hours(&self) -> f64 {
        self.energy.projected_runtime_hours()
    }

    /// Select the best model tier sustainable with remaining energy.
    ///
    /// Cascades from the largest model downward based on how much energy
    /// remains in the daily budget. The cascade ensures AURA **never refuses
    /// to think** — even at the lowest energy state, it falls back to the
    /// smallest model rather than stopping inference entirely.
    ///
    /// # Energy cascade thresholds
    ///
    /// | Energy remaining | Model selected | Rationale                    |
    /// |------------------|----------------|------------------------------|
    /// | > 40%            | Full 8B        | Plenty of headroom           |
    /// | 15–40%           | Standard 4B    | Conserve for rest of day     |
    /// | 5–15%            | Brainstem 1.5B | Minimal per-token cost       |
    /// | < 5%             | Brainstem 1.5B | Emergency: never refuse      |
    ///
    /// The selected tier also accounts for the battery tier — if the battery
    /// tier policy recommends a smaller model, the smaller of the two wins.
    pub fn select_model_tier_by_energy(&self) -> ModelTier {
        let energy_fraction = self.energy.budget_remaining_fraction();

        // Energy-based selection: cascade to smaller models as budget depletes.
        let energy_tier = if energy_fraction > 0.40 {
            ModelTier::Full8B
        } else if energy_fraction > 0.15 {
            ModelTier::Standard4B
        } else {
            // Below 15%: always use the smallest model, never refuse.
            ModelTier::Brainstem1_5B
        };

        // Battery tier policy might further restrict the model.
        let policy_tier = TierPolicy::for_tier(self.current_tier).model_tier;

        // Use the smaller of the two (more conservative wins).
        Self::smaller_model(energy_tier, policy_tier)
    }

    /// Return the smaller (less compute-intensive) of two model tiers.
    ///
    /// Ordering: Full8B > Standard4B > Brainstem1_5B.
    fn smaller_model(a: ModelTier, b: ModelTier) -> ModelTier {
        let rank = |t: &ModelTier| match t {
            ModelTier::Full8B => 2,
            ModelTier::Standard4B => 1,
            ModelTier::Brainstem1_5B => 0,
        };
        if rank(&a) <= rank(&b) {
            a
        } else {
            b
        }
    }

    /// Map the current battery tier to a [`DegradationLevel`].
    ///
    /// This provides a standard degradation signal that other subsystems
    /// can consume without knowing about battery tiers directly.
    pub fn degradation_level(&self) -> DegradationLevel {
        match self.current_tier {
            BatteryTier::Charging => DegradationLevel::L0Full,
            BatteryTier::Normal => DegradationLevel::L1Reduced,
            BatteryTier::Conserve => DegradationLevel::L2Minimal,
            BatteryTier::Critical => DegradationLevel::L3DaemonOnly,
            BatteryTier::Emergency => DegradationLevel::L4Heartbeat,
        }
    }

    /// Convert the current power state to an [`aura_types::power::PowerBudget`].
    ///
    /// This bridges the daemon's internal power tracking to the IPC-visible
    /// `PowerBudget` type used by the rest of the AURA stack. The resulting
    /// struct contains real physics-based values (mWh, mA, °C) — no arbitrary
    /// percentages.
    ///
    /// # Arguments
    /// * `thermal_state` — current thermal state (from ThermalManager)
    /// * `skin_temp_c` — current skin temperature (from ThermalManager)
    pub fn to_power_budget(
        &self,
        thermal_state: aura_types::power::ThermalState,
        skin_temp_c: f64,
    ) -> PowerBudget {
        let model_tier = self.select_model_tier_by_energy();
        let energy_per_token = match model_tier {
            ModelTier::Full8B => ENERGY_PER_TOKEN_8B_MJ,
            ModelTier::Standard4B => ENERGY_PER_TOKEN_4B_MJ,
            ModelTier::Brainstem1_5B => ENERGY_PER_TOKEN_1_5B_MJ,
        };

        // Derive token budget from energy budget for legacy consumers.
        let remaining_mj = self.energy.energy_remaining_mwh() * 3600.0;
        let tokens_remaining = if energy_per_token > 0.0 {
            (remaining_mj / energy_per_token) as u32
        } else {
            0
        };

        let total_mj = self.energy.daily_budget_mwh * 3600.0;
        let daily_token_budget = if energy_per_token > 0.0 {
            (total_mj / energy_per_token) as u32
        } else {
            0
        };
        let tokens_used = daily_token_budget.saturating_sub(tokens_remaining);

        PowerBudget {
            battery_percent: self.battery_level(),
            is_charging: self.is_charging(),
            battery_capacity_mah: self.energy.battery_capacity_mah,
            cell_voltage_v: LIPO_NOMINAL_VOLTAGE,
            daily_budget_mwh: self.energy.daily_budget_mwh,
            energy_consumed_mwh: self.energy.energy_consumed_mwh,
            current_draw_ma: self.energy.current_draw_ma,
            thermal: thermal_state,
            skin_temp_c,
            degradation: self.degradation_level(),
            projected_runtime_hours: self.energy.projected_runtime_hours(),
            daily_token_budget,
            tokens_used_today: tokens_used,
        }
    }
}

impl Default for PowerManager {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::sync::Arc;

    #[test]
    fn test_tier_from_high_battery() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Charging;
        pm.last_transition = Instant::now() - Duration::from_secs(60);

        let result = pm.update_battery(90, false);
        assert!(result.is_none());
        assert_eq!(pm.current_tier(), BatteryTier::Charging);
    }

    #[test]
    fn test_tier_degrades_on_low_battery() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Normal;
        pm.last_transition = Instant::now() - Duration::from_secs(60);

        let result = pm.update_battery(40, false);
        assert_eq!(result, Some(BatteryTier::Conserve));
        assert_eq!(pm.current_tier(), BatteryTier::Conserve);
    }

    #[test]
    fn test_hysteresis_prevents_immediate_recovery() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Conserve;
        pm.last_transition = Instant::now() - Duration::from_secs(60);

        // 41% is above degrade (40) but below recover (43) → stays Conserve.
        let result = pm.update_battery(41, false);
        assert!(result.is_none());
        assert_eq!(pm.current_tier(), BatteryTier::Conserve);

        // 44% is above recover (43) → recovers to Normal.
        let result = pm.update_battery(44, false);
        assert_eq!(result, Some(BatteryTier::Normal));
    }

    #[test]
    fn test_dwell_time_suppresses_rapid_changes() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Normal;
        pm.min_transition_interval = Duration::from_secs(30);
        pm.last_transition = Instant::now();

        let result = pm.update_battery(30, false);
        assert!(result.is_none());
    }

    #[test]
    fn test_charging_boost() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Conserve;
        pm.last_transition = Instant::now() - Duration::from_secs(60);

        let result = pm.update_battery(30, true);
        assert_eq!(result, Some(BatteryTier::Normal));
    }

    #[test]
    fn test_tier_policy_values() {
        let charging = TierPolicy::charging();
        assert_eq!(charging.max_inference_calls_per_hour, 120);
        assert!(charging.proactive_enabled);
        assert_eq!(charging.max_concurrent_goals, 8);
        assert!(charging.inference_power_mw > 0.0);

        let emergency = TierPolicy::emergency();
        assert_eq!(emergency.max_inference_calls_per_hour, 0);
        assert!(!emergency.proactive_enabled);
        assert_eq!(emergency.max_concurrent_goals, 0);
    }

    #[test]
    fn test_inference_call_tracking() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Critical; // max 5 calls/hr

        for _ in 0..5 {
            assert!(pm.record_inference_call());
        }
        assert!(!pm.record_inference_call());
        assert_eq!(pm.inference_calls_remaining(), 0);
    }

    #[test]
    fn test_callback_registration_and_dispatch() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Normal;
        pm.last_transition = Instant::now() - Duration::from_secs(60);

        pm.on_tier_change(Box::new(move |_old, _new| {
            counter_clone.fetch_add(1, Ordering::Relaxed);
        }))
        .expect("should register callback");

        pm.update_battery(40, false);
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_callback_capacity_limit() {
        let mut pm = PowerManager::new();
        for _ in 0..MAX_CALLBACKS {
            pm.on_tier_change(Box::new(|_, _| {}))
                .expect("within capacity");
        }
        let result = pm.on_tier_change(Box::new(|_, _| {}));
        assert!(result.is_err());
    }

    #[test]
    fn test_power_throttle_factor_charging() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Charging;
        pm.is_charging.store(true, Ordering::Relaxed);
        assert!((pm.power_throttle_factor() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_power_throttle_factor_energy_based() {
        let mut pm = PowerManager::new();
        pm.is_charging.store(false, Ordering::Relaxed);

        // Full budget → throttle should be 1.0
        assert!((pm.power_throttle_factor() - 1.0).abs() < 0.01);

        // Consume 90% of budget → throttle should be low
        pm.energy.energy_consumed_mwh = pm.energy.daily_budget_mwh * 0.90;
        let throttle = pm.power_throttle_factor();
        assert!(
            throttle < 0.3,
            "throttle should be low at 10% remaining: {throttle}"
        );

        // Consume 97% → throttle should be near 0
        pm.energy.energy_consumed_mwh = pm.energy.daily_budget_mwh * 0.97;
        let throttle = pm.power_throttle_factor();
        assert!(
            throttle < 0.05,
            "throttle should be near 0 at 3% remaining: {throttle}"
        );
    }

    #[test]
    fn test_battery_tier_display() {
        assert_eq!(BatteryTier::Charging.to_string(), "Charging (Tier 0)");
        assert_eq!(BatteryTier::Emergency.to_string(), "Emergency (Tier 4)");
    }

    #[test]
    fn test_emergency_tier_on_very_low_battery() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Critical;
        pm.last_transition = Instant::now() - Duration::from_secs(60);

        let result = pm.update_battery(5, false);
        assert_eq!(result, Some(BatteryTier::Emergency));
        assert!(!pm.should_allow_proactive());
        assert!(!pm.should_allow_background());
        assert_eq!(pm.max_concurrent_goals(), 0);
    }

    // ── Energy Tracker Tests ────────────────────────────────────────────

    #[test]
    fn test_energy_tracker_defaults() {
        let tracker = EnergyTracker::new();
        // 5000 × 3.85 × 0.85 × 0.05 = 818.125 mWh
        assert!((tracker.daily_budget_mwh - 818.125).abs() < 0.1);
        assert!((tracker.energy_consumed_mwh - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_energy_tracker_custom_capacity() {
        let tracker = EnergyTracker::with_capacity(4000.0, 0.10);
        // 4000 × 3.85 × 0.85 × 0.10 = 1309.0 mWh
        assert!((tracker.daily_budget_mwh - 1309.0).abs() < 1.0);
    }

    #[test]
    fn test_energy_tracker_record_tokens() {
        let mut tracker = EnergyTracker::new();
        // 1000 tokens at 0.3 mJ/token = 300 mJ = 0.0833 mWh
        tracker.record_tokens(1000, ENERGY_PER_TOKEN_4B_MJ);
        assert!((tracker.energy_consumed_mwh - 0.08333).abs() < 0.001);
    }

    #[test]
    fn test_energy_tracker_budget_fraction() {
        let mut tracker = EnergyTracker::new();
        assert!((tracker.budget_remaining_fraction() - 1.0).abs() < f64::EPSILON);

        tracker.energy_consumed_mwh = tracker.daily_budget_mwh * 0.5;
        assert!((tracker.budget_remaining_fraction() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_energy_tracker_projected_runtime() {
        let mut tracker = EnergyTracker::new();
        tracker.current_draw_ma = 100.0;
        // 818 mWh / (100 mA × 3.85V) = 818 / 385 ≈ 2.125 hours
        let runtime = tracker.projected_runtime_hours();
        assert!((runtime - 2.125).abs() < 0.01);
    }

    #[test]
    fn test_energy_tracker_tokens_remaining() {
        let tracker = EnergyTracker::new();
        let remaining = tracker.tokens_remaining_for_tier(BatteryTier::Normal);
        // 818 mWh at 0.3 mJ/token: (818 × 3600) / 0.3 ≈ 9.8M
        assert!(remaining > 9_000_000);
        assert!(remaining < 10_500_000);
    }

    #[test]
    fn test_energy_tracker_no_draw_infinite_runtime() {
        let tracker = EnergyTracker::new();
        assert!(tracker.projected_runtime_hours().is_infinite());
    }

    #[test]
    fn test_power_manager_record_tokens() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Normal; // 4B model, 0.3 mJ/token
        pm.record_tokens(1000);
        assert!(pm.energy.energy_consumed_mwh > 0.0);
    }

    #[test]
    fn test_power_manager_with_battery() {
        let pm = PowerManager::with_battery(4000.0, 0.10);
        assert!((pm.energy.battery_capacity_mah - 4000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_tier_energy_per_token() {
        assert!((BatteryTier::Charging.energy_per_token_mj() - 0.5).abs() < f64::EPSILON);
        assert!((BatteryTier::Normal.energy_per_token_mj() - 0.3).abs() < f64::EPSILON);
        assert!((BatteryTier::Conserve.energy_per_token_mj() - 0.1).abs() < f64::EPSILON);
        assert!((BatteryTier::Emergency.energy_per_token_mj() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_energy_remaining_mwh() {
        let mut pm = PowerManager::new();
        let initial = pm.energy_remaining_mwh();
        assert!(initial > 800.0); // Should be ~818 mWh

        pm.record_tokens(10000); // Consume some energy
        assert!(pm.energy_remaining_mwh() < initial);
    }

    #[test]
    fn test_update_current_draw() {
        let mut pm = PowerManager::new();
        pm.update_current_draw(250.0);
        assert!((pm.energy.current_draw_ma - 250.0).abs() < f64::EPSILON);
        assert!(pm.projected_runtime_hours().is_finite());
    }

    // ── Model Tier Cascade Tests ────────────────────────────────────────

    #[test]
    fn test_model_cascade_full_budget_selects_8b() {
        let pm = PowerManager::new();
        // Full budget, Normal tier → energy wants 8B, tier wants 4B → 4B wins
        let tier = pm.select_model_tier_by_energy();
        assert_eq!(tier, ModelTier::Standard4B);
    }

    #[test]
    fn test_model_cascade_charging_full_budget_selects_8b() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Charging;
        // Full budget, Charging tier → energy wants 8B, tier wants 8B → 8B
        let tier = pm.select_model_tier_by_energy();
        assert_eq!(tier, ModelTier::Full8B);
    }

    #[test]
    fn test_model_cascade_low_energy_forces_small_model() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Charging;
        // Consume 90% of budget → 10% remaining → below 15% → Brainstem
        pm.energy.energy_consumed_mwh = pm.energy.daily_budget_mwh * 0.90;
        let tier = pm.select_model_tier_by_energy();
        assert_eq!(tier, ModelTier::Brainstem1_5B);
    }

    #[test]
    fn test_model_cascade_medium_energy_selects_4b() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Charging;
        // Consume 70% → 30% remaining → between 15-40% → Standard4B
        pm.energy.energy_consumed_mwh = pm.energy.daily_budget_mwh * 0.70;
        let tier = pm.select_model_tier_by_energy();
        assert_eq!(tier, ModelTier::Standard4B);
    }

    #[test]
    fn test_model_cascade_never_refuses_at_emergency() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Emergency;
        // Even at emergency tier with near-zero energy, we still return a model
        pm.energy.energy_consumed_mwh = pm.energy.daily_budget_mwh * 0.99;
        let tier = pm.select_model_tier_by_energy();
        // Must be Brainstem — never refuse to think
        assert_eq!(tier, ModelTier::Brainstem1_5B);
    }

    #[test]
    fn test_model_cascade_battery_tier_restricts() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Conserve;
        // Full energy budget → energy wants 8B, but Conserve tier policy = Brainstem
        let tier = pm.select_model_tier_by_energy();
        assert_eq!(tier, ModelTier::Brainstem1_5B);
    }

    // ── PowerBudget Bridge Tests ────────────────────────────────────────

    #[test]
    fn test_to_power_budget_fields() {
        let pm = PowerManager::new();
        let budget = pm.to_power_budget(aura_types::power::ThermalState::Cool, 30.0);

        assert_eq!(budget.battery_percent, 75);
        assert!(!budget.is_charging);
        assert!((budget.battery_capacity_mah - 5000.0).abs() < f64::EPSILON);
        assert!((budget.cell_voltage_v - LIPO_NOMINAL_VOLTAGE).abs() < f64::EPSILON);
        assert!(budget.daily_budget_mwh > 800.0);
        assert!((budget.energy_consumed_mwh - 0.0).abs() < f64::EPSILON);
        assert_eq!(budget.thermal, aura_types::power::ThermalState::Cool);
        assert!((budget.skin_temp_c - 30.0).abs() < f64::EPSILON);
        assert_eq!(budget.degradation, DegradationLevel::L1Reduced);
    }

    #[test]
    fn test_to_power_budget_token_budget_derived() {
        let pm = PowerManager::new();
        let budget = pm.to_power_budget(aura_types::power::ThermalState::Cool, 25.0);

        // Token budget should be derived from energy budget, not zero
        assert!(budget.daily_token_budget > 0);
        // No tokens used yet → tokens_used_today should be 0
        assert_eq!(budget.tokens_used_today, 0);
    }

    #[test]
    fn test_to_power_budget_after_energy_consumed() {
        let mut pm = PowerManager::new();
        pm.record_tokens(10000);
        let budget = pm.to_power_budget(aura_types::power::ThermalState::Warm, 38.0);

        assert!(budget.energy_consumed_mwh > 0.0);
        assert!(budget.tokens_used_today > 0);
        assert_eq!(budget.thermal, aura_types::power::ThermalState::Warm);
    }

    #[test]
    fn test_degradation_level_mapping() {
        let mut pm = PowerManager::new();
        pm.current_tier = BatteryTier::Charging;
        assert_eq!(pm.degradation_level(), DegradationLevel::L0Full);

        pm.current_tier = BatteryTier::Normal;
        assert_eq!(pm.degradation_level(), DegradationLevel::L1Reduced);

        pm.current_tier = BatteryTier::Conserve;
        assert_eq!(pm.degradation_level(), DegradationLevel::L2Minimal);

        pm.current_tier = BatteryTier::Critical;
        assert_eq!(pm.degradation_level(), DegradationLevel::L3DaemonOnly);

        pm.current_tier = BatteryTier::Emergency;
        assert_eq!(pm.degradation_level(), DegradationLevel::L4Heartbeat);
    }

    #[test]
    fn test_smaller_model_ordering() {
        assert_eq!(
            PowerManager::smaller_model(ModelTier::Full8B, ModelTier::Standard4B),
            ModelTier::Standard4B
        );
        assert_eq!(
            PowerManager::smaller_model(ModelTier::Standard4B, ModelTier::Full8B),
            ModelTier::Standard4B
        );
        assert_eq!(
            PowerManager::smaller_model(ModelTier::Full8B, ModelTier::Brainstem1_5B),
            ModelTier::Brainstem1_5B
        );
        assert_eq!(
            PowerManager::smaller_model(ModelTier::Brainstem1_5B, ModelTier::Brainstem1_5B),
            ModelTier::Brainstem1_5B
        );
    }
}
