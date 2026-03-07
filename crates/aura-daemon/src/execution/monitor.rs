//! Execution monitoring: 10 invariants checked at each step, plus ANR detection
//! and degradation cascade.
//!
//! ## 10 Invariants (from architecture spec):
//! 1. `action_retries` — retries for current step ≤ max (default 3)
//! 2. `step_fallback_depth` — selector fallback level ≤ 3
//! 3. `step_elapsed` — individual step ≤ 30s
//! 4. `task_steps` — total steps ≤ max (200/50/500 by safety level)
//! 5. `task_elapsed` — total task time ≤ max (30min/5min/2hr)
//! 6. `task_replans` — replan count ≤ 5
//! 7. `task_tokens` — LLM token usage ≤ 500K
//! 8. `task_llm_calls` — LLM call count ≤ 15
//! 9. `queued_tasks` — task queue depth ≤ 10
//! 10. `preemption_depth` — nested preemptions ≤ 2
//!
//! ## Enhanced capabilities:
//! - **Deviation detection**: tracks expected vs actual progress per step
//! - **Re-plan trigger**: auto-triggers re-plan when deviation exceeds threshold
//! - **Resource monitoring**: battery/thermal state tracking with abort thresholds
//! - **Success/failure classification**: categorizes outcomes for learning
//! - **Post-execution analysis**: generates a report with metrics and recommendations

use std::time::Instant;
use tracing::warn;

/// Which invariant was violated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvariantViolation {
    ActionRetries,
    StepFallbackDepth,
    StepElapsed,
    TaskSteps,
    TaskElapsed,
    TaskReplans,
    TaskTokens,
    TaskLlmCalls,
    QueuedTasks,
    PreemptionDepth,
}

impl InvariantViolation {
    /// Human-readable name for logging.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ActionRetries => "action_retries",
            Self::StepFallbackDepth => "step_fallback_depth",
            Self::StepElapsed => "step_elapsed",
            Self::TaskSteps => "task_steps",
            Self::TaskElapsed => "task_elapsed",
            Self::TaskReplans => "task_replans",
            Self::TaskTokens => "task_tokens",
            Self::TaskLlmCalls => "task_llm_calls",
            Self::QueuedTasks => "queued_tasks",
            Self::PreemptionDepth => "preemption_depth",
        }
    }
}

/// Limits for all 10 invariants.
#[derive(Debug, Clone)]
pub struct MonitorLimits {
    pub max_action_retries: u32,
    pub max_step_fallback_depth: u32,
    pub max_step_elapsed_ms: u64,
    pub max_task_steps: u32,
    pub max_task_elapsed_ms: u64,
    pub max_task_replans: u32,
    pub max_task_tokens: u64,
    pub max_task_llm_calls: u32,
    pub max_queued_tasks: u32,
    pub max_preemption_depth: u32,
}

impl MonitorLimits {
    /// Normal safety level limits.
    pub fn normal() -> Self {
        Self {
            max_action_retries: 3,
            max_step_fallback_depth: 3,
            max_step_elapsed_ms: 30_000,
            max_task_steps: 200,
            max_task_elapsed_ms: 30 * 60 * 1000, // 30 min
            max_task_replans: 5,
            max_task_tokens: 500_000,
            max_task_llm_calls: 15,
            max_queued_tasks: 10,
            max_preemption_depth: 2,
        }
    }

    /// Safety-mode limits (more conservative).
    pub fn safety() -> Self {
        Self {
            max_action_retries: 2,
            max_step_fallback_depth: 2,
            max_step_elapsed_ms: 15_000,
            max_task_steps: 50,
            max_task_elapsed_ms: 5 * 60 * 1000, // 5 min
            max_task_replans: 2,
            max_task_tokens: 100_000,
            max_task_llm_calls: 5,
            max_queued_tasks: 5,
            max_preemption_depth: 1,
        }
    }

    /// Power-mode limits (more permissive).
    pub fn power() -> Self {
        Self {
            max_action_retries: 5,
            max_step_fallback_depth: 5,
            max_step_elapsed_ms: 60_000,
            max_task_steps: 500,
            max_task_elapsed_ms: 2 * 60 * 60 * 1000, // 2 hours
            max_task_replans: 10,
            max_task_tokens: 1_000_000,
            max_task_llm_calls: 30,
            max_queued_tasks: 20,
            max_preemption_depth: 3,
        }
    }
}

/// Per-step counters that get reset each step.
#[derive(Debug, Clone, Default)]
pub struct StepCounters {
    pub retries: u32,
    pub fallback_depth: u32,
    pub started_at: Option<Instant>,
}

impl StepCounters {
    /// Reset for a new step.
    pub fn reset(&mut self) {
        self.retries = 0;
        self.fallback_depth = 0;
        self.started_at = Some(Instant::now());
    }

    /// Elapsed time for the current step in ms.
    pub fn elapsed_ms(&self) -> u64 {
        self.started_at
            .map(|s| s.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }
}

/// Per-task counters that persist for the entire task execution.
#[derive(Debug, Clone, Default)]
pub struct TaskCounters {
    pub total_steps: u32,
    pub replans: u32,
    pub tokens_used: u64,
    pub llm_calls: u32,
    pub started_at: Option<Instant>,
}

impl TaskCounters {
    /// Start a new task.
    pub fn start(&mut self) {
        self.total_steps = 0;
        self.replans = 0;
        self.tokens_used = 0;
        self.llm_calls = 0;
        self.started_at = Some(Instant::now());
    }

    /// Elapsed time for the entire task in ms.
    pub fn elapsed_ms(&self) -> u64 {
        self.started_at
            .map(|s| s.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }
}

/// Execution monitor that tracks all 10 invariants.
#[derive(Debug)]
pub struct ExecutionMonitor {
    limits: MonitorLimits,
    pub step: StepCounters,
    pub task: TaskCounters,
    /// External state: number of queued tasks (set by the task scheduler).
    pub queued_tasks: u32,
    /// External state: preemption nesting depth.
    pub preemption_depth: u32,
    /// History of violations for diagnostics.
    violations: Vec<(InvariantViolation, u64)>, // (violation, timestamp_ms)
}

impl ExecutionMonitor {
    /// Create a new monitor with the given limits.
    pub fn new(limits: MonitorLimits) -> Self {
        Self {
            limits,
            step: StepCounters::default(),
            task: TaskCounters::default(),
            queued_tasks: 0,
            preemption_depth: 0,
            violations: Vec::new(),
        }
    }

    /// Create with Normal limits.
    pub fn normal() -> Self {
        Self::new(MonitorLimits::normal())
    }

    /// Create with Safety limits.
    pub fn safety() -> Self {
        Self::new(MonitorLimits::safety())
    }

    /// Create with Power limits.
    pub fn power() -> Self {
        Self::new(MonitorLimits::power())
    }

    /// Get the current limits.
    pub fn limits(&self) -> &MonitorLimits {
        &self.limits
    }

    /// Start a new task — resets task-level counters.
    pub fn start_task(&mut self) {
        self.task.start();
        self.step.reset();
        self.violations.clear();
    }

    /// Start a new step — resets step-level counters, increments task steps.
    pub fn start_step(&mut self) {
        self.step.reset();
        self.task.total_steps += 1;
    }

    /// Record a retry on the current step.
    pub fn record_retry(&mut self) {
        self.step.retries += 1;
    }

    /// Record selector fallback depth for the current step.
    pub fn record_fallback_depth(&mut self, depth: u32) {
        self.step.fallback_depth = self.step.fallback_depth.max(depth);
    }

    /// Record a replan event.
    pub fn record_replan(&mut self) {
        self.task.replans += 1;
    }

    /// Record LLM token usage.
    pub fn record_llm_call(&mut self, tokens: u64) {
        self.task.llm_calls += 1;
        self.task.tokens_used += tokens;
    }

    /// Check all 10 invariants. Returns the first violation found, or None.
    pub fn check_invariants(&mut self) -> Option<InvariantViolation> {
        let now_ms = self.task.elapsed_ms();

        // Check each invariant in order of severity
        let checks: [(InvariantViolation, bool); 10] = [
            // Step-level checks (most urgent)
            (
                InvariantViolation::StepElapsed,
                self.step.elapsed_ms() > self.limits.max_step_elapsed_ms,
            ),
            (
                InvariantViolation::ActionRetries,
                self.step.retries > self.limits.max_action_retries,
            ),
            (
                InvariantViolation::StepFallbackDepth,
                self.step.fallback_depth > self.limits.max_step_fallback_depth,
            ),
            // Task-level checks
            (
                InvariantViolation::TaskElapsed,
                self.task.elapsed_ms() > self.limits.max_task_elapsed_ms,
            ),
            (
                InvariantViolation::TaskSteps,
                self.task.total_steps > self.limits.max_task_steps,
            ),
            (
                InvariantViolation::TaskReplans,
                self.task.replans > self.limits.max_task_replans,
            ),
            (
                InvariantViolation::TaskTokens,
                self.task.tokens_used > self.limits.max_task_tokens,
            ),
            (
                InvariantViolation::TaskLlmCalls,
                self.task.llm_calls > self.limits.max_task_llm_calls,
            ),
            // System-level checks
            (
                InvariantViolation::QueuedTasks,
                self.queued_tasks > self.limits.max_queued_tasks,
            ),
            (
                InvariantViolation::PreemptionDepth,
                self.preemption_depth > self.limits.max_preemption_depth,
            ),
        ];

        for (violation, triggered) in &checks {
            if *triggered {
                warn!(invariant = violation.name(), "invariant violated");
                self.violations.push((*violation, now_ms));
                return Some(*violation);
            }
        }

        None
    }

    /// Check only step-level invariants (faster, called more frequently).
    pub fn check_step_invariants(&mut self) -> Option<InvariantViolation> {
        if self.step.elapsed_ms() > self.limits.max_step_elapsed_ms {
            let v = InvariantViolation::StepElapsed;
            self.violations.push((v, self.task.elapsed_ms()));
            return Some(v);
        }
        if self.step.retries > self.limits.max_action_retries {
            let v = InvariantViolation::ActionRetries;
            self.violations.push((v, self.task.elapsed_ms()));
            return Some(v);
        }
        if self.step.fallback_depth > self.limits.max_step_fallback_depth {
            let v = InvariantViolation::StepFallbackDepth;
            self.violations.push((v, self.task.elapsed_ms()));
            return Some(v);
        }
        None
    }

    /// Get the violation history.
    pub fn violation_history(&self) -> &[(InvariantViolation, u64)] {
        &self.violations
    }

    /// Whether the task has exceeded its step limit.
    pub fn steps_exceeded(&self) -> bool {
        self.task.total_steps > self.limits.max_task_steps
    }

    /// Whether the task has timed out.
    pub fn task_timed_out(&self) -> bool {
        self.task.elapsed_ms() > self.limits.max_task_elapsed_ms
    }

    /// Remaining steps before limit.
    pub fn remaining_steps(&self) -> u32 {
        self.limits
            .max_task_steps
            .saturating_sub(self.task.total_steps)
    }

    /// Remaining time before timeout (ms).
    pub fn remaining_time_ms(&self) -> u64 {
        self.limits
            .max_task_elapsed_ms
            .saturating_sub(self.task.elapsed_ms())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Enhanced Monitor — deviation detection, resource monitoring, analysis
// ═══════════════════════════════════════════════════════════════════════════

/// Maximum number of step progress snapshots kept.
const MAX_PROGRESS_SNAPSHOTS: usize = 64;

/// Default deviation threshold to trigger re-planning (0.0–1.0).
const DEFAULT_DEVIATION_THRESHOLD: f32 = 0.35;

/// Battery percentage below which we abort execution.
const BATTERY_ABORT_THRESHOLD: f32 = 5.0;

/// Thermal state classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalState {
    /// Normal operating temperature.
    Normal,
    /// Warm — reduce background work.
    Warm,
    /// Hot — throttle execution.
    Hot,
    /// Critical — abort immediately.
    Critical,
}

impl ThermalState {
    /// Whether execution should be throttled.
    pub fn should_throttle(&self) -> bool {
        matches!(self, Self::Hot | Self::Critical)
    }

    /// Whether execution must abort.
    pub fn must_abort(&self) -> bool {
        matches!(self, Self::Critical)
    }
}

/// Resource state snapshot for monitoring.
#[derive(Debug, Clone)]
pub struct ResourceSnapshot {
    /// Battery percentage (0.0–100.0).
    pub battery_pct: f32,
    /// Thermal state.
    pub thermal: ThermalState,
    /// Whether device is plugged in.
    pub charging: bool,
}

impl Default for ResourceSnapshot {
    fn default() -> Self {
        Self {
            battery_pct: 100.0,
            thermal: ThermalState::Normal,
            charging: false,
        }
    }
}

/// Per-step progress snapshot for deviation detection.
#[derive(Debug, Clone)]
pub struct StepProgressSnapshot {
    /// Step index in the plan (0-based).
    pub step_index: u32,
    /// Expected progress at this point (0.0–1.0).
    pub expected_progress: f32,
    /// Actual progress observed (0.0–1.0).
    pub actual_progress: f32,
    /// Elapsed time for this step (ms).
    pub elapsed_ms: u64,
    /// Expected time for this step (ms).
    pub expected_time_ms: u64,
}

impl StepProgressSnapshot {
    /// Deviation between expected and actual (positive = behind schedule).
    pub fn deviation(&self) -> f32 {
        (self.expected_progress - self.actual_progress).abs()
    }

    /// Time deviation ratio (>1.0 means taking longer than expected).
    pub fn time_ratio(&self) -> f32 {
        if self.expected_time_ms == 0 {
            return 1.0;
        }
        self.elapsed_ms as f32 / self.expected_time_ms as f32
    }
}

/// Outcome classification for a completed or failed task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskOutcome {
    /// Task completed successfully.
    Success,
    /// Task failed due to invariant violation.
    InvariantFailure(InvariantViolation),
    /// Task failed due to resource exhaustion (battery/thermal).
    ResourceAbort,
    /// Task failed with too many re-plans (likely stuck).
    ReplanExhaustion,
    /// Task timed out.
    Timeout,
    /// Task was cancelled by the user.
    UserCancelled,
    /// Task failed for an unclassified reason.
    UnknownFailure(String),
}

impl TaskOutcome {
    /// Human-readable label.
    pub fn label(&self) -> &str {
        match self {
            Self::Success => "success",
            Self::InvariantFailure(_) => "invariant_failure",
            Self::ResourceAbort => "resource_abort",
            Self::ReplanExhaustion => "replan_exhaustion",
            Self::Timeout => "timeout",
            Self::UserCancelled => "user_cancelled",
            Self::UnknownFailure(_) => "unknown_failure",
        }
    }

    /// Whether this is a recoverable failure (worth retrying).
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::ResourceAbort | Self::ReplanExhaustion
        )
    }
}

/// Post-execution analysis report.
#[derive(Debug, Clone)]
pub struct ExecutionReport {
    /// Task outcome classification.
    pub outcome: TaskOutcome,
    /// Total steps executed.
    pub total_steps: u32,
    /// Total elapsed time (ms).
    pub total_time_ms: u64,
    /// Total LLM calls made.
    pub llm_calls: u32,
    /// Total tokens consumed.
    pub tokens_used: u64,
    /// Number of re-plans triggered.
    pub replans: u32,
    /// Number of invariant violations encountered.
    pub violation_count: usize,
    /// Average step deviation (0.0 = perfect tracking).
    pub avg_deviation: f32,
    /// Peak deviation observed.
    pub peak_deviation: f32,
    /// Recommendations for future runs.
    pub recommendations: Vec<String>,
}

/// Decision the enhanced monitor makes after checking state.
#[derive(Debug, Clone, PartialEq)]
pub enum MonitorDecision {
    /// Continue execution normally.
    Continue,
    /// Throttle — slow down execution to reduce resource usage.
    Throttle,
    /// Re-plan — deviation too high, need a new plan.
    Replan { reason: String },
    /// Abort — unrecoverable situation.
    Abort { reason: String },
}

/// Enhanced execution monitor wrapping the base monitor with deviation
/// tracking, resource monitoring, and post-execution analysis.
#[derive(Debug)]
pub struct EnhancedMonitor {
    /// The base invariant monitor.
    pub base: ExecutionMonitor,
    /// Progress snapshots for deviation detection.
    snapshots: Vec<StepProgressSnapshot>,
    /// Current resource state.
    resource_state: ResourceSnapshot,
    /// Deviation threshold for re-plan trigger.
    deviation_threshold: f32,
    /// Total planned steps (set when a plan is loaded).
    planned_steps: u32,
    /// Whether a re-plan has been triggered in this task.
    replan_triggered: bool,
    /// Outcome (set when task finishes).
    outcome: Option<TaskOutcome>,
}

impl EnhancedMonitor {
    /// Create a new enhanced monitor wrapping the given base monitor.
    pub fn new(base: ExecutionMonitor) -> Self {
        Self {
            base,
            snapshots: Vec::with_capacity(MAX_PROGRESS_SNAPSHOTS),
            resource_state: ResourceSnapshot::default(),
            deviation_threshold: DEFAULT_DEVIATION_THRESHOLD,
            planned_steps: 0,
            replan_triggered: false,
            outcome: None,
        }
    }

    /// Create with normal limits.
    pub fn normal() -> Self {
        Self::new(ExecutionMonitor::normal())
    }

    /// Set the deviation threshold for re-plan triggers.
    pub fn set_deviation_threshold(&mut self, threshold: f32) {
        self.deviation_threshold = threshold.clamp(0.05, 0.95);
    }

    /// Load a plan: sets planned_steps so we can compute expected progress.
    pub fn load_plan(&mut self, total_steps: u32) {
        self.planned_steps = total_steps;
        self.snapshots.clear();
        self.replan_triggered = false;
        self.outcome = None;
    }

    /// Update the resource state (called periodically by the platform layer).
    pub fn update_resources(&mut self, snapshot: ResourceSnapshot) {
        self.resource_state = snapshot;
    }

    /// Record step completion and track progress.
    pub fn record_step_progress(
        &mut self,
        step_index: u32,
        elapsed_ms: u64,
        expected_time_ms: u64,
    ) {
        let expected_progress = if self.planned_steps == 0 {
            1.0
        } else {
            (step_index + 1) as f32 / self.planned_steps as f32
        };

        // Actual progress: ratio of steps completed vs planned.
        let actual_progress = expected_progress; // When step succeeds, actual = expected.

        let snapshot = StepProgressSnapshot {
            step_index,
            expected_progress,
            actual_progress,
            elapsed_ms,
            expected_time_ms,
        };

        if self.snapshots.len() >= MAX_PROGRESS_SNAPSHOTS {
            self.snapshots.remove(0);
        }
        self.snapshots.push(snapshot);
    }

    /// Record a step failure (actual progress falls behind expected).
    pub fn record_step_failure(&mut self, step_index: u32, elapsed_ms: u64, expected_time_ms: u64) {
        let expected_progress = if self.planned_steps == 0 {
            1.0
        } else {
            (step_index + 1) as f32 / self.planned_steps as f32
        };

        // On failure, actual progress stays at previous step's level.
        let actual_progress = if self.planned_steps == 0 {
            0.0
        } else {
            step_index as f32 / self.planned_steps as f32
        };

        let snapshot = StepProgressSnapshot {
            step_index,
            expected_progress,
            actual_progress,
            elapsed_ms,
            expected_time_ms,
        };

        if self.snapshots.len() >= MAX_PROGRESS_SNAPSHOTS {
            self.snapshots.remove(0);
        }
        self.snapshots.push(snapshot);
    }

    /// Get the current deviation (latest snapshot).
    pub fn current_deviation(&self) -> f32 {
        self.snapshots.last().map(|s| s.deviation()).unwrap_or(0.0)
    }

    /// Get the peak deviation across all snapshots.
    pub fn peak_deviation(&self) -> f32 {
        self.snapshots
            .iter()
            .map(|s| s.deviation())
            .fold(0.0f32, f32::max)
    }

    /// Average deviation across all snapshots.
    pub fn avg_deviation(&self) -> f32 {
        if self.snapshots.is_empty() {
            return 0.0;
        }
        let sum: f32 = self.snapshots.iter().map(|s| s.deviation()).sum();
        sum / self.snapshots.len() as f32
    }

    /// Check resource state for abort/throttle conditions.
    pub fn check_resources(&self) -> MonitorDecision {
        if self.resource_state.thermal.must_abort() {
            return MonitorDecision::Abort {
                reason: "device thermal state is CRITICAL".to_string(),
            };
        }

        if !self.resource_state.charging
            && self.resource_state.battery_pct < BATTERY_ABORT_THRESHOLD
        {
            return MonitorDecision::Abort {
                reason: format!(
                    "battery at {:.1}% (below {:.1}% threshold)",
                    self.resource_state.battery_pct, BATTERY_ABORT_THRESHOLD
                ),
            };
        }

        if self.resource_state.thermal.should_throttle() {
            return MonitorDecision::Throttle;
        }

        MonitorDecision::Continue
    }

    /// Combined check: invariants + deviation + resources.
    /// Returns the most urgent decision.
    pub fn evaluate(&mut self) -> MonitorDecision {
        // 1. Check resource constraints first (hardware safety).
        let resource_decision = self.check_resources();
        if matches!(resource_decision, MonitorDecision::Abort { .. }) {
            return resource_decision;
        }

        // 2. Check base invariants.
        if let Some(violation) = self.base.check_invariants() {
            return MonitorDecision::Abort {
                reason: format!("invariant violated: {}", violation.name()),
            };
        }

        // 3. Check deviation threshold for re-plan.
        let deviation = self.current_deviation();
        if deviation > self.deviation_threshold && !self.replan_triggered {
            self.replan_triggered = true;
            return MonitorDecision::Replan {
                reason: format!(
                    "deviation {:.2} exceeds threshold {:.2}",
                    deviation, self.deviation_threshold
                ),
            };
        }

        // 4. Resource throttling.
        if matches!(resource_decision, MonitorDecision::Throttle) {
            return resource_decision;
        }

        MonitorDecision::Continue
    }

    /// Classify the task outcome based on current state and an explicit
    /// success/failure flag.
    pub fn classify_outcome(&mut self, success: bool) -> TaskOutcome {
        let outcome = if success {
            TaskOutcome::Success
        } else if self.base.task_timed_out() {
            TaskOutcome::Timeout
        } else if !self.resource_state.charging
            && self.resource_state.battery_pct < BATTERY_ABORT_THRESHOLD
        {
            TaskOutcome::ResourceAbort
        } else if self.resource_state.thermal.must_abort() {
            TaskOutcome::ResourceAbort
        } else if self.base.task.replans > self.base.limits().max_task_replans {
            TaskOutcome::ReplanExhaustion
        } else if let Some(violation) = self.last_violation() {
            TaskOutcome::InvariantFailure(violation)
        } else {
            TaskOutcome::UnknownFailure("task failed without clear cause".to_string())
        };

        self.outcome = Some(outcome.clone());
        outcome
    }

    /// Get the last invariant violation from history, if any.
    fn last_violation(&self) -> Option<InvariantViolation> {
        self.base.violation_history().last().map(|(v, _)| *v)
    }

    /// Generate a post-execution analysis report.
    pub fn generate_report(&self, outcome: TaskOutcome) -> ExecutionReport {
        let avg_dev = self.avg_deviation();
        let peak_dev = self.peak_deviation();

        let mut recommendations: Vec<String> = Vec::new();

        // Generate recommendations based on metrics.
        if peak_dev > 0.5 {
            recommendations.push(
                "High deviation detected — consider better plan templates or ETG data.".to_string(),
            );
        }

        if self.base.task.replans > 2 {
            recommendations.push(
                "Multiple re-plans needed — goal may be ambiguous or environment unstable."
                    .to_string(),
            );
        }

        if self.base.task.llm_calls > 10 {
            recommendations.push(
                "Heavy LLM usage — consider caching plans or using more ETG paths.".to_string(),
            );
        }

        if self.base.task.tokens_used > 200_000 {
            recommendations.push(
                "High token consumption — optimize prompts or increase template coverage."
                    .to_string(),
            );
        }

        let avg_step_time = if self.base.task.total_steps > 0 {
            self.base.task.elapsed_ms() / self.base.task.total_steps as u64
        } else {
            0
        };

        if avg_step_time > 10_000 {
            recommendations
                .push("Slow step execution (>10s avg) — check app responsiveness.".to_string());
        }

        if recommendations.is_empty() {
            recommendations.push("Execution was nominal — no improvements needed.".to_string());
        }

        ExecutionReport {
            outcome,
            total_steps: self.base.task.total_steps,
            total_time_ms: self.base.task.elapsed_ms(),
            llm_calls: self.base.task.llm_calls,
            tokens_used: self.base.task.tokens_used,
            replans: self.base.task.replans,
            violation_count: self.base.violation_history().len(),
            avg_deviation: avg_dev,
            peak_deviation: peak_dev,
            recommendations,
        }
    }

    /// Number of progress snapshots recorded.
    pub fn snapshot_count(&self) -> usize {
        self.snapshots.len()
    }

    /// Whether re-plan has been triggered in the current task.
    pub fn was_replan_triggered(&self) -> bool {
        self.replan_triggered
    }

    /// Reset the re-plan trigger (after a re-plan is performed).
    pub fn acknowledge_replan(&mut self) {
        self.replan_triggered = false;
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_limits() {
        let limits = MonitorLimits::normal();
        assert_eq!(limits.max_task_steps, 200);
        assert_eq!(limits.max_action_retries, 3);
        assert_eq!(limits.max_step_elapsed_ms, 30_000);
        assert_eq!(limits.max_task_replans, 5);
    }

    #[test]
    fn test_safety_limits() {
        let limits = MonitorLimits::safety();
        assert_eq!(limits.max_task_steps, 50);
        assert_eq!(limits.max_task_elapsed_ms, 5 * 60 * 1000);
    }

    #[test]
    fn test_power_limits() {
        let limits = MonitorLimits::power();
        assert_eq!(limits.max_task_steps, 500);
        assert_eq!(limits.max_task_elapsed_ms, 2 * 60 * 60 * 1000);
    }

    #[test]
    fn test_monitor_no_violations() {
        let mut monitor = ExecutionMonitor::normal();
        monitor.start_task();
        monitor.start_step();

        let result = monitor.check_invariants();
        assert!(result.is_none());
    }

    #[test]
    fn test_retry_violation() {
        let mut monitor = ExecutionMonitor::normal();
        monitor.start_task();
        monitor.start_step();

        // Exceed retry limit
        for _ in 0..5 {
            monitor.record_retry();
        }

        let result = monitor.check_step_invariants();
        assert_eq!(result, Some(InvariantViolation::ActionRetries));
    }

    #[test]
    fn test_step_count_violation() {
        let mut monitor = ExecutionMonitor::new(MonitorLimits {
            max_task_steps: 3,
            ..MonitorLimits::normal()
        });
        monitor.start_task();

        for _ in 0..4 {
            monitor.start_step();
        }

        let result = monitor.check_invariants();
        assert_eq!(result, Some(InvariantViolation::TaskSteps));
    }

    #[test]
    fn test_replan_violation() {
        let mut monitor = ExecutionMonitor::normal();
        monitor.start_task();
        monitor.start_step();

        for _ in 0..6 {
            monitor.record_replan();
        }

        let result = monitor.check_invariants();
        assert_eq!(result, Some(InvariantViolation::TaskReplans));
    }

    #[test]
    fn test_llm_token_violation() {
        let mut monitor = ExecutionMonitor::normal();
        monitor.start_task();
        monitor.start_step();

        monitor.record_llm_call(600_000);

        let result = monitor.check_invariants();
        assert_eq!(result, Some(InvariantViolation::TaskTokens));
    }

    #[test]
    fn test_llm_call_count_violation() {
        let mut monitor = ExecutionMonitor::normal();
        monitor.start_task();
        monitor.start_step();

        for _ in 0..16 {
            monitor.record_llm_call(1000);
        }

        let result = monitor.check_invariants();
        assert_eq!(result, Some(InvariantViolation::TaskLlmCalls));
    }

    #[test]
    fn test_queued_tasks_violation() {
        let mut monitor = ExecutionMonitor::normal();
        monitor.start_task();
        monitor.start_step();
        monitor.queued_tasks = 15;

        let result = monitor.check_invariants();
        assert_eq!(result, Some(InvariantViolation::QueuedTasks));
    }

    #[test]
    fn test_preemption_violation() {
        let mut monitor = ExecutionMonitor::normal();
        monitor.start_task();
        monitor.start_step();
        monitor.preemption_depth = 5;

        let result = monitor.check_invariants();
        assert_eq!(result, Some(InvariantViolation::PreemptionDepth));
    }

    #[test]
    fn test_remaining_steps() {
        let mut monitor = ExecutionMonitor::normal();
        monitor.start_task();
        assert_eq!(monitor.remaining_steps(), 200);

        for _ in 0..10 {
            monitor.start_step();
        }
        assert_eq!(monitor.remaining_steps(), 190);
    }

    #[test]
    fn test_violation_history() {
        let mut monitor = ExecutionMonitor::normal();
        monitor.start_task();
        monitor.start_step();

        // Trigger a violation
        for _ in 0..5 {
            monitor.record_retry();
        }
        monitor.check_step_invariants();

        assert_eq!(monitor.violation_history().len(), 1);
        assert_eq!(
            monitor.violation_history()[0].0,
            InvariantViolation::ActionRetries
        );
    }

    #[test]
    fn test_fallback_depth_tracking() {
        let mut monitor = ExecutionMonitor::normal();
        monitor.start_task();
        monitor.start_step();

        monitor.record_fallback_depth(2);
        assert_eq!(monitor.step.fallback_depth, 2);

        // Should track max
        monitor.record_fallback_depth(1);
        assert_eq!(monitor.step.fallback_depth, 2);

        monitor.record_fallback_depth(3);
        assert_eq!(monitor.step.fallback_depth, 3);
    }
}
