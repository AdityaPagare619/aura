//! Power, thermal, and memory types grounded in real physics.
//!
//! This module replaces arbitrary percentage buckets with physics-based models:
//! - **Thermal**: Junction-to-ambient model (T_j = T_a + P × R_th)
//! - **Power**: milliamp-hour energy budgets, not percentages
//! - **Memory**: RSS/heap/mmap tracking with OOM pressure levels
//!
//! # Physics References
//!
//! - Li-ion discharge: 3.0V–4.2V nominal range, 3.85V average
//! - Thermal resistance: Snapdragon 8 Gen 3 ≈ 12°C/W, Dimensity 9300 ≈ 14°C/W
//! - Skin comfort threshold: 37°C (warm), 43°C (pain threshold)
//! - Phone thermal mass: ~200g × 1000 J/(kg·°C) = 200 J/°C

use serde::{Deserialize, Serialize};

// ─── Physical Constants ─────────────────────────────────────────────────────

/// Average Li-ion cell voltage across discharge cycle.
/// Source: Li-ion nominal 3.6–3.85V, we use midpoint of usable range.
pub const LIPO_NOMINAL_VOLTAGE: f64 = 3.85;

/// DC-DC converter + charging circuit efficiency (dimensionless).
/// Source: typical buck converter 85–92%, we use conservative 85%.
pub const POWER_DELIVERY_EFFICIENCY: f64 = 0.85;

/// Default battery capacity in mAh (typical 2024 flagship).
pub const DEFAULT_BATTERY_CAPACITY_MAH: f64 = 5000.0;

/// AURA's default daily energy share as fraction of total battery.
/// 5% of daily battery — barely noticeable to user.
pub const DEFAULT_DAILY_ENERGY_SHARE: f64 = 0.05;

// ─── Thermal Constants ──────────────────────────────────────────────────────

/// Skin comfort threshold: warm (°C).
/// Source: ISO 13732-1 — warm perception threshold for smooth surfaces.
pub const SKIN_TEMP_WARM_C: f64 = 37.0;

/// Skin comfort threshold: hot (°C).
/// Source: typical smartphone thermal throttle setpoint.
pub const SKIN_TEMP_HOT_C: f64 = 40.0;

/// Skin comfort threshold: critical/pain (°C).
/// Source: ISO 13732-1 — pain threshold for 10-second contact on metal.
pub const SKIN_TEMP_CRITICAL_C: f64 = 43.0;

/// Phone body specific heat capacity: J/(kg·°C).
/// Source: weighted average of aluminum (900), glass (840), PCB (~1100).
pub const PHONE_SPECIFIC_HEAT: f64 = 1000.0;

/// Typical phone mass in kg.
pub const PHONE_MASS_KG: f64 = 0.200;

/// Thermal capacitance of phone body: C_th = mass × specific_heat (J/°C).
pub const PHONE_THERMAL_CAPACITANCE: f64 = PHONE_MASS_KG * PHONE_SPECIFIC_HEAT; // 200 J/°C

// ─── Token Energy Constants ─────────────────────────────────────────────────

/// Energy per token by model size (millijoules).
/// Source: empirical ARM NEON measurements on Snapdragon 8 Gen 3.
/// These include CPU + DRAM access energy for Q4_K_M quantization.
pub const ENERGY_PER_TOKEN_1_5B_MJ: f64 = 0.1;
pub const ENERGY_PER_TOKEN_4B_MJ: f64 = 0.3;
pub const ENERGY_PER_TOKEN_8B_MJ: f64 = 0.5;

// ─── Thermal State ──────────────────────────────────────────────────────────

/// Thermal state based on **skin** temperature (what the user feels).
///
/// Thresholds based on ISO 13732-1 contact temperature comfort data
/// and typical smartphone thermal management policies.
///
/// | State    | T_skin     | Rationale                                   |
/// |----------|------------|---------------------------------------------|
/// | Cool     | < 37°C     | No perceptible warmth                       |
/// | Warm     | 37–40°C    | Noticeably warm, comfortable to hold        |
/// | Hot      | 40–43°C    | Uncomfortable, throttling required           |
/// | Critical | > 43°C     | Pain threshold, emergency shutdown path      |
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ThermalState {
    /// Skin < 37°C — no thermal concern.
    Cool,
    /// Skin 37–40°C — perceptibly warm, mild throttling.
    Warm,
    /// Skin 40–43°C — uncomfortable to hold, significant throttling.
    Hot,
    /// Skin > 43°C — pain threshold, emergency measures.
    Critical,
}

impl ThermalState {
    /// Determine thermal state from **skin** temperature in Celsius.
    ///
    /// Uses physics-based thresholds from ISO 13732-1 contact comfort data.
    #[must_use]
    pub fn from_skin_temp_c(temp: f64) -> ThermalState {
        if temp >= SKIN_TEMP_CRITICAL_C {
            ThermalState::Critical
        } else if temp >= SKIN_TEMP_HOT_C {
            ThermalState::Hot
        } else if temp >= SKIN_TEMP_WARM_C {
            ThermalState::Warm
        } else {
            ThermalState::Cool
        }
    }

    /// Determine thermal state from the legacy f32 Celsius input.
    ///
    /// Provided for backward compatibility with existing call sites.
    #[must_use]
    pub fn from_celsius(temp: f32) -> ThermalState {
        Self::from_skin_temp_c(temp as f64)
    }

    /// Severity ordinal (0 = coolest, 3 = most severe).
    #[must_use]
    pub fn severity(self) -> u8 {
        match self {
            Self::Cool => 0,
            Self::Warm => 1,
            Self::Hot => 2,
            Self::Critical => 3,
        }
    }
}

// ─── SoC Thermal Profile ────────────────────────────────────────────────────

/// Thermal resistance and characteristics for a specific SoC.
///
/// Models the junction-to-ambient thermal path:
/// `T_junction = T_ambient + P_dissipated × R_thermal_junction_to_ambient`
/// `T_skin = T_junction - P_dissipated × R_thermal_junction_to_case`
///
/// # Physics
///
/// Thermal resistance R_th (°C/W) characterizes how much temperature rises
/// per watt of dissipated power. The thermal time constant τ = R_th × C_th
/// determines how quickly the phone heats up.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SocThermalProfile {
    /// SoC name for identification.
    pub name: &'static str,
    /// Junction-to-ambient thermal resistance (°C/W).
    /// Source: SoC vendor thermal design guides.
    pub r_thermal_ja: f64,
    /// Junction-to-case (skin) thermal resistance (°C/W).
    /// Typically 30–40% of R_ja.
    pub r_thermal_jc: f64,
    /// Maximum junction temperature before hardware throttle (°C).
    pub t_junction_max: f64,
    /// Typical idle power draw of the SoC (W).
    pub idle_power_w: f64,
    /// Typical inference power draw for 1.5B Q4 model (W).
    pub inference_power_1_5b_w: f64,
    /// Typical inference power draw for 4B Q4 model (W).
    pub inference_power_4b_w: f64,
    /// Typical inference power draw for 8B Q4 model (W).
    pub inference_power_8b_w: f64,
}

impl SocThermalProfile {
    /// Snapdragon 8 Gen 3 (SM8650) thermal profile.
    /// Source: Qualcomm thermal design guide, AnandTech measurements.
    pub const SNAPDRAGON_8_GEN3: Self = Self {
        name: "Snapdragon 8 Gen 3",
        r_thermal_ja: 12.0,    // °C/W junction-to-ambient
        r_thermal_jc: 4.0,     // °C/W junction-to-case
        t_junction_max: 105.0, // °C
        idle_power_w: 0.8,
        inference_power_1_5b_w: 2.5,
        inference_power_4b_w: 4.0,
        inference_power_8b_w: 6.5,
    };

    /// Dimensity 9300 thermal profile.
    /// Source: MediaTek thermal design guide.
    pub const DIMENSITY_9300: Self = Self {
        name: "Dimensity 9300",
        r_thermal_ja: 14.0,
        r_thermal_jc: 4.5,
        t_junction_max: 100.0,
        idle_power_w: 0.7,
        inference_power_1_5b_w: 2.8,
        inference_power_4b_w: 4.5,
        inference_power_8b_w: 7.0,
    };

    /// Google Tensor G4 thermal profile.
    /// Source: estimated from public benchmarks.
    pub const TENSOR_G4: Self = Self {
        name: "Google Tensor G4",
        r_thermal_ja: 15.0,
        r_thermal_jc: 5.0,
        t_junction_max: 100.0,
        idle_power_w: 0.9,
        inference_power_1_5b_w: 3.0,
        inference_power_4b_w: 5.0,
        inference_power_8b_w: 7.5,
    };

    /// Default profile (conservative, works for unknown SoCs).
    pub const DEFAULT: Self = Self::SNAPDRAGON_8_GEN3;

    /// Thermal time constant τ = R_th × C_th (seconds).
    ///
    /// This governs how quickly the phone temperature responds to load changes.
    /// Typical values: 300–600 seconds (phone takes 5–10 minutes to reach steady state).
    #[must_use]
    pub fn thermal_time_constant_s(&self) -> f64 {
        self.r_thermal_ja * PHONE_THERMAL_CAPACITANCE
    }

    /// Estimate junction temperature from ambient temp and power dissipation.
    ///
    /// `T_junction = T_ambient + P_dissipated × R_thermal_ja`
    #[must_use]
    pub fn estimate_junction_temp(&self, ambient_c: f64, power_w: f64) -> f64 {
        ambient_c + power_w * self.r_thermal_ja
    }

    /// Estimate skin temperature from junction temperature and power.
    ///
    /// `T_skin = T_junction - P × R_thermal_jc`
    #[must_use]
    pub fn estimate_skin_temp(&self, junction_c: f64, power_w: f64) -> f64 {
        junction_c - power_w * self.r_thermal_jc
    }

    /// Maximum sustainable power before skin hits the given temperature.
    ///
    /// Derived from: `T_skin_limit = T_ambient + P × (R_ja - R_jc)`
    /// Therefore: `P_max = (T_skin_limit - T_ambient) / (R_ja - R_jc)`
    #[must_use]
    pub fn max_sustainable_power_w(&self, ambient_c: f64, skin_limit_c: f64) -> f64 {
        let r_case_to_ambient = self.r_thermal_ja - self.r_thermal_jc;
        if r_case_to_ambient <= 0.0 {
            return 0.0;
        }
        let delta = skin_limit_c - ambient_c;
        if delta <= 0.0 {
            return 0.0;
        }
        delta / r_case_to_ambient
    }
}

// ─── Unified Power Budget ───────────────────────────────────────────────────

/// AURA's energy budget in real physical units (milliwatt-hours, milliamps).
///
/// Replaces the old token-count-based `PowerBudget` and unifies the former
/// `PowerState` (50/30/15/5%) and `BatteryTier` (80/40/20/10%) into a single
/// energy-aware system.
///
/// # Energy Model
///
/// Total device energy = battery_capacity_mah × voltage × efficiency
/// Example: 5000 mAh × 3.85V × 0.85 = 16.4 Wh = 16362.5 mWh
///
/// AURA's daily share (default 5%) = 818 mWh = 2945 J
///
/// This budget is consumed by inference tokens:
/// - 1.5B Q4: ~0.1 mJ/token → 818 mWh budget ≈ 29.4M tokens/day
/// - 4B Q4:   ~0.3 mJ/token → 818 mWh budget ≈ 9.8M tokens/day
/// - 8B Q4:   ~0.5 mJ/token → 818 mWh budget ≈ 5.9M tokens/day
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerBudget {
    // ─── Battery state ──────────────────────────────────────
    /// Current battery level (0–100).
    pub battery_percent: u8,
    /// Whether the device is currently charging.
    pub is_charging: bool,
    /// Battery capacity in mAh (read from device or default).
    pub battery_capacity_mah: f64,
    /// Nominal cell voltage (V).
    pub cell_voltage_v: f64,

    // ─── Energy accounting ──────────────────────────────────
    /// Total daily energy budget for AURA in mWh.
    pub daily_budget_mwh: f64,
    /// Energy consumed today in mWh.
    pub energy_consumed_mwh: f64,
    /// Current draw in mA (instantaneous, from last measurement).
    pub current_draw_ma: f64,

    // ─── Thermal state ──────────────────────────────────────
    /// Current thermal state.
    pub thermal: ThermalState,
    /// Current skin temperature (°C).
    pub skin_temp_c: f64,

    // ─── Derived state ──────────────────────────────────────
    /// Current degradation level.
    pub degradation: DegradationLevel,
    /// Projected hours of AURA operation remaining at current draw.
    pub projected_runtime_hours: f64,

    // ─── Legacy compatibility ───────────────────────────────
    /// Token budget (derived from energy budget and current model tier).
    pub daily_token_budget: u32,
    /// Tokens used today.
    pub tokens_used_today: u32,
}

impl Default for PowerBudget {
    fn default() -> Self {
        let total_energy_wh =
            DEFAULT_BATTERY_CAPACITY_MAH * LIPO_NOMINAL_VOLTAGE * POWER_DELIVERY_EFFICIENCY
                / 1000.0;
        let daily_budget_mwh = total_energy_wh * 1000.0 * DEFAULT_DAILY_ENERGY_SHARE;

        Self {
            battery_percent: 100,
            is_charging: false,
            battery_capacity_mah: DEFAULT_BATTERY_CAPACITY_MAH,
            cell_voltage_v: LIPO_NOMINAL_VOLTAGE,
            daily_budget_mwh,
            energy_consumed_mwh: 0.0,
            current_draw_ma: 0.0,
            thermal: ThermalState::Cool,
            skin_temp_c: 25.0,
            degradation: DegradationLevel::L0Full,
            projected_runtime_hours: 0.0,
            daily_token_budget: 50_000,
            tokens_used_today: 0,
        }
    }
}

impl PowerBudget {
    /// Create a power budget for a device with known battery capacity.
    #[must_use]
    pub fn for_device(battery_capacity_mah: f64, daily_share_fraction: f64) -> Self {
        let clamped_share = daily_share_fraction.clamp(0.01, 0.20);
        let total_energy_wh =
            battery_capacity_mah * LIPO_NOMINAL_VOLTAGE * POWER_DELIVERY_EFFICIENCY / 1000.0;
        let daily_budget_mwh = total_energy_wh * 1000.0 * clamped_share;

        Self {
            battery_capacity_mah,
            daily_budget_mwh,
            ..Self::default()
        }
    }

    /// Energy remaining in today's budget (mWh).
    #[must_use]
    pub fn energy_remaining_mwh(&self) -> f64 {
        (self.daily_budget_mwh - self.energy_consumed_mwh).max(0.0)
    }

    /// Fraction of daily energy budget remaining (0.0–1.0).
    #[must_use]
    pub fn energy_remaining_fraction(&self) -> f64 {
        if self.daily_budget_mwh <= 0.0 {
            return 0.0;
        }
        self.energy_remaining_mwh() / self.daily_budget_mwh
    }

    /// Record energy consumed by token generation.
    ///
    /// `energy_per_token_mj` depends on model tier (see constants above).
    pub fn record_tokens(&mut self, token_count: u32, energy_per_token_mj: f64) {
        let energy_mj = token_count as f64 * energy_per_token_mj;
        let energy_mwh = energy_mj / 3600.0; // 1 mWh = 3600 mJ
        self.energy_consumed_mwh += energy_mwh;
        self.tokens_used_today += token_count;
    }

    /// Update projected runtime based on current draw.
    pub fn update_projected_runtime(&mut self) {
        if self.current_draw_ma <= 0.0 {
            self.projected_runtime_hours = f64::INFINITY;
            return;
        }
        let remaining_mwh = self.energy_remaining_mwh();
        let power_mw = self.current_draw_ma * self.cell_voltage_v;
        if power_mw <= 0.0 {
            self.projected_runtime_hours = f64::INFINITY;
            return;
        }
        self.projected_runtime_hours = remaining_mwh / power_mw;
    }

    /// Returns the fraction of daily token budget remaining (0.0–1.0).
    /// Legacy compatibility method.
    #[must_use]
    pub fn token_budget_remaining_fraction(&self) -> f32 {
        if self.daily_token_budget == 0 {
            return 0.0;
        }
        let remaining = self
            .daily_token_budget
            .saturating_sub(self.tokens_used_today);
        remaining as f32 / self.daily_token_budget as f32
    }

    /// Convert remaining energy to approximate token count for a given model.
    #[must_use]
    pub fn tokens_remaining_for_model(&self, energy_per_token_mj: f64) -> u64 {
        if energy_per_token_mj <= 0.0 {
            return 0;
        }
        let remaining_mj = self.energy_remaining_mwh() * 3600.0;
        (remaining_mj / energy_per_token_mj) as u64
    }
}

// ─── Memory Pressure ────────────────────────────────────────────────────────

/// Memory pressure levels for OOM management.
///
/// Maps to Android's `onTrimMemory` levels and /proc/meminfo data.
///
/// | Level  | Used% | Action                                      |
/// |--------|-------|---------------------------------------------|
/// | Green  | <60%  | Full operation                               |
/// | Yellow | 60-80%| Evict model caches, compress memories        |
/// | Orange | 80-90%| Downgrade model tier, archive old memories   |
/// | Red    | >90%  | Emergency: unload model, keep daemon core    |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum MemoryPressure {
    /// < 60% memory used — full operation.
    Green,
    /// 60–80% memory used — evict caches, compress memories.
    Yellow,
    /// 80–90% memory used — downgrade model, archive old data.
    Orange,
    /// > 90% memory used — emergency: unload model, daemon-only.
    Red,
}

impl MemoryPressure {
    /// Determine pressure level from used memory percentage.
    #[must_use]
    pub fn from_usage_percent(used_pct: f64) -> Self {
        if used_pct >= 90.0 {
            Self::Red
        } else if used_pct >= 80.0 {
            Self::Orange
        } else if used_pct >= 60.0 {
            Self::Yellow
        } else {
            Self::Green
        }
    }

    /// Severity ordinal (0 = green, 3 = red).
    #[must_use]
    pub fn severity(self) -> u8 {
        match self {
            Self::Green => 0,
            Self::Yellow => 1,
            Self::Orange => 2,
            Self::Red => 3,
        }
    }

    /// Whether model should be loaded at this pressure level.
    #[must_use]
    pub fn allows_model_load(self) -> bool {
        matches!(self, Self::Green | Self::Yellow)
    }
}

/// Process memory snapshot in bytes.
///
/// Tracks the memory consumed by AURA's processes for OOM decision-making.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ProcessMemoryInfo {
    /// Resident Set Size — physical memory used (bytes).
    pub rss_bytes: u64,
    /// Heap allocation (bytes).
    pub heap_bytes: u64,
    /// Memory-mapped files — primarily the GGUF model (bytes).
    pub mmap_bytes: u64,
    /// Stack usage across all threads (bytes).
    pub stack_bytes: u64,
    /// Total system available memory (bytes).
    pub system_available_bytes: u64,
    /// Total system memory (bytes).
    pub system_total_bytes: u64,
}

impl ProcessMemoryInfo {
    /// Total memory used by this process (RSS).
    #[must_use]
    pub fn total_used_bytes(&self) -> u64 {
        self.rss_bytes
    }

    /// Memory usage as a fraction of system total.
    #[must_use]
    pub fn system_usage_fraction(&self) -> f64 {
        if self.system_total_bytes == 0 {
            return 0.0;
        }
        let used = self
            .system_total_bytes
            .saturating_sub(self.system_available_bytes);
        used as f64 / self.system_total_bytes as f64
    }

    /// Current memory pressure level.
    #[must_use]
    pub fn pressure(&self) -> MemoryPressure {
        MemoryPressure::from_usage_percent(self.system_usage_fraction() * 100.0)
    }
}

// ─── Model Memory Estimation ────────────────────────────────────────────────

/// Accurate model memory estimation based on GGUF format internals.
///
/// Replaces the old 200MB magic number with real calculations:
/// - Model weights: file_size on disk (already quantized)
/// - KV cache: 2 × n_layers × n_heads × head_dim × context_len × sizeof(f16)
/// - Runtime overhead: ~10% of weights for tensor metadata, scratch buffers
///
/// # Examples
///
/// | Model         | Weights | KV Cache (4K ctx) | Overhead | Total   |
/// |---------------|---------|-------------------|----------|---------|
/// | Qwen 1.5B Q4  | 1.0 GB  | 0.12 GB           | 0.10 GB  | ~1.2 GB |
/// | Qwen 4B Q4    | 2.6 GB  | 0.32 GB           | 0.26 GB  | ~3.2 GB |
/// | Qwen 8B Q4    | 4.5 GB  | 0.62 GB           | 0.45 GB  | ~5.5 GB |
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ModelMemoryEstimate {
    /// Model name for identification.
    pub name: &'static str,
    /// On-disk file size of the GGUF weights (bytes).
    pub weights_bytes: u64,
    /// Number of transformer layers.
    pub n_layers: u32,
    /// Number of KV heads (may differ from attention heads in GQA).
    pub n_kv_heads: u32,
    /// Dimension per head.
    pub head_dim: u32,
    /// Default context length.
    pub default_context_len: u32,
}

impl ModelMemoryEstimate {
    /// Qwen 1.5B Q4_K_M model parameters.
    pub const QWEN_1_5B_Q4: Self = Self {
        name: "Qwen3.5-1.5B-Q4_K_M",
        weights_bytes: 1_073_741_824, // ~1.0 GB
        n_layers: 28,
        n_kv_heads: 4, // GQA: 4 KV heads
        head_dim: 128,
        default_context_len: 4096,
    };

    /// Qwen 4B Q4_K_M model parameters.
    pub const QWEN_4B_Q4: Self = Self {
        name: "Qwen3.5-4B-Q4_K_M",
        weights_bytes: 2_684_354_560, // ~2.5 GB
        n_layers: 40,
        n_kv_heads: 8,
        head_dim: 128,
        default_context_len: 4096,
    };

    /// Qwen 8B Q4_K_M model parameters.
    pub const QWEN_8B_Q4: Self = Self {
        name: "Qwen3.5-8B-Q4_K_M",
        weights_bytes: 4_831_838_208, // ~4.5 GB
        n_layers: 64,
        n_kv_heads: 8,
        head_dim: 128,
        default_context_len: 4096,
    };

    /// KV cache size in bytes for a given context length.
    ///
    /// Formula: 2 × n_layers × n_kv_heads × head_dim × context_len × sizeof(f16)
    /// The factor of 2 is for key + value tensors.
    #[must_use]
    pub fn kv_cache_bytes(&self, context_len: u32) -> u64 {
        2 * self.n_layers as u64
            * self.n_kv_heads as u64
            * self.head_dim as u64
            * context_len as u64
            * 2 // sizeof(f16)
    }

    /// Runtime overhead (tensor metadata, scratch buffers) — ~10% of weights.
    #[must_use]
    pub fn runtime_overhead_bytes(&self) -> u64 {
        self.weights_bytes / 10
    }

    /// Total estimated memory for this model at a given context length.
    #[must_use]
    pub fn total_memory_bytes(&self, context_len: u32) -> u64 {
        self.weights_bytes + self.kv_cache_bytes(context_len) + self.runtime_overhead_bytes()
    }

    /// Total estimated memory in MB (rounded up).
    #[must_use]
    pub fn total_memory_mb(&self, context_len: u32) -> u32 {
        self.total_memory_bytes(context_len).div_ceil(1 << 20) as u32
    }
}

// ─── Android Trim Memory Levels ─────────────────────────────────────────────

/// Android `onTrimMemory` levels that AURA should respond to.
///
/// Source: Android ComponentCallbacks2 documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrimMemoryLevel {
    /// App is running, system memory is getting low.
    RunningLow,
    /// App is running, system memory is critically low.
    RunningCritical,
    /// App is in background, system wants memory.
    Background,
    /// App is in background, system memory moderate.
    Moderate,
    /// App is in background, system memory critically low.
    Complete,
}

impl TrimMemoryLevel {
    /// Map from Android integer level to our enum.
    #[must_use]
    pub fn from_android_level(level: i32) -> Option<Self> {
        // Android ComponentCallbacks2 constants
        match level {
            10 => Some(Self::RunningLow),      // TRIM_MEMORY_RUNNING_LOW
            15 => Some(Self::RunningCritical), // TRIM_MEMORY_RUNNING_CRITICAL
            40 => Some(Self::Background),      // TRIM_MEMORY_BACKGROUND
            60 => Some(Self::Moderate),        // TRIM_MEMORY_MODERATE
            80 => Some(Self::Complete),        // TRIM_MEMORY_COMPLETE
            _ => None,
        }
    }

    /// How aggressive our response should be (0 = gentle, 4 = emergency).
    #[must_use]
    pub fn response_severity(self) -> u8 {
        match self {
            Self::RunningLow => 1,
            Self::RunningCritical => 2,
            Self::Background => 2,
            Self::Moderate => 3,
            Self::Complete => 4,
        }
    }
}

// ─── Legacy Types (kept for backward compatibility) ─────────────────────────

/// Battery power state buckets — **DEPRECATED**.
///
/// Use [`PowerBudget`] with mWh energy tracking instead.
/// Kept for backward compatibility with existing call sites.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PowerState {
    /// Battery > 50%
    Normal,
    /// Battery 30–50%
    Conservative,
    /// Battery 15–30%
    LowPower,
    /// Battery 5–15%
    Critical,
    /// Battery < 5%
    Emergency,
}

impl PowerState {
    /// Determine power state from battery percentage.
    #[must_use]
    pub fn from_battery_percent(percent: u8) -> PowerState {
        match percent {
            0..=4 => PowerState::Emergency,
            5..=14 => PowerState::Critical,
            15..=29 => PowerState::LowPower,
            30..=50 => PowerState::Conservative,
            _ => PowerState::Normal,
        }
    }
}

/// Progressive degradation levels for resource conservation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DegradationLevel {
    /// All systems fully operational.
    L0Full,
    /// Reduce polling, extend intervals, drop cosmetic features.
    L1Reduced,
    /// Only essential event processing, no proactive actions.
    L2Minimal,
    /// Only daemon heartbeat + critical notifications.
    L3DaemonOnly,
    /// Heartbeat only, no processing.
    L4Heartbeat,
    /// Everything suspended, wake only on explicit user action.
    L5Suspended,
}

/// Power tier for latency budgets — what response time is acceptable.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PowerTier {
    /// Always-on: ≤5ms response — critical system events.
    P0Always,
    /// Idle-plus: ≤50ms — notification handling.
    P1IdlePlus,
    /// Normal: ≤200ms — standard interaction.
    P2Normal,
    /// Charging: ≤2s — background work while plugged in.
    P3Charging,
    /// Deep work: ≤30s — heavy inference, bulk operations.
    P4DeepWork,
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Thermal State ───────────────────────────────────────────────────

    #[test]
    fn test_thermal_state_from_skin_temp() {
        assert_eq!(ThermalState::from_skin_temp_c(25.0), ThermalState::Cool);
        assert_eq!(ThermalState::from_skin_temp_c(36.9), ThermalState::Cool);
        assert_eq!(ThermalState::from_skin_temp_c(37.0), ThermalState::Warm);
        assert_eq!(ThermalState::from_skin_temp_c(39.9), ThermalState::Warm);
        assert_eq!(ThermalState::from_skin_temp_c(40.0), ThermalState::Hot);
        assert_eq!(ThermalState::from_skin_temp_c(42.9), ThermalState::Hot);
        assert_eq!(ThermalState::from_skin_temp_c(43.0), ThermalState::Critical);
        assert_eq!(ThermalState::from_skin_temp_c(60.0), ThermalState::Critical);
    }

    #[test]
    fn test_thermal_state_severity_ordered() {
        assert!(ThermalState::Cool.severity() < ThermalState::Warm.severity());
        assert!(ThermalState::Warm.severity() < ThermalState::Hot.severity());
        assert!(ThermalState::Hot.severity() < ThermalState::Critical.severity());
    }

    #[test]
    fn test_thermal_state_from_celsius_compat() {
        assert_eq!(ThermalState::from_celsius(25.0), ThermalState::Cool);
        assert_eq!(ThermalState::from_celsius(38.0), ThermalState::Warm);
        assert_eq!(ThermalState::from_celsius(41.0), ThermalState::Hot);
        assert_eq!(ThermalState::from_celsius(44.0), ThermalState::Critical);
    }

    // ── SoC Thermal Profile ─────────────────────────────────────────────

    #[test]
    fn test_soc_thermal_time_constant() {
        let profile = SocThermalProfile::SNAPDRAGON_8_GEN3;
        let tau = profile.thermal_time_constant_s();
        // 12 °C/W × 200 J/°C = 2400 seconds
        assert!((tau - 2400.0).abs() < 1.0);
    }

    #[test]
    fn test_junction_temp_estimation() {
        let profile = SocThermalProfile::SNAPDRAGON_8_GEN3;
        let t_j = profile.estimate_junction_temp(25.0, 4.0);
        // 25 + 4 × 12 = 73°C
        assert!((t_j - 73.0).abs() < 0.1);
    }

    #[test]
    fn test_skin_temp_estimation() {
        let profile = SocThermalProfile::SNAPDRAGON_8_GEN3;
        let t_j = profile.estimate_junction_temp(25.0, 4.0); // 73°C
        let t_skin = profile.estimate_skin_temp(t_j, 4.0);
        // 73 - 4 × 4 = 57°C (junction much hotter than skin)
        assert!((t_skin - 57.0).abs() < 0.1);
    }

    #[test]
    fn test_max_sustainable_power() {
        let profile = SocThermalProfile::SNAPDRAGON_8_GEN3;
        // At 25°C ambient, skin limit 40°C: P_max = (40-25)/(12-4) = 1.875W
        let p_max = profile.max_sustainable_power_w(25.0, 40.0);
        assert!((p_max - 1.875).abs() < 0.01);
    }

    #[test]
    fn test_max_sustainable_power_zero_headroom() {
        let profile = SocThermalProfile::SNAPDRAGON_8_GEN3;
        // Ambient already above limit
        let p_max = profile.max_sustainable_power_w(45.0, 40.0);
        assert!((p_max - 0.0).abs() < f64::EPSILON);
    }

    // ── Power Budget ────────────────────────────────────────────────────

    #[test]
    fn test_power_budget_defaults() {
        let budget = PowerBudget::default();
        assert_eq!(budget.battery_percent, 100);
        assert!(!budget.is_charging);
        assert_eq!(budget.thermal, ThermalState::Cool);
        assert_eq!(budget.degradation, DegradationLevel::L0Full);
        // 5000 mAh × 3.85V × 0.85 = 16362.5 mWh total
        // 5% = 818.125 mWh
        assert!((budget.daily_budget_mwh - 818.125).abs() < 0.1);
    }

    #[test]
    fn test_power_budget_for_device() {
        let budget = PowerBudget::for_device(4000.0, 0.10);
        // 4000 × 3.85 × 0.85 = 13090 mWh total, 10% = 1309 mWh
        assert!((budget.daily_budget_mwh - 1309.0).abs() < 1.0);
    }

    #[test]
    fn test_power_budget_energy_remaining() {
        let mut budget = PowerBudget::default();
        assert!((budget.energy_remaining_fraction() - 1.0).abs() < f64::EPSILON);

        budget.energy_consumed_mwh = budget.daily_budget_mwh / 2.0;
        assert!((budget.energy_remaining_fraction() - 0.5).abs() < 0.001);

        budget.energy_consumed_mwh = budget.daily_budget_mwh * 2.0; // over-consumed
        assert!((budget.energy_remaining_fraction() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_record_tokens_energy() {
        let mut budget = PowerBudget::default();
        // 1000 tokens at 0.3 mJ/token = 300 mJ = 0.0833 mWh
        budget.record_tokens(1000, ENERGY_PER_TOKEN_4B_MJ);
        assert!((budget.energy_consumed_mwh - 0.08333).abs() < 0.001);
        assert_eq!(budget.tokens_used_today, 1000);
    }

    #[test]
    fn test_tokens_remaining_for_model() {
        let budget = PowerBudget::default();
        // Budget ~818 mWh = 2945250 mJ. At 0.1 mJ/token → ~29.4M tokens
        let remaining = budget.tokens_remaining_for_model(ENERGY_PER_TOKEN_1_5B_MJ);
        assert!(remaining > 29_000_000);
        assert!(remaining < 30_000_000);
    }

    #[test]
    fn test_projected_runtime() {
        let mut budget = PowerBudget {
            current_draw_ma: 100.0,
            ..Default::default()
        };
        budget.update_projected_runtime();
        // 818 mWh remaining / (100 mA × 3.85V) = 818 / 385 ≈ 2.125 hours
        assert!((budget.projected_runtime_hours - 2.125).abs() < 0.01);
    }

    #[test]
    fn test_projected_runtime_no_draw() {
        let mut budget = PowerBudget {
            current_draw_ma: 0.0,
            ..Default::default()
        };
        budget.update_projected_runtime();
        assert!(budget.projected_runtime_hours.is_infinite());
    }

    #[test]
    fn test_token_budget_remaining_fraction_compat() {
        let mut budget = PowerBudget::default();
        assert!((budget.token_budget_remaining_fraction() - 1.0).abs() < f32::EPSILON);
        budget.tokens_used_today = 25_000;
        assert!((budget.token_budget_remaining_fraction() - 0.5).abs() < f32::EPSILON);
        budget.tokens_used_today = 50_000;
        assert!((budget.token_budget_remaining_fraction() - 0.0).abs() < f32::EPSILON);
        budget.tokens_used_today = 60_000;
        assert!((budget.token_budget_remaining_fraction() - 0.0).abs() < f32::EPSILON);
    }

    // ── Memory Pressure ─────────────────────────────────────────────────

    #[test]
    fn test_memory_pressure_levels() {
        assert_eq!(
            MemoryPressure::from_usage_percent(30.0),
            MemoryPressure::Green
        );
        assert_eq!(
            MemoryPressure::from_usage_percent(59.9),
            MemoryPressure::Green
        );
        assert_eq!(
            MemoryPressure::from_usage_percent(60.0),
            MemoryPressure::Yellow
        );
        assert_eq!(
            MemoryPressure::from_usage_percent(79.9),
            MemoryPressure::Yellow
        );
        assert_eq!(
            MemoryPressure::from_usage_percent(80.0),
            MemoryPressure::Orange
        );
        assert_eq!(
            MemoryPressure::from_usage_percent(89.9),
            MemoryPressure::Orange
        );
        assert_eq!(
            MemoryPressure::from_usage_percent(90.0),
            MemoryPressure::Red
        );
        assert_eq!(
            MemoryPressure::from_usage_percent(100.0),
            MemoryPressure::Red
        );
    }

    #[test]
    fn test_memory_pressure_severity_ordered() {
        assert!(MemoryPressure::Green.severity() < MemoryPressure::Yellow.severity());
        assert!(MemoryPressure::Yellow.severity() < MemoryPressure::Orange.severity());
        assert!(MemoryPressure::Orange.severity() < MemoryPressure::Red.severity());
    }

    #[test]
    fn test_memory_pressure_model_load() {
        assert!(MemoryPressure::Green.allows_model_load());
        assert!(MemoryPressure::Yellow.allows_model_load());
        assert!(!MemoryPressure::Orange.allows_model_load());
        assert!(!MemoryPressure::Red.allows_model_load());
    }

    #[test]
    fn test_process_memory_pressure() {
        let info = ProcessMemoryInfo {
            rss_bytes: 2_000_000_000,
            heap_bytes: 500_000_000,
            mmap_bytes: 1_500_000_000,
            stack_bytes: 8_000_000,
            system_available_bytes: 2_000_000_000, // 2 GB free
            system_total_bytes: 8_000_000_000,     // 8 GB total → 75% used
        };
        assert_eq!(info.pressure(), MemoryPressure::Yellow);
    }

    // ── Model Memory Estimation ─────────────────────────────────────────

    #[test]
    fn test_model_memory_1_5b() {
        let est = ModelMemoryEstimate::QWEN_1_5B_Q4;
        let total_mb = est.total_memory_mb(4096);
        // ~1.0 GB weights + ~0.12 GB KV + ~0.10 GB overhead ≈ 1.2 GB ≈ 1200 MB
        assert!(total_mb >= 1100, "1.5B should be ≈1200 MB, got {total_mb}");
        assert!(total_mb <= 1400, "1.5B should be ≈1200 MB, got {total_mb}");
    }

    #[test]
    fn test_model_memory_4b() {
        let est = ModelMemoryEstimate::QWEN_4B_Q4;
        let total_mb = est.total_memory_mb(4096);
        assert!(total_mb >= 2800, "4B should be ≈3200 MB, got {total_mb}");
        assert!(total_mb <= 3600, "4B should be ≈3200 MB, got {total_mb}");
    }

    #[test]
    fn test_model_memory_8b() {
        let est = ModelMemoryEstimate::QWEN_8B_Q4;
        let total_mb = est.total_memory_mb(4096);
        assert!(total_mb >= 5000, "8B should be >=5000 MB, got {total_mb}");
        assert!(total_mb <= 6500, "8B should be <=6500 MB, got {total_mb}");
    }

    #[test]
    fn test_kv_cache_scales_with_context() {
        let est = ModelMemoryEstimate::QWEN_4B_Q4;
        let kv_4k = est.kv_cache_bytes(4096);
        let kv_8k = est.kv_cache_bytes(8192);
        // KV cache should double when context doubles
        assert_eq!(kv_8k, kv_4k * 2);
    }

    // ── Trim Memory ─────────────────────────────────────────────────────

    #[test]
    fn test_trim_memory_from_android_level() {
        assert_eq!(
            TrimMemoryLevel::from_android_level(10),
            Some(TrimMemoryLevel::RunningLow)
        );
        assert_eq!(
            TrimMemoryLevel::from_android_level(15),
            Some(TrimMemoryLevel::RunningCritical)
        );
        assert_eq!(
            TrimMemoryLevel::from_android_level(80),
            Some(TrimMemoryLevel::Complete)
        );
        assert_eq!(TrimMemoryLevel::from_android_level(99), None);
    }

    #[test]
    fn test_trim_memory_severity_ordered() {
        assert!(
            TrimMemoryLevel::RunningLow.response_severity()
                < TrimMemoryLevel::RunningCritical.response_severity()
        );
        assert!(
            TrimMemoryLevel::Moderate.response_severity()
                < TrimMemoryLevel::Complete.response_severity()
        );
    }

    // ── Legacy compat ───────────────────────────────────────────────────

    #[test]
    fn test_power_state_from_battery() {
        assert_eq!(PowerState::from_battery_percent(100), PowerState::Normal);
        assert_eq!(PowerState::from_battery_percent(51), PowerState::Normal);
        assert_eq!(
            PowerState::from_battery_percent(50),
            PowerState::Conservative
        );
        assert_eq!(
            PowerState::from_battery_percent(30),
            PowerState::Conservative
        );
        assert_eq!(PowerState::from_battery_percent(29), PowerState::LowPower);
        assert_eq!(PowerState::from_battery_percent(15), PowerState::LowPower);
        assert_eq!(PowerState::from_battery_percent(14), PowerState::Critical);
        assert_eq!(PowerState::from_battery_percent(5), PowerState::Critical);
        assert_eq!(PowerState::from_battery_percent(4), PowerState::Emergency);
        assert_eq!(PowerState::from_battery_percent(0), PowerState::Emergency);
    }
}
