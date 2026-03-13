//! Goal lifecycle tracker — monitors goals from creation to completion.
//!
//! Implements a strict state machine for goal status transitions:
//!
//! ```text
//! Pending → Active → Completed
//!                  → Paused → Active (resume)
//!                  → Blocked → Active (unblock)
//!                  → Failed
//!                  → Cancelled
//! Pending → Cancelled
//! ```
//!
//! Features:
//! - Progress tracking per-step with percentage completion
//! - Blocked goals (waiting on dependencies)
//! - Timeout detection (deadline exceeded)
//! - Retry logic with exponential backoff
//! - Completion history for learning which goals succeed/fail and why

use aura_types::errors::GoalError;
use aura_types::goals::{Goal, GoalStatus};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use super::{BoundedMap, BoundedVec, CircularBuffer};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of goals being tracked simultaneously.
const MAX_TRACKED_GOALS: usize = 256;

/// Maximum number of completed goals retained in history.
const MAX_HISTORY_SIZE: usize = 1024;

/// Default maximum retries per goal.
const DEFAULT_MAX_RETRIES: u8 = 3;

/// Base backoff delay for retries (milliseconds).
const BASE_BACKOFF_MS: u64 = 1_000;

/// Maximum backoff delay (milliseconds).
const MAX_BACKOFF_MS: u64 = 60_000;

/// Maximum number of sub-goals tracked per parent goal.
const MAX_SUBGOALS_PER_PARENT: usize = 32;

/// Maximum number of progress snapshots retained for prediction.
const MAX_PROGRESS_SNAPSHOTS: usize = 64;

/// Maximum number of milestones per goal.
const MAX_MILESTONES_PER_GOAL: usize = 16;

/// Maximum number of dependency IDs a single goal may be blocked by.
const MAX_BLOCKED_BY: usize = 64;

/// Maximum number of error log entries per goal.
const MAX_ERROR_LOG: usize = 32;

/// Minimum progress delta per window to avoid stall detection.
const STALL_THRESHOLD: f32 = 0.01;

/// Default stall detection window (milliseconds) — 2 minutes.
const DEFAULT_STALL_WINDOW_MS: u64 = 120_000;

/// Regression threshold — progress must drop by at least this to trigger.
const REGRESSION_THRESHOLD: f32 = 0.02;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Extended tracking information for an active/tracked goal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedGoal {
    /// The underlying goal.
    pub goal: Goal,
    /// Current progress (0.0–1.0).
    pub progress: f32,
    /// Number of steps completed.
    pub steps_completed: usize,
    /// Total number of steps.
    pub steps_total: usize,
    /// When this goal started being actively worked on (epoch ms).
    pub started_at_ms: Option<u64>,
    /// Deadline for completion (epoch ms).
    pub deadline_ms: Option<u64>,
    /// Goal IDs that this goal is blocked by (bounded to prevent unbounded growth).
    pub blocked_by: BoundedVec<u64, MAX_BLOCKED_BY>,
    /// Number of retry attempts made.
    pub retry_count: u8,
    /// Maximum allowed retries.
    pub max_retries: u8,
    /// When the last retry was attempted (epoch ms).
    pub last_retry_at_ms: Option<u64>,
    /// When this goal was last paused (epoch ms).
    pub paused_at_ms: Option<u64>,
    /// Accumulated error messages from failed attempts (bounded to prevent unbounded growth).
    pub error_log: BoundedVec<String, MAX_ERROR_LOG>,
}

/// A completed goal stored in history for learning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedGoal {
    /// Goal ID.
    pub goal_id: u64,
    /// Goal description.
    pub description: String,
    /// Terminal status (Completed, Failed, or Cancelled).
    pub terminal_status: GoalStatus,
    /// Final progress when the goal reached terminal state.
    pub final_progress: f32,
    /// Total time from start to finish (ms).
    pub duration_ms: u64,
    /// Number of retries that were used.
    pub retries_used: u8,
    /// When this goal completed (epoch ms).
    pub completed_at_ms: u64,
    /// Error messages accumulated during execution.
    pub errors: Vec<String>,
}

/// Summary statistics from the tracker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackerStats {
    /// Total goals currently being tracked.
    pub total_tracked: usize,
    /// Goals in Pending state.
    pub pending_count: usize,
    /// Goals in Active state.
    pub active_count: usize,
    /// Goals in Blocked state.
    pub blocked_count: usize,
    /// Goals in Paused state (mapped from Blocked with "paused" reason).
    pub paused_count: usize,
    /// Total goals in history.
    pub history_size: usize,
    /// Success rate from history (0.0–1.0).
    pub success_rate: f32,
}

/// A progress snapshot for ETA prediction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressSnapshot {
    /// Progress value at this point (0.0–1.0).
    pub progress: f32,
    /// Timestamp when this snapshot was taken (epoch ms).
    pub timestamp_ms: u64,
}

/// Progress prediction result with estimated time of arrival.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressPrediction {
    /// Current progress (0.0–1.0).
    pub current_progress: f32,
    /// Estimated time to completion (milliseconds from now). `None` if
    /// insufficient data or stalled.
    pub eta_ms: Option<u64>,
    /// Average progress rate (progress units per millisecond).
    pub rate_per_ms: Option<f64>,
    /// Whether the goal appears stalled.
    pub is_stalled: bool,
    /// Whether regression has been detected.
    pub has_regression: bool,
    /// Confidence in the prediction (0.0–1.0). Higher with more data points.
    pub confidence: f32,
}

/// A milestone within a goal's lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Milestone {
    /// Human-readable label (e.g., "50% complete", "Data fetched").
    pub label: String,
    /// Progress threshold that triggers this milestone (0.0–1.0).
    pub threshold: f32,
    /// Whether this milestone has been reached.
    pub reached: bool,
    /// Timestamp when reached (epoch ms). `None` if not yet reached.
    pub reached_at_ms: Option<u64>,
}

/// A detected stall event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StallEvent {
    /// Goal ID that stalled.
    pub goal_id: u64,
    /// Progress at the time of stall detection.
    pub progress_at_stall: f32,
    /// How long the goal has been stalled (ms).
    pub stall_duration_ms: u64,
    /// Timestamp when the stall was detected (epoch ms).
    pub detected_at_ms: u64,
}

/// A detected regression event (progress went backward).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionEvent {
    /// Goal ID where regression occurred.
    pub goal_id: u64,
    /// Progress before regression.
    pub previous_progress: f32,
    /// Progress after regression.
    pub current_progress: f32,
    /// Timestamp when detected (epoch ms).
    pub detected_at_ms: u64,
}

/// Visualization-friendly progress data for a single goal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressVisualization {
    /// Goal ID.
    pub goal_id: u64,
    /// Goal description.
    pub description: String,
    /// Current progress (0.0–1.0).
    pub progress: f32,
    /// Steps completed / total.
    pub steps_completed: usize,
    pub steps_total: usize,
    /// Progress history (time series of snapshots).
    pub history: Vec<ProgressSnapshot>,
    /// Milestones and their status.
    pub milestones: Vec<Milestone>,
    /// Predicted ETA (if available).
    pub prediction: Option<ProgressPrediction>,
    /// Sub-goal progress bars (if this is a parent goal).
    pub sub_goal_progress: Vec<SubGoalProgress>,
}

/// Progress info for a sub-goal, used in aggregation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubGoalProgress {
    /// Sub-goal ID.
    pub goal_id: u64,
    /// Sub-goal description.
    pub description: String,
    /// Sub-goal progress (0.0–1.0).
    pub progress: f32,
    /// Weight of this sub-goal in the aggregate (0.0–1.0).
    pub weight: f32,
}

/// Extended per-goal tracking data for the new features.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalProgressMeta {
    /// Progress snapshots for ETA prediction.
    pub snapshots: CircularBuffer<ProgressSnapshot, MAX_PROGRESS_SNAPSHOTS>,
    /// Milestones for this goal.
    pub milestones: BoundedVec<Milestone, MAX_MILESTONES_PER_GOAL>,
    /// Sub-goal IDs with their weights (for parent goals).
    pub sub_goal_weights: BoundedVec<(u64, f32), MAX_SUBGOALS_PER_PARENT>,
    /// The highest progress value ever recorded (for regression detection).
    pub peak_progress: f32,
    /// Custom stall window override (ms). Uses DEFAULT_STALL_WINDOW_MS if None.
    pub stall_window_ms: Option<u64>,
}

/// The goal tracker — manages the lifecycle of all goals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalTracker {
    /// Currently tracked goals, keyed by goal ID.
    goals: BoundedMap<u64, TrackedGoal, MAX_TRACKED_GOALS>,
    /// History of completed goals (circular buffer, oldest evicted first).
    history: CircularBuffer<CompletedGoal, MAX_HISTORY_SIZE>,
    /// Extended progress metadata per goal (snapshots, milestones, sub-goals).
    progress_meta: BoundedMap<u64, GoalProgressMeta, MAX_TRACKED_GOALS>,
    /// Recent stall events for monitoring.
    stall_events: CircularBuffer<StallEvent, 64>,
    /// Recent regression events for monitoring.
    regression_events: CircularBuffer<RegressionEvent, 64>,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl GoalTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self {
            goals: BoundedMap::new(),
            history: CircularBuffer::new(),
            progress_meta: BoundedMap::new(),
            stall_events: CircularBuffer::new(),
            regression_events: CircularBuffer::new(),
        }
    }

    /// Start tracking a new goal. Returns error if at capacity or duplicate ID.
    #[instrument(skip(self), fields(goal_id = goal.id, description = %goal.description))]
    pub fn track(&mut self, goal: Goal) -> Result<(), GoalError> {
        let goal_id = goal.id;

        if self.goals.contains_key(&goal_id) {
            return Err(GoalError::AlreadyExists(goal_id));
        }

        let steps_total = goal.steps.len();
        let deadline_ms = goal.deadline_ms;

        let tracked = TrackedGoal {
            goal,
            progress: 0.0,
            steps_completed: 0,
            steps_total,
            started_at_ms: None,
            deadline_ms,
            blocked_by: BoundedVec::new(),
            retry_count: 0,
            max_retries: DEFAULT_MAX_RETRIES,
            last_retry_at_ms: None,
            paused_at_ms: None,
            error_log: BoundedVec::new(),
        };

        self.goals
            .try_insert(goal_id, tracked)
            .map_err(|_| GoalError::CapacityExceeded {
                max: MAX_TRACKED_GOALS,
            })?;

        // Initialize progress metadata for the new goal.
        let meta = GoalProgressMeta {
            snapshots: CircularBuffer::new(),
            milestones: BoundedVec::new(),
            sub_goal_weights: BoundedVec::new(),
            peak_progress: 0.0,
            stall_window_ms: None,
        };
        // Ignore capacity error here — if goals fits, meta should too (same CAP).
        let _ = self.progress_meta.try_insert(goal_id, meta);

        tracing::info!(goal_id, "goal tracking started");
        Ok(())
    }

    /// Transition a goal to Active state.
    #[instrument(skip(self))]
    pub fn activate(&mut self, goal_id: u64, now_ms: u64) -> Result<(), GoalError> {
        let tracked = self
            .goals
            .get_mut(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        Self::validate_transition(&tracked.goal.status, &GoalStatus::Active)?;

        tracked.goal.status = GoalStatus::Active;
        tracked.started_at_ms = Some(now_ms);
        tracked.paused_at_ms = None;

        tracing::info!(goal_id, "goal activated");
        Ok(())
    }

    /// Transition a goal to Blocked state.
    #[instrument(skip(self))]
    pub fn block(
        &mut self,
        goal_id: u64,
        reason: String,
        blocked_by: Vec<u64>,
    ) -> Result<(), GoalError> {
        let tracked = self
            .goals
            .get_mut(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        let target = GoalStatus::Blocked(reason.clone());
        Self::validate_transition(&tracked.goal.status, &target)?;

        tracked.goal.status = target;

        // Replace blocked_by list (bounded — excess dependencies silently dropped).
        tracked.blocked_by = BoundedVec::new();
        for dep in blocked_by {
            let _ = tracked.blocked_by.try_push(dep); // cap enforced; excess silently dropped
        }

        tracing::info!(goal_id, reason = %reason, "goal blocked");
        Ok(())
    }

    /// Pause a goal (special case of Blocked with "paused" reason).
    #[instrument(skip(self))]
    pub fn pause(&mut self, goal_id: u64, now_ms: u64) -> Result<(), GoalError> {
        let tracked = self
            .goals
            .get_mut(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        let target = GoalStatus::Blocked("paused".to_string());
        Self::validate_transition(&tracked.goal.status, &target)?;

        tracked.goal.status = target;
        tracked.paused_at_ms = Some(now_ms);

        tracing::info!(goal_id, "goal paused");
        Ok(())
    }

    /// Resume a paused or blocked goal to Active state.
    #[instrument(skip(self))]
    pub fn resume(&mut self, goal_id: u64, now_ms: u64) -> Result<(), GoalError> {
        let tracked = self
            .goals
            .get_mut(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        if !matches!(tracked.goal.status, GoalStatus::Blocked(_)) {
            return Err(GoalError::InvalidTransition {
                from: format!("{:?}", tracked.goal.status),
                to: "Active".to_string(),
            });
        }

        tracked.goal.status = GoalStatus::Active;
        tracked.blocked_by = BoundedVec::new();
        tracked.paused_at_ms = None;

        tracing::info!(goal_id, "goal resumed");
        Ok(())
    }

    /// Mark a goal as completed.
    #[instrument(skip(self))]
    pub fn complete(&mut self, goal_id: u64, now_ms: u64) -> Result<(), GoalError> {
        let tracked = self
            .goals
            .get_mut(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        Self::validate_transition(&tracked.goal.status, &GoalStatus::Completed)?;

        tracked.goal.status = GoalStatus::Completed;
        tracked.progress = 1.0;

        let completed = Self::to_completed_record(tracked, now_ms);
        self.history.push(completed);

        let _removed = self.goals.remove(&goal_id);
        let _meta_removed = self.progress_meta.remove(&goal_id);

        tracing::info!(goal_id, "goal completed");
        Ok(())
    }

    /// Mark a goal as failed.
    #[instrument(skip(self))]
    pub fn fail(&mut self, goal_id: u64, reason: String, now_ms: u64) -> Result<(), GoalError> {
        let tracked = self
            .goals
            .get_mut(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        let target = GoalStatus::Failed(reason.clone());
        Self::validate_transition(&tracked.goal.status, &target)?;

        tracked.goal.status = target;
        let _ = tracked.error_log.try_push(reason); // cap enforced; oldest errors evicted on overflow is not needed — excess silently dropped

        let completed = Self::to_completed_record(tracked, now_ms);
        self.history.push(completed);

        let _removed = self.goals.remove(&goal_id);
        let _meta_removed = self.progress_meta.remove(&goal_id);

        tracing::info!(goal_id, "goal failed");
        Ok(())
    }

    /// Cancel a goal.
    #[instrument(skip(self))]
    pub fn cancel(&mut self, goal_id: u64, now_ms: u64) -> Result<(), GoalError> {
        let tracked = self
            .goals
            .get_mut(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        Self::validate_transition(&tracked.goal.status, &GoalStatus::Cancelled)?;

        tracked.goal.status = GoalStatus::Cancelled;

        let completed = Self::to_completed_record(tracked, now_ms);
        self.history.push(completed);

        let _removed = self.goals.remove(&goal_id);
        let _meta_removed = self.progress_meta.remove(&goal_id);

        tracing::info!(goal_id, "goal cancelled");
        Ok(())
    }

    /// Update the progress of a goal based on completed steps.
    #[instrument(skip(self))]
    pub fn update_progress(
        &mut self,
        goal_id: u64,
        steps_completed: usize,
        steps_total: usize,
    ) -> Result<f32, GoalError> {
        let tracked = self
            .goals
            .get_mut(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        tracked.steps_completed = steps_completed;
        tracked.steps_total = steps_total;
        tracked.progress = if steps_total == 0 {
            0.0
        } else {
            (steps_completed as f32 / steps_total as f32).clamp(0.0, 1.0)
        };

        tracing::debug!(
            goal_id,
            progress = tracked.progress,
            steps = format!("{}/{}", steps_completed, steps_total),
            "progress updated"
        );

        Ok(tracked.progress)
    }

    /// Attempt a retry for a failed step. Returns the backoff delay (ms)
    /// if retries are still available, or an error if exhausted.
    #[instrument(skip(self))]
    pub fn attempt_retry(&mut self, goal_id: u64, now_ms: u64) -> Result<u64, GoalError> {
        let tracked = self
            .goals
            .get_mut(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        if tracked.retry_count >= tracked.max_retries {
            return Err(GoalError::RetriesExhausted {
                goal_id,
                attempts: tracked.retry_count,
            });
        }

        tracked.retry_count += 1;
        tracked.last_retry_at_ms = Some(now_ms);

        // Exponential backoff: base * 2^(retry_count - 1), capped at max.
        let backoff =
            (BASE_BACKOFF_MS * (1u64 << (tracked.retry_count as u64 - 1))).min(MAX_BACKOFF_MS);

        tracing::debug!(
            goal_id,
            retry = tracked.retry_count,
            max_retries = tracked.max_retries,
            backoff_ms = backoff,
            "retry scheduled"
        );

        Ok(backoff)
    }

    /// Check all tracked goals for deadline violations.
    ///
    /// Returns IDs of goals that have exceeded their deadlines.
    #[instrument(skip(self))]
    pub fn check_deadlines(&self, now_ms: u64) -> Vec<u64> {
        let mut overdue = Vec::new();
        for (_, tracked) in self.goals.iter() {
            if let Some(deadline) = tracked.deadline_ms {
                if now_ms > deadline && !tracked.goal.is_terminal() {
                    overdue.push(tracked.goal.id);
                }
            }
        }

        if !overdue.is_empty() {
            tracing::warn!(
                overdue_count = overdue.len(),
                "deadline violations detected"
            );
        }
        overdue
    }

    /// Check if all blocking dependencies for a goal are resolved.
    ///
    /// A dependency is resolved if the blocking goal is no longer tracked
    /// (i.e., it completed or was removed).
    #[instrument(skip(self))]
    pub fn check_unblock(&self, goal_id: u64) -> Result<bool, GoalError> {
        let tracked = self
            .goals
            .get(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        if tracked.blocked_by.is_empty() {
            return Ok(true);
        }

        // Goal is unblockable if none of the blocking goals are still tracked.
        let all_resolved = tracked
            .blocked_by
            .iter()
            .all(|dep_id| !self.goals.contains_key(dep_id));

        Ok(all_resolved)
    }

    /// Get a reference to a tracked goal by ID.
    pub fn get(&self, goal_id: u64) -> Option<&TrackedGoal> {
        self.goals.get(&goal_id)
    }

    /// Get a mutable reference to a tracked goal by ID.
    pub fn get_mut(&mut self, goal_id: u64) -> Option<&mut TrackedGoal> {
        self.goals.get_mut(&goal_id)
    }

    /// Get summary statistics.
    #[must_use]
    pub fn stats(&self) -> TrackerStats {
        let mut pending = 0usize;
        let mut active = 0usize;
        let mut blocked = 0usize;
        let mut paused = 0usize;

        for (_, tracked) in self.goals.iter() {
            match &tracked.goal.status {
                GoalStatus::Pending => pending += 1,
                GoalStatus::Active => active += 1,
                GoalStatus::Blocked(reason) => {
                    if reason == "paused" {
                        paused += 1;
                    } else {
                        blocked += 1;
                    }
                }
                _ => {} // Terminal states are in history, not tracked.
            }
        }

        let history_len = self.history.len();
        let success_count = self
            .history
            .iter()
            .filter(|cg| matches!(cg.terminal_status, GoalStatus::Completed))
            .count();
        let success_rate = if history_len == 0 {
            0.0
        } else {
            success_count as f32 / history_len as f32
        };

        TrackerStats {
            total_tracked: self.goals.len(),
            pending_count: pending,
            active_count: active,
            blocked_count: blocked,
            paused_count: paused,
            history_size: history_len,
            success_rate,
        }
    }

    /// Number of currently tracked goals.
    #[must_use]
    pub fn tracked_count(&self) -> usize {
        self.goals.len()
    }

    /// Number of completed goals in history.
    #[must_use]
    pub fn history_count(&self) -> usize {
        self.history.len()
    }

    // -----------------------------------------------------------------------
    // Sub-goal progress aggregation
    // -----------------------------------------------------------------------

    /// Register a sub-goal with a weight under a parent goal.
    ///
    /// Weights are relative — they will be normalized when computing
    /// the aggregate. For example, weights `[1.0, 2.0, 1.0]` mean
    /// the second sub-goal counts for 50% of aggregate progress.
    #[instrument(skip(self))]
    pub fn register_sub_goal(
        &mut self,
        parent_id: u64,
        sub_goal_id: u64,
        weight: f32,
    ) -> Result<(), GoalError> {
        if !self.goals.contains_key(&parent_id) {
            return Err(GoalError::NotFound(parent_id));
        }
        if !self.goals.contains_key(&sub_goal_id) {
            return Err(GoalError::NotFound(sub_goal_id));
        }

        let meta = self
            .progress_meta
            .get_mut(&parent_id)
            .ok_or(GoalError::NotFound(parent_id))?;

        // Avoid duplicate registration.
        let already = meta
            .sub_goal_weights
            .iter()
            .any(|(id, _)| *id == sub_goal_id);
        if already {
            return Ok(());
        }

        meta.sub_goal_weights
            .try_push((sub_goal_id, weight.max(0.0)))
            .map_err(|_| GoalError::CapacityExceeded {
                max: MAX_SUBGOALS_PER_PARENT,
            })?;

        tracing::debug!(parent_id, sub_goal_id, weight, "sub-goal registered");
        Ok(())
    }

    /// Compute the aggregated progress of a parent goal based on its
    /// weighted sub-goals. Returns the weighted average (0.0–1.0).
    ///
    /// If a sub-goal is no longer tracked (e.g., completed), its progress
    /// is taken from history if available, otherwise assumed 1.0 if completed
    /// or 0.0 if missing.
    #[instrument(skip(self))]
    pub fn aggregate_sub_goal_progress(&self, parent_id: u64) -> Result<f32, GoalError> {
        let meta = self
            .progress_meta
            .get(&parent_id)
            .ok_or(GoalError::NotFound(parent_id))?;

        if meta.sub_goal_weights.is_empty() {
            // No sub-goals registered; fall back to direct progress.
            return self
                .goals
                .get(&parent_id)
                .map(|t| t.progress)
                .ok_or(GoalError::NotFound(parent_id));
        }

        let total_weight: f32 = meta.sub_goal_weights.iter().map(|(_, w)| *w).sum();
        if total_weight <= f32::EPSILON {
            return Ok(0.0);
        }

        let mut weighted_sum: f32 = 0.0;
        for (sub_id, weight) in meta.sub_goal_weights.iter() {
            let sub_progress = if let Some(tracked) = self.goals.get(sub_id) {
                tracked.progress
            } else {
                // Check history for completed sub-goals.
                self.history
                    .iter()
                    .find(|cg| cg.goal_id == *sub_id)
                    .map(|cg| cg.final_progress)
                    .unwrap_or(0.0)
            };
            weighted_sum += sub_progress * weight;
        }

        Ok((weighted_sum / total_weight).clamp(0.0, 1.0))
    }

    /// Get sub-goal progress details for visualization.
    pub fn sub_goal_progress_details(
        &self,
        parent_id: u64,
    ) -> Result<Vec<SubGoalProgress>, GoalError> {
        let meta = self
            .progress_meta
            .get(&parent_id)
            .ok_or(GoalError::NotFound(parent_id))?;

        let total_weight: f32 = meta.sub_goal_weights.iter().map(|(_, w)| *w).sum();
        let norm = if total_weight > f32::EPSILON {
            total_weight
        } else {
            1.0
        };

        let mut details = Vec::with_capacity(meta.sub_goal_weights.len());
        for (sub_id, weight) in meta.sub_goal_weights.iter() {
            let (description, progress) = if let Some(tracked) = self.goals.get(sub_id) {
                (tracked.goal.description.clone(), tracked.progress)
            } else {
                let from_history = self.history.iter().find(|cg| cg.goal_id == *sub_id);
                match from_history {
                    Some(cg) => (cg.description.clone(), cg.final_progress),
                    None => (format!("Sub-goal {}", sub_id), 0.0),
                }
            };

            details.push(SubGoalProgress {
                goal_id: *sub_id,
                description,
                progress,
                weight: *weight / norm,
            });
        }

        Ok(details)
    }

    // -----------------------------------------------------------------------
    // Progress prediction (ETA)
    // -----------------------------------------------------------------------

    /// Record a progress snapshot for ETA prediction. Should be called
    /// whenever `update_progress` is called.
    #[instrument(skip(self))]
    pub fn record_progress_snapshot(
        &mut self,
        goal_id: u64,
        progress: f32,
        now_ms: u64,
    ) -> Result<(), GoalError> {
        let meta = self
            .progress_meta
            .get_mut(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        // Regression detection: if progress dropped below peak.
        if progress < meta.peak_progress - REGRESSION_THRESHOLD {
            tracing::warn!(
                goal_id,
                previous = meta.peak_progress,
                current = progress,
                "progress regression detected"
            );
            self.regression_events.push(RegressionEvent {
                goal_id,
                previous_progress: meta.peak_progress,
                current_progress: progress,
                detected_at_ms: now_ms,
            });
        }

        if progress > meta.peak_progress {
            meta.peak_progress = progress;
        }

        meta.snapshots.push(ProgressSnapshot {
            progress,
            timestamp_ms: now_ms,
        });

        // Check and update milestones.
        for milestone in meta.milestones.iter_mut() {
            if !milestone.reached && progress >= milestone.threshold {
                milestone.reached = true;
                milestone.reached_at_ms = Some(now_ms);
                tracing::info!(
                    goal_id,
                    milestone = %milestone.label,
                    "milestone reached"
                );
            }
        }

        Ok(())
    }

    /// Predict when a goal will complete, based on progress history.
    ///
    /// Uses linear regression over the recent progress snapshots to
    /// estimate the rate and extrapolate to 1.0.
    #[instrument(skip(self))]
    pub fn predict_progress(
        &self,
        goal_id: u64,
        now_ms: u64,
    ) -> Result<ProgressPrediction, GoalError> {
        let tracked = self
            .goals
            .get(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        let meta = self
            .progress_meta
            .get(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        let snapshots: Vec<&ProgressSnapshot> = meta.snapshots.iter().collect();
        let current_progress = tracked.progress;

        if snapshots.len() < 2 {
            return Ok(ProgressPrediction {
                current_progress,
                eta_ms: None,
                rate_per_ms: None,
                is_stalled: false,
                has_regression: current_progress < meta.peak_progress - REGRESSION_THRESHOLD,
                confidence: 0.0,
            });
        }

        // Linear regression: progress = m * time + b
        let n = snapshots.len() as f64;
        let sum_t: f64 = snapshots.iter().map(|s| s.timestamp_ms as f64).sum();
        let sum_p: f64 = snapshots.iter().map(|s| s.progress as f64).sum();
        let sum_tp: f64 = snapshots
            .iter()
            .map(|s| s.timestamp_ms as f64 * s.progress as f64)
            .sum();
        let sum_tt: f64 = snapshots
            .iter()
            .map(|s| (s.timestamp_ms as f64).powi(2))
            .sum();

        let denom = n * sum_tt - sum_t * sum_t;
        let rate_per_ms = if denom.abs() > f64::EPSILON {
            (n * sum_tp - sum_t * sum_p) / denom
        } else {
            0.0
        };

        // Stall detection: check if progress barely moved in the last window.
        let stall_window = meta.stall_window_ms.unwrap_or(DEFAULT_STALL_WINDOW_MS);
        let is_stalled = Self::detect_stall_from_snapshots(&snapshots, now_ms, stall_window);

        let has_regression = current_progress < meta.peak_progress - REGRESSION_THRESHOLD;

        // ETA: time for progress to go from current to 1.0 at current rate.
        let eta_ms = if rate_per_ms > f64::EPSILON && current_progress < 1.0 {
            let remaining = (1.0 - current_progress as f64) / rate_per_ms;
            if remaining > 0.0 && remaining < 1e12 {
                Some(remaining as u64)
            } else {
                None
            }
        } else {
            None
        };

        // Confidence increases with more data points (max out at ~20 snapshots).
        let confidence = ((snapshots.len() as f32) / 20.0).clamp(0.0, 1.0);

        Ok(ProgressPrediction {
            current_progress,
            eta_ms,
            rate_per_ms: if rate_per_ms.abs() > f64::EPSILON {
                Some(rate_per_ms)
            } else {
                None
            },
            is_stalled,
            has_regression,
            confidence,
        })
    }

    // -----------------------------------------------------------------------
    // Stall detection
    // -----------------------------------------------------------------------

    /// Detect stalled goals across all tracked goals.
    ///
    /// A goal is considered stalled if its progress has not increased by
    /// at least `STALL_THRESHOLD` within the stall detection window.
    #[instrument(skip(self))]
    pub fn detect_stalls(&mut self, now_ms: u64) -> Vec<StallEvent> {
        let mut stalls = Vec::new();

        // Collect goal IDs to check so we don't borrow self twice.
        let goal_ids: Vec<u64> = self.goals.iter().map(|(id, _)| *id).collect();

        for goal_id in goal_ids {
            let is_active = self
                .goals
                .get(&goal_id)
                .map(|t| matches!(t.goal.status, GoalStatus::Active))
                .unwrap_or(false);

            if !is_active {
                continue;
            }

            let (progress, stall_window) = {
                let meta = match self.progress_meta.get(&goal_id) {
                    Some(m) => m,
                    None => continue,
                };
                let tracked = match self.goals.get(&goal_id) {
                    Some(t) => t,
                    None => continue,
                };
                let stall_window = meta.stall_window_ms.unwrap_or(DEFAULT_STALL_WINDOW_MS);
                let snapshots: Vec<&ProgressSnapshot> = meta.snapshots.iter().collect();
                let stalled = Self::detect_stall_from_snapshots(&snapshots, now_ms, stall_window);
                if !stalled {
                    continue;
                }

                // Compute stall duration: how long since last meaningful progress.
                let last_meaningful = snapshots
                    .iter()
                    .rev()
                    .find(|s| {
                        s.progress
                            < snapshots.last().map(|l| l.progress).unwrap_or(0.0) - STALL_THRESHOLD
                    })
                    .map(|s| s.timestamp_ms);

                let stall_start = last_meaningful
                    .unwrap_or_else(|| snapshots.first().map(|s| s.timestamp_ms).unwrap_or(now_ms));

                (tracked.progress, now_ms.saturating_sub(stall_start))
            };

            let event = StallEvent {
                goal_id,
                progress_at_stall: progress,
                stall_duration_ms: stall_window,
                detected_at_ms: now_ms,
            };
            stalls.push(event.clone());
            self.stall_events.push(event);
        }

        if !stalls.is_empty() {
            tracing::warn!(stall_count = stalls.len(), "stalled goals detected");
        }

        stalls
    }

    // -----------------------------------------------------------------------
    // Milestone tracking
    // -----------------------------------------------------------------------

    /// Add a milestone to a tracked goal.
    #[instrument(skip(self))]
    pub fn add_milestone(
        &mut self,
        goal_id: u64,
        label: String,
        threshold: f32,
    ) -> Result<(), GoalError> {
        let meta = self
            .progress_meta
            .get_mut(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        let tracked = self
            .goals
            .get(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        let already_reached = tracked.progress >= threshold;

        meta.milestones
            .try_push(Milestone {
                label,
                threshold: threshold.clamp(0.0, 1.0),
                reached: already_reached,
                reached_at_ms: None,
            })
            .map_err(|_| GoalError::CapacityExceeded {
                max: MAX_MILESTONES_PER_GOAL,
            })?;

        Ok(())
    }

    /// Add standard milestones at 25%, 50%, 75%, and 100%.
    pub fn add_standard_milestones(&mut self, goal_id: u64) -> Result<(), GoalError> {
        self.add_milestone(goal_id, "25% complete".to_string(), 0.25)?;
        self.add_milestone(goal_id, "50% complete".to_string(), 0.50)?;
        self.add_milestone(goal_id, "75% complete".to_string(), 0.75)?;
        self.add_milestone(goal_id, "100% complete".to_string(), 1.00)?;
        Ok(())
    }

    /// Get milestones for a goal.
    pub fn get_milestones(&self, goal_id: u64) -> Result<Vec<Milestone>, GoalError> {
        let meta = self
            .progress_meta
            .get(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        Ok(meta.milestones.iter().cloned().collect())
    }

    // -----------------------------------------------------------------------
    // Regression detection
    // -----------------------------------------------------------------------

    /// Check a specific goal for progress regression.
    pub fn check_regression(&self, goal_id: u64) -> Result<bool, GoalError> {
        let tracked = self
            .goals
            .get(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        let meta = self
            .progress_meta
            .get(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        Ok(tracked.progress < meta.peak_progress - REGRESSION_THRESHOLD)
    }

    /// Get recent regression events.
    pub fn recent_regressions(&self) -> Vec<&RegressionEvent> {
        self.regression_events.iter().collect()
    }

    /// Get recent stall events.
    pub fn recent_stalls(&self) -> Vec<&StallEvent> {
        self.stall_events.iter().collect()
    }

    // -----------------------------------------------------------------------
    // Progress visualization
    // -----------------------------------------------------------------------

    /// Generate visualization-friendly progress data for a goal.
    pub fn progress_visualization(
        &self,
        goal_id: u64,
        now_ms: u64,
    ) -> Result<ProgressVisualization, GoalError> {
        let tracked = self
            .goals
            .get(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        let meta = self
            .progress_meta
            .get(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        let history: Vec<ProgressSnapshot> = meta.snapshots.iter().cloned().collect();
        let milestones: Vec<Milestone> = meta.milestones.iter().cloned().collect();
        let prediction = self.predict_progress(goal_id, now_ms).ok();
        let sub_goal_progress = self.sub_goal_progress_details(goal_id).unwrap_or_default();

        Ok(ProgressVisualization {
            goal_id,
            description: tracked.goal.description.clone(),
            progress: tracked.progress,
            steps_completed: tracked.steps_completed,
            steps_total: tracked.steps_total,
            history,
            milestones,
            prediction,
            sub_goal_progress,
        })
    }

    /// Set a custom stall detection window for a specific goal.
    pub fn set_stall_window(&mut self, goal_id: u64, window_ms: u64) -> Result<(), GoalError> {
        let meta = self
            .progress_meta
            .get_mut(&goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        meta.stall_window_ms = Some(window_ms);
        Ok(())
    }

    // -- Private helpers ----------------------------------------------------

    /// Validate that a status transition is allowed by the state machine.
    fn validate_transition(from: &GoalStatus, to: &GoalStatus) -> Result<(), GoalError> {
        let valid = match (from, to) {
            // From Pending.
            (GoalStatus::Pending, GoalStatus::Active) => true,
            (GoalStatus::Pending, GoalStatus::Cancelled) => true,

            // From Active.
            (GoalStatus::Active, GoalStatus::Completed) => true,
            (GoalStatus::Active, GoalStatus::Failed(_)) => true,
            (GoalStatus::Active, GoalStatus::Cancelled) => true,
            (GoalStatus::Active, GoalStatus::Blocked(_)) => true,

            // From Blocked/Paused.
            (GoalStatus::Blocked(_), GoalStatus::Active) => true,
            (GoalStatus::Blocked(_), GoalStatus::Cancelled) => true,
            (GoalStatus::Blocked(_), GoalStatus::Failed(_)) => true,

            _ => false,
        };

        if valid {
            Ok(())
        } else {
            Err(GoalError::InvalidTransition {
                from: format!("{:?}", from),
                to: format!("{:?}", to),
            })
        }
    }

    /// Convert a tracked goal into a completed goal record for history.
    fn to_completed_record(tracked: &TrackedGoal, now_ms: u64) -> CompletedGoal {
        let duration = tracked
            .started_at_ms
            .map(|start| now_ms.saturating_sub(start))
            .unwrap_or(0);

        CompletedGoal {
            goal_id: tracked.goal.id,
            description: tracked.goal.description.clone(),
            terminal_status: tracked.goal.status.clone(),
            final_progress: tracked.progress,
            duration_ms: duration,
            retries_used: tracked.retry_count,
            completed_at_ms: now_ms,
            errors: tracked.error_log.iter().cloned().collect(),
        }
    }

    /// Check if a sequence of progress snapshots indicates a stall.
    ///
    /// A stall is detected when all snapshots within the window show
    /// less than `STALL_THRESHOLD` total progress change.
    fn detect_stall_from_snapshots(
        snapshots: &[&ProgressSnapshot],
        now_ms: u64,
        window_ms: u64,
    ) -> bool {
        if snapshots.len() < 2 {
            return false;
        }

        let window_start = now_ms.saturating_sub(window_ms);

        // Get snapshots within the window.
        let in_window: Vec<&&ProgressSnapshot> = snapshots
            .iter()
            .filter(|s| s.timestamp_ms >= window_start)
            .collect();

        if in_window.len() < 2 {
            return false;
        }

        let min_progress = in_window
            .iter()
            .map(|s| s.progress)
            .fold(f32::INFINITY, f32::min);
        let max_progress = in_window
            .iter()
            .map(|s| s.progress)
            .fold(f32::NEG_INFINITY, f32::max);

        (max_progress - min_progress) < STALL_THRESHOLD
    }
}

impl Default for GoalTracker {
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
    use aura_types::goals::{GoalPriority, GoalSource};

    fn make_goal(id: u64) -> Goal {
        Goal {
            id,
            description: format!("Test goal {}", id),
            priority: GoalPriority::Medium,
            status: GoalStatus::Pending,
            steps: vec![],
            created_ms: 1_700_000_000_000,
            deadline_ms: None,
            parent_goal: None,
            source: GoalSource::UserExplicit,
        }
    }

    #[test]
    fn test_goal_lifecycle_happy_path() {
        let mut tracker = GoalTracker::new();
        let goal = make_goal(1);

        tracker.track(goal).expect("track should succeed");
        assert_eq!(tracker.tracked_count(), 1);

        tracker.activate(1, 100).expect("activate should succeed");
        let tracked = tracker.get(1).expect("should be tracked");
        assert!(matches!(tracked.goal.status, GoalStatus::Active));

        tracker.complete(1, 200).expect("complete should succeed");
        assert_eq!(tracker.tracked_count(), 0);
        assert_eq!(tracker.history_count(), 1);
    }

    #[test]
    fn test_invalid_transition_rejected() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();

        // Pending → Completed should fail (must go through Active first).
        let result = tracker.complete(1, 100);
        assert!(result.is_err());
        match result {
            Err(GoalError::InvalidTransition { from, to }) => {
                assert!(from.contains("Pending"));
                assert!(to.contains("Completed"));
            }
            other => panic!("expected InvalidTransition, got {:?}", other),
        }
    }

    #[test]
    fn test_block_and_resume() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();

        tracker
            .block(1, "waiting for data".to_string(), vec![42])
            .expect("block should succeed");

        let tracked = tracker.get(1).expect("should exist");
        assert!(matches!(tracked.goal.status, GoalStatus::Blocked(_)));
        assert_eq!(tracked.blocked_by.as_slice(), &[42u64]);

        tracker.resume(1, 200).expect("resume should succeed");
        let tracked = tracker.get(1).expect("should exist");
        assert!(matches!(tracked.goal.status, GoalStatus::Active));
        assert!(tracked.blocked_by.is_empty());
    }

    #[test]
    fn test_retry_with_exponential_backoff() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();

        let b1 = tracker.attempt_retry(1, 200).expect("retry 1");
        assert_eq!(b1, 1_000); // base * 2^0

        let b2 = tracker.attempt_retry(1, 300).expect("retry 2");
        assert_eq!(b2, 2_000); // base * 2^1

        let b3 = tracker.attempt_retry(1, 400).expect("retry 3");
        assert_eq!(b3, 4_000); // base * 2^2

        // 4th retry should fail (max_retries = 3).
        let result = tracker.attempt_retry(1, 500);
        assert!(result.is_err());
    }

    #[test]
    fn test_deadline_detection() {
        let mut tracker = GoalTracker::new();
        let mut goal = make_goal(1);
        goal.deadline_ms = Some(1_000);
        tracker.track(goal).ok();
        tracker.activate(1, 500).ok();

        // Before deadline — no violations.
        let overdue = tracker.check_deadlines(900);
        assert!(overdue.is_empty());

        // After deadline — violation detected.
        let overdue = tracker.check_deadlines(1_100);
        assert_eq!(overdue, vec![1]);
    }

    #[test]
    fn test_progress_update() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();

        let progress = tracker
            .update_progress(1, 3, 10)
            .expect("update should succeed");
        assert!((progress - 0.3).abs() < f32::EPSILON);

        let tracked = tracker.get(1).expect("should exist");
        assert_eq!(tracked.steps_completed, 3);
        assert_eq!(tracked.steps_total, 10);
    }

    #[test]
    fn test_duplicate_tracking_rejected() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        let result = tracker.track(make_goal(1));
        assert!(matches!(result, Err(GoalError::AlreadyExists(1))));
    }

    #[test]
    fn test_stats_computation() {
        let mut tracker = GoalTracker::new();

        tracker.track(make_goal(1)).ok(); // Pending
        tracker.track(make_goal(2)).ok();
        tracker.activate(2, 100).ok(); // Active
        tracker.track(make_goal(3)).ok();
        tracker.activate(3, 100).ok();
        tracker.block(3, "waiting".to_string(), vec![]).ok(); // Blocked
        tracker.track(make_goal(4)).ok();
        tracker.activate(4, 100).ok();
        tracker.pause(4, 150).ok(); // Paused (special Blocked)

        let stats = tracker.stats();
        assert_eq!(stats.total_tracked, 4);
        assert_eq!(stats.pending_count, 1);
        assert_eq!(stats.active_count, 1);
        assert_eq!(stats.blocked_count, 1);
        assert_eq!(stats.paused_count, 1);
    }

    #[test]
    fn test_dependency_unblock_check() {
        let mut tracker = GoalTracker::new();

        // Track two goals: goal 2 is blocked by goal 1.
        tracker.track(make_goal(1)).ok();
        tracker.track(make_goal(2)).ok();
        tracker.activate(1, 100).ok();
        tracker.activate(2, 100).ok();
        tracker
            .block(2, "depends on goal 1".to_string(), vec![1])
            .ok();

        // Goal 2 should still be blocked.
        assert!(!tracker.check_unblock(2).expect("check should succeed"));

        // Complete goal 1.
        tracker.complete(1, 200).ok();

        // Now goal 2 should be unblockable.
        assert!(tracker.check_unblock(2).expect("check should succeed"));
    }

    #[test]
    fn test_fail_goes_to_history() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();
        tracker.fail(1, "timeout".to_string(), 200).ok();

        assert_eq!(tracker.tracked_count(), 0);
        assert_eq!(tracker.history_count(), 1);

        let stats = tracker.stats();
        assert_eq!(stats.success_rate, 0.0); // 0 successes / 1 total
    }

    // ===================================================================
    // New tests for sub-goal aggregation, prediction, stalls, milestones
    // ===================================================================

    #[test]
    fn test_sub_goal_registration() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok(); // parent
        tracker.track(make_goal(2)).ok(); // sub-goal 1
        tracker.track(make_goal(3)).ok(); // sub-goal 2

        tracker
            .register_sub_goal(1, 2, 1.0)
            .expect("register sub-goal 2");
        tracker
            .register_sub_goal(1, 3, 2.0)
            .expect("register sub-goal 3");

        // Duplicate registration should be a no-op.
        tracker
            .register_sub_goal(1, 2, 5.0)
            .expect("duplicate should succeed silently");

        let details = tracker.sub_goal_progress_details(1).expect("details");
        assert_eq!(details.len(), 2);
    }

    #[test]
    fn test_sub_goal_registration_not_found() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();

        // Sub-goal doesn't exist.
        let result = tracker.register_sub_goal(1, 999, 1.0);
        assert!(matches!(result, Err(GoalError::NotFound(999))));

        // Parent doesn't exist.
        let result = tracker.register_sub_goal(999, 1, 1.0);
        assert!(matches!(result, Err(GoalError::NotFound(999))));
    }

    #[test]
    fn test_sub_goal_aggregation_weighted() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok(); // parent
        tracker.track(make_goal(2)).ok(); // sub-goal 1 (weight=1)
        tracker.track(make_goal(3)).ok(); // sub-goal 2 (weight=3)

        tracker.activate(2, 100).ok();
        tracker.activate(3, 100).ok();

        tracker.register_sub_goal(1, 2, 1.0).ok();
        tracker.register_sub_goal(1, 3, 3.0).ok();

        // Sub-goal 2: 50% done, sub-goal 3: 25% done.
        tracker.update_progress(2, 5, 10).ok(); // 0.5
        tracker.update_progress(3, 1, 4).ok(); // 0.25

        // Weighted avg = (0.5*1 + 0.25*3) / (1+3) = (0.5 + 0.75) / 4 = 0.3125
        let agg = tracker.aggregate_sub_goal_progress(1).expect("aggregate");
        assert!((agg - 0.3125).abs() < 0.001);
    }

    #[test]
    fn test_sub_goal_aggregation_with_completed_sub() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok(); // parent
        tracker.track(make_goal(2)).ok(); // sub-goal

        tracker.activate(2, 100).ok();
        tracker.register_sub_goal(1, 2, 1.0).ok();

        // Complete the sub-goal.
        tracker.update_progress(2, 10, 10).ok();
        tracker.complete(2, 200).ok();

        // Aggregation should use final_progress from history.
        let agg = tracker.aggregate_sub_goal_progress(1).expect("aggregate");
        assert!((agg - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_sub_goal_aggregation_no_subs_falls_back() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();
        tracker.update_progress(1, 3, 10).ok(); // 0.3

        let agg = tracker.aggregate_sub_goal_progress(1).expect("fallback");
        assert!((agg - 0.3).abs() < 0.001);
    }

    #[test]
    fn test_progress_snapshot_recording() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();

        tracker.record_progress_snapshot(1, 0.1, 1000).ok();
        tracker.record_progress_snapshot(1, 0.2, 2000).ok();
        tracker.record_progress_snapshot(1, 0.3, 3000).ok();

        let meta = tracker.progress_meta.get(&1).expect("meta");
        assert_eq!(meta.snapshots.len(), 3);
        assert!((meta.peak_progress - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_progress_prediction_eta() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();

        // Simulate steady progress: 0.1 every 1000ms.
        for i in 1..=5 {
            let progress = i as f32 * 0.1;
            let time = i as u64 * 1000;
            tracker.update_progress(1, i, 10).ok();
            tracker.record_progress_snapshot(1, progress, time).ok();
        }

        let pred = tracker.predict_progress(1, 5000).expect("predict");
        assert!((pred.current_progress - 0.5).abs() < 0.01);
        assert!(!pred.is_stalled);
        assert!(!pred.has_regression);
        assert!(pred.rate_per_ms.is_some());

        // Rate should be ~0.0001 (0.1 per 1000ms).
        let rate = pred.rate_per_ms.expect("rate");
        assert!(rate > 0.0);

        // ETA should be present (to go from 0.5 to 1.0).
        assert!(pred.eta_ms.is_some());
    }

    #[test]
    fn test_progress_prediction_insufficient_data() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();

        // Only one snapshot — not enough.
        tracker.record_progress_snapshot(1, 0.1, 1000).ok();
        tracker.update_progress(1, 1, 10).ok();

        let pred = tracker.predict_progress(1, 1000).expect("predict");
        assert_eq!(pred.eta_ms, None);
        assert_eq!(pred.rate_per_ms, None);
        assert_eq!(pred.confidence, 0.0);
    }

    #[test]
    fn test_stall_detection() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();

        // Set a short stall window for testing.
        tracker.set_stall_window(1, 5000).ok();

        // Record identical progress for a while.
        tracker.update_progress(1, 3, 10).ok();
        tracker.record_progress_snapshot(1, 0.3, 1000).ok();
        tracker.record_progress_snapshot(1, 0.3, 2000).ok();
        tracker.record_progress_snapshot(1, 0.3, 3000).ok();
        tracker.record_progress_snapshot(1, 0.3, 4000).ok();
        tracker.record_progress_snapshot(1, 0.3, 5000).ok();

        let stalls = tracker.detect_stalls(6000);
        assert!(!stalls.is_empty());
        assert_eq!(stalls[0].goal_id, 1);
        assert!((stalls[0].progress_at_stall - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_no_stall_when_making_progress() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();
        tracker.set_stall_window(1, 5000).ok();

        // Record increasing progress.
        tracker.record_progress_snapshot(1, 0.1, 1000).ok();
        tracker.record_progress_snapshot(1, 0.2, 2000).ok();
        tracker.record_progress_snapshot(1, 0.3, 3000).ok();
        tracker.update_progress(1, 3, 10).ok();

        let stalls = tracker.detect_stalls(4000);
        assert!(stalls.is_empty());
    }

    #[test]
    fn test_milestone_tracking() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();

        tracker.add_standard_milestones(1).ok();

        let milestones = tracker.get_milestones(1).expect("milestones");
        assert_eq!(milestones.len(), 4);
        assert!(!milestones[0].reached); // 25%
        assert!(!milestones[1].reached); // 50%

        // Progress to 30% — should trigger 25% milestone.
        tracker.update_progress(1, 3, 10).ok();
        tracker.record_progress_snapshot(1, 0.3, 1000).ok();

        let milestones = tracker.get_milestones(1).expect("milestones");
        assert!(milestones[0].reached); // 25% reached
        assert!(milestones[0].reached_at_ms.is_some());
        assert!(!milestones[1].reached); // 50% not yet
    }

    #[test]
    fn test_milestone_not_found_error() {
        let tracker = GoalTracker::new();
        let result = tracker.get_milestones(999);
        assert!(matches!(result, Err(GoalError::NotFound(999))));
    }

    #[test]
    fn test_regression_detection() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();

        // Progress up.
        tracker.record_progress_snapshot(1, 0.5, 1000).ok();
        tracker.update_progress(1, 5, 10).ok();

        // Progress regresses.
        tracker.record_progress_snapshot(1, 0.3, 2000).ok();
        tracker.update_progress(1, 3, 10).ok();

        // Check regression.
        let has_reg = tracker.check_regression(1).expect("check");
        assert!(has_reg);

        // Should have a regression event.
        let events = tracker.recent_regressions();
        assert!(!events.is_empty());
        assert_eq!(events[0].goal_id, 1);
        assert!((events[0].previous_progress - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_no_regression_when_increasing() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();

        tracker.record_progress_snapshot(1, 0.1, 1000).ok();
        tracker.record_progress_snapshot(1, 0.3, 2000).ok();
        tracker.record_progress_snapshot(1, 0.5, 3000).ok();
        tracker.update_progress(1, 5, 10).ok();

        let has_reg = tracker.check_regression(1).expect("check");
        assert!(!has_reg);
        assert!(tracker.recent_regressions().is_empty());
    }

    #[test]
    fn test_progress_visualization() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();
        tracker.add_standard_milestones(1).ok();

        tracker.update_progress(1, 3, 10).ok();
        tracker.record_progress_snapshot(1, 0.3, 1000).ok();
        tracker.record_progress_snapshot(1, 0.3, 2000).ok();

        let viz = tracker.progress_visualization(1, 2000).expect("viz");
        assert_eq!(viz.goal_id, 1);
        assert!((viz.progress - 0.3).abs() < 0.01);
        assert_eq!(viz.steps_completed, 3);
        assert_eq!(viz.steps_total, 10);
        assert_eq!(viz.history.len(), 2);
        assert_eq!(viz.milestones.len(), 4);
        assert!(viz.prediction.is_some());
    }

    #[test]
    fn test_progress_meta_cleaned_on_cancel() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();
        tracker.record_progress_snapshot(1, 0.3, 1000).ok();

        assert!(tracker.progress_meta.contains_key(&1));
        tracker.cancel(1, 2000).ok();
        assert!(!tracker.progress_meta.contains_key(&1));
    }

    #[test]
    fn test_progress_meta_cleaned_on_fail() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok();
        tracker.activate(1, 100).ok();
        tracker.record_progress_snapshot(1, 0.2, 1000).ok();

        assert!(tracker.progress_meta.contains_key(&1));
        tracker.fail(1, "error".to_string(), 2000).ok();
        assert!(!tracker.progress_meta.contains_key(&1));
    }

    #[test]
    fn test_sub_goal_progress_details_weights_normalized() {
        let mut tracker = GoalTracker::new();
        tracker.track(make_goal(1)).ok(); // parent
        tracker.track(make_goal(2)).ok(); // w=2
        tracker.track(make_goal(3)).ok(); // w=8
        tracker.activate(2, 100).ok();
        tracker.activate(3, 100).ok();

        tracker.register_sub_goal(1, 2, 2.0).ok();
        tracker.register_sub_goal(1, 3, 8.0).ok();

        let details = tracker.sub_goal_progress_details(1).expect("details");
        let total_norm_weight: f32 = details.iter().map(|d| d.weight).sum();
        assert!((total_norm_weight - 1.0).abs() < 0.001);
        assert!((details[0].weight - 0.2).abs() < 0.001);
        assert!((details[1].weight - 0.8).abs() < 0.001);
    }
}
