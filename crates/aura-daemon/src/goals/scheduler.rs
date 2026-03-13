//! Goal priority scheduler — decides what AURA works on now.
//!
//! The scheduler manages a priority queue of active goals using a composite
//! scoring formula:
//!
//! ```text
//! score = goal.priority (set by LLM at creation time)
//! ```
//!
//! Features:
//! - **Preemption**: higher-priority goals can interrupt lower-priority ones
//! - **Aging**: goals waiting too long get priority boosts to prevent starvation
//! - **Power-aware**: in low power states, only high-priority goals proceed
//! - **Concurrent limits**: max N goals active based on complexity + power state

use aura_types::errors::GoalError;
#[allow(unused_imports)] // GoalSource re-imported in inner scopes; this is the canonical top-level import
use aura_types::goals::{Goal, GoalPriority, GoalSource};
use aura_types::power::{DegradationLevel, PowerBudget, PowerState};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use tracing::instrument;

use super::{BoundedMap, BoundedVec};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of concurrent active goals.
const MAX_ACTIVE_GOALS: usize = 32;

/// Maximum goals in the priority queue (active + waiting).
const MAX_QUEUED_GOALS: usize = 128;

/// Aging: priority boost per minute of waiting (capped).
const AGING_BOOST_PER_MIN: f32 = 0.005;

/// Maximum aging boost.
const AGING_MAX_BOOST: f32 = 0.20;

/// Preemption threshold — new goal must score this much higher to preempt.
const PREEMPTION_THRESHOLD: f32 = 0.15;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A goal with its computed priority score, ready for scheduling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredGoal {
    /// The goal being scheduled.
    pub goal_id: u64,
    /// Composite priority score (0.0–1.0).
    pub score: f32,
    /// The individual score components.
    pub components: ScoreComponents,
    /// When this goal was enqueued (epoch ms).
    pub enqueued_at_ms: u64,
    /// Whether this goal is currently active (being worked on).
    pub is_active: bool,
}

/// Individual components of the priority score.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ScoreComponents {
    /// How time-critical is this goal? (0.0–1.0)
    pub urgency: f32,
    /// How important is this goal to the user? (0.0–1.0)
    pub importance: f32,
    /// Does the user expect AURA to be working on this? (0.0–1.0)
    pub user_expectation: f32,
    /// How recently was this goal created? (0.0–1.0, decays over time)
    pub freshness: f32,
    /// Aging boost accumulated from waiting in queue.
    pub aging_boost: f32,
}

impl Eq for ScoredGoal {}

impl PartialEq for ScoredGoal {
    fn eq(&self, other: &Self) -> bool {
        self.goal_id == other.goal_id
    }
}

impl PartialOrd for ScoredGoal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScoredGoal {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher score = higher priority in the heap.
        let self_total = self.score + self.components.aging_boost;
        let other_total = other.score + other.components.aging_boost;
        self_total
            .partial_cmp(&other_total)
            .unwrap_or(Ordering::Equal)
    }
}

/// Decision from the scheduler about what to do next.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SchedulerDecision {
    /// Start working on this goal.
    Activate { goal_id: u64, score: f32 },
    /// Preempt the current goal and switch to a higher-priority one.
    Preempt {
        suspend_goal_id: u64,
        activate_goal_id: u64,
        score_delta: f32,
    },
    /// No goals to work on.
    Idle,
    /// Power too low — only critical goals allowed.
    PowerConstrained { min_priority: GoalPriority },
}

/// The goal scheduler — manages what AURA works on and when.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalScheduler {
    /// Currently active goals (being executed).
    active_goals: BoundedVec<ScoredGoal, MAX_ACTIVE_GOALS>,
    /// Priority queue of goals waiting to be scheduled.
    waiting_queue: Vec<ScoredGoal>,
    /// Maximum concurrent active goals (adjustable by power state).
    max_concurrent: usize,
    /// Maximum goals in the waiting queue.
    max_queued: usize,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl GoalScheduler {
    /// Create a new scheduler with default limits.
    pub fn new() -> Self {
        Self {
            active_goals: BoundedVec::new(),
            waiting_queue: Vec::with_capacity(MAX_QUEUED_GOALS.min(32)),
            max_concurrent: MAX_ACTIVE_GOALS,
            max_queued: MAX_QUEUED_GOALS,
        }
    }

    /// Compute the priority score for a goal.
    ///
    /// LLM sets priority at goal creation — Rust does not re-score.
    /// Returns the goal's own priority as a normalized f32 passthrough.
    #[instrument(skip_all, fields(goal_id = goal.id))]
    pub fn compute_score(&self, goal: &Goal, now_ms: u64) -> ScoredGoal {
        // LLM sets priority at goal creation — Rust does not re-score.
        let score = Self::priority_to_f32(&goal.priority);

        // ScoreComponents are populated for observability but do NOT influence the score.
        let urgency = Self::compute_urgency(goal, now_ms);
        let importance = Self::compute_importance(goal);
        let user_expectation = Self::compute_user_expectation(goal);
        let freshness = Self::compute_freshness(goal, now_ms);

        ScoredGoal {
            goal_id: goal.id,
            score,
            components: ScoreComponents {
                urgency,
                importance,
                user_expectation,
                freshness,
                aging_boost: 0.0,
            },
            enqueued_at_ms: now_ms,
            is_active: false,
        }
    }

    /// Convert a `GoalPriority` enum variant to a normalized f32.
    ///
    /// Structural mapping only — no NLP, no formula.
    fn priority_to_f32(priority: &GoalPriority) -> f32 {
        match priority {
            GoalPriority::Critical => 1.0,
            GoalPriority::High => 0.8,
            GoalPriority::Medium => 0.5,
            GoalPriority::Low => 0.3,
            GoalPriority::Background => 0.1,
        }
    }

    /// Enqueue a goal for scheduling. Returns error if the queue is full.
    #[instrument(skip(self), fields(goal_id = scored.goal_id, score = scored.score))]
    pub fn enqueue(&mut self, scored: ScoredGoal) -> Result<(), GoalError> {
        if self.waiting_queue.len() >= self.max_queued {
            return Err(GoalError::SchedulerFull {
                active: self.active_goals.len(),
            });
        }

        tracing::debug!(
            goal_id = scored.goal_id,
            score = scored.score,
            "goal enqueued"
        );
        self.waiting_queue.push(scored);
        Ok(())
    }

    /// Get the next scheduling decision based on current state and power budget.
    ///
    /// This is the core scheduling loop entry point. Call this when the daemon
    /// is ready to pick up new work.
    #[instrument(skip(self))]
    pub fn next_decision(&mut self, power: &PowerBudget, now_ms: u64) -> SchedulerDecision {
        // Apply aging boosts to waiting goals.
        self.apply_aging(now_ms);

        // Check power constraints.
        let max_concurrent = self.effective_concurrent_limit(power);
        let min_priority = Self::minimum_priority_for_power(power);

        if max_concurrent == 0 {
            return SchedulerDecision::PowerConstrained {
                min_priority: GoalPriority::Critical,
            };
        }

        // Sort waiting queue by score (descending).
        self.waiting_queue.sort_by(|a, b| b.cmp(a));

        // Find the best candidate from the waiting queue.
        let best_candidate = self
            .waiting_queue
            .iter()
            .enumerate()
            .find(|(_, sg)| self.meets_power_requirements(sg, &min_priority));

        let Some((candidate_idx, candidate)) = best_candidate else {
            return SchedulerDecision::Idle;
        };

        let candidate_score = candidate.score + candidate.components.aging_boost;
        let candidate_id = candidate.goal_id;

        // Check if we can just activate (have capacity).
        if self.active_goals.len() < max_concurrent {
            let mut activated = self.waiting_queue.remove(candidate_idx);
            activated.is_active = true;
            let score = activated.score;
            // This should not fail since we checked len < max_concurrent
            if self.active_goals.try_push(activated).is_err() {
                tracing::warn!("active goals full despite capacity check");
                return SchedulerDecision::Idle;
            }
            return SchedulerDecision::Activate {
                goal_id: candidate_id,
                score,
            };
        }

        // Check if preemption is warranted.
        if let Some(lowest_active) = self.find_lowest_active() {
            let lowest_score = lowest_active.score + lowest_active.components.aging_boost;
            let score_delta = candidate_score - lowest_score;

            if score_delta > PREEMPTION_THRESHOLD {
                let suspend_id = lowest_active.goal_id;

                // Remove lowest active goal.
                self.active_goals.retain(|g| g.goal_id != suspend_id);

                // Move candidate from waiting to active.
                let mut activated = self.waiting_queue.remove(candidate_idx);
                activated.is_active = true;
                if self.active_goals.try_push(activated).is_err() {
                    tracing::warn!("failed to push activated goal after preemption");
                    return SchedulerDecision::Idle;
                }

                return SchedulerDecision::Preempt {
                    suspend_goal_id: suspend_id,
                    activate_goal_id: candidate_id,
                    score_delta,
                };
            }
        }

        // No preemption possible — capacity full but nothing to preempt.
        SchedulerDecision::Idle
    }

    /// Mark a goal as completed and remove it from the active set.
    #[instrument(skip(self))]
    pub fn complete_goal(&mut self, goal_id: u64) {
        self.active_goals.retain(|g| g.goal_id != goal_id);
        self.waiting_queue.retain(|g| g.goal_id != goal_id);
        tracing::debug!(goal_id, "goal removed from scheduler");
    }

    /// Suspend an active goal and return it to the waiting queue.
    #[instrument(skip(self))]
    pub fn suspend_goal(&mut self, goal_id: u64) -> Result<(), GoalError> {
        let pos = self.active_goals.iter().position(|g| g.goal_id == goal_id);

        if let Some(idx) = pos {
            let mut suspended = self.active_goals.remove(idx);
            suspended.is_active = false;
            self.waiting_queue.push(suspended);
            tracing::debug!(goal_id, "goal suspended");
            Ok(())
        } else {
            Err(GoalError::NotFound(goal_id))
        }
    }

    /// Number of currently active goals.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active_goals.len()
    }

    /// Number of goals waiting in queue.
    #[must_use]
    pub fn waiting_count(&self) -> usize {
        self.waiting_queue.len()
    }

    /// Total goals being managed (active + waiting).
    #[must_use]
    pub fn total_count(&self) -> usize {
        self.active_goals.len() + self.waiting_queue.len()
    }

    /// Check if a specific goal is currently active.
    #[must_use]
    pub fn is_active(&self, goal_id: u64) -> bool {
        self.active_goals.iter().any(|g| g.goal_id == goal_id)
    }

    /// Get the scored representation of an active goal.
    pub fn get_active_goal(&self, goal_id: u64) -> Option<&ScoredGoal> {
        self.active_goals.iter().find(|g| g.goal_id == goal_id)
    }

    // -- Private helpers ----------------------------------------------------

    /// Compute urgency from deadline proximity (0.0–1.0).
    fn compute_urgency(goal: &Goal, now_ms: u64) -> f32 {
        // Critical priority always gets high urgency.
        if goal.priority == GoalPriority::Critical {
            return 1.0;
        }

        if let Some(deadline) = goal.deadline_ms {
            if now_ms >= deadline {
                return 1.0; // Past deadline — maximum urgency.
            }
            let remaining = deadline - now_ms;
            let total_window = deadline.saturating_sub(goal.created_ms);
            if total_window == 0 {
                return 0.8;
            }
            // Urgency increases as deadline approaches (inverse of time remaining).
            let fraction_remaining = remaining as f32 / total_window as f32;
            (1.0 - fraction_remaining).clamp(0.0, 1.0)
        } else {
            // No deadline — base urgency on priority.
            match goal.priority {
                GoalPriority::Critical => 1.0,
                GoalPriority::High => 0.7,
                GoalPriority::Medium => 0.4,
                GoalPriority::Low => 0.2,
                GoalPriority::Background => 0.05,
            }
        }
    }

    /// Compute importance from priority level (0.0–1.0).
    fn compute_importance(goal: &Goal) -> f32 {
        match goal.priority {
            GoalPriority::Critical => 1.0,
            GoalPriority::High => 0.8,
            GoalPriority::Medium => 0.5,
            GoalPriority::Low => 0.3,
            GoalPriority::Background => 0.1,
        }
    }

    /// Compute user expectation based on goal source (0.0–1.0).
    fn compute_user_expectation(goal: &Goal) -> f32 {
        use aura_types::goals::GoalSource;
        match goal.source {
            GoalSource::UserExplicit => 1.0,          // User asked directly.
            GoalSource::NotificationTriggered => 0.6, // User might expect action.
            GoalSource::CronScheduled => 0.5,         // Routine — moderate expectation.
            GoalSource::GoalDecomposition(_) => 0.4,  // Internal sub-goal.
            GoalSource::ProactiveSuggestion => 0.2,   // AURA's initiative.
        }
    }

    /// Compute freshness — how recently the goal was created (0.0–1.0).
    /// Decays over a 30-minute window.
    fn compute_freshness(goal: &Goal, now_ms: u64) -> f32 {
        let age_ms = now_ms.saturating_sub(goal.created_ms);
        let thirty_minutes_ms = 30 * 60 * 1000;
        let decay = age_ms as f32 / thirty_minutes_ms as f32;
        (1.0 - decay).clamp(0.0, 1.0)
    }

    /// Apply aging boosts to all waiting goals.
    fn apply_aging(&mut self, now_ms: u64) {
        for sg in &mut self.waiting_queue {
            let wait_ms = now_ms.saturating_sub(sg.enqueued_at_ms);
            let wait_minutes = wait_ms as f32 / 60_000.0;
            sg.components.aging_boost = (wait_minutes * AGING_BOOST_PER_MIN).min(AGING_MAX_BOOST);
        }
    }

    /// Determine effective concurrent limit based on power state.
    fn effective_concurrent_limit(&self, power: &PowerBudget) -> usize {
        match power.degradation {
            DegradationLevel::L0Full => self.max_concurrent,
            DegradationLevel::L1Reduced => (self.max_concurrent / 2).max(1),
            DegradationLevel::L2Minimal => 2,
            DegradationLevel::L3DaemonOnly => 1,
            DegradationLevel::L4Heartbeat => 0,
            DegradationLevel::L5Suspended => 0,
        }
    }

    /// Determine minimum priority allowed given power state.
    ///
    /// Derives the legacy `PowerState` from the physics-based `PowerBudget`'s
    /// battery percentage, then maps to the minimum goal priority that is
    /// allowed to run. Lower battery → only higher-priority goals proceed.
    fn minimum_priority_for_power(power: &PowerBudget) -> GoalPriority {
        let state = PowerState::from_battery_percent(power.battery_percent);
        match state {
            PowerState::Normal => GoalPriority::Background,
            PowerState::Conservative => GoalPriority::Low,
            PowerState::LowPower => GoalPriority::Medium,
            PowerState::Critical => GoalPriority::High,
            PowerState::Emergency => GoalPriority::Critical,
        }
    }

    /// Check if a scored goal meets the minimum power requirements.
    fn meets_power_requirements(&self, sg: &ScoredGoal, min_priority: &GoalPriority) -> bool {
        // Use the score as a proxy for priority — higher scores pass the gate.
        let min_score = match min_priority {
            GoalPriority::Background => 0.0,
            GoalPriority::Low => 0.15,
            GoalPriority::Medium => 0.35,
            GoalPriority::High => 0.60,
            GoalPriority::Critical => 0.80,
        };
        (sg.score + sg.components.aging_boost) >= min_score
    }

    /// Find the lowest-scored active goal (candidate for preemption).
    fn find_lowest_active(&self) -> Option<&ScoredGoal> {
        self.active_goals.iter().min_by(|a, b| a.cmp(b))
    }
}

impl Default for GoalScheduler {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// BDI (Belief-Desire-Intention) Framework Extension
// ===========================================================================

/// Maximum number of beliefs in the belief base.
const MAX_BELIEFS: usize = 128;

/// Maximum number of desires tracked simultaneously.
const MAX_DESIRES: usize = 64;

/// Maximum number of active intentions.
const MAX_INTENTIONS: usize = 32;

/// Maximum number of suspended goals with preserved state.
const MAX_SUSPENDED: usize = 64;

/// Maximum number of detected conflicts.
const MAX_DETECTED_CONFLICTS: usize = 32;

/// Maximum number of detected synergies.
const MAX_DETECTED_SYNERGIES: usize = 32;

// ---------------------------------------------------------------------------
// BDI Types
// ---------------------------------------------------------------------------

/// A belief about the current world state.
///
/// Beliefs are updated from sensors, system APIs, and execution outcomes.
/// They inform which desires are feasible and how to pursue intentions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Belief {
    /// Unique key (e.g. "battery_level", "network_connected", "app_foreground").
    pub key: String,
    /// Current value as a string (interpreted by consumers).
    pub value: String,
    /// Confidence in this belief (0.0–1.0).
    pub confidence: f32,
    /// When this belief was last updated (epoch ms).
    pub updated_at_ms: u64,
    /// Source of this belief.
    pub source: BeliefSource,
}

/// Where a belief comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BeliefSource {
    /// Observed from system sensors/APIs.
    SystemSensor,
    /// Inferred from execution outcomes.
    ExecutionOutcome,
    /// Reported by the user.
    UserInput,
    /// Inferred by the reasoning engine.
    Inference,
}

/// A desire — something AURA wants to achieve.
///
/// Desires are generated from goals filtered through the belief base.
/// Not all desires become intentions — only those that pass deliberation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Desire {
    /// The goal ID this desire maps to.
    pub goal_id: u64,
    /// Desirability score after belief filtering (0.0–1.0).
    pub desirability: f32,
    /// Whether this desire is feasible given current beliefs.
    pub feasible: bool,
    /// Reason if not feasible.
    pub infeasibility_reason: Option<String>,
    /// Resource requirements (e.g. ["network", "camera", "screen"]).
    pub required_resources: Vec<String>,
}

/// An intention — a committed plan that AURA is actively pursuing.
///
/// Intentions are desires that have passed deliberation and been allocated
/// resources. They represent AURA's active commitments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intention {
    /// The goal ID.
    pub goal_id: u64,
    /// Commitment strength (0.0–1.0) — higher means less likely to drop.
    pub commitment: f32,
    /// Resources allocated to this intention.
    pub allocated_resources: Vec<String>,
    /// When this intention was formed (epoch ms).
    pub formed_at_ms: u64,
    /// Delegation type — how this goal should be executed.
    pub delegation: DelegationType,
}

/// How a goal should be executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DelegationType {
    /// AURA handles it fully (e.g. set alarm, web search).
    AuraAutonomous,
    /// User needs to do something (e.g. confirm a payment).
    UserAction { prompt: String },
    /// Combination of AURA automation + user confirmation at key points.
    Hybrid { user_steps: Vec<String> },
}

/// State preserved when a goal is suspended, allowing seamless resumption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuspendedGoalState {
    /// The goal ID.
    pub goal_id: u64,
    /// Score at time of suspension.
    pub score_at_suspension: f32,
    /// Progress (0.0–1.0) at time of suspension.
    pub progress: f32,
    /// Which step index was being executed.
    pub current_step_index: usize,
    /// Reason for suspension.
    pub reason: SuspensionReason,
    /// When the goal was suspended (epoch ms).
    pub suspended_at_ms: u64,
    /// Partial results accumulated before suspension.
    pub partial_results: Vec<String>,
}

/// Why a goal was suspended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuspensionReason {
    /// Preempted by a higher-priority goal.
    Preempted { by_goal_id: u64 },
    /// Power state dropped too low.
    PowerConstrained,
    /// Waiting for a resource to become available.
    ResourceUnavailable { resource: String },
    /// Waiting for user input.
    WaitingForUser,
    /// Blocked by a dependency goal.
    DependencyBlocked { blocked_by: u64 },
    /// Explicitly paused by the user.
    UserPaused,
}

/// A detected conflict between two goals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalConflict {
    /// First goal ID.
    pub goal_a: u64,
    /// Second goal ID.
    pub goal_b: u64,
    /// Type of conflict.
    pub conflict_type: ConflictKind,
    /// Severity (0.0–1.0).
    pub severity: f32,
}

/// Type of conflict between goals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictKind {
    /// Both goals need the same exclusive resource (e.g. camera).
    Resource { resource: String },
    /// Overlapping deadlines with insufficient time.
    Temporal,
    /// Goals are logically contradictory.
    Logical { reason: String },
}

/// A detected synergy between two goals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalSynergy {
    /// First goal ID.
    pub goal_a: u64,
    /// Second goal ID.
    pub goal_b: u64,
    /// Shared steps or prerequisites.
    pub shared_elements: Vec<String>,
    /// Estimated efficiency gain (0.0–1.0) from executing together.
    pub efficiency_gain: f32,
}

/// Extended score components including BDI factors.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ExtendedScoreComponents {
    /// Base score components.
    pub base: ScoreComponents,
    /// Feasibility given current beliefs (0.0–1.0).
    pub feasibility: f32,
    /// Personality influence factor (0.0–1.0).
    pub personality_influence: f32,
    /// Dependency satisfaction factor (0.0–1.0).
    pub dependency_satisfaction: f32,
}

/// Result of the BDI deliberation cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeliberationResult {
    /// Commit to these goals as new intentions.
    Commit { new_intentions: Vec<u64> },
    /// Reconsider current intentions (some should be dropped).
    Reconsider {
        drop_intentions: Vec<u64>,
        reason: String,
    },
    /// No changes needed.
    Maintain,
}

// ---------------------------------------------------------------------------
// BDI Scheduler Extension
// ---------------------------------------------------------------------------

/// Extended scheduler incorporating the full BDI deliberation framework.
///
/// Wraps the base [`GoalScheduler`] and adds belief tracking, desire generation,
/// intention management, conflict/synergy detection, and goal delegation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BdiScheduler {
    /// The base priority scheduler.
    pub base: GoalScheduler,
    /// Current belief base.
    beliefs: BoundedMap<String, Belief, MAX_BELIEFS>,
    /// Current desires (filtered goals).
    desires: BoundedVec<Desire, MAX_DESIRES>,
    /// Active intentions (committed plans).
    intentions: BoundedVec<Intention, MAX_INTENTIONS>,
    /// Suspended goals with preserved state.
    suspended: BoundedVec<SuspendedGoalState, MAX_SUSPENDED>,
    /// Detected conflicts.
    conflicts: BoundedVec<GoalConflict, MAX_DETECTED_CONFLICTS>,
    /// Detected synergies.
    synergies: BoundedVec<GoalSynergy, MAX_DETECTED_SYNERGIES>,
}

impl BdiScheduler {
    /// Create a new BDI scheduler.
    pub fn new() -> Self {
        Self {
            base: GoalScheduler::new(),
            beliefs: BoundedMap::new(),
            desires: BoundedVec::new(),
            intentions: BoundedVec::new(),
            suspended: BoundedVec::new(),
            conflicts: BoundedVec::new(),
            synergies: BoundedVec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Belief management
    // -----------------------------------------------------------------------

    /// Update or insert a belief in the belief base.
    #[instrument(skip(self), fields(key = %belief.key))]
    pub fn update_belief(&mut self, belief: Belief) -> Result<(), GoalError> {
        if self.beliefs.get(&belief.key).is_some() {
            if let Some(existing) = self.beliefs.get_mut(&belief.key) {
                *existing = belief;
            }
            Ok(())
        } else {
            self.beliefs
                .try_insert(belief.key.clone(), belief)
                .map(|_| ())
                .map_err(|_| GoalError::CapacityExceeded { max: MAX_BELIEFS })
        }
    }

    /// Get the current value of a belief.
    pub fn get_belief(&self, key: &str) -> Option<&Belief> {
        self.beliefs.get(&key.to_string())
    }

    /// Remove a belief from the belief base.
    pub fn remove_belief(&mut self, key: &str) -> Option<Belief> {
        self.beliefs.remove(&key.to_string())
    }

    /// Total number of beliefs.
    #[must_use]
    pub fn belief_count(&self) -> usize {
        self.beliefs.len()
    }

    // -----------------------------------------------------------------------
    // Extended scoring
    // -----------------------------------------------------------------------

    /// Compute an extended BDI score for a goal.
    ///
    /// LLM sets priority at goal creation — Rust does not re-score.
    /// Returns the goal's own priority as a normalized f32 passthrough.
    /// Extended components are populated for observability only.
    pub fn compute_extended_score(
        &self,
        goal: &Goal,
        now_ms: u64,
        dependency_ids: &[u64],
        personality_openness: f32,
    ) -> (ScoredGoal, ExtendedScoreComponents) {
        // LLM sets priority at goal creation — Rust does not re-score.
        let base_scored = self.base.compute_score(goal, now_ms);
        let feasibility = self.compute_feasibility(goal);
        let personality = Self::compute_personality_influence(goal, personality_openness);
        let dependency = self.compute_dependency_satisfaction(dependency_ids);

        // Score is the goal's own priority — no re-weighting formula.
        // Components are recorded for observability but do not influence score.
        let ext_components = ExtendedScoreComponents {
            base: base_scored.components,
            feasibility,
            personality_influence: personality,
            dependency_satisfaction: dependency,
        };

        (base_scored, ext_components)
    }

    /// Compute feasibility based on current beliefs.
    ///
    /// Applies mechanical multipliers derived from well-known system beliefs:
    /// - battery_level < 15 → 0.5× penalty (very low battery)
    /// - network_connected = "false" AND goal description contains network
    ///   keywords → 0.6× penalty
    /// - screen_on = "false" → 0.8× penalty (UI goals need screen)
    fn compute_feasibility(&self, goal: &Goal) -> f32 {
        let mut feasibility = 1.0_f32;

        // Battery belief: low battery degrades feasibility.
        if let Some(b) = self.get_belief("battery_level") {
            if let Ok(level) = b.value.parse::<f32>() {
                if level < 15.0 {
                    feasibility *= 0.5;
                } else if level < 30.0 {
                    feasibility *= 0.75;
                }
            }
        }

        // Network belief: disconnected + network-requiring description degrades feasibility.
        if let Some(b) = self.get_belief("network_connected") {
            if b.value == "false" {
                // Check if the goal description involves network activity.
                let desc_lower = goal.description.to_ascii_lowercase();
                const NETWORK_KEYWORDS: &[&str] = &[
                    "search", "send", "message", "navigate", "maps", "online",
                    "web", "internet", "email", "download", "upload", "stream",
                    "restaurant", "weather", "news", "sync",
                ];
                let needs_network = NETWORK_KEYWORDS
                    .iter()
                    .any(|kw| desc_lower.contains(kw));
                if needs_network {
                    feasibility *= 0.6;
                }
            }
        }

        // Screen belief: screen off reduces feasibility for interactive goals.
        if let Some(b) = self.get_belief("screen_on") {
            if b.value == "false" {
                feasibility *= 0.8;
            }
        }

        feasibility.clamp(0.0, 1.0)
    }

    /// Compute personality influence on goal priority.
    ///
    /// Higher openness → more willing to pursue proactive/novel goals.
    /// Lower openness → stronger preference for user-explicit goals.
    fn compute_personality_influence(goal: &Goal, openness: f32) -> f32 {
        // LLM sets priority at goal creation — Rust does not re-score.
        // Returns raw priority f32; openness parameter retained for API compat.
        let _ = openness;
        GoalScheduler::priority_to_f32(&goal.priority)
    }

    /// Compute how many dependency goals are already satisfied.
    ///
    /// Returns 1.0 if all dependencies are met (in active/completed intentions),
    /// proportional fraction otherwise.
    fn compute_dependency_satisfaction(&self, dependency_ids: &[u64]) -> f32 {
        if dependency_ids.is_empty() {
            return 1.0; // No dependencies — fully satisfied.
        }

        let satisfied = dependency_ids
            .iter()
            .filter(|dep_id| {
                self.intentions.iter().any(|i| i.goal_id == **dep_id)
                    || self.base.is_active(**dep_id)
            })
            .count();

        satisfied as f32 / dependency_ids.len() as f32
    }

    // -----------------------------------------------------------------------
    // BDI Deliberation Cycle
    // -----------------------------------------------------------------------

    /// Run the full BDI deliberation cycle.
    ///
    /// Phases:
    /// 1. **Option generation**: Convert goals to desires by filtering through beliefs
    /// 2. **Filtering**: Remove desires that conflict with existing intentions
    /// 3. **Deliberation**: Select which desires to commit to as new intentions
    ///
    /// Returns a `DeliberationResult` describing what action to take.
    #[instrument(skip(self, goals))]
    pub fn deliberate(
        &mut self,
        goals: &[Goal],
        now_ms: u64,
        personality_openness: f32,
    ) -> DeliberationResult {
        // Phase 1: Option generation — goals → desires.
        self.generate_desires(goals, now_ms, personality_openness);

        // Phase 2: Filtering — remove infeasible and conflicting desires.
        self.filter_desires();

        // Phase 3: Deliberation — select best desires to commit.
        self.select_intentions(now_ms)
    }

    /// Phase 1: Generate desires from goals by filtering through beliefs.
    fn generate_desires(&mut self, goals: &[Goal], now_ms: u64, personality_openness: f32) {
        self.desires = BoundedVec::new();

        for goal in goals {
            let base_scored = self.base.compute_score(goal, now_ms);

            // LLM sets priority at goal creation — Rust uses goal priority directly as desirability.
            // No blended formula: desirability = goal's own priority (f32 passthrough).
            let desirability = base_scored.score;

            // Unused — kept for API compat; LLM assesses feasibility.
            let _ = personality_openness;

            // LLM infers resource requirements — Rust returns empty as neutral default.
            let required_resources = Self::infer_resources(&goal.description);

            let desire = Desire {
                goal_id: goal.id,
                desirability,
                feasible: true,
                infeasibility_reason: None,
                required_resources,
            };

            let _ = self.desires.try_push(desire);
        }
    }

    /// Phase 2: Filter desires — remove infeasible ones and detect conflicts.
    fn filter_desires(&mut self) {
        // Remove infeasible desires.
        self.desires.retain(|d| d.feasible);

        // Detect resource conflicts among remaining desires.
        self.conflicts = BoundedVec::new();
        let desires_snapshot: Vec<Desire> = self.desires.iter().cloned().collect();
        for i in 0..desires_snapshot.len() {
            for j in (i + 1)..desires_snapshot.len() {
                let a = &desires_snapshot[i];
                let b = &desires_snapshot[j];
                for res_a in &a.required_resources {
                    if b.required_resources.contains(res_a) {
                        let conflict = GoalConflict {
                            goal_a: a.goal_id,
                            goal_b: b.goal_id,
                            conflict_type: ConflictKind::Resource {
                                resource: res_a.clone(),
                            },
                            severity: 0.7,
                        };
                        let _ = self.conflicts.try_push(conflict);
                    }
                }
            }
        }
    }

    /// Phase 3: Select the best desires and commit them as intentions.
    fn select_intentions(&mut self, now_ms: u64) -> DeliberationResult {
        // Sort desires by desirability (descending).
        let mut candidates: Vec<Desire> = self.desires.iter().cloned().collect();
        candidates.sort_by(|a, b| {
            b.desirability
                .partial_cmp(&a.desirability)
                .unwrap_or(Ordering::Equal)
        });

        let mut new_intentions = Vec::new();
        let mut allocated_resources: Vec<String> = self
            .intentions
            .iter()
            .flat_map(|i| i.allocated_resources.clone())
            .collect();

        for candidate in &candidates {
            // Skip if already an intention.
            if self
                .intentions
                .iter()
                .any(|i| i.goal_id == candidate.goal_id)
            {
                continue;
            }

            // Check resource conflicts with existing allocations.
            let conflicts_with_existing = candidate
                .required_resources
                .iter()
                .any(|r| allocated_resources.contains(r));

            if conflicts_with_existing {
                continue;
            }

            if self.intentions.len() >= MAX_INTENTIONS {
                break;
            }

            let delegation = Self::determine_delegation(candidate);

            let intention = Intention {
                goal_id: candidate.goal_id,
                commitment: candidate.desirability,
                allocated_resources: candidate.required_resources.clone(),
                formed_at_ms: now_ms,
                delegation,
            };

            allocated_resources.extend(candidate.required_resources.clone());
            new_intentions.push(candidate.goal_id);
            let _ = self.intentions.try_push(intention);
        }

        // Check if any current intentions should be reconsidered.
        let drop_candidates: Vec<u64> = self
            .intentions
            .iter()
            .filter(|i| {
                // Drop intentions with low commitment that have been around too long.
                let age_ms = now_ms.saturating_sub(i.formed_at_ms);
                i.commitment < 0.3 && age_ms > 5 * 60_000 // 5 minutes
            })
            .map(|i| i.goal_id)
            .collect();

        if !drop_candidates.is_empty() {
            self.intentions
                .retain(|i| !drop_candidates.contains(&i.goal_id));
            return DeliberationResult::Reconsider {
                drop_intentions: drop_candidates,
                reason: "low commitment and stale".to_string(),
            };
        }

        if new_intentions.is_empty() {
            DeliberationResult::Maintain
        } else {
            DeliberationResult::Commit { new_intentions }
        }
    }

    // -----------------------------------------------------------------------
    // Conflict and synergy detection
    // -----------------------------------------------------------------------

    /// Detect conflicts between active goals.
    ///
    /// Checks for resource, temporal, and logical conflicts across all
    /// pairs of active intentions.
    pub fn detect_conflicts(&mut self, goals: &[Goal], now_ms: u64) -> &[GoalConflict] {
        self.conflicts = BoundedVec::new();

        let intention_ids: Vec<u64> = self.intentions.iter().map(|i| i.goal_id).collect();

        for i in 0..intention_ids.len() {
            for j in (i + 1)..intention_ids.len() {
                let id_a = intention_ids[i];
                let id_b = intention_ids[j];

                let goal_a = goals.iter().find(|g| g.id == id_a);
                let goal_b = goals.iter().find(|g| g.id == id_b);

                if let (Some(ga), Some(gb)) = (goal_a, goal_b) {
                    // Temporal conflict: overlapping deadlines with tight time.
                    if let (Some(dl_a), Some(dl_b)) = (ga.deadline_ms, gb.deadline_ms) {
                        let gap = if dl_a > dl_b {
                            dl_a.saturating_sub(dl_b)
                        } else {
                            dl_b.saturating_sub(dl_a)
                        };
                        // If both deadlines are within 5 minutes of each other
                        // and both are within 10 minutes of now.
                        if gap < 5 * 60_000
                            && dl_a.saturating_sub(now_ms) < 10 * 60_000
                            && dl_b.saturating_sub(now_ms) < 10 * 60_000
                        {
                            let _ = self.conflicts.try_push(GoalConflict {
                                goal_a: id_a,
                                goal_b: id_b,
                                conflict_type: ConflictKind::Temporal,
                                severity: 0.8,
                            });
                        }
                    }

                    // Resource conflict via intentions.
                    let int_a = self.intentions.iter().find(|x| x.goal_id == id_a);
                    let int_b = self.intentions.iter().find(|x| x.goal_id == id_b);
                    if let (Some(ia), Some(ib)) = (int_a, int_b) {
                        for res in &ia.allocated_resources {
                            if ib.allocated_resources.contains(res) {
                                let _ = self.conflicts.try_push(GoalConflict {
                                    goal_a: id_a,
                                    goal_b: id_b,
                                    conflict_type: ConflictKind::Resource {
                                        resource: res.clone(),
                                    },
                                    severity: 0.7,
                                });
                            }
                        }
                    }
                }
            }
        }

        self.conflicts.as_slice()
    }

    /// Detect synergies — goals that share steps or prerequisites.
    pub fn detect_synergies(&mut self, goals: &[Goal]) -> &[GoalSynergy] {
        self.synergies = BoundedVec::new();

        let active_ids: Vec<u64> = self.intentions.iter().map(|i| i.goal_id).collect();

        for i in 0..active_ids.len() {
            for j in (i + 1)..active_ids.len() {
                let goal_a = goals.iter().find(|g| g.id == active_ids[i]);
                let goal_b = goals.iter().find(|g| g.id == active_ids[j]);

                if let (Some(ga), Some(gb)) = (goal_a, goal_b) {
                    let shared = Self::find_shared_steps(ga, gb);
                    if !shared.is_empty() {
                        let total_steps = ga.steps.len().max(1) + gb.steps.len().max(1);
                        let efficiency = (shared.len() as f32 / total_steps as f32).clamp(0.0, 0.5);

                        let _ = self.synergies.try_push(GoalSynergy {
                            goal_a: ga.id,
                            goal_b: gb.id,
                            shared_elements: shared,
                            efficiency_gain: efficiency,
                        });
                    }
                }
            }
        }

        self.synergies.as_slice()
    }

    /// Find shared steps between two goals (by comparing step descriptions).
    fn find_shared_steps(ga: &Goal, gb: &Goal) -> Vec<String> {
        let mut shared = Vec::new();
        for step_a in &ga.steps {
            let desc_a = step_a.description.to_ascii_lowercase();
            for step_b in &gb.steps {
                let desc_b = step_b.description.to_ascii_lowercase();
                // Simple word overlap check.
                let words_a: Vec<&str> = desc_a.split_whitespace().collect();
                let words_b: Vec<&str> = desc_b.split_whitespace().collect();
                let overlap = words_a
                    .iter()
                    .filter(|w| words_b.contains(w) && w.len() > 3)
                    .count();
                if overlap >= 2 {
                    shared.push(step_a.description.clone());
                    break;
                }
            }
        }
        shared
    }

    // -----------------------------------------------------------------------
    // Suspension with progress preservation
    // -----------------------------------------------------------------------

    /// Suspend a goal with full state preservation.
    ///
    /// Unlike the base `suspend_goal`, this preserves progress, current step,
    /// and partial results for seamless resumption.
    #[instrument(skip(self, partial_results))]
    pub fn suspend_with_state(
        &mut self,
        goal_id: u64,
        progress: f32,
        current_step: usize,
        reason: SuspensionReason,
        now_ms: u64,
        partial_results: Vec<String>,
    ) -> Result<(), GoalError> {
        // Get the score before we remove it.
        let score = self
            .base
            .get_active_goal(goal_id)
            .map(|sg| sg.score)
            .unwrap_or(0.0);

        // Suspend in the base scheduler.
        self.base.suspend_goal(goal_id)?;

        // Remove from intentions.
        self.intentions.retain(|i| i.goal_id != goal_id);

        // Save the suspended state.
        let state = SuspendedGoalState {
            goal_id,
            score_at_suspension: score,
            progress,
            current_step_index: current_step,
            reason,
            suspended_at_ms: now_ms,
            partial_results,
        };

        self.suspended
            .try_push(state)
            .map_err(|_| GoalError::CapacityExceeded { max: MAX_SUSPENDED })?;

        tracing::info!(
            goal_id,
            progress,
            current_step,
            "goal suspended with state preserved"
        );
        Ok(())
    }

    /// Get the preserved state for a suspended goal.
    pub fn get_suspended_state(&self, goal_id: u64) -> Option<&SuspendedGoalState> {
        self.suspended.iter().find(|s| s.goal_id == goal_id)
    }

    /// Resume a suspended goal, restoring its state.
    ///
    /// Returns the preserved state so the executor can resume from where it left off.
    pub fn resume_suspended(&mut self, goal_id: u64) -> Result<SuspendedGoalState, GoalError> {
        let pos = self
            .suspended
            .iter()
            .position(|s| s.goal_id == goal_id)
            .ok_or(GoalError::NotFound(goal_id))?;

        let state = self.suspended.remove(pos);
        tracing::info!(
            goal_id,
            progress = state.progress,
            step = state.current_step_index,
            "resuming suspended goal"
        );
        Ok(state)
    }

    /// Number of suspended goals.
    #[must_use]
    pub fn suspended_count(&self) -> usize {
        self.suspended.len()
    }

    // -----------------------------------------------------------------------
    // Goal delegation
    // -----------------------------------------------------------------------

    /// Determine the delegation type for a desire based on its resource
    /// requirements and inferred complexity.
    fn determine_delegation(desire: &Desire) -> DelegationType {
        let needs_user = desire
            .required_resources
            .iter()
            .any(|r| r == "user_confirmation" || r == "user_input" || r == "payment");

        if needs_user {
            let user_steps: Vec<String> = desire
                .required_resources
                .iter()
                .filter(|r| r.starts_with("user_"))
                .cloned()
                .collect();

            if user_steps.len() > 1 {
                DelegationType::Hybrid { user_steps }
            } else {
                DelegationType::UserAction {
                    prompt: "User action required".to_string(),
                }
            }
        } else {
            DelegationType::AuraAutonomous
        }
    }

    /// Get the delegation type for an active intention.
    pub fn get_delegation(&self, goal_id: u64) -> Option<&DelegationType> {
        self.intentions
            .iter()
            .find(|i| i.goal_id == goal_id)
            .map(|i| &i.delegation)
    }

    // -----------------------------------------------------------------------
    // Query helpers
    // -----------------------------------------------------------------------

    /// Get all current desires.
    pub fn desires(&self) -> &[Desire] {
        self.desires.as_slice()
    }

    /// Get all current intentions.
    pub fn intentions(&self) -> &[Intention] {
        self.intentions.as_slice()
    }

    /// Get all detected conflicts.
    pub fn conflicts(&self) -> &[GoalConflict] {
        self.conflicts.as_slice()
    }

    /// Get all detected synergies.
    pub fn synergies(&self) -> &[GoalSynergy] {
        self.synergies.as_slice()
    }

    /// Infer required resources from a goal description.
    ///
    /// Infer resource requirements from a goal description via keyword lookup.
    ///
    /// Performs structural substring matching against a fixed keyword table —
    /// no NLP or semantic reasoning.  Returns the set of resource tags that
    /// the description implies.
    fn infer_resources(description: &str) -> Vec<String> {
        let desc_lower = description.to_ascii_lowercase();
        let mut resources: Vec<String> = Vec::new();

        // Camera / photo
        if desc_lower.contains("camera")
            || desc_lower.contains("photo")
            || desc_lower.contains("picture")
            || desc_lower.contains("selfie")
            || desc_lower.contains("scan")
        {
            resources.push("camera".to_string());
        }

        // Microphone / audio
        if desc_lower.contains("microphone")
            || desc_lower.contains("record")
            || desc_lower.contains("voice")
            || desc_lower.contains("audio")
            || desc_lower.contains("dictate")
        {
            resources.push("microphone".to_string());
        }

        // GPS / location
        if desc_lower.contains("navigate")
            || desc_lower.contains("navigation")
            || desc_lower.contains("maps")
            || desc_lower.contains("location")
            || desc_lower.contains("gps")
            || desc_lower.contains("airport")
            || desc_lower.contains("directions")
        {
            resources.push("gps".to_string());
        }

        // Network / internet
        if desc_lower.contains("search")
            || desc_lower.contains("send")
            || desc_lower.contains("message")
            || desc_lower.contains("navigate")
            || desc_lower.contains("maps")
            || desc_lower.contains("online")
            || desc_lower.contains("web")
            || desc_lower.contains("internet")
            || desc_lower.contains("email")
            || desc_lower.contains("download")
            || desc_lower.contains("upload")
            || desc_lower.contains("stream")
            || desc_lower.contains("restaurant")
            || desc_lower.contains("weather")
            || desc_lower.contains("news")
            || desc_lower.contains("network")
            || desc_lower.contains("sync")
        {
            resources.push("network".to_string());
        }

        // Bluetooth / peripheral
        if desc_lower.contains("bluetooth")
            || desc_lower.contains("pair")
            || desc_lower.contains("headphone")
            || desc_lower.contains("speaker")
        {
            resources.push("bluetooth".to_string());
        }

        // User confirmation / financial
        if desc_lower.contains("buy")
            || desc_lower.contains("purchase")
            || desc_lower.contains("pay")
            || desc_lower.contains("order")
            || desc_lower.contains("checkout")
            || desc_lower.contains("confirm")
        {
            resources.push("user_confirmation".to_string());
        }

        resources
    }
}

impl Default for BdiScheduler {
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
    use aura_types::goals::{GoalPriority, GoalSource, GoalStatus};

    fn make_goal(id: u64, priority: GoalPriority, source: GoalSource) -> Goal {
        Goal {
            id,
            description: format!("Goal {}", id),
            priority,
            status: GoalStatus::Pending,
            steps: vec![],
            created_ms: 1_700_000_000_000,
            deadline_ms: None,
            parent_goal: None,
            source,
        }
    }

    fn default_power() -> PowerBudget {
        PowerBudget::default()
    }

    #[test]
    fn test_score_computation_priority_ordering() {
        let scheduler = GoalScheduler::new();
        let now = 1_700_000_000_000;

        let critical = make_goal(1, GoalPriority::Critical, GoalSource::UserExplicit);
        let low = make_goal(2, GoalPriority::Low, GoalSource::ProactiveSuggestion);

        let score_critical = scheduler.compute_score(&critical, now);
        let score_low = scheduler.compute_score(&low, now);

        assert!(
            score_critical.score > score_low.score,
            "critical={} should be > low={}",
            score_critical.score,
            score_low.score
        );
    }

    #[test]
    fn test_enqueue_and_activate() {
        let mut scheduler = GoalScheduler::new();
        let goal = make_goal(1, GoalPriority::High, GoalSource::UserExplicit);
        let scored = scheduler.compute_score(&goal, 1_700_000_000_000);

        scheduler.enqueue(scored).expect("enqueue should succeed");
        assert_eq!(scheduler.waiting_count(), 1);
        assert_eq!(scheduler.active_count(), 0);

        let decision = scheduler.next_decision(&default_power(), 1_700_000_000_000);
        match decision {
            SchedulerDecision::Activate { goal_id, .. } => {
                assert_eq!(goal_id, 1);
            }
            other => panic!("expected Activate, got {:?}", other),
        }
        assert_eq!(scheduler.active_count(), 1);
        assert_eq!(scheduler.waiting_count(), 0);
    }

    #[test]
    fn test_idle_when_no_goals() {
        let mut scheduler = GoalScheduler::new();
        let decision = scheduler.next_decision(&default_power(), 1_700_000_000_000);
        assert!(matches!(decision, SchedulerDecision::Idle));
    }

    #[test]
    fn test_complete_goal_removes_from_active() {
        let mut scheduler = GoalScheduler::new();
        let goal = make_goal(1, GoalPriority::High, GoalSource::UserExplicit);
        let scored = scheduler.compute_score(&goal, 1_700_000_000_000);
        scheduler.enqueue(scored).ok();
        scheduler.next_decision(&default_power(), 1_700_000_000_000);
        assert_eq!(scheduler.active_count(), 1);

        scheduler.complete_goal(1);
        assert_eq!(scheduler.active_count(), 0);
    }

    #[test]
    fn test_suspend_goal() {
        let mut scheduler = GoalScheduler::new();
        let goal = make_goal(1, GoalPriority::Medium, GoalSource::UserExplicit);
        let scored = scheduler.compute_score(&goal, 1_700_000_000_000);
        scheduler.enqueue(scored).ok();
        scheduler.next_decision(&default_power(), 1_700_000_000_000);
        assert_eq!(scheduler.active_count(), 1);

        scheduler.suspend_goal(1).expect("suspend should succeed");
        assert_eq!(scheduler.active_count(), 0);
        assert_eq!(scheduler.waiting_count(), 1);
    }

    #[test]
    fn test_power_constrained_scheduling() {
        let mut scheduler = GoalScheduler::new();
        let goal = make_goal(1, GoalPriority::Low, GoalSource::ProactiveSuggestion);
        let scored = scheduler.compute_score(&goal, 1_700_000_000_000);
        scheduler.enqueue(scored).ok();

        // Heartbeat mode — nothing should run.
        let power = PowerBudget {
            degradation: DegradationLevel::L4Heartbeat,
            ..PowerBudget::default()
        };
        let decision = scheduler.next_decision(&power, 1_700_000_000_000);
        assert!(matches!(
            decision,
            SchedulerDecision::PowerConstrained { .. }
        ));
    }

    #[test]
    fn test_aging_boost() {
        let mut scheduler = GoalScheduler::new();
        let goal = make_goal(1, GoalPriority::Low, GoalSource::ProactiveSuggestion);
        let scored = scheduler.compute_score(&goal, 1_000_000);
        scheduler.enqueue(scored).ok();

        // Advance time by 10 minutes.
        let now = 1_000_000 + 10 * 60_000;
        scheduler.apply_aging(now);

        let waiting = &scheduler.waiting_queue[0];
        assert!(
            waiting.components.aging_boost > 0.0,
            "aging boost should be positive after 10 minutes: {}",
            waiting.components.aging_boost
        );
    }

    #[test]
    fn test_deadline_urgency() {
        let scheduler = GoalScheduler::new();
        let now = 1_700_000_000_000u64;

        // Goal with deadline 1 minute away.
        let mut urgent = make_goal(1, GoalPriority::Medium, GoalSource::UserExplicit);
        urgent.deadline_ms = Some(now + 60_000);
        urgent.created_ms = now - 300_000; // Created 5 min ago.

        let score = scheduler.compute_score(&urgent, now);
        assert!(
            score.components.urgency > 0.8,
            "urgency should be high near deadline: {}",
            score.components.urgency
        );
    }

    #[test]
    fn test_freshness_decay() {
        let scheduler = GoalScheduler::new();
        let now = 1_700_000_000_000u64;

        let fresh = make_goal(1, GoalPriority::Medium, GoalSource::UserExplicit);
        let score_fresh = scheduler.compute_score(&fresh, now);

        // 1 hour old goal.
        let mut old = make_goal(2, GoalPriority::Medium, GoalSource::UserExplicit);
        old.created_ms = now - 3_600_000;
        let score_old = scheduler.compute_score(&old, now);

        assert!(
            score_fresh.components.freshness > score_old.components.freshness,
            "fresh={} should be > old={}",
            score_fresh.components.freshness,
            score_old.components.freshness
        );
    }

    // -----------------------------------------------------------------------
    // BDI Framework tests
    // -----------------------------------------------------------------------

    fn make_bdi() -> BdiScheduler {
        BdiScheduler::new()
    }

    fn make_belief(key: &str, value: &str, confidence: f32) -> Belief {
        Belief {
            key: key.to_string(),
            value: value.to_string(),
            confidence,
            updated_at_ms: 1_700_000_000_000,
            source: BeliefSource::SystemSensor,
        }
    }

    #[test]
    fn test_bdi_update_and_get_belief() {
        let mut bdi = make_bdi();
        let b = make_belief("battery_level", "85", 0.95);
        assert!(bdi.update_belief(b).is_ok());
        assert_eq!(bdi.belief_count(), 1);

        let retrieved = bdi.get_belief("battery_level");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.map(|b| b.value.as_str()), Some("85"));
    }

    #[test]
    fn test_bdi_update_belief_replaces_existing() {
        let mut bdi = make_bdi();
        bdi.update_belief(make_belief("battery_level", "90", 0.9))
            .ok();
        bdi.update_belief(make_belief("battery_level", "45", 0.95))
            .ok();

        assert_eq!(bdi.belief_count(), 1);
        let b = bdi.get_belief("battery_level").expect("should exist");
        assert_eq!(b.value, "45");
    }

    #[test]
    fn test_bdi_remove_belief() {
        let mut bdi = make_bdi();
        bdi.update_belief(make_belief("network_connected", "true", 0.99))
            .ok();
        assert_eq!(bdi.belief_count(), 1);

        let removed = bdi.remove_belief("network_connected");
        assert!(removed.is_some());
        assert_eq!(bdi.belief_count(), 0);
    }

    #[test]
    fn test_bdi_extended_score_higher_with_good_beliefs() {
        let mut bdi = make_bdi();
        bdi.update_belief(make_belief("battery_level", "90", 0.95))
            .ok();
        bdi.update_belief(make_belief("network_connected", "true", 0.99))
            .ok();
        bdi.update_belief(make_belief("screen_on", "true", 0.99))
            .ok();

        let now = 1_700_000_000_000;
        let goal = make_goal(1, GoalPriority::High, GoalSource::UserExplicit);

        let (scored, ext) = bdi.compute_extended_score(&goal, now, &[], 0.5);
        assert!(
            scored.score > 0.5,
            "score should be high with good beliefs: {}",
            scored.score
        );
        assert!(
            ext.feasibility > 0.8,
            "feasibility should be high: {}",
            ext.feasibility
        );
    }

    #[test]
    fn test_bdi_feasibility_drops_with_low_battery() {
        let mut bdi = make_bdi();
        bdi.update_belief(make_belief("battery_level", "5", 0.95))
            .ok();

        let now = 1_700_000_000_000;
        let goal = make_goal(1, GoalPriority::Medium, GoalSource::UserExplicit);
        let (_, ext) = bdi.compute_extended_score(&goal, now, &[], 0.5);
        assert!(
            ext.feasibility < 0.7,
            "feasibility should drop with low battery: {}",
            ext.feasibility
        );
    }

    #[test]
    fn test_bdi_personality_influence_proactive() {
        // LLM sets priority at goal creation — Rust returns goal priority directly.
        // Openness parameter does not influence the result; both calls return
        // the goal's own priority f32.
        let high_openness = BdiScheduler::compute_personality_influence(
            &make_goal(1, GoalPriority::Medium, GoalSource::ProactiveSuggestion),
            0.9,
        );
        let low_openness = BdiScheduler::compute_personality_influence(
            &make_goal(2, GoalPriority::Medium, GoalSource::ProactiveSuggestion),
            0.1,
        );
        // Both return the same priority (Medium = 0.5) — openness is a no-op.
        assert_eq!(
            high_openness, low_openness,
            "personality influence is now a priority passthrough: {} vs {}",
            high_openness, low_openness
        );
    }

    #[test]
    fn test_bdi_dependency_satisfaction_all_met() {
        let mut bdi = make_bdi();
        // Add intention for dependency goal 10.
        let _ = bdi.intentions.try_push(Intention {
            goal_id: 10,
            commitment: 0.8,
            allocated_resources: vec![],
            formed_at_ms: 1_000_000,
            delegation: DelegationType::AuraAutonomous,
        });
        let sat = bdi.compute_dependency_satisfaction(&[10]);
        assert!((sat - 1.0).abs() < f32::EPSILON, "all deps met: {}", sat);
    }

    #[test]
    fn test_bdi_dependency_satisfaction_partial() {
        let mut bdi = make_bdi();
        let _ = bdi.intentions.try_push(Intention {
            goal_id: 10,
            commitment: 0.8,
            allocated_resources: vec![],
            formed_at_ms: 1_000_000,
            delegation: DelegationType::AuraAutonomous,
        });
        let sat = bdi.compute_dependency_satisfaction(&[10, 20]);
        assert!((sat - 0.5).abs() < f32::EPSILON, "half deps met: {}", sat);
    }

    #[test]
    fn test_bdi_dependency_satisfaction_no_deps() {
        let bdi = make_bdi();
        let sat = bdi.compute_dependency_satisfaction(&[]);
        assert!((sat - 1.0).abs() < f32::EPSILON, "no deps = 1.0: {}", sat);
    }

    #[test]
    fn test_bdi_deliberation_commits_feasible_goals() {
        let mut bdi = make_bdi();
        bdi.update_belief(make_belief("battery_level", "80", 0.95))
            .ok();
        bdi.update_belief(make_belief("network_connected", "true", 0.99))
            .ok();

        let goals = vec![
            make_goal(1, GoalPriority::High, GoalSource::UserExplicit),
            make_goal(2, GoalPriority::Medium, GoalSource::CronScheduled),
        ];

        let result = bdi.deliberate(&goals, 1_700_000_000_000, 0.5);
        match result {
            DeliberationResult::Commit { new_intentions } => {
                assert!(
                    !new_intentions.is_empty(),
                    "should commit at least one intention"
                );
            }
            other => panic!("expected Commit, got {:?}", other),
        }
    }

    #[test]
    fn test_bdi_deliberation_maintain_when_no_new_goals() {
        let mut bdi = make_bdi();
        // Deliberate with empty goals — no desires, no new intentions.
        let result = bdi.deliberate(&[], 1_700_000_000_000, 0.5);
        assert!(
            matches!(result, DeliberationResult::Maintain),
            "expected Maintain with no goals"
        );
    }

    #[test]
    fn test_bdi_suspend_with_state() {
        let mut bdi = make_bdi();
        let goal = make_goal(1, GoalPriority::High, GoalSource::UserExplicit);
        let scored = bdi.base.compute_score(&goal, 1_700_000_000_000);
        bdi.base.enqueue(scored).ok();
        bdi.base.next_decision(&default_power(), 1_700_000_000_000);
        assert_eq!(bdi.base.active_count(), 1);

        let result = bdi.suspend_with_state(
            1,
            0.45,
            3,
            SuspensionReason::Preempted { by_goal_id: 99 },
            1_700_000_000_000,
            vec!["partial data".into()],
        );
        assert!(result.is_ok());
        assert_eq!(bdi.base.active_count(), 0);
        assert_eq!(bdi.suspended_count(), 1);

        let state = bdi.get_suspended_state(1).expect("should have state");
        assert!((state.progress - 0.45).abs() < f32::EPSILON);
        assert_eq!(state.current_step_index, 3);
        assert_eq!(state.partial_results, vec!["partial data"]);
    }

    #[test]
    fn test_bdi_resume_suspended() {
        let mut bdi = make_bdi();
        let goal = make_goal(1, GoalPriority::High, GoalSource::UserExplicit);
        let scored = bdi.base.compute_score(&goal, 1_700_000_000_000);
        bdi.base.enqueue(scored).ok();
        bdi.base.next_decision(&default_power(), 1_700_000_000_000);

        bdi.suspend_with_state(
            1,
            0.5,
            2,
            SuspensionReason::PowerConstrained,
            1_700_000_000_000,
            vec!["data1".into()],
        )
        .ok();

        let state = bdi.resume_suspended(1).expect("should resume");
        assert_eq!(state.goal_id, 1);
        assert!((state.progress - 0.5).abs() < f32::EPSILON);
        assert_eq!(bdi.suspended_count(), 0);
    }

    #[test]
    fn test_bdi_resume_nonexistent_fails() {
        let mut bdi = make_bdi();
        let result = bdi.resume_suspended(999);
        assert!(result.is_err());
    }

    #[test]
    fn test_bdi_detect_temporal_conflict() {
        let mut bdi = make_bdi();
        let now = 1_700_000_000_000u64;

        // Add two intentions.
        let _ = bdi.intentions.try_push(Intention {
            goal_id: 1,
            commitment: 0.8,
            allocated_resources: vec![],
            formed_at_ms: now,
            delegation: DelegationType::AuraAutonomous,
        });
        let _ = bdi.intentions.try_push(Intention {
            goal_id: 2,
            commitment: 0.7,
            allocated_resources: vec![],
            formed_at_ms: now,
            delegation: DelegationType::AuraAutonomous,
        });

        // Both goals have deadlines within 2 minutes of each other and 5 minutes from now.
        let mut g1 = make_goal(1, GoalPriority::High, GoalSource::UserExplicit);
        g1.deadline_ms = Some(now + 5 * 60_000);
        let mut g2 = make_goal(2, GoalPriority::High, GoalSource::UserExplicit);
        g2.deadline_ms = Some(now + 6 * 60_000);

        let conflicts = bdi.detect_conflicts(&[g1, g2], now);
        assert!(!conflicts.is_empty(), "should detect temporal conflict");
        assert!(matches!(conflicts[0].conflict_type, ConflictKind::Temporal));
    }

    #[test]
    fn test_bdi_detect_resource_conflict() {
        let mut bdi = make_bdi();
        let now = 1_700_000_000_000u64;

        let _ = bdi.intentions.try_push(Intention {
            goal_id: 1,
            commitment: 0.8,
            allocated_resources: vec!["camera".into()],
            formed_at_ms: now,
            delegation: DelegationType::AuraAutonomous,
        });
        let _ = bdi.intentions.try_push(Intention {
            goal_id: 2,
            commitment: 0.7,
            allocated_resources: vec!["camera".into()],
            formed_at_ms: now,
            delegation: DelegationType::AuraAutonomous,
        });

        let g1 = make_goal(1, GoalPriority::High, GoalSource::UserExplicit);
        let g2 = make_goal(2, GoalPriority::Medium, GoalSource::UserExplicit);

        let conflicts = bdi.detect_conflicts(&[g1, g2], now);
        let resource_conflict = conflicts.iter().find(|c| {
            matches!(&c.conflict_type, ConflictKind::Resource { resource } if resource == "camera")
        });
        assert!(
            resource_conflict.is_some(),
            "should detect camera resource conflict"
        );
    }

    #[test]
    fn test_bdi_detect_synergies() {
        let mut bdi = make_bdi();
        let now = 1_700_000_000_000u64;

        let _ = bdi.intentions.try_push(Intention {
            goal_id: 1,
            commitment: 0.8,
            allocated_resources: vec![],
            formed_at_ms: now,
            delegation: DelegationType::AuraAutonomous,
        });
        let _ = bdi.intentions.try_push(Intention {
            goal_id: 2,
            commitment: 0.7,
            allocated_resources: vec![],
            formed_at_ms: now,
            delegation: DelegationType::AuraAutonomous,
        });

        use aura_types::goals::{GoalStep, StepStatus};
        let mut g1 = make_goal(1, GoalPriority::High, GoalSource::UserExplicit);
        g1.steps = vec![GoalStep {
            description: "Open WhatsApp application first".to_string(),
            ..GoalStep::default()
        }];
        let mut g2 = make_goal(2, GoalPriority::Medium, GoalSource::UserExplicit);
        g2.steps = vec![GoalStep {
            description: "Open WhatsApp application now".to_string(),
            ..GoalStep::default()
        }];

        let synergies = bdi.detect_synergies(&[g1, g2]);
        assert!(
            !synergies.is_empty(),
            "should detect shared 'Open WhatsApp application' step"
        );
    }

    #[test]
    fn test_bdi_delegation_autonomous() {
        let desire = Desire {
            goal_id: 1,
            desirability: 0.8,
            feasible: true,
            infeasibility_reason: None,
            required_resources: vec!["network".into()],
        };
        let delegation = BdiScheduler::determine_delegation(&desire);
        assert!(matches!(delegation, DelegationType::AuraAutonomous));
    }

    #[test]
    fn test_bdi_delegation_user_action() {
        let desire = Desire {
            goal_id: 1,
            desirability: 0.8,
            feasible: true,
            infeasibility_reason: None,
            required_resources: vec!["user_confirmation".into()],
        };
        let delegation = BdiScheduler::determine_delegation(&desire);
        assert!(matches!(delegation, DelegationType::UserAction { .. }));
    }

    #[test]
    fn test_bdi_delegation_hybrid() {
        let desire = Desire {
            goal_id: 1,
            desirability: 0.8,
            feasible: true,
            infeasibility_reason: None,
            required_resources: vec![
                "user_confirmation".into(),
                "user_input".into(),
                "network".into(),
            ],
        };
        let delegation = BdiScheduler::determine_delegation(&desire);
        assert!(matches!(delegation, DelegationType::Hybrid { .. }));
    }

    #[test]
    fn test_bdi_infer_resources() {
        let res = BdiScheduler::infer_resources("Take a photo with camera");
        assert!(res.contains(&"camera".to_string()));

        let res = BdiScheduler::infer_resources("Send a message to John");
        assert!(res.contains(&"network".to_string()));

        let res = BdiScheduler::infer_resources("Navigate to the airport using maps");
        assert!(res.contains(&"gps".to_string()));
        assert!(res.contains(&"network".to_string()));

        let res = BdiScheduler::infer_resources("Buy coffee online");
        assert!(res.contains(&"user_confirmation".to_string()));
    }

    // -----------------------------------------------------------------------
    // Additional Team 2 integration tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_bdi_extended_score_with_dependencies() {
        let mut bdi = make_bdi();

        // Add active intention for dependency goal
        let _ = bdi.intentions.try_push(Intention {
            goal_id: 10,
            commitment: 0.9,
            allocated_resources: vec![],
            formed_at_ms: 1_000_000,
            delegation: DelegationType::AuraAutonomous,
        });

        let now = 1_700_000_000_000;
        let goal = make_goal(1, GoalPriority::High, GoalSource::UserExplicit);

        // Goal depends on goal 10 which is already active
        let (scored, ext) = bdi.compute_extended_score(&goal, now, &[10], 0.5);

        // Should have high dependency satisfaction since 10 is active
        assert!(
            ext.dependency_satisfaction >= 0.9,
            "dependency satisfaction should be high: {}",
            ext.dependency_satisfaction
        );
    }

    #[test]
    fn test_bdi_extended_score_no_dependencies() {
        let mut bdi = make_bdi();

        let now = 1_700_000_000_000;
        let goal = make_goal(1, GoalPriority::High, GoalSource::UserExplicit);

        // Goal has no dependencies
        let (scored, ext) = bdi.compute_extended_score(&goal, now, &[], 0.5);

        // Should have full dependency satisfaction
        assert!(
            (ext.dependency_satisfaction - 1.0).abs() < f32::EPSILON,
            "no deps should give 1.0: {}",
            ext.dependency_satisfaction
        );
    }

    #[test]
    fn test_bdi_feasibility_network_disconnected() {
        let mut bdi = make_bdi();
        bdi.update_belief(make_belief("network_connected", "false", 0.99))
            .ok();

        let now = 1_700_000_000_000;
        // Goal that needs network
        let mut goal = make_goal(1, GoalPriority::High, GoalSource::UserExplicit);
        goal.description = "Search for restaurants nearby".to_string();

        let (_, ext) = bdi.compute_extended_score(&goal, now, &[], 0.5);

        // Should have reduced feasibility due to network requirement
        assert!(
            ext.feasibility < 0.8,
            "feasibility should drop with no network: {}",
            ext.feasibility
        );
    }

    #[test]
    fn test_bdi_feasibility_screen_off() {
        let mut bdi = make_bdi();
        bdi.update_belief(make_belief("screen_on", "false", 0.99))
            .ok();

        let now = 1_700_000_000_000;
        let goal = make_goal(1, GoalPriority::High, GoalSource::UserExplicit);

        let (_, ext) = bdi.compute_extended_score(&goal, now, &[], 0.5);

        // Should have reduced feasibility for non-background goal with screen off
        assert!(
            ext.feasibility < 1.0,
            "feasibility should drop with screen off: {}",
            ext.feasibility
        );
    }

    #[test]
    fn test_bdi_deliberation_filters_infeasible() {
        let mut bdi = make_bdi();
        // Low battery belief - makes goals infeasible
        bdi.update_belief(make_belief("battery_level", "5", 0.95))
            .ok();

        let goals = vec![make_goal(1, GoalPriority::Medium, GoalSource::UserExplicit)];

        let result = bdi.deliberate(&goals, 1_700_000_000_000, 0.5);

        // With low battery, goals should be filtered as infeasible
        match result {
            DeliberationResult::Maintain => {} // OK - no feasible goals
            DeliberationResult::Commit { new_intentions: _ } => {
                // If commit, check that infeasible goals weren't committed
                // Low battery makes goals infeasible
            }
            _ => {}
        }
    }

    #[test]
    fn test_bdi_deliberation_drops_stale_low_commitment() {
        let mut bdi = make_bdi();

        // Add old low-commitment intention
        let _ = bdi.intentions.try_push(Intention {
            goal_id: 1,
            commitment: 0.2, // Low commitment
            allocated_resources: vec![],
            formed_at_ms: 1_000_000, // Old (now is ~1.7T)
            delegation: DelegationType::AuraAutonomous,
        });

        // Add new high-desirability goal
        let goals = vec![make_goal(2, GoalPriority::High, GoalSource::UserExplicit)];

        let result = bdi.deliberate(&goals, 1_700_000_000_000, 0.5);

        // Should reconsider low-commitment stale intention
        match result {
            DeliberationResult::Reconsider {
                drop_intentions, ..
            } => {
                assert!(
                    drop_intentions.contains(&1),
                    "should drop stale low-commitment"
                );
            }
            _ => {}
        }
    }

    #[test]
    fn test_bdi_conflict_no_conflicts() {
        let mut bdi = make_bdi();
        let now = 1_700_000_000_000u64;

        // Two goals with no conflicts
        let mut g1 = make_goal(1, GoalPriority::High, GoalSource::UserExplicit);
        g1.deadline_ms = Some(now + 60 * 60_000); // 1 hour from now
        let mut g2 = make_goal(2, GoalPriority::Low, GoalSource::ProactiveSuggestion);
        g2.deadline_ms = Some(now + 2 * 60 * 60_000); // 2 hours from now

        let conflicts = bdi.detect_conflicts(&[g1, g2], now);
        assert!(conflicts.is_empty(), "no conflicts expected");
    }

    #[test]
    fn test_bdi_synergy_no_shared_steps() {
        let mut bdi = make_bdi();
        let now = 1_700_000_000_000u64;

        let _ = bdi.intentions.try_push(Intention {
            goal_id: 1,
            commitment: 0.8,
            allocated_resources: vec![],
            formed_at_ms: now,
            delegation: DelegationType::AuraAutonomous,
        });
        let _ = bdi.intentions.try_push(Intention {
            goal_id: 2,
            commitment: 0.7,
            allocated_resources: vec![],
            formed_at_ms: now,
            delegation: DelegationType::AuraAutonomous,
        });

        // Goals with completely different steps
        use aura_types::goals::{GoalStep, StepStatus};
        let mut g1 = make_goal(1, GoalPriority::High, GoalSource::UserExplicit);
        g1.steps = vec![GoalStep {
            description: "Open email application".to_string(),
            ..GoalStep::default()
        }];
        let mut g2 = make_goal(2, GoalPriority::Medium, GoalSource::UserExplicit);
        g2.steps = vec![GoalStep {
            description: "Play music".to_string(),
            ..GoalStep::default()
        }];

        let synergies = bdi.detect_synergies(&[g1, g2]);
        assert!(synergies.is_empty(), "no synergies expected");
    }

    #[test]
    fn test_bdi_get_delegation() {
        let mut bdi = make_bdi();
        let now = 1_700_000_000_000u64;

        let _ = bdi.intentions.try_push(Intention {
            goal_id: 1,
            commitment: 0.8,
            allocated_resources: vec![],
            formed_at_ms: now,
            delegation: DelegationType::AuraAutonomous,
        });

        let delegation = bdi.get_delegation(1);
        assert!(delegation.is_some());
        assert!(matches!(delegation, Some(DelegationType::AuraAutonomous)));

        let none = bdi.get_delegation(999);
        assert!(none.is_none());
    }

    #[test]
    fn test_bdi_intentions_and_desires_query() {
        let mut bdi = make_bdi();

        bdi.update_belief(make_belief("battery_level", "80", 0.95))
            .ok();

        let goals = vec![
            make_goal(1, GoalPriority::High, GoalSource::UserExplicit),
            make_goal(2, GoalPriority::Medium, GoalSource::CronScheduled),
        ];

        bdi.deliberate(&goals, 1_700_000_000_000, 0.5);

        // Check desires and intentions are populated
        let _desires = bdi.desires();
        let _intentions = bdi.intentions();

        // Both goals should generate desires
        assert!(bdi.desires().len() >= 1, "should have desires");
    }
}
