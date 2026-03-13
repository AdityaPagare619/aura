//! Growth life arc — tracks learning, skill development, and goal progress.
//!
//! # Theoretical grounding (Self-Determination Theory)
//!
//! SDT identifies three core motivational pillars:
//! - **Autonomy**: user chooses their goals and skills freely
//! - **Competence**: making measurable progress on meaningful skills
//! - **Relatedness**: growth connects to values and identity
//!
//! This arc tracks EVIDENCE of autonomy (user-set goals), competence
//! (learning sessions, skill practice, measurable progress), and relatedness
//! (topics/goals the user keeps returning to). The LLM reasons about the
//! implications — the arc just tracks facts.
//!
//! # Scoring formula
//!
//! `score = learning_consistency * 0.40 + goal_momentum * 0.40 + breadth_bonus * 0.20`
//!
//! Day-zero: all sub-scores default to 0.5 → overall 0.5 (Stable).

use serde::{Deserialize, Serialize};

use super::primitives::{ArcHealth, ArcType, ProactiveTrigger, ONE_DAY_MS};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const W_LEARNING_CONSISTENCY: f32 = 0.40;
const W_GOAL_MOMENTUM: f32 = 0.40;
const W_BREADTH_BONUS: f32 = 0.20;

/// Rolling windows.
const ROLLING_WINDOW_7D_MS: u64 = 7 * ONE_DAY_MS;
const ROLLING_WINDOW_30D_MS: u64 = 30 * ONE_DAY_MS;

/// Target learning minutes per day (used for consistency scoring).
const TARGET_DAILY_LEARNING_MINUTES: f32 = 30.0;

/// Maximum learning session records in ring buffer.
const MAX_LEARNING_RECORDS: usize = 180;

/// Maximum active goals.
const MAX_GOALS: usize = 50;

/// Minimum events before triggering (day-zero guard).
const MIN_EVENTS_FOR_TRIGGER: u32 = 3;

/// Number of distinct topics/skills in 30 days to consider "broad" (max breadth bonus).
const BREADTH_TARGET_TOPICS: usize = 5;

// ---------------------------------------------------------------------------
// GrowthEvent
// ---------------------------------------------------------------------------

/// Events the user records into the growth arc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GrowthEvent {
    /// A learning/study session on a topic.
    LearningSession {
        topic: String,
        duration_minutes: u32,
    },
    /// Deliberate practice of a specific skill.
    SkillPractice {
        skill: String,
        duration_minutes: u32,
    },
    /// Progress update on an active goal.
    GoalProgress {
        goal_id: String,
        /// Progress percentage (0.0–100.0).
        progress_pct: f32,
        note: Option<String>,
    },
    /// Book reading session.
    BookReading {
        title: String,
        pages: u32,
    },
    /// Progress through an online course or structured curriculum.
    CourseProgress {
        course: String,
        lesson_count: u32,
    },
    /// User creates a new growth goal.
    GoalCreated {
        goal_id: String,
        description: String,
        target_deadline_ms: Option<u64>,
    },
}

// ---------------------------------------------------------------------------
// GrowthGoal
// ---------------------------------------------------------------------------

/// A user-defined growth goal tracked by the arc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthGoal {
    /// Opaque user-defined ID.
    pub goal_id: String,
    /// Human-readable description.
    pub description: String,
    /// Current progress percentage (0.0–100.0).
    pub progress_pct: f32,
    /// Unix ms timestamp of the most recent update.
    pub last_updated_ms: u64,
    /// Optional target deadline (unix ms).
    pub target_deadline_ms: Option<u64>,
    /// Whether the goal has been completed.
    pub completed: bool,
}

impl GrowthGoal {
    /// Whether this goal is stalled (no progress in >14 days).
    #[must_use]
    pub fn is_stalled(&self, now_ms: u64) -> bool {
        if self.completed {
            return false;
        }
        if self.last_updated_ms == 0 {
            return false; // Never updated — newly created
        }
        now_ms.saturating_sub(self.last_updated_ms) > 14 * ONE_DAY_MS
    }

    /// Whether this goal is overdue (deadline passed and not completed).
    #[must_use]
    pub fn is_overdue(&self, now_ms: u64) -> bool {
        if self.completed {
            return false;
        }
        match self.target_deadline_ms {
            None => false,
            Some(deadline) => now_ms > deadline,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal record types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LearningRecord {
    ts_ms: u64,
    topic: String,
    duration_minutes: u32,
}

// ---------------------------------------------------------------------------
// GrowthArc
// ---------------------------------------------------------------------------

/// Tracks learning activity and goal progress for the growth life arc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthArc {
    /// Current health level.
    pub health: ArcHealth,

    /// Number of learning sessions in the rolling 30-day window.
    pub learning_sessions_30d: u32,

    /// Average daily learning minutes over the rolling 7-day window.
    pub avg_daily_learning_minutes_7d: f32,

    /// Active (non-completed) goals.
    pub active_goals: Vec<GrowthGoal>,

    /// Unix ms timestamp of the last proactive trigger (0 = never).
    pub last_trigger_ms: u64,

    /// Total events recorded (lifetime, for day-zero guard).
    pub total_events: u32,

    // Internal ring buffer for learning records.
    learning_records: Vec<LearningRecord>,
}

impl GrowthArc {
    /// Create a new growth arc in the day-zero state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            health: ArcHealth::Stable,
            learning_sessions_30d: 0,
            avg_daily_learning_minutes_7d: 0.0,
            active_goals: Vec::with_capacity(8),
            last_trigger_ms: 0,
            total_events: 0,
            learning_records: Vec::with_capacity(16),
        }
    }

    /// Record a growth event at the given timestamp.
    pub fn record_event(&mut self, event: GrowthEvent, now_ms: u64) {
        self.total_events = self.total_events.saturating_add(1);

        match event {
            GrowthEvent::LearningSession {
                topic,
                duration_minutes,
            } => {
                self.push_learning_record(LearningRecord {
                    ts_ms: now_ms,
                    topic,
                    duration_minutes,
                });
                self.refresh_learning_stats(now_ms);
            }
            GrowthEvent::SkillPractice {
                skill,
                duration_minutes,
            } => {
                // Treat skill practice as a learning session for scoring.
                self.push_learning_record(LearningRecord {
                    ts_ms: now_ms,
                    topic: skill,
                    duration_minutes,
                });
                self.refresh_learning_stats(now_ms);
            }
            GrowthEvent::GoalProgress {
                goal_id,
                progress_pct,
                ..
            } => {
                if let Some(goal) = self.active_goals.iter_mut().find(|g| g.goal_id == goal_id) {
                    goal.progress_pct = progress_pct.clamp(0.0, 100.0);
                    goal.last_updated_ms = now_ms;
                    if (progress_pct - 100.0).abs() < f32::EPSILON {
                        goal.completed = true;
                    }
                }
            }
            GrowthEvent::BookReading { title, pages } => {
                // Convert pages to approximate minutes (avg 1.5 min/page).
                let minutes = (pages as f32 * 1.5) as u32;
                self.push_learning_record(LearningRecord {
                    ts_ms: now_ms,
                    topic: format!("book:{title}"),
                    duration_minutes: minutes,
                });
                self.refresh_learning_stats(now_ms);
            }
            GrowthEvent::CourseProgress { course, lesson_count } => {
                // Each lesson ≈ 15 minutes.
                let minutes = lesson_count * 15;
                self.push_learning_record(LearningRecord {
                    ts_ms: now_ms,
                    topic: format!("course:{course}"),
                    duration_minutes: minutes,
                });
                self.refresh_learning_stats(now_ms);
            }
            GrowthEvent::GoalCreated {
                goal_id,
                description,
                target_deadline_ms,
            } => {
                if self.active_goals.len() < MAX_GOALS {
                    self.active_goals.push(GrowthGoal {
                        goal_id,
                        description,
                        progress_pct: 0.0,
                        last_updated_ms: now_ms,
                        target_deadline_ms,
                        completed: false,
                    });
                }
                // Silently drop if at capacity — log would be better in production.
            }
        }
    }

    /// Push a learning record into the bounded ring buffer.
    fn push_learning_record(&mut self, record: LearningRecord) {
        if self.learning_records.len() >= MAX_LEARNING_RECORDS {
            self.learning_records.remove(0);
        }
        self.learning_records.push(record);
    }

    /// Recompute learning stats from stored records.
    fn refresh_learning_stats(&mut self, now_ms: u64) {
        let window_30d = now_ms.saturating_sub(ROLLING_WINDOW_30D_MS);
        self.learning_sessions_30d = self
            .learning_records
            .iter()
            .filter(|r| r.ts_ms >= window_30d)
            .count() as u32;

        let window_7d = now_ms.saturating_sub(ROLLING_WINDOW_7D_MS);
        let recent_7d: Vec<&LearningRecord> = self
            .learning_records
            .iter()
            .filter(|r| r.ts_ms >= window_7d)
            .collect();

        if recent_7d.is_empty() {
            self.avg_daily_learning_minutes_7d = 0.0;
        } else {
            let total_minutes: u32 = recent_7d.iter().map(|r| r.duration_minutes).sum();
            self.avg_daily_learning_minutes_7d = total_minutes as f32 / 7.0;
        }
    }

    /// Compute the growth arc score in `[0.0, 1.0]`.
    ///
    /// Day-zero: returns 0.5.
    #[must_use]
    pub fn score(&self, now_ms: u64) -> f32 {
        if self.total_events == 0 {
            return 0.5;
        }

        // --- Learning consistency score ---
        // Based on avg daily learning minutes vs target.
        let consistency_score = if self.avg_daily_learning_minutes_7d <= 0.0 {
            0.0
        } else {
            (self.avg_daily_learning_minutes_7d / TARGET_DAILY_LEARNING_MINUTES).min(1.0)
        };

        // --- Goal momentum score ---
        // Average progress of active non-completed goals, adjusted for staleness.
        let goal_momentum = self.compute_goal_momentum(now_ms);

        // --- Breadth bonus ---
        // Number of distinct topics in 30-day window.
        let window_30d = now_ms.saturating_sub(ROLLING_WINDOW_30D_MS);
        let distinct_topics: std::collections::HashSet<&str> = self
            .learning_records
            .iter()
            .filter(|r| r.ts_ms >= window_30d)
            .map(|r| r.topic.as_str())
            .collect();
        let breadth_score =
            (distinct_topics.len() as f32 / BREADTH_TARGET_TOPICS as f32).min(1.0);

        let raw = W_LEARNING_CONSISTENCY * consistency_score
            + W_GOAL_MOMENTUM * goal_momentum
            + W_BREADTH_BONUS * breadth_score;

        raw.clamp(0.0, 1.0)
    }

    /// Compute goal momentum score in `[0.0, 1.0]`.
    fn compute_goal_momentum(&self, now_ms: u64) -> f32 {
        let active: Vec<&GrowthGoal> = self
            .active_goals
            .iter()
            .filter(|g| !g.completed)
            .collect();

        if active.is_empty() {
            return 0.5; // No goals set — neutral, not penalised
        }

        let mut total_score = 0.0_f32;
        for goal in &active {
            let base = goal.progress_pct / 100.0;
            // Staleness penalty: stalled goals score is discounted.
            let stale_factor = if goal.is_stalled(now_ms) { 0.5 } else { 1.0 };
            // Overdue penalty: additional 0.3 discount.
            let overdue_factor = if goal.is_overdue(now_ms) { 0.7 } else { 1.0 };
            total_score += base * stale_factor * overdue_factor;
        }

        (total_score / active.len() as f32).clamp(0.0, 1.0)
    }

    /// Recompute and update the health level.
    pub fn update_health(&mut self, now_ms: u64) {
        let s = self.score(now_ms);
        self.health = ArcHealth::from_score(s);
    }

    /// Returns references to goals that are stalled or overdue.
    #[must_use]
    pub fn at_risk_goals(&self, now_ms: u64) -> Vec<&GrowthGoal> {
        self.active_goals
            .iter()
            .filter(|g| !g.completed && (g.is_stalled(now_ms) || g.is_overdue(now_ms)))
            .collect()
    }

    /// Check whether a proactive trigger should fire.
    #[must_use]
    pub fn check_proactive_trigger(&self, now_ms: u64) -> Option<ProactiveTrigger> {
        // Day-zero guard.
        if self.total_events < MIN_EVENTS_FOR_TRIGGER {
            return None;
        }

        let at_risk = self.at_risk_goals(now_ms);
        let should_trigger = self.health.warrants_proactive() || !at_risk.is_empty();
        if !should_trigger {
            return None;
        }

        // 24-hour cooldown.
        if self.last_trigger_ms > 0
            && now_ms.saturating_sub(self.last_trigger_ms) < ONE_DAY_MS
        {
            return None;
        }

        Some(ProactiveTrigger {
            arc_type: ArcType::Growth,
            health: self.health.clone(),
            triggered_at_ms: now_ms,
            context_for_llm: self.to_llm_context(now_ms),
        })
    }

    /// Acknowledge that a trigger was fired.
    pub fn mark_trigger_fired(&mut self, now_ms: u64) {
        self.last_trigger_ms = now_ms;
    }

    /// Build structured factual context for LLM injection.
    #[must_use]
    pub fn to_llm_context(&self, now_ms: u64) -> String {
        let stalled: Vec<&str> = self
            .at_risk_goals(now_ms)
            .iter()
            .map(|g| g.description.as_str())
            .collect();

        let goal_summary: Vec<String> = self
            .active_goals
            .iter()
            .filter(|g| !g.completed)
            .take(5) // Cap at 5 to avoid context overflow
            .map(|g| {
                format!(
                    "{}(progress={:.0}%,stalled={},overdue={})",
                    g.description,
                    g.progress_pct,
                    g.is_stalled(now_ms),
                    g.is_overdue(now_ms)
                )
            })
            .collect();

        format!(
            "[growth_arc] health={health} \
             learning_sessions_30d={ls30} \
             avg_daily_learning_minutes_7d={avg:.1} \
             active_goals={n_goals} \
             stalled_or_overdue_goals=[{stalled}] \
             goals=[{goals}] \
             total_events_lifetime={total}",
            health = self.health.label(),
            ls30 = self.learning_sessions_30d,
            avg = self.avg_daily_learning_minutes_7d,
            n_goals = self.active_goals.iter().filter(|g| !g.completed).count(),
            stalled = stalled.join("; "),
            goals = goal_summary.join("; "),
            total = self.total_events,
        )
    }
}

impl Default for GrowthArc {
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

    const T0: u64 = 4_000_000_000_u64;
    const ONE_HOUR: u64 = 3_600_000;

    #[test]
    fn test_day_zero_stable() {
        let arc = GrowthArc::new();
        assert_eq!(arc.health, ArcHealth::Stable);
        assert_eq!(arc.score(T0), 0.5);
    }

    #[test]
    fn test_no_trigger_day_zero() {
        let arc = GrowthArc::new();
        assert!(arc.check_proactive_trigger(T0).is_none());
    }

    #[test]
    fn test_learning_session_increases_stats() {
        let mut arc = GrowthArc::new();
        arc.record_event(
            GrowthEvent::LearningSession {
                topic: "Rust programming".into(),
                duration_minutes: 60,
            },
            T0,
        );
        assert_eq!(arc.learning_sessions_30d, 1);
        assert_eq!(arc.total_events, 1);
    }

    #[test]
    fn test_score_with_consistent_learning() {
        let mut arc = GrowthArc::new();
        // 7 days of 30 min learning.
        for d in 0..7u64 {
            arc.record_event(
                GrowthEvent::LearningSession {
                    topic: "topic_a".into(),
                    duration_minutes: 30,
                },
                T0 + d * ONE_DAY_MS,
            );
        }
        let s = arc.score(T0 + 7 * ONE_DAY_MS);
        assert!(s > 0.5, "consistent learning should score above neutral, got {s}");
    }

    #[test]
    fn test_goal_creation_and_progress() {
        let mut arc = GrowthArc::new();
        arc.record_event(
            GrowthEvent::GoalCreated {
                goal_id: "goal_1".into(),
                description: "Learn Rust async".into(),
                target_deadline_ms: Some(T0 + 30 * ONE_DAY_MS),
            },
            T0,
        );
        assert_eq!(arc.active_goals.len(), 1);
        assert_eq!(arc.active_goals[0].progress_pct, 0.0);

        arc.record_event(
            GrowthEvent::GoalProgress {
                goal_id: "goal_1".into(),
                progress_pct: 50.0,
                note: None,
            },
            T0 + ONE_HOUR,
        );
        assert!((arc.active_goals[0].progress_pct - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_goal_completion() {
        let mut arc = GrowthArc::new();
        arc.record_event(
            GrowthEvent::GoalCreated {
                goal_id: "g2".into(),
                description: "Write 10 tests".into(),
                target_deadline_ms: None,
            },
            T0,
        );
        arc.record_event(
            GrowthEvent::GoalProgress {
                goal_id: "g2".into(),
                progress_pct: 100.0,
                note: None,
            },
            T0 + ONE_HOUR,
        );
        assert!(arc.active_goals[0].completed);
    }

    #[test]
    fn test_stalled_goal_detected() {
        let mut arc = GrowthArc::new();
        arc.record_event(
            GrowthEvent::GoalCreated {
                goal_id: "g3".into(),
                description: "Finish course".into(),
                target_deadline_ms: None,
            },
            T0,
        );
        // No progress for 15 days → stalled.
        let stalled = arc.at_risk_goals(T0 + 15 * ONE_DAY_MS);
        assert_eq!(stalled.len(), 1, "goal should be stalled after 15 days");
    }

    #[test]
    fn test_skill_practice_counted_as_learning() {
        let mut arc = GrowthArc::new();
        arc.record_event(
            GrowthEvent::SkillPractice {
                skill: "piano".into(),
                duration_minutes: 20,
            },
            T0,
        );
        assert_eq!(arc.learning_sessions_30d, 1);
    }

    #[test]
    fn test_breadth_bonus() {
        let mut arc = GrowthArc::new();
        // Record 5 distinct topics.
        for (i, topic) in ["rust", "piano", "spanish", "cooking", "history"]
            .iter()
            .enumerate()
        {
            arc.record_event(
                GrowthEvent::LearningSession {
                    topic: topic.to_string(),
                    duration_minutes: 30,
                },
                T0 + i as u64 * ONE_HOUR,
            );
        }
        let s = arc.score(T0 + 10 * ONE_HOUR);
        // With 5 distinct topics (max breadth), breadth component = 1.0.
        assert!(s > 0.3, "breadth should contribute positively, got {s}");
    }

    #[test]
    fn test_trigger_cooldown() {
        let mut arc = GrowthArc::new();
        for i in 0..MIN_EVENTS_FOR_TRIGGER {
            arc.record_event(
                GrowthEvent::LearningSession {
                    topic: "test".into(),
                    duration_minutes: 5,
                },
                T0 + i as u64 * ONE_HOUR,
            );
        }
        arc.health = ArcHealth::NeedsAttention;

        let t1 = arc.check_proactive_trigger(T0 + 5 * ONE_HOUR);
        assert!(t1.is_some());
        arc.mark_trigger_fired(T0 + 5 * ONE_HOUR);

        assert!(arc.check_proactive_trigger(T0 + 10 * ONE_HOUR).is_none());
        assert!(arc
            .check_proactive_trigger(T0 + 5 * ONE_HOUR + ONE_DAY_MS)
            .is_some());
    }

    #[test]
    fn test_llm_context_structure() {
        let mut arc = GrowthArc::new();
        arc.record_event(
            GrowthEvent::LearningSession {
                topic: "test_topic".into(),
                duration_minutes: 30,
            },
            T0,
        );
        let ctx = arc.to_llm_context(T0 + ONE_HOUR);
        assert!(ctx.contains("[growth_arc]"));
        assert!(ctx.contains("health="));
        assert!(ctx.contains("learning_sessions_30d="));
    }

    #[test]
    fn test_max_goals_capacity() {
        let mut arc = GrowthArc::new();
        for i in 0..MAX_GOALS {
            arc.record_event(
                GrowthEvent::GoalCreated {
                    goal_id: format!("goal_{i}"),
                    description: format!("Goal {i}"),
                    target_deadline_ms: None,
                },
                T0 + i as u64 * ONE_HOUR,
            );
        }
        assert_eq!(arc.active_goals.len(), MAX_GOALS);

        // This one should be silently dropped (at capacity).
        arc.record_event(
            GrowthEvent::GoalCreated {
                goal_id: "overflow".into(),
                description: "overflow goal".into(),
                target_deadline_ms: None,
            },
            T0 + (MAX_GOALS + 1) as u64 * ONE_HOUR,
        );
        assert_eq!(arc.active_goals.len(), MAX_GOALS, "should not exceed max");
    }
}
