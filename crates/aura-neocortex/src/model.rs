//! Model management for AURA Neocortex.
//!
//! Handles intelligent model tier selection, cascading, loading/unloading
//! via `aura-llama-sys`, and post-load headroom verification.
//!
//! # Intelligent Cascading (Layer 3)
//!
//! Instead of static RAM-only tier selection, this module evaluates:
//! - Available RAM (hard constraint — cannot load what won't fit)
//! - Power state (battery-aware — prefer smaller models on low battery)
//! - Task complexity (reflexive tasks don't need 8B, deep reasoning does)
//! - Previous confidence (if prior attempt scored low, escalate to bigger model)
//!
//! The cascade flow: start at the cheapest tier that *might* handle the task,
//! then escalate upward if confidence is too low or the task proves too complex.
//!
//! # Model Tiers
//!
//! Uses the existing `ModelTier` enum from `aura_types::ipc` (3 variants):
//! - `Brainstem1_5B` — reflexive/simple tasks, reflection verdicts
//! - `Standard4B`    — standard planning and composition
//! - `Full8B`        — complex reasoning, multi-step strategies
//!
//! NOTE: The spec envisions 4 tiers (0.5B/1.5B/7B/14B). The current type
//! system has 3 tiers. When the type is extended, add a new tier between
//! Brainstem and Standard (or above Full) and update the cascade tables.

use std::path::{Path, PathBuf};
use std::time::Instant;

use aura_llama_sys::{LlamaContext, LlamaContextParams, LlamaModel, LlamaModelParams};
use aura_types::ipc::{InferenceMode, ModelParams, ModelTier};
#[allow(unused_imports)] // `error` used in #[cfg(target_os = "android")] paths
use tracing::{debug, error, info, warn};

// ─── Task complexity ────────────────────────────────────────────────────────

/// How complex the current inference task is, estimated from mode + context signals.
///
/// This drives initial tier selection: reflexive tasks start at Brainstem,
/// complex tasks start at Standard, deep tasks start at Full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskComplexity {
    /// Simple reflexive task: reflection verdicts, confidence checks, yes/no.
    Reflexive,
    /// Standard task: single-step planning, short conversation replies.
    Standard,
    /// Complex task: multi-step planning, composition with tools.
    Complex,
    /// Deep reasoning: strategic replanning, CoT-forced analysis, multi-tool.
    Deep,
}

impl TaskComplexity {
    /// Score task complexity from inference context signals.
    ///
    /// This is the core heuristic that maps observable signals to a complexity
    /// level. The signals are cheap to compute (no model inference needed).
    ///
    /// # Arguments
    /// - `mode` — the inference mode (Planner/Strategist are higher than Conversational)
    /// - `context_token_count` — estimated token count of the assembled context
    /// - `has_tools` — whether tool descriptions are included in the prompt
    /// - `force_cot` — whether chain-of-thought is being forced (Layer 1)
    /// - `is_retry` — whether this is a retry attempt (Layer 3)
    pub fn score(
        mode: InferenceMode,
        context_token_count: u32,
        has_tools: bool,
        force_cot: bool,
        is_retry: bool,
    ) -> Self {
        let mut score: u32 = 0;

        // Mode contribution (0-3)
        score += match mode {
            InferenceMode::Conversational => 0,
            InferenceMode::Composer => 1,
            InferenceMode::Planner => 2,
            InferenceMode::Strategist => 3,
        };

        // Context size contribution (0-2)
        score += if context_token_count > 2000 {
            2
        } else if context_token_count > 800 {
            1
        } else {
            0
        };

        // Tool usage bumps complexity
        if has_tools {
            score += 1;
        }

        // CoT forcing means the task needs deeper reasoning
        if force_cot {
            score += 1;
        }

        // Retry means the smaller model already failed
        if is_retry {
            score += 2;
        }

        match score {
            0..=1 => TaskComplexity::Reflexive,
            2..=3 => TaskComplexity::Standard,
            4..=5 => TaskComplexity::Complex,
            _ => TaskComplexity::Deep,
        }
    }

    /// Minimum tier recommended for this complexity level.
    ///
    /// This is a soft recommendation — the cascade system may start lower
    /// if RAM or power constraints require it.
    pub fn recommended_min_tier(self) -> ModelTier {
        match self {
            TaskComplexity::Reflexive => ModelTier::Brainstem1_5B,
            TaskComplexity::Standard => ModelTier::Brainstem1_5B,
            TaskComplexity::Complex => ModelTier::Standard4B,
            TaskComplexity::Deep => ModelTier::Full8B,
        }
    }
}

// ─── Power state ────────────────────────────────────────────────────────────

/// Device power state, affecting model tier selection.
///
/// On Android, this would come from `BatteryManager` via the daemon.
/// On host builds, defaults to `Normal` for development.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    /// Battery < 10% — use smallest possible model.
    Critical,
    /// Battery 10-30% — prefer smaller models, avoid Full.
    Low,
    /// Battery > 30% or unknown — normal tier selection.
    Normal,
    /// Plugged in — no power constraints, prefer quality.
    Charging,
}

impl PowerState {
    /// Maximum tier allowed under this power state.
    ///
    /// Hard ceiling: the cascade system will not escalate beyond this.
    pub fn max_allowed_tier(self) -> ModelTier {
        match self {
            PowerState::Critical => ModelTier::Brainstem1_5B,
            PowerState::Low => ModelTier::Standard4B,
            PowerState::Normal => ModelTier::Full8B,
            PowerState::Charging => ModelTier::Full8B,
        }
    }

    /// Derive power state from battery percentage and charging flag.
    pub fn from_battery(battery_percent: u8, is_charging: bool) -> Self {
        if is_charging {
            PowerState::Charging
        } else if battery_percent < 10 {
            PowerState::Critical
        } else if battery_percent <= 30 {
            PowerState::Low
        } else {
            PowerState::Normal
        }
    }
}

// ─── Tier capability metadata ───────────────────────────────────────────────

/// Per-tier capability metadata for cascade decision-making.
#[derive(Debug, Clone, Copy)]
pub struct TierCapability {
    /// Maximum complexity this tier handles reliably.
    pub max_complexity: TaskComplexity,
    /// Quality score (0.0-1.0) — higher means better output quality.
    /// Used as a weight in Best-of-N sampling.
    pub quality_score: f32,
    /// Estimated tokens per second on a typical Android device (Snapdragon 8 Gen 3).
    pub est_tokens_per_sec: f32,
    /// Confidence threshold: if the model's output confidence is below this,
    /// the cascade system should consider escalating to a bigger model.
    pub confidence_threshold: f32,
}

/// Get capability metadata for a tier.
pub fn tier_capability(tier: ModelTier) -> TierCapability {
    match tier {
        ModelTier::Brainstem1_5B => TierCapability {
            max_complexity: TaskComplexity::Standard,
            quality_score: 0.55,
            est_tokens_per_sec: 45.0,
            confidence_threshold: 0.65,
        },
        ModelTier::Standard4B => TierCapability {
            max_complexity: TaskComplexity::Complex,
            quality_score: 0.75,
            est_tokens_per_sec: 25.0,
            confidence_threshold: 0.55,
        },
        ModelTier::Full8B => TierCapability {
            max_complexity: TaskComplexity::Deep,
            quality_score: 0.90,
            est_tokens_per_sec: 12.0,
            confidence_threshold: 0.40,
        },
    }
}

// ─── Cascade decision ───────────────────────────────────────────────────────

/// Result of evaluating whether to cascade to a different tier.
#[derive(Debug, Clone)]
pub struct CascadeDecision {
    /// The recommended tier to use.
    pub recommended_tier: ModelTier,
    /// Human-readable reason for the decision (for logging/debugging).
    pub reason: String,
    /// Whether this decision represents an escalation from the current tier.
    pub is_escalation: bool,
}

// ─── Model tier helpers ─────────────────────────────────────────────────────

/// Filename of the GGUF model for a given tier.
pub fn tier_filename(tier: ModelTier) -> &'static str {
    match tier {
        ModelTier::Brainstem1_5B => "qwen3.5-1.5b-q4_k_m.gguf",
        ModelTier::Standard4B => "qwen3.5-4b-q4_k_m.gguf",
        ModelTier::Full8B => "qwen3.5-8b-q4_k_m.gguf",
    }
}

/// Approximate memory footprint in MB for a given tier.
pub fn tier_approx_memory_mb(tier: ModelTier) -> u32 {
    match tier {
        ModelTier::Brainstem1_5B => 900,
        ModelTier::Standard4B => 2400,
        ModelTier::Full8B => 4800,
    }
}

/// Downgrade to a smaller tier, or `None` if already at Brainstem.
pub fn tier_downgrade(tier: ModelTier) -> Option<ModelTier> {
    match tier {
        ModelTier::Full8B => Some(ModelTier::Standard4B),
        ModelTier::Standard4B => Some(ModelTier::Brainstem1_5B),
        ModelTier::Brainstem1_5B => None,
    }
}

/// Upgrade to a larger tier, or `None` if already at Full.
pub fn tier_upgrade(tier: ModelTier) -> Option<ModelTier> {
    match tier {
        ModelTier::Brainstem1_5B => Some(ModelTier::Standard4B),
        ModelTier::Standard4B => Some(ModelTier::Full8B),
        ModelTier::Full8B => None,
    }
}

/// Human-readable display name for a tier.
pub fn tier_display(tier: ModelTier) -> &'static str {
    match tier {
        ModelTier::Brainstem1_5B => "Brainstem (1.5B)",
        ModelTier::Standard4B => "Standard (4B)",
        ModelTier::Full8B => "Full (8B)",
    }
}

/// Numeric ordering for tier comparisons (higher = bigger model).
fn tier_ordinal(tier: ModelTier) -> u8 {
    match tier {
        ModelTier::Brainstem1_5B => 0,
        ModelTier::Standard4B => 1,
        ModelTier::Full8B => 2,
    }
}

/// All tiers in ascending order of size.
pub const ALL_TIERS: [ModelTier; 3] = [
    ModelTier::Brainstem1_5B,
    ModelTier::Standard4B,
    ModelTier::Full8B,
];

// ─── Intelligent tier selection ─────────────────────────────────────────────

/// Select the best model tier considering all available signals.
///
/// This replaces the simple RAM-only `select_tier()` with a multi-factor
/// decision that balances quality, speed, power, and capability.
///
/// # Algorithm
/// 1. Start with the complexity-recommended minimum tier
/// 2. If a previous attempt had low confidence, escalate one tier
/// 3. Clamp to the power state's max allowed tier
/// 4. Clamp to what fits in available RAM
/// 5. Return the result
pub fn select_tier_intelligent(
    available_mb: u32,
    power: PowerState,
    complexity: TaskComplexity,
    prev_confidence: Option<f32>,
) -> CascadeDecision {
    // Step 1: Start with complexity recommendation
    let mut tier = complexity.recommended_min_tier();
    let mut reason = format!("complexity={:?} recommends {:?}", complexity, tier);

    // Step 2: Escalate if previous attempt had low confidence
    if let Some(conf) = prev_confidence {
        let cap = tier_capability(tier);
        if conf < cap.confidence_threshold {
            if let Some(higher) = tier_upgrade(tier) {
                debug!(
                    prev_confidence = conf,
                    threshold = cap.confidence_threshold,
                    "escalating due to low confidence"
                );
                tier = higher;
                reason = format!(
                    "escalated from low confidence ({:.2} < {:.2})",
                    conf, cap.confidence_threshold
                );
            }
        }
    }

    // Step 3: Clamp to power ceiling
    let power_max = power.max_allowed_tier();
    if tier_ordinal(tier) > tier_ordinal(power_max) {
        warn!(
            requested = tier_display(tier),
            power_max = tier_display(power_max),
            "clamping tier due to power state"
        );
        tier = power_max;
        reason = format!("clamped by power state {:?}", power);
    }

    // Step 4: Clamp to RAM ceiling
    let ram_max = select_tier_by_ram(available_mb);
    if tier_ordinal(tier) > tier_ordinal(ram_max) {
        warn!(
            requested = tier_display(tier),
            ram_max = tier_display(ram_max),
            available_mb,
            "clamping tier due to RAM"
        );
        tier = ram_max;
        reason = format!("clamped by RAM ({} MB available)", available_mb);
    }

    let is_escalation = prev_confidence.is_some();

    info!(
        tier = tier_display(tier),
        reason = %reason,
        "intelligent tier selection complete"
    );

    CascadeDecision {
        recommended_tier: tier,
        reason,
        is_escalation,
    }
}

/// Evaluate whether to cascade up from the current tier based on output confidence.
///
/// Called after inference completes. If the output confidence is below the
/// tier's threshold and a higher tier is available (within RAM/power limits),
/// returns a decision to escalate.
pub fn should_cascade_up(
    current_tier: ModelTier,
    confidence: f32,
    available_mb: u32,
    power: PowerState,
) -> Option<CascadeDecision> {
    let cap = tier_capability(current_tier);

    // If confidence is acceptable, no cascade needed
    if confidence >= cap.confidence_threshold {
        return None;
    }

    // Try to upgrade
    let next = tier_upgrade(current_tier)?;

    // Check RAM constraint
    let needed_mb = tier_approx_memory_mb(next);
    if available_mb < needed_mb + 200 {
        info!(
            current = tier_display(current_tier),
            next = tier_display(next),
            available_mb,
            needed_mb,
            "cascade blocked by RAM"
        );
        return None;
    }

    // Check power constraint
    if tier_ordinal(next) > tier_ordinal(power.max_allowed_tier()) {
        info!(
            current = tier_display(current_tier),
            next = tier_display(next),
            power = ?power,
            "cascade blocked by power state"
        );
        return None;
    }

    Some(CascadeDecision {
        recommended_tier: next,
        reason: format!(
            "confidence {:.2} < threshold {:.2}, escalating {} -> {}",
            confidence,
            cap.confidence_threshold,
            tier_display(current_tier),
            tier_display(next)
        ),
        is_escalation: true,
    })
}

/// Simple RAM-only tier selection (used as a ceiling in intelligent selection).
pub fn select_tier_by_ram(available_mb: u32) -> ModelTier {
    if available_mb >= 5200 {
        ModelTier::Full8B
    } else if available_mb >= 2800 {
        ModelTier::Standard4B
    } else {
        ModelTier::Brainstem1_5B
    }
}

// ─── Loaded model handle ────────────────────────────────────────────────────

/// An active, loaded model with its context.
pub struct LoadedModel {
    pub tier: ModelTier,
    pub model_ptr: *mut LlamaModel,
    pub ctx_ptr: *mut LlamaContext,
    /// Model name (e.g. "qwen3.5-4b-q4_k_m.gguf").
    pub model_name: String,
    /// Memory used by this model in MB (approximate).
    pub memory_used_mb: u32,
    /// When the model was loaded (for idle timeout tracking).
    pub loaded_at: Instant,
    /// Last time inference was run.
    pub last_used: Instant,
    /// Number of inferences completed on this loaded instance.
    pub inference_count: u64,
    /// Cumulative confidence score for cascade tracking.
    pub cumulative_confidence: f64,
}

// Safety: the raw pointers are only accessed from a single thread (the
// neocortex binary is single-threaded with respect to model access).
unsafe impl Send for LoadedModel {}

impl LoadedModel {
    /// Whether this model's pointers are valid (non-null).
    /// On host stubs, they will be null — inference must use stub/canned paths.
    pub fn is_stub(&self) -> bool {
        self.model_ptr.is_null() || self.ctx_ptr.is_null()
    }

    /// Record a completed inference with its confidence score.
    pub fn record_inference(&mut self, confidence: f32) {
        self.last_used = Instant::now();
        self.inference_count += 1;
        self.cumulative_confidence += confidence as f64;
    }

    /// Average confidence across all inferences on this loaded model.
    ///
    /// Returns `None` if no inferences have been recorded.
    pub fn average_confidence(&self) -> Option<f32> {
        if self.inference_count == 0 {
            None
        } else {
            Some((self.cumulative_confidence / self.inference_count as f64) as f32)
        }
    }
}

impl Drop for LoadedModel {
    fn drop(&mut self) {
        if !self.ctx_ptr.is_null() {
            #[cfg(not(target_os = "android"))]
            aura_llama_sys::stubs::llama_free_context(self.ctx_ptr);
        }
        if !self.model_ptr.is_null() {
            #[cfg(not(target_os = "android"))]
            aura_llama_sys::stubs::llama_free_model(self.model_ptr);
        }
    }
}

// ─── Model manager ──────────────────────────────────────────────────────────

/// Manages model lifecycle: selection, loading, unloading, cascade tracking.
pub struct ModelManager {
    model_dir: PathBuf,
    loaded: Option<LoadedModel>,
    /// Number of cascade escalations in the current session.
    cascade_count: u32,
    /// Maximum cascades before giving up (prevents infinite loops).
    max_cascades: u32,
}

impl ModelManager {
    pub fn new(model_dir: PathBuf) -> Self {
        Self {
            model_dir,
            loaded: None,
            cascade_count: 0,
            max_cascades: 3,
        }
    }

    /// Is a model currently loaded?
    pub fn is_loaded(&self) -> bool {
        self.loaded.is_some()
    }

    /// Get a reference to the loaded model, if any.
    pub fn loaded(&self) -> Option<&LoadedModel> {
        self.loaded.as_ref()
    }

    /// Get a mutable reference to the loaded model (to update last_used).
    pub fn loaded_mut(&mut self) -> Option<&mut LoadedModel> {
        self.loaded.as_mut()
    }

    /// Get the currently loaded tier.
    pub fn current_tier(&self) -> Option<ModelTier> {
        self.loaded.as_ref().map(|m| m.tier)
    }

    /// Number of cascade escalations performed this session.
    pub fn cascade_count(&self) -> u32 {
        self.cascade_count
    }

    /// Whether we've exhausted cascade attempts.
    pub fn cascades_exhausted(&self) -> bool {
        self.cascade_count >= self.max_cascades
    }

    /// Reset cascade counter (e.g., after a new user request).
    pub fn reset_cascades(&mut self) {
        self.cascade_count = 0;
    }

    /// Perform a cascade escalation: unload current model, load the next tier up.
    ///
    /// Returns the new `(model_name, memory_used_mb)` on success.
    pub fn cascade_to(
        &mut self,
        new_tier: ModelTier,
        params: &ModelParams,
    ) -> Result<(String, u32), String> {
        if self.cascades_exhausted() {
            return Err(format!(
                "cascade limit reached ({}/{})",
                self.cascade_count, self.max_cascades
            ));
        }

        info!(
            from = self.current_tier().map(tier_display).unwrap_or("none"),
            to = tier_display(new_tier),
            cascade = self.cascade_count + 1,
            "performing cascade escalation"
        );

        // Build params with the new tier
        let mut new_params = params.clone();
        new_params.model_tier = new_tier;

        let model_path = self.model_dir.to_string_lossy().to_string();
        let result = self.load(&model_path, &new_params);

        if result.is_ok() {
            self.cascade_count += 1;
        }

        result
    }

    /// Load a model from the given path with the given parameters.
    ///
    /// The `params.model_tier` indicates the desired tier. We attempt to load
    /// it, then verify >= 200 MB headroom remains post-load. If headroom is
    /// insufficient, we downgrade to a smaller tier automatically.
    ///
    /// Returns `(model_name, memory_used_mb)` on success.
    pub fn load(
        &mut self,
        model_path: &str,
        params: &ModelParams,
    ) -> Result<(String, u32), String> {
        // If already loaded, unload first.
        if self.is_loaded() {
            info!("unloading existing model before loading new one");
            self.unload();
        }

        let available_mb = available_ram_mb();
        info!(available_mb, "detected available RAM");

        // Start with the requested tier, but may downgrade.
        let mut tier = params.model_tier;
        info!(tier = tier_display(tier), "requested model tier");

        // Try to load, downgrading if post-load headroom check fails.
        loop {
            let file_path = resolve_model_path(&self.model_dir, model_path, tier);
            info!(path = %file_path.display(), "attempting to load model");

            let ctx_params = LlamaContextParams {
                n_ctx: params.n_ctx,
                n_threads: params.n_threads,
                ..LlamaContextParams::default()
            };

            let start = Instant::now();
            let (model_ptr, ctx_ptr) = load_model_ffi(&file_path, &ctx_params);
            let load_time = start.elapsed();

            // Post-load headroom check: verify >= 200 MB remains.
            let post_load_ram = available_ram_mb();
            let headroom_ok = post_load_ram >= 200;

            if !headroom_ok {
                warn!(
                    post_load_ram,
                    tier = tier_display(tier),
                    "insufficient headroom after load, attempting downgrade"
                );
                free_model_ffi(model_ptr, ctx_ptr);

                if let Some(lower) = tier_downgrade(tier) {
                    tier = lower;
                    continue;
                } else {
                    return Err("insufficient RAM even for smallest model".into());
                }
            }

            let memory_used = tier_approx_memory_mb(tier);
            let model_name = tier_filename(tier).to_string();
            let now = Instant::now();

            let loaded = LoadedModel {
                tier,
                model_ptr,
                ctx_ptr,
                model_name: model_name.clone(),
                memory_used_mb: memory_used,
                loaded_at: now,
                last_used: now,
                inference_count: 0,
                cumulative_confidence: 0.0,
            };

            info!(
                model_name = %model_name,
                load_ms = load_time.as_millis() as u64,
                memory_used_mb = memory_used,
                stub = loaded.is_stub(),
                "model loaded successfully"
            );

            self.loaded = Some(loaded);
            return Ok((model_name, memory_used));
        }
    }

    /// Unload the current model, freeing resources.
    pub fn unload(&mut self) {
        if let Some(model) = self.loaded.take() {
            info!(
                tier = tier_display(model.tier),
                inference_count = model.inference_count,
                avg_confidence = model.average_confidence().unwrap_or(0.0),
                "unloading model"
            );
            drop(model);
        }
    }

    /// Check if the model has been idle longer than the timeout.
    ///
    /// Timeout values from spec:
    /// - Normal: 60 seconds
    /// - High-activity: 180 seconds
    /// - Charging: never unload
    pub fn should_idle_unload(&self, is_high_activity: bool, is_charging: bool) -> bool {
        if is_charging {
            return false;
        }

        let timeout_secs = if is_high_activity { 180 } else { 60 };

        match &self.loaded {
            Some(model) => model.last_used.elapsed().as_secs() > timeout_secs,
            None => false,
        }
    }
}

// ─── Path resolution ────────────────────────────────────────────────────────

/// Resolve the full model file path.
///
/// If `model_path` is an existing directory, we append the tier filename.
/// If `model_path` looks like a file, we use it directly (but may substitute
/// the tier filename for the last path component on downgrades).
/// Otherwise, we fall back to `model_dir/tier_filename`.
fn resolve_model_path(model_dir: &Path, model_path: &str, tier: ModelTier) -> PathBuf {
    let p = Path::new(model_path);

    if p.is_dir() {
        return p.join(tier_filename(tier));
    }

    if p.is_absolute() {
        if let Some(parent) = p.parent() {
            return parent.join(tier_filename(tier));
        }
    }

    model_dir.join(tier_filename(tier))
}

// ─── RAM detection ──────────────────────────────────────────────────────────

/// Get available RAM in MB.
///
/// On Android, reads `/proc/meminfo`.
/// On host (Windows/Linux), returns a reasonable estimate for development.
pub fn available_ram_mb() -> u32 {
    #[cfg(target_os = "android")]
    {
        read_proc_meminfo_available().unwrap_or(2048)
    }

    #[cfg(not(target_os = "android"))]
    {
        // Host development: assume 8 GB available so Full tier is selected.
        8192
    }
}

/// Parse MemAvailable from /proc/meminfo (Android only).
#[cfg(target_os = "android")]
fn read_proc_meminfo_available() -> Option<u32> {
    let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in contents.lines() {
        if line.starts_with("MemAvailable:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let kb: u64 = parts[1].parse().ok()?;
                return Some((kb / 1024) as u32);
            }
        }
    }
    None
}

/// Legacy RAM-only tier selection (kept for backward compatibility).
///
/// Prefer `select_tier_intelligent()` for new code.
pub fn select_tier(available_mb: u32) -> ModelTier {
    select_tier_by_ram(available_mb)
}

// ─── FFI wrappers ───────────────────────────────────────────────────────────

/// Load a model via the llama backend.  Returns (model_ptr, context_ptr).
///
/// On host stubs, returns non-null sentinel pointers from the stub backend.
/// On Android, loads the real GGUF model via FFI.
///
/// # Errors
/// Returns null pointers if loading fails — callers must check both pointers.
fn load_model_ffi(
    path: &Path,
    ctx_params: &LlamaContextParams,
) -> (*mut LlamaModel, *mut LlamaContext) {
    let model_params = LlamaModelParams::default();
    let path_str = path.to_string_lossy();

    // Ensure backend is initialized
    if !aura_llama_sys::is_backend_initialized() {
        #[cfg(not(target_os = "android"))]
        {
            if let Err(e) = aura_llama_sys::init_stub_backend(0xA0BA) {
                error!(error = %e, "failed to initialize stub backend");
                return (std::ptr::null_mut(), std::ptr::null_mut());
            }
        }
        #[cfg(target_os = "android")]
        {
            // On Android, the caller (neocortex main) must init the FFI backend
            // before loading models. If we get here without init, it's a bug.
            error!("FFI backend not initialized — call init_ffi_backend() before loading models");
            return (std::ptr::null_mut(), std::ptr::null_mut());
        }
    }

    let backend = aura_llama_sys::backend();

    match backend.load_model(&path_str, &model_params, ctx_params) {
        Ok((model_ptr, ctx_ptr)) => {
            info!(
                path = %path_str,
                stub = backend.is_stub(),
                "model loaded via backend"
            );
            (model_ptr, ctx_ptr)
        }
        Err(e) => {
            error!(error = %e, path = %path_str, "model load failed");
            (std::ptr::null_mut(), std::ptr::null_mut())
        }
    }
}

/// Free model + context via the llama backend.
fn free_model_ffi(model_ptr: *mut LlamaModel, ctx_ptr: *mut LlamaContext) {
    if aura_llama_sys::is_backend_initialized() {
        aura_llama_sys::backend().free_model(model_ptr, ctx_ptr);
        debug!("model freed via backend");
    } else {
        warn!("attempted to free model but backend not initialized — possible leak");
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Tier helpers ────────────────────────────────────────────────────

    #[test]
    fn tier_selection_by_ram() {
        assert_eq!(select_tier_by_ram(512), ModelTier::Brainstem1_5B);
        assert_eq!(select_tier_by_ram(1500), ModelTier::Brainstem1_5B);
        assert_eq!(select_tier_by_ram(2800), ModelTier::Standard4B);
        assert_eq!(select_tier_by_ram(4000), ModelTier::Standard4B);
        assert_eq!(select_tier_by_ram(5200), ModelTier::Full8B);
        assert_eq!(select_tier_by_ram(8192), ModelTier::Full8B);
    }

    #[test]
    fn tier_downgrade_chain() {
        assert_eq!(
            tier_downgrade(ModelTier::Full8B),
            Some(ModelTier::Standard4B)
        );
        assert_eq!(
            tier_downgrade(ModelTier::Standard4B),
            Some(ModelTier::Brainstem1_5B)
        );
        assert_eq!(tier_downgrade(ModelTier::Brainstem1_5B), None);
    }

    #[test]
    fn tier_upgrade_chain() {
        assert_eq!(
            tier_upgrade(ModelTier::Brainstem1_5B),
            Some(ModelTier::Standard4B)
        );
        assert_eq!(tier_upgrade(ModelTier::Standard4B), Some(ModelTier::Full8B));
        assert_eq!(tier_upgrade(ModelTier::Full8B), None);
    }

    #[test]
    fn tier_filenames_are_valid() {
        for tier in ALL_TIERS {
            let name = tier_filename(tier);
            assert!(name.ends_with(".gguf"));
            assert!(name.contains("qwen"));
        }
    }

    #[test]
    fn tier_memory_estimates_ordered() {
        let b = tier_approx_memory_mb(ModelTier::Brainstem1_5B);
        let s = tier_approx_memory_mb(ModelTier::Standard4B);
        let f = tier_approx_memory_mb(ModelTier::Full8B);
        assert!(b < s);
        assert!(s < f);
    }

    #[test]
    fn tier_ordinals_are_ordered() {
        assert!(tier_ordinal(ModelTier::Brainstem1_5B) < tier_ordinal(ModelTier::Standard4B));
        assert!(tier_ordinal(ModelTier::Standard4B) < tier_ordinal(ModelTier::Full8B));
    }

    // ── Task complexity ─────────────────────────────────────────────────

    #[test]
    fn complexity_scoring_reflexive() {
        let c = TaskComplexity::score(InferenceMode::Conversational, 200, false, false, false);
        assert_eq!(c, TaskComplexity::Reflexive);
    }

    #[test]
    fn complexity_scoring_standard() {
        let c = TaskComplexity::score(InferenceMode::Planner, 500, false, false, false);
        assert_eq!(c, TaskComplexity::Standard);
    }

    #[test]
    fn complexity_scoring_complex() {
        let c = TaskComplexity::score(InferenceMode::Planner, 1000, true, false, false);
        assert_eq!(c, TaskComplexity::Complex);
    }

    #[test]
    fn complexity_scoring_deep() {
        let c = TaskComplexity::score(InferenceMode::Strategist, 2500, true, true, false);
        assert_eq!(c, TaskComplexity::Deep);
    }

    #[test]
    fn retry_bumps_complexity() {
        let without =
            TaskComplexity::score(InferenceMode::Conversational, 200, false, false, false);
        let with = TaskComplexity::score(InferenceMode::Conversational, 200, false, false, true);
        assert!(with > without);
    }

    #[test]
    fn complexity_recommended_tiers() {
        assert_eq!(
            TaskComplexity::Reflexive.recommended_min_tier(),
            ModelTier::Brainstem1_5B
        );
        assert_eq!(
            TaskComplexity::Standard.recommended_min_tier(),
            ModelTier::Brainstem1_5B
        );
        assert_eq!(
            TaskComplexity::Complex.recommended_min_tier(),
            ModelTier::Standard4B
        );
        assert_eq!(
            TaskComplexity::Deep.recommended_min_tier(),
            ModelTier::Full8B
        );
    }

    // ── Power state ─────────────────────────────────────────────────────

    #[test]
    fn power_state_from_battery() {
        assert_eq!(PowerState::from_battery(5, false), PowerState::Critical);
        assert_eq!(PowerState::from_battery(20, false), PowerState::Low);
        assert_eq!(PowerState::from_battery(50, false), PowerState::Normal);
        assert_eq!(PowerState::from_battery(5, true), PowerState::Charging);
    }

    #[test]
    fn power_state_max_tiers() {
        assert_eq!(
            PowerState::Critical.max_allowed_tier(),
            ModelTier::Brainstem1_5B
        );
        assert_eq!(PowerState::Low.max_allowed_tier(), ModelTier::Standard4B);
        assert_eq!(PowerState::Normal.max_allowed_tier(), ModelTier::Full8B);
        assert_eq!(PowerState::Charging.max_allowed_tier(), ModelTier::Full8B);
    }

    // ── Intelligent tier selection ──────────────────────────────────────

    #[test]
    fn intelligent_selection_basic() {
        let decision =
            select_tier_intelligent(8192, PowerState::Normal, TaskComplexity::Standard, None);
        // Standard complexity starts at Brainstem, but should be acceptable
        assert_eq!(decision.recommended_tier, ModelTier::Brainstem1_5B);
        assert!(!decision.is_escalation);
    }

    #[test]
    fn intelligent_selection_deep_task() {
        let decision =
            select_tier_intelligent(8192, PowerState::Normal, TaskComplexity::Deep, None);
        assert_eq!(decision.recommended_tier, ModelTier::Full8B);
    }

    #[test]
    fn intelligent_selection_power_clamp() {
        let decision =
            select_tier_intelligent(8192, PowerState::Critical, TaskComplexity::Deep, None);
        // Deep wants Full8B but Critical power clamps to Brainstem
        assert_eq!(decision.recommended_tier, ModelTier::Brainstem1_5B);
    }

    #[test]
    fn intelligent_selection_ram_clamp() {
        let decision =
            select_tier_intelligent(1500, PowerState::Normal, TaskComplexity::Deep, None);
        // Deep wants Full8B but only 1500 MB available
        assert_eq!(decision.recommended_tier, ModelTier::Brainstem1_5B);
    }

    #[test]
    fn intelligent_selection_escalation() {
        let decision = select_tier_intelligent(
            8192,
            PowerState::Normal,
            TaskComplexity::Standard,
            Some(0.3), // Very low confidence from previous attempt
        );
        // Should escalate from Brainstem to Standard
        assert_eq!(decision.recommended_tier, ModelTier::Standard4B);
        assert!(decision.is_escalation);
    }

    // ── Cascade decisions ───────────────────────────────────────────────

    #[test]
    fn cascade_up_when_low_confidence() {
        let decision = should_cascade_up(
            ModelTier::Brainstem1_5B,
            0.3, // Below 0.65 threshold
            8192,
            PowerState::Normal,
        );
        assert!(decision.is_some());
        let d = decision.unwrap();
        assert_eq!(d.recommended_tier, ModelTier::Standard4B);
        assert!(d.is_escalation);
    }

    #[test]
    fn no_cascade_when_confidence_ok() {
        let decision = should_cascade_up(
            ModelTier::Brainstem1_5B,
            0.8, // Above 0.65 threshold
            8192,
            PowerState::Normal,
        );
        assert!(decision.is_none());
    }

    #[test]
    fn no_cascade_at_top_tier() {
        let decision = should_cascade_up(
            ModelTier::Full8B,
            0.1, // Very low but already at top
            8192,
            PowerState::Normal,
        );
        assert!(decision.is_none());
    }

    #[test]
    fn cascade_blocked_by_power() {
        let decision = should_cascade_up(
            ModelTier::Brainstem1_5B,
            0.3,
            8192,
            PowerState::Critical, // Critical blocks escalation
        );
        assert!(decision.is_none());
    }

    #[test]
    fn cascade_blocked_by_ram() {
        let decision = should_cascade_up(
            ModelTier::Standard4B,
            0.3,
            3000, // Not enough for Full8B (needs 5000+)
            PowerState::Normal,
        );
        assert!(decision.is_none());
    }

    // ── Tier capability ─────────────────────────────────────────────────

    #[test]
    fn tier_capabilities_are_ordered() {
        let b = tier_capability(ModelTier::Brainstem1_5B);
        let s = tier_capability(ModelTier::Standard4B);
        let f = tier_capability(ModelTier::Full8B);

        assert!(b.quality_score < s.quality_score);
        assert!(s.quality_score < f.quality_score);
        assert!(b.est_tokens_per_sec > s.est_tokens_per_sec);
        assert!(s.est_tokens_per_sec > f.est_tokens_per_sec);
    }

    // ── Model manager ───────────────────────────────────────────────────

    #[test]
    fn model_manager_load_unload_stub() {
        let dir = std::env::temp_dir().join("aura_test_models");
        let mut mgr = ModelManager::new(dir);

        assert!(!mgr.is_loaded());
        assert_eq!(mgr.cascade_count(), 0);

        let params = ModelParams::default();
        let result = mgr.load("/tmp/models", &params);
        assert!(result.is_ok());
        assert!(mgr.is_loaded());

        let (model_name, memory_used) = result.expect("load should succeed");
        assert!(model_name.contains("qwen"));
        assert!(memory_used > 0);

        // Stub backend returns sentinel non-null pointers (0x1, 0x2),
        // so LoadedModel::is_stub() returns false. Check stub mode via backend instead.
        assert!(!mgr.loaded().expect("should be loaded").is_stub());
        assert!(aura_llama_sys::backend().is_stub());

        mgr.unload();
        assert!(!mgr.is_loaded());
    }

    #[test]
    fn idle_timeout_logic() {
        let dir = std::env::temp_dir().join("aura_test_models_idle");
        let mut mgr = ModelManager::new(dir);
        let params = ModelParams::default();
        let _ = mgr.load("/tmp/models", &params);

        assert!(!mgr.should_idle_unload(false, false));
        assert!(!mgr.should_idle_unload(false, true));
    }

    #[test]
    fn loaded_model_inference_tracking() {
        let dir = std::env::temp_dir().join("aura_test_tracking");
        let mut mgr = ModelManager::new(dir);
        let params = ModelParams::default();
        let _ = mgr.load("/tmp/models", &params);

        let model = mgr.loaded_mut().expect("should be loaded");
        assert_eq!(model.inference_count, 0);
        assert!(model.average_confidence().is_none());

        model.record_inference(0.8);
        model.record_inference(0.6);
        assert_eq!(model.inference_count, 2);

        let avg = model.average_confidence().expect("should have avg");
        assert!((avg - 0.7).abs() < 0.01);
    }

    #[test]
    fn cascade_counter_management() {
        let dir = std::env::temp_dir().join("aura_test_cascade");
        let mut mgr = ModelManager::new(dir);

        assert_eq!(mgr.cascade_count(), 0);
        assert!(!mgr.cascades_exhausted());

        // Simulate cascades
        let params = ModelParams::default();
        let _ = mgr.load("/tmp/models", &params);

        let _ = mgr.cascade_to(ModelTier::Full8B, &params);
        assert_eq!(mgr.cascade_count(), 1);

        mgr.reset_cascades();
        assert_eq!(mgr.cascade_count(), 0);
    }

    #[test]
    fn available_ram_returns_reasonable_value() {
        let ram = available_ram_mb();
        assert!(ram >= 1024, "RAM should be at least 1 GB, got {ram} MB");
    }

    #[test]
    fn resolve_path_uses_model_dir_fallback() {
        let model_dir = PathBuf::from("/data/local/aura/models");
        let path = resolve_model_path(&model_dir, "some-name", ModelTier::Standard4B);
        assert!(path.to_string_lossy().contains("qwen3.5-4b"));
    }

    #[test]
    fn all_tiers_constant_is_ordered() {
        assert_eq!(ALL_TIERS.len(), 3);
        assert_eq!(ALL_TIERS[0], ModelTier::Brainstem1_5B);
        assert_eq!(ALL_TIERS[1], ModelTier::Standard4B);
        assert_eq!(ALL_TIERS[2], ModelTier::Full8B);
    }
}
