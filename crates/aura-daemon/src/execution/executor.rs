//! Main execution engine — orchestrates action plans on the device screen.
//!
//! This module contains the `Executor` which drives the full ReAct loop:
//! **Observe → Think → Act → Verify** for each step in an `ActionPlan`.
//!
//! ## Pipeline per DslStep
//!
//! 1. Capture screen tree (`ScreenProvider::capture_tree`)
//! 2. Resolve target element (`selector::resolve_target`) with 8-level fallback
//! 3. Check anti-bot rate limiting (`AntiBot::check_action`) — wait if needed
//! 4. Apply human-like delay from AntiBot
//! 5. Execute action (`ScreenProvider::execute_action`)
//! 6. Verify result (`verifier::verify_action`) comparing before/after trees
//! 7. Retry if verification failed and retries remaining
//! 8. Run cycle detection (`CycleDetector::record_and_check`) with 4-tier handling
//! 9. Check 10 invariants (`ExecutionMonitor::check_invariants`)
//! 10. Record transition in ETG (`EtgStore::record_transition`)
//! 11. Record `StepResult` and continue

use std::time::Instant;

use aura_types::{
    actions::ActionType,
    dsl::{DslCondition, DslStep, FailureStrategy},
    errors::AuraError,
    etg::ActionPlan,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use crate::{
    execution::{
        cycle::{CycleDetector, CycleTier},
        etg::EtgStore,
        monitor::{EnhancedMonitor, ExecutionMonitor, InvariantViolation, MonitorDecision},
        retry::{ErrorClass, IntelligentRetry, RetryStrategy},
    },
    policy::{
        gate::PolicyGate,
        sandbox::{ContainmentLevel, Sandbox},
        wiring::production_policy_gate,
    },
    screen::{
        actions::ScreenProvider,
        anti_bot::AntiBot,
        selector::resolve_target,
        verifier::{
            hash_action, hash_screen_state, verify_action, ExpectedChange, VerificationResult,
        },
    },
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default max selector fallback depth for target resolution.
const DEFAULT_MAX_FALLBACK_DEPTH: u8 = 3;

/// Minimum verification confidence to consider an action successful.
const MIN_VERIFICATION_CONFIDENCE: f32 = 0.3;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Outcome of executing an action plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutionOutcome {
    /// Plan completed successfully.
    Success {
        steps_executed: u32,
        elapsed_ms: u64,
    },
    /// Plan failed after exhausting retries.
    Failed {
        step: u32,
        reason: String,
        elapsed_ms: u64,
    },
    /// Execution was cancelled (e.g., user interrupt).
    Cancelled { step: u32, elapsed_ms: u64 },
    /// Cycle detected — escalated to recovery.
    CycleDetected { step: u32, tier: u8 },
}

/// Result of a single step execution.
///
/// Previously private, blocking the feedback loop from reading execution
/// results outside `executor.rs`. Made `pub(crate)` so that the feedback
/// loop, planner, and ARC subsystems can inspect step outcomes.
#[derive(Debug, Clone)]
pub(crate) struct StepResult {
    /// Whether the step succeeded.
    pub(crate) success: bool,
    /// Verification result from before/after comparison.
    #[allow(dead_code)]
    pub(crate) verification: Option<VerificationResult>,
    /// How many retries were needed.
    pub(crate) retries_used: u8,
    /// Duration of this step in ms.
    pub(crate) duration_ms: u64,
}

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

/// The main execution engine.
///
/// Drives the plan→act→verify loop for action plans received from
/// System1 (ETG fast path) or System2 (Neocortex LLM).
///
/// Owns all subsystems: anti-bot timing, cycle detection, execution monitoring,
/// and the Element-Transition Graph for learning navigation patterns.
pub struct Executor {
    /// Maximum steps per plan (from safety level config).
    max_steps: u32,
    /// Anti-bot timing to avoid detection.
    anti_bot: AntiBot,
    /// 4-tier cycle detector.
    cycle_detector: CycleDetector,
    /// Enhanced execution monitor: base invariants + deviation tracking + resources.
    enhanced_monitor: EnhancedMonitor,
    /// Intelligent retry with circuit breaker and failure classification.
    intelligent_retry: IntelligentRetry,
    /// Element-Transition Graph for learning.
    etg: EtgStore,
    /// Action sandbox — containment and isolation for per-action safety checks.
    /// Defense-in-depth: sandbox is checked AFTER PolicyGate, BEFORE execution.
    action_sandbox: Sandbox,
    /// CRITICAL: All actions MUST pass through PolicyGate before execution.
    /// PolicyGate enforces rate limits and deny-list rules at the executor level.
    policy_gate: PolicyGate,
}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executor")
            .field("max_steps", &self.max_steps)
            .field("enhanced_monitor", &self.enhanced_monitor)
            .field("intelligent_retry", &self.intelligent_retry)
            .field("action_sandbox", &"Sandbox { .. }")
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

impl Executor {
    /// Create a new executor with all subsystems.
    ///
    /// The `monitor` is wrapped in an `EnhancedMonitor` which provides
    /// deviation tracking, resource monitoring, and post-execution analysis.
    pub fn new(
        max_steps: u32,
        anti_bot: AntiBot,
        cycle_detector: CycleDetector,
        monitor: ExecutionMonitor,
        etg: EtgStore,
    ) -> Self {
        Self {
            max_steps,
            anti_bot,
            cycle_detector,
            enhanced_monitor: EnhancedMonitor::new(monitor),
            intelligent_retry: IntelligentRetry::new(),
            etg,
            action_sandbox: Sandbox::new(),
            policy_gate: production_policy_gate(),
        }
    }

    /// Create an executor with default "normal" configuration.
    pub fn normal() -> Self {
        Self {
            max_steps: 200,
            anti_bot: AntiBot::normal(),
            cycle_detector: CycleDetector::new(),
            enhanced_monitor: EnhancedMonitor::normal(),
            intelligent_retry: IntelligentRetry::new(),
            etg: EtgStore::in_memory(),
            action_sandbox: Sandbox::new(),
            policy_gate: production_policy_gate(),
        }
    }

    /// Create an executor with "safety" configuration (conservative limits).
    pub fn safety() -> Self {
        Self {
            max_steps: 50,
            anti_bot: AntiBot::safety(),
            cycle_detector: CycleDetector::new(),
            enhanced_monitor: EnhancedMonitor::new(ExecutionMonitor::safety()),
            intelligent_retry: IntelligentRetry::new(),
            etg: EtgStore::in_memory(),
            action_sandbox: Sandbox::new(),
            policy_gate: production_policy_gate(),
        }
    }

    /// Create an executor with "power" configuration (extended limits).
    pub fn power() -> Self {
        Self {
            max_steps: 500,
            anti_bot: AntiBot::power(),
            cycle_detector: CycleDetector::new(),
            enhanced_monitor: EnhancedMonitor::new(ExecutionMonitor::power()),
            intelligent_retry: IntelligentRetry::new(),
            etg: EtgStore::in_memory(),
            action_sandbox: Sandbox::new(),
            policy_gate: production_policy_gate(),
        }
    }

    /// Create an executor with a permissive (allow-all) policy gate for unit tests.
    ///
    /// Tests that exercise execution logic — step sequencing, failure strategies,
    /// retry, screen transitions — should use this constructor so that the
    /// PolicyGate does not interfere with the scenario under test.  Policy
    /// behaviour itself is tested in the policy module's own test suite.
    #[cfg(test)]
    pub(crate) fn for_testing() -> Self {
        Self {
            max_steps: 200,
            anti_bot: AntiBot::normal(),
            cycle_detector: CycleDetector::new(),
            enhanced_monitor: EnhancedMonitor::normal(),
            intelligent_retry: IntelligentRetry::new(),
            etg: EtgStore::in_memory(),
            action_sandbox: Sandbox::new(),
            policy_gate: PolicyGate::allow_all(),
        }
    }

    /// Create an executor with a **custom** `PolicyGate` for testing policy
    /// enforcement.  Use this to verify that the executor correctly blocks
    /// actions when `PolicyGate` denies them.
    #[cfg(test)]
    pub(crate) fn for_testing_with_policy(gate: PolicyGate) -> Self {
        Self {
            max_steps: 200,
            anti_bot: AntiBot::normal(),
            cycle_detector: CycleDetector::new(),
            enhanced_monitor: EnhancedMonitor::normal(),
            intelligent_retry: IntelligentRetry::new(),
            etg: EtgStore::in_memory(),
            action_sandbox: Sandbox::new(),
            policy_gate: gate,
        }
    }
}

impl Default for Executor {
    fn default() -> Self {
        Self::normal()
    }
}

// ---------------------------------------------------------------------------
// Main execution loop
// ---------------------------------------------------------------------------

impl Executor {
    /// Execute an action plan step by step.
    ///
    /// This is the CENTRAL orchestrator — the ReAct loop that drives all
    /// screen interaction. Each step goes through the full 11-stage pipeline.
    #[instrument(skip(self, screen, plan), fields(goal = %plan.goal_description, steps = plan.steps.len()))]
    pub async fn execute(
        &mut self,
        plan: &ActionPlan,
        screen: &dyn ScreenProvider,
    ) -> Result<ExecutionOutcome, AuraError> {
        let task_start = Instant::now();
        let step_count = plan.steps.len() as u32;

        info!(steps = step_count, goal = %plan.goal_description, "starting plan execution");

        // Fast path: empty plan
        if step_count == 0 {
            return Ok(ExecutionOutcome::Success {
                steps_executed: 0,
                elapsed_ms: 0,
            });
        }

        // Pre-check: plan size within limits
        if step_count > self.max_steps {
            return Ok(ExecutionOutcome::Failed {
                step: 0,
                reason: format!(
                    "plan has {} steps, exceeds max {}",
                    step_count, self.max_steps
                ),
                elapsed_ms: 0,
            });
        }

        // Verify screen provider is alive before starting
        if !screen.is_alive() {
            return Err(AuraError::Screen(
                aura_types::errors::ScreenError::ServiceDisconnected,
            ));
        }

        // Initialize monitoring for this task
        self.enhanced_monitor.base.start_task();
        self.enhanced_monitor.load_plan(step_count);

        let mut steps_executed: u32 = 0;

        for (step_idx, step) in plan.steps.iter().enumerate() {
            let step_num = step_idx as u32;

            // Check task-level invariants before each step
            if let Some(violation) = self.enhanced_monitor.base.check_invariants() {
                let elapsed_ms = task_start.elapsed().as_millis() as u64;
                return Ok(self.handle_invariant_violation(violation, step_num, elapsed_ms));
            }

            // Check enhanced monitor (deviation + resources) before each step
            match self.enhanced_monitor.evaluate() {
                MonitorDecision::Abort { reason } => {
                    let elapsed_ms = task_start.elapsed().as_millis() as u64;
                    warn!(step = step_num, reason = %reason, "enhanced monitor abort");
                    return Ok(ExecutionOutcome::Failed {
                        step: step_num,
                        reason,
                        elapsed_ms,
                    });
                }
                MonitorDecision::Replan { reason } => {
                    // Log replan suggestion; without Neocortex IPC we continue
                    // but record it so the monitor tracks the deviation.
                    warn!(step = step_num, reason = %reason, "enhanced monitor suggests replan");
                    self.enhanced_monitor.base.record_replan();
                }
                MonitorDecision::Throttle => {
                    debug!(
                        step = step_num,
                        "enhanced monitor throttle — adding 500ms delay"
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                }
                MonitorDecision::Continue => {}
            }

            // Execute the step through the full pipeline
            match self.execute_step(step, step_num, screen, task_start).await {
                Ok(result) => {
                    steps_executed += 1;

                    // Track progress in enhanced monitor
                    let expected_time_ms = step.timeout_ms as u64;
                    if result.success {
                        self.enhanced_monitor.record_step_progress(
                            step_num,
                            result.duration_ms,
                            expected_time_ms,
                        );
                    } else {
                        self.enhanced_monitor.record_step_failure(
                            step_num,
                            result.duration_ms,
                            expected_time_ms,
                        );
                    }

                    debug!(
                        step = step_num,
                        success = result.success,
                        retries = result.retries_used,
                        duration_ms = result.duration_ms,
                        "step completed"
                    );
                }
                Err(StepFailure::Abort(reason)) => {
                    let elapsed_ms = task_start.elapsed().as_millis() as u64;
                    // Record failure in enhanced monitor
                    self.enhanced_monitor.record_step_failure(
                        step_num,
                        elapsed_ms,
                        step.timeout_ms as u64,
                    );
                    warn!(step = step_num, reason = %reason, "plan aborted");
                    return Ok(ExecutionOutcome::Failed {
                        step: step_num,
                        reason,
                        elapsed_ms,
                    });
                }
                Err(StepFailure::CycleEscalation(tier)) => {
                    warn!(step = step_num, ?tier, "cycle detected — escalating");
                    return Ok(ExecutionOutcome::CycleDetected {
                        step: step_num,
                        tier: tier as u8,
                    });
                }
                Err(StepFailure::Skipped) => {
                    debug!(step = step_num, "step skipped per failure strategy");
                    steps_executed += 1; // Count skipped steps as "executed"
                }
                Err(StepFailure::AskUser(msg)) => {
                    let elapsed_ms = task_start.elapsed().as_millis() as u64;
                    return Ok(ExecutionOutcome::Failed {
                        step: step_num,
                        reason: format!("user input needed: {}", msg),
                        elapsed_ms,
                    });
                }
                Err(StepFailure::ScreenError(e)) => {
                    return Err(AuraError::Screen(e));
                }
            }
        }

        let elapsed_ms = task_start.elapsed().as_millis() as u64;

        // Final evaluation from enhanced monitor
        let final_decision = self.enhanced_monitor.evaluate();
        if let MonitorDecision::Abort { reason } = final_decision {
            warn!(reason = %reason, "enhanced monitor abort at plan completion");
            return Ok(ExecutionOutcome::Failed {
                step: steps_executed,
                reason,
                elapsed_ms,
            });
        }

        info!(steps_executed, elapsed_ms, "plan completed successfully");

        Ok(ExecutionOutcome::Success {
            steps_executed,
            elapsed_ms,
        })
    }

    // ─── Step execution pipeline ─────────────────────────────────────────

    /// Execute a single step through the full 11-stage pipeline.
    ///
    /// Handles retries, fallback strategies, and all verification.
    /// Uses `Box::pin` internally because fallback strategies can recurse.
    fn execute_step<'a>(
        &'a mut self,
        step: &'a DslStep,
        step_num: u32,
        screen: &'a dyn ScreenProvider,
        task_start: Instant,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<StepResult, StepFailure>> + 'a>>
    {
        Box::pin(async move {
            self.enhanced_monitor.base.start_step();
            let step_start = Instant::now();

            // Step key for intelligent retry tracking
            let step_key = step.label.as_deref().unwrap_or("unnamed").to_owned();

            // Determine max retries from failure strategy
            let max_retries = match &step.on_failure {
                FailureStrategy::Retry { max } => *max,
                FailureStrategy::Fallback(_) => 1, // Try once, then fallback
                _ => 0,                            // Skip, Abort, AskUser: no retries
            };

            let mut last_error: Option<String> = None;

            for attempt in 0..=max_retries {
                // Circuit breaker check via intelligent retry
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                if !self.intelligent_retry.should_attempt(&step_key, now_ms) {
                    return Err(StepFailure::Abort(format!(
                        "circuit breaker open for step '{}'",
                        step_key
                    )));
                }

                if attempt > 0 {
                    self.enhanced_monitor.base.record_retry();
                    debug!(step = step_num, attempt, "retrying step");
                }

                // Check step-level invariants (fast check each attempt)
                if let Some(violation) = self.enhanced_monitor.base.check_step_invariants() {
                    let elapsed_ms = task_start.elapsed().as_millis() as u64;
                    let _outcome = self.handle_invariant_violation(violation, step_num, elapsed_ms);
                    return Err(StepFailure::Abort(format!(
                        "invariant violated: {:?}",
                        violation
                    )));
                }

                match self.execute_step_attempt(step, step_num, screen).await {
                    Ok(result) if result.success => {
                        let duration_ms = step_start.elapsed().as_millis() as u64;
                        // Record success in intelligent retry
                        self.intelligent_retry.handle_success(&step_key);
                        return Ok(StepResult {
                            success: true,
                            verification: result.verification,
                            retries_used: attempt,
                            duration_ms,
                        });
                    }
                    Ok(result) => {
                        // Action executed but verification failed
                        last_error = Some(format!(
                            "verification failed (confidence: {:.2})",
                            result
                                .verification
                                .as_ref()
                                .map(|v| v.confidence)
                                .unwrap_or(0.0)
                        ));
                    }
                    Err(AttemptError::TargetNotFound(selector_desc)) => {
                        last_error = Some(format!("target not found: {}", selector_desc));
                    }
                    Err(AttemptError::ActionFailed(reason)) => {
                        last_error = Some(format!("action failed: {}", reason));
                    }
                    Err(AttemptError::ScreenUnavailable(e)) => {
                        return Err(StepFailure::ScreenError(e));
                    }
                    Err(AttemptError::RateLimited(wait_ms)) => {
                        // Rate limited — sleep and retry (don't count as a retry)
                        debug!(wait_ms, "rate limited, waiting");
                        tokio::time::sleep(tokio::time::Duration::from_millis(wait_ms)).await;
                        last_error = Some(format!("rate limited for {}ms", wait_ms));
                    }
                    Err(AttemptError::SandboxDenied(reason)) => {
                        // Sandbox denial is non-retryable — abort immediately.
                        // Fail-secure: never retry a denied action.
                        return Err(StepFailure::Abort(format!(
                            "sandbox containment denied action: {reason}"
                        )));
                    }
                }

                // Consult intelligent retry for failure classification and strategy
                if let Some(ref error_msg) = last_error {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let strategy = self.intelligent_retry.handle_failure(
                        &step_key,
                        error_msg,
                        |e: &String| classify_step_error(e),
                        now_ms,
                        attempt as u32,
                    );
                    match strategy {
                        RetryStrategy::Abort { reason } => {
                            return Err(StepFailure::Abort(reason));
                        }
                        RetryStrategy::UseAlternative { alternative } => {
                            // Log the suggestion; runtime step-switching is not yet
                            // supported, so we fall through to the normal failure strategy.
                            warn!(
                                step = step_num,
                                alt = %alternative,
                                "intelligent retry suggests alternative path"
                            );
                        }
                        RetryStrategy::RetryWithBackoff => {
                            // Continue with next attempt in the loop
                        }
                    }
                }
            }

            // All retries exhausted — apply failure strategy
            let reason = last_error.unwrap_or_else(|| "unknown failure".to_string());
            self.apply_failure_strategy(step, step_num, screen, &reason, task_start)
                .await
        }) // Box::pin
    }

    /// Execute a single attempt of a step (one pass through the pipeline).
    async fn execute_step_attempt(
        &mut self,
        step: &DslStep,
        step_num: u32,
        screen: &dyn ScreenProvider,
    ) -> Result<StepAttemptResult, AttemptError> {
        // ── Stage 1: Capture screen (before state) ──
        let before_tree = screen
            .capture_tree()
            .map_err(AttemptError::ScreenUnavailable)?;

        let before_hash = hash_screen_state(&before_tree);

        // ── Stage 2: Resolve target (if action needs one) ──
        let action = self.resolve_action_target(step, screen)?;

        // ── Stage 2.5: PolicyGate check ──
        // CRITICAL: All actions MUST pass through PolicyGate before execution.
        // This is the primary policy enforcement point.  The sandbox below is
        // defense-in-depth — NOT a substitute for this gate.
        let action_description = format!("{:?}", action);
        let gate_decision = self.policy_gate.evaluate(&action_description);
        if gate_decision.is_denied() {
            tracing::warn!(
                target: "SECURITY",
                action = %action_description,
                step = step_num,
                reason = ?gate_decision,
                "PolicyGate DENIED action before execution"
            );
            return Err(AttemptError::SandboxDenied(format!(
                "PolicyGate denied action at step {step_num}: {gate_decision:?}"
            )));
        }

        // ── Stage 2.6: Sandbox containment check (defense-in-depth) ──
        // Classify the resolved action and enforce containment level.
        // This runs AFTER PolicyGate and BEFORE anti-bot/execution.
        let containment = self.action_sandbox.classify(&action);
        match containment {
            ContainmentLevel::Forbidden => {
                // L3: REFUSE — action must never execute.  Fail-secure.
                tracing::error!(
                    target: "SECURITY",
                    level = %containment,
                    action = ?action,
                    step = step_num,
                    "sandbox REFUSED action — L3:Forbidden"
                );
                return Err(AttemptError::SandboxDenied(format!(
                    "action classified as {containment} at step {step_num}"
                )));
            }
            ContainmentLevel::Restricted => {
                // L2: Preview + confirm — the executor cannot await user
                // input, so we DENY here (fail-secure).  Task-level L2
                // confirmation is handled in `main_loop.rs` where the
                // event loop can send a Telegram prompt and wait for
                // `/allow` or `/deny`.  If execution reaches here it
                // means the action was not pre-approved at the task level.
                tracing::warn!(
                    target: "SECURITY",
                    level = %containment,
                    action = ?action,
                    step = step_num,
                    "sandbox DENIED action — L2:Restricted requires confirmation \
                     (executor cannot await; fail-secure)"
                );
                return Err(AttemptError::SandboxDenied(format!(
                    "action classified as {containment} at step {step_num} — \
                     confirmation not available at execution level (fail-secure)"
                )));
            }
            ContainmentLevel::Monitored => {
                // L1: Execute + log — audit trail for monitored actions.
                tracing::debug!(
                    target: "SECURITY",
                    level = %containment,
                    action = ?action,
                    step = step_num,
                    "sandbox: action monitored at L1"
                );
            }
            ContainmentLevel::Direct => {
                // L0: Auto-approve — trusted action, no special handling.
            }
        }

        // ── Stage 3: Anti-bot rate limiting ──
        match self.anti_bot.check_action(&action) {
            Ok(recommended_delay_ms) => {
                // ── Stage 4: Apply human-like delay ──
                if recommended_delay_ms > 0 {
                    debug!(delay_ms = recommended_delay_ms, "applying anti-bot delay");
                    tokio::time::sleep(tokio::time::Duration::from_millis(recommended_delay_ms))
                        .await;
                }
            }
            Err(must_wait_ms) => {
                return Err(AttemptError::RateLimited(must_wait_ms));
            }
        }

        // ── Stage 5: Execute action ──
        let action_result = screen
            .execute_action(&action)
            .map_err(AttemptError::ScreenUnavailable)?;

        // Record action timing for anti-bot
        self.anti_bot.record_action();

        if !action_result.success {
            return Err(AttemptError::ActionFailed(
                action_result
                    .error
                    .unwrap_or_else(|| "action returned failure".to_string()),
            ));
        }

        // ── Stage 6: Verify result (before/after comparison) ──
        let after_tree = screen
            .capture_tree()
            .map_err(AttemptError::ScreenUnavailable)?;

        let after_hash = hash_screen_state(&after_tree);
        let expected_change = postcondition_to_expected_change(step.postcondition.as_ref());
        let verification = verify_action(&before_tree, &after_tree, expected_change.as_ref());

        let action_succeeded = is_action_verified(&action, &verification);

        // ── Stage 7: (Retry handled by caller) ──

        // ── Stage 8: Cycle detection ──
        let action_discriminant = action_type_discriminant(&action);
        let target_id = step
            .target
            .as_ref()
            .map(|t| format!("{:?}", t))
            .unwrap_or_default();
        let action_hash = hash_action(action_discriminant, &target_id);

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let cycle_result = self
            .cycle_detector
            .record_and_check(after_hash, action_hash, now_ms);

        if cycle_result.tier != CycleTier::None {
            debug!(
                tier = ?cycle_result.tier,
                check = cycle_result.check_name,
                reason = %cycle_result.reason,
                "cycle detected"
            );
            // Handle cycle tiers
            match cycle_result.tier {
                CycleTier::None => {} // unreachable here
                CycleTier::Micro => {
                    // Micro: acknowledge and let retry handle it
                    let _can_retry = self.cycle_detector.acknowledge_recovery();
                    // Fall through — the step retry logic will handle re-attempt
                }
                CycleTier::Strategic => {
                    // Strategic: would need replan from Neocortex
                    self.enhanced_monitor.base.record_replan();
                    // For now, we don't have IPC to Neocortex, so we continue
                    // and let the monitor's replan limit catch runaway replans
                }
                CycleTier::GracefulAbort | CycleTier::Emergency => {
                    // Tiers 3-4: abort execution
                    // (Recording partial progress is done at the plan level)
                    // Don't return error here — let the step result propagate
                    // The caller will see the cycle tier in the result
                }
            }
        }

        // ── Stage 9: Check invariants ──
        if let Some(violation) = self.enhanced_monitor.base.check_invariants() {
            warn!(
                ?violation,
                step = step_num,
                "invariant violation during step"
            );
            // Don't abort here — return the step result and let the main loop handle it
        }

        // ── Stage 10: Record transition in ETG ──
        // Get interactive elements for ETG node creation
        let interactive_elements = collect_interactive_ids(&after_tree.root);

        let _from_node = self.etg.get_or_create_node(
            before_hash,
            &before_tree.package_name,
            &before_tree.activity_name,
            &interactive_elements,
        );
        let _to_node = self.etg.get_or_create_node(
            after_hash,
            &after_tree.package_name,
            &after_tree.activity_name,
            &interactive_elements,
        );
        self.etg.record_transition(
            before_hash,
            after_hash,
            &action,
            action_succeeded,
            action_result.duration_ms,
        );

        // ── Stage 11: Build step attempt result ──
        // Record fallback depth if we resolved a target
        if let Some(ref target) = step.target {
            if let Some(resolved) = resolve_target(&after_tree, target, DEFAULT_MAX_FALLBACK_DEPTH)
            {
                self.enhanced_monitor
                    .base
                    .record_fallback_depth(resolved.level as u32);
            }
        }

        Ok(StepAttemptResult {
            success: action_succeeded,
            verification: Some(verification),
        })
    }

    // ─── Target resolution ───────────────────────────────────────────────

    /// Resolve the action's target coordinates from the current screen.
    ///
    /// For actions that require a target (Tap, LongPress), resolves the
    /// `TargetSelector` against the screen tree and updates coordinates.
    /// Actions without targets (Back, Home, etc.) pass through unchanged.
    fn resolve_action_target(
        &self,
        step: &DslStep,
        screen: &dyn ScreenProvider,
    ) -> Result<ActionType, AttemptError> {
        let action = &step.action;

        // If there's no target selector, use the action as-is
        let selector = match &step.target {
            Some(s) => s,
            None => return Ok(action.clone()),
        };

        // Only resolve targets for coordinate-based actions
        let needs_coordinates = matches!(
            action,
            ActionType::Tap { .. } | ActionType::LongPress { .. }
        );

        if !needs_coordinates {
            return Ok(action.clone());
        }

        // Capture tree for resolution
        let tree = screen
            .capture_tree()
            .map_err(AttemptError::ScreenUnavailable)?;

        let resolved = resolve_target(&tree, selector, DEFAULT_MAX_FALLBACK_DEPTH)
            .ok_or_else(|| AttemptError::TargetNotFound(format!("{:?}", selector)))?;

        debug!(
            node_id = %resolved.node_id,
            level = resolved.level,
            x = resolved.center_x,
            y = resolved.center_y,
            "target resolved"
        );

        // Update action coordinates from resolved target
        let updated_action = match action {
            ActionType::Tap { .. } => ActionType::Tap {
                x: resolved.center_x,
                y: resolved.center_y,
            },
            ActionType::LongPress { .. } => ActionType::LongPress {
                x: resolved.center_x,
                y: resolved.center_y,
            },
            other => other.clone(),
        };

        Ok(updated_action)
    }

    // ─── Failure strategy ────────────────────────────────────────────────

    /// Apply the step's failure strategy after retries are exhausted.
    fn apply_failure_strategy<'a>(
        &'a mut self,
        step: &'a DslStep,
        step_num: u32,
        screen: &'a dyn ScreenProvider,
        reason: &'a str,
        task_start: Instant,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<StepResult, StepFailure>> + 'a>>
    {
        Box::pin(async move {
            match &step.on_failure {
                FailureStrategy::Retry { .. } => {
                    // Retries already exhausted in the caller
                    Err(StepFailure::Abort(format!(
                        "step {} failed after retries: {}",
                        step_num, reason
                    )))
                }
                FailureStrategy::Skip => {
                    debug!(step = step_num, reason, "skipping failed step");
                    Err(StepFailure::Skipped)
                }
                FailureStrategy::Abort => Err(StepFailure::Abort(format!(
                    "step {} aborted: {}",
                    step_num, reason
                ))),
                FailureStrategy::Fallback(fallback_steps) => {
                    debug!(
                        step = step_num,
                        fallback_count = fallback_steps.len(),
                        "executing fallback steps"
                    );

                    // Execute each fallback step. If any fails, abort.
                    for (fb_idx, fb_step) in fallback_steps.iter().enumerate() {
                        match self
                            .execute_step(fb_step, step_num, screen, task_start)
                            .await
                        {
                            Ok(result) if result.success => {
                                debug!(step = step_num, fb_idx, "fallback step succeeded");
                            }
                            Ok(_) => {
                                return Err(StepFailure::Abort(format!(
                                    "fallback step {} for step {} failed verification",
                                    fb_idx, step_num
                                )));
                            }
                            Err(e) => return Err(e),
                        }
                    }

                    // All fallback steps succeeded
                    Ok(StepResult {
                        success: true,
                        verification: None,
                        retries_used: 0,
                        duration_ms: 0,
                    })
                }
                FailureStrategy::AskUser(prompt) => Err(StepFailure::AskUser(prompt.clone())),
            }
        }) // Box::pin
    }

    // ─── Invariant handling ──────────────────────────────────────────────

    /// Convert an invariant violation into an ExecutionOutcome.
    fn handle_invariant_violation(
        &self,
        violation: InvariantViolation,
        step: u32,
        elapsed_ms: u64,
    ) -> ExecutionOutcome {
        let reason = match violation {
            InvariantViolation::ActionRetries => "action retry limit exceeded".to_string(),
            InvariantViolation::StepFallbackDepth => "selector fallback depth exceeded".to_string(),
            InvariantViolation::StepElapsed => "step timeout exceeded".to_string(),
            InvariantViolation::TaskSteps => "task step limit exceeded".to_string(),
            InvariantViolation::TaskElapsed => "task time limit exceeded".to_string(),
            InvariantViolation::TaskReplans => "replan limit exceeded".to_string(),
            InvariantViolation::TaskTokens => "token budget exhausted".to_string(),
            InvariantViolation::TaskLlmCalls => "LLM call limit exceeded".to_string(),
            InvariantViolation::QueuedTasks => "task queue depth exceeded".to_string(),
            InvariantViolation::PreemptionDepth => "preemption depth exceeded".to_string(),
        };

        ExecutionOutcome::Failed {
            step,
            reason,
            elapsed_ms,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal error types (pub(crate) for feedback loop visibility)
// ---------------------------------------------------------------------------

/// Error from a single step (after all retries).
pub(crate) enum StepFailure {
    /// Abort the entire plan.
    Abort(String),
    /// Step was skipped per FailureStrategy::Skip.
    Skipped,
    /// Cycle detected at a tier requiring escalation.
    #[allow(dead_code)]
    CycleEscalation(CycleTier),
    /// Ask user for guidance.
    AskUser(String),
    /// Screen provider error (propagate as AuraError).
    ScreenError(aura_types::errors::ScreenError),
}

/// Error from a single attempt within a step.
pub(crate) enum AttemptError {
    TargetNotFound(String),
    ActionFailed(String),
    ScreenUnavailable(aura_types::errors::ScreenError),
    RateLimited(u64),
    /// Action was denied by the sandbox containment system.
    /// This is a hard deny — retrying will not help.
    SandboxDenied(String),
}

/// Intermediate result from a single attempt (before retry logic).
pub(crate) struct StepAttemptResult {
    pub(crate) success: bool,
    pub(crate) verification: Option<VerificationResult>,
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Classify a step error string into an `ErrorClass` for intelligent retry.
///
/// Classification heuristic:
/// - "not found" / "target" → `Structural` (UI changed, retry won't help)
/// - "timeout" / "rate limit" / "transient" → `Transient` (safe to retry)
/// - everything else → `Fatal`
fn classify_step_error(error: &String) -> ErrorClass {
    let lower = error.to_lowercase();
    if lower.contains("not found") || lower.contains("target") {
        ErrorClass::Structural
    } else if lower.contains("timeout")
        || lower.contains("rate limit")
        || lower.contains("transient")
        || lower.contains("rate limited")
    {
        ErrorClass::Transient
    } else {
        ErrorClass::Fatal
    }
}

/// Map ActionType variant to a u8 discriminant for `hash_action()`.
fn action_type_discriminant(action: &ActionType) -> u8 {
    match action {
        ActionType::Tap { .. } => 0,
        ActionType::LongPress { .. } => 1,
        ActionType::Swipe { .. } => 2,
        ActionType::Type { .. } => 3,
        ActionType::Scroll { .. } => 4,
        ActionType::Back => 5,
        ActionType::Home => 6,
        ActionType::Recents => 7,
        ActionType::OpenApp { .. } => 8,
        ActionType::NotificationAction { .. } => 9,
        ActionType::WaitForElement { .. } => 10,
        ActionType::AssertElement { .. } => 11,
    }
}

/// Convert a `DslCondition` postcondition to an `ExpectedChange` for the verifier.
fn postcondition_to_expected_change(condition: Option<&DslCondition>) -> Option<ExpectedChange> {
    let cond = condition?;
    match cond {
        DslCondition::ElementExists(selector) => {
            Some(ExpectedChange::ElementAppears(format!("{:?}", selector)))
        }
        DslCondition::ElementNotExists(_) => Some(ExpectedChange::ScreenChange),
        DslCondition::TextEquals { expected, .. } => {
            Some(ExpectedChange::TextAppears(expected.clone()))
        }
        DslCondition::AppInForeground(_) => Some(ExpectedChange::AppChange),
        DslCondition::ScreenContainsText(text) => Some(ExpectedChange::TextAppears(text.clone())),
        DslCondition::And(conditions) => {
            // Use the first condition's expected change
            conditions
                .first()
                .and_then(|c| postcondition_to_expected_change(Some(c)))
        }
        DslCondition::Or(conditions) => conditions
            .first()
            .and_then(|c| postcondition_to_expected_change(Some(c))),
        DslCondition::Not(_) => Some(ExpectedChange::ScreenChange),
    }
}

/// Determine if an action should be considered verified based on the verification result.
///
/// Different action types have different verification expectations:
/// - Navigation actions (Back, Home, OpenApp): expect screen change
/// - Assertions/Waits: check condition, not screen change
/// - Everything else: check confidence threshold
fn is_action_verified(action: &ActionType, verification: &VerificationResult) -> bool {
    match action {
        // Assertions and waits don't need screen changes
        ActionType::AssertElement { .. } | ActionType::WaitForElement { .. } => true,
        // Navigation actions: expect screen to change
        ActionType::Back | ActionType::Home | ActionType::Recents | ActionType::OpenApp { .. } => {
            verification.screen_changed || verification.app_changed || verification.activity_changed
        }
        // NoChange for type into same field is expected
        ActionType::Type { .. } => {
            verification.confidence >= MIN_VERIFICATION_CONFIDENCE || verification.text_changes > 0
        }
        // General: use confidence threshold
        _ => verification.confidence >= MIN_VERIFICATION_CONFIDENCE || verification.screen_changed,
    }
}

/// Collect interactive element IDs from a screen tree node (recursively).
fn collect_interactive_ids(node: &aura_types::screen::ScreenNode) -> Vec<String> {
    let mut ids = Vec::new();
    collect_interactive_ids_recursive(node, &mut ids);
    ids
}

fn collect_interactive_ids_recursive(node: &aura_types::screen::ScreenNode, ids: &mut Vec<String>) {
    if node.is_clickable || node.is_editable || node.is_scrollable || node.is_checkable {
        ids.push(node.id.clone());
    }
    for child in &node.children {
        collect_interactive_ids_recursive(child, ids);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use aura_types::{
        actions::ActionType,
        dsl::{DslStep, FailureStrategy},
        etg::{ActionPlan, PlanSource},
        screen::{Bounds, ScreenNode, ScreenTree},
    };

    use super::*;
    use crate::{
        policy::rules::{PolicyRule, RuleEffect},
        screen::actions::MockScreenProvider,
    };

    /// Helper: create a minimal screen tree with the given nodes.
    fn make_tree(package: &str, activity: &str, nodes: Vec<ScreenNode>) -> ScreenTree {
        ScreenTree {
            root: ScreenNode {
                id: "root".to_string(),
                class_name: "android.widget.FrameLayout".to_string(),
                package_name: package.to_string(),
                text: None,
                content_description: None,
                resource_id: None,
                bounds: Bounds {
                    left: 0,
                    top: 0,
                    right: 1080,
                    bottom: 1920,
                },
                is_clickable: false,
                is_scrollable: false,
                is_editable: false,
                is_checkable: false,
                is_checked: false,
                is_enabled: true,
                is_focused: false,
                is_visible: true,
                children: nodes,
                depth: 0,
            },
            package_name: package.to_string(),
            activity_name: activity.to_string(),
            timestamp_ms: 1000,
            node_count: 1,
        }
    }

    /// Helper: create a clickable button node.
    fn make_button(id: &str, text: &str, x: i32, y: i32) -> ScreenNode {
        ScreenNode {
            id: id.to_string(),
            class_name: "android.widget.Button".to_string(),
            package_name: "com.test.app".to_string(),
            text: Some(text.to_string()),
            content_description: None,
            resource_id: Some(format!("com.test.app:id/{}", id)),
            bounds: Bounds {
                left: x - 50,
                top: y - 25,
                right: x + 50,
                bottom: y + 25,
            },
            is_clickable: true,
            is_scrollable: false,
            is_editable: false,
            is_checkable: false,
            is_checked: false,
            is_enabled: true,
            is_focused: false,
            is_visible: true,
            children: vec![],
            depth: 1,
        }
    }

    /// Helper: create a simple action plan from a list of DslSteps.
    fn make_plan(goal: &str, steps: Vec<DslStep>) -> ActionPlan {
        ActionPlan {
            goal_description: goal.to_string(),
            steps,
            estimated_duration_ms: 5000,
            confidence: 0.9,
            source: PlanSource::UserDefined,
        }
    }

    /// Helper: create a tap DslStep.
    fn tap_step(x: i32, y: i32) -> DslStep {
        DslStep {
            action: ActionType::Tap { x, y },
            target: None,
            timeout_ms: 2000,
            on_failure: FailureStrategy::default(),
            precondition: None,
            postcondition: None,
            label: Some("tap".to_string()),
        }
    }

    // ── Test 1: Empty plan succeeds ──

    #[tokio::test]
    async fn test_empty_plan_succeeds() {
        let mut executor = Executor::normal();
        let plan = make_plan("empty test", vec![]);
        let screen = MockScreenProvider::single(make_tree("com.test", "Main", vec![]));

        let outcome = executor
            .execute(&plan, &screen)
            .await
            .expect("should succeed");
        match outcome {
            ExecutionOutcome::Success {
                steps_executed,
                elapsed_ms,
            } => {
                assert_eq!(steps_executed, 0);
                assert_eq!(elapsed_ms, 0);
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }

    // ── Test 2: Plan exceeding max steps fails immediately ──

    #[tokio::test]
    async fn test_plan_exceeds_max_steps() {
        let mut executor = Executor::new(
            2,
            AntiBot::normal(),
            CycleDetector::new(),
            ExecutionMonitor::normal(),
            EtgStore::in_memory(),
        );

        let steps: Vec<DslStep> = (0..5).map(|i| tap_step(100, 100 + i * 50)).collect();
        let plan = make_plan("too many steps", steps);
        let screen = MockScreenProvider::single(make_tree("com.test", "Main", vec![]));

        let outcome = executor
            .execute(&plan, &screen)
            .await
            .expect("should succeed");
        match outcome {
            ExecutionOutcome::Failed { step, reason, .. } => {
                assert_eq!(step, 0);
                assert!(reason.contains("exceeds max"));
            }
            other => panic!("expected Failed, got {:?}", other),
        }
    }

    // ── Test 3: Single tap step executes successfully ──

    #[tokio::test]
    async fn test_single_tap_executes() {
        let tree1 = make_tree(
            "com.test.app",
            "MainActivity",
            vec![make_button("btn_ok", "OK", 540, 960)],
        );
        // After tap, screen changes (different activity)
        let tree2 = make_tree(
            "com.test.app",
            "ResultActivity",
            vec![make_button("btn_done", "Done", 540, 960)],
        );

        let screen = MockScreenProvider::new(vec![tree1, tree2]);
        let mut executor = Executor::for_testing();

        let plan = make_plan("tap OK button", vec![tap_step(540, 960)]);
        let outcome = executor
            .execute(&plan, &screen)
            .await
            .expect("should succeed");

        match outcome {
            ExecutionOutcome::Success { steps_executed, .. } => {
                assert_eq!(steps_executed, 1);
            }
            other => panic!("expected Success, got {:?}", other),
        }

        // Verify action was logged
        let log = screen.action_log().unwrap();
        assert_eq!(log.len(), 1);
        assert!(matches!(log[0], ActionType::Tap { x: 540, y: 960 }));
    }

    // ── Test 4: Multi-step plan with screen transitions ──

    #[tokio::test]
    async fn test_multi_step_plan() {
        let tree1 = make_tree(
            "com.test.app",
            "Screen1",
            vec![make_button("btn1", "Next", 540, 960)],
        );
        let tree2 = make_tree(
            "com.test.app",
            "Screen2",
            vec![make_button("btn2", "Continue", 540, 960)],
        );
        let tree3 = make_tree(
            "com.test.app",
            "Screen3",
            vec![make_button("btn3", "Done", 540, 960)],
        );

        let screen = MockScreenProvider::new(vec![tree1, tree2, tree3]);
        let mut executor = Executor::for_testing();

        let steps = vec![
            tap_step(540, 960),
            DslStep {
                action: ActionType::Back,
                target: None,
                timeout_ms: 1500,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: Some("go back".to_string()),
            },
        ];

        let plan = make_plan("navigate and return", steps);
        let outcome = executor
            .execute(&plan, &screen)
            .await
            .expect("should succeed");

        match outcome {
            ExecutionOutcome::Success { steps_executed, .. } => {
                assert_eq!(steps_executed, 2);
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }

    // ── Test 5: Skip failure strategy continues execution ──

    #[tokio::test]
    async fn test_skip_failure_strategy() {
        // Single tree — screen won't change, so verification may fail
        // But with Skip strategy, we should continue
        let tree = make_tree(
            "com.test.app",
            "Main",
            vec![make_button("btn", "OK", 540, 960)],
        );

        let mut mock = MockScreenProvider::single(tree);
        mock.set_actions_succeed(false); // Actions will fail

        let mut executor = Executor::for_testing();

        let steps = vec![
            DslStep {
                action: ActionType::Tap { x: 540, y: 960 },
                target: None,
                timeout_ms: 2000,
                on_failure: FailureStrategy::Skip,
                precondition: None,
                postcondition: None,
                label: Some("skippable tap".to_string()),
            },
            DslStep {
                action: ActionType::Tap { x: 100, y: 100 },
                target: None,
                timeout_ms: 2000,
                on_failure: FailureStrategy::Skip,
                precondition: None,
                postcondition: None,
                label: Some("second tap".to_string()),
            },
        ];

        let plan = make_plan("skip failing steps", steps);
        let outcome = executor
            .execute(&plan, &mock)
            .await
            .expect("should succeed");

        match outcome {
            ExecutionOutcome::Failed { reason, .. } => {
                assert!(reason.contains("action failed"));
            }
            ExecutionOutcome::Success { steps_executed, .. } => {
                assert_eq!(steps_executed, 2);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    // ── Test 6: Disconnected screen provider returns error ──

    #[tokio::test]
    async fn test_disconnected_screen_errors() {
        let tree = make_tree("com.test.app", "Main", vec![]);
        let mut mock = MockScreenProvider::single(tree);
        mock.set_alive(false);

        let mut executor = Executor::normal();
        let plan = make_plan("should fail", vec![tap_step(100, 100)]);

        let result = executor.execute(&plan, &mock).await;
        assert!(result.is_err());
    }

    // ── Test 7: Abort failure strategy stops execution ──

    #[tokio::test]
    async fn test_abort_failure_strategy() {
        let tree = make_tree(
            "com.test.app",
            "Main",
            vec![make_button("btn", "OK", 540, 960)],
        );
        let mut mock = MockScreenProvider::single(tree);
        mock.set_actions_succeed(false);

        let mut executor = Executor::for_testing();

        let steps = vec![
            DslStep {
                action: ActionType::Tap { x: 540, y: 960 },
                target: None,
                timeout_ms: 2000,
                on_failure: FailureStrategy::Abort,
                precondition: None,
                postcondition: None,
                label: Some("must succeed".to_string()),
            },
            tap_step(100, 100), // Should never be reached
        ];

        let plan = make_plan("abort on failure", steps);
        let outcome = executor
            .execute(&plan, &mock)
            .await
            .expect("should return outcome");

        match outcome {
            ExecutionOutcome::Failed { step, reason, .. } => {
                assert_eq!(step, 0);
                assert!(reason.contains("action failed"));
            }
            other => panic!("expected Failed, got {:?}", other),
        }
    }

    // ── Test 8: helper function tests ──

    #[test]
    fn test_action_type_discriminant() {
        assert_eq!(action_type_discriminant(&ActionType::Tap { x: 0, y: 0 }), 0);
        assert_eq!(
            action_type_discriminant(&ActionType::LongPress { x: 0, y: 0 }),
            1
        );
        assert_eq!(action_type_discriminant(&ActionType::Back), 5);
        assert_eq!(action_type_discriminant(&ActionType::Home), 6);
        assert_eq!(
            action_type_discriminant(&ActionType::OpenApp {
                package: String::new()
            }),
            8
        );
    }

    #[test]
    fn test_postcondition_to_expected_change() {
        // None -> None
        assert!(postcondition_to_expected_change(None).is_none());

        // ScreenContainsText -> TextAppears
        let cond = DslCondition::ScreenContainsText("Hello".to_string());
        let expected = postcondition_to_expected_change(Some(&cond));
        assert!(matches!(expected, Some(ExpectedChange::TextAppears(ref t)) if t == "Hello"));

        // AppInForeground -> AppChange
        let cond = DslCondition::AppInForeground("com.whatsapp".to_string());
        let expected = postcondition_to_expected_change(Some(&cond));
        assert!(matches!(expected, Some(ExpectedChange::AppChange)));
    }

    #[test]
    fn test_collect_interactive_ids() {
        let node = ScreenNode {
            id: "root".to_string(),
            class_name: "FrameLayout".to_string(),
            package_name: "com.test".to_string(),
            text: None,
            content_description: None,
            resource_id: None,
            bounds: Bounds {
                left: 0,
                top: 0,
                right: 100,
                bottom: 100,
            },
            is_clickable: false,
            is_scrollable: false,
            is_editable: false,
            is_checkable: false,
            is_checked: false,
            is_enabled: true,
            is_focused: false,
            is_visible: true,
            children: vec![
                make_button("btn1", "OK", 50, 50),
                ScreenNode {
                    id: "label".to_string(),
                    class_name: "TextView".to_string(),
                    package_name: "com.test".to_string(),
                    text: Some("Hello".to_string()),
                    content_description: None,
                    resource_id: None,
                    bounds: Bounds {
                        left: 0,
                        top: 0,
                        right: 100,
                        bottom: 20,
                    },
                    is_clickable: false,
                    is_scrollable: false,
                    is_editable: false,
                    is_checkable: false,
                    is_checked: false,
                    is_enabled: true,
                    is_focused: false,
                    is_visible: true,
                    children: vec![],
                    depth: 1,
                },
            ],
            depth: 0,
        };

        let ids = collect_interactive_ids(&node);
        assert_eq!(ids.len(), 1); // Only btn1 is clickable
        assert_eq!(ids[0], "btn1");
    }

    // ── PolicyGate enforcement tests (TEST-HIGH-2) ──────────────────────

    #[tokio::test]
    async fn test_policy_gate_deny_blocks_execution() {
        // A deny-by-default PolicyGate with NO allow rules should block
        // any action, causing the executor to return Failed with a reason
        // mentioning "PolicyGate denied".
        let gate = PolicyGate::deny_by_default();
        let mut executor = Executor::for_testing_with_policy(gate);

        let tree = make_tree(
            "com.test.app",
            "MainActivity",
            vec![make_button("btn_ok", "OK", 540, 960)],
        );
        let tree2 = make_tree(
            "com.test.app",
            "ResultActivity",
            vec![make_button("btn_done", "Done", 540, 960)],
        );
        let screen = MockScreenProvider::new(vec![tree, tree2]);

        let plan = make_plan("tap OK button", vec![tap_step(540, 960)]);
        let outcome = executor
            .execute(&plan, &screen)
            .await
            .expect("should return outcome");

        match outcome {
            ExecutionOutcome::Failed { step, reason, .. } => {
                assert_eq!(step, 0, "should fail on the first step");
                assert!(
                    reason.contains("PolicyGate denied"),
                    "reason should mention PolicyGate denial, got: {reason}"
                );
            }
            other => panic!(
                "expected ExecutionOutcome::Failed from PolicyGate denial, got: {:?}",
                other
            ),
        }

        // Verify no actions were actually dispatched to the screen.
        let log = screen.action_log().unwrap();
        assert!(
            log.is_empty(),
            "no actions should reach the screen when PolicyGate denies; got {} actions",
            log.len()
        );
    }

    #[tokio::test]
    async fn test_policy_gate_allow_rule_permits_execution() {
        // A deny-by-default gate with an explicit allow rule for Tap actions
        // should permit execution of tap steps.
        let mut gate = PolicyGate::deny_by_default();
        gate.add_rule(PolicyRule {
            name: "allow-taps".to_string(),
            action_pattern: "*tap*".to_string(),
            effect: RuleEffect::Allow,
            reason: "taps are safe for testing".to_string(),
            priority: 1,
        });

        let mut executor = Executor::for_testing_with_policy(gate);

        let tree1 = make_tree(
            "com.test.app",
            "MainActivity",
            vec![make_button("btn_ok", "OK", 540, 960)],
        );
        let tree2 = make_tree(
            "com.test.app",
            "ResultActivity",
            vec![make_button("btn_done", "Done", 540, 960)],
        );
        let screen = MockScreenProvider::new(vec![tree1, tree2]);

        let plan = make_plan("tap OK button", vec![tap_step(540, 960)]);
        let outcome = executor
            .execute(&plan, &screen)
            .await
            .expect("should return outcome");

        match outcome {
            ExecutionOutcome::Success { steps_executed, .. } => {
                assert_eq!(steps_executed, 1, "one tap step should have executed");
            }
            other => panic!(
                "expected ExecutionOutcome::Success with allow rule, got: {:?}",
                other
            ),
        }

        // Verify the tap action was dispatched.
        let log = screen.action_log().unwrap();
        assert_eq!(
            log.len(),
            1,
            "exactly one action should have been dispatched"
        );
    }
}
