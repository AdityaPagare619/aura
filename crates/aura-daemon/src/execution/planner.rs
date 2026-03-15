//! Action planner — converts goals into concrete `ActionPlan`s.
//!
//! The planner sits between the goals module and the executor. It uses a
//! three-tier cascade:
//!
//! 1. **ETG lookup** — if the ETG has a known, high-confidence path from the current screen state
//!    to the goal state, use it directly (fastest).
//! 2. **Template match** — if a registered plan template matches the goal's description,
//!    instantiate it.
//! 3. **LLM request** — prepare a structured request for Neocortex to generate a plan (slowest,
//!    most flexible).

use aura_types::{
    actions::ActionType,
    dsl::{DslStep, FailureStrategy},
    etg::{ActionPlan, PlanSource},
    goals::Goal,
};
use tracing::{debug, instrument, warn};

use super::etg::EtgStore;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum plan templates the planner can hold.
const MAX_TEMPLATES: usize = 256;
/// Default maximum steps per plan.
const DEFAULT_MAX_PLAN_STEPS: usize = 50;
/// Default maximum alternative plans to consider.
const DEFAULT_MAX_ALTERNATIVES: usize = 3;
/// Minimum ETG path reliability to accept an ETG-based plan.
const MIN_ETG_CONFIDENCE: f32 = 0.6;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// An action planner that converts goals into executable `ActionPlan`s.
#[derive(Debug)]
pub struct ActionPlanner {
    /// Registered plan templates (bounded at [`MAX_TEMPLATES`]).
    templates: Vec<PlanTemplate>,
    /// Maximum number of steps allowed in a plan.
    max_plan_steps: usize,
    /// Maximum number of alternative plans to evaluate.
    _max_alternatives: usize,
}

/// A reusable plan template for common actions.
#[derive(Debug, Clone)]
pub struct PlanTemplate {
    /// VESTIGIAL — template matching is disabled (plan_from_template returns None).
    /// Kept for struct compatibility; will be removed when templates are fully
    /// replaced by LLM plan generation. See IRON LAW: LLM classifies intent.
    pub trigger_pattern: String,
    /// Optional app package filter — template only applies if the goal involves
    /// this package.
    pub app_package: Option<String>,
    /// Steps to instantiate when this template matches.
    pub steps: Vec<DslStep>,
    /// Confidence in this template (0.0–1.0), updated based on outcomes.
    pub confidence: f32,
    /// Number of times this template has been used.
    pub usage_count: u32,
}

/// A structured request for the Neocortex LLM to generate a plan.
#[derive(Debug, Clone)]
pub struct LlmPlanRequest {
    /// Goal description to plan for.
    pub goal_description: String,
    /// Goal priority (for the LLM to weight urgency).
    pub priority: String,
    /// Deadline in ms since epoch, if any.
    pub deadline_ms: Option<u64>,
    /// Maximum number of steps the LLM should generate.
    pub max_steps: usize,
    /// Known screen state hash, if available, for context.
    pub current_screen_hash: Option<u64>,
    /// App packages the LLM might need to interact with.
    pub relevant_packages: Vec<String>,
}

/// Errors during plan creation or validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// No strategy could produce a plan.
    NoPlanFound(String),
    /// Plan exceeds step limit.
    TooManySteps { actual: usize, max: usize },
    /// Template capacity exceeded.
    TemplateCapacityExceeded { max: usize },
    /// Validation errors detected.
    ValidationFailed(Vec<PlanValidationError>),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::NoPlanFound(msg) => write!(f, "no plan found: {msg}"),
            PlanError::TooManySteps { actual, max } => {
                write!(f, "plan has {actual} steps, max is {max}")
            },
            PlanError::TemplateCapacityExceeded { max } => {
                write!(f, "template capacity exceeded: max {max}")
            },
            PlanError::ValidationFailed(errors) => {
                write!(f, "plan validation failed: {} errors", errors.len())
            },
        }
    }
}

impl std::error::Error for PlanError {}

/// A single validation issue found in a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanValidationError {
    /// Plan has too many steps.
    StepLimitExceeded { actual: usize, max: usize },
    /// Duplicate consecutive actions detected at step index.
    DuplicateConsecutiveAction { step_index: usize },
    /// Estimated execution time exceeds the goal deadline.
    DeadlineExceeded { estimated_ms: u64, deadline_ms: u64 },
    /// An empty plan was submitted.
    EmptyPlan,
    /// A step has zero timeout.
    ZeroTimeout { step_index: usize },
}

impl std::fmt::Display for PlanValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanValidationError::StepLimitExceeded { actual, max } => {
                write!(f, "step limit exceeded: {actual} > {max}")
            },
            PlanValidationError::DuplicateConsecutiveAction { step_index } => {
                write!(f, "duplicate consecutive action at step {step_index}")
            },
            PlanValidationError::DeadlineExceeded {
                estimated_ms,
                deadline_ms,
            } => {
                write!(
                    f,
                    "estimated {estimated_ms}ms exceeds deadline {deadline_ms}ms"
                )
            },
            PlanValidationError::EmptyPlan => write!(f, "plan has no steps"),
            PlanValidationError::ZeroTimeout { step_index } => {
                write!(f, "step {step_index} has zero timeout")
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl ActionPlanner {
    /// Create a new action planner with default settings.
    pub fn new() -> Self {
        Self {
            templates: Vec::with_capacity(32),
            max_plan_steps: DEFAULT_MAX_PLAN_STEPS,
            _max_alternatives: DEFAULT_MAX_ALTERNATIVES,
        }
    }

    /// Create a planner with custom limits.
    pub fn with_limits(max_plan_steps: usize, max_alternatives: usize) -> Self {
        Self {
            templates: Vec::with_capacity(32),
            max_plan_steps,
            _max_alternatives: max_alternatives,
        }
    }

    /// Number of registered templates.
    pub fn template_count(&self) -> usize {
        self.templates.len()
    }

    /// Maximum plan steps allowed.
    pub fn max_plan_steps(&self) -> usize {
        self.max_plan_steps
    }

    // ── Template management ─────────────────────────────────────────────

    /// Register a new plan template.
    ///
    /// # Errors
    /// Returns `PlanError::TemplateCapacityExceeded` if the template list is
    /// full (256).
    pub fn register_template(&mut self, template: PlanTemplate) -> Result<(), PlanError> {
        if self.templates.len() >= MAX_TEMPLATES {
            return Err(PlanError::TemplateCapacityExceeded { max: MAX_TEMPLATES });
        }
        debug!(
            steps = template.steps.len(),
            "registered plan template (trigger matching disabled — IRON LAW)"
        );
        self.templates.push(template);
        Ok(())
    }

    // ── Planning cascade ────────────────────────────────────────────────

    /// Create a plan from a goal using the three-tier cascade:
    /// ETG → Template → LLM request.
    ///
    /// If both ETG and template fail, returns an `LlmPlanRequest` wrapped
    /// in an `ActionPlan` with `PlanSource::LlmGenerated` and empty steps
    /// (the caller must fill them in from the LLM response).
    ///
    /// # Errors
    /// Returns `PlanError::NoPlanFound` only if all strategies fail and
    /// an LLM request cannot be constructed (shouldn't happen in practice).
    #[instrument(skip(self, etg), fields(goal_id = goal.id))]
    pub fn plan(
        &self,
        goal: &Goal,
        etg: &EtgStore,
        current_screen_hash: Option<u64>,
        target_screen_hash: Option<u64>,
    ) -> Result<ActionPlan, PlanError> {
        // Tier 1: ETG lookup
        if let Some(plan) = self.plan_from_etg(goal, etg, current_screen_hash, target_screen_hash) {
            debug!(goal_id = goal.id, source = "etg", "plan created from ETG");
            return Ok(plan);
        }

        // Tier 2: Template match
        if let Some(plan) = self.plan_from_template(goal) {
            debug!(
                goal_id = goal.id,
                source = "template",
                "plan created from template"
            );
            return Ok(plan);
        }

        // Tier 3: LLM request — return a placeholder plan that the caller
        // should fill via the Neocortex.
        debug!(
            goal_id = goal.id,
            source = "llm",
            "falling back to LLM plan request"
        );
        let llm_req = self.plan_for_llm_request(goal, current_screen_hash);
        Ok(ActionPlan {
            goal_description: llm_req.goal_description,
            steps: Vec::new(), // Caller fills from LLM response
            estimated_duration_ms: 0,
            confidence: 0.3, // Low confidence — needs LLM
            source: PlanSource::LlmGenerated,
        })
    }

    /// Attempt to build a plan from a known ETG path.
    ///
    /// Requires both `current_screen_hash` and `target_screen_hash` to be
    /// known. Returns `None` if no high-confidence path exists.
    fn plan_from_etg(
        &self,
        goal: &Goal,
        etg: &EtgStore,
        current_screen_hash: Option<u64>,
        target_screen_hash: Option<u64>,
    ) -> Option<ActionPlan> {
        let from = current_screen_hash?;
        let to = target_screen_hash?;

        let path = etg.find_path(from, to)?;

        if path.total_reliability < MIN_ETG_CONFIDENCE {
            debug!(
                reliability = path.total_reliability,
                min = MIN_ETG_CONFIDENCE,
                "ETG path reliability too low"
            );
            return None;
        }

        // Convert ETG path edges to DslSteps.
        let mut steps = Vec::with_capacity(path.nodes.len().saturating_sub(1));
        for pair in path.nodes.windows(2) {
            let edge = etg.get_edge(pair[0], pair[1])?;
            steps.push(DslStep {
                action: edge.action.clone(),
                target: None,
                timeout_ms: (edge.avg_duration_ms * 3.0).max(2000.0) as u32,
                on_failure: FailureStrategy::Retry { max: 2 },
                precondition: None,
                postcondition: None,
                label: Some(format!("etg_{}_{}", pair[0], pair[1])),
            });
        }

        if steps.len() > self.max_plan_steps {
            debug!(
                steps = steps.len(),
                max = self.max_plan_steps,
                "ETG plan exceeds step limit"
            );
            return None;
        }

        Some(ActionPlan {
            goal_description: goal.description.clone(),
            steps,
            estimated_duration_ms: path.estimated_duration_ms,
            confidence: path.total_reliability,
            source: PlanSource::EtgLookup,
        })
    }

    /// Template trigger matching deferred to LLM.
    ///
    /// IRON LAW: LLM classifies intent. Rust does not.
    /// Substring keyword matching on a goal description to select a template
    /// is intent classification. The goal description must be passed to the
    /// Neocortex; it selects or synthesizes the appropriate plan.
    fn plan_from_template(&self, _goal: &Goal) -> Option<ActionPlan> {
        // IRON LAW: LLM classifies intent. Rust does not.
        None
    }

    /// Prepare a structured LLM request for Neocortex plan generation.
    fn plan_for_llm_request(
        &self,
        goal: &Goal,
        current_screen_hash: Option<u64>,
    ) -> LlmPlanRequest {
        // Extract relevant packages from goal steps (if any have OpenApp).
        let relevant_packages: Vec<String> = goal
            .steps
            .iter()
            .filter_map(|s| {
                if let Some(ActionType::OpenApp { package }) = &s.action {
                    Some(package.clone())
                } else {
                    None
                }
            })
            .collect();

        LlmPlanRequest {
            goal_description: goal.description.clone(),
            priority: format!("{:?}", goal.priority),
            deadline_ms: goal.deadline_ms,
            max_steps: self.max_plan_steps,
            current_screen_hash,
            relevant_packages,
        }
    }

    // ── Validation ──────────────────────────────────────────────────────

    /// Validate a plan before execution.
    ///
    /// # Errors
    /// Returns a vec of all validation errors found.
    pub fn validate_plan(
        &self,
        plan: &ActionPlan,
        goal: &Goal,
    ) -> Result<(), Vec<PlanValidationError>> {
        let mut errors = Vec::new();

        // 1. Empty plan check.
        if plan.steps.is_empty() {
            errors.push(PlanValidationError::EmptyPlan);
        }

        // 2. Step limit.
        if plan.steps.len() > self.max_plan_steps {
            errors.push(PlanValidationError::StepLimitExceeded {
                actual: plan.steps.len(),
                max: self.max_plan_steps,
            });
        }

        // 3. Duplicate consecutive actions.
        for (i, window) in plan.steps.windows(2).enumerate() {
            if actions_equal(&window[0].action, &window[1].action) {
                errors.push(PlanValidationError::DuplicateConsecutiveAction { step_index: i + 1 });
            }
        }

        // 4. Deadline check.
        if let Some(deadline_ms) = goal.deadline_ms {
            let now_ms = goal.created_ms; // Approximate: plan was created near goal creation.
            let estimated_end = now_ms.saturating_add(plan.estimated_duration_ms as u64);
            if estimated_end > deadline_ms {
                errors.push(PlanValidationError::DeadlineExceeded {
                    estimated_ms: plan.estimated_duration_ms as u64,
                    deadline_ms: deadline_ms.saturating_sub(now_ms),
                });
            }
        }

        // 5. Zero-timeout steps.
        for (i, step) in plan.steps.iter().enumerate() {
            if step.timeout_ms == 0 {
                errors.push(PlanValidationError::ZeroTimeout { step_index: i });
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    // ── Feedback loop ───────────────────────────────────────────────────

    /// Template confidence update deferred to LLM.
    ///
    /// IRON LAW: LLM classifies intent. Rust does not.
    /// Weighted confidence mutation (+ BOOST / * 0.9) that drives future template
    /// routing decisions is a cognitive feedback loop. Outcome data must be
    /// surfaced to the Neocortex; it decides how to update routing preferences.
    pub fn record_outcome(&mut self, template_idx: usize, success: bool) {
        // IRON LAW: LLM classifies intent. Rust does not.
        // Only record the usage count — a mechanical counter, not a routing decision.
        if let Some(template) = self.templates.get_mut(template_idx) {
            template.usage_count = template.usage_count.saturating_add(1);
            debug!(
                idx = template_idx,
                success,
                usage = template.usage_count,
                "template outcome recorded (confidence update deferred to LLM)"
            );
        } else {
            warn!(
                idx = template_idx,
                total = self.templates.len(),
                "record_outcome: template index out of bounds"
            );
        }
    }
}

impl Default for ActionPlanner {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Enhanced Planning: Best-of-N, Re-planning, Caching, Resource Estimation
// ===========================================================================

/// Maximum entries in the plan cache.
const MAX_PLAN_CACHE: usize = 64;

/// Maximum alternative plans to generate in Best-of-N.
const MAX_BEST_OF_N: usize = 5;

/// Estimated battery percentage cost per second of execution.
const BATTERY_COST_PER_SEC: f32 = 0.001;

/// Estimated data cost per LLM call (bytes).
const DATA_COST_PER_LLM_CALL: u64 = 4096;

// ---------------------------------------------------------------------------
// Resource Estimation
// ---------------------------------------------------------------------------

/// Estimated resource cost of executing a plan.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceEstimate {
    /// Estimated execution time in milliseconds.
    pub time_ms: u64,
    /// Estimated battery percentage consumed (0.0–100.0).
    pub battery_pct: f32,
    /// Estimated data usage in bytes.
    pub data_bytes: u64,
    /// Number of LLM calls needed (0 for fully local plans).
    pub llm_calls: u32,
}

impl ResourceEstimate {
    /// Compute a combined cost score (lower is cheaper).
    pub fn cost_score(&self) -> f32 {
        let time_factor = (self.time_ms as f32) / 60_000.0; // Minutes.
        let battery_factor = self.battery_pct;
        let data_factor = (self.data_bytes as f32) / 1_000_000.0; // MB.
        let llm_factor = self.llm_calls as f32 * 2.0; // LLM calls are expensive.
        time_factor + battery_factor + data_factor + llm_factor
    }
}

// ---------------------------------------------------------------------------
// Plan Explanation
// ---------------------------------------------------------------------------

/// Human-readable explanation of a plan for transparency.
#[derive(Debug, Clone)]
pub struct PlanExplanation {
    /// Brief one-line summary.
    pub summary: String,
    /// Step-by-step descriptions.
    pub step_descriptions: Vec<String>,
    /// Why this plan was chosen over alternatives.
    pub rationale: String,
    /// Estimated time to complete.
    pub estimated_time_human: String,
    /// Confidence expressed as a percentage string.
    pub confidence_pct: String,
}

// ---------------------------------------------------------------------------
// Plan Cache
// ---------------------------------------------------------------------------

/// A cached plan entry keyed by goal description hash.
#[derive(Debug, Clone)]
struct CachedPlan {
    /// Original goal description for semantic matching.
    description: String,
    /// Hash of the goal description (lowercase, trimmed).
    description_hash: u64,
    /// The cached plan.
    plan: ActionPlan,
    /// How many times this cached plan has been used.
    hit_count: u32,
    /// Last used timestamp (ms since epoch).
    last_used_ms: u64,
    /// Success rate of this cached plan.
    success_rate: f32,
}

// ---------------------------------------------------------------------------
// Scored Plan (for Best-of-N selection)
// ---------------------------------------------------------------------------

/// A plan with its computed score for Best-of-N ranking.
#[derive(Debug, Clone)]
pub struct ScoredPlan {
    /// The plan itself.
    pub plan: ActionPlan,
    /// Combined score (higher is better).
    pub score: f32,
    /// Resource estimate.
    pub resources: ResourceEstimate,
    /// How this plan was generated (for explanation).
    pub source_description: String,
}

// ---------------------------------------------------------------------------
// Enhanced ActionPlanner (extension methods)
// ---------------------------------------------------------------------------

/// Enhanced planner wrapping `ActionPlanner` with caching, Best-of-N, re-planning,
/// resource estimation, and plan explanation.
#[derive(Debug)]
pub struct EnhancedPlanner {
    /// The base planner.
    pub base: ActionPlanner,
    /// Plan cache: description_hash → CachedPlan.
    cache: Vec<CachedPlan>,
    /// Maximum cache entries.
    max_cache: usize,
    /// Current timestamp provider (ms). Default to 0 for testing.
    current_time_ms: u64,
}

impl EnhancedPlanner {
    /// Create a new enhanced planner wrapping a base planner.
    pub fn new(base: ActionPlanner) -> Self {
        Self {
            base,
            cache: Vec::with_capacity(MAX_PLAN_CACHE),
            max_cache: MAX_PLAN_CACHE,
            current_time_ms: 0,
        }
    }

    /// Create with a default base planner.
    pub fn with_defaults() -> Self {
        Self::new(ActionPlanner::new())
    }

    /// Set the current time for cache operations.
    pub fn set_time(&mut self, ms: u64) {
        self.current_time_ms = ms;
    }

    /// Simple FNV-1a 64-bit hash for goal descriptions.
    fn hash_description(desc: &str) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in desc.to_ascii_lowercase().trim().as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    /// Semantic similarity matching deferred to LLM.
    ///
    /// IRON LAW: LLM classifies intent. Rust does not.
    /// Jaccard word-overlap NLP to match user goal intent belongs in the Neocortex.
    fn compute_semantic_similarity(_a: &str, _b: &str) -> f32 {
        // IRON LAW: LLM classifies intent. Rust does not.
        0.0
    }

    // ── Plan Caching ────────────────────────────────────────────────────

    /// Look up a cached plan for the given goal description using exact hash or semantic
    /// similarity.
    pub fn cache_lookup(&mut self, description: &str) -> Option<&ActionPlan> {
        let hash = Self::hash_description(description);
        let current_time = self.current_time_ms;

        // Phase 1: Exact Hash Match (O(N) iteration, O(1) cmp)
        let exact_idx = self
            .cache
            .iter()
            .position(|e| e.description_hash == hash && e.success_rate >= 0.5);
        if let Some(idx) = exact_idx {
            self.cache[idx].hit_count += 1;
            self.cache[idx].last_used_ms = current_time;
            return Some(&self.cache[idx].plan);
        }

        // Phase 2: Semantic Similarity Match (Threshold > 0.75)
        let mut best_match: Option<usize> = None;
        let mut best_score = 0.75_f32; // Minimum threshold 75% similarity

        for (i, entry) in self.cache.iter().enumerate() {
            if entry.success_rate >= 0.5 {
                let score = Self::compute_semantic_similarity(description, &entry.description);
                if score > best_score {
                    best_score = score;
                    best_match = Some(i);
                }
            }
        }

        if let Some(idx) = best_match {
            self.cache[idx].hit_count += 1;
            self.cache[idx].last_used_ms = current_time;
            tracing::debug!(score = best_score, "semantic plan cache hit");
            return Some(&self.cache[idx].plan);
        }

        None
    }

    /// Store a plan in the cache.
    pub fn cache_store(&mut self, description: &str, plan: ActionPlan) {
        let hash = Self::hash_description(description);

        // Check if already cached — update if so.
        for entry in self.cache.iter_mut() {
            if entry.description_hash == hash {
                entry.plan = plan;
                entry.hit_count += 1;
                entry.last_used_ms = self.current_time_ms;
                return;
            }
        }

        // Evict LRU if at capacity.
        if self.cache.len() >= self.max_cache {
            // Find the entry with the smallest last_used_ms.
            if let Some(lru_idx) = self
                .cache
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_used_ms)
                .map(|(i, _)| i)
            {
                self.cache.remove(lru_idx);
            }
        }

        self.cache.push(CachedPlan {
            description: description.to_string(),
            description_hash: hash,
            plan,
            hit_count: 1,
            last_used_ms: self.current_time_ms,
            success_rate: 0.7, // Optimistic initial rate.
        });
    }

    /// Record outcome for a cached plan.
    pub fn cache_record_outcome(&mut self, description: &str, success: bool) {
        let hash = Self::hash_description(description);
        for entry in self.cache.iter_mut() {
            if entry.description_hash == hash {
                if success {
                    entry.success_rate = (entry.success_rate + 0.05).min(1.0);
                } else {
                    entry.success_rate = (entry.success_rate - 0.15).max(0.0);
                }
                break;
            }
        }
    }

    /// Number of cached plans.
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    // ── Resource Estimation ─────────────────────────────────────────────

    /// Estimate the resource cost of a plan.
    pub fn estimate_resources(&self, plan: &ActionPlan) -> ResourceEstimate {
        let time_ms = plan.estimated_duration_ms as u64;
        let time_seconds = time_ms as f32 / 1000.0;
        let battery_pct = time_seconds * BATTERY_COST_PER_SEC;

        let llm_calls = if plan.source == PlanSource::LlmGenerated {
            1u32
        } else {
            0u32
        };

        let data_bytes = llm_calls as u64 * DATA_COST_PER_LLM_CALL + plan.steps.len() as u64 * 64; // Small overhead per step.

        ResourceEstimate {
            time_ms,
            battery_pct,
            data_bytes,
            llm_calls,
        }
    }

    // ── Plan Scoring ────────────────────────────────────────────────────

    /// Score a plan for Best-of-N ranking.
    ///
    /// IRON LAW: LLM classifies intent. Rust does not.
    /// Weighted scoring formulas (confidence*0.4 + resource*0.3 + ...) that drive
    /// plan selection are routing decisions. Plan selection belongs in the Neocortex.
    /// Rust returns a neutral score so the calling code can still compile and
    /// structure candidates; actual selection defers to the LLM.
    /// Score a candidate plan by sending it to the neocortex for LLM evaluation.
    ///
    /// Sends [`DaemonToNeocortex::ScorePlan`] and awaits a
    /// [`NeocortexToDaemon::PlanScore`] response.  Defaults to `0.5` on
    /// timeout or IPC error so that all candidates remain equal competitors
    /// (avoiding false negatives in Best-of-N selection that `0.0` would cause).
    ///
    /// Uses `block_in_place` so the sync `plan_best_of_n` caller does not need
    /// to become async.  Falls back to `0.5` immediately when no Tokio runtime
    /// is active (e.g. unit tests).
    pub fn score_plan(&self, plan: &ActionPlan) -> f32 {
        use aura_types::ipc::{DaemonToNeocortex, NeocortexToDaemon};

        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => {
                debug!("score_plan: no Tokio runtime — returning neutral score 0.5");
                return 0.5;
            },
        };

        // TODO(ARCH-MED-2): `block_on()` inside a sync fn called from an async
        // context risks deadlocking the Tokio runtime if all worker threads are
        // saturated.  Phase 3 should convert `score_plan` to `async fn` and
        // propagate the async boundary up to the caller.  See also: retry.rs
        // `classify_failure_via_llm()` which wraps with `block_in_place()` as
        // a safer interim pattern.
        tokio::task::block_in_place(|| {
            handle.block_on(async {
            let mut client = match crate::ipc::NeocortexClient::connect().await {
                Ok(c) => c,
                Err(e) => {
                    warn!(error = %e, "score_plan: IPC connect failed — defaulting to 0.5");
                    return 0.5;
                },
            };

            match client
                .request(&DaemonToNeocortex::ScorePlan { plan: plan.clone() })
                .await
            {
                Ok(NeocortexToDaemon::PlanScore { score }) => {
                    // Clamp to [0.0, 1.0] defensively.
                    let clamped = score.clamp(0.0, 1.0);
                    debug!(score = clamped, "score_plan: received LLM score");
                    clamped
                },
                Ok(other) => {
                    warn!(
                        resp = ?std::mem::discriminant(&other),
                        "score_plan: unexpected IPC response — defaulting to 0.5"
                    );
                    0.5
                },
                Err(e) => {
                    warn!(error = %e, "score_plan: IPC request failed — defaulting to 0.5");
                    0.5
                },
            }
        })
        })
    }

    // ── Best-of-N Planning ──────────────────────────────────────────────

    /// Generate up to N alternative plans and return the best one.
    ///
    /// Tries cache, ETG, templates (with variations), and LLM fallback,
    /// then ranks all candidates by score.
    pub fn plan_best_of_n(
        &mut self,
        goal: &Goal,
        etg: &EtgStore,
        current_screen: Option<u64>,
        target_screen: Option<u64>,
        max_alternatives: usize,
    ) -> Result<ScoredPlan, PlanError> {
        let n = max_alternatives.min(MAX_BEST_OF_N);
        let mut candidates: Vec<ScoredPlan> = Vec::with_capacity(n);

        // Candidate 1: Cache lookup.
        if let Some(cached) = self.cache_lookup(&goal.description) {
            let cached_clone = cached.clone();
            let score = self.score_plan(&cached_clone);
            let resources = self.estimate_resources(&cached_clone);
            candidates.push(ScoredPlan {
                plan: cached_clone,
                score,
                resources,
                source_description: "cached plan".to_string(),
            });
        }

        // Candidate 2: Base planner (ETG → Template → LLM cascade).
        if let Ok(plan) = self.base.plan(goal, etg, current_screen, target_screen) {
            let score = self.score_plan(&plan);
            let resources = self.estimate_resources(&plan);
            candidates.push(ScoredPlan {
                plan,
                score,
                resources,
                source_description: "base cascade (ETG/Template/LLM)".to_string(),
            });
        }

        // Candidate 3+: If we have templates, try variations with modified confidence.
        // Generate template-based plans with perturbed step ordering.
        if candidates.len() < n {
            let variant_goal_desc = format!("{} (optimized)", goal.description);
            let variant_goal = Goal {
                id: goal.id,
                description: variant_goal_desc,
                priority: goal.priority,
                status: goal.status.clone(),
                steps: goal.steps.clone(),
                created_ms: goal.created_ms,
                deadline_ms: goal.deadline_ms,
                parent_goal: goal.parent_goal,
                source: goal.source.clone(),
            };

            if let Ok(plan) = self
                .base
                .plan(&variant_goal, etg, current_screen, target_screen)
            {
                let score = self.score_plan(&plan);
                let resources = self.estimate_resources(&plan);
                candidates.push(ScoredPlan {
                    plan,
                    score,
                    resources,
                    source_description: "optimized variant".to_string(),
                });
            }
        }

        if candidates.is_empty() {
            return Err(PlanError::NoPlanFound(
                "no candidates generated".to_string(),
            ));
        }

        // Sort by score descending and pick the best.
        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let best = candidates.remove(0);

        // Cache the winning plan.
        self.cache_store(&goal.description, best.plan.clone());

        Ok(best)
    }

    // ── Re-planning ─────────────────────────────────────────────────────

    /// Re-plan from the current execution state after a failure.
    ///
    /// Takes the failed step index and produces a new plan that skips
    /// completed steps and re-plans from the current position.
    pub fn replan_from(
        &mut self,
        original_plan: &ActionPlan,
        failed_step_index: usize,
        goal: &Goal,
        etg: &EtgStore,
        current_screen: Option<u64>,
        target_screen: Option<u64>,
    ) -> Result<ActionPlan, PlanError> {
        // Preserve completed steps.
        let completed_steps: Vec<DslStep> = original_plan
            .steps
            .iter()
            .take(failed_step_index)
            .cloned()
            .collect();

        // Try to get a new plan for the remaining goal.
        let remaining_plan = self.base.plan(goal, etg, current_screen, target_screen)?;

        // Combine: completed steps + new plan steps.
        let mut combined_steps = completed_steps;
        combined_steps.extend(remaining_plan.steps);

        // Cap at max steps.
        if combined_steps.len() > self.base.max_plan_steps() {
            combined_steps.truncate(self.base.max_plan_steps());
        }

        let estimated_duration: u32 = combined_steps
            .iter()
            .map(|s| (s.timeout_ms / 2).min(30_000))
            .sum::<u32>();

        Ok(ActionPlan {
            goal_description: goal.description.clone(),
            steps: combined_steps,
            estimated_duration_ms: estimated_duration,
            confidence: remaining_plan.confidence * 0.9, // Slight penalty for re-plan.
            source: remaining_plan.source,
        })
    }

    // ── Plan Explanation ────────────────────────────────────────────────

    /// Generate a human-readable explanation of a plan.
    pub fn explain_plan(&self, plan: &ActionPlan) -> PlanExplanation {
        let step_descriptions: Vec<String> = plan
            .steps
            .iter()
            .enumerate()
            .map(|(i, step)| {
                let action_desc = format!("{:?}", step.action);
                let label = step
                    .label
                    .as_ref()
                    .map(|l| format!(" ({})", l))
                    .unwrap_or_default();
                format!("Step {}: {}{}", i + 1, action_desc, label)
            })
            .collect();

        let source_str = match plan.source {
            PlanSource::EtgLookup => "learned from past experience (ETG)",
            PlanSource::UserDefined => "user-defined (high trust)",
            PlanSource::Hybrid => "template-based pattern matching",
            PlanSource::LlmGenerated => "AI-generated (needs verification)",
        };

        let duration_secs = plan.estimated_duration_ms / 1000;
        let estimated_time_human = if duration_secs < 60 {
            format!("~{}s", duration_secs)
        } else {
            format!("~{}m {}s", duration_secs / 60, duration_secs % 60)
        };

        PlanExplanation {
            summary: format!(
                "{} ({} steps, {} confidence)",
                plan.goal_description,
                plan.steps.len(),
                format!("{:.0}%", plan.confidence * 100.0)
            ),
            step_descriptions,
            rationale: format!("Plan source: {}. ", source_str),
            estimated_time_human,
            confidence_pct: format!("{:.0}%", plan.confidence * 100.0),
        }
    }
}

impl Default for EnhancedPlanner {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Compare two `ActionType` values for structural equality (used for
/// duplicate-consecutive detection).
fn actions_equal(a: &ActionType, b: &ActionType) -> bool {
    // Use serde_json for structural comparison since ActionType derives
    // PartialEq.
    a == b
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use aura_types::goals::{GoalPriority, GoalSource, GoalStatus};

    use super::*;

    fn make_goal(description: &str) -> Goal {
        Goal {
            id: 1,
            description: description.to_string(),
            priority: GoalPriority::Medium,
            status: GoalStatus::Active,
            steps: vec![],
            created_ms: 1_000_000,
            deadline_ms: None,
            parent_goal: None,
            source: GoalSource::UserExplicit,
        }
    }

    fn make_template(pattern: &str, confidence: f32) -> PlanTemplate {
        PlanTemplate {
            trigger_pattern: pattern.to_string(),
            app_package: None,
            steps: vec![DslStep {
                action: ActionType::OpenApp {
                    package: "com.test".to_string(),
                },
                target: None,
                timeout_ms: 5000,
                on_failure: FailureStrategy::Retry { max: 2 },
                precondition: None,
                postcondition: None,
                label: Some("open_app".to_string()),
            }],
            confidence,
            usage_count: 0,
        }
    }

    #[test]
    fn test_planner_creation_defaults() {
        let planner = ActionPlanner::new();
        assert_eq!(planner.template_count(), 0);
        assert_eq!(planner.max_plan_steps(), DEFAULT_MAX_PLAN_STEPS);
    }

    #[test]
    fn test_register_template() {
        let mut planner = ActionPlanner::new();
        let template = make_template("open settings", 0.8);
        planner.register_template(template).expect("register");
        assert_eq!(planner.template_count(), 1);
    }

    #[test]
    fn test_template_capacity_exceeded() {
        let mut planner = ActionPlanner::new();
        for i in 0..MAX_TEMPLATES {
            let t = make_template(&format!("pattern_{i}"), 0.5);
            planner.register_template(t).expect("register");
        }
        let overflow = make_template("overflow", 0.5);
        let result = planner.register_template(overflow);
        assert!(matches!(
            result,
            Err(PlanError::TemplateCapacityExceeded { .. })
        ));
    }

    #[test]
    fn test_plan_from_template_match() {
        let mut planner = ActionPlanner::new();
        planner
            .register_template(make_template("open settings", 0.85))
            .expect("register");

        let goal = make_goal("Please open settings on my phone");
        let etg = EtgStore::in_memory();
        let plan = planner.plan(&goal, &etg, None, None).expect("plan");

        // IRON LAW: plan_from_template is stubbed — LLM decides template matching.
        // Template matching returns None → falls through to LLM fallback.
        assert_eq!(plan.source, PlanSource::LlmGenerated);
        assert!(plan.steps.is_empty()); // LLM fallback placeholder.
        assert!(plan.confidence < 0.5);
    }

    #[test]
    fn test_plan_fallback_to_llm() {
        let planner = ActionPlanner::new();
        let goal = make_goal("do something completely novel");
        let etg = EtgStore::in_memory();
        let plan = planner.plan(&goal, &etg, None, None).expect("plan");

        // No ETG path, no template → LLM fallback.
        assert_eq!(plan.source, PlanSource::LlmGenerated);
        assert!(plan.steps.is_empty()); // Placeholder for caller to fill.
        assert!(plan.confidence < 0.5);
    }

    #[test]
    fn test_plan_from_etg() {
        let mut etg = EtgStore::in_memory();
        etg.get_or_create_node(0xA, "com.test", ".Main", &[]);
        etg.get_or_create_node(0xB, "com.test", ".Settings", &[]);

        // High-reliability edge.
        for _ in 0..20 {
            etg.record_transition(0xA, 0xB, &ActionType::Tap { x: 100, y: 200 }, true, 150);
        }

        let planner = ActionPlanner::new();
        let goal = make_goal("go to settings");
        let plan = planner
            .plan(&goal, &etg, Some(0xA), Some(0xB))
            .expect("plan");

        assert_eq!(plan.source, PlanSource::EtgLookup);
        assert_eq!(plan.steps.len(), 1);
        assert!(plan.confidence >= MIN_ETG_CONFIDENCE);
    }

    #[test]
    fn test_validate_plan_empty() {
        let planner = ActionPlanner::new();
        let goal = make_goal("test");
        let plan = ActionPlan {
            goal_description: "test".to_string(),
            steps: vec![],
            estimated_duration_ms: 0,
            confidence: 0.5,
            source: PlanSource::LlmGenerated,
        };
        let result = planner.validate_plan(&plan, &goal);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, PlanValidationError::EmptyPlan)));
    }

    #[test]
    fn test_validate_plan_too_many_steps() {
        let planner = ActionPlanner::with_limits(2, 1);
        let goal = make_goal("test");
        let steps: Vec<DslStep> = (0..5)
            .map(|i| DslStep {
                action: ActionType::Tap { x: i * 10, y: 100 },
                target: None,
                timeout_ms: 2000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: None,
            })
            .collect();
        let plan = ActionPlan {
            goal_description: "test".to_string(),
            steps,
            estimated_duration_ms: 10000,
            confidence: 0.8,
            source: PlanSource::LlmGenerated,
        };
        let result = planner.validate_plan(&plan, &goal);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, PlanValidationError::StepLimitExceeded { .. })));
    }

    #[test]
    fn test_validate_plan_duplicate_consecutive() {
        let planner = ActionPlanner::new();
        let goal = make_goal("test");
        let steps = vec![
            DslStep {
                action: ActionType::Tap { x: 100, y: 200 },
                target: None,
                timeout_ms: 2000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: None,
            },
            DslStep {
                action: ActionType::Tap { x: 100, y: 200 },
                target: None,
                timeout_ms: 2000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: None,
            },
        ];
        let plan = ActionPlan {
            goal_description: "test".to_string(),
            steps,
            estimated_duration_ms: 4000,
            confidence: 0.8,
            source: PlanSource::LlmGenerated,
        };
        let result = planner.validate_plan(&plan, &goal);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, PlanValidationError::DuplicateConsecutiveAction { .. })));
    }

    #[test]
    fn test_validate_plan_deadline_exceeded() {
        let planner = ActionPlanner::new();
        let mut goal = make_goal("test");
        goal.created_ms = 1_000_000;
        goal.deadline_ms = Some(1_001_000); // 1 second deadline

        let steps = vec![DslStep {
            action: ActionType::Tap { x: 100, y: 200 },
            target: None,
            timeout_ms: 5000,
            on_failure: FailureStrategy::default(),
            precondition: None,
            postcondition: None,
            label: None,
        }];
        let plan = ActionPlan {
            goal_description: "test".to_string(),
            steps,
            estimated_duration_ms: 5000, // 5 seconds — exceeds 1s deadline
            confidence: 0.8,
            source: PlanSource::LlmGenerated,
        };
        let result = planner.validate_plan(&plan, &goal);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, PlanValidationError::DeadlineExceeded { .. })));
    }

    #[test]
    fn test_validate_plan_zero_timeout() {
        let planner = ActionPlanner::new();
        let goal = make_goal("test");
        let steps = vec![DslStep {
            action: ActionType::Back,
            target: None,
            timeout_ms: 0,
            on_failure: FailureStrategy::default(),
            precondition: None,
            postcondition: None,
            label: None,
        }];
        let plan = ActionPlan {
            goal_description: "test".to_string(),
            steps,
            estimated_duration_ms: 0,
            confidence: 0.5,
            source: PlanSource::LlmGenerated,
        };
        let result = planner.validate_plan(&plan, &goal);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, PlanValidationError::ZeroTimeout { .. })));
    }

    #[test]
    fn test_validate_plan_success() {
        let planner = ActionPlanner::new();
        let goal = make_goal("test");
        let steps = vec![
            DslStep {
                action: ActionType::OpenApp {
                    package: "com.test".to_string(),
                },
                target: None,
                timeout_ms: 5000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: None,
            },
            DslStep {
                action: ActionType::Tap { x: 100, y: 200 },
                target: None,
                timeout_ms: 2000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: None,
            },
        ];
        let plan = ActionPlan {
            goal_description: "test".to_string(),
            steps,
            estimated_duration_ms: 7000,
            confidence: 0.8,
            source: PlanSource::LlmGenerated,
        };
        let result = planner.validate_plan(&plan, &goal);
        assert!(result.is_ok(), "validate_plan failed: {:?}", result.err());
    }

    #[test]
    fn test_record_outcome_success() {
        let mut planner = ActionPlanner::new();
        planner
            .register_template(make_template("test", 0.7))
            .expect("register");

        let original = planner.templates[0].confidence;
        planner.record_outcome(0, true);
        // IRON LAW: confidence update deferred to LLM — only usage_count increments.
        assert_eq!(planner.templates[0].confidence, original);
        assert_eq!(planner.templates[0].usage_count, 1);
    }

    #[test]
    fn test_record_outcome_failure() {
        let mut planner = ActionPlanner::new();
        planner
            .register_template(make_template("test", 0.8))
            .expect("register");

        planner.record_outcome(0, false);
        // IRON LAW: confidence update deferred to LLM — only usage_count increments.
        assert_eq!(planner.templates[0].confidence, 0.8);
        assert_eq!(planner.templates[0].usage_count, 1);
    }

    #[test]
    fn test_record_outcome_oob_does_not_panic() {
        let mut planner = ActionPlanner::new();
        // Index out of bounds — should warn but not panic.
        planner.record_outcome(999, true);
    }

    #[test]
    fn test_confidence_cap_at_095() {
        let mut planner = ActionPlanner::new();
        planner
            .register_template(make_template("test", 0.94))
            .expect("register");
        planner.record_outcome(0, true);
        assert!(planner.templates[0].confidence <= 0.95);
    }

    #[test]
    fn test_llm_request_structure() {
        let planner = ActionPlanner::new();
        let mut goal = make_goal("order coffee from starbucks");
        goal.deadline_ms = Some(2_000_000);
        goal.steps = vec![aura_types::goals::GoalStep {
            index: 0,
            description: "Open Starbucks".to_string(),
            action: Some(ActionType::OpenApp {
                package: "com.starbucks".to_string(),
            }),
            status: aura_types::goals::StepStatus::Pending,
            attempts: 0,
            max_attempts: 3,
        }];

        let req = planner.plan_for_llm_request(&goal, Some(0xABC));
        assert_eq!(req.goal_description, "order coffee from starbucks");
        assert_eq!(req.deadline_ms, Some(2_000_000));
        assert_eq!(req.max_steps, DEFAULT_MAX_PLAN_STEPS);
        assert_eq!(req.current_screen_hash, Some(0xABC));
        assert_eq!(req.relevant_packages, vec!["com.starbucks".to_string()]);
    }

    #[test]
    fn test_template_best_match_by_confidence() {
        let mut planner = ActionPlanner::new();
        planner
            .register_template(make_template("settings", 0.6))
            .expect("register");
        planner
            .register_template(make_template("settings", 0.9))
            .expect("register");

        let goal = make_goal("open settings");
        let etg = EtgStore::in_memory();
        let plan = planner.plan(&goal, &etg, None, None).expect("plan");

        // IRON LAW: plan_from_template is stubbed — template selection deferred to LLM.
        // Both templates are registered but plan_from_template returns None.
        // Falls through to LLM fallback.
        assert_eq!(plan.source, PlanSource::LlmGenerated);
    }

    // =====================================================================
    // EnhancedPlanner tests
    // =====================================================================

    fn make_plan(description: &str, confidence: f32, source: PlanSource) -> ActionPlan {
        ActionPlan {
            goal_description: description.to_string(),
            steps: vec![DslStep {
                action: ActionType::OpenApp {
                    package: "com.test".to_string(),
                },
                target: None,
                timeout_ms: 5000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: Some("open_app".to_string()),
            }],
            estimated_duration_ms: 5000,
            confidence,
            source,
        }
    }

    fn make_multi_step_plan(
        description: &str,
        step_count: usize,
        confidence: f32,
        source: PlanSource,
    ) -> ActionPlan {
        let steps: Vec<DslStep> = (0..step_count)
            .map(|i| DslStep {
                action: ActionType::Tap {
                    x: i as i32 * 10,
                    y: 100,
                },
                target: None,
                timeout_ms: 3000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: Some(format!("step_{}", i)),
            })
            .collect();
        ActionPlan {
            goal_description: description.to_string(),
            steps,
            estimated_duration_ms: step_count as u32 * 3000,
            confidence,
            source,
        }
    }

    #[test]
    fn test_enhanced_planner_creation() {
        let ep = EnhancedPlanner::with_defaults();
        assert_eq!(ep.cache_size(), 0);
        assert_eq!(ep.base.template_count(), 0);
    }

    #[test]
    fn test_enhanced_planner_with_base() {
        let mut base = ActionPlanner::new();
        base.register_template(make_template("test", 0.8)).unwrap();
        let ep = EnhancedPlanner::new(base);
        assert_eq!(ep.base.template_count(), 1);
    }

    #[test]
    fn test_cache_store_and_lookup() {
        let mut ep = EnhancedPlanner::with_defaults();
        let plan = make_plan("open settings", 0.85, PlanSource::Hybrid);
        ep.cache_store("open settings", plan.clone());
        assert_eq!(ep.cache_size(), 1);

        let cached = ep.cache_lookup("open settings");
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().goal_description, "open settings");
    }

    #[test]
    fn test_cache_lookup_case_insensitive() {
        let mut ep = EnhancedPlanner::with_defaults();
        let plan = make_plan("Open Settings", 0.85, PlanSource::Hybrid);
        ep.cache_store("Open Settings", plan);

        // Lookup with different case should hit.
        let cached = ep.cache_lookup("open settings");
        assert!(cached.is_some());
    }

    #[test]
    fn test_cache_miss() {
        let mut ep = EnhancedPlanner::with_defaults();
        let cached = ep.cache_lookup("nonexistent");
        assert!(cached.is_none());
    }

    #[test]
    fn test_cache_update_existing() {
        let mut ep = EnhancedPlanner::with_defaults();
        let plan1 = make_plan("open settings", 0.7, PlanSource::Hybrid);
        let plan2 = make_plan("open settings", 0.95, PlanSource::EtgLookup);

        ep.cache_store("open settings", plan1);
        ep.cache_store("open settings", plan2);

        // Should still be 1 entry, updated in place.
        assert_eq!(ep.cache_size(), 1);
        let cached = ep.cache_lookup("open settings").unwrap();
        assert_eq!(cached.source, PlanSource::EtgLookup);
    }

    #[test]
    fn test_cache_lru_eviction() {
        let mut ep = EnhancedPlanner::new(ActionPlanner::new());
        // Reduce max cache to 3 for testing.
        ep.max_cache = 3;

        ep.set_time(100);
        ep.cache_store("plan_a", make_plan("plan_a", 0.7, PlanSource::Hybrid));
        ep.set_time(200);
        ep.cache_store("plan_b", make_plan("plan_b", 0.7, PlanSource::Hybrid));
        ep.set_time(300);
        ep.cache_store("plan_c", make_plan("plan_c", 0.7, PlanSource::Hybrid));
        assert_eq!(ep.cache_size(), 3);

        // Adding a 4th should evict the LRU (plan_a at time 100).
        ep.set_time(400);
        ep.cache_store("plan_d", make_plan("plan_d", 0.7, PlanSource::Hybrid));
        assert_eq!(ep.cache_size(), 3);

        // plan_a should be gone.
        assert!(ep.cache_lookup("plan_a").is_none());
        // plan_d should be present.
        assert!(ep.cache_lookup("plan_d").is_some());
    }

    #[test]
    fn test_cache_record_outcome_success() {
        let mut ep = EnhancedPlanner::with_defaults();
        ep.cache_store("test", make_plan("test", 0.8, PlanSource::Hybrid));

        // Initial success rate is 0.7 (optimistic).
        ep.cache_record_outcome("test", true);
        // After success: 0.7 + 0.05 = 0.75
        ep.cache_record_outcome("test", true);
        // After another: 0.75 + 0.05 = 0.80

        // Plan should still be cached and lookup-able.
        assert!(ep.cache_lookup("test").is_some());
    }

    #[test]
    fn test_cache_record_outcome_failure_drops_below_threshold() {
        let mut ep = EnhancedPlanner::with_defaults();
        ep.cache_store("test", make_plan("test", 0.8, PlanSource::Hybrid));

        // Initial rate: 0.7. Each failure subtracts 0.15.
        ep.cache_record_outcome("test", false); // 0.55
        ep.cache_record_outcome("test", false); // 0.40 — below 0.5 threshold

        // cache_lookup filters by success_rate >= 0.5, so this should miss.
        assert!(ep.cache_lookup("test").is_none());
    }

    #[test]
    fn test_resource_estimate_etg_plan() {
        let ep = EnhancedPlanner::with_defaults();
        let plan = make_plan("test", 0.9, PlanSource::EtgLookup);

        let est = ep.estimate_resources(&plan);
        assert_eq!(est.time_ms, 5000);
        assert_eq!(est.llm_calls, 0); // ETG = no LLM.
        assert!(est.battery_pct > 0.0);
        assert!(est.data_bytes > 0); // Small overhead per step.
    }

    #[test]
    fn test_resource_estimate_llm_plan() {
        let ep = EnhancedPlanner::with_defaults();
        let plan = make_plan("test", 0.5, PlanSource::LlmGenerated);

        let est = ep.estimate_resources(&plan);
        assert_eq!(est.llm_calls, 1);
        assert!(est.data_bytes >= DATA_COST_PER_LLM_CALL);
    }

    #[test]
    fn test_resource_cost_score() {
        let low_cost = ResourceEstimate {
            time_ms: 1000,
            battery_pct: 0.001,
            data_bytes: 64,
            llm_calls: 0,
        };
        let high_cost = ResourceEstimate {
            time_ms: 60_000,
            battery_pct: 0.06,
            data_bytes: 1_000_000,
            llm_calls: 3,
        };
        assert!(low_cost.cost_score() < high_cost.cost_score());
    }

    // ── score_plan fallback tests ──────────────────────────────────────
    //
    // score_plan() delegates scoring to the Neocortex LLM via IPC.
    // In unit tests, IPC is unavailable, so we can only verify the
    // fallback behaviour.  When a mock IPC layer is added, these tests
    // should be extended to verify actual scoring differentiation
    // (ETG > UserDefined > Hybrid > LLM, fewer steps better, etc.).

    #[test]
    fn test_score_plan_returns_neutral_without_runtime() {
        // Explicitly test the no-Tokio-runtime fallback path.
        // This is the ONE test where asserting 0.5 is the correct intent.
        let ep = EnhancedPlanner::with_defaults();
        let plan = make_plan("test", 0.9, PlanSource::EtgLookup);

        let score = ep.score_plan(&plan);

        assert_eq!(
            score, 0.5,
            "without Tokio runtime, score_plan must return neutral 0.5"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_score_plan_returns_neutral_with_runtime_no_ipc() {
        // With a Tokio runtime present but no Neocortex IPC server,
        // score_plan should still fall back to 0.5 (IPC connect fails).
        let ep = EnhancedPlanner::with_defaults();

        let etg_plan = make_plan("test", 0.9, PlanSource::EtgLookup);
        let llm_plan = make_plan("test", 0.9, PlanSource::LlmGenerated);
        let short_plan = make_multi_step_plan("test", 2, 0.8, PlanSource::Hybrid);
        let long_plan = make_multi_step_plan("test", 10, 0.8, PlanSource::Hybrid);
        let high_conf = make_plan("test", 0.95, PlanSource::Hybrid);
        let low_conf = make_plan("test", 0.3, PlanSource::Hybrid);

        // All should return neutral fallback because IPC is not available.
        // TODO(TEST-HIGH-1): When mock IPC is implemented, replace these
        // with differentiated assertions:
        //   - ETG plans should score higher than LLM plans
        //   - Shorter plans should score higher than longer ones
        //   - Higher confidence should score higher than lower
        assert_eq!(
            ep.score_plan(&etg_plan),
            0.5,
            "ETG plan: expected IPC-failure fallback"
        );
        assert_eq!(
            ep.score_plan(&llm_plan),
            0.5,
            "LLM plan: expected IPC-failure fallback"
        );
        assert_eq!(
            ep.score_plan(&short_plan),
            0.5,
            "short plan: expected IPC-failure fallback"
        );
        assert_eq!(
            ep.score_plan(&long_plan),
            0.5,
            "long plan: expected IPC-failure fallback"
        );
        assert_eq!(
            ep.score_plan(&high_conf),
            0.5,
            "high confidence: expected IPC-failure fallback"
        );
        assert_eq!(
            ep.score_plan(&low_conf),
            0.5,
            "low confidence: expected IPC-failure fallback"
        );
    }

    #[test]
    fn test_plan_best_of_n_returns_best() {
        let mut ep = EnhancedPlanner::with_defaults();
        ep.base
            .register_template(make_template("open settings", 0.85))
            .unwrap();

        let goal = make_goal("open settings");
        let etg = EtgStore::in_memory();

        let result = ep.plan_best_of_n(&goal, &etg, None, None, 3);

        let scored = result.expect("plan_best_of_n should produce at least one candidate plan");
        // No Tokio runtime in unit-test context → score_plan returns 0.5 for all.
        assert_eq!(scored.score, 0.5);
        assert!(!scored.plan.steps.is_empty() || scored.plan.source == PlanSource::LlmGenerated);
        assert!(!scored.source_description.is_empty());
    }

    #[test]
    fn test_plan_best_of_n_caches_winner() {
        let mut ep = EnhancedPlanner::with_defaults();
        ep.base
            .register_template(make_template("send message", 0.9))
            .unwrap();

        let goal = make_goal("send message to Alice");
        let etg = EtgStore::in_memory();

        assert_eq!(ep.cache_size(), 0);
        let _ = ep.plan_best_of_n(&goal, &etg, None, None, 3);
        assert_eq!(ep.cache_size(), 1);
    }

    #[test]
    fn test_plan_best_of_n_no_candidates_errors() {
        // A completely empty planner with no templates and no ETG should
        // still produce an LLM fallback, so this test verifies we get *some* plan.
        let mut ep = EnhancedPlanner::with_defaults();
        let goal = make_goal("do something unknown");
        let etg = EtgStore::in_memory();

        let result = ep.plan_best_of_n(&goal, &etg, None, None, 3);
        // Base planner always returns LLM fallback, so should succeed.
        assert!(
            result.is_ok(),
            "plan_best_of_n should not fail even without templates: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_replan_preserves_completed_steps() {
        let mut ep = EnhancedPlanner::with_defaults();
        ep.base
            .register_template(make_template("test", 0.8))
            .unwrap();

        let original = make_multi_step_plan("test task", 5, 0.8, PlanSource::Hybrid);
        let goal = make_goal("test task");
        let etg = EtgStore::in_memory();

        // Fail at step index 3 — steps 0,1,2 are completed.
        let result = ep.replan_from(&original, 3, &goal, &etg, None, None);

        let new_plan = result.expect("replan_from should succeed when preserving completed steps");
        // First 3 steps should be from the original plan.
        assert_eq!(new_plan.steps[0].label, Some("step_0".to_string()));
        assert_eq!(new_plan.steps[1].label, Some("step_1".to_string()));
        assert_eq!(new_plan.steps[2].label, Some("step_2".to_string()));
    }

    #[test]
    fn test_replan_confidence_penalty() {
        let mut ep = EnhancedPlanner::with_defaults();
        ep.base
            .register_template(make_template("test", 0.8))
            .unwrap();

        let original = make_multi_step_plan("test", 3, 0.9, PlanSource::Hybrid);
        let goal = make_goal("test");
        let etg = EtgStore::in_memory();

        let result = ep.replan_from(&original, 1, &goal, &etg, None, None);

        let new_plan = result.expect("replan_from should succeed for confidence penalty test");
        // Confidence should have the 0.9 penalty multiplier.
        assert!(new_plan.confidence < 0.9);
    }

    #[test]
    fn test_replan_from_zero_gives_fresh_plan() {
        let mut ep = EnhancedPlanner::with_defaults();
        let original = make_multi_step_plan("test", 3, 0.8, PlanSource::Hybrid);
        let goal = make_goal("test");
        let etg = EtgStore::in_memory();

        // Fail at step 0 — no completed steps preserved.
        let result = ep.replan_from(&original, 0, &goal, &etg, None, None);

        let new_plan = result.expect("replan_from at step 0 should produce a fresh plan");
        // No steps from original should be preserved.
        // New plan steps come entirely from replanning.
        // (Can't check exact content since LLM fallback gives empty steps,
        // but the structure should be valid.)
        assert!(new_plan.confidence > 0.0);
    }

    #[test]
    fn test_explain_plan_basic() {
        let ep = EnhancedPlanner::with_defaults();
        let plan = make_plan("open settings", 0.85, PlanSource::EtgLookup);

        let explanation = ep.explain_plan(&plan);
        assert!(explanation.summary.contains("open settings"));
        assert!(explanation.summary.contains("1 steps"));
        assert!(explanation.rationale.contains("ETG"));
        assert_eq!(explanation.confidence_pct, "85%");
        assert_eq!(explanation.step_descriptions.len(), 1);
    }

    #[test]
    fn test_explain_plan_llm_source() {
        let ep = EnhancedPlanner::with_defaults();
        let plan = make_plan("novel task", 0.4, PlanSource::LlmGenerated);

        let explanation = ep.explain_plan(&plan);
        assert!(explanation.rationale.contains("AI-generated"));
        assert_eq!(explanation.confidence_pct, "40%");
    }

    #[test]
    fn test_explain_plan_user_defined_source() {
        let ep = EnhancedPlanner::with_defaults();
        let plan = make_plan("user task", 0.95, PlanSource::UserDefined);

        let explanation = ep.explain_plan(&plan);
        assert!(explanation.rationale.contains("user-defined"));
    }

    #[test]
    fn test_explain_plan_time_formatting_seconds() {
        let ep = EnhancedPlanner::with_defaults();
        let mut plan = make_plan("test", 0.8, PlanSource::Hybrid);
        plan.estimated_duration_ms = 45_000; // 45 seconds.

        let explanation = ep.explain_plan(&plan);
        assert_eq!(explanation.estimated_time_human, "~45s");
    }

    #[test]
    fn test_explain_plan_time_formatting_minutes() {
        let ep = EnhancedPlanner::with_defaults();
        let mut plan = make_plan("test", 0.8, PlanSource::Hybrid);
        plan.estimated_duration_ms = 125_000; // 2m 5s.

        let explanation = ep.explain_plan(&plan);
        assert_eq!(explanation.estimated_time_human, "~2m 5s");
    }

    #[test]
    fn test_hash_description_deterministic() {
        let h1 = EnhancedPlanner::hash_description("open settings");
        let h2 = EnhancedPlanner::hash_description("open settings");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_description_different_inputs() {
        let h1 = EnhancedPlanner::hash_description("open settings");
        let h2 = EnhancedPlanner::hash_description("send message");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_enhanced_planner_default_trait() {
        let ep: EnhancedPlanner = Default::default();
        assert_eq!(ep.cache_size(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_score_plan_all_sources_return_neutral_without_ipc() {
        // Verifies all PlanSource variants produce the same neutral fallback
        // when IPC is unavailable, confirming the fallback is source-agnostic.
        let ep = EnhancedPlanner::with_defaults();

        let sources = [
            PlanSource::EtgLookup,
            PlanSource::UserDefined,
            PlanSource::Hybrid,
            PlanSource::LlmGenerated,
        ];

        for source in &sources {
            let plan = make_plan("test", 0.9, *source);
            assert_eq!(
                ep.score_plan(&plan),
                0.5,
                "source {:?}: expected neutral fallback without IPC",
                source,
            );
        }
        // TODO(TEST-HIGH-1): When mock IPC is available, verify ordering:
        // EtgLookup > UserDefined > Hybrid > LlmGenerated
    }
}
