use serde::{Deserialize, Serialize};

/// Raw accessibility event directly from Android's AccessibilityService.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub event_type: u32,
    pub package_name: String,
    pub class_name: String,
    pub text: Option<String>,
    pub content_description: Option<String>,
    pub timestamp_ms: u64,
    pub source_node_id: Option<String>,
}

/// Notification received from NotificationListenerService.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationEvent {
    pub package: String,
    pub title: String,
    pub text: String,
    pub category: NotificationCategory,
    pub timestamp_ms: u64,
    pub is_ongoing: bool,
    pub actions: Vec<String>,
}

/// Category classification for notifications.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NotificationCategory {
    Message,
    Email,
    Social,
    Transport,
    Alarm,
    Reminder,
    System,
    Other,
}

/// Parsed event after Stage 1 processing — intent classified, entities extracted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedEvent {
    pub source: EventSource,
    pub intent: Intent,
    pub content: String,
    pub entities: Vec<String>,
    pub timestamp_ms: u64,
    pub raw_event_type: u32,
}

/// Where the event originated.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventSource {
    Accessibility,
    Notification,
    UserCommand,
    Cron,
    Internal,
}

/// Classified intent of the event.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Intent {
    InformationRequest,
    ActionRequest,
    ConversationContinue,
    SystemAlert,
    ProactiveOpportunity,
    RoutineEvent,
}

/// Scored event after Stage 2 (Amygdala) — relevance scored, gate decision made.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredEvent {
    pub parsed: ParsedEvent,
    /// Total composite score (0.0–1.0).
    pub score_total: f32,
    /// Lexical keyword score.
    pub score_lex: f32,
    /// Source weight score.
    pub score_src: f32,
    /// Temporal relevance score.
    pub score_time: f32,
    /// Anomaly score.
    pub score_anom: f32,
    /// Gate decision based on score thresholds.
    pub gate_decision: GateDecision,
}

/// Gate decision from the Amygdala scoring pipeline.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum GateDecision {
    /// Score ≥ 0.90 — bypass all queues, immediate processing.
    EmergencyBypass,
    /// Score ≥ threshold (default 0.65) — wake neocortex immediately.
    InstantWake,
    /// Score below threshold — accumulate until batch threshold met.
    SlowAccumulate,
    /// Score near zero or duplicate — discard silently.
    Suppress,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raw_event_creation() {
        let event = RawEvent {
            event_type: 32,
            package_name: "com.whatsapp".to_string(),
            class_name: "android.widget.TextView".to_string(),
            text: Some("Hello!".to_string()),
            content_description: None,
            timestamp_ms: 1_700_000_000_000,
            source_node_id: Some("node_42".to_string()),
        };
        assert_eq!(event.event_type, 32);
        assert_eq!(event.package_name, "com.whatsapp");
        assert!(event.text.is_some());
        assert!(event.content_description.is_none());
    }

    #[test]
    fn test_notification_category_eq() {
        assert_eq!(NotificationCategory::Message, NotificationCategory::Message);
        assert_ne!(NotificationCategory::Email, NotificationCategory::Social);
    }

    #[test]
    fn test_gate_decision_variants() {
        let decisions = [
            GateDecision::EmergencyBypass,
            GateDecision::InstantWake,
            GateDecision::SlowAccumulate,
            GateDecision::Suppress,
        ];
        for d in &decisions {
            let serialized = serde_json::to_string(d).unwrap();
            let deser: GateDecision = serde_json::from_str(&serialized).unwrap();
            assert_eq!(*d, deser);
        }
    }

    #[test]
    fn test_scored_event_construction() {
        let parsed = ParsedEvent {
            source: EventSource::Notification,
            intent: Intent::ActionRequest,
            content: "New message from Alice".to_string(),
            entities: vec!["Alice".to_string()],
            timestamp_ms: 1_700_000_000_000,
            raw_event_type: 64,
        };
        let scored = ScoredEvent {
            parsed,
            score_total: 0.78,
            score_lex: 0.30,
            score_src: 0.20,
            score_time: 0.18,
            score_anom: 0.10,
            gate_decision: GateDecision::InstantWake,
        };
        assert!(scored.score_total > 0.65);
        assert_eq!(scored.gate_decision, GateDecision::InstantWake);
    }
}
