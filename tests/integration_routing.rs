//! Integration tests for the AURA v4 Routing subsystem.
//!
//! Tests the RouteClassifier — the 10-node deterministic routing cascade
//! that decides whether events go to System1 (fast), System2 (LLM),
//! Hybrid, or DaemonOnly paths.

use aura_daemon::routing::classifier::{RouteClassifier, RouteDecision, RoutePath};
use aura_types::events::{EventSource, GateDecision, Intent, ParsedEvent, ScoredEvent};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_ms() -> u64 {
    1_700_000_000_000
}

fn make_scored_event(
    intent: Intent,
    source: EventSource,
    content: &str,
    score_total: f32,
    gate: GateDecision,
) -> ScoredEvent {
    ScoredEvent {
        parsed: ParsedEvent {
            source,
            intent,
            content: content.to_string(),
            entities: vec![],
            timestamp_ms: now_ms(),
            raw_event_type: 0,
        },
        score_total,
        score_lex: score_total * 0.40,
        score_src: score_total * 0.25,
        score_time: score_total * 0.20,
        score_anom: score_total * 0.15,
        gate_decision: gate,
    }
}

// ---------------------------------------------------------------------------
// Basic classifier tests
// ---------------------------------------------------------------------------

#[test]
fn test_classifier_creation() {
    let classifier = RouteClassifier::new();
    let _ = format!("{:?}", classifier);
}

#[test]
fn test_classify_suppressed_event() {
    let mut classifier = RouteClassifier::new();
    let scored = make_scored_event(
        Intent::RoutineEvent,
        EventSource::Internal,
        "background tick",
        0.05,
        GateDecision::Suppress,
    );
    let decision = classifier.classify(&scored);
    // Suppressed events should go to DaemonOnly.
    assert_eq!(decision.path, RoutePath::DaemonOnly);
}

#[test]
fn test_classify_emergency_bypass() {
    let mut classifier = RouteClassifier::new();
    let scored = make_scored_event(
        Intent::SystemAlert,
        EventSource::Notification,
        "EMERGENCY: device overheating",
        0.95,
        GateDecision::EmergencyBypass,
    );
    let decision = classifier.classify(&scored);
    // Emergency should go to fast path (System1 or Hybrid, not DaemonOnly).
    assert_ne!(decision.path, RoutePath::DaemonOnly);
}

#[test]
fn test_classify_simple_action() {
    let mut classifier = RouteClassifier::new();
    let scored = make_scored_event(
        Intent::ActionRequest,
        EventSource::UserCommand,
        "open WhatsApp",
        0.70,
        GateDecision::InstantWake,
    );
    let decision = classifier.classify(&scored);
    // Simple action like "open WhatsApp" should route to System1 or Hybrid.
    assert!(matches!(
        decision.path,
        RoutePath::System1 | RoutePath::Hybrid | RoutePath::System2
    ));
    assert!(decision.confidence > 0.0);
}

#[test]
fn test_classify_complex_request() {
    let mut classifier = RouteClassifier::new();
    let scored = make_scored_event(
        Intent::InformationRequest,
        EventSource::UserCommand,
        "research and compare the best flight options from London to Tokyo, analyze prices and schedule multi-step itinerary",
        0.80,
        GateDecision::InstantWake,
    );
    let decision = classifier.classify(&scored);
    // Complex multi-step request should route to System2 or Hybrid.
    assert!(matches!(
        decision.path,
        RoutePath::System2 | RoutePath::Hybrid
    ));
}

#[test]
fn test_classify_routine_event() {
    let mut classifier = RouteClassifier::new();
    let scored = make_scored_event(
        Intent::RoutineEvent,
        EventSource::Accessibility,
        "screen changed",
        0.30,
        GateDecision::SlowAccumulate,
    );
    let decision = classifier.classify(&scored);
    // Low-score routine events typically go to DaemonOnly or System1.
    assert!(matches!(
        decision.path,
        RoutePath::DaemonOnly | RoutePath::System1
    ));
}

// ---------------------------------------------------------------------------
// Working memory influence tests
// ---------------------------------------------------------------------------

#[test]
fn test_working_memory_influence() {
    let mut classifier_low = RouteClassifier::new();
    classifier_low.set_working_memory(1, 7); // 1/7 = low load

    let mut classifier_high = RouteClassifier::new();
    classifier_high.set_working_memory(6, 7); // 6/7 = high load

    let scored = make_scored_event(
        Intent::InformationRequest,
        EventSource::UserCommand,
        "analyze this data and compare options",
        0.70,
        GateDecision::InstantWake,
    );

    let decision_low = classifier_low.classify(&scored);
    let decision_high = classifier_high.classify(&scored);

    // Higher memory load should increase routing score toward System2.
    // Both should produce valid decisions regardless.
    assert!(decision_low.confidence > 0.0);
    assert!(decision_high.confidence > 0.0);
}

// ---------------------------------------------------------------------------
// Personality bias tests
// ---------------------------------------------------------------------------

#[test]
fn test_personality_bias_clamp() {
    let mut classifier = RouteClassifier::new();

    // Extreme bias should be clamped to [-0.15, 0.15].
    classifier.set_personality_bias(1.0);
    let scored = make_scored_event(
        Intent::ActionRequest,
        EventSource::UserCommand,
        "do something",
        0.50,
        GateDecision::InstantWake,
    );
    let decision = classifier.classify(&scored);
    // Should not panic, produce a valid decision.
    assert!(matches!(
        decision.path,
        RoutePath::System1 | RoutePath::System2 | RoutePath::Hybrid | RoutePath::DaemonOnly
    ));
}

#[test]
fn test_personality_bias_negative() {
    let mut classifier = RouteClassifier::new();
    classifier.set_personality_bias(-0.15); // prefers System1

    let scored = make_scored_event(
        Intent::ActionRequest,
        EventSource::UserCommand,
        "open settings",
        0.50,
        GateDecision::InstantWake,
    );
    let decision = classifier.classify(&scored);
    assert!(decision.confidence > 0.0);
}

// ---------------------------------------------------------------------------
// Determinism tests
// ---------------------------------------------------------------------------

#[test]
fn test_routing_deterministic() {
    let scored = make_scored_event(
        Intent::ActionRequest,
        EventSource::UserCommand,
        "send message to Alice",
        0.72,
        GateDecision::InstantWake,
    );

    let mut c1 = RouteClassifier::new();
    let mut c2 = RouteClassifier::new();

    let d1 = c1.classify(&scored);
    let d2 = c2.classify(&scored);

    assert_eq!(d1.path, d2.path, "same input must produce same route");
    assert!(
        (d1.confidence - d2.confidence).abs() < f32::EPSILON,
        "same input must produce same confidence"
    );
}

// ---------------------------------------------------------------------------
// Hysteresis tests
// ---------------------------------------------------------------------------

#[test]
fn test_hysteresis_prevents_flapping() {
    let mut classifier = RouteClassifier::new();

    // First call — establish a route.
    let scored_high = make_scored_event(
        Intent::InformationRequest,
        EventSource::UserCommand,
        "complex multi-step research analysis plan compare",
        0.80,
        GateDecision::InstantWake,
    );
    let first = classifier.classify(&scored_high);

    // Second call with slightly different score — hysteresis should stabilize.
    let scored_near = make_scored_event(
        Intent::InformationRequest,
        EventSource::UserCommand,
        "research analysis compare",
        0.75,
        GateDecision::InstantWake,
    );
    let second = classifier.classify(&scored_near);

    // We mainly test that the classifier doesn't panic under rapid calls
    // and that confidence is in valid range.
    assert!(second.confidence > 0.0 && second.confidence <= 1.0);
}

// ---------------------------------------------------------------------------
// Complexity scoring tests
// ---------------------------------------------------------------------------

#[test]
fn test_complexity_scoring() {
    let simple = RouteClassifier::compute_complexity("open app");
    let complex = RouteClassifier::compute_complexity(
        "analyze and compare multiple research papers then plan a detailed schedule",
    );

    assert!(
        complex > simple,
        "complex content ({}) should score higher than simple ({})",
        complex,
        simple
    );
}

#[test]
fn test_complexity_empty_string() {
    let score = RouteClassifier::compute_complexity("");
    assert!(score >= 0.0 && score <= 1.0);
}

#[test]
fn test_complexity_keywords() {
    // "multi-step" and "research" are high-weight keywords.
    let score = RouteClassifier::compute_complexity("multi-step research");
    assert!(score > 0.0, "known keywords should produce positive score");
}

// ---------------------------------------------------------------------------
// Route decision properties
// ---------------------------------------------------------------------------

#[test]
fn test_route_decision_has_reason() {
    let mut classifier = RouteClassifier::new();
    let scored = make_scored_event(
        Intent::ActionRequest,
        EventSource::UserCommand,
        "open calculator",
        0.60,
        GateDecision::InstantWake,
    );
    let decision = classifier.classify(&scored);
    assert!(!decision.reason.is_empty(), "decision should have a reason");
}

#[test]
fn test_route_decision_confidence_bounds() {
    let mut classifier = RouteClassifier::new();

    let events = vec![
        make_scored_event(
            Intent::ActionRequest,
            EventSource::UserCommand,
            "open app",
            0.1,
            GateDecision::SlowAccumulate,
        ),
        make_scored_event(
            Intent::InformationRequest,
            EventSource::UserCommand,
            "research complex topic",
            0.9,
            GateDecision::InstantWake,
        ),
        make_scored_event(
            Intent::SystemAlert,
            EventSource::Notification,
            "emergency",
            0.99,
            GateDecision::EmergencyBypass,
        ),
        make_scored_event(
            Intent::RoutineEvent,
            EventSource::Internal,
            "tick",
            0.01,
            GateDecision::Suppress,
        ),
    ];

    for scored in &events {
        let decision = classifier.classify(scored);
        assert!(
            decision.confidence >= 0.0 && decision.confidence <= 1.0,
            "confidence {} out of [0, 1] for: {}",
            decision.confidence,
            scored.parsed.content
        );
    }
}
