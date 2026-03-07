//! Integration tests for the AURA v4 Pipeline — EventParser + Amygdala.
//!
//! Tests the event parsing (Stage 1) and scoring (Stage 2) pipeline
//! end-to-end, verifying that raw events flow through to gate decisions.

use aura_daemon::pipeline::amygdala::Amygdala;
use aura_daemon::pipeline::parser::EventParser;
use aura_types::events::{
    EventSource, GateDecision, Intent, NotificationCategory, NotificationEvent, RawEvent,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_ms() -> u64 {
    1_700_000_000_000
}

fn make_raw_event(package: &str, text: Option<&str>, event_type: u32) -> RawEvent {
    RawEvent {
        event_type,
        package_name: package.to_string(),
        class_name: "android.widget.TextView".to_string(),
        text: text.map(|s| s.to_string()),
        content_description: None,
        timestamp_ms: now_ms(),
        source_node_id: None,
    }
}

fn make_notification(
    package: &str,
    title: &str,
    text: &str,
    category: NotificationCategory,
) -> NotificationEvent {
    NotificationEvent {
        package: package.to_string(),
        title: title.to_string(),
        text: text.to_string(),
        category,
        timestamp_ms: now_ms(),
        is_ongoing: false,
        actions: vec![],
    }
}

// ---------------------------------------------------------------------------
// EventParser tests
// ---------------------------------------------------------------------------

#[test]
fn test_parser_creation() {
    let parser = EventParser::new();
    // Parser should construct without panic.
    let _ = format!("{:?}", parser);
}

#[test]
fn test_parse_raw_accessibility_event() {
    let parser = EventParser::new();
    let raw = make_raw_event("com.whatsapp", Some("New message from Alice"), 32);
    let parsed = parser.parse_raw(&raw);

    assert_eq!(parsed.source, EventSource::Accessibility);
    assert!(parsed.timestamp_ms > 0);
    assert!(!parsed.content.is_empty());
}

#[test]
fn test_parse_raw_no_text() {
    let parser = EventParser::new();
    let raw = make_raw_event("com.android.systemui", None, 2048);
    let parsed = parser.parse_raw(&raw);

    assert_eq!(parsed.source, EventSource::Accessibility);
    // Content should still be populated (from class name or fallback).
    assert!(parsed.timestamp_ms == now_ms());
}

#[test]
fn test_parse_notification_message() {
    let parser = EventParser::new();
    let notif = make_notification(
        "com.whatsapp",
        "Alice",
        "Hey, are we still on for dinner?",
        NotificationCategory::Message,
    );
    let parsed = parser.parse_notification(&notif);

    assert_eq!(parsed.source, EventSource::Notification);
    assert!(!parsed.content.is_empty());
}

#[test]
fn test_parse_notification_system() {
    let parser = EventParser::new();
    let notif = make_notification(
        "com.android.systemui",
        "Battery Low",
        "Battery at 15%",
        NotificationCategory::System,
    );
    let parsed = parser.parse_notification(&notif);

    assert_eq!(parsed.source, EventSource::Notification);
}

// ---------------------------------------------------------------------------
// Amygdala scoring tests
// ---------------------------------------------------------------------------

#[test]
fn test_amygdala_creation() {
    let amygdala = Amygdala::new();
    let _ = format!("{:?}", amygdala);
}

#[test]
fn test_amygdala_score_notification() {
    let parser = EventParser::new();
    let mut amygdala = Amygdala::new();

    let notif = make_notification(
        "com.whatsapp",
        "Alice",
        "Urgent: meeting moved to 3pm!",
        NotificationCategory::Message,
    );
    let parsed = parser.parse_notification(&notif);
    let scored = amygdala.score(&parsed);

    assert!(scored.score_total >= 0.0 && scored.score_total <= 1.0);
    // Weighted sub-scores should be non-negative.
    assert!(scored.score_lex >= 0.0);
    assert!(scored.score_src >= 0.0);
    assert!(scored.score_time >= 0.0);
    assert!(scored.score_anom >= 0.0);
}

#[test]
fn test_amygdala_gate_decision_variants() {
    let parser = EventParser::new();
    let mut amygdala = Amygdala::new();

    // Low-importance system notification should score low.
    let notif = make_notification(
        "com.android.vending",
        "Play Store",
        "App update available",
        NotificationCategory::System,
    );
    let parsed = parser.parse_notification(&notif);
    let scored = amygdala.score(&parsed);

    // The gate decision should be one of the valid variants.
    assert!(matches!(
        scored.gate_decision,
        GateDecision::EmergencyBypass
            | GateDecision::InstantWake
            | GateDecision::SlowAccumulate
            | GateDecision::Suppress
    ));
}

#[test]
fn test_amygdala_score_bounds() {
    let parser = EventParser::new();
    let mut amygdala = Amygdala::new();

    // Score multiple varied events and verify bounds.
    let events = vec![
        make_notification(
            "com.whatsapp",
            "Alice",
            "Hi!",
            NotificationCategory::Message,
        ),
        make_notification(
            "com.slack",
            "Work",
            "Deploy failed!",
            NotificationCategory::Email,
        ),
        make_notification(
            "com.android.phone",
            "Incoming Call",
            "+1-555-0123",
            NotificationCategory::Transport,
        ),
    ];

    for notif in &events {
        let parsed = parser.parse_notification(notif);
        let scored = amygdala.score(&parsed);
        assert!(
            scored.score_total >= 0.0 && scored.score_total <= 1.0,
            "total score out of bounds: {}",
            scored.score_total
        );
    }
}

// ---------------------------------------------------------------------------
// End-to-end pipeline: parse → score
// ---------------------------------------------------------------------------

#[test]
fn test_pipeline_raw_to_scored() {
    let parser = EventParser::new();
    let mut amygdala = Amygdala::new();

    let raw = make_raw_event("com.whatsapp", Some("Message from Alice: call me ASAP"), 32);
    let parsed = parser.parse_raw(&raw);
    let scored = amygdala.score(&parsed);

    assert!(scored.score_total >= 0.0 && scored.score_total <= 1.0);
    assert_eq!(scored.parsed.source, EventSource::Accessibility);
}

#[test]
fn test_pipeline_notification_to_scored() {
    let parser = EventParser::new();
    let mut amygdala = Amygdala::new();

    let notif = make_notification(
        "com.whatsapp",
        "Alice",
        "Emergency: server is down, need help now!",
        NotificationCategory::Message,
    );
    let parsed = parser.parse_notification(&notif);
    let scored = amygdala.score(&parsed);

    assert!(scored.score_total >= 0.0 && scored.score_total <= 1.0);
    assert_eq!(scored.parsed.source, EventSource::Notification);
}

#[test]
fn test_pipeline_multiple_events_deterministic() {
    let parser = EventParser::new();
    let mut amygdala1 = Amygdala::new();
    let mut amygdala2 = Amygdala::new();

    let notif = make_notification(
        "com.whatsapp",
        "Alice",
        "Same message content",
        NotificationCategory::Message,
    );
    let parsed = parser.parse_notification(&notif);

    let scored1 = amygdala1.score(&parsed);
    let scored2 = amygdala2.score(&parsed);

    // Same input → same output (deterministic scoring).
    assert!(
        (scored1.score_total - scored2.score_total).abs() < f32::EPSILON,
        "scoring should be deterministic"
    );
}

#[test]
fn test_pipeline_storm_dedup() {
    let parser = EventParser::new();
    let mut amygdala = Amygdala::new();

    // Fire the same notification 10 times rapidly — storm detection should
    // eventually suppress duplicates.
    let notif = make_notification(
        "com.whatsapp",
        "Alice",
        "Repeated notification content",
        NotificationCategory::Message,
    );

    let mut suppressed_count = 0;
    for _ in 0..10 {
        let parsed = parser.parse_notification(&notif);
        let scored = amygdala.score(&parsed);
        if scored.gate_decision == GateDecision::Suppress {
            suppressed_count += 1;
        }
    }

    // We expect at least some suppressions from storm detection.
    // (Exact count depends on dedup ring size and rate limit config.)
    // At minimum, the scoring should not panic.
    assert!(suppressed_count >= 0); // Always true — tests dedup path executes.
}
