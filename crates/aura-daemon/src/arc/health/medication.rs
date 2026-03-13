//! Medication schedule and adherence tracking (spec §3.1).
//!
//! Tracks multiple medication schedules with dose windows, escalation
//! levels for missed doses, and computes an adherence score used by the
//! health domain.
//!
//! # Escalation levels
//!
//! | Level | Trigger                         | Action           |
//! |-------|---------------------------------|------------------|
//! | 0     | Dose window opens               | Silent           |
//! | 1     | 50% of window elapsed           | Gentle reminder  |
//! | 2     | 75% of window elapsed           | Urgent reminder  |
//! | 3     | Window closed, dose missed      | Log + alert      |

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::arc::ArcError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of medications tracked simultaneously.
const MAX_MEDICATIONS: usize = 32;

/// Maximum dose history entries per medication (ring buffer).
const MAX_DOSE_HISTORY: usize = 365;

/// Default dose window in seconds (30 minutes).
const DEFAULT_DOSE_WINDOW_SECS: i64 = 1800;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Escalation level for a pending dose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum EscalationLevel {
    /// Window open, no action needed yet.
    Silent = 0,
    /// 50% of window elapsed — gentle reminder.
    Gentle = 1,
    /// 75% of window elapsed — urgent reminder.
    Urgent = 2,
    /// Window closed — dose missed.
    Missed = 3,
}

/// How often a medication should be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Frequency {
    /// Once daily at a specific hour (0-23).
    Daily { hour: u8 },
    /// Multiple times per day at specified hours.
    MultiDaily { hours: [u8; 4], count: u8 },
    /// Every N hours.
    EveryNHours { n: u8 },
    /// As needed (PRN) — not scheduled.
    AsNeeded,
}

/// A single dose record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoseRecord {
    /// Medication ID this dose belongs to.
    pub med_id: u64,
    /// Unix epoch seconds when the dose was taken (or missed).
    pub timestamp: i64,
    /// Whether the dose was actually taken.
    pub taken: bool,
    /// Seconds late relative to the scheduled time (0 if on time, negative if early).
    pub lateness_secs: i64,
}

/// A medication schedule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MedicationSchedule {
    /// Unique identifier for this medication.
    pub id: u64,
    /// Medication name (user-facing).
    pub name: String,
    /// Dosing frequency.
    pub frequency: Frequency,
    /// Window size in seconds within which a dose is considered on-time.
    pub window_secs: i64,
    /// Whether this medication is currently active.
    pub active: bool,
    /// Importance weight for adherence scoring (0.0 to 1.0).
    pub importance: f32,
}

impl MedicationSchedule {
    /// Create a new daily medication schedule.
    #[must_use]
    pub fn new_daily(id: u64, name: String, hour: u8, importance: f32) -> Self {
        Self {
            id,
            name,
            frequency: Frequency::Daily { hour: hour.min(23) },
            window_secs: DEFAULT_DOSE_WINDOW_SECS,
            active: true,
            importance: importance.clamp(0.0, 1.0),
        }
    }
}

// ---------------------------------------------------------------------------
// DoseWindow — pending dose evaluation
// ---------------------------------------------------------------------------

/// Result of evaluating whether a dose is pending.
#[derive(Debug, Clone)]
pub struct DoseWindow {
    pub med_id: u64,
    pub scheduled_at: i64,
    pub window_end: i64,
    pub escalation: EscalationLevel,
}

impl DoseWindow {
    /// Compute the escalation level for a given `now` timestamp.
    #[must_use]
    pub fn compute_escalation(scheduled_at: i64, window_secs: i64, now: i64) -> EscalationLevel {
        if now < scheduled_at {
            return EscalationLevel::Silent;
        }
        let elapsed = now - scheduled_at;
        let window_end = scheduled_at + window_secs;
        if now >= window_end {
            EscalationLevel::Missed
        } else if elapsed >= (window_secs * 3) / 4 {
            EscalationLevel::Urgent
        } else if elapsed >= window_secs / 2 {
            EscalationLevel::Gentle
        } else {
            EscalationLevel::Silent
        }
    }
}

// ---------------------------------------------------------------------------
// MedicationManager
// ---------------------------------------------------------------------------

/// Manages medication schedules and dose history.
///
/// All collections are bounded: max [`MAX_MEDICATIONS`] schedules and
/// [`MAX_DOSE_HISTORY`] history entries per medication (ring buffer).
#[derive(Debug, Serialize, Deserialize)]
pub struct MedicationManager {
    schedules: Vec<MedicationSchedule>,
    /// Ring buffer of dose records, newest at the end.
    history: Vec<DoseRecord>,
    /// Write cursor for ring buffer.
    history_cursor: usize,
}

impl MedicationManager {
    /// Create a new empty medication manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schedules: Vec::with_capacity(8),
            history: Vec::with_capacity(64),
            history_cursor: 0,
        }
    }

    /// Add a medication schedule.
    pub fn add_schedule(&mut self, schedule: MedicationSchedule) -> Result<(), ArcError> {
        if self.schedules.len() >= MAX_MEDICATIONS {
            return Err(ArcError::CapacityExceeded {
                collection: "medication_schedules".into(),
                max: MAX_MEDICATIONS,
            });
        }
        debug!(id = schedule.id, name = %schedule.name, "medication scheduled");
        self.schedules.push(schedule);
        Ok(())
    }

    /// Remove a medication schedule by ID.
    pub fn remove_schedule(&mut self, med_id: u64) -> Result<(), ArcError> {
        let initial_len = self.schedules.len();
        self.schedules.retain(|s| s.id != med_id);
        if self.schedules.len() == initial_len {
            return Err(ArcError::NotFound {
                entity: "medication".into(),
                id: med_id,
            });
        }
        Ok(())
    }

    /// Record a dose event.
    pub fn record_dose(
        &mut self,
        med_id: u64,
        timestamp: i64,
        taken: bool,
    ) -> Result<(), ArcError> {
        let record = DoseRecord {
            med_id,
            timestamp,
            taken,
            lateness_secs: 0, // Caller or schedule check fills this
        };

        if self.history.len() < MAX_DOSE_HISTORY {
            self.history.push(record);
        } else {
            // Ring buffer: overwrite oldest
            let idx = self.history_cursor % MAX_DOSE_HISTORY;
            self.history[idx] = record;
        }
        self.history_cursor += 1;

        debug!(med_id, taken, "dose recorded");
        Ok(())
    }

    /// Record a dose with lateness information.
    pub fn record_dose_with_lateness(
        &mut self,
        med_id: u64,
        timestamp: i64,
        taken: bool,
        lateness_secs: i64,
    ) -> Result<(), ArcError> {
        let record = DoseRecord {
            med_id,
            timestamp,
            taken,
            lateness_secs,
        };

        if self.history.len() < MAX_DOSE_HISTORY {
            self.history.push(record);
        } else {
            let idx = self.history_cursor % MAX_DOSE_HISTORY;
            self.history[idx] = record;
        }
        self.history_cursor += 1;

        debug!(med_id, taken, lateness_secs, "dose recorded with lateness");
        Ok(())
    }

    /// Compute adherence score across all active medications.
    ///
    /// Score = weighted average of per-medication adherence rates.
    /// Each medication's adherence = (taken doses) / (total scheduled doses).
    /// Weight = medication importance.
    #[must_use]
    pub fn adherence_score(&self) -> f32 {
        let active: Vec<&MedicationSchedule> = self.schedules.iter().filter(|s| s.active).collect();
        if active.is_empty() {
            // No medications → perfect adherence (nothing to miss)
            return 1.0;
        }

        let mut weighted_sum = 0.0_f32;
        let mut weight_sum = 0.0_f32;

        for med in &active {
            let (taken, total) = self.per_med_adherence(med.id);
            let rate = if total > 0 {
                taken as f32 / total as f32
            } else {
                1.0 // No doses recorded yet → assume adherent
            };
            weighted_sum += rate * med.importance;
            weight_sum += med.importance;
        }

        if weight_sum > 0.0 {
            (weighted_sum / weight_sum).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// Count (taken, total) doses for a specific medication.
    #[must_use]
    fn per_med_adherence(&self, med_id: u64) -> (usize, usize) {
        let mut taken = 0usize;
        let mut total = 0usize;
        for record in &self.history {
            if record.med_id == med_id {
                total += 1;
                if record.taken {
                    taken += 1;
                }
            }
        }
        (taken, total)
    }

    /// Check all medications for pending doses at the given time.
    ///
    /// Returns dose windows with escalation levels.
    #[must_use]
    pub fn check_pending_doses(&self, now: i64) -> Vec<DoseWindow> {
        let mut pending = Vec::new();
        for med in self.schedules.iter().filter(|s| s.active) {
            if let Some(scheduled_at) = self.next_scheduled_time(med, now) {
                let escalation = DoseWindow::compute_escalation(scheduled_at, med.window_secs, now);
                if escalation != EscalationLevel::Silent || now >= scheduled_at {
                    pending.push(DoseWindow {
                        med_id: med.id,
                        scheduled_at,
                        window_end: scheduled_at + med.window_secs,
                        escalation,
                    });
                }
            }
        }
        pending
    }

    /// Determine the most recent/next scheduled time for a medication.
    ///
    /// Simplified: for daily meds, compute today's dose time relative to `now`.
    fn next_scheduled_time(&self, med: &MedicationSchedule, now: i64) -> Option<i64> {
        match med.frequency {
            Frequency::Daily { hour } => {
                // Compute the scheduled time for today
                let day_start = (now / 86400) * 86400; // Midnight UTC
                let scheduled = day_start + (hour as i64) * 3600;

                // If we're past the window, next dose is tomorrow
                if now > scheduled + med.window_secs {
                    Some(scheduled + 86400)
                } else {
                    Some(scheduled)
                }
            }
            Frequency::MultiDaily { hours, count } => {
                let day_start = (now / 86400) * 86400;
                let valid_count = (count as usize).min(hours.len());

                // Find the next upcoming dose time
                for &h in &hours[..valid_count] {
                    let scheduled = day_start + (h as i64) * 3600;
                    if now <= scheduled + med.window_secs {
                        return Some(scheduled);
                    }
                }
                // All today's doses past → first dose tomorrow
                if valid_count > 0 {
                    Some(day_start + 86400 + (hours[0] as i64) * 3600)
                } else {
                    None
                }
            }
            Frequency::EveryNHours { n } => {
                if n == 0 {
                    return None;
                }
                // Find the last dose for this med
                let last_dose_time = self
                    .history
                    .iter()
                    .filter(|r| r.med_id == med.id && r.taken)
                    .map(|r| r.timestamp)
                    .max()
                    .unwrap_or(0);
                Some(last_dose_time + (n as i64) * 3600)
            }
            Frequency::AsNeeded => None,
        }
    }

    /// Total number of dose records tracked.
    #[must_use]
    pub fn total_doses_tracked(&self) -> usize {
        self.history.len()
    }

    /// Number of active medication schedules.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.schedules.iter().filter(|s| s.active).count()
    }

    /// Read-only access to schedules.
    #[must_use]
    pub fn schedules(&self) -> &[MedicationSchedule] {
        &self.schedules
    }

    // -----------------------------------------------------------------------
    // Nudge engine
    // -----------------------------------------------------------------------

    /// Generate medication nudges based on adherence patterns.
    ///
    /// Analyzes per-medication adherence and timing patterns to produce
    /// encouraging or optimizing nudges.  Returns up to 3 nudges, sorted
    /// by type (alerts first, then timing, then positive).
    #[must_use]
    pub fn generate_nudges(&self) -> Vec<MedicationNudge> {
        let mut nudges = Vec::with_capacity(4);

        for med in self.schedules.iter().filter(|s| s.active) {
            let (taken, total) = self.per_med_adherence(med.id);
            if total < 5 {
                continue; // Not enough data
            }

            let rate = taken as f32 / total as f32;

            // Adherence alert
            if rate < 0.7 {
                nudges.push(MedicationNudge {
                    med_id: med.id,
                    med_name: med.name.clone(),
                    nudge_type: NudgeType::AdherenceAlert,
                    signal: "adherence_alert",
                    confidence: (1.0 - rate).clamp(0.0, 1.0),
                });
            }

            // Positive reinforcement
            if rate >= 0.9 && total >= 10 {
                nudges.push(MedicationNudge {
                    med_id: med.id,
                    med_name: med.name.clone(),
                    nudge_type: NudgeType::PositiveReinforcement,
                    signal: "positive_reinforcement",
                    confidence: 0.8,
                });
            }

            // Timing optimization: average lateness of taken doses
            let taken_doses: Vec<&DoseRecord> = self
                .history
                .iter()
                .filter(|r| r.med_id == med.id && r.taken && r.lateness_secs > 0)
                .collect();

            if !taken_doses.is_empty() {
                let avg_lateness = taken_doses.iter().map(|r| r.lateness_secs).sum::<i64>() as f64
                    / taken_doses.len() as f64;
                if avg_lateness > 600.0 {
                    nudges.push(MedicationNudge {
                        med_id: med.id,
                        med_name: med.name.clone(),
                        nudge_type: NudgeType::TimingOptimization,
                        signal: "timing_optimization",
                        confidence: (avg_lateness as f32 / 1800.0).min(1.0),
                    });
                }
            }
        }

        // Sort: AdherenceAlert first, then TimingOptimization, then Positive
        nudges.sort_by_key(|n| n.nudge_type.sort_key());
        nudges.truncate(3);
        nudges
    }
}

// ---------------------------------------------------------------------------
// Nudge types
// ---------------------------------------------------------------------------

/// Type of medication nudge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeType {
    /// Adherence is dropping — encouragement needed.
    AdherenceAlert,
    /// Consistently late — suggest time shift.
    TimingOptimization,
    /// Streak of good adherence — positive reinforcement.
    PositiveReinforcement,
}

impl NudgeType {
    /// Sorting key: lower = higher priority.
    #[must_use]
    fn sort_key(self) -> u8 {
        match self {
            Self::AdherenceAlert => 0,
            Self::TimingOptimization => 1,
            Self::PositiveReinforcement => 2,
        }
    }
}

/// A medication timing optimization nudge.
#[derive(Debug, Clone)]
pub struct MedicationNudge {
    /// Medication ID.
    pub med_id: u64,
    /// Medication name.
    pub med_name: String,
    /// Type of nudge.
    pub nudge_type: NudgeType,
    /// Signal key passed to the LLM — not user-facing prose.
    /// Values: "adherence_alert", "positive_reinforcement", "timing_optimization".
    pub signal: &'static str,
    /// Confidence that this nudge is relevant (0.0–1.0).
    pub confidence: f32,
}

impl Default for MedicationManager {
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
    fn test_escalation_levels() {
        // Window of 1800s (30 min)
        let scheduled = 10000;
        let window = 1800;

        assert_eq!(
            DoseWindow::compute_escalation(scheduled, window, 9000),
            EscalationLevel::Silent
        );
        assert_eq!(
            DoseWindow::compute_escalation(scheduled, window, 10000),
            EscalationLevel::Silent
        );
        assert_eq!(
            DoseWindow::compute_escalation(scheduled, window, 10900),
            EscalationLevel::Gentle
        );
        assert_eq!(
            DoseWindow::compute_escalation(scheduled, window, 11350),
            EscalationLevel::Urgent
        );
        assert_eq!(
            DoseWindow::compute_escalation(scheduled, window, 11800),
            EscalationLevel::Missed
        );
    }

    #[test]
    fn test_adherence_score_no_meds() {
        let mgr = MedicationManager::new();
        assert!((mgr.adherence_score() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_adherence_score_perfect() {
        let mut mgr = MedicationManager::new();
        let med = MedicationSchedule::new_daily(1, "Aspirin".into(), 8, 1.0);
        mgr.add_schedule(med).expect("add");

        // Record 10 taken doses
        for i in 0..10 {
            mgr.record_dose(1, 1000 + i * 100, true).expect("record");
        }

        let score = mgr.adherence_score();
        assert!((score - 1.0).abs() < 0.001, "got {score}");
    }

    #[test]
    fn test_adherence_score_half() {
        let mut mgr = MedicationManager::new();
        let med = MedicationSchedule::new_daily(1, "Aspirin".into(), 8, 1.0);
        mgr.add_schedule(med).expect("add");

        // 5 taken, 5 missed
        for i in 0..5 {
            mgr.record_dose(1, 1000 + i * 100, true).expect("record");
        }
        for i in 5..10 {
            mgr.record_dose(1, 1000 + i * 100, false).expect("record");
        }

        let score = mgr.adherence_score();
        assert!((score - 0.5).abs() < 0.001, "got {score}");
    }

    #[test]
    fn test_capacity_limit() {
        let mut mgr = MedicationManager::new();
        for i in 0..MAX_MEDICATIONS {
            let med = MedicationSchedule::new_daily(i as u64, format!("Med{i}"), 8, 0.5);
            assert!(mgr.add_schedule(med).is_ok());
        }
        let overflow = MedicationSchedule::new_daily(999, "Overflow".into(), 8, 0.5);
        assert!(mgr.add_schedule(overflow).is_err());
    }

    #[test]
    fn test_ring_buffer_overflow() {
        let mut mgr = MedicationManager::new();
        let med = MedicationSchedule::new_daily(1, "Test".into(), 8, 1.0);
        mgr.add_schedule(med).expect("add");

        // Exceed MAX_DOSE_HISTORY
        for i in 0..(MAX_DOSE_HISTORY + 50) {
            mgr.record_dose(1, i as i64, true).expect("record");
        }

        // History should be capped
        assert_eq!(mgr.history.len(), MAX_DOSE_HISTORY);
    }

    #[test]
    fn test_remove_schedule() {
        let mut mgr = MedicationManager::new();
        let med = MedicationSchedule::new_daily(42, "Remove".into(), 8, 0.5);
        mgr.add_schedule(med).expect("add");
        assert_eq!(mgr.schedules.len(), 1);

        mgr.remove_schedule(42).expect("remove");
        assert_eq!(mgr.schedules.len(), 0);

        // Removing again should fail
        assert!(mgr.remove_schedule(42).is_err());
    }

    #[test]
    fn test_weighted_adherence() {
        let mut mgr = MedicationManager::new();

        // High-importance med: 100% adherence
        let med_a = MedicationSchedule::new_daily(1, "Important".into(), 8, 1.0);
        mgr.add_schedule(med_a).expect("add");
        for i in 0..10 {
            mgr.record_dose(1, i * 100, true).expect("record");
        }

        // Low-importance med: 0% adherence
        let med_b = MedicationSchedule::new_daily(2, "Minor".into(), 12, 0.1);
        mgr.add_schedule(med_b).expect("add");
        for i in 0..10 {
            mgr.record_dose(2, i * 100, false).expect("record");
        }

        let score = mgr.adherence_score();
        // Weighted: (1.0 * 1.0 + 0.0 * 0.1) / (1.0 + 0.1) = 1.0/1.1 ≈ 0.909
        assert!(score > 0.85 && score < 0.95, "got {score}");
    }

    // -----------------------------------------------------------------------
    // Nudge tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_nudges_no_meds() {
        let mgr = MedicationManager::new();
        let nudges = mgr.generate_nudges();
        assert!(nudges.is_empty());
    }

    #[test]
    fn test_nudges_poor_adherence() {
        let mut mgr = MedicationManager::new();
        let med = MedicationSchedule::new_daily(1, "Aspirin".into(), 8, 1.0);
        mgr.add_schedule(med).expect("add");

        // 2 taken, 8 missed → 20% adherence
        for i in 0..2 {
            mgr.record_dose(1, 1000 + i * 100, true).expect("ok");
        }
        for i in 2..10 {
            mgr.record_dose(1, 1000 + i * 100, false).expect("ok");
        }

        let nudges = mgr.generate_nudges();
        assert!(!nudges.is_empty(), "should generate adherence alert");
        assert_eq!(nudges[0].nudge_type, NudgeType::AdherenceAlert);
        assert_eq!(nudges[0].med_name, "Aspirin");
    }

    #[test]
    fn test_nudges_good_adherence() {
        let mut mgr = MedicationManager::new();
        let med = MedicationSchedule::new_daily(1, "VitaminD".into(), 8, 1.0);
        mgr.add_schedule(med).expect("add");

        // 10 taken, 0 missed → 100% adherence
        for i in 0..10 {
            mgr.record_dose(1, 1000 + i * 100, true).expect("ok");
        }

        let nudges = mgr.generate_nudges();
        let positive = nudges
            .iter()
            .any(|n| n.nudge_type == NudgeType::PositiveReinforcement);
        assert!(positive, "should generate positive reinforcement");
        // Verify the signal key is correct (not user-facing prose).
        let pos_nudge = nudges.iter().find(|n| n.nudge_type == NudgeType::PositiveReinforcement).unwrap();
        assert_eq!(pos_nudge.signal, "positive_reinforcement");
    }

    #[test]
    fn test_nudges_late_timing() {
        let mut mgr = MedicationManager::new();
        let med = MedicationSchedule::new_daily(1, "Metformin".into(), 8, 1.0);
        mgr.add_schedule(med).expect("add");

        // 10 taken doses, all 15 min (900s) late
        for i in 0..10 {
            mgr.record_dose_with_lateness(1, 1000 + i * 86400, true, 900)
                .expect("ok");
        }

        let nudges = mgr.generate_nudges();
        let timing = nudges
            .iter()
            .any(|n| n.nudge_type == NudgeType::TimingOptimization);
        assert!(timing, "should detect late timing pattern");
    }

    #[test]
    fn test_nudges_max_three() {
        let mut mgr = MedicationManager::new();

        // Create 5 medications, all with poor adherence → many potential nudges
        for i in 0..5 {
            let med = MedicationSchedule::new_daily(i as u64, format!("Med{i}"), 8, 0.8);
            mgr.add_schedule(med).expect("add");
            // 1 taken, 9 missed per med
            mgr.record_dose(i as u64, 1000, true).expect("ok");
            for j in 1..10 {
                mgr.record_dose(i as u64, 1000 + j * 100, false)
                    .expect("ok");
            }
        }

        let nudges = mgr.generate_nudges();
        assert!(
            nudges.len() <= 3,
            "should never return more than 3, got {}",
            nudges.len()
        );
    }
}
