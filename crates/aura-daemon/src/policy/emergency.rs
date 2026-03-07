//! Emergency Stop System — kill switch for AURA.
//!
//! When things go wrong, the emergency stop immediately halts all action
//! execution, disables proactive behaviors, and alerts the user.
//!
//! # Trigger Conditions
//!
//! - Action failure rate > 50% in recent window
//! - Same action repeated > 5 times in 30 seconds (loop detection)
//! - User explicitly says "stop" / "halt" / "emergency"
//! - Screen state not changing despite actions (stuck/frozen)
//! - Watchdog timeout (main loop hasn't heartbeated)
//!
//! # State Machine
//!
//! ```text
//! Normal ──trigger──► Activated ──recover──► Recovering ──resume──► Normal
//!                         │                       │
//!                         └───(still failing)─────┘
//! ```
//!
//! # Recovery
//!
//! After an emergency stop, AURA does NOT immediately resume full activity.
//! It enters a gradual ramp-up phase: reduced action rate, elevated
//! logging, and user confirmation for anything above L0.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use aura_types::errors::SecurityError;

// ---------------------------------------------------------------------------
// EmergencyState
// ---------------------------------------------------------------------------

/// The current state of the emergency stop system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmergencyState {
    /// Normal operation — no emergency.
    Normal,
    /// Emergency stop is active — all actions halted.
    Activated,
    /// Recovering from an emergency — gradual ramp-up.
    Recovering,
}

impl std::fmt::Display for EmergencyState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normal => write!(f, "NORMAL"),
            Self::Activated => write!(f, "EMERGENCY_ACTIVATED"),
            Self::Recovering => write!(f, "RECOVERING"),
        }
    }
}

// ---------------------------------------------------------------------------
// EmergencyReason
// ---------------------------------------------------------------------------

/// Why the emergency stop was triggered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EmergencyReason {
    /// Action failure rate exceeded threshold.
    HighFailureRate {
        failure_count: u32,
        total_count: u32,
    },
    /// Same action repeated too many times (loop detected).
    ActionLoop {
        action: String,
        count: u32,
        window_seconds: u32,
    },
    /// User explicitly requested stop.
    UserRequested { trigger_phrase: String },
    /// Screen not changing despite actions (frozen/crashed app).
    ScreenFrozen { unchanged_actions: u32 },
    /// Watchdog timeout — main loop missed heartbeat.
    WatchdogTimeout { last_heartbeat_ms_ago: u64 },
    /// Manual trigger from code/API.
    ManualTrigger { reason: String },
}

impl std::fmt::Display for EmergencyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HighFailureRate {
                failure_count,
                total_count,
            } => write!(
                f,
                "high failure rate: {failure_count}/{total_count} actions failed"
            ),
            Self::ActionLoop {
                action,
                count,
                window_seconds,
            } => write!(
                f,
                "action loop: '{action}' repeated {count}x in {window_seconds}s"
            ),
            Self::UserRequested { trigger_phrase } => {
                write!(f, "user requested: \"{trigger_phrase}\"")
            }
            Self::ScreenFrozen { unchanged_actions } => {
                write!(
                    f,
                    "screen frozen: {unchanged_actions} actions with no screen change"
                )
            }
            Self::WatchdogTimeout {
                last_heartbeat_ms_ago,
            } => write!(
                f,
                "watchdog timeout: {last_heartbeat_ms_ago}ms since last heartbeat"
            ),
            Self::ManualTrigger { reason } => write!(f, "manual: {reason}"),
        }
    }
}

// ---------------------------------------------------------------------------
// IncidentReport
// ---------------------------------------------------------------------------

/// A record of an emergency stop incident.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentReport {
    /// When the emergency was triggered (unix ms).
    pub triggered_at_ms: u64,
    /// Why it was triggered.
    pub reason: EmergencyReason,
    /// State when triggered (what was AURA doing).
    pub context: String,
    /// How long the emergency lasted (ms), if resolved.
    pub duration_ms: Option<u64>,
    /// Whether recovery was successful.
    pub recovered: bool,
}

// ---------------------------------------------------------------------------
// AnomalyDetector
// ---------------------------------------------------------------------------

/// Sliding-window anomaly detector for automatic emergency triggers.
pub struct AnomalyDetector {
    /// Recent action outcomes: true = success, false = failure.
    recent_outcomes: VecDeque<(Instant, bool)>,
    /// Recent action strings for loop detection.
    recent_actions: VecDeque<(Instant, String)>,
    /// Recent screen-changed flags.
    recent_screen_changes: VecDeque<(Instant, bool)>,

    // Configuration
    /// Window size for failure rate calculation.
    failure_window: Duration,
    /// Number of recent actions to track.
    max_recent_actions: usize,
    /// Failure rate threshold (0.0 - 1.0).
    failure_rate_threshold: f32,
    /// Minimum actions before failure rate check applies.
    min_actions_for_rate: u32,
    /// Max identical actions within loop window.
    loop_max_repeats: u32,
    /// Loop detection window.
    loop_window: Duration,
    /// Max actions without screen change before declaring frozen.
    frozen_threshold: u32,
}

impl AnomalyDetector {
    /// Create a new anomaly detector with default configuration.
    pub fn new() -> Self {
        Self {
            recent_outcomes: VecDeque::new(),
            recent_actions: VecDeque::new(),
            recent_screen_changes: VecDeque::new(),
            failure_window: Duration::from_secs(60),
            max_recent_actions: 50,
            failure_rate_threshold: 0.5,
            min_actions_for_rate: 10,
            loop_max_repeats: 5,
            loop_window: Duration::from_secs(30),
            frozen_threshold: 10,
        }
    }

    /// Record an action outcome.
    pub fn record_outcome(&mut self, success: bool) {
        let now = Instant::now();
        self.recent_outcomes.push_back((now, success));
        self.prune_old_outcomes(now);
    }

    /// Record an action string for loop detection.
    pub fn record_action(&mut self, action: &str) {
        let now = Instant::now();
        self.recent_actions.push_back((now, action.to_string()));
        while self.recent_actions.len() > self.max_recent_actions {
            self.recent_actions.pop_front();
        }
    }

    /// Record whether the screen changed after an action.
    pub fn record_screen_change(&mut self, changed: bool) {
        let now = Instant::now();
        self.recent_screen_changes.push_back((now, changed));
        while self.recent_screen_changes.len() > self.max_recent_actions {
            self.recent_screen_changes.pop_front();
        }
    }

    /// Check for anomalies. Returns the first detected anomaly reason, if any.
    pub fn detect_anomaly(&self) -> Option<EmergencyReason> {
        // 1. Check failure rate.
        if let Some(reason) = self.check_failure_rate() {
            return Some(reason);
        }

        // 2. Check action loops.
        if let Some(reason) = self.check_action_loop() {
            return Some(reason);
        }

        // 3. Check screen frozen.
        if let Some(reason) = self.check_screen_frozen() {
            return Some(reason);
        }

        None
    }

    /// Check if the failure rate exceeds threshold.
    fn check_failure_rate(&self) -> Option<EmergencyReason> {
        let now = Instant::now();
        let recent: Vec<_> = self
            .recent_outcomes
            .iter()
            .filter(|(t, _)| now.duration_since(*t) <= self.failure_window)
            .collect();

        let total = recent.len() as u32;
        if total < self.min_actions_for_rate {
            return None;
        }

        let failures = recent.iter().filter(|(_, ok)| !ok).count() as u32;
        let rate = failures as f32 / total as f32;

        if rate > self.failure_rate_threshold {
            Some(EmergencyReason::HighFailureRate {
                failure_count: failures,
                total_count: total,
            })
        } else {
            None
        }
    }

    /// Check if the same action is being repeated (loop).
    fn check_action_loop(&self) -> Option<EmergencyReason> {
        let now = Instant::now();

        // Group recent actions by string within the loop window.
        let recent_in_window: Vec<_> = self
            .recent_actions
            .iter()
            .filter(|(t, _)| now.duration_since(*t) <= self.loop_window)
            .collect();

        if recent_in_window.is_empty() {
            return None;
        }

        // Count occurrences of each action.
        let mut counts: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
        for (_, action) in &recent_in_window {
            *counts.entry(action.as_str()).or_insert(0) += 1;
        }

        for (action, count) in &counts {
            if *count > self.loop_max_repeats {
                return Some(EmergencyReason::ActionLoop {
                    action: action.to_string(),
                    count: *count,
                    window_seconds: self.loop_window.as_secs() as u32,
                });
            }
        }

        None
    }

    /// Check if the screen is frozen (no changes despite actions).
    fn check_screen_frozen(&self) -> Option<EmergencyReason> {
        // Look at the most recent N screen change records.
        let recent: Vec<_> = self
            .recent_screen_changes
            .iter()
            .rev()
            .take(self.frozen_threshold as usize)
            .collect();

        if recent.len() < self.frozen_threshold as usize {
            return None;
        }

        let all_unchanged = recent.iter().all(|(_, changed)| !changed);
        if all_unchanged {
            Some(EmergencyReason::ScreenFrozen {
                unchanged_actions: self.frozen_threshold,
            })
        } else {
            None
        }
    }

    /// Check if user text contains an emergency stop phrase.
    pub fn check_user_stop_phrase(text: &str) -> Option<EmergencyReason> {
        let lower = text.to_ascii_lowercase();
        let stop_phrases = [
            "stop",
            "halt",
            "emergency",
            "emergency stop",
            "kill",
            "abort",
            "stop everything",
            "halt aura",
        ];

        for phrase in &stop_phrases {
            if lower.contains(phrase) {
                return Some(EmergencyReason::UserRequested {
                    trigger_phrase: phrase.to_string(),
                });
            }
        }

        None
    }

    /// Prune outcomes older than the failure window.
    fn prune_old_outcomes(&mut self, now: Instant) {
        while let Some((t, _)) = self.recent_outcomes.front() {
            if now.duration_since(*t) > self.failure_window {
                self.recent_outcomes.pop_front();
            } else {
                break;
            }
        }
    }

    /// Reset all tracked state.
    pub fn reset(&mut self) {
        self.recent_outcomes.clear();
        self.recent_actions.clear();
        self.recent_screen_changes.clear();
    }
}

impl Default for AnomalyDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Watchdog
// ---------------------------------------------------------------------------

/// Watchdog timer — triggers emergency if the main loop stops heartbeating.
pub struct Watchdog {
    /// Last heartbeat time.
    last_heartbeat: Instant,
    /// Maximum allowed interval between heartbeats.
    timeout: Duration,
}

impl Watchdog {
    /// Create a new watchdog with the given timeout.
    pub fn new(timeout: Duration) -> Self {
        Self {
            last_heartbeat: Instant::now(),
            timeout,
        }
    }

    /// Create a watchdog with the default 30-second timeout.
    pub fn with_default_timeout() -> Self {
        Self::new(Duration::from_secs(30))
    }

    /// Record a heartbeat from the main loop.
    pub fn heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
    }

    /// Check if the watchdog has timed out.
    pub fn is_timed_out(&self) -> bool {
        self.last_heartbeat.elapsed() > self.timeout
    }

    /// Time since last heartbeat.
    pub fn time_since_heartbeat(&self) -> Duration {
        self.last_heartbeat.elapsed()
    }

    /// Check and return an anomaly reason if timed out.
    pub fn check(&self) -> Option<EmergencyReason> {
        if self.is_timed_out() {
            Some(EmergencyReason::WatchdogTimeout {
                last_heartbeat_ms_ago: self.last_heartbeat.elapsed().as_millis() as u64,
            })
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// EmergencyStop
// ---------------------------------------------------------------------------

/// The emergency stop controller.
///
/// Manages the state machine, anomaly detection, watchdog timer,
/// and incident reporting.
pub struct EmergencyStop {
    /// Current state.
    state: EmergencyState,
    /// Anomaly detector.
    detector: AnomalyDetector,
    /// Watchdog timer.
    watchdog: Watchdog,
    /// History of incidents.
    incidents: Vec<IncidentReport>,
    /// Maximum incidents to retain.
    max_incidents: usize,
    /// When the current emergency was activated (if any).
    activated_at: Option<Instant>,
    /// Actions allowed per minute during recovery (ramp-up).
    recovery_rate_limit: u32,
    /// Number of recovery attempts for the current incident.
    recovery_attempts: u32,
    /// Maximum recovery attempts before requiring user intervention.
    max_recovery_attempts: u32,
}

impl EmergencyStop {
    /// Create a new emergency stop system.
    pub fn new() -> Self {
        Self {
            state: EmergencyState::Normal,
            detector: AnomalyDetector::new(),
            watchdog: Watchdog::with_default_timeout(),
            incidents: Vec::new(),
            max_incidents: 100,
            activated_at: None,
            recovery_rate_limit: 10,
            recovery_attempts: 0,
            max_recovery_attempts: 3,
        }
    }

    /// Current state.
    pub fn state(&self) -> EmergencyState {
        self.state
    }

    /// Whether actions are currently allowed.
    pub fn actions_allowed(&self) -> bool {
        self.state == EmergencyState::Normal || self.state == EmergencyState::Recovering
    }

    /// Whether we are in an emergency state.
    pub fn is_emergency(&self) -> bool {
        self.state == EmergencyState::Activated
    }

    /// Whether we are recovering.
    pub fn is_recovering(&self) -> bool {
        self.state == EmergencyState::Recovering
    }

    /// Get the anomaly detector for recording observations.
    pub fn detector(&self) -> &AnomalyDetector {
        &self.detector
    }

    /// Get a mutable reference to the anomaly detector.
    pub fn detector_mut(&mut self) -> &mut AnomalyDetector {
        &mut self.detector
    }

    /// Record a main-loop heartbeat.
    pub fn heartbeat(&mut self) {
        self.watchdog.heartbeat();
    }

    // -----------------------------------------------------------------------
    // Activation
    // -----------------------------------------------------------------------

    /// Activate the emergency stop.
    ///
    /// Immediately halts all actions:
    /// - Sets state to Activated
    /// - Records an incident
    /// - Logs the emergency
    ///
    /// Returns the incident report.
    pub fn activate(
        &mut self,
        reason: EmergencyReason,
        context: &str,
    ) -> Result<IncidentReport, SecurityError> {
        if self.state == EmergencyState::Activated {
            return Err(SecurityError::EmergencyActive {
                reason: "already activated".to_string(),
            });
        }

        self.state = EmergencyState::Activated;
        self.activated_at = Some(Instant::now());

        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let report = IncidentReport {
            triggered_at_ms: timestamp_ms,
            reason: reason.clone(),
            context: context.to_string(),
            duration_ms: None,
            recovered: false,
        };

        // Store incident.
        if self.incidents.len() >= self.max_incidents {
            self.incidents.remove(0);
        }
        self.incidents.push(report.clone());

        tracing::error!(
            target: "SECURITY",
            reason = %reason,
            context = context,
            "EMERGENCY STOP ACTIVATED"
        );

        Ok(report)
    }

    /// Check for anomalies and auto-trigger if detected.
    ///
    /// This should be called periodically (e.g. on each main-loop tick).
    /// Returns the reason if an emergency was triggered.
    pub fn check_and_trigger(&mut self) -> Option<EmergencyReason> {
        if self.state == EmergencyState::Activated {
            return None; // Already in emergency.
        }

        // Check watchdog.
        if let Some(reason) = self.watchdog.check() {
            let _ = self.activate(reason.clone(), "watchdog timeout");
            return Some(reason);
        }

        // Check anomaly detector.
        if let Some(reason) = self.detector.detect_anomaly() {
            let _ = self.activate(reason.clone(), "anomaly detected");
            return Some(reason);
        }

        None
    }

    // -----------------------------------------------------------------------
    // Recovery
    // -----------------------------------------------------------------------

    /// Begin recovery from an emergency.
    ///
    /// Transitions from Activated to Recovering. Returns an error if
    /// not currently in emergency state.
    pub fn begin_recovery(&mut self) -> Result<(), SecurityError> {
        if self.state != EmergencyState::Activated {
            return Err(SecurityError::RecoveryFailed {
                reason: format!("cannot recover from state: {}", self.state),
            });
        }

        self.recovery_attempts += 1;

        if self.recovery_attempts > self.max_recovery_attempts {
            return Err(SecurityError::RecoveryFailed {
                reason: format!(
                    "max recovery attempts ({}) exceeded — user intervention required",
                    self.max_recovery_attempts
                ),
            });
        }

        self.state = EmergencyState::Recovering;
        self.detector.reset(); // Clear anomaly tracking for fresh start.

        tracing::info!(
            target: "SECURITY",
            attempt = self.recovery_attempts,
            rate_limit = self.recovery_rate_limit,
            "emergency recovery: beginning ramp-up"
        );

        Ok(())
    }

    /// Complete recovery — return to normal operation.
    ///
    /// Should only be called after the recovery period has passed
    /// without further anomalies.
    pub fn complete_recovery(&mut self) -> Result<(), SecurityError> {
        if self.state != EmergencyState::Recovering {
            return Err(SecurityError::RecoveryFailed {
                reason: format!("cannot complete recovery from state: {}", self.state),
            });
        }

        // Update the incident with duration.
        if let Some(activated_at) = self.activated_at {
            let duration_ms = activated_at.elapsed().as_millis() as u64;
            if let Some(incident) = self.incidents.last_mut() {
                incident.duration_ms = Some(duration_ms);
                incident.recovered = true;
            }
        }

        self.state = EmergencyState::Normal;
        self.activated_at = None;
        self.recovery_attempts = 0;

        tracing::info!(
            target: "SECURITY",
            "emergency recovery COMPLETE — resuming normal operation"
        );

        Ok(())
    }

    /// Cancel recovery and re-activate emergency (things went wrong again).
    pub fn abort_recovery(&mut self, reason: &str) -> Result<(), SecurityError> {
        if self.state != EmergencyState::Recovering {
            return Err(SecurityError::RecoveryFailed {
                reason: "not in recovery state".to_string(),
            });
        }

        self.state = EmergencyState::Activated;

        tracing::warn!(
            target: "SECURITY",
            reason = reason,
            "emergency recovery ABORTED — re-entering emergency"
        );

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    /// Get all incident reports.
    pub fn incidents(&self) -> &[IncidentReport] {
        &self.incidents
    }

    /// Number of incidents ever recorded.
    pub fn incident_count(&self) -> usize {
        self.incidents.len()
    }

    /// Get the last incident.
    pub fn last_incident(&self) -> Option<&IncidentReport> {
        self.incidents.last()
    }

    /// Current recovery rate limit (actions per minute).
    pub fn recovery_rate_limit(&self) -> u32 {
        self.recovery_rate_limit
    }

    /// How long the current emergency has been active.
    pub fn emergency_duration(&self) -> Option<Duration> {
        self.activated_at.map(|t| t.elapsed())
    }

    /// Produce a status summary for diagnostics/notification.
    pub fn status_summary(&self) -> EmergencyStatusSummary {
        EmergencyStatusSummary {
            state: self.state,
            incident_count: self.incidents.len() as u32,
            recovery_attempts: self.recovery_attempts,
            emergency_duration_ms: self.emergency_duration().map(|d| d.as_millis() as u64),
            watchdog_ok: !self.watchdog.is_timed_out(),
        }
    }

    /// Process user text input — check for stop phrases.
    pub fn process_user_input(&mut self, text: &str) -> Option<EmergencyReason> {
        if let Some(reason) = AnomalyDetector::check_user_stop_phrase(text) {
            let _ = self.activate(reason.clone(), &format!("user input: {text}"));
            return Some(reason);
        }
        None
    }
}

impl Default for EmergencyStop {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// EmergencyStatusSummary
// ---------------------------------------------------------------------------

/// Compact status for diagnostics and notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmergencyStatusSummary {
    pub state: EmergencyState,
    pub incident_count: u32,
    pub recovery_attempts: u32,
    pub emergency_duration_ms: Option<u64>,
    pub watchdog_ok: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- EmergencyState tests --

    #[test]
    fn test_emergency_state_display() {
        assert_eq!(EmergencyState::Normal.to_string(), "NORMAL");
        assert_eq!(EmergencyState::Activated.to_string(), "EMERGENCY_ACTIVATED");
        assert_eq!(EmergencyState::Recovering.to_string(), "RECOVERING");
    }

    #[test]
    fn test_initial_state_is_normal() {
        let es = EmergencyStop::new();
        assert_eq!(es.state(), EmergencyState::Normal);
        assert!(es.actions_allowed());
        assert!(!es.is_emergency());
    }

    // -- Activation tests --

    #[test]
    fn test_activate_transitions_to_activated() {
        let mut es = EmergencyStop::new();
        let reason = EmergencyReason::ManualTrigger {
            reason: "test".to_string(),
        };
        let report = es.activate(reason, "test context").unwrap();
        assert_eq!(es.state(), EmergencyState::Activated);
        assert!(es.is_emergency());
        assert!(!es.actions_allowed());
        assert!(!report.recovered);
    }

    #[test]
    fn test_double_activate_returns_error() {
        let mut es = EmergencyStop::new();
        let reason = EmergencyReason::ManualTrigger {
            reason: "test".to_string(),
        };
        es.activate(reason.clone(), "").unwrap();
        let result = es.activate(reason, "");
        assert!(result.is_err());
    }

    #[test]
    fn test_incident_recorded_on_activate() {
        let mut es = EmergencyStop::new();
        let reason = EmergencyReason::ManualTrigger {
            reason: "test".to_string(),
        };
        es.activate(reason, "ctx").unwrap();
        assert_eq!(es.incident_count(), 1);
        let inc = es.last_incident().unwrap();
        assert_eq!(inc.context, "ctx");
        assert!(!inc.recovered);
    }

    // -- Recovery tests --

    #[test]
    fn test_recovery_from_activated() {
        let mut es = EmergencyStop::new();
        es.activate(
            EmergencyReason::ManualTrigger {
                reason: "t".to_string(),
            },
            "",
        )
        .unwrap();

        es.begin_recovery().unwrap();
        assert_eq!(es.state(), EmergencyState::Recovering);
        assert!(es.is_recovering());
        assert!(es.actions_allowed()); // Recovering allows limited actions.
    }

    #[test]
    fn test_complete_recovery() {
        let mut es = EmergencyStop::new();
        es.activate(
            EmergencyReason::ManualTrigger {
                reason: "t".to_string(),
            },
            "",
        )
        .unwrap();
        es.begin_recovery().unwrap();
        es.complete_recovery().unwrap();
        assert_eq!(es.state(), EmergencyState::Normal);
        assert!(es.actions_allowed());
    }

    #[test]
    fn test_recovery_from_normal_fails() {
        let mut es = EmergencyStop::new();
        assert!(es.begin_recovery().is_err());
    }

    #[test]
    fn test_complete_recovery_from_activated_fails() {
        let mut es = EmergencyStop::new();
        es.activate(
            EmergencyReason::ManualTrigger {
                reason: "t".to_string(),
            },
            "",
        )
        .unwrap();
        // Cannot complete recovery without first beginning it.
        assert!(es.complete_recovery().is_err());
    }

    #[test]
    fn test_abort_recovery() {
        let mut es = EmergencyStop::new();
        es.activate(
            EmergencyReason::ManualTrigger {
                reason: "t".to_string(),
            },
            "",
        )
        .unwrap();
        es.begin_recovery().unwrap();
        es.abort_recovery("still failing").unwrap();
        assert_eq!(es.state(), EmergencyState::Activated);
    }

    #[test]
    fn test_max_recovery_attempts() {
        let mut es = EmergencyStop::new();

        // Cycle through max recovery attempts.
        for _ in 0..3 {
            es.activate(
                EmergencyReason::ManualTrigger {
                    reason: "t".to_string(),
                },
                "",
            )
            .unwrap_or_else(|_| IncidentReport {
                triggered_at_ms: 0,
                reason: EmergencyReason::ManualTrigger {
                    reason: "t".to_string(),
                },
                context: String::new(),
                duration_ms: None,
                recovered: false,
            });
            // If already activated, force state for the test.
            es.state = EmergencyState::Activated;
            es.begin_recovery().unwrap();
            // Abort to go back to activated without completing.
            es.abort_recovery("retry").unwrap();
        }

        // 4th attempt should exceed max_recovery_attempts (3).
        es.state = EmergencyState::Activated;
        let result = es.begin_recovery();
        assert!(result.is_err());
    }

    // -- AnomalyDetector tests --

    #[test]
    fn test_no_anomaly_initially() {
        let detector = AnomalyDetector::new();
        assert!(detector.detect_anomaly().is_none());
    }

    #[test]
    fn test_high_failure_rate_detection() {
        let mut detector = AnomalyDetector::new();
        // Record 10 actions, 6 failures.
        for i in 0..10 {
            detector.record_outcome(i < 4); // first 4 succeed, last 6 fail
        }
        let anomaly = detector.detect_anomaly();
        assert!(anomaly.is_some());
        assert!(matches!(
            anomaly.unwrap(),
            EmergencyReason::HighFailureRate { .. }
        ));
    }

    #[test]
    fn test_no_failure_rate_anomaly_below_threshold() {
        let mut detector = AnomalyDetector::new();
        // Record 10 actions, 3 failures (30% < 50%).
        for i in 0..10 {
            detector.record_outcome(i < 7);
        }
        // Only check failure rate — if no loops or frozen screen, should be None.
        assert!(detector.check_failure_rate().is_none());
    }

    #[test]
    fn test_action_loop_detection() {
        let mut detector = AnomalyDetector::new();
        for _ in 0..6 {
            detector.record_action("tap(100, 200)");
        }
        let anomaly = detector.check_action_loop();
        assert!(anomaly.is_some());
        assert!(matches!(
            anomaly.unwrap(),
            EmergencyReason::ActionLoop { .. }
        ));
    }

    #[test]
    fn test_no_loop_with_varied_actions() {
        let mut detector = AnomalyDetector::new();
        for i in 0..10 {
            detector.record_action(&format!("action_{i}"));
        }
        assert!(detector.check_action_loop().is_none());
    }

    #[test]
    fn test_screen_frozen_detection() {
        let mut detector = AnomalyDetector::new();
        for _ in 0..10 {
            detector.record_screen_change(false);
        }
        let anomaly = detector.check_screen_frozen();
        assert!(anomaly.is_some());
        assert!(matches!(
            anomaly.unwrap(),
            EmergencyReason::ScreenFrozen { .. }
        ));
    }

    #[test]
    fn test_no_frozen_with_changes() {
        let mut detector = AnomalyDetector::new();
        for i in 0..10 {
            detector.record_screen_change(i % 3 == 0); // Some changes.
        }
        assert!(detector.check_screen_frozen().is_none());
    }

    #[test]
    fn test_user_stop_phrase_detection() {
        assert!(AnomalyDetector::check_user_stop_phrase("please stop").is_some());
        assert!(AnomalyDetector::check_user_stop_phrase("HALT AURA").is_some());
        assert!(AnomalyDetector::check_user_stop_phrase("emergency!").is_some());
        assert!(AnomalyDetector::check_user_stop_phrase("abort now").is_some());
        assert!(AnomalyDetector::check_user_stop_phrase("continue working").is_none());
    }

    #[test]
    fn test_detector_reset() {
        let mut detector = AnomalyDetector::new();
        for _ in 0..10 {
            detector.record_outcome(false);
            detector.record_action("same");
        }
        detector.reset();
        assert!(detector.detect_anomaly().is_none());
    }

    // -- Watchdog tests --

    #[test]
    fn test_watchdog_not_timed_out_initially() {
        let watchdog = Watchdog::with_default_timeout();
        assert!(!watchdog.is_timed_out());
        assert!(watchdog.check().is_none());
    }

    #[test]
    fn test_watchdog_heartbeat_resets() {
        let mut watchdog = Watchdog::new(Duration::from_millis(100));
        watchdog.heartbeat();
        assert!(!watchdog.is_timed_out());
    }

    #[test]
    fn test_watchdog_time_since_heartbeat() {
        let watchdog = Watchdog::with_default_timeout();
        let elapsed = watchdog.time_since_heartbeat();
        assert!(elapsed < Duration::from_secs(1));
    }

    // -- EmergencyReason display tests --

    #[test]
    fn test_emergency_reason_display() {
        let r1 = EmergencyReason::HighFailureRate {
            failure_count: 7,
            total_count: 10,
        };
        assert!(r1.to_string().contains("7/10"));

        let r2 = EmergencyReason::ActionLoop {
            action: "tap".to_string(),
            count: 6,
            window_seconds: 30,
        };
        assert!(r2.to_string().contains("tap"));
        assert!(r2.to_string().contains("6x"));

        let r3 = EmergencyReason::UserRequested {
            trigger_phrase: "stop".to_string(),
        };
        assert!(r3.to_string().contains("stop"));
    }

    // -- Integration tests --

    #[test]
    fn test_check_and_trigger_with_failures() {
        let mut es = EmergencyStop::new();
        for _ in 0..10 {
            es.detector_mut().record_outcome(false);
        }
        let result = es.check_and_trigger();
        assert!(result.is_some());
        assert!(es.is_emergency());
    }

    #[test]
    fn test_process_user_input_triggers_emergency() {
        let mut es = EmergencyStop::new();
        let result = es.process_user_input("please stop everything");
        assert!(result.is_some());
        assert!(es.is_emergency());
    }

    #[test]
    fn test_process_user_input_no_trigger() {
        let mut es = EmergencyStop::new();
        let result = es.process_user_input("hello, how are you?");
        assert!(result.is_none());
        assert!(!es.is_emergency());
    }

    #[test]
    fn test_status_summary() {
        let es = EmergencyStop::new();
        let summary = es.status_summary();
        assert_eq!(summary.state, EmergencyState::Normal);
        assert_eq!(summary.incident_count, 0);
        assert!(summary.watchdog_ok);
    }

    #[test]
    fn test_full_lifecycle() {
        let mut es = EmergencyStop::new();

        // Normal → Activated.
        es.activate(
            EmergencyReason::ManualTrigger {
                reason: "test".to_string(),
            },
            "test",
        )
        .unwrap();
        assert!(es.is_emergency());

        // Activated → Recovering.
        es.begin_recovery().unwrap();
        assert!(es.is_recovering());
        assert!(es.actions_allowed());

        // Recovering → Normal.
        es.complete_recovery().unwrap();
        assert_eq!(es.state(), EmergencyState::Normal);
        assert!(es.actions_allowed());

        // Incident should be marked as recovered.
        let inc = es.last_incident().unwrap();
        assert!(inc.recovered);
        assert!(inc.duration_ms.is_some());
    }

    #[test]
    fn test_emergency_default() {
        let es = EmergencyStop::default();
        assert_eq!(es.state(), EmergencyState::Normal);
    }

    #[test]
    fn test_anomaly_detector_default() {
        let d = AnomalyDetector::default();
        assert!(d.detect_anomaly().is_none());
    }
}
