use serde::{Deserialize, Serialize};

// ── DaemonEvent — daemon lifecycle & health events ──────────────────────────

/// A snapshot of the daemon's health at a point in time.
///
/// Embedded in [`DaemonEvent::Heartbeat`] and passed to subsystems that react
/// to resource constraints (memory consolidation, LLM throttle, proactive
/// initiative budget).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthSnapshot {
    /// Timestamp (ms since UNIX epoch) when this snapshot was taken.
    pub timestamp_ms: u64,
    /// Battery level as an integer percentage [0, 100].
    pub battery_pct: u8,
    /// Memory currently consumed by the daemon process in bytes.
    pub memory_usage_bytes: u64,
    /// Thermal severity: 0 = Normal, 1 = Warm, 2 = Hot, 3 = Critical, 4 = Shutdown.
    pub thermal_level: u8,
    /// `true` when battery is below the 10% low-power threshold.
    pub low_power_mode: bool,
    /// `true` when memory pressure is above the critical threshold.
    pub memory_pressure_critical: bool,
    /// `true` when the device is actively charging.
    #[serde(default)]
    pub is_charging: bool,
}

/// Lifecycle and resource events emitted by the AURA daemon's self-management
/// system.
///
/// These events flow through the heartbeat channel (`health_event_tx`) and are
/// consumed by the main loop's `select!` branch to trigger reactive subsystem
/// adjustments — all on-device, no cloud dependency.
///
/// # Design invariant
///
/// All variants are cheaply cloneable and `Serialize`/`Deserialize` so they
/// can be persisted to the telemetry DB or forwarded over IPC without extra
/// conversion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonEvent {
    // ── Heartbeat / periodic ──────────────────────────────────────────────

    /// Periodic heartbeat carrying a full health snapshot.
    ///
    /// Emitted by the `run_heartbeat_loop` task at the interval dictated by
    /// `HealthMonitor::check_interval_ms()` (30 s normal, 60 s low-power).
    Heartbeat(HealthSnapshot),

    // ── Memory pressure ───────────────────────────────────────────────────

    /// Daemon RSS crossed the memory-pressure warning threshold.
    ///
    /// `critical = false` → warm consolidation should run.
    /// `critical = true`  → activate safe mode and stop all non-essential work.
    MemoryPressure {
        /// Whether the situation is critical (vs. merely elevated).
        critical: bool,
        /// Current RSS in bytes at the time the event was raised.
        current_bytes: u64,
        /// The threshold that was crossed, in bytes.
        threshold_bytes: u64,
    },

    // ── Battery ───────────────────────────────────────────────────────────

    /// Battery dropped below 20 % — reduce proactive initiative budget.
    BatteryLow {
        /// Battery level as a percentage [0, 100] at the time of the event.
        pct: u8,
    },

    /// Battery dropped below 5 % — halt all proactive work immediately.
    BatteryCritical {
        /// Battery level as a percentage [0, 100] at the time of the event.
        pct: u8,
    },

    // ── Thermal ───────────────────────────────────────────────────────────

    /// Thermal state is Critical or Shutdown — pause LLM inference tasks.
    ///
    /// Normal service resumes automatically when a subsequent `Heartbeat`
    /// reports a thermal level below Critical.
    ThermalCritical,

    // ── Lifecycle ─────────────────────────────────────────────────────────

    /// All startup phases completed successfully.  The daemon is ready to
    /// accept accessibility events, user commands, and IPC messages.
    DaemonReady {
        /// Daemon version string (e.g. "4.0.0").
        version: String,
    },

    /// Daemon is shutting down gracefully (cancel flag set).  Consumers
    /// should flush pending state and release resources.
    DaemonShutdown {
        /// Human-readable reason for shutdown (e.g. "cancel flag set").
        reason: String,
    },
}

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
    /// Optional full accessibility tree snapshot, attached by the JNI bridge
    /// for `TYPE_WINDOW_STATE_CHANGED` (event_type == 32) events.
    /// When present, enables ScreenCache + SemanticGraph processing in the
    /// main loop.  When absent, the loop falls back to lightweight
    /// package::class summaries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_nodes: Option<Vec<crate::screen::ScreenNode>>,
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
    /// Bounded: max MAX_NOTIFICATION_ACTIONS items enforced at collection site.
    pub actions: Vec<String>,
}

/// Max action buttons in a [`NotificationEvent`].
pub const MAX_NOTIFICATION_ACTIONS: usize = 8;

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
///
/// Architecture law: the `intent` field MUST be populated by the LLM (neocortex) only.
/// The daemon is strictly forbidden from performing keyword matching or regex-based
/// intent classification. The daemon routes raw events; the LLM reasons about intent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedEvent {
    pub source: EventSource,
    pub intent: Intent,
    pub content: String,
    /// Bounded: max MAX_PARSED_EVENT_ENTITIES items enforced at collection site.
    pub entities: Vec<String>,
    pub timestamp_ms: u64,
    pub raw_event_type: u32,
}

/// Max extracted entities in a [`ParsedEvent`].
pub const MAX_PARSED_EVENT_ENTITIES: usize = 16;

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
///
/// Architecture law: this enum records a fact determined by the LLM. The daemon
/// MUST NOT infer or assign this value via keyword matching, regex, or heuristics.
/// The daemon sets `Intent::RoutineEvent` as a neutral default; only the LLM
/// may classify into any other variant.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Intent {
    InformationRequest,
    ActionRequest,
    ConversationContinue,
    SystemAlert,
    ProactiveOpportunity,
    RoutineEvent,
}

impl Intent {
    /// Returns a stable, lowercase-kebab string label for this intent.
    /// Used by OutcomeBus, ReactionDetector, and BDI belief keys.
    pub fn as_str(&self) -> &'static str {
        match self {
            Intent::InformationRequest => "information-request",
            Intent::ActionRequest => "action-request",
            Intent::ConversationContinue => "conversation-continue",
            Intent::SystemAlert => "system-alert",
            Intent::ProactiveOpportunity => "proactive-opportunity",
            Intent::RoutineEvent => "routine-event",
        }
    }
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
            raw_nodes: None,
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
