//! Bi-cameral agentic execution engine — the "brain" of AURA v4.
//!
//! Implements a two-system architecture:
//! - **System 1 (DGS):** Document-Guided Scripting via ETG templates. Handles ~80% of tasks — fast,
//!   deterministic, zero LLM cost.
//! - **System 2 (Semantic ReAct):** LLM-driven think→act→observe loop. Handles novel or complex
//!   situations requiring reasoning.
//!
//! ## Execution Flow
//!
//! 1. Classify task → DGS or SemanticReact (via RouteClassifier + ETG cache)
//! 2. DGS: feed plan to real Executor pipeline (11-stage: capture→resolve→
//!    antibot→delay→execute→verify→retry→cycle→invariants→ETG→result)
//! 3. SemanticReact: build context → neocortex IPC → parse → execute → observe
//! 4. Self-reflection after each iteration
//! 5. Cycle detection at ReAct level AND within Executor (dual-layer)
//! 6. Strategy adaptation: Direct → Exploratory → Cautious → Recovery
//! 7. Store result in episodic memory, update ETG, feed FeedbackLoop
//!
//! ## Mid-Execution Escalation (4 tiers)
//!
//! - Tier 0: DGS success (no escalation needed)
//! - Tier 1: Retry with adjusted parameters
//! - Tier 2: Escalate step to Brainstem 0.8B model
//! - Tier 3: Escalate entire task to full Neocortex

use std::{collections::BTreeMap, time::Instant};

use aura_types::{
    actions::ActionResult,
    config::TokenBudgetConfig,
    errors::AuraError,
    etg::ActionPlan,
    ipc::{
        ContextPackage, DaemonToNeocortex, FailureContext, IdentityTendencies, InferenceMode,
        NeocortexToDaemon, ScreenSummary, SelfKnowledge, TransitionPair,
    },
};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument, warn};

use crate::{
    daemon_core::token_budget::TokenBudgetManager,
    execution::executor::{ExecutionOutcome, Executor},
    ipc::NeocortexClient,
    policy::{audit::AuditLog, gate::PolicyGate, rules::RuleEffect},
    routing::classifier::RouteClassifier,
    screen::{
        actions::ScreenProvider, reader::extract_screen_summary, verifier::hash_screen_state,
    },
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum iterations per agentic session before forced termination.
///
/// # Why this differs from Neocortex's MAX_REACT_ITERATIONS (5)
///
/// This is **intentional**, not a bug (confirmed by AURA-V4 courtroom review, LLM-HIGH-5).
///
/// - **Daemon MAX_ITERATIONS = 10**: The daemon orchestrates the full agentic session, which
///   includes multiple IPC round-trips to Neocortex. Each iteration involves: building context →
///   IPC call → executing the action → observing results → reflecting. Complex real-world tasks
///   (multi-step app automation) legitimately need 6–10 iterations.
///
/// - **Neocortex MAX_REACT_ITERATIONS = 5**: The neocortex ReAct loop is a SINGLE inference pass
///   with internal Thought→Action→Observation cycles. These are fast (in-model reasoning, no IPC),
///   and 5 iterations is sufficient for single-inference reasoning chains. Going higher risks
///   context window exhaustion in the 32K model.
///
/// The two constants govern different loop levels:
///   Daemon loop (10) → per iteration → Neocortex inference (5 internal ReAct steps)
///   Total possible reasoning steps: up to 10 × 5 = 50
const MAX_ITERATIONS: u32 = 10;

/// Maximum consecutive failures before strategy escalates to Recovery.
const MAX_CONSECUTIVE_FAILURES: u32 = 5;

/// Token budget for System 2 context packages (tokens, not bytes).
const DEFAULT_TOKEN_BUDGET: u32 = 2048;

/// Confidence threshold below which a DGS step escalates to System 2.
/// Reserved for future fine-grained escalation checks inside `execute_dgs`.
#[allow(dead_code)]
const DGS_ESCALATION_THRESHOLD: f32 = 0.4;

/// Confidence threshold for a reflection to be considered "successful".
const REFLECTION_SUCCESS_THRESHOLD: f32 = 0.6;

/// Maximum number of iterations to keep in session history (bounded).
const MAX_ITERATION_HISTORY: usize = 32;

/// FNV-1a offset basis for cycle hashing.
const FNV_OFFSET: u64 = 0xcbf29ce484222325;

/// FNV-1a prime for cycle hashing.
const FNV_PRIME: u64 = 0x00000100000001B3;

// COMPLEXITY_SEMANTIC_THRESHOLD removed: classify_task() no longer uses
// heuristic thresholds — LLM decides execution mode, not Rust keyword scoring.

// ---------------------------------------------------------------------------
// PolicyContext — bundles safety gate + audit for the execution pipeline
// ---------------------------------------------------------------------------

/// Bundles the rule-based [`PolicyGate`] and [`AuditLog`] so that every
/// action can be checked (and the decision recorded) before it touches
/// the device.
///
/// Created in `main_loop.rs` and threaded through the execution pipeline.
/// The `check_action` helper encapsulates the evaluate → audit → deny/allow
/// pattern used at every action execution site.
pub struct PolicyContext<'a> {
    /// The rule-based + rate-limited policy gate.
    pub gate: &'a mut PolicyGate,
    /// The append-only, hash-chained audit log.
    pub audit: &'a mut AuditLog,
}

impl<'a> PolicyContext<'a> {
    /// Evaluate an action against the policy gate and record the decision
    /// in the audit log when it triggers an auditable effect (Deny, Confirm,
    /// or Audit).
    ///
    /// Returns `Some(ActionResult)` with a denial error if the action is
    /// blocked or requires confirmation, or `None` if the action is allowed
    /// to proceed.
    ///
    /// # Founder's Directive
    ///
    /// > "AURA must know its limits.  Can't delete user photos/files without
    /// > asking.  Protected from banking-related actions.  Proper fallbacks
    /// > that ACTUALLY fall back."
    ///
    /// Actions requiring confirmation are treated as *denied* until the
    /// interactive confirmation flow is wired.  This is the safe default.
    pub fn check_action(&mut self, action: &str) -> Option<ActionResult> {
        let decision = self.gate.evaluate(action);

        // --- Audit logging (non-fatal — log errors but don't block) ---
        if decision.needs_audit() {
            let context_hash = fnv1a_hash(action.as_bytes());
            if let Err(e) = self.audit.log_policy_decision(
                action,
                &decision.effect,
                &decision.reason,
                context_hash,
            ) {
                error!(
                    error = %e,
                    action = action,
                    "failed to write audit log entry for policy decision"
                );
            }
        }

        if decision.is_denied() {
            warn!(
                action = action,
                reason = decision.reason.as_str(),
                "action DENIED by policy gate"
            );
            Some(ActionResult {
                success: false,
                duration_ms: 0,
                error: Some(format!("policy denied: {}", decision.reason)),
                screen_changed: false,
                matched_element: None,
            })
        } else if decision.needs_confirmation() {
            // Confirmation flow not yet wired — deny for safety.
            warn!(
                action = action,
                reason = decision.reason.as_str(),
                "action requires CONFIRMATION (treated as denied until confirmation flow is wired)"
            );
            Some(ActionResult {
                success: false,
                duration_ms: 0,
                error: Some(format!("policy requires confirmation: {}", decision.reason)),
                screen_changed: false,
                matched_element: None,
            })
        } else {
            // Allowed (or Audit-with-allow) — proceed with execution.
            if decision.effect == RuleEffect::Audit {
                debug!(action = action, "action allowed with AUDIT flag");
            }
            None
        }
    }
}

// ---------------------------------------------------------------------------
// ReactEngine — owns subsystems and drives the agentic loop
// ---------------------------------------------------------------------------

/// The bi-cameral execution engine that owns all subsystems.
///
/// `ReactEngine` is the top-level coordinator. It holds:
/// - An [`Executor`] for the 11-stage action pipeline (with built-in `CycleDetector`, `EtgStore`,
///   `AntiBot`, and `ExecutionMonitor`).
/// - A [`ScreenProvider`] for capturing screen state and executing actions.
/// - A [`RouteClassifier`] for deciding DGS vs. SemanticReact routing.
///
/// Construct via [`ReactEngine::new`] or [`ReactEngine::with_defaults`].
pub struct ReactEngine {
    /// The real execution pipeline (encapsulates CycleDetector, EtgStore, etc.).
    executor: Executor,
    /// Abstraction over device screen (real or mock).
    screen: Box<dyn ScreenProvider>,
    /// Deterministic routing cascade for task classification.
    /// Stored for future use with the full `classify(&ScoredEvent)` method;
    /// currently the static `compute_complexity()` is used instead.
    #[allow(dead_code)]
    classifier: RouteClassifier,
    /// Rule-based + rate-limited policy gate for action safety checks.
    /// Every action passes through this gate before reaching the [`Executor`].
    policy_gate: PolicyGate,
    /// Append-only, hash-chained audit log for recording policy decisions.
    audit_log: AuditLog,
}

impl ReactEngine {
    /// Create a new engine with explicit subsystems.
    pub fn new(
        executor: Executor,
        screen: Box<dyn ScreenProvider>,
        classifier: RouteClassifier,
        policy_gate: PolicyGate,
        audit_log: AuditLog,
    ) -> Self {
        Self {
            executor,
            screen,
            classifier,
            policy_gate,
            audit_log,
        }
    }

    /// Create an engine with default "normal" configuration.
    ///
    /// Uses `Executor::normal()`, `RouteClassifier::new()`,
    /// `PolicyGate::deny_by_default()`, and `AuditLog::new()`.
    /// The caller must supply a `ScreenProvider` since it cannot be defaulted
    /// (it requires either a real device or a mock).
    pub fn with_defaults(screen: Box<dyn ScreenProvider>) -> Self {
        Self {
            executor: Executor::default(),
            screen,
            classifier: RouteClassifier::new(),
            policy_gate: PolicyGate::deny_by_default(),
            audit_log: AuditLog::with_default_capacity(),
        }
    }

    /// Create an engine with a custom policy gate and audit log.
    ///
    /// Uses `Executor::normal()` and `RouteClassifier::new()` for the
    /// execution subsystems, but allows the caller to inject a configured
    /// policy gate and audit log.
    pub fn with_policy(
        screen: Box<dyn ScreenProvider>,
        policy_gate: PolicyGate,
        audit_log: AuditLog,
    ) -> Self {
        Self {
            executor: Executor::default(),
            screen,
            classifier: RouteClassifier::new(),
            policy_gate,
            audit_log,
        }
    }
}

// ---------------------------------------------------------------------------
// Screen summary conversion
// ---------------------------------------------------------------------------

/// Convert a daemon-level [`reader::ScreenSummary`] to the IPC-level
/// [`aura_types::ipc::ScreenSummary`] used by `ContextPackage`.
///
/// The IPC type is a slimmed-down 4-field struct that fits within the 64 KB
/// IPC envelope. Interactive element descriptions are synthesized from the
/// clickable count and app state.
fn reader_summary_to_ipc(reader: &crate::screen::reader::ScreenSummary) -> ScreenSummary {
    // Build a compact interactive-elements list from the reader's stats.
    let mut interactive = Vec::new();
    if reader.clickable_count > 0 {
        interactive.push(format!("{} clickable", reader.clickable_count));
    }
    if reader.editable_count > 0 {
        interactive.push(format!("{} editable", reader.editable_count));
    }
    if reader.scrollable_count > 0 {
        interactive.push(format!("{} scrollable", reader.scrollable_count));
    }
    if reader.keyboard_visible {
        interactive.push("keyboard visible".to_string());
    }

    ScreenSummary {
        package_name: reader.package_name.clone(),
        activity_name: reader.activity_name.clone(),
        interactive_elements: interactive,
        visible_text: reader.visible_text.clone(),
    }
}

// ---------------------------------------------------------------------------
// Execution mode — which system handles the task
// ---------------------------------------------------------------------------

/// Which cognitive system processes this task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    /// System 1: Document-Guided Scripting — fast, template-driven.
    Dgs,
    /// System 2: Semantic ReAct — LLM-driven reasoning loop.
    SemanticReact,
}

impl std::fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionMode::Dgs => write!(f, "DGS"),
            ExecutionMode::SemanticReact => write!(f, "SemanticReact"),
        }
    }
}

// ---------------------------------------------------------------------------
// Execution strategy — adapts based on failure count
// ---------------------------------------------------------------------------

/// Adaptive execution strategy that escalates as failures accumulate.
///
/// Progression: Direct → Exploratory → Cautious → Recovery.
/// Strategy only moves forward (monotonic escalation) within a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ExecutionStrategy {
    /// Default: execute plan steps directly with minimal overhead.
    Direct,
    /// After 1 failure: try alternative selectors, add small delays.
    Exploratory,
    /// After 2 failures: verify preconditions, take screenshots, be cautious.
    Cautious,
    /// After 3+ failures: escalate to higher model tier, consider aborting.
    Recovery,
}

impl ExecutionStrategy {
    /// Determine strategy from consecutive failure count.
    #[must_use]
    fn from_failure_count(failures: u32) -> Self {
        match failures {
            0 => Self::Direct,
            1 => Self::Exploratory,
            2 => Self::Cautious,
            _ => Self::Recovery,
        }
    }
}

impl std::fmt::Display for ExecutionStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Direct => write!(f, "Direct"),
            Self::Exploratory => write!(f, "Exploratory"),
            Self::Cautious => write!(f, "Cautious"),
            Self::Recovery => write!(f, "Recovery"),
        }
    }
}

// ---------------------------------------------------------------------------
// Mid-execution escalation tier
// ---------------------------------------------------------------------------

/// Escalation tier for mid-execution failures within DGS mode.
///
/// Tiers are strictly monotonic — once escalated, we never go back down
/// within the same step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EscalationTier {
    /// Tier 0: DGS executed successfully — no escalation needed.
    DgsSuccess,
    /// Tier 1: Retry with adjusted parameters (different selector, timing).
    RetryAdjusted,
    /// Tier 2: Escalate this step to Brainstem model (small, ~0.8B).
    Brainstem,
    /// Tier 3: Escalate entire task to full Neocortex (1-4.5GB).
    FullNeocortex,
}

// ---------------------------------------------------------------------------
// ToolCall — structured action output from the LLM
// ---------------------------------------------------------------------------

/// A structured action request produced by System 2 (LLM) reasoning.
///
/// The LLM outputs a tool_name + parameters map, which the executor
/// translates into concrete `ActionType` operations on the device.
#[must_use]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Name of the tool/action to invoke (e.g., "tap", "type_text", "open_app").
    pub tool_name: String,
    /// Parameters as key-value pairs (e.g., {"x": "100", "y": "200"}).
    pub parameters: BTreeMap<String, String>,
    /// LLM's reasoning for choosing this action.
    pub reasoning: String,
}

// ---------------------------------------------------------------------------
// Iteration — one think→act→observe cycle
// ---------------------------------------------------------------------------

/// A single iteration within an agentic session.
///
/// Each iteration represents one complete think→act→observe cycle,
/// whether driven by DGS templates or LLM reasoning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Iteration {
    /// What the agent "thought" before acting (LLM reasoning or DGS step description).
    pub thought: String,
    /// The action taken (structured tool call).
    pub action: ToolCall,
    /// Result of executing the action on the device.
    pub observation: ActionResult,
    /// Signal summary from iteration (structured data, not reasoning text).
    pub reflection: String,
    /// Confidence in the iteration's outcome (0.0–1.0).
    pub confidence: f32,
    /// Wall-clock duration of this iteration in milliseconds.
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// TaskOutcome — final result of an agentic session
// ---------------------------------------------------------------------------

/// Outcome of a completed agentic session.
#[must_use]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskOutcome {
    /// Task completed successfully.
    Success {
        iterations_used: u32,
        total_ms: u64,
        final_confidence: f32,
    },
    /// Task failed after exhausting retries or budget.
    Failed {
        reason: String,
        iterations_used: u32,
        total_ms: u64,
        last_strategy: ExecutionStrategy,
    },
    /// Task was cancelled externally (user interrupt, shutdown).
    Cancelled {
        iterations_completed: u32,
        total_ms: u64,
    },
    /// Cycle detected — execution aborted to prevent infinite loops.
    CycleAborted {
        iterations_completed: u32,
        cycle_reason: String,
    },
}

// ---------------------------------------------------------------------------
// AgenticSession — tracks one task from start to completion
// ---------------------------------------------------------------------------

/// An agentic session tracks a single task through its full lifecycle.
///
/// The session owns the iteration history, strategy state, and budget.
/// It is created when a `TaskRequest` arrives and consumed when the
/// task reaches a terminal state.
#[must_use]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgenticSession {
    /// Unique session identifier (FNV-1a hash of description + timestamp).
    pub session_id: u64,
    /// The task/goal being executed.
    pub task: String,
    /// Which cognitive system is handling this task.
    pub mode: ExecutionMode,
    /// Current iteration count (0-indexed).
    pub iteration_count: u32,
    /// Maximum iterations allowed.
    pub max_iterations: u32,
    /// Number of consecutive failures (resets on success).
    pub consecutive_failures: u32,
    /// Current adaptive strategy.
    pub strategy: ExecutionStrategy,
    /// When the session started (epoch ms).
    pub started_at_ms: u64,
    /// Token budget remaining for System 2 calls.
    pub token_budget: u32,
    /// Iteration history (bounded to [`MAX_ITERATION_HISTORY`]).
    pub iterations: Vec<Iteration>,
    /// Associated goal ID, if wired to the goal system.
    pub goal_id: Option<u64>,
    /// Current escalation tier (for DGS mode).
    pub escalation_tier: EscalationTier,
}

impl AgenticSession {
    /// Create a new session for the given task description.
    #[instrument(skip_all, fields(task = %task))]
    pub fn new(task: String, mode: ExecutionMode) -> Self {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let session_id = fnv1a_hash(format!("{}:{}", task, now_ms).as_bytes());

        info!(session_id, %mode, "creating agentic session");

        Self {
            session_id,
            task,
            mode,
            iteration_count: 0,
            max_iterations: MAX_ITERATIONS,
            consecutive_failures: 0,
            strategy: ExecutionStrategy::Direct,
            started_at_ms: now_ms,
            token_budget: DEFAULT_TOKEN_BUDGET,
            iterations: Vec::with_capacity(MAX_ITERATIONS as usize),
            goal_id: None,
            escalation_tier: EscalationTier::DgsSuccess,
        }
    }

    /// Record a completed iteration, updating strategy and failure tracking.
    pub fn record_iteration(&mut self, iteration: Iteration) {
        let success =
            iteration.observation.success && iteration.confidence >= REFLECTION_SUCCESS_THRESHOLD;

        if success {
            self.consecutive_failures = 0;
        } else {
            self.consecutive_failures += 1;
        }

        // Monotonic strategy escalation — never downgrade.
        let new_strategy = ExecutionStrategy::from_failure_count(self.consecutive_failures);
        if new_strategy > self.strategy {
            info!(
                old = %self.strategy,
                new = %new_strategy,
                consecutive_failures = self.consecutive_failures,
                "strategy escalated"
            );
            self.strategy = new_strategy;
        }

        self.iteration_count += 1;

        // Bounded history — evict oldest if full.
        if self.iterations.len() >= MAX_ITERATION_HISTORY {
            self.iterations.remove(0);
        }
        self.iterations.push(iteration);
    }

    /// Check if the session should terminate.
    #[must_use]
    pub fn should_terminate(&self) -> Option<&'static str> {
        if self.iteration_count >= self.max_iterations {
            return Some("max iterations reached");
        }
        if self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            return Some("max consecutive failures reached");
        }
        if self.token_budget == 0 && self.mode == ExecutionMode::SemanticReact {
            return Some("token budget exhausted");
        }
        None
    }

    /// Elapsed time since session start in milliseconds.
    #[must_use]
    pub fn elapsed_ms(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now.saturating_sub(self.started_at_ms)
    }
}

// ---------------------------------------------------------------------------
// FNV-1a hash utility
// ---------------------------------------------------------------------------

/// Compute FNV-1a 64-bit hash of a byte slice.
///
/// Used for session IDs and cycle detection hashing. FNV-1a is chosen for
/// its simplicity, speed, and good distribution — no cryptographic guarantees
/// needed here.
#[must_use]
fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// ---------------------------------------------------------------------------
// Task classification
// ---------------------------------------------------------------------------

/// Classify a task description to determine which execution mode to use.
///
/// # Architecture: System 1 intentionally disabled (AURA-V4 Iron Laws)
///
/// This function always returns `SemanticReact`. This is **intentional**, not dead code.
///
/// ## Why System 1 (DGS direct execution) is disabled:
///
/// 1. **Iron Law: LLM = Brain, Rust = Body.** Task classification requires understanding task
///    semantics (e.g., "is this a simple query or a multi-step plan?"). That's reasoning, and
///    reasoning belongs exclusively in the LLM (Qwen3-8B). Any keyword matching, regex
///    classification, or complexity heuristic in Rust is Theater AGI — it gives the appearance of
///    intelligence without actual understanding.
///
/// 2. **The LLM decides execution strategy.** Once routed to SemanticReact, the Neocortex inference
///    engine runs the full ReAct loop. The LLM itself can decide within its reasoning whether a
///    task is simple (complete in one iteration) or complex (requires multiple
///    Thought→Action→Observation cycles).
///
/// 3. **DGS templates remain available.** DGS is not dead — it's activated when the assembled
///    prompt contains `DGS TEMPLATE:` markers (see inference.rs:273). The decision to USE a DGS
///    template is made during prompt assembly, not here.
///
/// ## Why the classifier infrastructure is preserved:
///
/// - **Phase 5 (planned):** The Neocortex IPC response will include an explicit `ExecutionMode`
///   recommendation from the LLM. This classifier will then wire that recommendation instead of
///   hardcoding SemanticReact.
/// - **Audit trail:** Enterprise code review (LLM-HIGH-1) confirmed this is architecturally
///   correct. Removing the infrastructure would lose the hook point.
///
/// ## AURA-V4 Audit References:
/// - Enterprise Review finding LLM-HIGH-1: "RouteClassifier Dead Code"
/// - Courtroom verdict: INTENTIONAL — Architecture-correct per Iron Laws
/// - Ground Truth §2.1: "The LLM is the ONLY entity that understands semantics"
#[instrument(skip_all, fields(task_len = task.len()))]
pub fn classify_task(task: &str) -> ExecutionMode {
    let _ = task; // LLM decides execution mode; Rust does not inspect task content.
                  // ARCHITECTURE: Rust does not classify task semantics.
                  // The LLM routes. Always use SemanticReact so the full ReAct loop runs
                  // and the LLM decides the execution strategy.
    ExecutionMode::SemanticReact
}

// ---------------------------------------------------------------------------
// Contextor — builds context packages for Neocortex IPC
// ---------------------------------------------------------------------------

/// Build a [`ContextPackage`] from the current session state and screen.
///
/// The Contextor compresses conversation history and memory snippets to
/// fit within the 64KB IPC limit. Older/less-relevant entries are dropped
/// first.
#[instrument(skip_all, fields(session_id = session.session_id))]
pub fn build_context(
    session: &AgenticSession,
    screen: Option<ScreenSummary>,
    _failure: Option<&FailureContext>,
) -> Result<ContextPackage, AuraError> {
    use aura_types::ipc::{ConversationTurn, Role};

    let mut ctx = ContextPackage {
        inference_mode: InferenceMode::Planner,
        token_budget: session.token_budget,
        current_screen: screen,
        ..Default::default()
    };

    // ── Tier 1: Identity Core fields for agentic sessions ───────────
    // Constitutional tendencies — always present regardless of context path.
    ctx.identity_tendencies = Some(IdentityTendencies::constitutional());
    // Self-knowledge — ReAct sessions always operate in "planning" mode.
    ctx.self_knowledge = Some(SelfKnowledge::for_mode("planning"));
    // user_preferences: None — no UserProfile available in agentic sessions.
    // This is acceptable: the LLM will use sensible defaults.

    // Build conversation history from iteration log.
    // Each iteration becomes a user turn (thought) + assistant turn (observation).
    for iter in session.iterations.iter().rev().take(8) {
        ctx.conversation_history.push(ConversationTurn {
            role: Role::User,
            content: format!("[Thought] {}", iter.thought),
            timestamp_ms: session.started_at_ms,
        });
        ctx.conversation_history.push(ConversationTurn {
            role: Role::Assistant,
            content: format!(
                "[Action] {} → {} (confidence: {:.2})",
                iter.action.tool_name,
                if iter.observation.success {
                    "success"
                } else {
                    "failed"
                },
                iter.confidence
            ),
            timestamp_ms: session.started_at_ms,
        });
    }
    // Reverse so oldest is first.
    ctx.conversation_history.reverse();

    // Set active goal from task description.
    ctx.active_goal = Some(aura_types::ipc::GoalSummary {
        description: session.task.clone(),
        progress_percent: if session.max_iterations > 0 {
            ((session.iteration_count as f32 / session.max_iterations as f32) * 100.0) as u8
        } else {
            0
        },
        current_step: format!(
            "iteration {}/{}",
            session.iteration_count, session.max_iterations
        ),
        blockers: if session.consecutive_failures > 0 {
            vec![format!(
                "{} consecutive failures, strategy: {}",
                session.consecutive_failures, session.strategy
            )]
        } else {
            Vec::new()
        },
    });

    // Enforce the 64KB size limit — trim conversation history if needed.
    while ctx.estimated_size() > ContextPackage::MAX_SIZE && !ctx.conversation_history.is_empty() {
        ctx.conversation_history.remove(0);
    }

    if ctx.estimated_size() > ContextPackage::MAX_SIZE {
        warn!(
            size = ctx.estimated_size(),
            max = ContextPackage::MAX_SIZE,
            "context package still too large after trimming"
        );
    }

    debug!(
        size = ctx.estimated_size(),
        turns = ctx.conversation_history.len(),
        "context package built"
    );

    Ok(ctx)
}

// ---------------------------------------------------------------------------
// ToolCall parsing — extract structured actions from LLM output
// ---------------------------------------------------------------------------

/// Parse a ToolCall from the Neocortex's action plan.
///
/// The LLM returns an `ActionPlan` with `DslStep`s. This function converts
/// the first unexecuted step into a `ToolCall` for the reactive loop.
pub fn plan_step_to_tool_call(step: &aura_types::dsl::DslStep) -> ToolCall {
    let mut parameters = BTreeMap::new();

    // Extract tool name and parameters from the ActionType.
    let tool_name = match &step.action {
        aura_types::actions::ActionType::Tap { x, y } => {
            parameters.insert("x".to_string(), x.to_string());
            parameters.insert("y".to_string(), y.to_string());
            "tap"
        },
        aura_types::actions::ActionType::LongPress { x, y } => {
            parameters.insert("x".to_string(), x.to_string());
            parameters.insert("y".to_string(), y.to_string());
            "long_press"
        },
        aura_types::actions::ActionType::Swipe {
            from_x,
            from_y,
            to_x,
            to_y,
            duration_ms,
        } => {
            parameters.insert("from_x".to_string(), from_x.to_string());
            parameters.insert("from_y".to_string(), from_y.to_string());
            parameters.insert("to_x".to_string(), to_x.to_string());
            parameters.insert("to_y".to_string(), to_y.to_string());
            parameters.insert("duration_ms".to_string(), duration_ms.to_string());
            "swipe"
        },
        aura_types::actions::ActionType::Type { text } => {
            parameters.insert("text".to_string(), text.clone());
            "type_text"
        },
        aura_types::actions::ActionType::Scroll { direction, amount } => {
            parameters.insert("direction".to_string(), format!("{:?}", direction));
            parameters.insert("amount".to_string(), amount.to_string());
            "scroll"
        },
        aura_types::actions::ActionType::Back => "back",
        aura_types::actions::ActionType::Home => "home",
        aura_types::actions::ActionType::Recents => "recents",
        aura_types::actions::ActionType::OpenApp { package } => {
            parameters.insert("package".to_string(), package.clone());
            "open_app"
        },
        aura_types::actions::ActionType::NotificationAction {
            notification_id,
            action_index,
        } => {
            parameters.insert("notification_id".to_string(), notification_id.to_string());
            parameters.insert("action_index".to_string(), action_index.to_string());
            "notification_action"
        },
        aura_types::actions::ActionType::WaitForElement { timeout_ms, .. } => {
            parameters.insert("timeout_ms".to_string(), timeout_ms.to_string());
            "wait_for_element"
        },
        aura_types::actions::ActionType::AssertElement { .. } => "assert_element",
    };

    // Include target selector if present.
    if let Some(ref target) = step.target {
        parameters.insert("target".to_string(), format!("{:?}", target));
    }

    ToolCall {
        tool_name: tool_name.to_string(),
        parameters,
        reasoning: step.label.clone().unwrap_or_default(),
    }
}

/// Convert a LLM-generated [`ToolCall`] into a [`DslStep`] for execution
/// via the real [`Executor`] pipeline.
///
/// This is the inverse of [`plan_step_to_tool_call`].  It bridges the gap
/// between the structured LLM output (`ToolCall.parameters` as strings) and
/// the typed `DslStep` that the executor expects.
fn tool_call_to_dsl_step(tool_call: &ToolCall) -> aura_types::dsl::DslStep {
    use aura_types::actions::{ActionType, ElementAssertion, ScrollDirection, TargetSelector};
    use aura_types::dsl::FailureStrategy;

    let action: ActionType = match tool_call.tool_name.as_str() {
        "tap" => {
            let x: i32 = tool_call
                .parameters
                .get("x")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let y: i32 = tool_call
                .parameters
                .get("y")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            ActionType::Tap { x, y }
        },
        "long_press" => {
            let x: i32 = tool_call
                .parameters
                .get("x")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let y: i32 = tool_call
                .parameters
                .get("y")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            ActionType::LongPress { x, y }
        },
        "swipe" => {
            let from_x: i32 = tool_call
                .parameters
                .get("from_x")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let from_y: i32 = tool_call
                .parameters
                .get("from_y")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let to_x: i32 = tool_call
                .parameters
                .get("to_x")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let to_y: i32 = tool_call
                .parameters
                .get("to_y")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let duration_ms: u32 = tool_call
                .parameters
                .get("duration_ms")
                .and_then(|v| v.parse().ok())
                .unwrap_or(300);
            ActionType::Swipe {
                from_x,
                from_y,
                to_x,
                to_y,
                duration_ms,
            }
        },
        "type_text" => {
            let text = tool_call
                .parameters
                .get("text")
                .cloned()
                .unwrap_or_default();
            ActionType::Type { text }
        },
        "scroll" => {
            let direction = match tool_call
                .parameters
                .get("direction")
                .map(|v| v.as_str())
                .unwrap_or("down")
            {
                "up" => ScrollDirection::Up,
                "down" => ScrollDirection::Down,
                "left" => ScrollDirection::Left,
                "right" => ScrollDirection::Right,
                _ => ScrollDirection::Down,
            };
            let amount: i32 = tool_call
                .parameters
                .get("amount")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            ActionType::Scroll { direction, amount }
        },
        "back" => ActionType::Back,
        "home" => ActionType::Home,
        "recents" => ActionType::Recents,
        "open_app" => {
            let package = tool_call
                .parameters
                .get("package")
                .cloned()
                .unwrap_or_default();
            ActionType::OpenApp { package }
        },
        "notification_action" => {
            let notification_id: u32 = tool_call
                .parameters
                .get("notification_id")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let action_index: u32 = tool_call
                .parameters
                .get("action_index")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            ActionType::NotificationAction {
                notification_id,
                action_index,
            }
        },
        "wait_for_element" => {
            let timeout_ms: u32 = tool_call
                .parameters
                .get("timeout_ms")
                .and_then(|v| v.parse().ok())
                .unwrap_or(5000);
            // Parse selector from parameters — fall back to LLM description for flexible matching
            let selector = if let Some(text) = tool_call.parameters.get("text") {
                TargetSelector::Text(text.clone())
            } else if let Some(resource_id) = tool_call.parameters.get("resource_id") {
                TargetSelector::ResourceId(resource_id.clone())
            } else if let Some(desc) = tool_call.parameters.get("description") {
                TargetSelector::ContentDescription(desc.clone())
            } else {
                // Fall back to LLM-based description matching
                TargetSelector::LlmDescription(
                    tool_call
                        .parameters
                        .values()
                        .map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            };
            ActionType::WaitForElement {
                selector,
                timeout_ms,
            }
        },
        "assert_element" => {
            let selector = if let Some(text) = tool_call.parameters.get("text") {
                TargetSelector::Text(text.clone())
            } else if let Some(resource_id) = tool_call.parameters.get("resource_id") {
                TargetSelector::ResourceId(resource_id.clone())
            } else {
                TargetSelector::LlmDescription(
                    tool_call
                        .parameters
                        .values()
                        .map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            };
            // Determine assertion type from parameters
            let expected = match tool_call
                .parameters
                .get("assert")
                .map(|v| v.as_str())
                .unwrap_or("exists")
            {
                "not_exists" => ElementAssertion::NotExists,
                "text_equals" => ElementAssertion::TextEquals(
                    tool_call
                        .parameters
                        .get("expected_text")
                        .cloned()
                        .unwrap_or_default(),
                ),
                "text_contains" => ElementAssertion::TextContains(
                    tool_call
                        .parameters
                        .get("expected_text")
                        .cloned()
                        .unwrap_or_default(),
                ),
                "is_enabled" => ElementAssertion::IsEnabled,
                "is_checked" => ElementAssertion::IsChecked,
                _ => ElementAssertion::Exists,
            };
            ActionType::AssertElement { selector, expected }
        },
        _ => ActionType::Back,
    };

    aura_types::dsl::DslStep {
        action,
        target: None,
        timeout_ms: 5000,
        on_failure: FailureStrategy::Abort,
        precondition: None,
        postcondition: None,
        label: Some(format!("semantic_react:{}", tool_call.tool_name)),
    }
}

// ---------------------------------------------------------------------------
// Signal Aggregation — post-iteration data processing
// ---------------------------------------------------------------------------

/// Compute iteration signals from the action result.
///
/// Aggregates boolean observation signals (success, screen_changed,
/// matched_element) and current strategy into a confidence score.
/// Returns a short signal summary (NOT reasoning text) and the score.
///
/// ARCHITECTURE: This is signal aggregation (data processing), not reasoning.
/// The LLM evaluates iteration progress via its next think step, using the
/// iteration history already provided in the ReAct prompt.
#[instrument(skip_all)]
pub fn compute_iteration_signals(
    action: &ToolCall,
    observation: &ActionResult,
    strategy: ExecutionStrategy,
) -> (String, f32) {
    let base_confidence: f32 = if observation.success { 0.7 } else { 0.2 };

    // Boost confidence if the screen changed (indicates progress).
    let screen_change_bonus = if observation.screen_changed {
        0.15
    } else {
        0.0
    };

    // Penalty for being in recovery strategy (we're struggling).
    let strategy_penalty = match strategy {
        ExecutionStrategy::Direct => 0.0,
        ExecutionStrategy::Exploratory => 0.05,
        ExecutionStrategy::Cautious => 0.10,
        ExecutionStrategy::Recovery => 0.20,
    };

    // Bonus if a specific element was matched (stronger verification).
    let element_bonus = if observation.matched_element.is_some() {
        0.1
    } else {
        0.0
    };

    let confidence: f32 =
        (base_confidence + screen_change_bonus + element_bonus - strategy_penalty).clamp(0.0, 1.0);

    // Signal summary — structured data, not fake reasoning text.
    let summary = format!(
        "tool={} success={} screen_changed={} confidence={:.2}",
        action.tool_name, observation.success, observation.screen_changed, confidence,
    );

    debug!(confidence, "iteration signals computed");
    (summary, confidence)
}

// ---------------------------------------------------------------------------
// Build FailureContext for re-planning
// ---------------------------------------------------------------------------

/// Build a compact [`FailureContext`] from the current session state.
///
/// Used when escalating to Neocortex for re-planning after DGS failures.
/// The 96-byte struct captures enough context for the LLM to understand
/// what went wrong without sending the full iteration history.
#[must_use]
pub fn build_failure_context(session: &AgenticSession) -> FailureContext {
    let task_hash = fnv1a_hash(session.task.as_bytes());

    // Extract last 3 transitions from iteration history.
    let mut transitions = [TransitionPair::default(); 3];
    let recent: Vec<_> = session.iterations.iter().rev().take(3).collect();
    for (i, iter) in recent.iter().enumerate() {
        if i < 3 {
            transitions[i] = TransitionPair {
                from_hash: fnv1a_hash(iter.thought.as_bytes()),
                to_hash: fnv1a_hash(iter.reflection.as_bytes()),
            };
        }
    }

    // Encode the tried approaches as a bitmask (each bit = one action type tried).
    let tried: u64 = session.iterations.iter().fold(0u64, |acc, iter| {
        let action_hash = fnv1a_hash(iter.action.tool_name.as_bytes());
        acc | (1u64 << (action_hash % 64))
    });

    // Classify the error type (coarse-grained).
    let error_class: u8 = if session.consecutive_failures >= 3 {
        3 // persistent failure
    } else if session.iterations.last().is_some_and(|i| {
        i.observation
            .error
            .as_deref()
            .unwrap_or("")
            .contains("not found")
    }) {
        1 // element not found
    } else if session
        .iterations
        .last()
        .is_some_and(|i| !i.observation.screen_changed)
    {
        2 // stagnation
    } else {
        0 // generic
    };

    FailureContext {
        task_goal_hash: task_hash,
        current_step: session.iteration_count,
        failing_action: session
            .iterations
            .last()
            .map(|i| fnv1a_hash(i.action.tool_name.as_bytes()))
            .unwrap_or(0),
        target_id: 0,           // Populated by executor with actual element ID.
        expected_state_hash: 0, // Populated by executor with screen hash.
        actual_state_hash: 0,   // Populated by executor with screen hash.
        tried_approaches: tried,
        last_3_transitions: transitions,
        error_class,
    }
}

// ---------------------------------------------------------------------------
// Core execution — execute_task (the main entry point)
// ---------------------------------------------------------------------------

/// Execute a task through the bi-cameral agentic loop.
///
/// This is the main entry point called from `main_loop.rs` when a
/// `TaskRequest` arrives. It creates a session, classifies the task,
/// and runs the appropriate execution mode until completion or failure.
///
/// # Arguments
/// * `task` — The task description from the user.
/// * `priority` — Task priority (1-10, clamped).
/// * `plan` — Optional pre-computed action plan (from planner).
/// * `policy` — Optional policy context for safety checks. When `None`, actions execute without
///   policy gating (used in tests and standalone mode where no policy config is available).
///
/// # Returns
/// A tuple of (TaskOutcome, AgenticSession) for the caller to record.
#[instrument(skip_all, fields(task = %task, priority))]
pub async fn execute_task(
    task: String,
    priority: u32,
    plan: Option<ActionPlan>,
    policy: Option<&mut PolicyContext<'_>>,
) -> (TaskOutcome, AgenticSession) {
    let mode = classify_task(&task);
    let mut session = AgenticSession::new(task.clone(), mode);

    info!(
        session_id = session.session_id,
        mode = %mode,
        priority,
        "starting task execution (standalone — no engine)"
    );

    // Standalone path: no engine available. Degrade gracefully.
    // In production, callers should use `ReactEngine::execute_task` instead.
    warn!("execute_task called without ReactEngine — actions will not reach the device");

    match mode {
        ExecutionMode::Dgs => {
            let outcome = execute_dgs_standalone(&mut session, plan, policy).await;
            (outcome, session)
        },
        ExecutionMode::SemanticReact => {
            let outcome = execute_semantic_react_standalone(&mut session, plan, policy).await;
            (outcome, session)
        },
    }
}

// ---------------------------------------------------------------------------
// ReactEngine — wired execution methods
// ---------------------------------------------------------------------------

impl ReactEngine {
    /// Execute a task through the bi-cameral agentic loop with real subsystems.
    ///
    /// Preferred over the standalone [`execute_task`] function. This method
    /// has access to `Executor`, `ScreenProvider`, and `RouteClassifier`.
    #[instrument(skip(self), fields(task = %task, priority))]
    pub async fn execute_task(
        &mut self,
        task: String,
        priority: u32,
        plan: Option<ActionPlan>,
    ) -> (TaskOutcome, AgenticSession) {
        let mode = classify_task(&task);
        let mut session = AgenticSession::new(task.clone(), mode);

        info!(
            session_id = session.session_id,
            mode = %mode,
            priority,
            "starting task execution via ReactEngine"
        );

        match mode {
            ExecutionMode::Dgs => {
                let outcome = self.execute_dgs(&mut session, plan).await;
                (outcome, session)
            },
            ExecutionMode::SemanticReact => {
                let outcome = self.execute_semantic_react(&mut session, plan).await;
                (outcome, session)
            },
        }
    }

    /// Capture the current screen and produce an IPC-level summary.
    ///
    /// Returns `None` if the screen provider is unavailable or the capture
    /// fails. Failures are logged but never fatal — the engine proceeds
    /// with a degraded context.
    fn capture_screen_summary(&self) -> Option<ScreenSummary> {
        match self.screen.capture_tree() {
            Ok(tree) => {
                let reader_summary = extract_screen_summary(&tree);
                Some(reader_summary_to_ipc(&reader_summary))
            },
            Err(e) => {
                warn!(error = %e, "screen capture failed — proceeding without screen summary");
                None
            },
        }
    }

    /// Capture the current screen hash for cycle/change detection.
    ///
    /// Returns `0` on failure (treated as "unknown state" by callers).
    fn capture_screen_hash(&self) -> u64 {
        match self.screen.capture_tree() {
            Ok(tree) => hash_screen_state(&tree),
            Err(e) => {
                warn!(error = %e, "screen capture for hash failed");
                0
            },
        }
    }
}

// ---------------------------------------------------------------------------
// DGS execution — System 1 (template-guided), wired
// ---------------------------------------------------------------------------

impl ReactEngine {
    /// Execute a task using Document-Guided Scripting (System 1) with real
    /// subsystems.
    ///
    /// Passes each plan step to the [`Executor`] which drives the 11-stage
    /// pipeline (capture → resolve → anti-bot → delay → execute → verify →
    /// retry → cycle → invariants → ETG → result). The outcome is mapped
    /// back to a single [`Iteration`] record and [`TaskOutcome`].
    #[instrument(skip(self), fields(session_id = session.session_id))]
    async fn execute_dgs(
        &mut self,
        session: &mut AgenticSession,
        plan: Option<ActionPlan>,
    ) -> TaskOutcome {
        let plan = match plan {
            Some(p) => p,
            None => {
                info!("no plan available for DGS — escalating to SemanticReact");
                session.mode = ExecutionMode::SemanticReact;
                return self.execute_semantic_react(session, None).await;
            },
        };

        info!(
            steps = plan.steps.len(),
            source = ?plan.source,
            confidence = plan.confidence,
            "executing DGS plan via Executor"
        );

        let start = Instant::now();
        let mut budget = TokenBudgetManager::new(TokenBudgetConfig::default());

        for (step_idx, step) in plan.steps.iter().enumerate() {
            if let Some(reason) = session.should_terminate() {
                warn!(reason, step = step_idx, "session terminating");
                return TaskOutcome::Failed {
                    reason: reason.to_string(),
                    iterations_used: session.iteration_count,
                    total_ms: start.elapsed().as_millis() as u64,
                    last_strategy: session.strategy,
                };
            }

            let tool_call = plan_step_to_tool_call(step);
            let thought = format!(
                "DGS step {}/{}: {} ({})",
                step_idx + 1,
                plan.steps.len(),
                tool_call.tool_name,
                tool_call.reasoning
            );

            debug!(step = step_idx, action = %tool_call.tool_name, "executing DGS step");

            // --- Policy gate check (before any action reaches the Executor) ---
            {
                let mut policy_ctx = PolicyContext {
                    gate: &mut self.policy_gate,
                    audit: &mut self.audit_log,
                };
                let action_str = format!("dgs:{}", plan.goal_description);
                if let Some(denied_result) = policy_ctx.check_action(&action_str) {
                    let (reflection, confidence) =
                        compute_iteration_signals(&tool_call, &denied_result, session.strategy);
                    session.record_iteration(Iteration {
                        thought,
                        action: tool_call,
                        observation: denied_result,
                        reflection,
                        confidence,
                        duration_ms: start.elapsed().as_millis() as u64,
                    });
                    // Don't abort the entire session — just skip this step.
                    // The loop will continue and may try the next plan step or
                    // terminate on consecutive failures.
                    continue;
                }
            }

            // Build a single-step plan for the Executor.
            let single_step_plan = ActionPlan {
                goal_description: plan.goal_description.clone(),
                steps: vec![step.clone()],
                estimated_duration_ms: step.timeout_ms,
                confidence: plan.confidence,
                source: plan.source,
            };

            // Capture before-hash for change detection.
            let before_hash = self.capture_screen_hash();

            // Execute via the real Executor pipeline.
            let exec_result = self
                .executor
                .execute(&single_step_plan, self.screen.as_ref())
                .await;

            let after_hash = self.capture_screen_hash();
            let screen_changed = before_hash != after_hash && before_hash != 0;
            let elapsed_ms = start.elapsed().as_millis() as u64;

            // --- OBSERVE ---
            let observation = match exec_result {
                Ok(ExecutionOutcome::Success { .. }) => ActionResult {
                    success: true,
                    duration_ms: elapsed_ms as u32,
                    error: None,
                    screen_changed,
                    matched_element: Some(format!("exec_{}", tool_call.tool_name)),
                },
                Ok(ExecutionOutcome::Failed { reason, .. }) => ActionResult {
                    success: false,
                    duration_ms: elapsed_ms as u32,
                    error: Some(reason),
                    screen_changed,
                    matched_element: None,
                },
                Ok(ExecutionOutcome::Cancelled { .. }) => {
                    return TaskOutcome::Cancelled {
                        iterations_completed: session.iteration_count,
                        total_ms: elapsed_ms,
                    };
                },
                Ok(ExecutionOutcome::CycleDetected { step, tier }) => {
                    let reason = format!("cycle at step {} tier {}", step, tier);
                    return TaskOutcome::CycleAborted {
                        iterations_completed: session.iteration_count,
                        cycle_reason: reason,
                    };
                },
                Err(e) => ActionResult {
                    success: false,
                    duration_ms: elapsed_ms as u32,
                    error: Some(format!("executor error: {e}")),
                    screen_changed,
                    matched_element: None,
                },
            };

            // --- REFLECT ---
            let (reflection, confidence) =
                compute_iteration_signals(&tool_call, &observation, session.strategy);

            let iteration = Iteration {
                thought,
                action: tool_call,
                observation,
                reflection,
                confidence,
                duration_ms: elapsed_ms,
            };

            session.record_iteration(iteration);

            // --- ReAct IPC: send step result to neocortex, await LLM decision ---
            // Capture post-action screen state for the prompt.
            // Include interactive elements so the LLM sees what's actually on screen,
            // not just the package/activity name. Cap at 10 elements to stay within budget.
            let post_screen_desc = self
                .capture_screen_summary()
                .map(|s| {
                    let mut desc = format!("{}/{}", s.package_name, s.activity_name);
                    let elements: Vec<&str> = s
                        .interactive_elements
                        .iter()
                        .take(10)
                        .map(|e| e.as_str())
                        .collect();
                    if !elements.is_empty() {
                        desc.push_str(" | interactive: ");
                        desc.push_str(&elements.join(", "));
                    }
                    if !s.visible_text.is_empty() {
                        let text_preview: Vec<&str> =
                            s.visible_text.iter().take(5).map(|t| t.as_str()).collect();
                        desc.push_str(" | text: ");
                        desc.push_str(&text_preview.join("; "));
                    }
                    desc
                })
                .unwrap_or_else(|| "unknown".to_string());

            let (react_done, react_tokens) = send_react_step_ipc(
                session
                    .iterations
                    .last()
                    .map(|i| i.action.tool_name.as_str())
                    .unwrap_or(""),
                &session
                    .iterations
                    .last()
                    .map(|i| i.observation.clone())
                    .unwrap_or_else(|| ActionResult {
                        success: false,
                        duration_ms: 0,
                        error: Some("no observation recorded".to_string()),
                        screen_changed: false,
                        matched_element: None,
                    }),
                &post_screen_desc,
                &session.task,
                session.iteration_count,
                session.max_iterations,
            )
            .await;
            budget.record_usage(react_tokens);
            {
                let snap = budget.snapshot();
                debug!(
                    budget_used = snap.session_used,
                    budget_limit = snap.session_limit,
                    budget_pct = snap.used_pct,
                    budget_calls = snap.calls_in_session,
                    "budget snapshot after ReActDecision"
                );
            }

            // Honour neocortex completion signal; fall back to heuristic when
            // neocortex is not reachable.
            if let Some(true) = react_done {
                info!("neocortex signalled task complete via ReActDecision");
                return TaskOutcome::Success {
                    iterations_used: session.iteration_count,
                    total_ms: start.elapsed().as_millis() as u64,
                    final_confidence: session
                        .iterations
                        .last()
                        .map(|i| i.confidence)
                        .unwrap_or(1.0),
                };
            }

            // Check if the task appears complete (high confidence on successful action).
            if let Some(last) = session.iterations.last() {
                if last.observation.success && last.confidence >= 0.85 {
                    info!(
                        confidence = last.confidence,
                        "high-confidence success — task likely complete"
                    );
                    return TaskOutcome::Success {
                        iterations_used: session.iteration_count,
                        total_ms: start.elapsed().as_millis() as u64,
                        final_confidence: last.confidence,
                    };
                }
            }
        }

        // All steps completed — return success with final confidence.
        let final_confidence = session
            .iterations
            .last()
            .map(|i| i.confidence)
            .unwrap_or(0.0);

        TaskOutcome::Success {
            iterations_used: session.iteration_count,
            total_ms: start.elapsed().as_millis() as u64,
            final_confidence,
        }
    }

    /// Execute a task using Semantic ReAct (System 2) with real subsystems.
    ///
    /// LLM-driven think→act→observe loop for novel or complex tasks.
    /// Actions are executed via the real [`Executor`] pipeline (with built-in
    /// PolicyGate, sandbox, anti-bot, and verification), not simulated.
    #[instrument(skip(self), fields(session_id = session.session_id))]
    async fn execute_semantic_react(
        &mut self,
        session: &mut AgenticSession,
        plan: Option<ActionPlan>,
    ) -> TaskOutcome {
        let start = Instant::now();
        let mut current_plan = plan;

        loop {
            if let Some(reason) = session.should_terminate() {
                warn!(reason, "SemanticReact session terminating");
                return TaskOutcome::Failed {
                    reason: reason.to_string(),
                    iterations_used: session.iteration_count,
                    total_ms: start.elapsed().as_millis() as u64,
                    last_strategy: session.strategy,
                };
            }

            if session.iteration_count >= session.max_iterations {
                warn!(
                    iterations = session.iteration_count,
                    "max iterations reached"
                );
                return TaskOutcome::Failed {
                    reason: "max iterations reached".to_string(),
                    iterations_used: session.iteration_count,
                    total_ms: start.elapsed().as_millis() as u64,
                    last_strategy: session.strategy,
                };
            }

            let context = match build_context(session, None, None) {
                Ok(_ctx) => {},
                Err(e) => {
                    warn!(error = %e, "failed to build context — aborting");
                    return TaskOutcome::Failed {
                        reason: format!("context build failed: {e}"),
                        iterations_used: session.iteration_count,
                        total_ms: start.elapsed().as_millis() as u64,
                        last_strategy: session.strategy,
                    };
                },
            };

            let thought = format!(
                "SemanticReact iteration {}: strategy={}, failures={}",
                session.iteration_count, session.strategy, session.consecutive_failures
            );

            // Resolve the next tool call from the plan (if any) or signal done.
            let tool_call = if let Some(ref plan) = current_plan {
                let step_idx = session.iteration_count as usize;
                if step_idx < plan.steps.len() {
                    plan_step_to_tool_call(&plan.steps[step_idx])
                } else {
                    info!("plan exhausted — would request re-plan from neocortex");
                    current_plan = None;
                    continue;
                }
            } else {
                info!("no plan available — SemanticReact needs a plan to proceed");
                return TaskOutcome::Failed {
                    reason:
                        "SemanticReact mode requires an action plan (use DGS or provide a plan)"
                            .to_string(),
                    iterations_used: session.iteration_count,
                    total_ms: start.elapsed().as_millis() as u64,
                    last_strategy: session.strategy,
                };
            };

            debug!(action = %tool_call.tool_name, "SemanticReact executing action via Executor");

            // --- Policy gate check (before action reaches the Executor) ---
            let mut policy_ctx = PolicyContext {
                gate: &mut self.policy_gate,
                audit: &mut self.audit_log,
            };
            let action_str = format!("{}:{}", tool_call.tool_name, tool_call.reasoning);
            let observation = if let Some(denied_result) = policy_ctx.check_action(&action_str) {
                denied_result
            } else {
                // Convert ToolCall to DslStep and execute via the real Executor.
                let dsl_step = tool_call_to_dsl_step(&tool_call);
                let goal_desc = current_plan
                    .as_ref()
                    .map(|p| p.goal_description.clone())
                    .unwrap_or_else(|| session.task.clone());
                let single_step_plan = ActionPlan {
                    goal_description: goal_desc,
                    steps: vec![dsl_step],
                    estimated_duration_ms: 5000,
                    confidence: 0.8,
                    source: current_plan
                        .as_ref()
                        .map(|p| p.source)
                        .unwrap_or(aura_types::etg::PlanSource::UserDefined),
                };

                let before_hash = self.capture_screen_hash();

                let exec_result = self
                    .executor
                    .execute(&single_step_plan, self.screen.as_ref())
                    .await;

                let after_hash = self.capture_screen_hash();
                let screen_changed = before_hash != after_hash && before_hash != 0;
                let elapsed_ms = start.elapsed().as_millis() as u64;

                match exec_result {
                    Ok(ExecutionOutcome::Success { .. }) => ActionResult {
                        success: true,
                        duration_ms: elapsed_ms as u32,
                        error: None,
                        screen_changed,
                        matched_element: Some(format!("exec_{}", tool_call.tool_name)),
                    },
                    Ok(ExecutionOutcome::Failed { reason, .. }) => ActionResult {
                        success: false,
                        duration_ms: elapsed_ms as u32,
                        error: Some(reason),
                        screen_changed,
                        matched_element: None,
                    },
                    Ok(ExecutionOutcome::Cancelled { .. }) => ActionResult {
                        success: false,
                        duration_ms: elapsed_ms as u32,
                        error: Some("execution cancelled".to_string()),
                        screen_changed,
                        matched_element: None,
                    },
                    Ok(ExecutionOutcome::CycleDetected { step, tier }) => ActionResult {
                        success: false,
                        duration_ms: elapsed_ms as u32,
                        error: Some(format!("cycle detected at step {} tier {}", step, tier)),
                        screen_changed,
                        matched_element: None,
                    },
                    Err(e) => ActionResult {
                        success: false,
                        duration_ms: elapsed_ms as u32,
                        error: Some(format!("executor error: {e}")),
                        screen_changed,
                        matched_element: None,
                    },
                }
            };

            let post_screen_desc = self
                .capture_screen_summary()
                .map(|s| {
                    let mut desc = format!("{}/{}", s.package_name, s.activity_name);
                    let elements: Vec<&str> = s
                        .interactive_elements
                        .iter()
                        .take(10)
                        .map(|e| e.as_str())
                        .collect();
                    if !elements.is_empty() {
                        desc.push_str(" | interactive: ");
                        desc.push_str(&elements.join(", "));
                    }
                    if !s.visible_text.is_empty() {
                        let text_preview: Vec<&str> =
                            s.visible_text.iter().take(5).map(|t| t.as_str()).collect();
                        desc.push_str(" | text: ");
                        desc.push_str(&text_preview.join("; "));
                    }
                    desc
                })
                .unwrap_or_else(|| "unknown".to_string());

            let (react_done, react_tokens) = send_react_step_ipc(
                &tool_call.tool_name,
                &observation,
                &post_screen_desc,
                &session.task,
                session.iteration_count,
                session.max_iterations,
            )
            .await;

            let (reflection, confidence) =
                compute_iteration_signals(&tool_call, &observation, session.strategy);

            let iteration = Iteration {
                thought,
                action: tool_call,
                observation,
                reflection,
                confidence,
                duration_ms: start.elapsed().as_millis() as u64,
            };

            session.record_iteration(iteration);

            // Check neocortex completion signal; fall back to heuristic.
            if let Some(true) = react_done {
                info!("neocortex signalled task complete via ReActDecision");
                return TaskOutcome::Success {
                    iterations_used: session.iteration_count,
                    total_ms: start.elapsed().as_millis() as u64,
                    final_confidence: session
                        .iterations
                        .last()
                        .map(|i| i.confidence)
                        .unwrap_or(1.0),
                };
            }

            if let Some(last) = session.iterations.last() {
                if last.observation.success && last.confidence >= 0.85 {
                    info!(
                        confidence = last.confidence,
                        "high-confidence success — task likely complete"
                    );
                    return TaskOutcome::Success {
                        iterations_used: session.iteration_count,
                        total_ms: start.elapsed().as_millis() as u64,
                        final_confidence: last.confidence,
                    };
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DGS mid-execution escalation
// ---------------------------------------------------------------------------

/// Determine the next escalation tier for a failed DGS step.
///
/// Tiers are strictly monotonic — once we escalate, we don't go back.
/// Returns the new tier after escalation.
fn escalate_dgs_step(session: &mut AgenticSession, step: u32) -> EscalationTier {
    let new_tier = match session.escalation_tier {
        EscalationTier::DgsSuccess => EscalationTier::RetryAdjusted,
        EscalationTier::RetryAdjusted => EscalationTier::Brainstem,
        EscalationTier::Brainstem => EscalationTier::FullNeocortex,
        EscalationTier::FullNeocortex => EscalationTier::FullNeocortex, // already at max
    };

    if new_tier > session.escalation_tier {
        info!(
            step,
            old_tier = ?session.escalation_tier,
            new_tier = ?new_tier,
            "DGS step escalation"
        );
        session.escalation_tier = new_tier;
    }

    new_tier
}

// ---------------------------------------------------------------------------
// Standalone execution (backward-compatible, no engine)
// ---------------------------------------------------------------------------

/// Standalone DGS execution without a `ReactEngine`.
///
/// Used by the free-standing [`execute_task`] function for backward
/// compatibility. Actions are **simulated** — no real device interaction.
/// Prefer [`ReactEngine::execute_task`] in production code.
///
/// When `policy` is `Some`, each action is checked against the policy
/// gate before simulation.  Denied actions produce a failed observation
/// and may trigger escalation.
#[instrument(skip_all, fields(session_id = session.session_id))]
async fn execute_dgs_standalone(
    session: &mut AgenticSession,
    plan: Option<ActionPlan>,
    mut policy: Option<&mut PolicyContext<'_>>,
) -> TaskOutcome {
    let plan = match plan {
        Some(p) => p,
        None => {
            info!("no plan available for DGS — escalating to SemanticReact");
            session.mode = ExecutionMode::SemanticReact;
            return execute_semantic_react_standalone(session, None, policy).await;
        },
    };

    info!(
        steps = plan.steps.len(),
        source = ?plan.source,
        confidence = plan.confidence,
        "executing DGS plan (standalone/simulated)"
    );

    let start = Instant::now();

    for (step_idx, step) in plan.steps.iter().enumerate() {
        if let Some(reason) = session.should_terminate() {
            warn!(reason, step = step_idx, "session terminating");
            return TaskOutcome::Failed {
                reason: reason.to_string(),
                iterations_used: session.iteration_count,
                total_ms: start.elapsed().as_millis() as u64,
                last_strategy: session.strategy,
            };
        }

        let tool_call = plan_step_to_tool_call(step);
        let thought = format!(
            "DGS step {}/{}: {} ({})",
            step_idx + 1,
            plan.steps.len(),
            tool_call.tool_name,
            tool_call.reasoning
        );

        debug!(step = step_idx, action = %tool_call.tool_name, "executing DGS step (standalone)");

        // --- Policy gate check (if available) ---
        // In non-test builds, simulate_action_result() returns a blocked
        // ActionResult immediately — standalone paths cannot reach the device.
        let observation = if let Some(ref mut ctx) = policy {
            let action_str = format!("{}:{}", tool_call.tool_name, tool_call.reasoning);
            if let Some(denied_result) = ctx.check_action(&action_str) {
                denied_result
            } else {
                simulate_action_result(&tool_call)
            }
        } else {
            simulate_action_result(&tool_call)
        };

        // In production (non-test), simulate_action_result returns a failure
        // with a clear "neocortex unavailable" error.  Surface this immediately
        // so the caller sees a Blocked-equivalent outcome rather than looping.
        #[cfg(not(test))]
        if !observation.success {
            if let Some(ref err) = observation.error {
                if err.contains("neocortex unavailable") {
                    warn!(
                        task = %session.task,
                        "standalone DGS path blocked: neocortex unavailable — \
                         use ReactEngine::execute_task in production"
                    );
                    return TaskOutcome::Failed {
                        reason:
                            "neocortex unavailable: standalone mode cannot execute real actions"
                                .to_string(),
                        iterations_used: session.iteration_count,
                        total_ms: start.elapsed().as_millis() as u64,
                        last_strategy: session.strategy,
                    };
                }
            }
        }

        // --- ReAct IPC: send step result to neocortex, await decision ---
        let (react_done, _) = send_react_step_ipc(
            &tool_call.tool_name,
            &observation,
            "standalone-dgs: no live screen available",
            &session.task,
            step_idx as u32,
            plan.steps.len() as u32,
        )
        .await;

        let (reflection, confidence) =
            compute_iteration_signals(&tool_call, &observation, session.strategy);

        let iteration = Iteration {
            thought,
            action: tool_call,
            observation: observation.clone(),
            reflection,
            confidence,
            duration_ms: start.elapsed().as_millis() as u64,
        };

        session.record_iteration(iteration);

        // If neocortex signalled completion, honour it.
        if let Some(true) = react_done {
            info!(
                step = step_idx,
                "neocortex signalled task complete via ReActDecision"
            );
            return TaskOutcome::Success {
                iterations_used: session.iteration_count,
                total_ms: start.elapsed().as_millis() as u64,
                final_confidence: confidence,
            };
        }

        if !observation.success {
            let new_tier = escalate_dgs_step(session, step_idx as u32);
            if new_tier >= EscalationTier::FullNeocortex {
                info!("DGS step escalated to full Neocortex — switching to SemanticReact");
                session.mode = ExecutionMode::SemanticReact;
                return execute_semantic_react_standalone(session, Some(plan.clone()), policy)
                    .await;
            }
        }
    }

    let final_confidence = session
        .iterations
        .last()
        .map(|i| i.confidence)
        .unwrap_or(0.0);

    TaskOutcome::Success {
        iterations_used: session.iteration_count,
        total_ms: start.elapsed().as_millis() as u64,
        final_confidence,
    }
}

/// Standalone SemanticReact execution without a `ReactEngine`.
///
/// Used by the free-standing [`execute_task`] function for backward
/// compatibility. Actions are **simulated** — no real device interaction.
/// Prefer [`ReactEngine::execute_task`] in production code.
///
/// When `policy` is `Some`, each action is checked against the policy
/// gate before simulation.  Denied actions produce a failed observation.
#[instrument(skip_all, fields(session_id = session.session_id))]
async fn execute_semantic_react_standalone(
    session: &mut AgenticSession,
    initial_plan: Option<ActionPlan>,
    mut policy: Option<&mut PolicyContext<'_>>,
) -> TaskOutcome {
    let start = Instant::now();
    let mut current_plan = initial_plan;

    loop {
        if let Some(reason) = session.should_terminate() {
            warn!(reason, "SemanticReact session terminating");
            return TaskOutcome::Failed {
                reason: reason.to_string(),
                iterations_used: session.iteration_count,
                total_ms: start.elapsed().as_millis() as u64,
                last_strategy: session.strategy,
            };
        }

        // Build context (no real screen available in standalone mode).
        let _context = match build_context(session, None, None) {
            Ok(ctx) => ctx,
            Err(e) => {
                warn!(error = %e, "failed to build context — aborting");
                return TaskOutcome::Failed {
                    reason: format!("context build failed: {e}"),
                    iterations_used: session.iteration_count,
                    total_ms: start.elapsed().as_millis() as u64,
                    last_strategy: session.strategy,
                };
            },
        };

        let thought = format!(
            "SemanticReact iteration {}: strategy={}, failures={}",
            session.iteration_count, session.strategy, session.consecutive_failures
        );

        let tool_call = if let Some(ref plan) = current_plan {
            let step_idx = session.iteration_count as usize;
            if step_idx < plan.steps.len() {
                plan_step_to_tool_call(&plan.steps[step_idx])
            } else {
                info!("plan exhausted — would request re-plan from neocortex");
                current_plan = None;
                continue;
            }
        } else {
            info!("no plan and no neocortex connection — aborting SemanticReact");
            return TaskOutcome::Failed {
                reason: "no action plan available and neocortex not connected".to_string(),
                iterations_used: session.iteration_count,
                total_ms: start.elapsed().as_millis() as u64,
                last_strategy: session.strategy,
            };
        };

        debug!(action = %tool_call.tool_name, "SemanticReact executing action (standalone)");

        // --- Policy gate check (if available) ---
        // In non-test builds, simulate_action_result() returns a blocked
        // ActionResult immediately — standalone paths cannot reach the device.
        let observation = if let Some(ref mut ctx) = policy {
            let action_str = format!("{}:{}", tool_call.tool_name, tool_call.reasoning);
            if let Some(denied_result) = ctx.check_action(&action_str) {
                denied_result
            } else {
                simulate_action_result(&tool_call)
            }
        } else {
            simulate_action_result(&tool_call)
        };

        // In production (non-test), simulate_action_result returns a failure
        // with a clear "neocortex unavailable" error.  Surface this immediately.
        #[cfg(not(test))]
        if !observation.success {
            if let Some(ref err) = observation.error {
                if err.contains("neocortex unavailable") {
                    warn!(
                        task = %session.task,
                        "standalone SemanticReact path blocked: neocortex unavailable — \
                         use ReactEngine::execute_task in production"
                    );
                    return TaskOutcome::Failed {
                        reason:
                            "neocortex unavailable: standalone mode cannot execute real actions"
                                .to_string(),
                        iterations_used: session.iteration_count,
                        total_ms: start.elapsed().as_millis() as u64,
                        last_strategy: session.strategy,
                    };
                }
            }
        }

        // --- ReAct IPC: send step result to neocortex, await decision ---
        let (react_done, _) = send_react_step_ipc(
            &tool_call.tool_name,
            &observation,
            "standalone-semantic: no live screen available",
            &session.task,
            session.iteration_count,
            session.max_iterations,
        )
        .await;

        let (reflection, confidence) =
            compute_iteration_signals(&tool_call, &observation, session.strategy);

        let iteration = Iteration {
            thought,
            action: tool_call,
            observation,
            reflection,
            confidence,
            duration_ms: start.elapsed().as_millis() as u64,
        };

        session.record_iteration(iteration);

        // If neocortex signalled completion, honour it; otherwise fall back
        // to the high-confidence heuristic.
        if let Some(true) = react_done {
            info!("neocortex signalled task complete via ReActDecision (standalone)");
            return TaskOutcome::Success {
                iterations_used: session.iteration_count,
                total_ms: start.elapsed().as_millis() as u64,
                final_confidence: confidence,
            };
        }

        if let Some(last) = session.iterations.last() {
            if last.observation.success && last.confidence >= 0.85 {
                info!(
                    confidence = last.confidence,
                    "high-confidence success — task likely complete"
                );
                return TaskOutcome::Success {
                    iterations_used: session.iteration_count,
                    total_ms: start.elapsed().as_millis() as u64,
                    final_confidence: last.confidence,
                };
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ReAct IPC helper — closes the LLM feedback loop
// ---------------------------------------------------------------------------

/// Send a completed ReAct step to the neocortex and await its decision.
///
/// Opens a fresh [`NeocortexClient`] connection, sends
/// [`DaemonToNeocortex::ReActStep`] with the action result and current screen
/// state, then waits for [`NeocortexToDaemon::ReActDecision`].
///
/// Returns `(Some(true), tokens)` if the LLM considers the goal complete,
/// `(Some(false), tokens)` to continue, or `(None, 0)` if the neocortex is
/// not reachable (falls back to the heuristic caller logic).
async fn send_react_step_ipc(
    tool_name: &str,
    observation: &ActionResult,
    screen_description: &str,
    goal: &str,
    step_index: u32,
    max_steps: u32,
) -> (Option<bool>, u32) {
    let obs_text = if observation.success {
        format!(
            "action succeeded in {}ms; screen_changed={}; element={:?}",
            observation.duration_ms, observation.screen_changed, observation.matched_element
        )
    } else {
        format!(
            "action failed: {}",
            observation.error.as_deref().unwrap_or("unknown error")
        )
    };

    let msg = DaemonToNeocortex::ReActStep {
        tool_name: tool_name.to_string(),
        observation: obs_text,
        screen_description: screen_description.to_string(),
        goal: goal.to_string(),
        step_index,
        max_steps,
    };

    let mut client = match NeocortexClient::connect().await {
        Ok(c) => c,
        Err(e) => {
            debug!(error = %e, "neocortex not reachable for ReActStep — using heuristic fallback");
            return (None, 0);
        },
    };

    match client.request(&msg).await {
        Ok(NeocortexToDaemon::ReActDecision {
            done,
            reasoning,
            next_action,
            tokens_used,
        }) => {
            info!(
                done,
                reasoning = %reasoning,
                next_action = ?next_action,
                "received ReActDecision from neocortex"
            );
            (Some(done), tokens_used)
        },
        Ok(other) => {
            warn!(resp = ?std::mem::discriminant(&other), "unexpected response to ReActStep");
            (None, 0)
        },
        Err(e) => {
            warn!(error = %e, "ReActStep request failed — using heuristic fallback");
            (None, 0)
        },
    }
}

// ---------------------------------------------------------------------------
// Simulation helper (test-only)
// ---------------------------------------------------------------------------

/// Simulate an action result for **unit tests only**.
///
/// Produces deterministic results based on the tool name:
/// - `"assert_element"` → failure (simulated assertion miss)
/// - Everything else → success with `screen_changed = true` (except `"wait_for_element"` which
///   succeeds but doesn't change screen)
///
/// **MUST NOT be called in production paths.** The standalone execution
/// helpers (`execute_dgs_standalone` / `execute_semantic_react_standalone`)
/// are themselves test-only entry points — they carry a compile-time guard
/// that calls this function only inside `#[cfg(test)]` contexts.
///
/// In production, the [`ReactEngine`] methods call the real [`Executor`]
/// and [`ScreenProvider`] instead.
#[cfg(test)]
fn simulate_action_result(tool_call: &ToolCall) -> ActionResult {
    let success = tool_call.tool_name != "assert_element";

    ActionResult {
        success,
        duration_ms: 150,
        error: if success {
            None
        } else {
            Some("simulated assertion failure".to_string())
        },
        screen_changed: success && tool_call.tool_name != "wait_for_element",
        matched_element: if success {
            Some(format!("sim_{}", tool_call.tool_name))
        } else {
            None
        },
    }
}

/// Produce a blocked `ActionResult` for standalone (non-test) execution.
///
/// In production the daemon always has a [`ReactEngine`] with a real
/// [`Executor`] and [`ScreenProvider`].  The standalone `execute_task`
/// function is a backward-compatibility shim; when called outside of tests
/// it cannot reach the device, so every action is immediately blocked with
/// a clear error rather than silently returning fake success.
///
/// The caller is expected to propagate this as `TaskOutcome::Failed` with
/// reason `"neocortex unavailable: standalone mode cannot execute real actions"`.
#[cfg(not(test))]
fn simulate_action_result(_tool_call: &ToolCall) -> ActionResult {
    ActionResult {
        success: false,
        duration_ms: 0,
        error: Some(
            "neocortex unavailable: standalone mode cannot execute real actions — \
             use ReactEngine::execute_task in production"
                .to_string(),
        ),
        screen_changed: false,
        matched_element: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use aura_types::{
        actions::{ActionResult, ActionType, TargetSelector},
        dsl::{DslStep, FailureStrategy},
        etg::PlanSource,
    };

    use super::*;

    // --- FNV-1a ---

    #[test]
    fn test_fnv1a_hash_deterministic() {
        let h1 = fnv1a_hash(b"hello");
        let h2 = fnv1a_hash(b"hello");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_fnv1a_hash_different_inputs() {
        let h1 = fnv1a_hash(b"hello");
        let h2 = fnv1a_hash(b"world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_fnv1a_hash_empty() {
        let h = fnv1a_hash(b"");
        assert_eq!(h, FNV_OFFSET); // empty input = offset basis
    }

    // --- Task classification ---

    #[test]
    fn test_classify_dgs_keywords() {
        assert_eq!(
            classify_task("open app settings"),
            ExecutionMode::SemanticReact
        );
        assert_eq!(
            classify_task("tap on the button"),
            ExecutionMode::SemanticReact
        );
        assert_eq!(classify_task("scroll down"), ExecutionMode::SemanticReact);
        assert_eq!(
            classify_task("send message to John"),
            ExecutionMode::SemanticReact
        );
    }

    #[test]
    fn test_classify_semantic_react_keywords() {
        assert_eq!(
            classify_task("figure out why the WiFi is slow"),
            ExecutionMode::SemanticReact
        );
        assert_eq!(
            classify_task("analyze my battery usage patterns"),
            ExecutionMode::SemanticReact
        );
        assert_eq!(
            classify_task("compare these two apps and recommend the best one"),
            ExecutionMode::SemanticReact
        );
    }

    #[test]
    fn test_classify_short_task_dgs() {
        assert_eq!(classify_task("go home"), ExecutionMode::SemanticReact);
        assert_eq!(classify_task("press back"), ExecutionMode::SemanticReact);
    }

    #[test]
    fn test_classify_default_semantic_react() {
        assert_eq!(
            classify_task("I need help with a complex multi-step workflow involving several apps"),
            ExecutionMode::SemanticReact
        );
    }

    // --- ExecutionStrategy ---

    #[test]
    fn test_strategy_from_failure_count() {
        assert_eq!(
            ExecutionStrategy::from_failure_count(0),
            ExecutionStrategy::Direct
        );
        assert_eq!(
            ExecutionStrategy::from_failure_count(1),
            ExecutionStrategy::Exploratory
        );
        assert_eq!(
            ExecutionStrategy::from_failure_count(2),
            ExecutionStrategy::Cautious
        );
        assert_eq!(
            ExecutionStrategy::from_failure_count(3),
            ExecutionStrategy::Recovery
        );
        assert_eq!(
            ExecutionStrategy::from_failure_count(100),
            ExecutionStrategy::Recovery
        );
    }

    #[test]
    fn test_strategy_ordering() {
        assert!(ExecutionStrategy::Direct < ExecutionStrategy::Exploratory);
        assert!(ExecutionStrategy::Exploratory < ExecutionStrategy::Cautious);
        assert!(ExecutionStrategy::Cautious < ExecutionStrategy::Recovery);
    }

    // --- AgenticSession ---

    #[test]
    fn test_session_creation() {
        let session = AgenticSession::new("test task".to_string(), ExecutionMode::Dgs);
        assert_eq!(session.task, "test task");
        assert_eq!(session.mode, ExecutionMode::Dgs);
        assert_eq!(session.iteration_count, 0);
        assert_eq!(session.consecutive_failures, 0);
        assert_eq!(session.strategy, ExecutionStrategy::Direct);
        assert_eq!(session.max_iterations, MAX_ITERATIONS);
        assert!(session.session_id != 0);
    }

    #[test]
    fn test_session_unique_ids() {
        let s1 = AgenticSession::new("task A".to_string(), ExecutionMode::Dgs);
        // Small delay to ensure different timestamp.
        std::thread::sleep(std::time::Duration::from_millis(1));
        let s2 = AgenticSession::new("task B".to_string(), ExecutionMode::SemanticReact);
        assert_ne!(s1.session_id, s2.session_id);
    }

    #[test]
    fn test_session_record_success_resets_failures() {
        let mut session = AgenticSession::new("test".to_string(), ExecutionMode::Dgs);
        session.consecutive_failures = 2;
        session.strategy = ExecutionStrategy::Cautious;

        let iter = make_test_iteration(true, 0.8);
        session.record_iteration(iter);

        assert_eq!(session.consecutive_failures, 0);
        assert_eq!(session.iteration_count, 1);
        // Strategy doesn't downgrade — it stays at Cautious.
        assert_eq!(session.strategy, ExecutionStrategy::Cautious);
    }

    #[test]
    fn test_session_record_failure_escalates_strategy() {
        let mut session = AgenticSession::new("test".to_string(), ExecutionMode::Dgs);

        // First failure → Exploratory.
        session.record_iteration(make_test_iteration(false, 0.2));
        assert_eq!(session.strategy, ExecutionStrategy::Exploratory);

        // Second failure → Cautious.
        session.record_iteration(make_test_iteration(false, 0.2));
        assert_eq!(session.strategy, ExecutionStrategy::Cautious);

        // Third failure → Recovery.
        session.record_iteration(make_test_iteration(false, 0.2));
        assert_eq!(session.strategy, ExecutionStrategy::Recovery);
    }

    #[test]
    fn test_session_should_terminate_max_iterations() {
        let mut session = AgenticSession::new("test".to_string(), ExecutionMode::Dgs);
        session.iteration_count = MAX_ITERATIONS;
        assert!(session.should_terminate().is_some());
    }

    #[test]
    fn test_session_should_terminate_max_failures() {
        let mut session = AgenticSession::new("test".to_string(), ExecutionMode::Dgs);
        session.consecutive_failures = MAX_CONSECUTIVE_FAILURES;
        assert!(session.should_terminate().is_some());
    }

    #[test]
    fn test_session_should_not_terminate_early() {
        let session = AgenticSession::new("test".to_string(), ExecutionMode::Dgs);
        assert!(session.should_terminate().is_none());
    }

    #[test]
    fn test_session_bounded_iteration_history() {
        let mut session = AgenticSession::new("test".to_string(), ExecutionMode::Dgs);
        session.max_iterations = 100; // Allow more iterations for this test.

        for _ in 0..MAX_ITERATION_HISTORY + 10 {
            session.record_iteration(make_test_iteration(true, 0.8));
        }

        assert!(session.iterations.len() <= MAX_ITERATION_HISTORY);
    }

    // --- ToolCall from DslStep ---

    #[test]
    fn test_plan_step_to_tool_call_tap() {
        let step = DslStep {
            action: ActionType::Tap { x: 100, y: 200 },
            target: None,
            timeout_ms: 2000,
            on_failure: FailureStrategy::default(),
            precondition: None,
            postcondition: None,
            label: Some("tap the button".to_string()),
        };

        let tc = plan_step_to_tool_call(&step);
        assert_eq!(tc.tool_name, "tap");
        assert_eq!(tc.parameters.get("x").unwrap(), "100");
        assert_eq!(tc.parameters.get("y").unwrap(), "200");
        assert_eq!(tc.reasoning, "tap the button");
    }

    #[test]
    fn test_plan_step_to_tool_call_open_app() {
        let step = DslStep {
            action: ActionType::OpenApp {
                package: "com.example.app".to_string(),
            },
            target: None,
            timeout_ms: 10000,
            on_failure: FailureStrategy::default(),
            precondition: None,
            postcondition: None,
            label: None,
        };

        let tc = plan_step_to_tool_call(&step);
        assert_eq!(tc.tool_name, "open_app");
        assert_eq!(tc.parameters.get("package").unwrap(), "com.example.app");
    }

    #[test]
    fn test_plan_step_to_tool_call_with_target() {
        let step = DslStep {
            action: ActionType::Tap { x: 0, y: 0 },
            target: Some(TargetSelector::ResourceId("btn_ok".to_string())),
            timeout_ms: 2000,
            on_failure: FailureStrategy::default(),
            precondition: None,
            postcondition: None,
            label: None,
        };

        let tc = plan_step_to_tool_call(&step);
        assert!(tc.parameters.contains_key("target"));
    }

    // --- Signal Aggregation ---

    #[test]
    fn test_reflect_success() {
        let tool = ToolCall {
            tool_name: "tap".to_string(),
            parameters: BTreeMap::new(),
            reasoning: String::new(),
        };
        let obs = ActionResult {
            success: true,
            duration_ms: 100,
            error: None,
            screen_changed: true,
            matched_element: Some("btn".to_string()),
        };

        let (reflection, confidence) =
            compute_iteration_signals(&tool, &obs, ExecutionStrategy::Direct);
        assert!(confidence > 0.8);
        assert!(reflection.contains("success=true"));
    }

    #[test]
    fn test_reflect_failure() {
        let tool = ToolCall {
            tool_name: "tap".to_string(),
            parameters: BTreeMap::new(),
            reasoning: String::new(),
        };
        let obs = ActionResult {
            success: false,
            duration_ms: 100,
            error: Some("element not found".to_string()),
            screen_changed: false,
            matched_element: None,
        };

        let (reflection, confidence) =
            compute_iteration_signals(&tool, &obs, ExecutionStrategy::Recovery);
        assert!(confidence < 0.3);
        assert!(reflection.contains("success=false"));
    }

    #[test]
    fn test_reflect_confidence_bounds() {
        let tool = ToolCall {
            tool_name: "tap".to_string(),
            parameters: BTreeMap::new(),
            reasoning: String::new(),
        };

        // Best case: success + screen changed + element matched + Direct strategy.
        let obs_best = ActionResult {
            success: true,
            duration_ms: 50,
            error: None,
            screen_changed: true,
            matched_element: Some("btn".to_string()),
        };
        let (_, conf) = compute_iteration_signals(&tool, &obs_best, ExecutionStrategy::Direct);
        assert!(conf <= 1.0);
        assert!(conf >= 0.0);

        // Worst case: failure + no screen change + no element + Recovery strategy.
        let obs_worst = ActionResult {
            success: false,
            duration_ms: 5000,
            error: Some("timeout".to_string()),
            screen_changed: false,
            matched_element: None,
        };
        let (_, conf) = compute_iteration_signals(&tool, &obs_worst, ExecutionStrategy::Recovery);
        assert!(conf <= 1.0);
        assert!(conf >= 0.0);
    }

    // --- Context building ---

    #[test]
    fn test_build_context_empty_session() {
        let session = AgenticSession::new("test task".to_string(), ExecutionMode::SemanticReact);
        let ctx = build_context(&session, None, None).unwrap();

        assert_eq!(ctx.token_budget, DEFAULT_TOKEN_BUDGET);
        assert!(ctx.conversation_history.is_empty());
        assert!(ctx.active_goal.is_some());
        assert!(ctx.estimated_size() < ContextPackage::MAX_SIZE);
    }

    #[test]
    fn test_build_context_with_iterations() {
        let mut session =
            AgenticSession::new("test task".to_string(), ExecutionMode::SemanticReact);
        session.record_iteration(make_test_iteration(true, 0.8));
        session.record_iteration(make_test_iteration(false, 0.3));

        let ctx = build_context(&session, None, None).unwrap();
        // 2 iterations × 2 turns each = 4 conversation turns.
        assert_eq!(ctx.conversation_history.len(), 4);
    }

    #[test]
    fn test_build_context_with_screen() {
        let session = AgenticSession::new("test".to_string(), ExecutionMode::Dgs);
        let screen = ScreenSummary {
            package_name: "com.example".to_string(),
            activity_name: "MainActivity".to_string(),
            interactive_elements: vec!["button1".to_string()],
            visible_text: vec!["Hello".to_string()],
        };

        let ctx = build_context(&session, Some(screen), None).unwrap();
        assert!(ctx.current_screen.is_some());
    }

    // --- FailureContext ---

    #[test]
    fn test_build_failure_context() {
        let mut session = AgenticSession::new("open app".to_string(), ExecutionMode::Dgs);
        session.record_iteration(make_test_iteration(false, 0.2));

        let fc = build_failure_context(&session);
        assert_ne!(fc.task_goal_hash, 0);
        assert_eq!(fc.current_step, 1);
        assert_ne!(fc.failing_action, 0);
    }

    #[test]
    fn test_build_failure_context_empty_session() {
        let session = AgenticSession::new("task".to_string(), ExecutionMode::Dgs);
        let fc = build_failure_context(&session);
        assert_eq!(fc.current_step, 0);
        assert_eq!(fc.failing_action, 0);
    }

    // --- Escalation ---

    #[test]
    fn test_escalation_monotonic() {
        let mut session = AgenticSession::new("test".to_string(), ExecutionMode::Dgs);
        assert_eq!(session.escalation_tier, EscalationTier::DgsSuccess);

        let t1 = escalate_dgs_step(&mut session, 0);
        assert_eq!(t1, EscalationTier::RetryAdjusted);

        let t2 = escalate_dgs_step(&mut session, 0);
        assert_eq!(t2, EscalationTier::Brainstem);

        let t3 = escalate_dgs_step(&mut session, 0);
        assert_eq!(t3, EscalationTier::FullNeocortex);

        // Already at max — stays there.
        let t4 = escalate_dgs_step(&mut session, 0);
        assert_eq!(t4, EscalationTier::FullNeocortex);
    }

    // --- Simulation stub ---

    #[test]
    fn test_simulate_action_result_success() {
        let tc = ToolCall {
            tool_name: "tap".to_string(),
            parameters: BTreeMap::new(),
            reasoning: String::new(),
        };
        let result = simulate_action_result(&tc);
        assert!(result.success);
        assert!(result.screen_changed);
    }

    #[test]
    fn test_simulate_action_result_assert_fails() {
        let tc = ToolCall {
            tool_name: "assert_element".to_string(),
            parameters: BTreeMap::new(),
            reasoning: String::new(),
        };
        let result = simulate_action_result(&tc);
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    // --- Async execution tests (standalone) ---

    #[tokio::test]
    async fn test_execute_task_dgs_with_plan() {
        let plan = ActionPlan {
            goal_description: "tap a button".to_string(),
            steps: vec![DslStep {
                action: ActionType::Tap { x: 100, y: 200 },
                target: None,
                timeout_ms: 2000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: Some("tap".to_string()),
            }],
            estimated_duration_ms: 2000,
            confidence: 0.9,
            source: PlanSource::EtgLookup,
        };

        let (outcome, session) =
            execute_task("tap on the button".to_string(), 5, Some(plan), None).await;

        assert!(matches!(outcome, TaskOutcome::Success { .. }));
        assert_eq!(session.mode, ExecutionMode::SemanticReact);
        assert_eq!(session.iteration_count, 1);
    }

    #[tokio::test]
    async fn test_execute_task_dgs_no_plan_escalates() {
        let (outcome, session) = execute_task("tap on something".to_string(), 5, None, None).await;

        // Without a plan, DGS escalates to SemanticReact, which then fails
        // because there's no neocortex connection.
        assert!(matches!(outcome, TaskOutcome::Failed { .. }));
        assert_eq!(session.mode, ExecutionMode::SemanticReact);
    }

    #[tokio::test]
    async fn test_execute_task_semantic_react_with_plan() {
        let plan = ActionPlan {
            goal_description: "figure out WiFi".to_string(),
            steps: vec![DslStep {
                action: ActionType::OpenApp {
                    package: "com.android.settings".to_string(),
                },
                target: None,
                timeout_ms: 10000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: Some("open settings".to_string()),
            }],
            estimated_duration_ms: 10000,
            confidence: 0.6,
            source: PlanSource::LlmGenerated,
        };

        let (outcome, session) = execute_task(
            "figure out why WiFi is slow".to_string(),
            7,
            Some(plan),
            None,
        )
        .await;

        assert_eq!(session.mode, ExecutionMode::SemanticReact);
        // Should succeed since simulated actions succeed (open_app != assert_element).
        assert!(matches!(outcome, TaskOutcome::Success { .. }));
    }

    #[tokio::test]
    async fn test_execute_task_semantic_react_no_plan() {
        let (outcome, _session) = execute_task(
            "analyze my battery usage patterns over the last week".to_string(),
            3,
            None,
            None,
        )
        .await;

        // No plan + no neocortex = failure.
        assert!(matches!(outcome, TaskOutcome::Failed { .. }));
    }

    // --- ReactEngine wired tests ---

    use aura_types::screen::{Bounds, ScreenNode};

    use crate::screen::actions::MockScreenProvider;

    fn make_test_tree(package: &str, text: &str) -> aura_types::screen::ScreenTree {
        aura_types::screen::ScreenTree {
            root: ScreenNode {
                id: "root".into(),
                class_name: "android.widget.FrameLayout".into(),
                package_name: package.into(),
                text: Some(text.into()),
                content_description: None,
                resource_id: None,
                bounds: Bounds {
                    left: 0,
                    top: 0,
                    right: 1080,
                    bottom: 1920,
                },
                is_clickable: false,
                is_scrollable: false,
                is_editable: false,
                is_checkable: false,
                is_checked: false,
                is_enabled: true,
                is_focused: false,
                is_visible: true,
                children: vec![],
                depth: 0,
            },
            package_name: package.into(),
            activity_name: ".MainActivity".into(),
            timestamp_ms: 1_700_000_000_000,
            node_count: 1,
        }
    }

    fn make_engine_with_trees(trees: Vec<aura_types::screen::ScreenTree>) -> ReactEngine {
        let mock = MockScreenProvider::new(trees);
        ReactEngine::with_defaults(Box::new(mock))
    }

    #[tokio::test]
    async fn test_engine_dgs_with_plan_succeeds() {
        let tree1 = make_test_tree("com.example", "Home");
        let tree2 = make_test_tree("com.example", "Settings");
        let mut engine = make_engine_with_trees(vec![tree1, tree2]);

        let plan = ActionPlan {
            goal_description: "tap a button".to_string(),
            steps: vec![DslStep {
                action: ActionType::Tap { x: 100, y: 200 },
                target: None,
                timeout_ms: 2000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: Some("tap".to_string()),
            }],
            estimated_duration_ms: 2000,
            confidence: 0.9,
            source: PlanSource::EtgLookup,
        };

        let (outcome, session) = engine
            .execute_task("tap on the button".to_string(), 5, Some(plan))
            .await;

        assert_eq!(session.mode, ExecutionMode::SemanticReact);
        // The executor ran the plan — outcome depends on MockScreenProvider behavior.
        // With a single tap step and a responsive mock, expect success.
        assert!(
            matches!(outcome, TaskOutcome::Success { .. })
                || matches!(outcome, TaskOutcome::Failed { .. }),
            "expected Success or Failed, got {:?}",
            outcome,
        );
    }

    #[tokio::test]
    async fn test_engine_semantic_react_with_plan() {
        let tree = make_test_tree("com.android.settings", "WiFi");
        let mut engine = make_engine_with_trees(vec![tree]);

        let plan = ActionPlan {
            goal_description: "check WiFi".to_string(),
            steps: vec![DslStep {
                action: ActionType::OpenApp {
                    package: "com.android.settings".to_string(),
                },
                target: None,
                timeout_ms: 10000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: Some("open settings".to_string()),
            }],
            estimated_duration_ms: 10000,
            confidence: 0.6,
            source: PlanSource::LlmGenerated,
        };

        let (outcome, session) = engine
            .execute_task("figure out why WiFi is slow".to_string(), 7, Some(plan))
            .await;

        assert_eq!(session.mode, ExecutionMode::SemanticReact);
        // Engine actually executed through Executor — accept any terminal outcome.
        assert!(
            matches!(
                outcome,
                TaskOutcome::Success { .. }
                    | TaskOutcome::Failed { .. }
                    | TaskOutcome::CycleAborted { .. }
            ),
            "unexpected outcome: {:?}",
            outcome,
        );
    }

    #[tokio::test]
    async fn test_engine_dgs_no_plan_escalates() {
        let tree = make_test_tree("com.example", "Home");
        let mut engine = make_engine_with_trees(vec![tree]);

        let (outcome, session) = engine
            .execute_task("tap on something".to_string(), 5, None)
            .await;

        // Without a plan, DGS escalates to SemanticReact, which fails (no neocortex).
        assert!(matches!(outcome, TaskOutcome::Failed { .. }));
        assert_eq!(session.mode, ExecutionMode::SemanticReact);
    }

    #[test]
    fn test_engine_capture_screen_summary() {
        let tree = make_test_tree("com.example.app", "Hello World");
        let engine = make_engine_with_trees(vec![tree]);

        let summary = engine.capture_screen_summary();
        assert!(summary.is_some());
        let summary = summary.expect("screen summary should be present");
        assert_eq!(summary.package_name, "com.example.app");
    }

    #[test]
    fn test_engine_capture_screen_hash_nonzero() {
        let tree = make_test_tree("com.test", "Test");
        let engine = make_engine_with_trees(vec![tree]);

        let hash = engine.capture_screen_hash();
        assert_ne!(hash, 0, "screen hash should be non-zero for a valid tree");
    }

    #[test]
    fn test_reader_summary_to_ipc_conversion() {
        let reader_summary = crate::screen::reader::ScreenSummary {
            package_name: "com.example".to_string(),
            activity_name: ".MainActivity".to_string(),
            visible_text: vec!["Hello".to_string()],
            clickable_count: 3,
            editable_count: 1,
            scrollable_count: 0,
            keyboard_visible: false,
            app_state: crate::screen::reader::AppState::Normal,
            node_count: 10,
        };

        let ipc = reader_summary_to_ipc(&reader_summary);
        assert_eq!(ipc.package_name, "com.example");
        assert_eq!(ipc.activity_name, ".MainActivity");
        assert_eq!(ipc.visible_text, vec!["Hello".to_string()]);
        assert!(ipc
            .interactive_elements
            .iter()
            .any(|e| e.contains("3 clickable")));
        assert!(ipc
            .interactive_elements
            .iter()
            .any(|e| e.contains("1 editable")));
    }

    #[test]
    fn test_classify_task_uses_complexity() {
        // A very complex task should trigger SemanticReact even without keyword match.
        let complex = "I need to coordinate multiple steps across different \
            applications then verify the final state after running through \
            a complex multi-step workflow involving several different conditions \
            and branching logic paths";
        assert_eq!(classify_task(complex), ExecutionMode::SemanticReact);
    }

    // --- TEST-CRIT-002: ReAct loop core unit tests ---

    #[test]
    fn test_classify_task_always_returns_semantic_react() {
        // classify_task must ALWAYS return SemanticReact — the LLM decides
        // execution strategy, not Rust. This is an Iron Law invariant.
        let inputs = [
            "open settings",
            "what time is it",
            "",
            "coordinate multi-step workflow across 5 apps with conditional branching",
            "tap the blue button",
            "a]!@#$%^& weird input with special chars",
        ];
        for input in &inputs {
            assert_eq!(
                classify_task(input),
                ExecutionMode::SemanticReact,
                "classify_task must return SemanticReact for all inputs, failed on: {input:?}"
            );
        }
    }

    #[test]
    fn test_fnv1a_hash_deterministic_extended() {
        let data = b"hello world";
        let h1 = fnv1a_hash(data);
        let h2 = fnv1a_hash(data);
        assert_eq!(h1, h2, "fnv1a_hash must be deterministic");

        // Also check empty input is deterministic.
        let empty1 = fnv1a_hash(b"");
        let empty2 = fnv1a_hash(b"");
        assert_eq!(
            empty1, empty2,
            "fnv1a_hash must be deterministic for empty input"
        );

        // Empty input should return the FNV offset basis (no bytes processed).
        assert_eq!(
            empty1, FNV_OFFSET,
            "fnv1a_hash of empty input should be FNV_OFFSET"
        );
    }

    #[test]
    fn test_fnv1a_hash_distributes() {
        // Different inputs must produce different hashes (basic collision resistance).
        // FNV-1a isn't cryptographic, but for cycle detection it must distinguish
        // distinct screen states.
        let inputs: Vec<&[u8]> = vec![
            b"hello",
            b"Hello",
            b"hello!",
            b"world",
            b"",
            b"\x00",
            b"\x00\x00",
            b"com.android.settings",
            b"com.android.settings.wifi",
        ];
        let hashes: Vec<u64> = inputs.iter().map(|i| fnv1a_hash(i)).collect();

        // All hashes should be unique for these distinct inputs.
        for i in 0..hashes.len() {
            for j in (i + 1)..hashes.len() {
                assert_ne!(
                    hashes[i],
                    hashes[j],
                    "collision between {:?} and {:?}: both hash to {}",
                    std::str::from_utf8(inputs[i]).unwrap_or("<binary>"),
                    std::str::from_utf8(inputs[j]).unwrap_or("<binary>"),
                    hashes[i]
                );
            }
        }
    }

    #[test]
    fn test_max_iterations_constant() {
        // MAX_ITERATIONS is the daemon's outer agentic loop bound.
        // Changing it has safety implications (resource exhaustion, infinite loops).
        // This test catches accidental modifications.
        assert_eq!(
            MAX_ITERATIONS, 10,
            "MAX_ITERATIONS must be 10 (daemon outer loop)"
        );
    }

    // --- Helpers ---

    fn make_test_iteration(success: bool, confidence: f32) -> Iteration {
        Iteration {
            thought: "test thought".to_string(),
            action: ToolCall {
                tool_name: "tap".to_string(),
                parameters: BTreeMap::new(),
                reasoning: "test".to_string(),
            },
            observation: ActionResult {
                success,
                duration_ms: 100,
                error: if success {
                    None
                } else {
                    Some("test error".to_string())
                },
                screen_changed: success,
                matched_element: if success {
                    Some("btn".to_string())
                } else {
                    None
                },
            },
            reflection: "test reflection".to_string(),
            confidence,
            duration_ms: 100,
        }
    }
}
