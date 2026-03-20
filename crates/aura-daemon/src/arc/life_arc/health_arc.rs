//! Health life arc — tracks exercise, sleep, and wellbeing patterns.
//!
//! # Distinction from `arc::health::HealthDomain`
//!
//! `HealthDomain` is the DAEMON health subsystem (medication tracking, vitals
//! monitoring, device sensor integration — all pulling from Android health APIs).
//!
//! This `HealthArc` is the LIFE ARC layer: it tracks user-reported or
//! user-triggered health behaviours for the Life Arc scoring system. It feeds
//! the LLM context injection pipeline, not the daemon sensor pipeline.
//!
//! # What this tracks (FACTS, not advice)
//!
//! - Exercise session events (user-reported or inferred)
//! - Sleep records (duration and self-reported quality)
//! - Meal/nutrition signals (self-reported)
//! - Mood records (self-reported)
//!
//! # Scoring formula
//!
//! `score = exercise_score * 0.40 + sleep_score * 0.35 + mood_score * 0.25`
//!
//! Circadian science: CONSISTENCY matters more than intensity.
//! Day-zero: all sub-scores default to 0.5 → overall 0.5 (Stable).

use serde::{Deserialize, Serialize};

use super::primitives::{ArcHealth, ArcType, ProactiveTrigger, ONE_DAY_MS};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const W_EXERCISE: f32 = 0.40;
const W_SLEEP: f32 = 0.35;
const W_MOOD: f32 = 0.25;

/// Rolling window for exercise/sleep scoring.
const ROLLING_WINDOW_7D_MS: u64 = 7 * ONE_DAY_MS;
const ROLLING_WINDOW_30D_MS: u64 = 30 * ONE_DAY_MS;

/// Max exercise sessions stored in ring buffer.
const MAX_EXERCISE_SESSIONS: usize = 120;
/// Max sleep records stored.
const MAX_SLEEP_RECORDS: usize = 60;
/// Max mood records stored.
const MAX_MOOD_RECORDS: usize = 60;

/// Minimum events before triggering (day-zero guard).
const MIN_EVENTS_FOR_TRIGGER: u32 = 3;

// ---------------------------------------------------------------------------
// HealthEvent
// ---------------------------------------------------------------------------

/// Events the user or daemon records into the health life arc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthEvent {
    /// A physical exercise session.
    ExerciseSession {
        /// Duration in minutes.
        duration_minutes: u32,
        /// User-provided activity label (e.g. "running", "yoga", "gym").
        activity_type: String,
        /// Self-reported intensity 1–10.
        intensity: u8,
    },
    /// A sleep record.
    SleepRecord {
        /// Hours slept.
        duration_hours: f32,
        /// Self-reported sleep quality 1–10 (optional).
        quality_self_reported: Option<u8>,
    },
    /// A meal/nutrition record.
    MealRecord {
        /// Free-text description of the meal.
        description: String,
        /// Self-reported nutrition quality 1–10 (optional).
        self_reported_nutrition_score: Option<u8>,
    },
    /// A mood record.
    UserMoodRecord {
        /// Mood score 1–10 (1 = very low, 10 = very high).
        mood_score: u8,
        /// Optional note.
        note: Option<String>,
    },
    /// User updated their exercise frequency goal.
    ExerciseGoalSet {
        /// Target number of sessions per week.
        sessions_per_week: u8,
    },
}

// ---------------------------------------------------------------------------
// Internal record types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExerciseRecord {
    ts_ms: u64,
    duration_minutes: u32,
    intensity: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SleepRecord {
    ts_ms: u64,
    duration_hours: f32,
    quality: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MoodRecord {
    ts_ms: u64,
    mood_score: u8,
}

// ---------------------------------------------------------------------------
// HealthArc
// ---------------------------------------------------------------------------

/// Tracks health-related behavioural patterns for the life arc system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthArc {
    /// Current health level.
    pub health: ArcHealth,

    /// Number of exercise sessions in the rolling 30-day window.
    pub exercise_sessions_30d: u32,

    /// Average sleep hours over the rolling 7-day window.
    pub avg_sleep_hours_7d: f32,

    /// Mood trend over 7 days: exponential moving average in `[1.0, 10.0]`.
    /// 0.0 = no mood data recorded yet.
    pub mood_trend_7d: f32,

    /// Unix ms timestamp of the most recent exercise session.
    pub last_exercise_ms: u64,

    /// Unix ms timestamp of the last proactive trigger (0 = never).
    pub last_trigger_ms: u64,

    /// User-set exercise frequency goal (sessions per week). Default: 3.
    pub exercise_goal_sessions_per_week: u8,

    /// Total events recorded (lifetime, for day-zero guard).
    pub total_events: u32,

    // Internal ring buffers.
    exercise_records: Vec<ExerciseRecord>,
    sleep_records: Vec<SleepRecord>,
    mood_records: Vec<MoodRecord>,
}

impl HealthArc {
    /// Create a new health arc in the day-zero state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            health: ArcHealth::Stable,
            exercise_sessions_30d: 0,
            avg_sleep_hours_7d: 0.0,
            mood_trend_7d: 0.0,
            last_exercise_ms: 0,
            last_trigger_ms: 0,
            exercise_goal_sessions_per_week: 3,
            total_events: 0,
            exercise_records: Vec::with_capacity(16),
            sleep_records: Vec::with_capacity(16),
            mood_records: Vec::with_capacity(16),
        }
    }

    /// Record a health event at the given timestamp.
    pub fn record_event(&mut self, event: HealthEvent, now_ms: u64) {
        self.total_events = self.total_events.saturating_add(1);

        match event {
            HealthEvent::ExerciseSession {
                duration_minutes,
                intensity,
                ..
            } => {
                if self.exercise_records.len() >= MAX_EXERCISE_SESSIONS {
                    self.exercise_records.remove(0);
                }
                self.exercise_records.push(ExerciseRecord {
                    ts_ms: now_ms,
                    duration_minutes,
                    intensity: intensity.clamp(1, 10),
                });
                self.last_exercise_ms = now_ms;
                self.refresh_exercise_count(now_ms);
            }
            HealthEvent::SleepRecord {
                duration_hours,
                quality_self_reported,
            } => {
                if self.sleep_records.len() >= MAX_SLEEP_RECORDS {
                    self.sleep_records.remove(0);
                }
                self.sleep_records.push(SleepRecord {
                    ts_ms: now_ms,
                    duration_hours: duration_hours.clamp(0.0, 24.0),
                    quality: quality_self_reported,
                });
                self.refresh_sleep_avg(now_ms);
            }
            HealthEvent::MealRecord { .. } => {
                // Nutrition signals recorded but not yet factored into scoring.
                // Placeholder for future scoring integration.
            }
            HealthEvent::UserMoodRecord { mood_score, .. } => {
                if self.mood_records.len() >= MAX_MOOD_RECORDS {
                    self.mood_records.remove(0);
                }
                self.mood_records.push(MoodRecord {
                    ts_ms: now_ms,
                    mood_score: mood_score.clamp(1, 10),
                });
                self.refresh_mood_trend(now_ms);
            }
            HealthEvent::ExerciseGoalSet { sessions_per_week } => {
                self.exercise_goal_sessions_per_week = sessions_per_week.clamp(1, 14);
            }
        }
    }

    /// Recompute rolling 30-day exercise session count.
    fn refresh_exercise_count(&mut self, now_ms: u64) {
        let window_start = now_ms.saturating_sub(ROLLING_WINDOW_30D_MS);
        self.exercise_sessions_30d = self
            .exercise_records
            .iter()
            .filter(|r| r.ts_ms >= window_start)
            .count() as u32;
    }

    /// Recompute 7-day average sleep hours.
    fn refresh_sleep_avg(&mut self, now_ms: u64) {
        let window_start = now_ms.saturating_sub(ROLLING_WINDOW_7D_MS);
        let recent: Vec<f32> = self
            .sleep_records
            .iter()
            .filter(|r| r.ts_ms >= window_start)
            .map(|r| r.duration_hours)
            .collect();
        if recent.is_empty() {
            self.avg_sleep_hours_7d = 0.0;
        } else {
            self.avg_sleep_hours_7d = recent.iter().sum::<f32>() / recent.len() as f32;
        }
    }

    /// Recompute 7-day mood trend (exponential moving average).
    fn refresh_mood_trend(&mut self, now_ms: u64) {
        let window_start = now_ms.saturating_sub(ROLLING_WINDOW_7D_MS);
        let recent: Vec<f32> = self
            .mood_records
            .iter()
            .filter(|r| r.ts_ms >= window_start)
            .map(|r| r.mood_score as f32)
            .collect();
        if recent.is_empty() {
            // Preserve previous trend.
            return;
        }
        // Simple mean for the 7-day window.
        let mean = recent.iter().sum::<f32>() / recent.len() as f32;
        self.mood_trend_7d = mean;
    }

    /// Compute the health arc score in `[0.0, 1.0]`.
    ///
    /// Day-zero: returns 0.5 with no events.
    #[must_use]
    pub fn score(&self, now_ms: u64) -> f32 {
        if self.total_events == 0 {
            return 0.5; // Day-zero neutral
        }

        // --- Exercise score ---
        // Target: user's goal in sessions/week × 4 weeks = sessions/month.
        let monthly_target = self.exercise_goal_sessions_per_week as f32 * 4.0;
        let exercise_score = if monthly_target > 0.0 {
            let window_start = now_ms.saturating_sub(ROLLING_WINDOW_30D_MS);
            let count = self
                .exercise_records
                .iter()
                .filter(|r| r.ts_ms >= window_start)
                .count() as f32;
            (count / monthly_target).min(1.0)
        } else {
            0.5
        };

        // Consistency bonus: exercise within last 3 days adds a small boost.
        let recency_bonus = if self.last_exercise_ms > 0
            && now_ms.saturating_sub(self.last_exercise_ms) < 3 * ONE_DAY_MS
        {
            0.1_f32
        } else {
            0.0
        };
        let exercise_score = (exercise_score + recency_bonus).min(1.0);

        // --- Sleep score ---
        // Optimal: 7–9 hours. Outside: penalised proportionally.
        // Day-zero (no sleep data): neutral 0.5.
        let sleep_score = if self.avg_sleep_hours_7d <= 0.0 {
            0.5 // No sleep data
        } else {
            let h = self.avg_sleep_hours_7d;
            if (7.0..=9.0).contains(&h) {
                1.0
            } else if (6.0..7.0).contains(&h) {
                0.7
            } else if h > 9.0 && h <= 10.0 {
                0.8 // Slightly over is less concerning than under
            } else if (5.0..6.0).contains(&h) {
                0.4
            } else {
                0.2 // Severe under/over-sleep
            }
        };

        // --- Mood score ---
        // Normalise from [1,10] to [0,1]. Day-zero (0.0): neutral 0.5.
        let mood_score = if self.mood_trend_7d <= 0.0 {
            0.5
        } else {
            ((self.mood_trend_7d - 1.0) / 9.0).clamp(0.0, 1.0)
        };

        let raw = W_EXERCISE * exercise_score + W_SLEEP * sleep_score + W_MOOD * mood_score;
        raw.clamp(0.0, 1.0)
    }

    /// Recompute and update the health level.
    pub fn update_health(&mut self, now_ms: u64) {
        let s = self.score(now_ms);
        self.health = ArcHealth::from_score(s);
    }

    /// Check whether a proactive trigger should fire.
    #[must_use]
    pub fn check_proactive_trigger(&self, now_ms: u64) -> Option<ProactiveTrigger> {
        // Day-zero guard.
        if self.total_events < MIN_EVENTS_FOR_TRIGGER {
            return None;
        }

        if !self.health.warrants_proactive() {
            return None;
        }

        // 24-hour cooldown.
        if self.last_trigger_ms > 0 && now_ms.saturating_sub(self.last_trigger_ms) < ONE_DAY_MS {
            return None;
        }

        Some(ProactiveTrigger {
            arc_type: ArcType::Health,
            health: self.health.clone(),
            triggered_at_ms: now_ms,
            context_for_llm: self.to_llm_context(),
        })
    }

    /// Acknowledge that a trigger was fired.
    pub fn mark_trigger_fired(&mut self, now_ms: u64) {
        self.last_trigger_ms = now_ms;
    }

    /// Build structured factual context for LLM injection.
    #[must_use]
    pub fn to_llm_context(&self) -> String {
        let days_since_exercise = if self.last_exercise_ms == 0 {
            "unknown".to_string()
        } else {
            // Will be stale until caller passes now_ms; using stored last_exercise_ms
            // as relative context is sufficient for LLM.
            "see_last_exercise_ms".to_string()
        };

        format!(
            "[health_arc] health={health} \
             exercise_sessions_30d={ex30} \
             exercise_goal_sessions_per_week={goal} \
             avg_sleep_hours_7d={sleep:.2} \
             mood_trend_7d={mood:.1} \
             days_since_exercise={dse} \
             total_events_lifetime={total}",
            health = self.health.label(),
            ex30 = self.exercise_sessions_30d,
            goal = self.exercise_goal_sessions_per_week,
            sleep = self.avg_sleep_hours_7d,
            mood = self.mood_trend_7d,
            dse = days_since_exercise,
            total = self.total_events,
        )
    }

    /// Build structured context with the caller-supplied `now_ms` for accurate
    /// "days since exercise" computation.
    #[must_use]
    pub fn to_llm_context_at(&self, now_ms: u64) -> String {
        let days_since_exercise = if self.last_exercise_ms == 0 {
            "never".to_string()
        } else {
            let ms = now_ms.saturating_sub(self.last_exercise_ms);
            format!("{:.1}", ms as f64 / ONE_DAY_MS as f64)
        };

        format!(
            "[health_arc] health={health} \
             exercise_sessions_30d={ex30} \
             exercise_goal_sessions_per_week={goal} \
             avg_sleep_hours_7d={sleep:.2} \
             mood_trend_7d={mood:.1} \
             days_since_last_exercise={dse} \
             total_events_lifetime={total}",
            health = self.health.label(),
            ex30 = self.exercise_sessions_30d,
            goal = self.exercise_goal_sessions_per_week,
            sleep = self.avg_sleep_hours_7d,
            mood = self.mood_trend_7d,
            dse = days_since_exercise,
            total = self.total_events,
        )
    }
}

impl Default for HealthArc {
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

    const T0: u64 = 3_000_000_000_u64;
    const ONE_HOUR: u64 = 3_600_000;

    #[test]
    fn test_day_zero_stable() {
        let arc = HealthArc::new();
        assert_eq!(arc.health, ArcHealth::Stable);
        assert_eq!(arc.score(T0), 0.5);
    }

    #[test]
    fn test_no_trigger_day_zero() {
        let arc = HealthArc::new();
        assert!(arc.check_proactive_trigger(T0).is_none());
    }

    #[test]
    fn test_exercise_increases_score() {
        let mut arc = HealthArc::new();
        arc.exercise_goal_sessions_per_week = 3;
        // Record 12 sessions (goal = 3/week × 4 weeks = 12) → full score.
        for i in 0..12u64 {
            arc.record_event(
                HealthEvent::ExerciseSession {
                    duration_minutes: 45,
                    activity_type: "running".into(),
                    intensity: 7,
                },
                T0 + i * 2 * ONE_DAY_MS,
            );
        }
        let s = arc.score(T0 + 24 * ONE_DAY_MS);
        assert!(s > 0.5, "full exercise goal should score > 0.5, got {s}");
    }

    #[test]
    fn test_sleep_score_optimal() {
        let mut arc = HealthArc::new();
        // Record 7 nights of 8h sleep.
        for d in 0..7u64 {
            arc.record_event(
                HealthEvent::SleepRecord {
                    duration_hours: 8.0,
                    quality_self_reported: Some(8),
                },
                T0 + d * ONE_DAY_MS,
            );
        }
        // Sleep component should be 1.0 (8h = optimal).
        assert!(
            (arc.avg_sleep_hours_7d - 8.0).abs() < 0.01,
            "avg should be 8h, got {}",
            arc.avg_sleep_hours_7d
        );
    }

    #[test]
    fn test_mood_tracking() {
        let mut arc = HealthArc::new();
        for i in 0..5u64 {
            arc.record_event(
                HealthEvent::UserMoodRecord {
                    mood_score: 8,
                    note: None,
                },
                T0 + i * ONE_HOUR,
            );
        }
        assert!(
            (arc.mood_trend_7d - 8.0).abs() < 0.1,
            "mood avg should ~8, got {}",
            arc.mood_trend_7d
        );
    }

    #[test]
    fn test_trigger_fires_when_at_risk() {
        let mut arc = HealthArc::new();
        // Add enough events.
        for i in 0..MIN_EVENTS_FOR_TRIGGER {
            arc.record_event(
                HealthEvent::UserMoodRecord {
                    mood_score: 2,
                    note: None,
                },
                T0 + i as u64 * ONE_HOUR,
            );
        }
        arc.health = ArcHealth::AtRisk;
        let t = arc.check_proactive_trigger(T0 + 10 * ONE_HOUR);
        assert!(t.is_some());
    }

    #[test]
    fn test_trigger_cooldown_respected() {
        let mut arc = HealthArc::new();
        for i in 0..MIN_EVENTS_FOR_TRIGGER {
            arc.record_event(
                HealthEvent::UserMoodRecord {
                    mood_score: 1,
                    note: None,
                },
                T0 + i as u64 * ONE_HOUR,
            );
        }
        arc.health = ArcHealth::NeedsAttention;

        let _ = arc.check_proactive_trigger(T0 + 5 * ONE_HOUR);
        arc.mark_trigger_fired(T0 + 5 * ONE_HOUR);

        let t2 = arc.check_proactive_trigger(T0 + 10 * ONE_HOUR);
        assert!(t2.is_none(), "should be on cooldown");

        let t3 = arc.check_proactive_trigger(T0 + 5 * ONE_HOUR + ONE_DAY_MS);
        assert!(t3.is_some(), "should fire after 24h");
    }

    #[test]
    fn test_exercise_goal_configurable() {
        let mut arc = HealthArc::new();
        arc.record_event(
            HealthEvent::ExerciseGoalSet {
                sessions_per_week: 5,
            },
            T0,
        );
        assert_eq!(arc.exercise_goal_sessions_per_week, 5);
    }

    #[test]
    fn test_llm_context_structure() {
        let mut arc = HealthArc::new();
        arc.record_event(
            HealthEvent::ExerciseSession {
                duration_minutes: 30,
                activity_type: "yoga".into(),
                intensity: 4,
            },
            T0,
        );
        let ctx = arc.to_llm_context_at(T0 + ONE_HOUR);
        assert!(ctx.contains("[health_arc]"));
        assert!(ctx.contains("health="));
        assert!(ctx.contains("exercise_sessions_30d="));
    }
}
