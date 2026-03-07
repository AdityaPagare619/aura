use serde::{Deserialize, Serialize};

use crate::actions::ActionType;

/// A goal that AURA is tracking for the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: u64,
    pub description: String,
    pub priority: GoalPriority,
    pub status: GoalStatus,
    pub steps: Vec<GoalStep>,
    pub created_ms: u64,
    pub deadline_ms: Option<u64>,
    pub parent_goal: Option<u64>,
    pub source: GoalSource,
}

/// Priority level for goals.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum GoalPriority {
    Critical,
    High,
    Medium,
    Low,
    Background,
}

/// Current status of a goal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GoalStatus {
    Pending,
    Active,
    Blocked(String),
    Completed,
    Failed(String),
    Cancelled,
}

/// A single step within a goal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalStep {
    pub index: u32,
    pub description: String,
    pub action: Option<ActionType>,
    pub status: StepStatus,
    pub attempts: u8,
    pub max_attempts: u8,
}

impl Default for GoalStep {
    fn default() -> Self {
        Self {
            index: 0,
            description: String::new(),
            action: None,
            status: StepStatus::Pending,
            attempts: 0,
            max_attempts: 3,
        }
    }
}

/// Status of an individual goal step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    InProgress,
    Succeeded,
    Failed(String),
    Skipped,
}

/// How this goal originated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GoalSource {
    /// User explicitly stated the goal.
    UserExplicit,
    /// AURA proactively suggested this.
    ProactiveSuggestion,
    /// Scheduled via cron/alarm.
    CronScheduled,
    /// Triggered by a notification.
    NotificationTriggered,
    /// Decomposed from parent goal (contains parent goal id).
    GoalDecomposition(u64),
}

impl Goal {
    /// Check if this goal has reached a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            GoalStatus::Completed | GoalStatus::Failed(_) | GoalStatus::Cancelled
        )
    }

    /// Fraction of steps completed (0.0–1.0).
    #[must_use]
    pub fn progress(&self) -> f32 {
        if self.steps.is_empty() {
            return 0.0;
        }
        let done = self
            .steps
            .iter()
            .filter(|s| matches!(s.status, StepStatus::Succeeded | StepStatus::Skipped))
            .count();
        done as f32 / self.steps.len() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_goal_progress_tracking() {
        let goal = Goal {
            id: 1,
            description: "Order coffee".to_string(),
            priority: GoalPriority::Medium,
            status: GoalStatus::Active,
            steps: vec![
                GoalStep {
                    index: 0,
                    description: "Open Starbucks app".to_string(),
                    action: Some(ActionType::OpenApp {
                        package: "com.starbucks".to_string(),
                    }),
                    status: StepStatus::Succeeded,
                    attempts: 1,
                    max_attempts: 3,
                },
                GoalStep {
                    index: 1,
                    description: "Tap order button".to_string(),
                    action: Some(ActionType::Tap { x: 540, y: 1200 }),
                    status: StepStatus::InProgress,
                    attempts: 0,
                    max_attempts: 3,
                },
                GoalStep {
                    index: 2,
                    description: "Confirm order".to_string(),
                    action: None,
                    status: StepStatus::Pending,
                    attempts: 0,
                    max_attempts: 3,
                },
            ],
            created_ms: 1_700_000_000_000,
            deadline_ms: None,
            parent_goal: None,
            source: GoalSource::UserExplicit,
        };

        // 1 of 3 steps done
        let progress = goal.progress();
        assert!((progress - 1.0 / 3.0).abs() < 0.01);
        assert!(!goal.is_terminal());
    }

    #[test]
    fn test_goal_terminal_states() {
        let mut goal = Goal {
            id: 2,
            description: "Test".to_string(),
            priority: GoalPriority::Low,
            status: GoalStatus::Completed,
            steps: vec![],
            created_ms: 0,
            deadline_ms: None,
            parent_goal: None,
            source: GoalSource::ProactiveSuggestion,
        };
        assert!(goal.is_terminal());

        goal.status = GoalStatus::Failed("timeout".to_string());
        assert!(goal.is_terminal());

        goal.status = GoalStatus::Cancelled;
        assert!(goal.is_terminal());

        goal.status = GoalStatus::Active;
        assert!(!goal.is_terminal());

        goal.status = GoalStatus::Blocked("waiting".to_string());
        assert!(!goal.is_terminal());
    }

    #[test]
    fn test_goal_step_defaults() {
        let step = GoalStep::default();
        assert_eq!(step.max_attempts, 3);
        assert_eq!(step.attempts, 0);
        assert_eq!(step.status, StepStatus::Pending);
        assert!(step.action.is_none());
    }

    #[test]
    fn test_goal_source_variants() {
        let decomp = GoalSource::GoalDecomposition(42);
        match decomp {
            GoalSource::GoalDecomposition(parent_id) => assert_eq!(parent_id, 42),
            _ => panic!("expected GoalDecomposition"),
        }
    }
}
