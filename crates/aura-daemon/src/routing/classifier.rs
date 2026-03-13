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
/// Routing decisions are based on structural signals only: intent type,
/// event source, gate decision, and amygdala score_total. The LLM determines
/// semantic complexity — Rust does not classify natural language.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteClassifier {
    last_route: Option<RoutePath>,
    last_routing_score: f32,
    complexity_threshold: f32,
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
        }
    }

    /// Run the 10-node deterministic routing cascade on a scored event.
    #[instrument(skip(self, scored), fields(intent = ?scored.parsed.intent, source = ?scored.parsed.source))]
    pub fn classify(&mut self, scored: &ScoredEvent) -> RouteDecision {
        let event = &scored.parsed;
        // Use amygdala structural score as the routing signal — no NLP formula.
        let routing_score = scored.score_total;
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
            0.30, // score_total below complexity_threshold → System1
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

}
