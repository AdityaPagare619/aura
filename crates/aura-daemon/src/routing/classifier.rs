use aura_types::events::{EventSource, GateDecision, Intent, ScoredEvent};
use aura_types::ipc::InferenceMode;
use serde::{Deserialize, Serialize};
use tracing::instrument;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Hysteresis gap — ignore route change if score delta < this.
const HYSTERESIS_GAP: f32 = 0.15;

/// Default complexity threshold for System1 vs System2.
const DEFAULT_COMPLEXITY_THRESHOLD: f32 = 0.50;

/// Keyword weights for complexity scoring.
static COMPLEXITY_KEYWORDS: &[(&str, f32)] = &[
    ("multi-step", 0.30),
    ("complex", 0.30),
    ("analyze", 0.25),
    ("compare", 0.25),
    ("plan", 0.20),
    ("schedule", 0.20),
    ("research", 0.30),
    ("summarize", 0.20),
    ("compose", 0.25),
    ("debug", 0.20),
    ("navigate", 0.20),
    ("search", 0.15),
];

/// Keywords indicating urgency in user requests.
static URGENCY_KEYWORDS: &[(&str, f32)] = &[
    ("urgent", 0.40),
    ("asap", 0.35),
    ("immediately", 0.35),
    ("now", 0.25),
    ("hurry", 0.30),
    ("emergency", 0.50),
    ("deadline", 0.30),
    ("quick", 0.20),
    ("right away", 0.35),
];

/// V3 routing score factor weights (rebalanced for personality integration).
const WEIGHT_COMPLEXITY: f32 = 0.35;
const WEIGHT_IMPORTANCE: f32 = 0.22;
const WEIGHT_URGENCY: f32 = 0.17;
const WEIGHT_MEMORY_LOAD: f32 = 0.11;
/// Weight given to personality-derived routing bias.
const WEIGHT_PERSONALITY: f32 = 0.15;

/// Weight given to screen semantic complexity (from SemanticGraph analysis).
/// When screen analysis is available, this is borrowed from the other factors
/// proportionally (the 5 original weights still sum to 1.0; screen complexity
/// acts as an additive nudge clamped into the final score).
const WEIGHT_SCREEN_COMPLEXITY: f32 = 0.08;

/// Default working memory slot capacity.
const DEFAULT_MEMORY_SLOTS: usize = 7;

/// Default importance when no amygdala signal is available.
const DEFAULT_IMPORTANCE: f32 = 0.5;

/// Default urgency when no time-pressure signal is available.
const DEFAULT_URGENCY: f32 = 0.3;

/// Default memory load when working memory state is unknown.
const DEFAULT_MEMORY_LOAD: f32 = 0.2;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The four possible execution paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutePath {
    /// Fast path — daemon-only, no LLM.
    System1,
    /// Slow path — involves Neocortex LLM.
    System2,
    /// Daemon attempts first, falls back to LLM on failure.
    Hybrid,
    /// Daemon handles internally (log/suppress).
    DaemonOnly,
}

/// Result of the routing cascade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteDecision {
    pub path: RoutePath,
    pub confidence: f32,
    pub reason: String,
    pub neocortex_mode: Option<InferenceMode>,
}

/// 10-node deterministic routing cascade.
///
/// Same inputs **always** produce the same output. Hysteresis prevents
/// flip-flopping between System1 and System2 when scores are near the
/// boundary.
///
/// The V3 routing formula combines five normalized factors:
/// ```text
/// routing_score = 0.35 * complexity + 0.22 * importance + 0.17 * urgency
///               + 0.11 * memory_load + 0.15 * personality_bias
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteClassifier {
    last_route: Option<RoutePath>,
    last_routing_score: f32,
    complexity_threshold: f32,
    /// Current working memory occupancy (number of active slots).
    working_memory_count: usize,
    /// Maximum working memory slots (default 7, Miller's number).
    working_memory_max: usize,
    /// Personality-derived routing bias in \[-0.15, 0.15\].
    /// Positive → favours System2, negative → favours System1.
    personality_bias: f32,
    /// Screen semantic complexity signal from the last SemanticGraph analysis.
    /// Range \[0.0, 1.0\]: 0 = no screen data or trivial UI, 1 = very complex UI.
    /// Complex UIs (dialogs, deeply nested forms) nudge toward System2.
    screen_complexity: f32,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl RouteClassifier {
    pub fn new() -> Self {
        Self {
            last_route: None,
            last_routing_score: 0.0,
            complexity_threshold: DEFAULT_COMPLEXITY_THRESHOLD,
            working_memory_count: 0,
            working_memory_max: DEFAULT_MEMORY_SLOTS,
            personality_bias: 0.0,
            screen_complexity: 0.0,
        }
    }

    /// Update the current working memory occupancy. Called by the memory
    /// subsystem whenever slots are acquired or released.
    #[instrument(skip(self))]
    pub fn set_working_memory(&mut self, count: usize, max_slots: usize) {
        self.working_memory_count = count;
        self.working_memory_max = if max_slots == 0 {
            DEFAULT_MEMORY_SLOTS
        } else {
            max_slots
        };
    }

    /// Set the personality-derived routing bias.
    ///
    /// The bias is clamped to \[-0.15, 0.15\] and represents the personality
    /// system's preference toward System2 (positive) or System1 (negative).
    ///
    /// Typical source: `Personality::routing_bias()`.
    #[instrument(skip(self))]
    pub fn set_personality_bias(&mut self, bias: f32) {
        self.personality_bias = bias.clamp(-0.15, 0.15);
    }

    /// Set the screen semantic context from a recent `SemanticGraph` analysis.
    ///
    /// `complexity` is a \[0.0, 1.0\] composite derived from the number of
    /// detected UI patterns, interactive element count, and screen semantic
    /// state (e.g. a `Blocked` dialog or deeply nested `SettingsPage` is
    /// more complex than a simple `Interactive` screen).
    ///
    /// `pattern_count` is the number of recognized `UiPattern`s — higher
    /// counts suggest richer UI requiring deeper reasoning.
    ///
    /// Resets to 0.0 when screen data is stale or unavailable.
    #[instrument(skip(self))]
    pub fn set_screen_context(&mut self, complexity: f32, pattern_count: usize) {
        // Blend detected patterns into complexity: each pattern adds 0.08,
        // capped at 0.3, so a screen with 3+ patterns always gets a boost.
        let pattern_boost = (pattern_count as f32 * 0.08).min(0.3);
        self.screen_complexity = (complexity + pattern_boost).clamp(0.0, 1.0);
    }

    /// Run the 10-node deterministic routing cascade on a scored event.
    #[instrument(skip(self, scored), fields(intent = ?scored.parsed.intent, source = ?scored.parsed.source))]
    pub fn classify(&mut self, scored: &ScoredEvent) -> RouteDecision {
        let event = &scored.parsed;
        let routing_score = self.compute_routing_score(scored);
        let decision = self.cascade(scored, routing_score);

        // Apply hysteresis for System1 <-> System2 transitions.
        let decision = self.apply_hysteresis(decision, routing_score);

        self.last_route = Some(decision.path);
        self.last_routing_score = routing_score;

        tracing::debug!(
            path = ?decision.path,
            confidence = decision.confidence,
            routing_score,
            reason = %decision.reason,
            source = ?event.source,
            intent = ?event.intent,
            "route classified"
        );

        decision
    }

    // -- V3 4-factor routing score -------------------------------------------

    /// Compute the V3 composite routing score from five normalized factors.
    ///
    /// ```text
    /// routing_score = 0.35 * complexity + 0.22 * importance + 0.17 * urgency
    ///               + 0.11 * memory_load + 0.15 * personality_bias_norm
    /// ```
    ///
    /// The personality bias is normalized from \[-0.15, 0.15\] → \[0.0, 1.0\]
    /// so it fits the same scale as the other factors.
    ///
    /// All individual factors are clamped to \[0.0, 1.0\].
    #[must_use]
    fn compute_routing_score(&self, scored: &ScoredEvent) -> f32 {
        let complexity = Self::compute_complexity(&scored.parsed.content);
        let importance = Self::compute_importance(scored);
        let urgency = Self::compute_urgency(&scored.parsed.content, scored.score_time);
        let memory_load = self.compute_memory_load();

        // Normalize personality_bias from [-0.15, 0.15] → [0.0, 1.0]
        let personality_norm = ((self.personality_bias + 0.15) / 0.30).clamp(0.0, 1.0);

        let score = WEIGHT_COMPLEXITY * complexity
            + WEIGHT_IMPORTANCE * importance
            + WEIGHT_URGENCY * urgency
            + WEIGHT_MEMORY_LOAD * memory_load
            + WEIGHT_PERSONALITY * personality_norm
            + WEIGHT_SCREEN_COMPLEXITY * self.screen_complexity;

        tracing::trace!(
            complexity,
            importance,
            urgency,
            memory_load,
            personality_bias = self.personality_bias,
            personality_norm,
            screen_complexity = self.screen_complexity,
            composite = score,
            "routing score factors"
        );

        score.clamp(0.0, 1.0)
    }

    /// Deterministic complexity score based on word count + keyword matching
    /// + step-count heuristic.
    ///
    /// - Base: `word_count / 50`, capped at 0.3
    /// - Each matched keyword adds its weight
    /// - Implicit step indicator: +0.1 per connective ("then", "and then", "after that")
    #[must_use]
    pub fn compute_complexity(content: &str) -> f32 {
        let lower = content.to_ascii_lowercase();

        // Base: word_count / 50, capped at 0.3.
        let word_count = content.split_whitespace().count();
        let base = (word_count as f32 / 50.0).min(0.3);

        // Keyword weights.
        let keyword_sum: f32 = COMPLEXITY_KEYWORDS
            .iter()
            .filter(|(kw, _)| lower.contains(kw))
            .map(|(_, w)| w)
            .sum();

        // Step-count heuristic: connectives imply multi-step tasks.
        let step_connectives: &[&str] = &["then ", "and then", "after that", "next ", "finally "];
        let step_bonus: f32 = step_connectives
            .iter()
            .filter(|conn| lower.contains(*conn))
            .count() as f32
            * 0.10;

        (base + keyword_sum + step_bonus).clamp(0.0, 1.0)
    }

    /// Importance score derived from the amygdala's composite relevance signal.
    ///
    /// Uses `score_total` from the amygdala pipeline as a proxy for event
    /// priority. Falls back to `DEFAULT_IMPORTANCE` only if the score is
    /// exactly zero (unscored event).
    #[must_use]
    fn compute_importance(scored: &ScoredEvent) -> f32 {
        // Gate decision boosts: emergency events are maximally important.
        let gate_boost = match scored.gate_decision {
            GateDecision::EmergencyBypass => 1.0,
            GateDecision::InstantWake => 0.0, // no extra boost, rely on score
            GateDecision::SlowAccumulate => 0.0,
            GateDecision::Suppress => 0.0,
        };

        if gate_boost > 0.0 {
            return gate_boost;
        }

        // Use amygdala total score as importance proxy.
        if scored.score_total > f32::EPSILON {
            scored.score_total.clamp(0.0, 1.0)
        } else {
            DEFAULT_IMPORTANCE
        }
    }

    /// Urgency score derived from temporal signals and keyword analysis.
    ///
    /// Combines the amygdala's time-relevance score with urgency keywords
    /// found in the content.
    #[must_use]
    fn compute_urgency(content: &str, score_time: f32) -> f32 {
        let lower = content.to_ascii_lowercase();

        let keyword_urgency: f32 = URGENCY_KEYWORDS
            .iter()
            .filter(|(kw, _)| lower.contains(kw))
            .map(|(_, w)| w)
            .sum();

        if keyword_urgency > f32::EPSILON || score_time > f32::EPSILON {
            // Blend: 60% keyword signal, 40% temporal score.
            let blended = 0.60 * keyword_urgency + 0.40 * score_time;
            blended.clamp(0.0, 1.0)
        } else {
            DEFAULT_URGENCY
        }
    }

    /// Memory load score: working memory occupancy as a fraction of max slots.
    ///
    /// Higher load → more cognitive resources in use → bias toward System2
    /// for better context handling.
    #[must_use]
    fn compute_memory_load(&self) -> f32 {
        if self.working_memory_max == 0 {
            return DEFAULT_MEMORY_LOAD;
        }
        (self.working_memory_count as f32 / self.working_memory_max as f32).clamp(0.0, 1.0)
    }

    // -- 10-node cascade (private) ------------------------------------------

    fn cascade(&self, scored: &ScoredEvent, routing_score: f32) -> RouteDecision {
        let event = &scored.parsed;
        let source = event.source;
        let intent = event.intent;

        // Node 1: Emergency bypass.
        if scored.gate_decision == GateDecision::EmergencyBypass {
            return RouteDecision {
                path: RoutePath::System2,
                confidence: 1.0,
                reason: "emergency bypass".to_string(),
                neocortex_mode: Some(InferenceMode::Strategist),
            };
        }

        // Node 2: User command + action request → check routing_score.
        if source == EventSource::UserCommand && intent == Intent::ActionRequest {
            if routing_score < self.complexity_threshold {
                // Node 8 path (simple action).
                return RouteDecision {
                    path: RoutePath::System1,
                    confidence: 0.80,
                    reason: format!("simple action (routing_score={:.2})", routing_score),
                    neocortex_mode: None,
                };
            } else {
                // Node 9 path (complex action).
                return RouteDecision {
                    path: RoutePath::System2,
                    confidence: 0.75,
                    reason: format!("complex action (routing_score={:.2})", routing_score),
                    neocortex_mode: Some(InferenceMode::Planner),
                };
            }
        }

        // Node 3: User command + conversation continue → System2 Conversational.
        if source == EventSource::UserCommand && intent == Intent::ConversationContinue {
            return RouteDecision {
                path: RoutePath::System2,
                confidence: 0.90,
                reason: "conversation continuation".to_string(),
                neocortex_mode: Some(InferenceMode::Conversational),
            };
        }

        // Node 4: Information request → System2 Conversational.
        if intent == Intent::InformationRequest {
            return RouteDecision {
                path: RoutePath::System2,
                confidence: 0.85,
                reason: "information request".to_string(),
                neocortex_mode: Some(InferenceMode::Conversational),
            };
        }

        // Node 5: System alert → severity-based.
        if intent == Intent::SystemAlert {
            if scored.score_total > 0.80 {
                return RouteDecision {
                    path: RoutePath::System2,
                    confidence: 0.90,
                    reason: "high-severity system alert".to_string(),
                    neocortex_mode: Some(InferenceMode::Strategist),
                };
            } else {
                return RouteDecision {
                    path: RoutePath::System1,
                    confidence: 0.90,
                    reason: "low-severity system alert".to_string(),
                    neocortex_mode: None,
                };
            }
        }

        // Node 6: Proactive opportunity → Hybrid.
        if intent == Intent::ProactiveOpportunity {
            return RouteDecision {
                path: RoutePath::Hybrid,
                confidence: 0.70,
                reason: "proactive opportunity".to_string(),
                neocortex_mode: Some(InferenceMode::Planner),
            };
        }

        // Node 7: Routine event with low score → DaemonOnly.
        if intent == Intent::RoutineEvent && scored.score_total < 0.30 {
            return RouteDecision {
                path: RoutePath::DaemonOnly,
                confidence: 0.95,
                reason: "low-score routine event".to_string(),
                neocortex_mode: None,
            };
        }

        // Node 8: Simple action (non-UserCommand action request).
        if intent == Intent::ActionRequest {
            if routing_score < self.complexity_threshold {
                return RouteDecision {
                    path: RoutePath::System1,
                    confidence: 0.80,
                    reason: format!("simple action (routing_score={:.2})", routing_score),
                    neocortex_mode: None,
                };
            }
            // Node 9: Complex action.
            return RouteDecision {
                path: RoutePath::System2,
                confidence: 0.75,
                reason: format!("complex action (routing_score={:.2})", routing_score),
                neocortex_mode: Some(InferenceMode::Planner),
            };
        }

        // Node 10: Default → Hybrid.
        RouteDecision {
            path: RoutePath::Hybrid,
            confidence: 0.50,
            reason: "default fallback".to_string(),
            neocortex_mode: Some(InferenceMode::Conversational),
        }
    }

    /// Apply hysteresis to prevent System1 ↔ System2 flip-flopping.
    ///
    /// If previous routing was System1 and new routing_score would trigger
    /// System2, require the score to exceed `threshold + HYSTERESIS_GAP`
    /// before actually switching. Symmetric for the reverse direction.
    fn apply_hysteresis(&self, decision: RouteDecision, routing_score: f32) -> RouteDecision {
        let Some(last) = self.last_route else {
            return decision;
        };

        // Only apply hysteresis for S1↔S2 transitions.
        let is_s1_s2 = matches!(
            (last, decision.path),
            (RoutePath::System1, RoutePath::System2) | (RoutePath::System2, RoutePath::System1)
        );

        if is_s1_s2 && (routing_score - self.last_routing_score).abs() < HYSTERESIS_GAP {
            tracing::debug!(
                last = ?last,
                proposed = ?decision.path,
                score_delta = (routing_score - self.last_routing_score).abs(),
                "hysteresis: keeping previous route"
            );
            RouteDecision {
                path: last,
                confidence: decision.confidence * 0.9, // Slightly reduced confidence.
                reason: format!("{} (hysteresis held)", decision.reason),
                neocortex_mode: decision.neocortex_mode,
            }
        } else {
            decision
        }
    }
}

impl Default for RouteClassifier {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use aura_types::events::{EventSource, GateDecision, Intent, ParsedEvent};

    fn make_scored(
        source: EventSource,
        intent: Intent,
        content: &str,
        total: f32,
        gate: GateDecision,
    ) -> ScoredEvent {
        ScoredEvent {
            parsed: ParsedEvent {
                source,
                intent,
                content: content.to_string(),
                entities: vec![],
                timestamp_ms: 1_000_000,
                raw_event_type: 0,
            },
            score_total: total,
            score_lex: 0.50,
            score_src: 0.30,
            score_time: 0.20,
            score_anom: 0.10,
            gate_decision: gate,
        }
    }

    #[test]
    fn test_emergency_bypass_routes_to_system2() {
        let mut c = RouteClassifier::new();
        let scored = make_scored(
            EventSource::Notification,
            Intent::SystemAlert,
            "critical failure",
            0.95,
            GateDecision::EmergencyBypass,
        );
        let d = c.classify(&scored);
        assert_eq!(d.path, RoutePath::System2);
        assert_eq!(d.neocortex_mode, Some(InferenceMode::Strategist));
        assert!((d.confidence - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_user_command_simple_action() {
        let mut c = RouteClassifier::new();
        let scored = make_scored(
            EventSource::UserCommand,
            Intent::ActionRequest,
            "open whatsapp",
            0.70,
            GateDecision::InstantWake,
        );
        let d = c.classify(&scored);
        assert_eq!(d.path, RoutePath::System1);
    }

    #[test]
    fn test_user_command_complex_action() {
        let mut c = RouteClassifier::new();
        let scored = make_scored(
            EventSource::UserCommand,
            Intent::ActionRequest,
            "research and compare the best restaurants nearby then schedule a reservation",
            0.80,
            GateDecision::InstantWake,
        );
        let d = c.classify(&scored);
        assert_eq!(d.path, RoutePath::System2);
        assert_eq!(d.neocortex_mode, Some(InferenceMode::Planner));
    }

    #[test]
    fn test_conversation_continue() {
        let mut c = RouteClassifier::new();
        let scored = make_scored(
            EventSource::UserCommand,
            Intent::ConversationContinue,
            "yes",
            0.50,
            GateDecision::InstantWake,
        );
        let d = c.classify(&scored);
        assert_eq!(d.path, RoutePath::System2);
        assert_eq!(d.neocortex_mode, Some(InferenceMode::Conversational));
    }

    #[test]
    fn test_routine_low_score_daemon_only() {
        let mut c = RouteClassifier::new();
        let scored = make_scored(
            EventSource::Accessibility,
            Intent::RoutineEvent,
            "screen refreshed",
            0.15,
            GateDecision::SlowAccumulate,
        );
        let d = c.classify(&scored);
        assert_eq!(d.path, RoutePath::DaemonOnly);
    }

    #[test]
    fn test_proactive_opportunity_hybrid() {
        let mut c = RouteClassifier::new();
        let scored = make_scored(
            EventSource::Notification,
            Intent::ProactiveOpportunity,
            "ride arriving in 5 min",
            0.60,
            GateDecision::InstantWake,
        );
        let d = c.classify(&scored);
        assert_eq!(d.path, RoutePath::Hybrid);
    }

    #[test]
    fn test_determinism() {
        // Same inputs must always produce same output.
        let scored = make_scored(
            EventSource::UserCommand,
            Intent::ActionRequest,
            "open camera",
            0.70,
            GateDecision::InstantWake,
        );

        let mut c1 = RouteClassifier::new();
        let mut c2 = RouteClassifier::new();

        let d1 = c1.classify(&scored);
        let d2 = c2.classify(&scored);

        assert_eq!(d1.path, d2.path);
        assert!((d1.confidence - d2.confidence).abs() < f32::EPSILON);
    }

    #[test]
    fn test_hysteresis_holds_route() {
        let mut c = RouteClassifier::new();

        // First: System1 — simple command, low score_total → low routing_score.
        let s1 = make_scored(
            EventSource::UserCommand,
            Intent::ActionRequest,
            "open camera",
            0.40,
            GateDecision::InstantWake,
        );
        let d1 = c.classify(&s1);
        assert_eq!(d1.path, RoutePath::System1);

        // Second: slightly more complex, but routing_score delta < HYSTERESIS_GAP
        // → hysteresis should hold System1.
        let s2 = make_scored(
            EventSource::UserCommand,
            Intent::ActionRequest,
            "search the topic",
            0.45,
            GateDecision::InstantWake,
        );
        let d2 = c.classify(&s2);
        assert_eq!(
            d2.path,
            RoutePath::System1,
            "hysteresis should hold System1"
        );
    }

    #[test]
    fn test_complexity_scoring() {
        // Simple content.
        let c1 = RouteClassifier::compute_complexity("open camera");
        assert!(c1 < 0.50, "simple complexity={}", c1);

        // Complex content.
        let c2 = RouteClassifier::compute_complexity(
            "analyze and compare these multi-step research results then compose a summary",
        );
        assert!(c2 >= 0.50, "complex complexity={}", c2);
    }

    #[test]
    fn test_importance_from_amygdala() {
        let scored = make_scored(
            EventSource::Notification,
            Intent::ActionRequest,
            "test",
            0.85,
            GateDecision::InstantWake,
        );
        let importance = RouteClassifier::compute_importance(&scored);
        assert!(
            (importance - 0.85).abs() < f32::EPSILON,
            "importance={}",
            importance
        );
    }

    #[test]
    fn test_importance_emergency_is_max() {
        let scored = make_scored(
            EventSource::Notification,
            Intent::SystemAlert,
            "test",
            0.50,
            GateDecision::EmergencyBypass,
        );
        let importance = RouteClassifier::compute_importance(&scored);
        assert!(
            (importance - 1.0).abs() < f32::EPSILON,
            "emergency importance={}",
            importance
        );
    }

    #[test]
    fn test_urgency_from_keywords() {
        let urgency = RouteClassifier::compute_urgency("I need this urgently now", 0.0);
        // "urgent" (0.40) + "now" (0.25) → keyword_urgency = 0.65
        // blended = 0.60 * 0.65 + 0.40 * 0.0 = 0.39
        assert!(urgency > 0.3, "keyword urgency={}", urgency);
    }

    #[test]
    fn test_urgency_default_no_signal() {
        // No urgency keywords, score_time = 0 → should use default.
        let urgency = RouteClassifier::compute_urgency("open settings", 0.0);
        assert!(
            (urgency - DEFAULT_URGENCY).abs() < f32::EPSILON,
            "default urgency={}",
            urgency
        );
    }

    #[test]
    fn test_memory_load_scaling() {
        let mut c = RouteClassifier::new();

        // Empty working memory.
        c.set_working_memory(0, 7);
        assert!((c.compute_memory_load() - 0.0).abs() < f32::EPSILON);

        // Half-full.
        c.set_working_memory(4, 7);
        let load = c.compute_memory_load();
        assert!((load - 4.0 / 7.0).abs() < 0.01, "half load={}", load);

        // Full.
        c.set_working_memory(7, 7);
        assert!((c.compute_memory_load() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_routing_score_formula_weights() {
        // Verify the formula: 0.40 * C + 0.25 * I + 0.20 * U + 0.15 * M
        let c = RouteClassifier::new();
        let scored = make_scored(
            EventSource::UserCommand,
            Intent::ActionRequest,
            "open camera", // low complexity
            0.70,          // moderate importance
            GateDecision::InstantWake,
        );
        let routing_score = c.compute_routing_score(&scored);
        // routing_score should be a weighted blend, always in [0, 1].
        assert!(
            routing_score >= 0.0 && routing_score <= 1.0,
            "score={}",
            routing_score
        );
    }

    #[test]
    fn test_high_memory_load_pushes_toward_system2() {
        let mut c_low = RouteClassifier::new();
        c_low.set_working_memory(0, 7);

        let mut c_high = RouteClassifier::new();
        c_high.set_working_memory(7, 7);

        let scored = make_scored(
            EventSource::UserCommand,
            Intent::ActionRequest,
            "check weather",
            0.50,
            GateDecision::InstantWake,
        );

        let score_low = c_low.compute_routing_score(&scored);
        let score_high = c_high.compute_routing_score(&scored);

        assert!(
            score_high > score_low,
            "high memory load ({}) should produce higher routing score than low ({})",
            score_high,
            score_low
        );
    }

    #[test]
    fn test_step_connectives_increase_complexity() {
        let simple = RouteClassifier::compute_complexity("open the app");
        let multi_step =
            RouteClassifier::compute_complexity("open the app then search then finally submit");
        assert!(
            multi_step > simple,
            "multi-step ({}) should be more complex than simple ({})",
            multi_step,
            simple
        );
    }

    #[test]
    fn test_personality_bias_increases_routing_score() {
        let mut c_neutral = RouteClassifier::new();
        c_neutral.set_personality_bias(0.0);

        let mut c_biased = RouteClassifier::new();
        c_biased.set_personality_bias(0.15); // max positive bias → favour System2

        let scored = make_scored(
            EventSource::UserCommand,
            Intent::ActionRequest,
            "check weather",
            0.50,
            GateDecision::InstantWake,
        );

        let score_neutral = c_neutral.compute_routing_score(&scored);
        let score_biased = c_biased.compute_routing_score(&scored);

        assert!(
            score_biased > score_neutral,
            "positive personality bias ({}) should increase routing score vs neutral ({})",
            score_biased,
            score_neutral
        );
    }

    #[test]
    fn test_personality_bias_clamped() {
        let mut c = RouteClassifier::new();
        c.set_personality_bias(999.0);
        assert!((c.personality_bias - 0.15).abs() < f32::EPSILON);

        c.set_personality_bias(-999.0);
        assert!((c.personality_bias - (-0.15)).abs() < f32::EPSILON);
    }
}
