//! OutcomeBus — closes the feedback loop between execution and learning.
//!
//! Every execution path (System1, System2, Hybrid, ReAct, Proactive) produces
//! an [`ExecutionOutcome`] that is published to the bus.  On each dispatch
//! cycle the bus drains its buffer and fans out to 6 subscriber functions:
//!
//! 1. **Learning + Dreaming** — Hebbian associations, dimension features,
//!    skill tracking, ETG trace recording, capability gap detection.
//! 2. **Episodic Memory** — stores the interaction as a retrievable episode.
//! 3. **BDI Goals** — updates beliefs with execution evidence.
//! 4. **Identity** — adjusts relationship trust via interaction recording.
//! 5. **Anti-Sycophancy** — enriches the sycophancy guard with outcome-derived
//!    behavioural signals (overconfidence, fallback patterns).
//!
//! ## Ownership
//!
//! The bus does NOT hold references to subsystems.  Instead, `dispatch()` takes
//! individual `&mut` references via split borrows from the caller, who
//! destructures [`LoopSubsystems`] before calling.  This satisfies the borrow
//! checker without introducing `Arc<Mutex<>>` or trait-based indirection.

use tracing::{debug, info, warn};

use aura_types::outcome::{ExecutionOutcome, OutcomeResult, RouteKind};

use crate::arc::learning::{Feature, Outcome as LearningOutcome};
use crate::arc::ArcManager;
use crate::goals::scheduler::{BdiScheduler, Belief, BeliefSource};
use crate::identity::anti_sycophancy::ResponseRecord;
use crate::identity::relationship::InteractionType;
use crate::identity::IdentityEngine;
use crate::memory::AuraMemory;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Maximum number of outcomes buffered before forced drain.
/// Safety valve — if the loop forgets to dispatch, we don't OOM.
const MAX_PENDING: usize = 256;

/// User ID used for system-level interaction recording.
/// AURA is a single-user system; this identifies the primary user.
const SYSTEM_USER_ID: &str = "primary";

// ---------------------------------------------------------------------------
// OutcomeBus
// ---------------------------------------------------------------------------

/// Collects execution outcomes and dispatches them to cognitive subsystems.
///
/// The bus operates in a publish-then-dispatch cycle:
/// 1. Execution paths call `publish()` to buffer an outcome.
/// 2. At end of each event-loop iteration, the loop calls `dispatch()`.
/// 3. `dispatch()` drains the buffer and fans out to all subscribers.
#[derive(Debug)]
pub struct OutcomeBus {
    /// Buffered outcomes awaiting dispatch.
    pending: Vec<ExecutionOutcome>,
    /// Lifetime count of successfully dispatched outcomes.
    pub total_dispatched: u64,
    /// Lifetime count of dispatch errors (non-fatal).
    pub dispatch_errors: u64,
    /// Rolling window of recent success count (reset after read).
    recent_successes: u32,
    /// Rolling window of recent failure count (reset after read).
    recent_failures: u32,
    /// Per-outcome capability summaries from the last dispatch cycle.
    /// Each entry is `(capability_id, succeeded)`.  Populated during
    /// `dispatch()`, consumed by `drain_capability_outcomes()` in the
    /// caller's post-dispatch wiring (e.g. GoalRegistry.update_confidence).
    recent_capability_outcomes: Vec<(String, bool)>,
}

impl OutcomeBus {
    /// Create a new, empty outcome bus.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: Vec::with_capacity(16),
            total_dispatched: 0,
            dispatch_errors: 0,
            recent_successes: 0,
            recent_failures: 0,
            recent_capability_outcomes: Vec::new(),
        }
    }

    /// Buffer an outcome for later dispatch.
    ///
    /// If the buffer exceeds [`MAX_PENDING`], the oldest outcome is dropped
    /// to prevent unbounded growth.  This should never happen under normal
    /// operation — it indicates the loop is not calling `dispatch()`.
    pub fn publish(&mut self, outcome: ExecutionOutcome) {
        if self.pending.len() >= MAX_PENDING {
            warn!(
                dropped_intent = %self.pending[0].intent,
                buffer_size = MAX_PENDING,
                "OutcomeBus buffer overflow — dropping oldest outcome"
            );
            self.pending.remove(0);
            self.dispatch_errors = self.dispatch_errors.saturating_add(1);
        }
        debug!(
            intent = %outcome.intent,
            result = ?outcome.result,
            route = ?outcome.route,
            duration_ms = outcome.duration_ms,
            "outcome published to bus"
        );
        self.pending.push(outcome);
    }

    /// Number of outcomes waiting to be dispatched.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Drain all pending outcomes without dispatching to any subscriber.
    ///
    /// Used when safe mode is active: we clear the buffer to prevent
    /// unbounded growth but intentionally skip learning, memory, BDI,
    /// identity, and anti-sycophancy dispatch to avoid propagating
    /// potentially corrupt state.
    pub fn drain_all(&mut self) {
        self.pending.clear();
    }

    /// Returns (successes, failures) counted since the last call, then resets
    /// the counters. This provides a rolling feedback signal for systems like
    /// `SemanticReact` that need to adapt thresholds based on recent outcomes.
    pub fn recent_success_failure_counts(&mut self) -> (u32, u32) {
        let s = self.recent_successes;
        let f = self.recent_failures;
        self.recent_successes = 0;
        self.recent_failures = 0;
        (s, f)
    }

    /// Drain per-outcome capability summaries from the last dispatch cycle.
    ///
    /// Returns `(capability_id, succeeded)` pairs that can be fed into
    /// `GoalRegistry::update_confidence()`.  The buffer is cleared after read.
    pub fn drain_capability_outcomes(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.recent_capability_outcomes)
    }

    /// Drain all pending outcomes and dispatch to cognitive subsystems.
    ///
    /// Takes individual subsystem references via split borrows.  The caller
    /// destructures `LoopSubsystems` before calling this method:
    ///
    /// ```ignore
    /// let LoopSubsystems {
    ///     outcome_bus, arc_manager, memory, identity, bdi_scheduler, ..
    /// } = subs;
    /// outcome_bus.dispatch(arc_manager, memory, bdi_scheduler, identity, now).await;
    /// ```
    ///
    /// Each subscriber is independent — a failure in one does NOT prevent
    /// dispatch to the others.  Errors are logged and counted.
    pub async fn dispatch(
        &mut self,
        arc_manager: Option<&mut ArcManager>,
        memory: &AuraMemory,
        bdi_scheduler: Option<&mut BdiScheduler>,
        identity: &mut IdentityEngine,
        now_ms: u64,
    ) {
        if self.pending.is_empty() {
            return;
        }

        // ── Privacy Sovereignty: check consent BEFORE dispatching ────
        // Learning and memory storage are gated on the "learning" consent
        // category.  If the user has revoked consent, we skip those
        // subscribers but still dispatch to non-learning subscribers
        // (identity trust, anti-sycophancy, BDI goals) so the system
        // remains functional.
        let learning_consented = identity.consent_tracker.has_consent("learning", now_ms);
        if !learning_consented {
            tracing::info!(
                "learning consent not granted — skipping learning/memory dispatch"
            );
        }

        let outcomes: Vec<ExecutionOutcome> = self.pending.drain(..).collect();
        let batch_size = outcomes.len();

        for outcome in &outcomes {
            // Track success/failure for SemanticReact threshold adaptation.
            match outcome.result {
                OutcomeResult::Success => {
                    self.recent_successes = self.recent_successes.saturating_add(1);
                }
                OutcomeResult::Failure | OutcomeResult::PartialSuccess => {
                    self.recent_failures = self.recent_failures.saturating_add(1);
                }
                OutcomeResult::UserCancelled => {
                    // Cancellations don't count as success or failure.
                }
            }

            // Capture capability summary for GoalRegistry wiring.
            // The outcome's intent (task description) serves as the capability_id.
            let succeeded = matches!(outcome.result, OutcomeResult::Success);
            if !matches!(outcome.result, OutcomeResult::UserCancelled) {
                self.recent_capability_outcomes.push((
                    outcome.intent.clone(),
                    succeeded,
                ));
            }

            // 1. Learning + Dreaming — gated on consent
            if learning_consented {
                if let Some(ref mut arc) = arc_manager {
                    dispatch_to_learning(arc, outcome, now_ms);
                }
            }

            // 2. Episodic Memory (async) — gated on consent (memory is learning data)
            if learning_consented {
                dispatch_to_memory(memory, outcome, now_ms).await;
            }

            // 3. BDI Goals
            if let Some(ref mut bdi) = bdi_scheduler {
                dispatch_to_goals(bdi, outcome, now_ms);
            }

            // 4. Identity (relationship tracker)
            dispatch_to_identity(identity, outcome, now_ms);

            // 5. Anti-Sycophancy enrichment
            dispatch_to_anti_sycophancy(identity, outcome);

            self.total_dispatched = self.total_dispatched.saturating_add(1);
        }

        if batch_size > 1 {
            info!(
                batch_size,
                total = self.total_dispatched,
                errors = self.dispatch_errors,
                learning_consented,
                "OutcomeBus batch dispatched"
            );
        } else {
            debug!(
                total = self.total_dispatched,
                learning_consented,
                "OutcomeBus outcome dispatched"
            );
        }
    }
}

impl Default for OutcomeBus {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Subscriber 1: Learning + Dreaming
// ===========================================================================

/// Dispatch an outcome to the learning engine and dreaming engine.
///
/// This is the richest subscriber — it feeds:
/// - Hebbian associations between intent and route concepts
/// - Dimension discovery features (time, route, result, confidence bucket)
/// - Skill registry outcome tracking
/// - ETG trace success/failure recording
/// - Capability gap detection on failures
fn dispatch_to_learning(
    arc: &mut ArcManager,
    outcome: &ExecutionOutcome,
    now_ms: u64,
) {
    let learning_outcome = map_to_learning_outcome(outcome.result);

    // --- Hebbian: associate intent with route ---
    let route_concept = route_to_concept(outcome.route);
    if let Err(e) = arc.learning.observe(
        &outcome.intent,
        &route_concept,
        learning_outcome,
        now_ms,
    ) {
        warn!(
            intent = %outcome.intent,
            error = %e,
            "learning.observe failed for intent↔route"
        );
    }

    // --- Hebbian: associate intent with result ---
    let result_concept = result_to_concept(outcome.result);
    if let Err(e) = arc.learning.observe(
        &outcome.intent,
        &result_concept,
        learning_outcome,
        now_ms,
    ) {
        warn!(
            intent = %outcome.intent,
            error = %e,
            "learning.observe failed for intent↔result"
        );
    }

    // --- Dimension discovery: build feature vector from outcome ---
    let features = build_outcome_features(outcome);
    if !features.is_empty() {
        arc.learning.observe_features(&features, now_ms);
    }

    // --- Skill registry: record outcome for the intent as a "skill" ---
    let skill_success = matches!(
        outcome.result,
        OutcomeResult::Success | OutcomeResult::PartialSuccess
    );
    arc.learning.skills.record_outcome(
        &outcome.intent,
        skill_success,
        outcome.duration_ms,
        now_ms,
    );

    // --- Dreaming: ETG trace recording ---
    match outcome.result {
        OutcomeResult::Success => {
            arc.learning.dreaming.record_trace_success(&outcome.intent, now_ms);
        }
        OutcomeResult::Failure | OutcomeResult::Timeout => {
            arc.learning.dreaming.record_trace_failure(&outcome.intent, now_ms);

            // Capability gap detection — record the failure pattern
            let app_context = route_to_concept(outcome.route);
            arc.learning.dreaming.record_failure(
                &outcome.response_summary,
                &app_context,
                now_ms,
            );
        }
        OutcomeResult::PartialSuccess => {
            // Partial success still strengthens the trace (better than nothing)
            // but with a weaker signal — record as success, the confidence
            // in the trace will naturally adjust via success_rate().
            arc.learning.dreaming.record_trace_success(&outcome.intent, now_ms);
        }
        // Cancelled/PolicyBlocked are not execution attempts, skip trace
        OutcomeResult::UserCancelled | OutcomeResult::PolicyBlocked => {}
    }

    debug!(
        intent = %outcome.intent,
        route = ?outcome.route,
        result = ?outcome.result,
        "learning+dreaming dispatch complete"
    );
}

/// Map [`OutcomeResult`] to the learning engine's tri-state [`LearningOutcome`].
///
/// The mapping is deliberate:
/// - Success/PartialSuccess → strengthens Hebbian associations
/// - Failure/Timeout → weakens Hebbian associations
/// - Cancelled/PolicyBlocked → neutral (external factors, not learning signal)
fn map_to_learning_outcome(result: OutcomeResult) -> LearningOutcome {
    match result {
        OutcomeResult::Success | OutcomeResult::PartialSuccess => LearningOutcome::Success,
        OutcomeResult::Failure | OutcomeResult::Timeout => LearningOutcome::Failure,
        OutcomeResult::UserCancelled | OutcomeResult::PolicyBlocked => LearningOutcome::Neutral,
    }
}

/// Build a [`Feature`] vector from an outcome for dimension discovery.
///
/// These features allow the dimension engine to discover emergent
/// correlations between time-of-day, route choices, result patterns,
/// and confidence levels.  The features are NOT hardcoded intelligence —
/// they are raw observables fed into Bayesian/Hebbian learning.
fn build_outcome_features(outcome: &ExecutionOutcome) -> Vec<Feature> {
    let mut features = Vec::with_capacity(4);

    // Time bucket: extract hour from epoch ms → 4 time buckets
    // (6-hour windows: dawn/morning/afternoon/evening)
    let hour = ((outcome.started_at_ms / 3_600_000) % 24) as u8;
    let time_bucket = match hour {
        0..=5 => "night",
        6..=11 => "morning",
        12..=17 => "afternoon",
        18..=23 => "evening",
        _ => unreachable!(),
    };
    features.push(Feature::TimeBucket(time_bucket.to_owned()));

    // Route as activity type
    let route_activity = match outcome.route {
        RouteKind::System1 => "fast_path",
        RouteKind::System2 => "deep_reasoning",
        RouteKind::Hybrid => "hybrid_reasoning",
        RouteKind::React => "agentic_task",
        RouteKind::Proactive => "proactive_initiative",
        RouteKind::DaemonOnly => "internal_processing",
    };
    features.push(Feature::ActivityType(route_activity.to_owned()));

    // Result as a custom feature
    let result_name = match outcome.result {
        OutcomeResult::Success => "success",
        OutcomeResult::PartialSuccess => "partial",
        OutcomeResult::Failure => "failure",
        OutcomeResult::UserCancelled => "cancelled",
        OutcomeResult::PolicyBlocked => "blocked",
        OutcomeResult::Timeout => "timeout",
    };
    features.push(Feature::Custom(
        "exec_result".to_owned(),
        result_name.to_owned(),
    ));

    // Confidence bucket as custom feature (discretise into 4 bins)
    let conf_bucket = if outcome.confidence < 0.25 {
        "low"
    } else if outcome.confidence < 0.50 {
        "medium_low"
    } else if outcome.confidence < 0.75 {
        "medium_high"
    } else {
        "high"
    };
    features.push(Feature::Custom(
        "confidence".to_owned(),
        conf_bucket.to_owned(),
    ));

    features
}

/// Convert a [`RouteKind`] to a Hebbian concept string.
fn route_to_concept(route: RouteKind) -> String {
    match route {
        RouteKind::System1 => "route:system1".to_owned(),
        RouteKind::System2 => "route:system2".to_owned(),
        RouteKind::Hybrid => "route:hybrid".to_owned(),
        RouteKind::React => "route:react".to_owned(),
        RouteKind::Proactive => "route:proactive".to_owned(),
        RouteKind::DaemonOnly => "route:daemon_only".to_owned(),
    }
}

/// Convert an [`OutcomeResult`] to a Hebbian concept string.
fn result_to_concept(result: OutcomeResult) -> String {
    match result {
        OutcomeResult::Success => "result:success".to_owned(),
        OutcomeResult::PartialSuccess => "result:partial".to_owned(),
        OutcomeResult::Failure => "result:failure".to_owned(),
        OutcomeResult::UserCancelled => "result:cancelled".to_owned(),
        OutcomeResult::PolicyBlocked => "result:blocked".to_owned(),
        OutcomeResult::Timeout => "result:timeout".to_owned(),
    }
}

// ===========================================================================
// Subscriber 2: Episodic Memory
// ===========================================================================

/// Store the outcome as an episodic memory.
///
/// The episode captures what happened, how well it went, and contextual tags.
/// This allows future retrieval by the dreaming engine, the contextor, and
/// the proactive engine when they need to recall past interaction quality.
async fn dispatch_to_memory(
    memory: &AuraMemory,
    outcome: &ExecutionOutcome,
    now_ms: u64,
) {
    // Emotional valence: map effectiveness score from [0,1] to [-1,1]
    // where 0.5 effectiveness → 0.0 valence (neutral).
    let effectiveness = outcome.effectiveness_score();
    let emotional_valence = (effectiveness - 0.5) * 2.0;

    // Base importance: combine confidence and result severity.
    // Failed interactions are MORE important to remember (negativity bias)
    // — matches human episodic memory where failures are more salient.
    let result_salience = match outcome.result {
        OutcomeResult::Failure => 0.8,
        OutcomeResult::Timeout => 0.7,
        OutcomeResult::PolicyBlocked => 0.6,
        OutcomeResult::PartialSuccess => 0.5,
        OutcomeResult::UserCancelled => 0.4,
        OutcomeResult::Success => 0.3,
    };
    let base_importance = (result_salience * 0.6 + outcome.confidence * 0.4).clamp(0.0, 1.0);

    // Context tags for retrieval
    let mut tags = Vec::with_capacity(5);
    tags.push(format!("route:{}", route_tag(outcome.route)));
    tags.push(format!("result:{}", result_tag(outcome.result)));
    if let Some(domain) = outcome.domain {
        tags.push(format!("domain:{domain}"));
    }
    if outcome.was_fallback {
        tags.push("fallback".to_owned());
    }
    if outcome.react_iterations > 0 {
        tags.push(format!("react_iters:{}", outcome.react_iterations));
    }

    // Episodic content: structured but human-readable
    let content = format!(
        "[{route}] {intent}: {result} (conf={conf:.2}, {dur}ms){input_ctx}",
        route = route_tag(outcome.route),
        intent = outcome.intent,
        result = result_tag(outcome.result),
        conf = outcome.confidence,
        dur = outcome.duration_ms,
        input_ctx = if outcome.input_summary.is_empty() {
            String::new()
        } else {
            format!(" — \"{}\"", &outcome.input_summary)
        },
    );

    match memory.store_episodic(content, emotional_valence, base_importance, tags, now_ms).await {
        Ok(episode_id) => {
            debug!(episode_id, intent = %outcome.intent, "episodic memory stored");
        }
        Err(e) => {
            warn!(
                intent = %outcome.intent,
                error = %e,
                "failed to store episodic memory for outcome"
            );
        }
    }
}

/// Short tag for a route kind (used in memory tags and content).
fn route_tag(route: RouteKind) -> &'static str {
    match route {
        RouteKind::System1 => "s1",
        RouteKind::System2 => "s2",
        RouteKind::Hybrid => "hybrid",
        RouteKind::React => "react",
        RouteKind::Proactive => "proactive",
        RouteKind::DaemonOnly => "daemon",
    }
}

/// Short tag for a result kind (used in memory tags and content).
fn result_tag(result: OutcomeResult) -> &'static str {
    match result {
        OutcomeResult::Success => "ok",
        OutcomeResult::PartialSuccess => "partial",
        OutcomeResult::Failure => "fail",
        OutcomeResult::UserCancelled => "cancelled",
        OutcomeResult::PolicyBlocked => "blocked",
        OutcomeResult::Timeout => "timeout",
    }
}

// ===========================================================================
// Subscriber 3: BDI Goals
// ===========================================================================

/// Update the BDI scheduler's belief base with execution evidence.
///
/// Two beliefs are produced:
/// 1. `"outcome:{intent}"` — the latest result for this intent category.
/// 2. `"goal:{id}:last_outcome"` — if the outcome is linked to a goal.
///
/// These beliefs feed the deliberation cycle: desires become intentions
/// only when the belief base shows they are feasible.  Outcome beliefs
/// provide the "did it work last time?" evidence.
fn dispatch_to_goals(
    bdi: &mut BdiScheduler,
    outcome: &ExecutionOutcome,
    now_ms: u64,
) {
    // Belief 1: per-intent outcome tracking
    let intent_belief = Belief {
        key: format!("outcome:{}", outcome.intent),
        value: format!(
            "{}:conf={:.2}:dur={}ms",
            result_tag(outcome.result),
            outcome.confidence,
            outcome.duration_ms,
        ),
        confidence: outcome.confidence,
        updated_at_ms: now_ms,
        source: BeliefSource::ExecutionOutcome,
    };

    if let Err(e) = bdi.update_belief(intent_belief) {
        warn!(
            intent = %outcome.intent,
            error = %e,
            "BDI belief update failed for outcome intent"
        );
    }

    // Belief 2: per-goal outcome (only if linked)
    if let Some(goal_id) = outcome.goal_id {
        let goal_belief = Belief {
            key: format!("goal:{goal_id}:last_outcome"),
            value: result_tag(outcome.result).to_owned(),
            confidence: outcome.confidence,
            updated_at_ms: now_ms,
            source: BeliefSource::ExecutionOutcome,
        };

        if let Err(e) = bdi.update_belief(goal_belief) {
            warn!(
                goal_id,
                error = %e,
                "BDI belief update failed for goal outcome"
            );
        }
    }

    debug!(
        intent = %outcome.intent,
        goal_id = ?outcome.goal_id,
        "BDI beliefs updated from outcome"
    );
}

// ===========================================================================
// Subscriber 4: Identity (relationship trust)
// ===========================================================================

/// Update the identity engine's relationship tracker.
///
/// The mapping from outcome to interaction type follows cognitive science:
/// - Successful interactions build trust (Positive).
/// - Failed interactions erode trust slightly (Negative).
/// - Cancelled/blocked interactions are neutral (user or policy choice, not AURA failure).
fn dispatch_to_identity(
    identity: &mut IdentityEngine,
    outcome: &ExecutionOutcome,
    now_ms: u64,
) {
    let interaction = match outcome.result {
        OutcomeResult::Success | OutcomeResult::PartialSuccess => InteractionType::Positive,
        OutcomeResult::Failure | OutcomeResult::Timeout => InteractionType::Negative,
        OutcomeResult::UserCancelled | OutcomeResult::PolicyBlocked => InteractionType::Neutral,
    };

    identity.record_interaction(SYSTEM_USER_ID, interaction, now_ms);

    debug!(
        interaction = ?interaction,
        intent = %outcome.intent,
        "identity interaction recorded from outcome"
    );
}

// ===========================================================================
// Subscriber 5: Anti-Sycophancy enrichment
// ===========================================================================

/// Enrich the sycophancy guard with outcome-derived behavioural signals.
///
/// This does NOT do keyword detection on the response text (that would violate
/// the zero-template principle).  Instead, it derives structural signals from
/// the outcome metadata:
///
/// - **agreed**: Outcome was successful AND confidence was high → AURA likely
///   affirmed the user's request without pushback.
/// - **hedged**: Confidence was low → AURA was uncertain, which manifests as
///   hedging language in practice.
/// - **reversed_opinion**: Outcome was a fallback → AURA changed strategy
///   mid-execution, analogous to opinion reversal.
/// - **praised**: Always false here — praise detection requires response content
///   analysis which happens in the response pipeline, not the outcome bus.
/// - **challenged**: Outcome was PolicyBlocked → AURA pushed back on the request.
///
/// These are structural/behavioural signals, not keyword-based.  The sycophancy
/// guard's sliding window will accumulate these patterns and detect trends.
fn dispatch_to_anti_sycophancy(
    identity: &mut IdentityEngine,
    outcome: &ExecutionOutcome,
) {
    let record = ResponseRecord {
        agreed: outcome.is_success() && outcome.confidence >= 0.7,
        hedged: outcome.confidence < 0.4,
        reversed_opinion: outcome.was_fallback,
        praised: false,
        challenged: matches!(outcome.result, OutcomeResult::PolicyBlocked),
    };

    identity.sycophancy_guard.record_response(record);

    debug!(
        agreed = record.agreed,
        hedged = record.hedged,
        reversed = record.reversed_opinion,
        challenged = record.challenged,
        "anti-sycophancy enriched from outcome"
    );
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use aura_types::outcome::{ExecutionOutcome, OutcomeResult, RouteKind};

    #[test]
    fn test_outcome_bus_publish_and_count() {
        let mut bus = OutcomeBus::new();
        assert_eq!(bus.pending_count(), 0);

        let outcome = ExecutionOutcome::new(
            "test_intent".into(),
            OutcomeResult::Success,
            150,
            0.85,
            RouteKind::System1,
            1000,
        );
        bus.publish(outcome);
        assert_eq!(bus.pending_count(), 1);
    }

    #[test]
    fn test_overflow_drops_oldest() {
        let mut bus = OutcomeBus::new();

        for i in 0..MAX_PENDING + 5 {
            let outcome = ExecutionOutcome::new(
                format!("intent_{i}"),
                OutcomeResult::Success,
                100,
                0.5,
                RouteKind::System1,
                i as u64 * 1000,
            );
            bus.publish(outcome);
        }

        // Buffer should never exceed MAX_PENDING
        assert_eq!(bus.pending_count(), MAX_PENDING);
        // Oldest should have been dropped — first intent should be "intent_5"
        assert_eq!(bus.pending[0].intent, "intent_5");
        assert_eq!(bus.dispatch_errors, 5);
    }

    #[test]
    fn test_map_to_learning_outcome() {
        assert_eq!(
            map_to_learning_outcome(OutcomeResult::Success),
            LearningOutcome::Success
        );
        assert_eq!(
            map_to_learning_outcome(OutcomeResult::PartialSuccess),
            LearningOutcome::Success
        );
        assert_eq!(
            map_to_learning_outcome(OutcomeResult::Failure),
            LearningOutcome::Failure
        );
        assert_eq!(
            map_to_learning_outcome(OutcomeResult::Timeout),
            LearningOutcome::Failure
        );
        assert_eq!(
            map_to_learning_outcome(OutcomeResult::UserCancelled),
            LearningOutcome::Neutral
        );
        assert_eq!(
            map_to_learning_outcome(OutcomeResult::PolicyBlocked),
            LearningOutcome::Neutral
        );
    }

    #[test]
    fn test_build_outcome_features() {
        // Epoch ms for 2024-01-15 10:30:00 UTC ≈ morning
        let outcome = ExecutionOutcome::new(
            "weather_query".into(),
            OutcomeResult::Success,
            200,
            0.92,
            RouteKind::System1,
            // 10:30 AM → hour 10 → "morning"
            10 * 3_600_000 + 30 * 60_000,
        );

        let features = build_outcome_features(&outcome);
        assert_eq!(features.len(), 4);

        // Verify time bucket
        match &features[0] {
            Feature::TimeBucket(t) => assert_eq!(t, "morning"),
            other => panic!("expected TimeBucket, got {other:?}"),
        }

        // Verify activity type
        match &features[1] {
            Feature::ActivityType(a) => assert_eq!(a, "fast_path"),
            other => panic!("expected ActivityType, got {other:?}"),
        }

        // Verify result feature
        match &features[2] {
            Feature::Custom(k, v) => {
                assert_eq!(k, "exec_result");
                assert_eq!(v, "success");
            }
            other => panic!("expected Custom exec_result, got {other:?}"),
        }

        // Verify confidence bucket
        match &features[3] {
            Feature::Custom(k, v) => {
                assert_eq!(k, "confidence");
                assert_eq!(v, "high");
            }
            other => panic!("expected Custom confidence, got {other:?}"),
        }
    }

    #[test]
    fn test_route_and_result_concepts() {
        assert_eq!(route_to_concept(RouteKind::System1), "route:system1");
        assert_eq!(route_to_concept(RouteKind::React), "route:react");
        assert_eq!(result_to_concept(OutcomeResult::Success), "result:success");
        assert_eq!(result_to_concept(OutcomeResult::Timeout), "result:timeout");
    }

    #[test]
    fn test_anti_sycophancy_record_from_outcome() {
        // High-confidence success → agreed=true, hedged=false
        let outcome = ExecutionOutcome::new(
            "test".into(),
            OutcomeResult::Success,
            100,
            0.9,
            RouteKind::System1,
            1000,
        );

        let record = ResponseRecord {
            agreed: outcome.is_success() && outcome.confidence >= 0.7,
            hedged: outcome.confidence < 0.4,
            reversed_opinion: outcome.was_fallback,
            praised: false,
            challenged: matches!(outcome.result, OutcomeResult::PolicyBlocked),
        };

        assert!(record.agreed);
        assert!(!record.hedged);
        assert!(!record.reversed_opinion);
        assert!(!record.praised);
        assert!(!record.challenged);
    }

    #[test]
    fn test_anti_sycophancy_record_low_confidence_failure() {
        // Low-confidence failure with fallback → hedged=true, reversed=true
        let outcome = ExecutionOutcome::new(
            "test".into(),
            OutcomeResult::Failure,
            500,
            0.2,
            RouteKind::Hybrid,
            1000,
        )
        .with_fallback();

        let record = ResponseRecord {
            agreed: outcome.is_success() && outcome.confidence >= 0.7,
            hedged: outcome.confidence < 0.4,
            reversed_opinion: outcome.was_fallback,
            praised: false,
            challenged: matches!(outcome.result, OutcomeResult::PolicyBlocked),
        };

        assert!(!record.agreed);
        assert!(record.hedged);
        assert!(record.reversed_opinion);
        assert!(!record.challenged);
    }

    #[test]
    fn test_anti_sycophancy_record_policy_blocked() {
        // Policy blocked → challenged=true
        let outcome = ExecutionOutcome::new(
            "dangerous_request".into(),
            OutcomeResult::PolicyBlocked,
            10,
            0.95,
            RouteKind::DaemonOnly,
            1000,
        );

        let record = ResponseRecord {
            agreed: outcome.is_success() && outcome.confidence >= 0.7,
            hedged: outcome.confidence < 0.4,
            reversed_opinion: outcome.was_fallback,
            praised: false,
            challenged: matches!(outcome.result, OutcomeResult::PolicyBlocked),
        };

        assert!(!record.agreed);
        assert!(!record.hedged);
        assert!(!record.reversed_opinion);
        assert!(record.challenged);
    }

    #[test]
    fn test_route_tags() {
        assert_eq!(route_tag(RouteKind::System1), "s1");
        assert_eq!(route_tag(RouteKind::System2), "s2");
        assert_eq!(route_tag(RouteKind::React), "react");
    }

    #[test]
    fn test_result_tags() {
        assert_eq!(result_tag(OutcomeResult::Success), "ok");
        assert_eq!(result_tag(OutcomeResult::Failure), "fail");
        assert_eq!(result_tag(OutcomeResult::Timeout), "timeout");
    }
}
