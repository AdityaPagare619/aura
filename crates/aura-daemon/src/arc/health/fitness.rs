//! Fitness tracking — steps, workouts, activity scoring (spec §3.3).
//!
//! Tracks daily step counts, detects workout sessions, estimates calories,
//! and produces an activity score for the health domain composite.

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::arc::ArcError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum daily records retained (ring buffer).
const MAX_DAILY_RECORDS: usize = 90;

/// Maximum workout sessions retained.
const MAX_WORKOUT_SESSIONS: usize = 365;

/// Default daily step goal.
const DEFAULT_STEP_GOAL: u32 = 10_000;

/// MET values for common activity types (approximate).
const MET_WALKING: f32 = 3.5;
const MET_RUNNING: f32 = 8.0;
const MET_CYCLING: f32 = 6.0;
const MET_STRENGTH: f32 = 5.0;
const MET_DEFAULT: f32 = 4.0;

/// Calories per step (rough average for 70 kg person).
const CALORIES_PER_STEP: f32 = 0.04;

/// Continuous sedentary threshold in seconds (2 hours).
const SEDENTARY_THRESHOLD_SECS: i64 = 7200;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Type of workout activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityType {
    Walking,
    Running,
    Cycling,
    Strength,
    Swimming,
    Yoga,
    Other,
}

impl ActivityType {
    /// Metabolic equivalent of task (MET) for calorie estimation.
    #[must_use]
    pub fn met_value(self) -> f32 {
        match self {
            ActivityType::Walking => MET_WALKING,
            ActivityType::Running => MET_RUNNING,
            ActivityType::Cycling => MET_CYCLING,
            ActivityType::Strength => MET_STRENGTH,
            ActivityType::Swimming => 7.0,
            ActivityType::Yoga => 3.0,
            ActivityType::Other => MET_DEFAULT,
        }
    }
}

/// A single workout session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkoutSession {
    pub activity: ActivityType,
    /// Start time (unix epoch seconds).
    pub start_time: i64,
    /// Duration in seconds.
    pub duration_secs: u32,
    /// Estimated calories burned.
    pub calories: f32,
    /// Average heart rate during session (if available).
    pub avg_heart_rate: Option<f32>,
    /// Steps during this session (if applicable).
    pub steps: u32,
}

/// Daily activity summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyActivity {
    /// Date as days since unix epoch (day_index = timestamp / 86400).
    pub day_index: i64,
    /// Total steps for the day.
    pub steps: u32,
    /// Total calories burned (estimated).
    pub calories: f32,
    /// Total active minutes.
    pub active_minutes: u32,
    /// Number of workout sessions.
    pub workout_count: u16,
    /// Longest sedentary period in seconds.
    pub max_sedentary_secs: i64,
}

// ---------------------------------------------------------------------------
// FitnessTracker
// ---------------------------------------------------------------------------

/// Tracks fitness metrics: steps, workouts, activity scoring.
///
/// All collections are bounded with ring buffers.
#[derive(Debug, Serialize, Deserialize)]
pub struct FitnessTracker {
    /// Daily activity summaries (ring buffer).
    daily: Vec<DailyActivity>,
    daily_cursor: usize,
    /// Workout sessions (ring buffer).
    workouts: Vec<WorkoutSession>,
    workout_cursor: usize,
    /// Daily step goal.
    step_goal: u32,
    /// User weight in kg (for calorie estimation).
    weight_kg: f32,
    /// Timestamp of last activity observation.
    last_activity_at: i64,
    /// Current day's accumulated steps.
    today_steps: u32,
    /// Current day index.
    today_index: i64,
}

impl FitnessTracker {
    /// Create a new fitness tracker with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            daily: Vec::with_capacity(32),
            daily_cursor: 0,
            workouts: Vec::with_capacity(32),
            workout_cursor: 0,
            step_goal: DEFAULT_STEP_GOAL,
            weight_kg: 70.0,
            last_activity_at: 0,
            today_steps: 0,
            today_index: 0,
        }
    }

    /// Set the daily step goal.
    pub fn set_step_goal(&mut self, goal: u32) {
        self.step_goal = goal.max(1);
    }

    /// Set user weight for calorie estimation.
    pub fn set_weight_kg(&mut self, kg: f32) {
        self.weight_kg = kg.clamp(20.0, 300.0);
    }

    /// Record steps from a sensor sync.
    ///
    /// `timestamp` is unix epoch seconds, `steps` is the incremental count.
    pub fn record_steps(&mut self, timestamp: i64, steps: u32) {
        let day_index = timestamp / 86400;

        // Roll over to new day if needed
        if day_index != self.today_index {
            self.flush_today(self.today_index);
            self.today_steps = 0;
            self.today_index = day_index;
        }

        self.today_steps = self.today_steps.saturating_add(steps);
        self.last_activity_at = timestamp;

        debug!(steps, total = self.today_steps, "steps recorded");
    }

    /// Record a completed workout session.
    pub fn record_workout(&mut self, session: WorkoutSession) -> Result<(), ArcError> {
        if self.workouts.len() < MAX_WORKOUT_SESSIONS {
            self.workouts.push(session);
        } else {
            let idx = self.workout_cursor % MAX_WORKOUT_SESSIONS;
            self.workouts[idx] = session;
        }
        self.workout_cursor += 1;

        Ok(())
    }

    /// Estimate calories for a workout.
    ///
    /// `calories = MET × weight_kg × duration_hours`
    #[must_use]
    pub fn estimate_calories(&self, activity: ActivityType, duration_secs: u32) -> f32 {
        let hours = duration_secs as f32 / 3600.0;
        activity.met_value() * self.weight_kg * hours
    }

    /// Compute activity score (0.0 to 1.0).
    ///
    /// Factors:
    /// - Step completion ratio (40%)
    /// - Active minutes vs target (30%)
    /// - Workout frequency (20%)
    /// - Sedentary avoidance (10%)
    #[must_use]
    pub fn activity_score(&self) -> f32 {
        let step_ratio = if self.step_goal > 0 {
            (self.today_steps as f32 / self.step_goal as f32).min(1.5)
        } else {
            0.5
        };
        // Normalize: 100% goal = 0.8, 150%+ = 1.0
        let step_score = (step_ratio / 1.5).min(1.0);

        // Active minutes from recent workouts (last 7 days)
        let recent_active_mins = self.recent_active_minutes(7);
        // Target: 150 min/week (WHO recommendation)
        let active_score = (recent_active_mins as f32 / 150.0).min(1.0);

        // Workout frequency: at least 3 sessions per week
        let recent_workouts = self.recent_workout_count(7);
        let workout_score = (recent_workouts as f32 / 3.0).min(1.0);

        // Sedentary avoidance: based on max sedentary period today
        let sedentary_score = if self.last_activity_at > 0 {
            1.0 // Simplified: if there's any activity, score is good
        } else {
            0.5
        };

        let total =
            step_score * 0.4 + active_score * 0.3 + workout_score * 0.2 + sedentary_score * 0.1;

        total.clamp(0.0, 1.0)
    }

    /// Sum of active minutes from workouts in the last N days.
    fn recent_active_minutes(&self, days: i64) -> u32 {
        let cutoff = self.today_index.saturating_sub(days);
        self.workouts
            .iter()
            .filter(|w| w.start_time / 86400 >= cutoff)
            .map(|w| w.duration_secs / 60)
            .sum()
    }

    /// Count of workouts in the last N days.
    fn recent_workout_count(&self, days: i64) -> usize {
        let cutoff = self.today_index.saturating_sub(days);
        self.workouts
            .iter()
            .filter(|w| w.start_time / 86400 >= cutoff)
            .count()
    }

    /// Flush current day's data to the daily ring buffer.
    fn flush_today(&mut self, day_index: i64) {
        if self.today_steps == 0 && day_index == 0 {
            return; // Don't flush empty initial state
        }

        let daily = DailyActivity {
            day_index,
            steps: self.today_steps,
            calories: self.today_steps as f32 * CALORIES_PER_STEP,
            active_minutes: self.today_active_minutes(),
            workout_count: self.today_workout_count(day_index),
            max_sedentary_secs: 0, // Simplified — would need motion sensor data
        };

        if self.daily.len() < MAX_DAILY_RECORDS {
            self.daily.push(daily);
        } else {
            let idx = self.daily_cursor % MAX_DAILY_RECORDS;
            self.daily[idx] = daily;
        }
        self.daily_cursor += 1;
    }

    /// Active minutes for the current day.
    fn today_active_minutes(&self) -> u32 {
        self.workouts
            .iter()
            .filter(|w| w.start_time / 86400 == self.today_index)
            .map(|w| w.duration_secs / 60)
            .sum()
    }

    /// Workout count for a given day.
    fn today_workout_count(&self, day_index: i64) -> u16 {
        self.workouts
            .iter()
            .filter(|w| w.start_time / 86400 == day_index)
            .count()
            .min(u16::MAX as usize) as u16
    }

    /// Total workout sessions tracked.
    #[must_use]
    pub fn total_sessions(&self) -> usize {
        self.workouts.len()
    }

    /// Today's step count.
    #[must_use]
    pub fn today_steps(&self) -> u32 {
        self.today_steps
    }

    /// Access to daily summaries.
    #[must_use]
    pub fn daily_records(&self) -> &[DailyActivity] {
        &self.daily
    }

    /// Detect if user has been sedentary too long.
    ///
    /// Returns seconds since last activity, or `None` if no activity recorded.
    #[must_use]
    pub fn sedentary_check(&self, now: i64) -> Option<i64> {
        if self.last_activity_at == 0 {
            return None;
        }
        let elapsed = now - self.last_activity_at;
        if elapsed >= SEDENTARY_THRESHOLD_SECS {
            Some(elapsed)
        } else {
            None
        }
    }

    // -----------------------------------------------------------------------
    // Recommendations engine
    // -----------------------------------------------------------------------

    /// Generate fitness recommendations based on recent activity data.
    ///
    /// Produces up to 4 recommendations covering steps, activity minutes,
    /// workout frequency, and sedentary behavior.  Sorted by priority
    /// (1 = highest).
    #[must_use]
    pub fn generate_recommendations(&self, now: i64) -> Vec<FitnessRecommendation> {
        let mut recs = Vec::with_capacity(4);

        // Step completion
        if self.step_goal > 0 {
            let ratio = self.today_steps as f32 / self.step_goal as f32;
            if ratio < 0.5 {
                recs.push(FitnessRecommendation {
                    category: "steps",
                    signal: "steps_low",
                    priority: 1,
                    confidence: (1.0 - ratio).clamp(0.0, 1.0),
                });
            } else if ratio < 0.8 {
                recs.push(FitnessRecommendation {
                    category: "steps",
                    signal: "steps_near_goal",
                    priority: 2,
                    confidence: (0.8 - ratio).clamp(0.0, 1.0) * 2.0,
                });
            }
        }

        // Active minutes (WHO: 150 min/week)
        let active_mins = self.recent_active_minutes(7);
        if active_mins < 75 {
            recs.push(FitnessRecommendation {
                category: "activity",
                signal: "activity_very_low",
                priority: 1,
                confidence: (1.0 - active_mins as f32 / 150.0).clamp(0.0, 1.0),
            });
        } else if active_mins < 150 {
            recs.push(FitnessRecommendation {
                category: "activity",
                signal: "activity_near_goal",
                priority: 2,
                confidence: (1.0 - active_mins as f32 / 150.0).clamp(0.0, 1.0),
            });
        }

        // Workout frequency
        let workout_count = self.recent_workout_count(7);
        if workout_count == 0 {
            recs.push(FitnessRecommendation {
                category: "workout",
                signal: "workout_none",
                priority: 1,
                confidence: 0.9,
            });
        } else if workout_count < 3 {
            recs.push(FitnessRecommendation {
                category: "workout",
                signal: "workout_low",
                priority: 3,
                confidence: 0.5,
            });
        }

        // Sedentary check
        if self.sedentary_check(now).is_some() {
            recs.push(FitnessRecommendation {
                category: "sedentary",
                signal: "sedentary_alert",
                priority: 1,
                confidence: 0.85,
            });
        }

        recs.sort_by_key(|r| r.priority);
        recs.truncate(4);
        recs
    }
}

// ---------------------------------------------------------------------------
// Recommendation types
// ---------------------------------------------------------------------------

/// A fitness improvement recommendation.
#[derive(Debug, Clone)]
pub struct FitnessRecommendation {
    /// Category: "steps", "activity", "workout", "sedentary".
    pub category: &'static str,
    /// Signal code identifying the specific condition (passed to LLM for message composition).
    /// Examples: "steps_low", "steps_near_goal", "activity_very_low", "activity_near_goal",
    /// "workout_none", "workout_low", "sedentary_alert".
    pub signal: &'static str,
    /// Priority: 1 = high, 2 = medium, 3 = low.
    pub priority: u8,
    /// Confidence that this recommendation is relevant (0.0–1.0).
    pub confidence: f32,
}

impl Default for FitnessTracker {
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
    fn test_met_values() {
        assert!(ActivityType::Running.met_value() > ActivityType::Walking.met_value());
        assert!(ActivityType::Swimming.met_value() > ActivityType::Yoga.met_value());
    }

    #[test]
    fn test_calorie_estimation() {
        let tracker = FitnessTracker::new(); // 70 kg default
                                             // 30 min running: 8.0 * 70 * 0.5 = 280
        let cal = tracker.estimate_calories(ActivityType::Running, 1800);
        assert!((cal - 280.0).abs() < 1.0, "got {cal}");
    }

    #[test]
    fn test_step_recording() {
        let mut tracker = FitnessTracker::new();
        tracker.record_steps(86400, 5000);
        assert_eq!(tracker.today_steps(), 5000);

        tracker.record_steps(86400 + 3600, 3000);
        assert_eq!(tracker.today_steps(), 8000);
    }

    #[test]
    fn test_day_rollover() {
        let mut tracker = FitnessTracker::new();
        tracker.record_steps(86400, 10000);
        assert_eq!(tracker.today_steps(), 10000);

        // New day
        tracker.record_steps(86400 * 2, 500);
        assert_eq!(tracker.today_steps(), 500);
        // Previous day should be flushed
        assert_eq!(tracker.daily.len(), 1);
        assert_eq!(tracker.daily[0].steps, 10000);
    }

    #[test]
    fn test_activity_score_default() {
        let tracker = FitnessTracker::new();
        let score = tracker.activity_score();
        assert!(score >= 0.0 && score <= 1.0, "got {score}");
    }

    #[test]
    fn test_sedentary_check() {
        let mut tracker = FitnessTracker::new();
        assert!(tracker.sedentary_check(1000).is_none()); // No activity yet

        tracker.record_steps(1000, 100);
        assert!(tracker.sedentary_check(2000).is_none()); // Only 1000s
        assert!(tracker.sedentary_check(1000 + 7200).is_some()); // 2 hours
    }

    #[test]
    fn test_workout_recording() {
        let mut tracker = FitnessTracker::new();
        let session = WorkoutSession {
            activity: ActivityType::Running,
            start_time: 86400,
            duration_secs: 1800,
            calories: 280.0,
            avg_heart_rate: Some(150.0),
            steps: 3000,
        };
        tracker.record_workout(session).expect("record");
        assert_eq!(tracker.total_sessions(), 1);
    }

    #[test]
    fn test_step_goal_customization() {
        let mut tracker = FitnessTracker::new();
        tracker.set_step_goal(8000);
        assert_eq!(tracker.step_goal, 8000);

        tracker.set_step_goal(0);
        assert_eq!(tracker.step_goal, 1); // Minimum of 1
    }

    // -----------------------------------------------------------------------
    // Recommendation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_recommendations_no_activity() {
        let tracker = FitnessTracker::new();
        let recs = tracker.generate_recommendations(86400);
        // No steps, no workouts → should generate step + workout recs
        let has_steps = recs.iter().any(|r| r.category == "steps");
        let has_workout = recs.iter().any(|r| r.category == "workout");
        assert!(has_steps, "should recommend steps");
        assert!(has_workout, "should recommend workouts");
    }

    #[test]
    fn test_recommendations_good_activity() {
        let mut tracker = FitnessTracker::new();
        let day = 86400;
        tracker.record_steps(day, 12000); // Above goal

        // 3 workouts this week, >150 min
        for i in 0..3 {
            let session = WorkoutSession {
                activity: ActivityType::Running,
                start_time: day + i * 3600,
                duration_secs: 3600, // 1h each
                calories: 500.0,
                avg_heart_rate: None,
                steps: 5000,
            };
            tracker.record_workout(session).expect("ok");
        }

        let recs = tracker.generate_recommendations(day + 1000);
        // Good activity → few recs
        assert!(recs.len() <= 1, "expected <=1 recs, got {}", recs.len());
    }

    #[test]
    fn test_recommendations_sedentary() {
        let mut tracker = FitnessTracker::new();
        tracker.record_steps(1000, 100);
        // 3 hours later
        let now = 1000 + 3 * 3600;
        let recs = tracker.generate_recommendations(now);
        let has_sedentary = recs.iter().any(|r| r.category == "sedentary");
        assert!(has_sedentary, "should detect prolonged sitting");
    }

    #[test]
    fn test_recommendations_partial_steps() {
        let mut tracker = FitnessTracker::new();
        let day = 86400;
        tracker.record_steps(day, 7000); // 70% of 10k goal
        let recs = tracker.generate_recommendations(day + 100);
        let step_recs: Vec<_> = recs.iter().filter(|r| r.category == "steps").collect();
        assert!(!step_recs.is_empty(), "should have step recommendation");
        assert_eq!(
            step_recs[0].priority, 2,
            "near-goal should be medium priority"
        );
    }

    #[test]
    fn test_recommendations_max_four() {
        let tracker = FitnessTracker::new();
        // Worst case: no steps, no workouts, no activity, but no sedentary (no activity_at)
        let recs = tracker.generate_recommendations(86400);
        assert!(
            recs.len() <= 4,
            "should never return more than 4, got {}",
            recs.len()
        );
    }
}
