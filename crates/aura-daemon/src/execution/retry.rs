//! Retry logic with exponential backoff and intelligent error classification.
//!
//! ## Original (preserved) API
//!
//! - `RetryPolicy` — exponential backoff configuration
//! - `RetryOutcome<T, E>` — success/exhausted result
//! - `retry_with_backoff` — synchronous retry loop
//! - `retry_with_backoff_async` — async retry loop
//!
//! ## Intelligent Retry Extensions (SPEC §6.4)
//!
//! - `ErrorClass` — Transient / Structural / Fatal classification
//! - `FailureLedger` — per-operation failure history for learning
//! - `CircuitBreaker` — circuit breaker pattern (Closed → Open → HalfOpen)
//! - `IntelligentRetry` — orchestrator combining classification, ledger,
//!   circuit breaker, and strategy selection
//!
//! Constants from architecture spec:
//! - Base delay: 200ms
//! - Jitter: ±50ms
//! - Max delay: 10,000ms
//! - Factor: 2x per retry

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

// ═══════════════════════════════════════════════════════════════════════════
// ORIGINAL API (preserved — no changes)
// ═══════════════════════════════════════════════════════════════════════════

/// Configuration for exponential backoff retry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (not counting the initial attempt).
    pub max_retries: u32,
    /// Base delay before first retry (ms).
    pub base_delay_ms: u64,
    /// Multiplicative factor per retry (typically 2).
    pub backoff_factor: u32,
    /// Maximum delay cap (ms).
    pub max_delay_ms: u64,
    /// Jitter range (ms). Actual jitter is ±jitter_ms.
    pub jitter_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 200,
            backoff_factor: 2,
            max_delay_ms: 10_000,
            jitter_ms: 50,
        }
    }
}

impl RetryPolicy {
    /// Create a policy with no retries (for testing or one-shot operations).
    pub fn no_retry() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Create a fast-retry policy (short delays for time-sensitive operations).
    pub fn fast() -> Self {
        Self {
            max_retries: 2,
            base_delay_ms: 100,
            backoff_factor: 2,
            max_delay_ms: 1_000,
            jitter_ms: 25,
        }
    }

    /// Create an aggressive retry policy (more attempts, longer delays).
    pub fn aggressive() -> Self {
        Self {
            max_retries: 5,
            base_delay_ms: 500,
            backoff_factor: 2,
            max_delay_ms: 30_000,
            jitter_ms: 100,
        }
    }

    /// Compute the delay for a given attempt number (0-indexed).
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::ZERO;
        }

        let retry_num = attempt - 1;
        // base * factor^retry_num, capped at max
        let delay_ms = self
            .base_delay_ms
            .saturating_mul(self.backoff_factor.pow(retry_num) as u64)
            .min(self.max_delay_ms);

        // Apply jitter: we use a simple deterministic jitter based on attempt number
        // In production this would use actual randomness, but deterministic is fine
        // for reproducible behavior
        let jitter = self.compute_jitter(attempt);
        let final_delay = if jitter >= 0 {
            delay_ms.saturating_add(jitter as u64)
        } else {
            delay_ms.saturating_sub((-jitter) as u64)
        };

        Duration::from_millis(final_delay.min(self.max_delay_ms))
    }

    /// Simple jitter computation: alternates between +/- based on attempt parity.
    fn compute_jitter(&self, attempt: u32) -> i64 {
        if self.jitter_ms == 0 {
            return 0;
        }
        // Use a hash-like derivation from attempt number for pseudo-randomness
        let hash = attempt.wrapping_mul(2654435761); // Knuth multiplicative hash
        let magnitude = (hash as u64 % (self.jitter_ms * 2 + 1)) as i64;
        magnitude - self.jitter_ms as i64
    }
}

/// The outcome of a retry-wrapped operation.
#[derive(Debug)]
pub enum RetryOutcome<T, E> {
    /// Operation succeeded on attempt N (1-indexed).
    Success { value: T, attempt: u32 },
    /// All attempts exhausted. Contains the last error.
    Exhausted { error: E, attempts: u32 },
}

impl<T, E> RetryOutcome<T, E> {
    /// Whether the operation eventually succeeded.
    pub fn is_success(&self) -> bool {
        matches!(self, RetryOutcome::Success { .. })
    }

    /// Convert to a Result, losing attempt information.
    pub fn into_result(self) -> Result<T, E> {
        match self {
            RetryOutcome::Success { value, .. } => Ok(value),
            RetryOutcome::Exhausted { error, .. } => Err(error),
        }
    }
}

/// Execute a closure with retry and exponential backoff.
///
/// The closure is called up to `1 + policy.max_retries` times. Between
/// retries, the function sleeps for the computed backoff duration.
///
/// This is a synchronous version. For async, use `retry_with_backoff_async`.
pub fn retry_with_backoff<T, E, F>(
    policy: &RetryPolicy,
    mut operation: F,
) -> RetryOutcome<T, E>
where
    F: FnMut(u32) -> Result<T, E>,
{
    let total_attempts = 1 + policy.max_retries;

    // Run the first attempt separately so `last_error` is non-optional,
    // eliminating the need for `.unwrap()` on `Option<E>`.
    let delay = policy.delay_for_attempt(0);
    if !delay.is_zero() {
        std::thread::sleep(delay);
    }

    let mut last_error = match operation(0) {
        Ok(value) => {
            return RetryOutcome::Success {
                value,
                attempt: 1,
            };
        }
        Err(e) => e,
    };

    for attempt in 1..total_attempts {
        let delay = policy.delay_for_attempt(attempt);
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }

        match operation(attempt) {
            Ok(value) => {
                return RetryOutcome::Success {
                    value,
                    attempt: attempt + 1,
                };
            }
            Err(e) => {
                last_error = e;
            }
        }
    }

    RetryOutcome::Exhausted {
        error: last_error,
        attempts: total_attempts,
    }
}

/// Async version of `retry_with_backoff` using `tokio::time::sleep`.
pub async fn retry_with_backoff_async<T, E, F, Fut>(
    policy: &RetryPolicy,
    mut operation: F,
) -> RetryOutcome<T, E>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let total_attempts = 1 + policy.max_retries;

    // Run the first attempt separately so `last_error` is non-optional,
    // eliminating the need for `.unwrap()` on `Option<E>`.
    let delay = policy.delay_for_attempt(0);
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }

    let mut last_error = match operation(0).await {
        Ok(value) => {
            return RetryOutcome::Success {
                value,
                attempt: 1,
            };
        }
        Err(e) => e,
    };

    for attempt in 1..total_attempts {
        let delay = policy.delay_for_attempt(attempt);
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }

        match operation(attempt).await {
            Ok(value) => {
                return RetryOutcome::Success {
                    value,
                    attempt: attempt + 1,
                };
            }
            Err(e) => {
                last_error = e;
            }
        }
    }

    RetryOutcome::Exhausted {
        error: last_error,
        attempts: total_attempts,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// INTELLIGENT RETRY EXTENSIONS (SPEC §6.4)
// ═══════════════════════════════════════════════════════════════════════════

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum operations tracked by the failure ledger.
const MAX_LEDGER_OPERATIONS: usize = 256;

/// Maximum failure entries per operation in the ledger.
const MAX_FAILURES_PER_OP: usize = 64;

/// Circuit breaker: failures needed to open the circuit.
const CIRCUIT_BREAKER_THRESHOLD: u32 = 5;

/// Circuit breaker: time to wait before transitioning from Open → HalfOpen (ms).
const CIRCUIT_BREAKER_RECOVERY_MS: u64 = 30_000;

/// Circuit breaker: successes needed in HalfOpen to close again.
const HALF_OPEN_SUCCESS_THRESHOLD: u32 = 2;

/// Maximum alternative paths per operation.
const MAX_ALTERNATIVES: usize = 8;

// ---------------------------------------------------------------------------
// ErrorClass — error classification (§6.4)
// ---------------------------------------------------------------------------

/// Classification of an error for retry strategy selection.
///
/// From AURA-V4-ACTIVE-AGENCY.md §6.4:
/// - **Transient**: network timeout, temporary unavailability — safe to retry
/// - **Structural**: API changed, permission revoked — retry won't help
/// - **Fatal**: unrecoverable — abort immediately
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorClass {
    /// Temporary failure: retry with backoff.
    Transient,
    /// Non-temporary but non-fatal: try alternative path.
    Structural,
    /// Unrecoverable: abort immediately.
    Fatal,
}

impl ErrorClass {
    /// Whether this error class is retryable with the same operation.
    #[must_use]
    pub fn is_retryable(self) -> bool {
        matches!(self, ErrorClass::Transient)
    }

    /// Whether this error class should trigger alternative path search.
    #[must_use]
    pub fn should_find_alternative(self) -> bool {
        matches!(self, ErrorClass::Structural)
    }

    /// Whether this error class is fatal and should abort.
    #[must_use]
    pub fn is_fatal(self) -> bool {
        matches!(self, ErrorClass::Fatal)
    }
}

// ---------------------------------------------------------------------------
// FailureRecord — single failure event
// ---------------------------------------------------------------------------

/// A single recorded failure with classification and context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureRecord {
    /// When the failure occurred (ms since epoch).
    pub timestamp_ms: u64,
    /// Error classification.
    pub error_class: ErrorClass,
    /// Short description of the error.
    pub description: String,
    /// Which attempt number this failure occurred on (0-indexed).
    pub attempt: u32,
    /// Whether recovery was eventually successful.
    pub recovered: bool,
}

// ---------------------------------------------------------------------------
// FailureLedger — per-operation failure history
// ---------------------------------------------------------------------------

/// Per-operation failure history for learning and strategy adaptation.
///
/// The ledger tracks failure patterns per named operation, allowing the
/// intelligent retry system to:
/// - Detect recurring failures and skip futile retries
/// - Learn which error classes are most common for each operation
/// - Adapt retry policies based on historical success rates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureLedger {
    /// Map from operation name → list of failure records.
    operations: HashMap<String, Vec<FailureRecord>>,
}

impl FailureLedger {
    /// Create an empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self {
            operations: HashMap::with_capacity(32),
        }
    }

    /// Record a failure for an operation.
    pub fn record_failure(
        &mut self,
        operation: &str,
        record: FailureRecord,
    ) -> Result<(), &'static str> {
        if self.operations.len() >= MAX_LEDGER_OPERATIONS
            && !self.operations.contains_key(operation)
        {
            return Err("failure ledger full");
        }

        let entries = self
            .operations
            .entry(operation.to_owned())
            .or_insert_with(|| Vec::with_capacity(8));

        if entries.len() >= MAX_FAILURES_PER_OP {
            entries.remove(0); // evict oldest
        }
        entries.push(record);

        debug!(operation, "failure recorded in ledger");
        Ok(())
    }

    /// Mark the most recent failure for an operation as recovered.
    pub fn mark_recovered(&mut self, operation: &str) {
        if let Some(entries) = self.operations.get_mut(operation) {
            if let Some(last) = entries.last_mut() {
                last.recovered = true;
            }
        }
    }

    /// Get the historical failure rate for an operation (0.0 = never fails, 1.0 = always fails).
    #[must_use]
    pub fn failure_rate(&self, operation: &str) -> f32 {
        match self.operations.get(operation) {
            None => 0.0,
            Some(entries) if entries.is_empty() => 0.0,
            Some(entries) => {
                let unrecovered = entries.iter().filter(|r| !r.recovered).count();
                unrecovered as f32 / entries.len() as f32
            }
        }
    }

    /// Count of failures for an operation.
    #[must_use]
    pub fn failure_count(&self, operation: &str) -> usize {
        self.operations.get(operation).map_or(0, |e| e.len())
    }

    /// Get the most common error class for an operation.
    #[must_use]
    pub fn dominant_error_class(&self, operation: &str) -> Option<ErrorClass> {
        let entries = self.operations.get(operation)?;
        if entries.is_empty() {
            return None;
        }

        let mut counts: HashMap<ErrorClass, usize> = HashMap::new();
        for r in entries {
            *counts.entry(r.error_class).or_insert(0) += 1;
        }

        counts
            .into_iter()
            .max_by_key(|(_, c)| *c)
            .map(|(class, _)| class)
    }

    /// Recent failure count within a time window (ms).
    #[must_use]
    pub fn recent_failures(&self, operation: &str, window_ms: u64, now_ms: u64) -> usize {
        let cutoff = now_ms.saturating_sub(window_ms);
        self.operations.get(operation).map_or(0, |entries| {
            entries
                .iter()
                .filter(|r| r.timestamp_ms >= cutoff)
                .count()
        })
    }

    /// Number of tracked operations.
    #[must_use]
    pub fn operation_count(&self) -> usize {
        self.operations.len()
    }

    /// Suggest a retry policy based on historical failure patterns.
    ///
    /// - High failure rate → aggressive policy (more retries, longer delays)
    /// - Mostly transient failures → standard policy
    /// - Mostly structural → no retry (switch to alternative)
    #[must_use]
    pub fn suggest_policy(&self, operation: &str) -> RetryPolicy {
        let rate = self.failure_rate(operation);
        let dominant = self.dominant_error_class(operation);

        match dominant {
            Some(ErrorClass::Fatal) => RetryPolicy::no_retry(),
            Some(ErrorClass::Structural) => RetryPolicy::no_retry(),
            Some(ErrorClass::Transient) if rate > 0.7 => RetryPolicy::aggressive(),
            _ if rate > 0.5 => RetryPolicy {
                max_retries: 4,
                base_delay_ms: 300,
                backoff_factor: 2,
                max_delay_ms: 15_000,
                jitter_ms: 75,
            },
            _ => RetryPolicy::default(),
        }
    }
}

impl Default for FailureLedger {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// CircuitState
// ---------------------------------------------------------------------------

/// State of a circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CircuitState {
    /// Normal operation — requests flow through.
    Closed,
    /// Too many failures — requests are rejected immediately.
    Open,
    /// Recovering — a limited number of requests are allowed to test.
    HalfOpen,
}

// ---------------------------------------------------------------------------
// CircuitBreaker
// ---------------------------------------------------------------------------

/// Circuit breaker pattern for protecting operations from cascading failures.
///
/// State machine:
/// ```text
/// Closed ──(failures ≥ threshold)──> Open
/// Open ──(recovery timer expires)──> HalfOpen
/// HalfOpen ──(success_count ≥ threshold)──> Closed
/// HalfOpen ──(any failure)──> Open
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreaker {
    /// Current state of the circuit.
    state: CircuitState,
    /// Consecutive failure count.
    failure_count: u32,
    /// Consecutive success count (in HalfOpen state).
    half_open_successes: u32,
    /// Timestamp (ms) when the circuit was opened.
    opened_at_ms: u64,
    /// Timestamp (ms) of the first failure in the current failure window.
    /// Used so recovery time is measured from the start of the failure
    /// sequence, not from the failure that actually tripped the threshold.
    first_failure_ms: u64,
    /// Failure threshold to open the circuit.
    threshold: u32,
    /// Recovery period in ms.
    recovery_ms: u64,
    /// Successes needed in HalfOpen to close.
    half_open_threshold: u32,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with default thresholds.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            half_open_successes: 0,
            opened_at_ms: 0,
            first_failure_ms: 0,
            threshold: CIRCUIT_BREAKER_THRESHOLD,
            recovery_ms: CIRCUIT_BREAKER_RECOVERY_MS,
            half_open_threshold: HALF_OPEN_SUCCESS_THRESHOLD,
        }
    }

    /// Create a circuit breaker with custom thresholds.
    #[must_use]
    pub fn with_thresholds(
        failure_threshold: u32,
        recovery_ms: u64,
        half_open_threshold: u32,
    ) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            half_open_successes: 0,
            opened_at_ms: 0,
            first_failure_ms: 0,
            threshold: failure_threshold.max(1),
            recovery_ms,
            half_open_threshold: half_open_threshold.max(1),
        }
    }

    /// Current state of the circuit.
    #[must_use]
    pub fn state(&self) -> CircuitState {
        self.state
    }

    /// Check if the circuit allows a request at the given time.
    ///
    /// Also transitions Open → HalfOpen if the recovery timer has elapsed.
    pub fn allow_request(&mut self, now_ms: u64) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if now_ms >= self.opened_at_ms + self.recovery_ms {
                    self.state = CircuitState::HalfOpen;
                    self.half_open_successes = 0;
                    debug!("circuit breaker: Open → HalfOpen");
                    true
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// Record a successful operation.
    pub fn record_success(&mut self) {
        match self.state {
            CircuitState::Closed => {
                self.failure_count = 0;
            }
            CircuitState::HalfOpen => {
                self.half_open_successes += 1;
                if self.half_open_successes >= self.half_open_threshold {
                    self.state = CircuitState::Closed;
                    self.failure_count = 0;
                    info!("circuit breaker: HalfOpen → Closed (recovered)");
                }
            }
            CircuitState::Open => {
                // Should not happen, but handle gracefully
            }
        }
    }

    /// Record a failed operation.
    pub fn record_failure(&mut self, now_ms: u64) {
        match self.state {
            CircuitState::Closed => {
                self.failure_count += 1;
                // Track when the failure window started
                if self.failure_count == 1 {
                    self.first_failure_ms = now_ms;
                }
                if self.failure_count >= self.threshold {
                    self.state = CircuitState::Open;
                    // Recovery time is measured from the start of the failure
                    // window, not from the failure that tripped the threshold.
                    self.opened_at_ms = self.first_failure_ms;
                    warn!(
                        failures = self.failure_count,
                        "circuit breaker: Closed → Open"
                    );
                }
            }
            CircuitState::HalfOpen => {
                // Any failure in HalfOpen → back to Open
                self.state = CircuitState::Open;
                self.opened_at_ms = now_ms;
                self.half_open_successes = 0;
                warn!("circuit breaker: HalfOpen → Open (failed during recovery)");
            }
            CircuitState::Open => {
                // Already open, update timestamp
                self.opened_at_ms = now_ms;
            }
        }
    }

    /// Manually reset the circuit breaker to Closed state.
    pub fn reset(&mut self) {
        self.state = CircuitState::Closed;
        self.failure_count = 0;
        self.half_open_successes = 0;
    }

    /// Current consecutive failure count.
    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.failure_count
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// RetryStrategy — selected strategy for a given error
// ---------------------------------------------------------------------------

/// Strategy selected by the intelligent retry system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryStrategy {
    /// Retry with the standard backoff policy.
    RetryWithBackoff,
    /// Don't retry — try an alternative path instead.
    UseAlternative { alternative: String },
    /// Abort — error is fatal or circuit is open.
    Abort { reason: String },
}

// ---------------------------------------------------------------------------
// IntelligentRetry — orchestrator
// ---------------------------------------------------------------------------

/// Orchestrator combining error classification, failure learning,
/// circuit breaker, and strategy selection for intelligent retry decisions.
///
/// Usage:
/// 1. Create an `IntelligentRetry` instance per operation domain
/// 2. Before executing, call `should_attempt()` (checks circuit breaker)
/// 3. On failure, call `handle_failure()` for a strategy recommendation
/// 4. On success, call `handle_success()`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntelligentRetry {
    /// Failure history for learning.
    ledger: FailureLedger,
    /// Per-operation circuit breakers.
    breakers: HashMap<String, CircuitBreaker>,
    /// Per-operation alternative paths.
    alternatives: HashMap<String, Vec<String>>,
}

impl IntelligentRetry {
    /// Create a new intelligent retry orchestrator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ledger: FailureLedger::new(),
            breakers: HashMap::with_capacity(32),
            alternatives: HashMap::with_capacity(16),
        }
    }

    /// Register alternative paths for an operation.
    ///
    /// When the primary operation fails structurally, the system can
    /// recommend switching to one of these alternatives.
    pub fn register_alternatives(
        &mut self,
        operation: &str,
        alts: Vec<String>,
    ) {
        let bounded: Vec<String> = alts.into_iter().take(MAX_ALTERNATIVES).collect();
        self.alternatives.insert(operation.to_owned(), bounded);
    }

    /// Check if an operation should be attempted (circuit breaker check).
    ///
    /// Returns `true` if the circuit allows the request, `false` if open.
    pub fn should_attempt(&mut self, operation: &str, now_ms: u64) -> bool {
        let breaker = self
            .breakers
            .entry(operation.to_owned())
            .or_insert_with(CircuitBreaker::new);
        breaker.allow_request(now_ms)
    }

    /// Handle a successful operation: update circuit breaker and ledger.
    pub fn handle_success(&mut self, operation: &str) {
        if let Some(breaker) = self.breakers.get_mut(operation) {
            breaker.record_success();
        }
        self.ledger.mark_recovered(operation);
    }

    /// Handle a failed operation: classify the error, update ledger/breaker,
    /// and recommend a retry strategy.
    ///
    /// The `classify` callback classifies the error into an `ErrorClass`.
    /// This allows callers to provide domain-specific classification logic.
    pub fn handle_failure<E, C>(
        &mut self,
        operation: &str,
        error: &E,
        classify: C,
        now_ms: u64,
        attempt: u32,
    ) -> RetryStrategy
    where
        E: std::fmt::Display,
        C: FnOnce(&E) -> ErrorClass,
    {
        let error_class = classify(error);
        let description = format!("{}", error);

        // Record in ledger
        let record = FailureRecord {
            timestamp_ms: now_ms,
            error_class,
            description,
            attempt,
            recovered: false,
        };
        let _ = self.ledger.record_failure(operation, record);

        // Update circuit breaker
        let breaker = self
            .breakers
            .entry(operation.to_owned())
            .or_insert_with(CircuitBreaker::new);
        breaker.record_failure(now_ms);

        // Determine strategy
        match error_class {
            ErrorClass::Fatal => RetryStrategy::Abort {
                reason: format!("fatal error: {error}"),
            },
            ErrorClass::Structural => {
                // Try to find an alternative
                if let Some(alts) = self.alternatives.get(operation) {
                    if let Some(alt) = alts.first() {
                        return RetryStrategy::UseAlternative {
                            alternative: alt.clone(),
                        };
                    }
                }
                RetryStrategy::Abort {
                    reason: format!("structural error, no alternatives: {error}"),
                }
            }
            ErrorClass::Transient => {
                // Check if circuit is now open
                if breaker.state() == CircuitState::Open {
                    return RetryStrategy::Abort {
                        reason: "circuit breaker open".into(),
                    };
                }
                RetryStrategy::RetryWithBackoff
            }
        }
    }

    /// Get the failure ledger for external analysis.
    #[must_use]
    pub fn ledger(&self) -> &FailureLedger {
        &self.ledger
    }

    /// Get a suggested retry policy for an operation based on history.
    #[must_use]
    pub fn suggested_policy(&self, operation: &str) -> RetryPolicy {
        self.ledger.suggest_policy(operation)
    }

    /// Get the circuit breaker state for an operation.
    #[must_use]
    pub fn circuit_state(&self, operation: &str) -> CircuitState {
        self.breakers
            .get(operation)
            .map_or(CircuitState::Closed, |b| b.state())
    }

    /// Number of tracked operations in the ledger.
    #[must_use]
    pub fn tracked_operations(&self) -> usize {
        self.ledger.operation_count()
    }

    /// Manually reset a circuit breaker for an operation.
    pub fn reset_circuit(&mut self, operation: &str) {
        if let Some(breaker) = self.breakers.get_mut(operation) {
            breaker.reset();
        }
    }
}

impl Default for IntelligentRetry {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Original tests (preserved) ──────────────────────────────────────

    #[test]
    fn test_default_policy() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_retries, 3);
        assert_eq!(p.base_delay_ms, 200);
        assert_eq!(p.backoff_factor, 2);
        assert_eq!(p.max_delay_ms, 10_000);
        assert_eq!(p.jitter_ms, 50);
    }

    #[test]
    fn test_delay_for_attempt_zero() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay_for_attempt(0), Duration::ZERO);
    }

    #[test]
    fn test_exponential_growth() {
        let p = RetryPolicy {
            jitter_ms: 0, // no jitter for deterministic test
            ..RetryPolicy::default()
        };
        // attempt 1: 200ms
        assert_eq!(p.delay_for_attempt(1), Duration::from_millis(200));
        // attempt 2: 400ms
        assert_eq!(p.delay_for_attempt(2), Duration::from_millis(400));
        // attempt 3: 800ms
        assert_eq!(p.delay_for_attempt(3), Duration::from_millis(800));
        // attempt 4: 1600ms
        assert_eq!(p.delay_for_attempt(4), Duration::from_millis(1600));
    }

    #[test]
    fn test_delay_capped_at_max() {
        let p = RetryPolicy {
            base_delay_ms: 5000,
            max_delay_ms: 10_000,
            jitter_ms: 0,
            ..RetryPolicy::default()
        };
        // attempt 2: 5000 * 2 = 10000, at cap
        assert_eq!(p.delay_for_attempt(2), Duration::from_millis(10_000));
        // attempt 3: would be 20000 but capped
        assert_eq!(p.delay_for_attempt(3), Duration::from_millis(10_000));
    }

    #[test]
    fn test_retry_succeeds_first_attempt() {
        let policy = RetryPolicy::no_retry();
        let outcome = retry_with_backoff(&policy, |_| Ok::<_, &str>(42));
        assert!(outcome.is_success());
        match outcome {
            RetryOutcome::Success { value, attempt } => {
                assert_eq!(value, 42);
                assert_eq!(attempt, 1);
            }
            _ => panic!("expected success"),
        }
    }

    #[test]
    fn test_retry_succeeds_on_second_attempt() {
        let policy = RetryPolicy {
            max_retries: 3,
            base_delay_ms: 1, // tiny delay for test speed
            jitter_ms: 0,
            ..RetryPolicy::default()
        };
        let mut call_count = 0u32;
        let outcome = retry_with_backoff(&policy, |_attempt| {
            call_count += 1;
            if call_count < 2 {
                Err("transient failure")
            } else {
                Ok(99)
            }
        });

        assert!(outcome.is_success());
        match outcome {
            RetryOutcome::Success { value, attempt } => {
                assert_eq!(value, 99);
                assert_eq!(attempt, 2);
            }
            _ => panic!("expected success"),
        }
    }

    #[test]
    fn test_retry_exhausted() {
        let policy = RetryPolicy {
            max_retries: 2,
            base_delay_ms: 1,
            jitter_ms: 0,
            ..RetryPolicy::default()
        };
        let outcome = retry_with_backoff(&policy, |_| Err::<(), _>("always fails"));

        assert!(!outcome.is_success());
        match outcome {
            RetryOutcome::Exhausted { error, attempts } => {
                assert_eq!(error, "always fails");
                assert_eq!(attempts, 3); // 1 initial + 2 retries
            }
            _ => panic!("expected exhausted"),
        }
    }

    #[test]
    fn test_into_result() {
        let policy = RetryPolicy::no_retry();
        let outcome = retry_with_backoff(&policy, |_| Ok::<_, &str>(42));
        assert_eq!(outcome.into_result(), Ok(42));

        let outcome2 = retry_with_backoff(&RetryPolicy::no_retry(), |_| {
            Err::<(), _>("fail")
        });
        assert_eq!(outcome2.into_result(), Err("fail"));
    }

    #[test]
    fn test_jitter_bounded() {
        let p = RetryPolicy::default();
        for attempt in 1..10 {
            let delay = p.delay_for_attempt(attempt);
            let base = p.base_delay_ms * p.backoff_factor.pow(attempt - 1) as u64;
            let base = base.min(p.max_delay_ms);
            let diff = if delay.as_millis() as u64 > base {
                delay.as_millis() as u64 - base
            } else {
                base - delay.as_millis() as u64
            };
            assert!(
                diff <= p.jitter_ms,
                "jitter {} exceeds ±{} for attempt {}",
                diff,
                p.jitter_ms,
                attempt
            );
        }
    }

    #[tokio::test]
    async fn test_async_retry_succeeds() {
        let policy = RetryPolicy {
            max_retries: 2,
            base_delay_ms: 1,
            jitter_ms: 0,
            ..RetryPolicy::default()
        };
        let mut count = 0u32;
        let outcome = retry_with_backoff_async(&policy, |_| {
            count += 1;
            async move {
                if count < 2 {
                    Err("transient")
                } else {
                    Ok(77)
                }
            }
        })
        .await;

        assert!(outcome.is_success());
    }

    // ── Intelligent retry extension tests ───────────────────────────────

    #[test]
    fn test_error_class_properties() {
        assert!(ErrorClass::Transient.is_retryable());
        assert!(!ErrorClass::Structural.is_retryable());
        assert!(!ErrorClass::Fatal.is_retryable());

        assert!(!ErrorClass::Transient.should_find_alternative());
        assert!(ErrorClass::Structural.should_find_alternative());
        assert!(!ErrorClass::Fatal.should_find_alternative());

        assert!(!ErrorClass::Transient.is_fatal());
        assert!(!ErrorClass::Structural.is_fatal());
        assert!(ErrorClass::Fatal.is_fatal());
    }

    #[test]
    fn test_failure_ledger_record() {
        let mut ledger = FailureLedger::new();
        let record = FailureRecord {
            timestamp_ms: 1000,
            error_class: ErrorClass::Transient,
            description: "timeout".into(),
            attempt: 0,
            recovered: false,
        };
        assert!(ledger.record_failure("fetch_data", record).is_ok());
        assert_eq!(ledger.failure_count("fetch_data"), 1);
        assert_eq!(ledger.operation_count(), 1);
    }

    #[test]
    fn test_failure_ledger_rate() {
        let mut ledger = FailureLedger::new();

        // 3 failures, 1 recovered
        for i in 0..3 {
            let record = FailureRecord {
                timestamp_ms: 1000 + i * 100,
                error_class: ErrorClass::Transient,
                description: "timeout".into(),
                attempt: 0,
                recovered: i == 0, // first one recovered
            };
            let _ = ledger.record_failure("fetch", record);
        }

        let rate = ledger.failure_rate("fetch");
        // 2 unrecovered out of 3 = ~0.667
        assert!(
            (rate - 0.667).abs() < 0.01,
            "expected ~0.667, got {rate}"
        );
    }

    #[test]
    fn test_failure_ledger_dominant_class() {
        let mut ledger = FailureLedger::new();

        // 3 transient, 1 structural
        for _ in 0..3 {
            let _ = ledger.record_failure(
                "api_call",
                FailureRecord {
                    timestamp_ms: 1000,
                    error_class: ErrorClass::Transient,
                    description: "timeout".into(),
                    attempt: 0,
                    recovered: false,
                },
            );
        }
        let _ = ledger.record_failure(
            "api_call",
            FailureRecord {
                timestamp_ms: 2000,
                error_class: ErrorClass::Structural,
                description: "404".into(),
                attempt: 0,
                recovered: false,
            },
        );

        assert_eq!(
            ledger.dominant_error_class("api_call"),
            Some(ErrorClass::Transient)
        );
    }

    #[test]
    fn test_failure_ledger_recent_failures() {
        let mut ledger = FailureLedger::new();

        // Old failure
        let _ = ledger.record_failure(
            "op",
            FailureRecord {
                timestamp_ms: 1000,
                error_class: ErrorClass::Transient,
                description: "old".into(),
                attempt: 0,
                recovered: false,
            },
        );
        // Recent failure
        let _ = ledger.record_failure(
            "op",
            FailureRecord {
                timestamp_ms: 9000,
                error_class: ErrorClass::Transient,
                description: "recent".into(),
                attempt: 0,
                recovered: false,
            },
        );

        assert_eq!(ledger.recent_failures("op", 5000, 10_000), 1);
        assert_eq!(ledger.recent_failures("op", 15_000, 10_000), 2);
    }

    #[test]
    fn test_failure_ledger_suggest_policy() {
        let ledger = FailureLedger::new();
        // No data → default policy
        let policy = ledger.suggest_policy("unknown");
        assert_eq!(policy.max_retries, 3);

        // Structural dominant → no retry
        let mut ledger2 = FailureLedger::new();
        for _ in 0..5 {
            let _ = ledger2.record_failure(
                "broken",
                FailureRecord {
                    timestamp_ms: 1000,
                    error_class: ErrorClass::Structural,
                    description: "broken".into(),
                    attempt: 0,
                    recovered: false,
                },
            );
        }
        let policy2 = ledger2.suggest_policy("broken");
        assert_eq!(policy2.max_retries, 0);
    }

    #[test]
    fn test_circuit_breaker_closed() {
        let mut cb = CircuitBreaker::new();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request(1000));
    }

    #[test]
    fn test_circuit_breaker_opens_on_threshold() {
        let mut cb = CircuitBreaker::new();

        for i in 0..CIRCUIT_BREAKER_THRESHOLD {
            cb.record_failure(1000 + i as u64 * 100);
        }

        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow_request(2000));
    }

    #[test]
    fn test_circuit_breaker_recovery() {
        let mut cb = CircuitBreaker::new();

        // Open the circuit
        for _ in 0..CIRCUIT_BREAKER_THRESHOLD {
            cb.record_failure(1000);
        }
        assert_eq!(cb.state(), CircuitState::Open);

        // Before recovery period — still open
        assert!(!cb.allow_request(1000 + CIRCUIT_BREAKER_RECOVERY_MS - 1));
        assert_eq!(cb.state(), CircuitState::Open);

        // After recovery period → transitions to HalfOpen
        assert!(cb.allow_request(1000 + CIRCUIT_BREAKER_RECOVERY_MS));
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Successes in HalfOpen close the circuit
        for _ in 0..HALF_OPEN_SUCCESS_THRESHOLD {
            cb.record_success();
        }
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_half_open_failure() {
        let mut cb = CircuitBreaker::new();

        // Open and recover to HalfOpen
        for _ in 0..CIRCUIT_BREAKER_THRESHOLD {
            cb.record_failure(1000);
        }
        cb.allow_request(1000 + CIRCUIT_BREAKER_RECOVERY_MS);
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Failure in HalfOpen → back to Open
        cb.record_failure(50_000);
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_circuit_breaker_reset() {
        let mut cb = CircuitBreaker::new();
        for _ in 0..CIRCUIT_BREAKER_THRESHOLD {
            cb.record_failure(1000);
        }
        assert_eq!(cb.state(), CircuitState::Open);

        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn test_circuit_breaker_custom_thresholds() {
        let mut cb = CircuitBreaker::with_thresholds(2, 5000, 1);
        cb.record_failure(1000);
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure(1100);
        assert_eq!(cb.state(), CircuitState::Open);

        // HalfOpen after 5000ms
        assert!(cb.allow_request(6100));
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // 1 success closes it (half_open_threshold=1)
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_intelligent_retry_should_attempt() {
        let mut ir = IntelligentRetry::new();
        // No history → should attempt
        assert!(ir.should_attempt("new_op", 1000));
    }

    #[test]
    fn test_intelligent_retry_handle_transient() {
        let mut ir = IntelligentRetry::new();

        let strategy = ir.handle_failure(
            "fetch",
            &"timeout",
            |_| ErrorClass::Transient,
            1000,
            0,
        );

        assert_eq!(strategy, RetryStrategy::RetryWithBackoff);
        assert_eq!(ir.tracked_operations(), 1);
    }

    #[test]
    fn test_intelligent_retry_handle_structural_with_alt() {
        let mut ir = IntelligentRetry::new();
        ir.register_alternatives("fetch", vec!["fetch_v2".into(), "cache".into()]);

        let strategy = ir.handle_failure(
            "fetch",
            &"404 not found",
            |_| ErrorClass::Structural,
            1000,
            0,
        );

        assert_eq!(
            strategy,
            RetryStrategy::UseAlternative {
                alternative: "fetch_v2".into()
            }
        );
    }

    #[test]
    fn test_intelligent_retry_handle_structural_no_alt() {
        let mut ir = IntelligentRetry::new();

        let strategy = ir.handle_failure(
            "fetch",
            &"404 not found",
            |_| ErrorClass::Structural,
            1000,
            0,
        );

        match strategy {
            RetryStrategy::Abort { reason } => {
                assert!(reason.contains("no alternatives"));
            }
            _ => panic!("expected abort, got {strategy:?}"),
        }
    }

    #[test]
    fn test_intelligent_retry_handle_fatal() {
        let mut ir = IntelligentRetry::new();

        let strategy = ir.handle_failure(
            "auth",
            &"invalid credentials",
            |_| ErrorClass::Fatal,
            1000,
            0,
        );

        match strategy {
            RetryStrategy::Abort { reason } => {
                assert!(reason.contains("fatal"));
            }
            _ => panic!("expected abort"),
        }
    }

    #[test]
    fn test_intelligent_retry_circuit_opens() {
        let mut ir = IntelligentRetry::new();

        // Fail enough times to open the circuit
        for i in 0..(CIRCUIT_BREAKER_THRESHOLD + 1) {
            let _ = ir.handle_failure(
                "fetch",
                &"timeout",
                |_| ErrorClass::Transient,
                1000 + i as u64 * 100,
                i,
            );
        }

        assert_eq!(ir.circuit_state("fetch"), CircuitState::Open);
        assert!(!ir.should_attempt("fetch", 2000));
    }

    #[test]
    fn test_intelligent_retry_success_recovery() {
        let mut ir = IntelligentRetry::new();

        // Fail to open circuit
        for i in 0..CIRCUIT_BREAKER_THRESHOLD {
            let _ = ir.handle_failure(
                "fetch",
                &"timeout",
                |_| ErrorClass::Transient,
                1000 + i as u64 * 100,
                i,
            );
        }
        assert_eq!(ir.circuit_state("fetch"), CircuitState::Open);

        // Wait for recovery
        assert!(ir.should_attempt("fetch", 1000 + CIRCUIT_BREAKER_RECOVERY_MS));
        assert_eq!(ir.circuit_state("fetch"), CircuitState::HalfOpen);

        // Succeed to close
        for _ in 0..HALF_OPEN_SUCCESS_THRESHOLD {
            ir.handle_success("fetch");
        }
        assert_eq!(ir.circuit_state("fetch"), CircuitState::Closed);
    }

    #[test]
    fn test_intelligent_retry_suggested_policy() {
        let ir = IntelligentRetry::new();
        // No history → default
        let policy = ir.suggested_policy("unknown");
        assert_eq!(policy.max_retries, 3);
    }

    #[test]
    fn test_intelligent_retry_reset_circuit() {
        let mut ir = IntelligentRetry::new();
        for i in 0..CIRCUIT_BREAKER_THRESHOLD {
            let _ = ir.handle_failure(
                "op",
                &"fail",
                |_| ErrorClass::Transient,
                1000 + i as u64 * 100,
                i,
            );
        }
        assert_eq!(ir.circuit_state("op"), CircuitState::Open);

        ir.reset_circuit("op");
        assert_eq!(ir.circuit_state("op"), CircuitState::Closed);
    }

    #[test]
    fn test_failure_ledger_mark_recovered() {
        let mut ledger = FailureLedger::new();
        let _ = ledger.record_failure(
            "op",
            FailureRecord {
                timestamp_ms: 1000,
                error_class: ErrorClass::Transient,
                description: "err".into(),
                attempt: 0,
                recovered: false,
            },
        );
        assert!((ledger.failure_rate("op") - 1.0).abs() < f32::EPSILON);

        ledger.mark_recovered("op");
        assert!((ledger.failure_rate("op") - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_ledger_bounded() {
        let mut ledger = FailureLedger::new();
        // Fill one operation with max failures
        for i in 0..(MAX_FAILURES_PER_OP + 10) {
            let _ = ledger.record_failure(
                "op",
                FailureRecord {
                    timestamp_ms: i as u64,
                    error_class: ErrorClass::Transient,
                    description: format!("err_{i}"),
                    attempt: 0,
                    recovered: false,
                },
            );
        }
        assert!(ledger.failure_count("op") <= MAX_FAILURES_PER_OP);
    }

    #[test]
    fn test_circuit_success_resets_failure_count() {
        let mut cb = CircuitBreaker::new();
        cb.record_failure(1000);
        cb.record_failure(1100);
        assert_eq!(cb.consecutive_failures(), 2);

        cb.record_success();
        assert_eq!(cb.consecutive_failures(), 0);
    }
}
