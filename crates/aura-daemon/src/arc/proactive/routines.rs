//! Routine detection and automation engine (SPEC-ARC section 8.3.3).
//!
//! Learns recurring user patterns (e.g., "opens news app at 8am on weekdays")
//! and offers to automate them. Detected routines require 3+ observations at
//! the same time (+-30 min) before being surfaced.

use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use super::super::ArcError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of detected routines tracked.
const MAX_DETECTED_ROUTINES: usize = 64;

/// Maximum number of active automations.
const MAX_ACTIVE_AUTOMATIONS: usize = 16;

/// Maximum number of action descriptors per automation.
const MAX_ACTIONS_PER_AUTOMATION: usize = 20;

/// Minimum observations before a pattern is considered a routine.
const MIN_OBSERVATIONS_FOR_ROUTINE: u32 = 3;

/// Time tolerance for pattern matching (minutes).
const TIME_TOLERANCE_MINUTES: u8 = 30;

/// Maximum observation entries retained (prevents unbounded growth).
const MAX_OBSERVATIONS: usize = 512;

// ---------------------------------------------------------------------------
// DetectedRoutine
// ---------------------------------------------------------------------------

/// A recurring behavioural pattern detected from user actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedRoutine {
    /// Hash of the action pattern (for deduplication).
    pub pattern_hash: u64,
    /// Human-readable description of the routine.
    pub description: String,
    /// Confidence in this being a real routine [0.0, 1.0].
    pub confidence: f32,
    /// Number of times this pattern has been observed.
    pub times_observed: u32,
    /// Average hour-of-day when this pattern occurs.
    pub avg_time_of_day: f32,
    /// Bitmask of days this routine is active (bit 0 = Monday, bit 6 = Sunday).
    pub days_active: u8,
}

impl DetectedRoutine {
    /// Check whether this routine is active on a given day of the week (0=Mon, 6=Sun).
    #[must_use]
    pub fn is_active_on_day(&self, day_of_week: u8) -> bool {
        if day_of_week > 6 {
            return false;
        }
        (self.days_active >> day_of_week) & 1 == 1
    }

    /// Set a day as active.
    pub fn set_day_active(&mut self, day_of_week: u8) {
        if day_of_week <= 6 {
            self.days_active |= 1 << day_of_week;
        }
    }
}

// ---------------------------------------------------------------------------
// Automation
// ---------------------------------------------------------------------------

/// An automation built from a detected routine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Automation {
    /// ID of the source routine.
    pub routine_id: u64,
    /// Whether this automation is enabled.
    pub enabled: bool,
    /// Action descriptors to execute (bounded).
    pub actions: Vec<String>,
    /// Timestamp (ms) of last execution.
    pub last_run_ms: u64,
    /// Hour of day when this automation should run.
    pub trigger_hour: u8,
    /// Day-of-week bitmask (mirrors the routine's days_active).
    pub trigger_days: u8,
}

impl Automation {
    /// Check whether this automation is due at the given hour and day.
    #[must_use]
    pub fn is_due(&self, hour: u8, day_of_week: u8) -> bool {
        if !self.enabled {
            return false;
        }
        if day_of_week > 6 {
            return false;
        }
        let day_match = (self.trigger_days >> day_of_week) & 1 == 1;
        let hour_match = hour == self.trigger_hour;
        day_match && hour_match
    }
}

// ---------------------------------------------------------------------------
// Observation (internal)
// ---------------------------------------------------------------------------

/// An internal record of an observed user action.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Observation {
    /// Hash of the action string.
    action_hash: u64,
    /// The raw action string.
    action: String,
    /// Hour of day (0..23).
    hour: u8,
    /// Day of week (0=Mon, 6=Sun).
    day_of_week: u8,
    /// Timestamp (ms) of observation.
    timestamp_ms: u64,
}

// ---------------------------------------------------------------------------
// RoutineManager
// ---------------------------------------------------------------------------

/// Detects recurring user patterns and manages automations.
#[derive(Debug, Serialize, Deserialize)]
pub struct RoutineManager {
    /// Detected routines (bounded).
    detected_routines: Vec<DetectedRoutine>,
    /// Active automations (bounded).
    active_automations: Vec<Automation>,
    /// Raw observations for pattern learning (bounded ring buffer).
    observations: Vec<Observation>,
}

impl RoutineManager {
    /// Create a new routine manager with empty state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            detected_routines: Vec::with_capacity(MAX_DETECTED_ROUTINES),
            active_automations: Vec::with_capacity(MAX_ACTIVE_AUTOMATIONS),
            observations: Vec::with_capacity(MAX_OBSERVATIONS),
        }
    }

    /// Number of detected routines.
    #[must_use]
    pub fn routine_count(&self) -> usize {
        self.detected_routines.len()
    }

    /// Number of active automations.
    #[must_use]
    pub fn automation_count(&self) -> usize {
        self.active_automations.len()
    }

    /// Number of raw observations stored.
    #[must_use]
    pub fn observation_count(&self) -> usize {
        self.observations.len()
    }

    /// Read-only access to detected routines.
    #[must_use]
    pub fn routines(&self) -> &[DetectedRoutine] {
        &self.detected_routines
    }

    /// Read-only access to active automations.
    #[must_use]
    pub fn automations(&self) -> &[Automation] {
        &self.active_automations
    }

    /// Compute a simple hash for an action string (FNV-1a).
    #[must_use]
    fn action_hash(action: &str) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in action.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    /// Record an observed user action for pattern learning.
    ///
    /// After recording, checks if the action forms a pattern (same action
    /// at approximately the same time on 3+ different days).
    #[instrument(name = "routine_observe", skip(self, action))]
    pub fn observe_action(
        &mut self,
        action: &str,
        hour: u8,
        day_of_week: u8,
        now_ms: u64,
    ) -> Result<(), ArcError> {
        if day_of_week > 6 {
            return Err(ArcError::DomainError {
                domain: super::super::DomainId::Productivity,
                detail: format!("invalid day_of_week: {day_of_week} (must be 0..6)"),
            });
        }
        if hour > 23 {
            return Err(ArcError::DomainError {
                domain: super::super::DomainId::Productivity,
                detail: format!("invalid hour: {hour} (must be 0..23)"),
            });
        }

        let hash = Self::action_hash(action);

        // Enforce bounded observations.
        if self.observations.len() >= MAX_OBSERVATIONS {
            self.observations.remove(0); // evict oldest
        }

        self.observations.push(Observation {
            action_hash: hash,
            action: action.to_string(),
            hour,
            day_of_week,
            timestamp_ms: now_ms,
        });

        debug!(
            action_hash = hash,
            hour,
            day = day_of_week,
            "action observed"
        );

        // Check if this forms a new pattern.
        self.detect_pattern(hash, action, hour, day_of_week)?;

        Ok(())
    }

    /// Detect whether observations for the given action hash form a routine.
    ///
    /// Pattern rule: same action at same time (+-30 min) on 3+ distinct days.
    fn detect_pattern(
        &mut self,
        action_hash: u64,
        action: &str,
        hour: u8,
        _day_of_week: u8,
    ) -> Result<(), ArcError> {
        // Count observations of this action within time tolerance.
        let mut matching_days: u8 = 0; // bitmask
        let mut hour_sum: f32 = 0.0;
        let mut match_count: u32 = 0;

        for obs in &self.observations {
            if obs.action_hash != action_hash {
                continue;
            }
            // Check time tolerance.
            let hour_diff = (obs.hour as i16 - hour as i16).unsigned_abs();
            // Handle wrap-around (e.g., 23 and 0 are 1 hour apart).
            let circular_diff = hour_diff.min(24 - hour_diff);
            if circular_diff <= TIME_TOLERANCE_MINUTES as u16 / 60 + 1 {
                matching_days |= 1 << obs.day_of_week;
                hour_sum += obs.hour as f32;
                match_count += 1;
            }
        }

        let distinct_days = matching_days.count_ones();
        if distinct_days < MIN_OBSERVATIONS_FOR_ROUTINE {
            return Ok(());
        }

        let avg_hour = if match_count > 0 {
            hour_sum / match_count as f32
        } else {
            hour as f32
        };

        // Check if we already have this routine.
        if let Some(existing) = self
            .detected_routines
            .iter_mut()
            .find(|r| r.pattern_hash == action_hash)
        {
            existing.times_observed = match_count;
            existing.avg_time_of_day = avg_hour;
            existing.days_active = matching_days;
            existing.confidence =
                (distinct_days as f32 / 7.0).min(1.0) * (match_count as f32 / 10.0).min(1.0);
            debug!(hash = action_hash, times = match_count, "routine updated");
            return Ok(());
        }

        // Create new routine.
        if self.detected_routines.len() >= MAX_DETECTED_ROUTINES {
            // Evict lowest-confidence routine.
            if let Some((idx, _)) =
                self.detected_routines
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        a.confidence
                            .partial_cmp(&b.confidence)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
            {
                self.detected_routines.swap_remove(idx);
            }
        }

        let confidence =
            (distinct_days as f32 / 7.0).min(1.0) * (match_count as f32 / 10.0).min(1.0);

        info!(
            hash = action_hash,
            days = distinct_days,
            confidence,
            "new routine detected"
        );

        self.detected_routines.push(DetectedRoutine {
            pattern_hash: action_hash,
            description: format!("Routine: {action}"),
            confidence,
            times_observed: match_count,
            avg_time_of_day: avg_hour,
            days_active: matching_days,
        });

        Ok(())
    }

    /// Create an automation from a detected routine.
    pub fn create_automation(
        &mut self,
        routine_hash: u64,
        actions: Vec<String>,
    ) -> Result<(), ArcError> {
        if self.active_automations.len() >= MAX_ACTIVE_AUTOMATIONS {
            return Err(ArcError::CapacityExceeded {
                collection: "active_automations".into(),
                max: MAX_ACTIVE_AUTOMATIONS,
            });
        }
        if actions.len() > MAX_ACTIONS_PER_AUTOMATION {
            return Err(ArcError::CapacityExceeded {
                collection: "automation_actions".into(),
                max: MAX_ACTIONS_PER_AUTOMATION,
            });
        }

        let routine = self
            .detected_routines
            .iter()
            .find(|r| r.pattern_hash == routine_hash)
            .ok_or_else(|| ArcError::NotFound {
                entity: "routine".into(),
                id: routine_hash,
            })?;

        let trigger_hour = routine.avg_time_of_day.round() as u8;
        let trigger_days = routine.days_active;

        self.active_automations.push(Automation {
            routine_id: routine_hash,
            enabled: true,
            actions,
            last_run_ms: 0,
            trigger_hour: trigger_hour.min(23),
            trigger_days,
        });

        info!(routine_id = routine_hash, "automation created");
        Ok(())
    }

    /// Enable or disable an automation by routine ID.
    pub fn set_automation_enabled(&mut self, routine_id: u64, enabled: bool) {
        for a in &mut self.active_automations {
            if a.routine_id == routine_id {
                a.enabled = enabled;
            }
        }
    }

    /// Check which automations are due at the given hour and day-of-week.
    ///
    /// Returns references to due automations, sorted by routine_id for
    /// deterministic ordering.
    #[must_use]
    pub fn check_automations(&self, hour: u8, day_of_week: u8) -> Vec<&Automation> {
        let mut due: Vec<&Automation> = self
            .active_automations
            .iter()
            .filter(|a| a.is_due(hour, day_of_week))
            .collect();
        due.sort_by_key(|a| a.routine_id);
        due
    }

    /// Mark an automation as having run at the given timestamp.
    pub fn mark_automation_run(&mut self, routine_id: u64, now_ms: u64) {
        for a in &mut self.active_automations {
            if a.routine_id == routine_id {
                a.last_run_ms = now_ms;
            }
        }
    }
}

impl Default for RoutineManager {
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

    #[test]
    fn test_new_routine_manager() {
        let rm = RoutineManager::new();
        assert_eq!(rm.routine_count(), 0);
        assert_eq!(rm.automation_count(), 0);
        assert_eq!(rm.observation_count(), 0);
    }

    #[test]
    fn test_observe_action_records() {
        let mut rm = RoutineManager::new();
        rm.observe_action("open_news", 8, 0, 1000)
            .expect("observe ok");
        assert_eq!(rm.observation_count(), 1);
    }

    #[test]
    fn test_observe_action_invalid_day() {
        let mut rm = RoutineManager::new();
        let result = rm.observe_action("open_news", 8, 7, 1000);
        assert!(result.is_err());
    }

    #[test]
    fn test_observe_action_invalid_hour() {
        let mut rm = RoutineManager::new();
        let result = rm.observe_action("open_news", 25, 0, 1000);
        assert!(result.is_err());
    }

    #[test]
    fn test_pattern_detection_after_3_days() {
        let mut rm = RoutineManager::new();

        // Observe the same action at ~8am on 3 different days.
        rm.observe_action("open_news", 8, 0, 1_000_000)
            .expect("day 0");
        assert_eq!(rm.routine_count(), 0, "2 days not enough");

        rm.observe_action("open_news", 8, 1, 2_000_000)
            .expect("day 1");
        assert_eq!(rm.routine_count(), 0, "2 days still not enough");

        rm.observe_action("open_news", 8, 2, 3_000_000)
            .expect("day 2");
        assert_eq!(rm.routine_count(), 1, "3 days should trigger detection");

        let routine = &rm.routines()[0];
        assert!(routine.times_observed >= 3);
        assert!(routine.confidence > 0.0);
    }

    #[test]
    fn test_detected_routine_days_bitmask() {
        let mut r = DetectedRoutine {
            pattern_hash: 42,
            description: "test".into(),
            confidence: 0.5,
            times_observed: 3,
            avg_time_of_day: 8.0,
            days_active: 0,
        };

        r.set_day_active(0); // Monday
        r.set_day_active(2); // Wednesday
        r.set_day_active(4); // Friday

        assert!(r.is_active_on_day(0));
        assert!(!r.is_active_on_day(1));
        assert!(r.is_active_on_day(2));
        assert!(!r.is_active_on_day(3));
        assert!(r.is_active_on_day(4));
        assert!(!r.is_active_on_day(7)); // out of range
    }

    #[test]
    fn test_create_automation() {
        let mut rm = RoutineManager::new();

        // First create a routine.
        for day in 0..3 {
            rm.observe_action("check_email", 9, day, (day as u64 + 1) * 1_000_000)
                .expect("observe");
        }
        assert_eq!(rm.routine_count(), 1);

        let hash = rm.routines()[0].pattern_hash;
        rm.create_automation(hash, vec!["open_email_app".into()])
            .expect("create ok");
        assert_eq!(rm.automation_count(), 1);

        let auto = &rm.automations()[0];
        assert!(auto.enabled);
        assert_eq!(auto.actions.len(), 1);
    }

    #[test]
    fn test_create_automation_not_found() {
        let mut rm = RoutineManager::new();
        let result = rm.create_automation(9999, vec!["action".into()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_automation_bounded() {
        let mut rm = RoutineManager::new();

        // Create enough routines first.
        for i in 0..MAX_ACTIVE_AUTOMATIONS {
            let action = format!("action_{i}");
            for day in 0..3u8 {
                rm.observe_action(&action, 8, day, (i as u64 * 10 + day as u64) * 1_000_000)
                    .expect("observe");
            }
        }

        // Create automations up to limit.
        let hashes: Vec<u64> = rm.routines().iter().map(|r| r.pattern_hash).collect();
        for (i, &hash) in hashes.iter().enumerate() {
            if i >= MAX_ACTIVE_AUTOMATIONS {
                break;
            }
            let result = rm.create_automation(hash, vec!["run".into()]);
            assert!(result.is_ok(), "failed at automation {i}");
        }

        assert_eq!(rm.automation_count(), MAX_ACTIVE_AUTOMATIONS);

        // Any additional should fail (if there's a routine available).
        if hashes.len() > MAX_ACTIVE_AUTOMATIONS {
            let result = rm.create_automation(hashes[MAX_ACTIVE_AUTOMATIONS], vec!["run".into()]);
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_check_automations_due() {
        let mut rm = RoutineManager::new();

        // Detect a pattern.
        for day in 0..3 {
            rm.observe_action("morning_music", 7, day, (day as u64 + 1) * 1_000_000)
                .expect("observe");
        }

        let hash = rm.routines()[0].pattern_hash;
        rm.create_automation(hash, vec!["play_music".into()])
            .expect("create ok");

        // Check at the right hour and day.
        let due = rm.check_automations(7, 0);
        assert_eq!(due.len(), 1);

        // Wrong hour.
        let due_wrong = rm.check_automations(12, 0);
        assert!(due_wrong.is_empty());
    }

    #[test]
    fn test_automation_is_due() {
        let a = Automation {
            routine_id: 1,
            enabled: true,
            actions: vec!["test".into()],
            last_run_ms: 0,
            trigger_hour: 8,
            trigger_days: 0b0010101, // Mon, Wed, Fri
        };

        assert!(a.is_due(8, 0)); // Monday at 8
        assert!(!a.is_due(9, 0)); // Monday at 9
        assert!(a.is_due(8, 2)); // Wednesday at 8
        assert!(!a.is_due(8, 1)); // Tuesday at 8
    }

    #[test]
    fn test_observations_bounded() {
        let mut rm = RoutineManager::new();
        for i in 0..(MAX_OBSERVATIONS + 50) {
            let _ = rm.observe_action(
                &format!("action_{}", i % 100),
                (i % 24) as u8,
                (i % 7) as u8,
                i as u64 * 1000,
            );
        }
        assert!(
            rm.observation_count() <= MAX_OBSERVATIONS,
            "got {}",
            rm.observation_count()
        );
    }

    #[test]
    fn test_set_automation_enabled() {
        let mut rm = RoutineManager::new();

        for day in 0..3 {
            rm.observe_action("task", 10, day, (day as u64 + 1) * 1_000_000)
                .expect("observe");
        }
        let hash = rm.routines()[0].pattern_hash;
        rm.create_automation(hash, vec!["run".into()])
            .expect("create");

        rm.set_automation_enabled(hash, false);
        let due = rm.check_automations(10, 0);
        assert!(due.is_empty(), "disabled automation should not be due");

        rm.set_automation_enabled(hash, true);
        let due2 = rm.check_automations(10, 0);
        assert_eq!(due2.len(), 1);
    }
}
