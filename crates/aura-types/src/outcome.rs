//! Execution outcome types for the OutcomeBus feedback loop.
//!
//! Every execution path in the daemon produces an `ExecutionOutcome`
//! that is published to the OutcomeBus, which dispatches it to the
//! learning engine, episodic memory, BDI scheduler, identity engine,
//! anti-sycophancy guard, and dreaming engine.
//!
//! These types live in `aura-types` so both the daemon and neocortex
//! crates can reference them without circular dependencies.

use serde::{Deserialize, Serialize};

/// Rich outcome of any execution path through the daemon.
///
/// Captures everything downstream subscribers need:
/// - What was intended (intent classification + input summary)
/// - How it was routed (System1/System2/Hybrid/ReAct/Proactive)
/// - What happened (success/failure/partial/cancelled/blocked/timeout)
/// - How long it took
/// - How confident the system was
/// - What the user did next (reaction detection — populated asynchronously)
/// - Goal linkage for BDI scheduler updates
/// - Domain for cross-domain learning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionOutcome {
    /// What the user asked for (normalized intent label).
    pub intent: String,

    /// The execution result.
    pub result: OutcomeResult,

    /// Wall-clock duration of the execution in milliseconds.
    pub duration_ms: u64,

    /// System's confidence in the result (0.0–1.0).
    /// For System1: cache hit confidence.
    /// For System2: neocortex plan confidence.
    /// For ReAct: final_confidence from TaskOutcome.
    pub confidence: f32,

    /// Which execution route was taken.
    pub route: RouteKind,

    /// User's reaction to this outcome (initially NoReaction, updated
    /// asynchronously by the reaction detection window).
    pub user_reaction: UserReaction,

    /// Truncated summary of the user's input (max 256 chars).
    /// Used by episodic memory and dreaming for context.
    pub input_summary: String,

    /// Truncated summary of AURA's response (max 512 chars).
    /// Used by anti-sycophancy enrichment and episodic memory.
    pub response_summary: String,

    /// Timestamp when execution started (epoch ms).
    pub started_at_ms: u64,

    /// Timestamp when execution completed (epoch ms).
    pub completed_at_ms: u64,

    /// Optional goal ID if this outcome is linked to a BDI goal.
    pub goal_id: Option<u64>,

    /// Life domain this outcome relates to (if determinable).
    /// Maps to `DomainId` in the arc module, stored as u8 for type portability.
    pub domain: Option<u8>,

    /// Route classifier confidence (how sure the classifier was about routing).
    pub route_confidence: f32,

    /// Number of ReAct iterations used (0 for non-ReAct routes).
    pub react_iterations: u32,

    /// Whether this execution involved a fallback (e.g., System2 fell back to System1).
    pub was_fallback: bool,
}

impl ExecutionOutcome {
    /// Maximum length for input summaries stored in outcomes.
    pub const MAX_INPUT_SUMMARY: usize = 256;

    /// Maximum length for response summaries stored in outcomes.
    pub const MAX_RESPONSE_SUMMARY: usize = 512;

    /// Create a new outcome with required fields; optional fields default.
    pub fn new(
        intent: String,
        result: OutcomeResult,
        duration_ms: u64,
        confidence: f32,
        route: RouteKind,
        started_at_ms: u64,
    ) -> Self {
        Self {
            intent,
            result,
            duration_ms,
            confidence: confidence.clamp(0.0, 1.0),
            route,
            user_reaction: UserReaction::NoReaction,
            input_summary: String::new(),
            response_summary: String::new(),
            started_at_ms,
            completed_at_ms: started_at_ms.saturating_add(duration_ms),
            goal_id: None,
            domain: None,
            route_confidence: 0.0,
            react_iterations: 0,
            was_fallback: false,
        }
    }

    /// Set the input summary, truncating to [`MAX_INPUT_SUMMARY`] bytes.
    pub fn with_input_summary(mut self, summary: &str) -> Self {
        self.input_summary = truncate_string(summary, Self::MAX_INPUT_SUMMARY);
        self
    }

    /// Set the response summary, truncating to [`MAX_RESPONSE_SUMMARY`] bytes.
    pub fn with_response_summary(mut self, summary: &str) -> Self {
        self.response_summary = truncate_string(summary, Self::MAX_RESPONSE_SUMMARY);
        self
    }

    /// Set the goal ID for BDI scheduler linkage.
    pub fn with_goal(mut self, goal_id: u64) -> Self {
        self.goal_id = Some(goal_id);
        self
    }

    /// Set the domain for cross-domain learning.
    pub fn with_domain(mut self, domain_id: u8) -> Self {
        self.domain = Some(domain_id);
        self
    }

    /// Set the route classifier confidence.
    pub fn with_route_confidence(mut self, confidence: f32) -> Self {
        self.route_confidence = confidence.clamp(0.0, 1.0);
        self
    }

    /// Set the ReAct iteration count.
    pub fn with_react_iterations(mut self, iterations: u32) -> Self {
        self.react_iterations = iterations;
        self
    }

    /// Set the user reaction classification.
    pub fn with_user_reaction(mut self, reaction: UserReaction) -> Self {
        self.user_reaction = reaction;
        self
    }

    /// Mark this outcome as a fallback route.
    pub fn with_fallback(mut self) -> Self {
        self.was_fallback = true;
        self
    }

    /// Whether the outcome represents a successful execution.
    pub fn is_success(&self) -> bool {
        matches!(self.result, OutcomeResult::Success | OutcomeResult::PartialSuccess)
    }

    /// Whether the user reacted positively.
    pub fn is_positive_reaction(&self) -> bool {
        matches!(self.user_reaction, UserReaction::ExplicitPositive)
    }

    /// Compute a simple effectiveness score (0.0–1.0) combining
    /// result, confidence, and user reaction.
    ///
    /// Formula:
    /// ```text
    /// base = result_weight * 0.5 + confidence * 0.3 + reaction_weight * 0.2
    /// ```
    ///
    /// This is NOT hardcoded intelligence — it's a utility function
    /// for subscribers that need a single scalar from the outcome.
    /// The learning engine uses the full outcome struct for Hebbian/Bayesian updates.
    pub fn effectiveness_score(&self) -> f32 {
        let result_weight = match self.result {
            OutcomeResult::Success => 1.0,
            OutcomeResult::PartialSuccess => 0.6,
            OutcomeResult::Failure => 0.0,
            OutcomeResult::UserCancelled => 0.3,
            OutcomeResult::PolicyBlocked => 0.1,
            OutcomeResult::Timeout => 0.05,
        };
        let reaction_weight = match self.user_reaction {
            UserReaction::ExplicitPositive => 1.0,
            UserReaction::FollowUp => 0.7,
            UserReaction::NoReaction => 0.5,
            UserReaction::TopicChange => 0.3,
            UserReaction::Repetition => 0.1,
            UserReaction::ExplicitNegative => 0.0,
        };
        (result_weight * 0.5 + self.confidence * 0.3 + reaction_weight * 0.2).clamp(0.0, 1.0)
    }
}

/// The execution result — what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutcomeResult {
    /// Execution completed successfully.
    Success,
    /// Execution partially succeeded (e.g., System1 responded but plan failed).
    PartialSuccess,
    /// Execution failed.
    Failure,
    /// User cancelled the execution.
    UserCancelled,
    /// Policy gate blocked the execution.
    PolicyBlocked,
    /// Execution timed out.
    Timeout,
}

/// Which execution route was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteKind {
    /// Fast path — System1 ETG cache, no LLM.
    System1,
    /// Slow path — System2 neocortex LLM.
    System2,
    /// Daemon tried System1 first, fell back to System2.
    Hybrid,
    /// Full ReAct agentic loop (Think→Act→Observe→Reflect).
    React,
    /// Proactive engine initiated (not user-triggered).
    Proactive,
    /// Daemon-internal only (log/suppress, no user-visible action).
    DaemonOnly,
}

/// User's reaction to an outcome.
///
/// Populated asynchronously by the reaction detection window
/// (30-second observation after response delivery). The initial
/// value is always `NoReaction`; the OutcomeBus updates it when
/// the next user input arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserReaction {
    /// User expressed explicit satisfaction ("thanks", "great", "perfect").
    ExplicitPositive,
    /// User expressed explicit dissatisfaction ("no", "wrong", "that's not what I meant").
    ExplicitNegative,
    /// User asked a follow-up question in the same topic (implicit positive).
    FollowUp,
    /// User changed topic entirely (implicit neutral/negative).
    TopicChange,
    /// User repeated the same request (implicit failure signal).
    Repetition,
    /// No user reaction within the observation window.
    NoReaction,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Truncate a string to at most `max_bytes`, cutting at a char boundary.
fn truncate_string(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_owned();
    }
    // Find the last char boundary at or before max_bytes.
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_new_clamps_confidence() {
        let o = ExecutionOutcome::new(
            "test".into(),
            OutcomeResult::Success,
            100,
            1.5, // should be clamped to 1.0
            RouteKind::System1,
            1000,
        );
        assert_eq!(o.confidence, 1.0);
        assert_eq!(o.completed_at_ms, 1100);
    }

    #[test]
    fn effectiveness_score_range() {
        let o = ExecutionOutcome::new(
            "test".into(),
            OutcomeResult::Success,
            100,
            0.9,
            RouteKind::System1,
            1000,
        );
        let score = o.effectiveness_score();
        assert!(score >= 0.0 && score <= 1.0, "score out of range: {}", score);
    }

    #[test]
    fn truncate_string_at_char_boundary() {
        let s = "Hello 世界!"; // 世 = 3 bytes, 界 = 3 bytes
        let truncated = truncate_string(s, 8);
        // "Hello " = 6 bytes, "世" = 3 bytes = 9 bytes total
        // At 8 bytes, we must cut before "世" → "Hello "
        assert_eq!(truncated, "Hello ");
    }

    #[test]
    fn truncate_short_string_unchanged() {
        let s = "Hi";
        assert_eq!(truncate_string(s, 100), "Hi");
    }
}
