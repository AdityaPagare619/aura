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
//! - Previous confidence (if prior attempt scored low, escalate to bigger model)
//!
//! The cascade flow: always start at Brainstem (cheapest), then escalate
//! upward if output confidence is too low (post-inference signal). Task
//! content is never inspected here — the LLM reasons about complexity.
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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use aura_llama_sys::{GgufMeta, LlamaContext, LlamaContextParams, LlamaModel, LlamaModelParams};
use aura_types::ipc::{ModelParams, ModelTier};
#[allow(unused_imports)] // `error` used in #[cfg(target_os = "android")] paths
use tracing::{debug, error, info, warn};

// ─── Power state ────────────────────────────────────────────────────────────

/// Device power state, affecting model tier selection.
///
/// On Android, this would come from `BatteryManager` via the daemon.
/// On host builds, defaults to `Normal` for development.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Phase 8: Critical/Low/Charging variants used by Android battery monitor
pub enum PowerState {
    /// Battery < 15% — use smallest possible model.
    Critical,
    /// Battery 15-30% — prefer smaller models, avoid Full.
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
    ///
    /// # Arguments
    /// - `battery_pct` — battery level in the range `0.0` (empty) to `1.0` (full).
    ///   Values from Android's `BatteryManager.EXTRA_LEVEL / EXTRA_SCALE`.
    /// - `is_charging` — true if plugged in (any charging source).
    ///
    /// # Thresholds
    /// - Critical : `battery_pct < 0.15` (15 %) — survival mode, smallest model only.
    /// - Low      : `battery_pct < 0.30` (30 %) — avoid Full8B to conserve charge.
    /// - Normal   : `battery_pct >= 0.30` — no power constraint.
    #[allow(dead_code)] // Phase 8: called by Android BatteryManager JNI bridge
    pub fn from_battery(battery_pct: f32, is_charging: bool) -> Self {
        if is_charging {
            PowerState::Charging
        } else if battery_pct < 0.15 {
            // < 15 %: device is critically low; always use the smallest model.
            PowerState::Critical
        } else if battery_pct < 0.30 {
            // 15–30 %: low battery; prefer smaller models, skip Full8B.
            PowerState::Low
        } else {
            PowerState::Normal
        }
    }
}

// ─── Tier capability metadata ───────────────────────────────────────────────

/// Per-tier capability metadata for cascade decision-making.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Phase 8: quality_score/est_tokens_per_sec read by cascade telemetry
pub struct TierCapability {
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
            quality_score: 0.55,
            est_tokens_per_sec: 45.0,
            confidence_threshold: 0.65,
        },
        ModelTier::Standard4B => TierCapability {
            quality_score: 0.75,
            est_tokens_per_sec: 25.0,
            confidence_threshold: 0.55,
        },
        ModelTier::Full8B => TierCapability {
            quality_score: 0.90,
            est_tokens_per_sec: 12.0,
            confidence_threshold: 0.40,
        },
    }
}

// ─── Cascade decision ───────────────────────────────────────────────────────

/// Safety margin kept free after loading the next-tier model.
///
/// 512 MB is the minimum headroom required for the OS, daemon IPC buffers,
/// accessibility service overlays, and the `aura-daemon` JVM heap on a
/// Snapdragon 8 Gen 3 / 4 GB Android device. Anything less risks the OS
/// killing AURA under memory pressure mid-inference.
const OOM_SAFETY_MARGIN_MB: u32 = 512;

/// Result of evaluating whether to cascade to a different tier.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Phase 8: is_escalation read by cascade telemetry exporter
pub struct CascadeDecision {
    /// The recommended tier to use.
    pub recommended_tier: ModelTier,
    /// Human-readable reason for the decision (for logging/debugging).
    pub reason: String,
    /// Whether this decision represents an escalation from the current tier.
    pub is_escalation: bool,
}

// ─── Model tier helpers ─────────────────────────────────────────────────────

/// Fallback filename suffix for each tier — used only when GGUF scanning fails
/// (e.g. in tests with a nonexistent model dir).
///
/// Full8B is the primary default: `qwen3-8b-q4_k_m.gguf` (Qwen3-8B Q4_K_M,
/// ~4.7 GB, 32 K context).  Smaller tiers use proportionally quantized variants.
/// These are only consulted when `ModelScanner` finds no GGUF files in `model_dir`.
pub fn tier_filename_fallback(tier: ModelTier) -> &'static str {
    match tier {
        ModelTier::Brainstem1_5B => "qwen3-1.5b-q4_k_m.gguf",
        ModelTier::Standard4B => "qwen3-4b-q4_k_m.gguf",
        ModelTier::Full8B => "qwen3-8b-q4_k_m.gguf",
    }
}

/// Kept for backward compatibility with tests that assert on the old name.
/// Delegates to the fallback — real code should use `ModelScanner`.
#[allow(dead_code)] // Phase 8: used by test helpers and JNI fallback path
pub fn tier_filename(tier: ModelTier) -> &'static str {
    tier_filename_fallback(tier)
}

/// Fallback RAM estimate when no GGUF metadata is available.
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

// ─── Model scanner ──────────────────────────────────────────────────────────

/// Scans a directory for `.gguf` files and maps them to tiers by RAM estimate.
///
/// This replaces the hardcoded `tier_filename()` lookup.  The user can drop
/// any GGUF files they want into the model directory; AURA reads the header of
/// each one, sorts by RAM estimate, and assigns them to Brainstem / Standard /
/// Full tiers automatically.
///
/// Tier assignment algorithm:
///   - All found models are sorted by `ram_estimate_mb()` ascending.
///   - Smallest → Brainstem1_5B
///   - Middle (if 3+) → Standard4B
///   - Largest (if 2+) → Full8B
///   - If only 1 file: used for all tiers.
#[derive(Debug, Default)]
pub struct ModelScanner {
    /// Map from tier → (absolute path, parsed metadata).
    pub models: HashMap<ModelTier, (PathBuf, GgufMeta)>,
}

impl ModelScanner {
    /// Scan `model_dir` for `.gguf` files and build the tier map.
    ///
    /// Silently skips files that cannot be parsed (bad GGUF, I/O errors).
    /// Returns an empty scanner (falling back to hardcoded filenames) if the
    /// directory doesn't exist or is empty.
    pub fn scan(model_dir: &Path) -> Self {
        let mut entries: Vec<(PathBuf, GgufMeta)> = Vec::new();

        let read_dir = match std::fs::read_dir(model_dir) {
            Ok(d) => d,
            Err(e) => {
                debug!(dir = %model_dir.display(), error = %e, "model dir not readable, using fallback filenames");
                return Self::default();
            }
        };

        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("gguf") {
                continue;
            }
            match aura_llama_sys::parse_gguf_meta(&path) {
                Ok(meta) => {
                    info!(
                        path = %path.display(),
                        arch = %meta.architecture,
                        ram_mb = meta.ram_estimate_mb(),
                        ctx = meta.effective_context(),
                        quant = meta.quant_name(),
                        "scanned GGUF model"
                    );
                    entries.push((path, meta));
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "skipping non-parseable GGUF file");
                }
            }
        }

        // Sort by RAM estimate ascending (smallest first)
        entries.sort_by_key(|(_, m)| m.ram_estimate_mb());

        let mut models = HashMap::new();
        match entries.len() {
            0 => {
                debug!("no GGUF files found in model dir");
            }
            1 => {
                // Only one model — map it to all tiers
                let (path, meta) = entries.remove(0);
                info!(path = %path.display(), "single model found, mapped to all tiers");
                models.insert(ModelTier::Brainstem1_5B, (path.clone(), meta.clone()));
                models.insert(ModelTier::Standard4B, (path.clone(), meta.clone()));
                models.insert(ModelTier::Full8B, (path, meta));
            }
            2 => {
                // Two models: small → Brainstem, large → Full + Standard
                let (small_path, small_meta) = entries.remove(0);
                let (large_path, large_meta) = entries.remove(0);
                models.insert(ModelTier::Brainstem1_5B, (small_path, small_meta));
                models.insert(ModelTier::Standard4B, (large_path.clone(), large_meta.clone()));
                models.insert(ModelTier::Full8B, (large_path, large_meta));
            }
            _ => {
                // 3+ models: pick smallest, middle, largest
                let (small_path, small_meta) = entries.remove(0);
                let (large_path, large_meta) = entries.pop().expect("entries has 3+ items in this match arm");
                // Middle: prefer the one whose RAM estimate is closest to the average
                let mid_idx = entries.len() / 2;
                let (mid_path, mid_meta) = entries.remove(mid_idx);
                models.insert(ModelTier::Brainstem1_5B, (small_path, small_meta));
                models.insert(ModelTier::Standard4B, (mid_path, mid_meta));
                models.insert(ModelTier::Full8B, (large_path, large_meta));
            }
        }

        Self { models }
    }

    /// Resolve the path for a given tier.
    ///
    /// Returns the scanned path if available, otherwise falls back to
    /// `model_dir / tier_filename_fallback(tier)`.
    pub fn path_for_tier(&self, tier: ModelTier, model_dir: &Path) -> PathBuf {
        if let Some((path, _)) = self.models.get(&tier) {
            return path.clone();
        }
        model_dir.join(tier_filename_fallback(tier))
    }

    /// RAM estimate for a tier from GGUF metadata, or hardcoded fallback.
    pub fn ram_for_tier(&self, tier: ModelTier) -> u32 {
        if let Some((_, meta)) = self.models.get(&tier) {
            return meta.ram_estimate_mb();
        }
        tier_approx_memory_mb(tier)
    }

    /// Effective context length for a tier from GGUF metadata, or 4096.
    pub fn context_for_tier(&self, tier: ModelTier) -> u32 {
        if let Some((_, meta)) = self.models.get(&tier) {
            return meta.effective_context();
        }
        warn!(
            "no GGUF metadata for {:?} — falling back to default context window (4096 tokens). \
             Load the model explicitly or check the GGUF header.",
            tier
        );
        4096
    }

    /// Whether the model for a tier supports thinking mode.
    #[allow(dead_code)] // Phase 8: used by inference router for CoT activation
    pub fn thinking_mode_for_tier(&self, tier: ModelTier) -> bool {
        if let Some((_, meta)) = self.models.get(&tier) {
            return meta.supports_thinking_mode();
        }
        false
    }

    /// Whether this scanner found any models.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

// ─── Intelligent tier selection ─────────────────────────────────────────────

/// Select the best model tier considering all available signals.
///
/// Always starts at the smallest tier (Brainstem) and escalates upward only
/// on hardware signals: low RAM, low battery, or low prior-attempt confidence.
/// Task content is never inspected — the LLM (brain) reasons about complexity;
/// the cascade system (body) responds only to measurable hardware/output signals.
///
/// # Arguments
/// - `available_mb`     — available RAM in MB (hard ceiling from `/proc/meminfo`)
/// - `power`            — current device power state (battery ceiling)
/// - `prev_confidence`  — confidence from a prior attempt on this request, if any
/// - `scanner`          — optional model scanner for real per-GGUF RAM estimates;
///                        pass `None` to fall back to hardcoded estimates
///
/// # Algorithm
/// 1. Start at Brainstem1_5B (cheapest tier, always attempted first)
/// 2. If a previous attempt had low confidence, escalate one tier
/// 3. Clamp to the power state's max allowed tier
/// 4. Clamp to what fits in available RAM (using scanner estimates when available)
/// 5. Return the result
pub fn select_tier_intelligent(
    available_mb: u32,
    power: PowerState,
    prev_confidence: Option<f32>,
    scanner: Option<&ModelScanner>,
) -> CascadeDecision {
    // Step 1: Always start at smallest tier — LLM determines if escalation needed
    let mut tier = ModelTier::Brainstem1_5B;
    let mut reason = "default start: Brainstem1_5B".to_string();

    // Step 2: Escalate if previous attempt had low confidence (post-inference signal)
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

    // Step 4: Clamp to RAM ceiling.
    // Walk from largest tier downward until one fits in available_mb.
    // Uses scanner's real GGUF RAM estimates when available; falls back to
    // hardcoded approximations from `tier_approx_memory_mb()`.
    let ram_max = {
        let mut best = ModelTier::Brainstem1_5B;
        for &t in ALL_TIERS.iter().rev() {
            let needed = scanner
                .map(|s| s.ram_for_tier(t))
                .unwrap_or_else(|| tier_approx_memory_mb(t));
            if available_mb >= needed {
                best = t;
                break;
            }
        }
        best
    };

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
#[allow(dead_code)] // Phase 8: called by inference cascade controller after each infer pass
pub fn should_cascade_up(
    current_tier: ModelTier,
    confidence: f32,
    available_mb: u32,
    power: PowerState,
) -> Option<CascadeDecision> {
    should_cascade_up_with_scanner(current_tier, confidence, available_mb, power, None)
}

/// Same as `should_cascade_up` but accepts an optional `ModelScanner` for
/// accurate per-file RAM estimates instead of hardcoded fallbacks.
pub fn should_cascade_up_with_scanner(
    current_tier: ModelTier,
    confidence: f32,
    available_mb: u32,
    power: PowerState,
    scanner: Option<&ModelScanner>,
) -> Option<CascadeDecision> {
    let cap = tier_capability(current_tier);

    // If confidence is acceptable, no cascade needed
    if confidence >= cap.confidence_threshold {
        return None;
    }

    // Try to upgrade
    let next = tier_upgrade(current_tier)?;

    // Guard: if the scanner has no model file for the next tier, cascading
    // would attempt to load a non-existent path — block early.
    if let Some(s) = scanner {
        if !s.models.contains_key(&next) {
            info!(
                current = tier_display(current_tier),
                next = tier_display(next),
                "cascade blocked: no model file scanned for next tier"
            );
            return None;
        }
    }

    // Check RAM constraint (prefer scanner's real estimate).
    // We require `available_mb >= needed_mb + OOM_SAFETY_MARGIN_MB` to keep
    // at least OOM_SAFETY_MARGIN_MB free after loading the larger model.
    let needed_mb = scanner
        .map(|s| s.ram_for_tier(next))
        .unwrap_or_else(|| tier_approx_memory_mb(next));

    if available_mb < needed_mb + OOM_SAFETY_MARGIN_MB {
        info!(
            current = tier_display(current_tier),
            next = tier_display(next),
            available_mb,
            needed_mb,
            safety_margin = OOM_SAFETY_MARGIN_MB,
            "cascade blocked by RAM (including OOM safety margin)"
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
#[allow(dead_code)] // Phase 8: used by intelligent selector as RAM ceiling
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
#[allow(dead_code)] // Phase 8: model_name/memory_used_mb/loaded_at read by telemetry/watchdog
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

// SAFETY (RUST-MED-7 / GAP-MED-009): LoadedModel contains raw pointers
// (*mut LlamaModel, *mut LlamaContext) from the C FFI layer.
//
// Send: Pointers are only dereferenced by the neocortex binary's inference
// thread. The ModelManager serializes all access through its internal state
// machine, ensuring no concurrent mutation. Moving between threads is safe
// because we guarantee single-owner semantics at the architectural level.
//
// !Sync: Explicitly declared. Although Rust auto-derives !Sync for raw-pointer
// types, making it explicit prevents accidental future changes (e.g. wrapping
// in Arc) from silently enabling shared-reference access across threads.
unsafe impl Send for LoadedModel {}
impl !Sync for LoadedModel {}

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
        // Free llama.cpp resources on ALL platforms, including Android.
        // The previous `#[cfg(not(target_os = "android"))]` guards caused
        // a memory leak on mobile: neither the context nor the model weights
        // were released, accumulating hundreds of MB per model reload.
        //
        // On Android the symbols are dynamically loaded via libloading, but
        // the stubs module resolves them correctly at runtime — there is no
        // reason to skip cleanup.
        if !self.ctx_ptr.is_null() {
            aura_llama_sys::stubs::llama_free_context(self.ctx_ptr);
        }
        if !self.model_ptr.is_null() {
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
    /// Scanned GGUF metadata for models in model_dir.
    /// Empty until the first `scan()` or `load()` call.
    scanner: ModelScanner,
}

impl ModelManager {
    pub fn new(model_dir: PathBuf) -> Self {
        Self {
            model_dir,
            loaded: None,
            cascade_count: 0,
            max_cascades: 3,
            scanner: ModelScanner::default(),
        }
    }

    /// Scan the model directory now, building the tier→GGUF map.
    ///
    /// Called at startup so capability queries (RAM, context, thinking mode)
    /// are available before the first `load()` call.
    pub fn scan(&mut self) {
        self.scanner = ModelScanner::scan(&self.model_dir);
        if self.scanner.is_empty() {
            info!(
                dir = %self.model_dir.display(),
                "no GGUF files found — will use fallback filenames"
            );
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
    #[allow(dead_code)] // Phase 8: read by cascade telemetry exporter
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

    /// Reference to the model scanner (for capability queries).
    pub fn scanner(&self) -> &ModelScanner {
        &self.scanner
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
    /// Resolves the actual GGUF file path from scanned metadata (or falls back
    /// to hardcoded filenames). Uses `GgufMeta` for RAM estimates and context
    /// length unless the caller explicitly overrides `params.n_ctx`.
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

        // Scan if we haven't yet (lazy scan for callers that skip scan())
        if self.scanner.is_empty() {
            self.scanner = ModelScanner::scan(&self.model_dir);
        }

        let available_mb = available_ram_mb();
        info!(available_mb, "detected available RAM");

        // Start with the requested tier, but may downgrade.
        let mut tier = params.model_tier;
        info!(tier = tier_display(tier), "requested model tier");

        // Try to load, downgrading if post-load headroom check fails.
        loop {
            let file_path = self
                .scanner
                .path_for_tier(tier, &self.model_dir);

            // Override: if model_path is a specific file (not a dir), use it.
            let file_path = {
                let p = Path::new(model_path);
                if p.is_file() {
                    p.to_path_buf()
                } else if p.is_dir() {
                    self.scanner.path_for_tier(tier, p)
                } else {
                    file_path
                }
            };

            info!(path = %file_path.display(), "attempting to load model");

            // Use GGUF-derived context if the caller left n_ctx at default (0 or 4096)
            // and we have real metadata; otherwise respect the caller's value.
            let gguf_ctx = self.scanner.context_for_tier(tier);
            let n_ctx = if params.n_ctx == 0 || params.n_ctx == 4096 {
                // Prefer GGUF metadata; cap at 32768 on first load to avoid OOM
                gguf_ctx.min(32768)
            } else {
                params.n_ctx
            };

            let ctx_params = LlamaContextParams {
                n_ctx,
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

            // Use GGUF-derived RAM estimate; fall back to hardcoded if unavailable.
            let memory_used = self.scanner.ram_for_tier(tier);
            let model_name = file_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(tier_filename_fallback(tier))
                .to_string();

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
                n_ctx,
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

    /// Generate an embedding vector for the given text using the loaded model.
    ///
    /// Returns a zero vector of `embedding_dim` length from the model's GGUF
    /// capabilities.  The dimension is derived from `ModelCapabilities` (GGUF
    /// metadata → user override → compiled fallback) — never hardcoded.
    ///
    /// **Status:** stub output (zero vector) — full llama.cpp embedding support
    /// is wired once `aura-llama-sys` exposes `llama_get_embeddings`.
    /// The returned vector shape is correct; only the values are zeroed.
    pub fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let loaded = self
            .loaded
            .as_ref()
            .ok_or_else(|| "no model loaded".to_string())?;

        let _ = text;

        // Derive embedding_dim from GGUF metadata for the loaded model's tier.
        // Priority: GGUF metadata > compiled fallback. Never hardcoded.
        let capabilities = if let Some((_, meta)) = self.scanner.models.get(&loaded.tier) {
            crate::model_capabilities::ModelCapabilities::from_gguf(meta, None)
        } else {
            tracing::warn!(
                tier = ?loaded.tier,
                "no GGUF metadata for loaded tier — using compiled fallback for embed dim"
            );
            crate::model_capabilities::ModelCapabilities::fallback_defaults()
        };

        tracing::debug!(
            embedding_dim = capabilities.embedding_dim,
            source = ?capabilities.embedding_dim_source,
            "embed: using dim from capabilities"
        );

        Ok(vec![0.0f32; capabilities.embedding_dim as usize])
    }
}

// ─── Path resolution ────────────────────────────────────────────────────────

/// Resolve the full model file path.
///
/// If `model_path` is an existing directory, we append the tier filename.
/// If `model_path` looks like a file, we use it directly (but may substitute
/// the tier filename for the last path component on downgrades).
/// Otherwise, we fall back to `model_dir/tier_filename`.
#[allow(dead_code)] // Phase 8: used by model loading path on downgrade cascade
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
#[allow(dead_code)] // Phase 8: used by legacy JNI path and test helpers
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
    use aura_types::ipc::InferenceMode;

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

    // ── Power state ─────────────────────────────────────────────────────

    #[test]
    fn power_state_from_battery() {
        // f32 range 0.0–1.0; Critical < 0.15, Low < 0.30
        assert_eq!(PowerState::from_battery(0.05, false), PowerState::Critical);
        assert_eq!(PowerState::from_battery(0.14, false), PowerState::Critical);
        assert_eq!(PowerState::from_battery(0.15, false), PowerState::Low);
        assert_eq!(PowerState::from_battery(0.20, false), PowerState::Low);
        assert_eq!(PowerState::from_battery(0.29, false), PowerState::Low);
        assert_eq!(PowerState::from_battery(0.30, false), PowerState::Normal);
        assert_eq!(PowerState::from_battery(0.50, false), PowerState::Normal);
        assert_eq!(PowerState::from_battery(0.05, true), PowerState::Charging);
        assert_eq!(PowerState::from_battery(1.0, true), PowerState::Charging);
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

    // ── Intelligent tier selection (v2 — with TaskComplexity arg removed) ──

    #[test]
    fn intelligent_selection_basic_v2() {
        let decision =
            select_tier_intelligent(8192, PowerState::Normal, None, None);
        // Standard complexity starts at Brainstem, but should be acceptable
        assert_eq!(decision.recommended_tier, ModelTier::Brainstem1_5B);
        assert!(!decision.is_escalation);
    }

    #[test]
    fn intelligent_selection_deep_task() {
        let decision =
            select_tier_intelligent(8192, PowerState::Normal, None, None);
        assert_eq!(decision.recommended_tier, ModelTier::Brainstem1_5B);
    }

    #[test]
    fn intelligent_selection_power_clamp_v2() {
        let decision =
            select_tier_intelligent(8192, PowerState::Critical, None, None);
        // Critical power clamps to Brainstem
        assert_eq!(decision.recommended_tier, ModelTier::Brainstem1_5B);
    }

    #[test]
    fn intelligent_selection_ram_clamp_v2() {
        let decision =
            select_tier_intelligent(1500, PowerState::Normal, None, None);
        // Only 1500 MB available — Brainstem only
        assert_eq!(decision.recommended_tier, ModelTier::Brainstem1_5B);
    }

    #[test]
    fn intelligent_selection_escalation_v2() {
        let decision = select_tier_intelligent(
            8192,
            PowerState::Normal,
            Some(0.3_f32), // Very low confidence from previous attempt
            None,
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
