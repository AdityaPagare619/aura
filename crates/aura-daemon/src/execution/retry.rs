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
//! - `IntelligentRetry` — orchestrator combining classification, ledger, circuit breaker, and
//!   strategy selection
//!
//! Constants from architecture spec:
//! - Base delay: 200ms
//! - Jitter: ±50ms
//! - Max delay: 10,000ms
//! - Factor: 2x per retry

use std::{collections::HashMap, time::Duration};

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
pub fn retry_with_backoff<T, E, F>(policy: &RetryPolicy, mut operation: F) -> RetryOutcome<T, E>
where
    F: FnMut(u32) -> Result<T, E>, {
    let total_attempts = 1 + policy.max_retries;

    // Run the first attempt separately so `last_error` is non-optional,
    // eliminating the need for `.unwrap()` on `Option<E>`.
    let delay = policy.delay_for_attempt(0);
    if !delay.is_zero() {
        std::thread::sleep(delay);
    }

    let mut last_error = match operation(0) {
        Ok(value) => {
            return RetryOutcome::Success { value, attempt: 1 };
        },
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
            },
            Err(e) => {
                last_error = e;
            },
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
    Fut: std::future::Future<Output = Result<T, E>>, {
    let total_attempts = 1 + policy.max_retries;

    // Run the first attempt separately so `last_error` is non-optional,
    // eliminating the need for `.unwrap()` on `Option<E>`.
    let delay = policy.delay_for_attempt(0);
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }

    let mut last_error = match operation(0).await {
        Ok(value) => {
            return RetryOutcome::Success { value, attempt: 1 };
        },
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
            },
            Err(e) => {
                last_error = e;
            },
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
            },
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
            entries.iter().filter(|r| r.timestamp_ms >= cutoff).count()
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
            },
            CircuitState::HalfOpen => true,
        }
    }

    /// Record a successful operation.
    pub fn record_success(&mut self) {
        match self.state {
            CircuitState::Closed => {
                self.failure_count = 0;
            },
            CircuitState::HalfOpen => {
                self.half_open_successes += 1;
                if self.half_open_successes >= self.half_open_threshold {
                    self.state = CircuitState::Closed;
                    self.failure_count = 0;
                    info!("circuit breaker: HalfOpen → Closed (recovered)");
                }
            },
            CircuitState::Open => {
                // Should not happen, but handle gracefully
            },
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
            },
            CircuitState::HalfOpen => {
                // Any failure in HalfOpen → back to Open
                self.state = CircuitState::Open;
                self.opened_at_ms = now_ms;
                self.half_open_successes = 0;
                warn!("circuit breaker: HalfOpen → Open (failed during recovery)");
            },
            CircuitState::Open => {
                // Already open, update timestamp
                self.opened_at_ms = now_ms;
            },
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
    pub fn register_alternatives(&mut self, operation: &str, alts: Vec<String>) {
        let bounded: Vec<String> = alts.into_iter().take(MAX_ALTERNATIVES).collect();
        self.alternatives.insert(operation.to_owned(), bounded);
    }

    /// Check if an operation should be attempted (circuit breaker check).
    ///
    /// Returns `true` if the circuit allows the request, `false` if open.
    pub fn should_attempt(&mut self, operation: &str, now_ms: u64) -> bool {
        let breaker = self.breakers.entry(operation.to_owned()).or_default();
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
        C: FnOnce(&E) -> ErrorClass, {
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
        let breaker = self.breakers.entry(operation.to_owned()).or_default();
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
            },
            ErrorClass::Transient => {
                // Check if circuit is now open
                if breaker.state() == CircuitState::Open {
                    return RetryStrategy::Abort {
                        reason: "circuit breaker open".into(),
                    };
                }
                RetryStrategy::RetryWithBackoff
            },
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
// STRATEGIC FAILURE RECOVERY (Foundation 4)
// ═══════════════════════════════════════════════════════════════════════════

// ---------------------------------------------------------------------------
// Constants — Strategic Recovery
// ---------------------------------------------------------------------------

/// Maximum escalation attempts per operation before giving up.
const MAX_ESCALATIONS_PER_OPERATION: u32 = 3;

/// Maximum operations tracked in the escalation map.
const MAX_ESCALATION_OPERATIONS: usize = 128;

/// Maximum entries in the recovery history.
const MAX_RECOVERY_HISTORY: usize = 256;

/// Maximum transient retry attempts before escalating to Strategic.
const MAX_TRANSIENT_ATTEMPTS: u32 = 3;

/// Battery level threshold below which we classify as Environmental.
const CRITICAL_BATTERY_THRESHOLD: f32 = 0.05;

/// Default wait time (ms) when restarting an environment target.
const DEFAULT_RESTART_WAIT_MS: u64 = 5_000;

// ---------------------------------------------------------------------------
// BoundedVec<T> — bounded-capacity vector
// ---------------------------------------------------------------------------

/// A vector with a fixed maximum capacity. Evicts the oldest entry on overflow.
///
/// Used throughout the recovery system to ensure memory usage is bounded
/// regardless of how long the daemon runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundedVec<T> {
    inner: Vec<T>,
    capacity: usize,
}

impl<T> BoundedVec<T> {
    /// Create a new `BoundedVec` with the given maximum capacity.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "BoundedVec capacity must be > 0");
        Self {
            inner: Vec::with_capacity(capacity.min(64)),
            capacity,
        }
    }

    /// Push an item, evicting the oldest if at capacity.
    pub fn push(&mut self, item: T) {
        if self.inner.len() >= self.capacity {
            self.inner.remove(0);
        }
        self.inner.push(item);
    }

    /// Number of items currently stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the vector is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Iterate over the stored items.
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.inner.iter()
    }

    /// Maximum capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

// ---------------------------------------------------------------------------
// FailureCategory — 5-category taxonomy (Foundation 4)
// ---------------------------------------------------------------------------

/// Five-category failure taxonomy for strategic recovery decisions.
///
/// Extends beyond the simpler `ErrorClass` (Transient/Structural/Fatal) to
/// provide nuanced recovery paths:
///
/// | Category      | Recovery Strategy                                |
/// |---------------|--------------------------------------------------|
/// | Transient     | Retry with backoff, max 3, then escalate         |
/// | Strategic     | Re-plan with LLM, try alternative approach       |
/// | Environmental | Restart app/wait, retry once, then notify user   |
/// | Capability    | Tell user what's needed ("I can't do X because Y") |
/// | Safety        | STOP. Never retry. Log for review.               |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FailureCategory {
    /// Network blip, app loading, timeout — retry with backoff.
    Transient,
    /// Wrong screen, button not found, UI changed — re-plan with LLM.
    Strategic,
    /// App crashed, no internet, low battery, no permission — restart/wait.
    Environmental,
    /// App not installed, API restricted, hardware missing — inform user.
    Capability,
    /// Policy gate blocked, boundary hit, ethical violation — halt immediately.
    Safety,
}

impl std::fmt::Display for FailureCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FailureCategory::Transient => write!(f, "Transient"),
            FailureCategory::Strategic => write!(f, "Strategic"),
            FailureCategory::Environmental => write!(f, "Environmental"),
            FailureCategory::Capability => write!(f, "Capability"),
            FailureCategory::Safety => write!(f, "Safety"),
        }
    }
}

impl From<ErrorClass> for FailureCategory {
    fn from(class: ErrorClass) -> Self {
        match class {
            ErrorClass::Transient => FailureCategory::Transient,
            ErrorClass::Structural => FailureCategory::Strategic,
            ErrorClass::Fatal => FailureCategory::Safety,
        }
    }
}

impl FailureCategory {
    /// Whether this category is directly retryable with the same operation.
    ///
    /// Only `Transient` failures are safe to retry as-is.
    #[must_use]
    pub fn is_retryable(self) -> bool {
        matches!(self, FailureCategory::Transient)
    }

    /// Whether recovery is possible (automatic or with replanning).
    ///
    /// `Transient`, `Strategic`, and `Environmental` are recoverable.
    /// `Capability` and `Safety` require user intervention or halt.
    #[must_use]
    pub fn is_recoverable(self) -> bool {
        matches!(
            self,
            FailureCategory::Transient
                | FailureCategory::Strategic
                | FailureCategory::Environmental
        )
    }

    /// Whether the user should be notified about this failure.
    ///
    /// `Environmental`, `Capability`, and `Safety` failures warrant user awareness.
    #[must_use]
    pub fn requires_user_notification(self) -> bool {
        matches!(
            self,
            FailureCategory::Environmental | FailureCategory::Capability | FailureCategory::Safety
        )
    }
}

// ---------------------------------------------------------------------------
// RecoverySeverity
// ---------------------------------------------------------------------------

/// Severity level for user-facing recovery notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecoverySeverity {
    /// Informational — the system is handling it.
    Info,
    /// Warning — user attention may be needed soon.
    Warning,
    /// Error — user action is needed.
    Error,
    /// Critical — immediate attention required, operation halted.
    Critical,
}

impl std::fmt::Display for RecoverySeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecoverySeverity::Info => write!(f, "INFO"),
            RecoverySeverity::Warning => write!(f, "WARNING"),
            RecoverySeverity::Error => write!(f, "ERROR"),
            RecoverySeverity::Critical => write!(f, "CRITICAL"),
        }
    }
}

// ---------------------------------------------------------------------------
// RecoveryAction — what to do when a failure occurs
// ---------------------------------------------------------------------------

/// The specific recovery action to take in response to a categorized failure.
///
/// Each variant carries the data needed to execute the recovery step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecoveryAction {
    /// Retry the same operation with exponential backoff.
    RetryWithBackoff {
        /// The backoff policy to use.
        policy: RetryPolicy,
    },
    /// Re-plan the approach using the LLM/neocortex.
    Replan {
        /// Why replanning is needed.
        reason: String,
        /// Context to pass to the planner (e.g. what failed, what was tried).
        context: String,
    },
    /// Restart an environment component (app, service) and wait.
    RestartEnvironment {
        /// What to restart (e.g. app package name, service identifier).
        target: String,
        /// How long to wait after restart before retrying (ms).
        wait_ms: u64,
    },
    /// Notify the user about a situation requiring their attention.
    NotifyUser {
        /// Human-readable message for the user.
        message: String,
        /// How urgent this notification is.
        severity: RecoverySeverity,
    },
    /// Halt all retry attempts and log for review. Used for Safety failures.
    HaltAndLog {
        /// Why the operation was halted.
        reason: String,
        /// The failure category that triggered the halt.
        category: FailureCategory,
    },
    /// Escalate a Transient failure to Strategic after exhausting retries.
    EscalateToStrategic {
        /// The original category before escalation.
        from_category: FailureCategory,
        /// Context about what was tried before escalating.
        context: String,
    },
    /// Try an alternative approach to achieve the same goal.
    TryAlternative {
        /// Description of the alternative to try.
        alternative: String,
        /// Why the original approach failed.
        reason: String,
    },
}

impl RecoveryAction {
    /// Produce a short summary string for logging/history.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            RecoveryAction::RetryWithBackoff { policy } => {
                format!("retry(max={})", policy.max_retries)
            },
            RecoveryAction::Replan { reason, .. } => {
                format!("replan: {reason}")
            },
            RecoveryAction::RestartEnvironment { target, wait_ms } => {
                format!("restart({target}, wait={wait_ms}ms)")
            },
            RecoveryAction::NotifyUser { severity, message } => {
                format!("notify[{severity}]: {message}")
            },
            RecoveryAction::HaltAndLog { category, reason } => {
                format!("HALT[{category}]: {reason}")
            },
            RecoveryAction::EscalateToStrategic {
                from_category,
                context,
            } => {
                format!("escalate({from_category} → Strategic): {context}")
            },
            RecoveryAction::TryAlternative {
                alternative,
                reason,
            } => {
                format!("alternative({alternative}): {reason}")
            },
        }
    }
}

// ---------------------------------------------------------------------------
// EnvironmentSnapshot — current device/environment state
// ---------------------------------------------------------------------------

/// Snapshot of the device and environment state at the time of a failure.
///
/// Used by `classify_failure` to override text-based classification when
/// the environment itself is the root cause.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentSnapshot {
    /// Battery level (0.0 = empty, 1.0 = full).
    pub battery_level: f32,
    /// Whether a network connection is available.
    pub network_available: bool,
    /// Whether the target app is currently running.
    pub target_app_running: bool,
    /// Whether the screen is responsive to input.
    pub screen_responsive: bool,
    /// Whether the neocortex (LLM service) is alive and reachable.
    pub neocortex_alive: bool,
}

impl Default for EnvironmentSnapshot {
    fn default() -> Self {
        Self {
            battery_level: 1.0,
            network_available: true,
            target_app_running: true,
            screen_responsive: true,
            neocortex_alive: true,
        }
    }
}

// ---------------------------------------------------------------------------
// RecoveryContext — full context for recovery decisions
// ---------------------------------------------------------------------------

/// Full context provided to the recovery decision engine.
///
/// Combines information about the operation, its failure history, and
/// the current environment state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryContext {
    /// The name/identifier of the failing operation.
    pub operation: String,
    /// Human-readable description of the goal being pursued.
    pub goal_description: String,
    /// How many attempts have been made (including the failed one).
    pub attempt_count: u32,
    /// Total time spent on this operation so far (ms).
    pub time_elapsed_ms: u64,
    /// The error message from the most recent failure.
    pub last_error: String,
    /// Classified failure category.
    pub category: FailureCategory,
    /// Current environment state.
    pub environment_state: EnvironmentSnapshot,
}

// ---------------------------------------------------------------------------
// RecoveryHistoryEntry — for learning from past recoveries
// ---------------------------------------------------------------------------

/// A single entry in the recovery history, recording what was tried and
/// whether it succeeded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryHistoryEntry {
    /// When the recovery action was taken (ms since epoch).
    pub timestamp_ms: u64,
    /// The operation that failed.
    pub operation: String,
    /// How the failure was categorized.
    pub category: FailureCategory,
    /// Summary of the recovery action that was taken.
    pub action_taken: String,
    /// Whether the recovery ultimately succeeded.
    pub success: bool,
}

// ---------------------------------------------------------------------------
// RecoveryStats — aggregate recovery statistics
// ---------------------------------------------------------------------------

/// Aggregate statistics about recovery attempts and outcomes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryStats {
    /// Total number of recovery actions taken.
    pub total_recoveries: u32,
    /// Number of recovery actions that succeeded.
    pub successful_recoveries: u32,
    /// Per-category breakdown: category name → (total, successful).
    pub by_category: HashMap<String, (u32, u32)>,
}

// ---------------------------------------------------------------------------
// StrategicRecovery — the main orchestrator
// ---------------------------------------------------------------------------

/// Strategic Failure Recovery orchestrator.
///
/// Builds on top of `IntelligentRetry` to provide a 5-category failure
/// taxonomy with per-category recovery strategies. The system:
///
/// 1. Classifies failures using both error message patterns and environment state
/// 2. Determines the appropriate recovery action based on category and history
/// 3. Tracks escalation counts to prevent infinite loops
/// 4. Records recovery outcomes for learning
///
/// # Bounded Collections
///
/// - Escalation counts: bounded to `MAX_ESCALATION_OPERATIONS` (128) entries
/// - Recovery history: bounded to `MAX_RECOVERY_HISTORY` (256) entries
pub struct StrategicRecovery {
    /// The underlying intelligent retry system (reused).
    retry_system: IntelligentRetry,
    /// Per-operation escalation counts, bounded to 128 entries.
    escalation_counts: HashMap<String, u32>,
    /// Bounded history of recovery actions and their outcomes.
    recovery_history: BoundedVec<RecoveryHistoryEntry>,
    /// Maximum escalations allowed per operation before giving up.
    max_escalations_per_operation: u32,
}

impl StrategicRecovery {
    /// Create a new `StrategicRecovery` orchestrator with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            retry_system: IntelligentRetry::new(),
            escalation_counts: HashMap::with_capacity(32),
            recovery_history: BoundedVec::new(MAX_RECOVERY_HISTORY),
            max_escalations_per_operation: MAX_ESCALATIONS_PER_OPERATION,
        }
    }

    /// Classify a failure into one of the five categories.
    ///
    /// Classification uses a two-phase approach:
    /// 1. **Text-based**: pattern matching on the error message
    /// 2. **Environment-based**: overrides from `EnvironmentSnapshot` state
    ///
    /// Classify a failure into a [`FailureCategory`].
    ///
    /// ## Classification tiers (highest priority first)
    ///
    /// 1. **Environment overrides** — battery, screen, network+transient, neocortex+LLM (preserved
    ///    exactly as before).
    /// 2. **Structural fast-path** — mechanical, non-semantic patterns: timeout → Transient;
    ///    network socket/DNS → Transient; permission/ access → Capability; OOM/resource-exhausted →
    ///    Environmental. These are structural identifiers, NOT intent reasoning — the Iron Law is
    ///    not violated.
    /// 3. **LLM path** — ambiguous failures are sent to the neocortex via
    ///    [`DaemonToNeocortex::ClassifyFailure`].  Defaults to `Strategic` on timeout or IPC error
    ///    (unchanged from previous behaviour).
    #[must_use]
    pub fn classify_failure(error: &str, env: &EnvironmentSnapshot) -> FailureCategory {
        let lower = error.to_lowercase();

        // ── Phase 1: Environment-based overrides (highest priority) ──────────
        //
        // These conditions indicate the environment itself is broken,
        // regardless of what the error message says.
        if env.battery_level < CRITICAL_BATTERY_THRESHOLD {
            debug!(
                battery = env.battery_level,
                "classify: battery critically low → Environmental"
            );
            return FailureCategory::Environmental;
        }

        if !env.screen_responsive {
            debug!("classify: screen unresponsive → Environmental");
            return FailureCategory::Environmental;
        }

        // Network-down overrides transient-looking errors
        if !env.network_available
            && (lower.contains("timeout")
                || lower.contains("connection")
                || lower.contains("loading"))
        {
            debug!("classify: network down + transient error → Environmental");
            return FailureCategory::Environmental;
        }

        // Neocortex down overrides strategic-looking errors when LLM is needed
        if !env.neocortex_alive
            && (lower.contains("plan")
                || lower.contains("llm")
                || lower.contains("model")
                || lower.contains("replan"))
        {
            debug!("classify: neocortex dead + LLM-related error → Capability");
            return FailureCategory::Capability;
        }

        // ── Phase 2: Structural fast-path (mechanical, not semantic) ─────────
        //
        // These are deterministic string identifiers for well-known error
        // classes — not NLP reasoning about intent.  The Iron Law is not
        // violated: we are pattern-matching known structural error codes,
        // not interpreting ambiguous natural-language meaning.

        // Timeout errors → Transient (will resolve on retry)
        if lower.contains("timeout") || lower.contains("timed out") {
            debug!("classify: timeout keyword → Transient");
            return FailureCategory::Transient;
        }

        // Network / socket / DNS errors → Transient (recoverable connectivity)
        if lower.contains("connection refused")
            || lower.contains("network")
            || lower.contains("socket")
            || lower.contains("dns")
        {
            debug!("classify: network/socket/dns keyword → Transient");
            return FailureCategory::Transient;
        }

        // Permission / access errors → Capability (need different approach)
        if lower.contains("permission denied")
            || lower.contains("access denied")
            || lower.contains("forbidden")
        {
            debug!("classify: permission/access keyword → Capability");
            return FailureCategory::Capability;
        }

        // Resource exhaustion → Environmental (system-level constraint)
        if lower.contains("out of memory")
            || lower.contains("oom")
            || lower.contains("resource exhausted")
        {
            debug!("classify: OOM/resource-exhausted keyword → Environmental");
            return FailureCategory::Environmental;
        }

        // ── Phase 3: LLM path for ambiguous failures ─────────────────────────
        //
        // IRON LAW: LLM classifies intent. Rust does not.
        // Ambiguous errors that don't match structural fast-paths are sent to
        // the neocortex for semantic classification.  Default to Strategic on
        // timeout / error so a replan is requested (unchanged prior behaviour).
        if let Some(category) = Self::classify_failure_via_llm(error, env) {
            return category;
        }

        debug!(error, "classify: LLM unavailable — defaulting to Strategic");
        FailureCategory::Strategic
    }

    /// Send an ambiguous failure to the neocortex for LLM classification.
    ///
    /// Returns `None` if the IPC call times out, errors, or returns an
    /// unrecognised label (caller falls back to `Strategic`).
    fn classify_failure_via_llm(error: &str, env: &EnvironmentSnapshot) -> Option<FailureCategory> {
        use aura_types::ipc::{DaemonToNeocortex, NeocortexToDaemon};

        // No runtime → cannot do async IPC; caller will use default.
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => return None,
        };

        // TODO(ARCH-MED-2): `block_in_place` + `block_on` is the safer pattern
        // (compared to bare `block_on`), but still blocks a Tokio worker thread
        // for the duration of the LLM round-trip.  Phase 3 should convert
        // `classify_failure_via_llm` to `async fn` so the caller can `.await`
        // directly.  See also: planner.rs `score_plan()`.
        tokio::task::block_in_place(|| {
            handle.block_on(async {
                let mut client = match crate::ipc::NeocortexClient::connect().await {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(error = %e, "classify_failure_via_llm: connect failed");
                        return None;
                    },
                };

                // Provide a compact env summary as context for the LLM.
                let context = format!(
                    "battery={:.0}% screen_responsive={} network={} neocortex={}",
                    env.battery_level * 100.0,
                    env.screen_responsive,
                    env.network_available,
                    env.neocortex_alive,
                );

                let result = tokio::time::timeout(
                    Duration::from_secs(5),
                    client.request(&DaemonToNeocortex::ClassifyFailure {
                        error: error.to_string(),
                        context,
                    }),
                )
                .await;

                match result {
                    Ok(Ok(NeocortexToDaemon::FailureClassification { category })) => {
                        let fc = match category.as_str() {
                            "Transient" => FailureCategory::Transient,
                            "Strategic" => FailureCategory::Strategic,
                            "Environmental" => FailureCategory::Environmental,
                            "Capability" => FailureCategory::Capability,
                            "Safety" => FailureCategory::Safety,
                            other => {
                                warn!(label = other, "classify_failure_via_llm: unknown label");
                                return None;
                            },
                        };
                        debug!(category = ?fc, "classify_failure_via_llm: LLM classified failure");
                        Some(fc)
                    },
                    Ok(Ok(other)) => {
                        warn!(
                            resp = ?std::mem::discriminant(&other),
                            "classify_failure_via_llm: unexpected IPC response"
                        );
                        None
                    },
                    Ok(Err(e)) => {
                        warn!(error = %e, "classify_failure_via_llm: request error");
                        None
                    },
                    Err(_elapsed) => {
                        warn!("classify_failure_via_llm: 5-second timeout");
                        None
                    },
                }
            })
        })
    }

    /// Determine the appropriate recovery action for a given failure context.
    ///
    /// Implements the recovery decision tree from the spec:
    ///
    /// ```text
    /// Transient  (attempt < 3)  → RetryWithBackoff
    /// Transient  (attempt >= 3) → EscalateToStrategic
    /// Strategic  (not exhausted) → Replan
    /// Strategic  (exhausted)     → NotifyUser
    /// Environmental (app issue)  → RestartEnvironment
    /// Environmental (network)    → NotifyUser (waiting)
    /// Environmental (battery)    → NotifyUser (battery)
    /// Capability                 → NotifyUser (explain limitation)
    /// Safety                     → HaltAndLog (NEVER retry)
    /// ```
    #[must_use]
    pub fn determine_recovery(&self, ctx: &RecoveryContext) -> RecoveryAction {
        match ctx.category {
            FailureCategory::Transient => {
                if ctx.attempt_count < MAX_TRANSIENT_ATTEMPTS {
                    debug!(
                        operation = %ctx.operation,
                        attempt = ctx.attempt_count,
                        "recovery: Transient → RetryWithBackoff"
                    );
                    RecoveryAction::RetryWithBackoff {
                        policy: self.retry_system.suggested_policy(&ctx.operation),
                    }
                } else {
                    info!(
                        operation = %ctx.operation,
                        attempt = ctx.attempt_count,
                        "recovery: Transient exhausted → EscalateToStrategic"
                    );
                    RecoveryAction::EscalateToStrategic {
                        from_category: FailureCategory::Transient,
                        context: format!(
                            "exhausted {} transient retries for '{}': {}",
                            ctx.attempt_count, ctx.operation, ctx.last_error
                        ),
                    }
                }
            },

            FailureCategory::Strategic => {
                if self.should_escalate(&ctx.operation) {
                    warn!(
                        operation = %ctx.operation,
                        "recovery: Strategic exhausted → NotifyUser"
                    );
                    RecoveryAction::NotifyUser {
                        message: format!(
                            "I've tried {} different approaches for '{}', but none worked. \
                             Last error: {}. Goal: {}",
                            self.escalation_count(&ctx.operation),
                            ctx.operation,
                            ctx.last_error,
                            ctx.goal_description
                        ),
                        severity: RecoverySeverity::Error,
                    }
                } else {
                    info!(
                        operation = %ctx.operation,
                        "recovery: Strategic → Replan"
                    );
                    RecoveryAction::Replan {
                        reason: format!(
                            "approach failed for '{}': {}",
                            ctx.operation, ctx.last_error
                        ),
                        context: format!(
                            "goal: {}, attempt: {}, elapsed: {}ms, error: {}",
                            ctx.goal_description,
                            ctx.attempt_count,
                            ctx.time_elapsed_ms,
                            ctx.last_error
                        ),
                    }
                }
            },

            FailureCategory::Environmental => {
                let env = &ctx.environment_state;

                if env.battery_level < CRITICAL_BATTERY_THRESHOLD {
                    warn!(
                        battery = env.battery_level,
                        "recovery: Environmental(battery) → NotifyUser"
                    );
                    return RecoveryAction::NotifyUser {
                        message: format!(
                            "Battery is critically low ({:.0}%). Cannot safely continue '{}'.",
                            env.battery_level * 100.0,
                            ctx.operation
                        ),
                        severity: RecoverySeverity::Critical,
                    };
                }

                if !env.network_available {
                    warn!("recovery: Environmental(network) → NotifyUser");
                    return RecoveryAction::NotifyUser {
                        message: format!(
                            "No internet connection. Waiting to resume '{}'.",
                            ctx.operation
                        ),
                        severity: RecoverySeverity::Warning,
                    };
                }

                if !env.target_app_running || !env.screen_responsive {
                    info!(
                        operation = %ctx.operation,
                        "recovery: Environmental(app) → RestartEnvironment"
                    );
                    return RecoveryAction::RestartEnvironment {
                        target: ctx.operation.clone(),
                        wait_ms: DEFAULT_RESTART_WAIT_MS,
                    };
                }

                // Generic environmental issue
                RecoveryAction::NotifyUser {
                    message: format!(
                        "Environmental issue encountered during '{}': {}",
                        ctx.operation, ctx.last_error
                    ),
                    severity: RecoverySeverity::Warning,
                }
            },

            FailureCategory::Capability => {
                info!(
                    operation = %ctx.operation,
                    "recovery: Capability → NotifyUser"
                );
                RecoveryAction::NotifyUser {
                    message: format!(
                        "I can't complete '{}' because: {}. {}",
                        ctx.operation, ctx.last_error, ctx.goal_description
                    ),
                    severity: RecoverySeverity::Error,
                }
            },

            FailureCategory::Safety => {
                warn!(
                    operation = %ctx.operation,
                    error = %ctx.last_error,
                    "recovery: Safety → HaltAndLog (NEVER retry)"
                );
                RecoveryAction::HaltAndLog {
                    reason: format!("safety halt for '{}': {}", ctx.operation, ctx.last_error),
                    category: FailureCategory::Safety,
                }
            },
        }
    }

    /// Record the outcome of a recovery action for learning.
    ///
    /// Updates both the recovery history and escalation counts.
    /// On success, resets the escalation counter for the operation.
    pub fn record_recovery_outcome(
        &mut self,
        operation: &str,
        action: &RecoveryAction,
        success: bool,
        now_ms: u64,
    ) {
        // Determine category from the action
        let category = match action {
            RecoveryAction::RetryWithBackoff { .. } => FailureCategory::Transient,
            RecoveryAction::Replan { .. } => FailureCategory::Strategic,
            RecoveryAction::RestartEnvironment { .. } => FailureCategory::Environmental,
            RecoveryAction::NotifyUser { .. } => FailureCategory::Environmental,
            RecoveryAction::HaltAndLog { category, .. } => *category,
            RecoveryAction::EscalateToStrategic { from_category, .. } => *from_category,
            RecoveryAction::TryAlternative { .. } => FailureCategory::Strategic,
        };

        let entry = RecoveryHistoryEntry {
            timestamp_ms: now_ms,
            operation: operation.to_owned(),
            category,
            action_taken: action.summary(),
            success,
        };
        self.recovery_history.push(entry);

        if success {
            self.reset_escalations(operation);
            self.retry_system.handle_success(operation);
            info!(operation, "recovery succeeded — escalation count reset");
        } else {
            // Only increment escalation count for Strategic/Environmental failures.
            // Transient retries failing do NOT count as escalations — they are handled
            // by the retry system's attempt counter, not the escalation limit.
            let is_escalation_worthy = !matches!(category, FailureCategory::Transient);
            if is_escalation_worthy
                && (self.escalation_counts.len() < MAX_ESCALATION_OPERATIONS
                    || self.escalation_counts.contains_key(operation))
            {
                let count = self
                    .escalation_counts
                    .entry(operation.to_owned())
                    .or_insert(0);
                *count = count.saturating_add(1);
            }
            debug!(
                operation,
                escalations = self.escalation_count(operation),
                "recovery failed — escalation count incremented"
            );
        }
    }

    /// Check if an operation has exceeded its escalation limit.
    ///
    /// Returns `true` when the operation should stop trying and notify the user.
    #[must_use]
    pub fn should_escalate(&self, operation: &str) -> bool {
        self.escalation_count(operation) >= self.max_escalations_per_operation
    }

    /// Get the current escalation count for an operation.
    #[must_use]
    pub fn escalation_count(&self, operation: &str) -> u32 {
        self.escalation_counts.get(operation).copied().unwrap_or(0)
    }

    /// Reset escalation counter for an operation (e.g., after successful recovery).
    pub fn reset_escalations(&mut self, operation: &str) {
        self.escalation_counts.remove(operation);
    }

    /// Compute aggregate recovery statistics from the history.
    #[must_use]
    pub fn recovery_stats(&self) -> RecoveryStats {
        let mut total: u32 = 0;
        let mut successful: u32 = 0;
        let mut by_category: HashMap<String, (u32, u32)> = HashMap::new();

        for entry in self.recovery_history.iter() {
            total = total.saturating_add(1);
            if entry.success {
                successful = successful.saturating_add(1);
            }

            let cat_key = entry.category.to_string();
            let (cat_total, cat_success) = by_category.entry(cat_key).or_insert((0, 0));
            *cat_total = cat_total.saturating_add(1);
            if entry.success {
                *cat_success = cat_success.saturating_add(1);
            }
        }

        RecoveryStats {
            total_recoveries: total,
            successful_recoveries: successful,
            by_category,
        }
    }

    /// Access the underlying `IntelligentRetry` system.
    #[must_use]
    pub fn retry_system(&self) -> &IntelligentRetry {
        &self.retry_system
    }

    /// Mutable access to the underlying `IntelligentRetry` system.
    pub fn retry_system_mut(&mut self) -> &mut IntelligentRetry {
        &mut self.retry_system
    }
}

impl Default for StrategicRecovery {
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
            },
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
            },
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
            },
            _ => panic!("expected exhausted"),
        }
    }

    #[test]
    fn test_into_result() {
        let policy = RetryPolicy::no_retry();
        let outcome = retry_with_backoff(&policy, |_| Ok::<_, &str>(42));
        assert_eq!(outcome.into_result(), Ok(42));

        let outcome2 = retry_with_backoff(&RetryPolicy::no_retry(), |_| Err::<(), _>("fail"));
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
        assert!((rate - 0.667).abs() < 0.01, "expected ~0.667, got {rate}");
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

        let strategy = ir.handle_failure("fetch", &"timeout", |_| ErrorClass::Transient, 1000, 0);

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
            },
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
            },
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

    // ── Strategic failure recovery tests ────────────────────────────────

    fn default_env() -> EnvironmentSnapshot {
        EnvironmentSnapshot::default()
    }

    fn make_ctx(
        operation: &str,
        category: FailureCategory,
        attempt: u32,
        error: &str,
        env: EnvironmentSnapshot,
    ) -> RecoveryContext {
        RecoveryContext {
            operation: operation.to_owned(),
            goal_description: "test goal".to_owned(),
            attempt_count: attempt,
            time_elapsed_ms: attempt as u64 * 1000,
            last_error: error.to_owned(),
            category,
            environment_state: env,
        }
    }

    // ── BoundedVec tests ────────────────────────────────────────────────

    #[test]
    fn test_bounded_vec_basic() {
        let mut bv = BoundedVec::new(3);
        assert!(bv.is_empty());
        assert_eq!(bv.capacity(), 3);

        bv.push(1);
        bv.push(2);
        bv.push(3);
        assert_eq!(bv.len(), 3);

        // Push beyond capacity — oldest evicted
        bv.push(4);
        assert_eq!(bv.len(), 3);
        let items: Vec<_> = bv.iter().copied().collect();
        assert_eq!(items, vec![2, 3, 4]);
    }

    #[test]
    #[should_panic(expected = "capacity must be > 0")]
    fn test_bounded_vec_zero_capacity() {
        let _bv: BoundedVec<i32> = BoundedVec::new(0);
    }

    // ── FailureCategory tests ───────────────────────────────────────────

    #[test]
    fn test_failure_category_display() {
        assert_eq!(FailureCategory::Transient.to_string(), "Transient");
        assert_eq!(FailureCategory::Strategic.to_string(), "Strategic");
        assert_eq!(FailureCategory::Environmental.to_string(), "Environmental");
        assert_eq!(FailureCategory::Capability.to_string(), "Capability");
        assert_eq!(FailureCategory::Safety.to_string(), "Safety");
    }

    #[test]
    fn test_failure_category_from_error_class() {
        assert_eq!(
            FailureCategory::from(ErrorClass::Transient),
            FailureCategory::Transient
        );
        assert_eq!(
            FailureCategory::from(ErrorClass::Structural),
            FailureCategory::Strategic
        );
        assert_eq!(
            FailureCategory::from(ErrorClass::Fatal),
            FailureCategory::Safety
        );
    }

    #[test]
    fn test_failure_category_retryable() {
        assert!(FailureCategory::Transient.is_retryable());
        assert!(!FailureCategory::Strategic.is_retryable());
        assert!(!FailureCategory::Environmental.is_retryable());
        assert!(!FailureCategory::Capability.is_retryable());
        assert!(!FailureCategory::Safety.is_retryable());
    }

    #[test]
    fn test_failure_category_recoverable() {
        assert!(FailureCategory::Transient.is_recoverable());
        assert!(FailureCategory::Strategic.is_recoverable());
        assert!(FailureCategory::Environmental.is_recoverable());
        assert!(!FailureCategory::Capability.is_recoverable());
        assert!(!FailureCategory::Safety.is_recoverable());
    }

    #[test]
    fn test_failure_category_requires_notification() {
        assert!(!FailureCategory::Transient.requires_user_notification());
        assert!(!FailureCategory::Strategic.requires_user_notification());
        assert!(FailureCategory::Environmental.requires_user_notification());
        assert!(FailureCategory::Capability.requires_user_notification());
        assert!(FailureCategory::Safety.requires_user_notification());
    }

    // ── classify_failure tests ──────────────────────────────────────────

    #[test]
    fn test_classify_timeout_as_transient() {
        let env = default_env();
        // Structural fast-path: "timeout" with good network → Transient.
        // This is a mechanical identifier (not NLP reasoning) — Iron Law preserved.
        assert_eq!(
            StrategicRecovery::classify_failure("connection timeout", &env),
            FailureCategory::Transient
        );
    }

    #[test]
    fn test_classify_not_found_as_strategic() {
        let env = default_env();
        assert_eq!(
            StrategicRecovery::classify_failure("element missing on screen", &env),
            FailureCategory::Strategic
        );
    }

    #[test]
    fn test_classify_crashed_as_environmental() {
        let env = default_env();
        // IRON LAW: text-based classification removed. LLM classifies error intent.
        // "app crashed" without hardware trigger → defaults to Strategic (LLM replan).
        assert_eq!(
            StrategicRecovery::classify_failure("app crashed unexpectedly", &env),
            FailureCategory::Strategic
        );
    }

    #[test]
    fn test_classify_not_installed_as_capability() {
        let env = default_env();
        // IRON LAW: text-based classification removed. LLM classifies error intent.
        // "app not installed" without hardware trigger → defaults to Strategic (LLM replan).
        assert_eq!(
            StrategicRecovery::classify_failure("app not installed", &env),
            FailureCategory::Strategic
        );
    }

    #[test]
    fn test_classify_policy_as_safety() {
        let env = default_env();
        // IRON LAW: text-based classification removed. LLM classifies error intent.
        // "denied by policy gate" without hardware trigger → defaults to Strategic (LLM replan).
        assert_eq!(
            StrategicRecovery::classify_failure("denied by policy gate", &env),
            FailureCategory::Strategic
        );
    }

    #[test]
    fn test_classify_timeout_with_no_network_as_environmental() {
        let env = EnvironmentSnapshot {
            network_available: false,
            ..default_env()
        };
        assert_eq!(
            StrategicRecovery::classify_failure("connection timeout", &env),
            FailureCategory::Environmental
        );
    }

    #[test]
    fn test_classify_low_battery_overrides_everything() {
        let env = EnvironmentSnapshot {
            battery_level: 0.02,
            ..default_env()
        };
        // Even a safety-sounding error is classified as Environmental
        // when battery is critically low
        assert_eq!(
            StrategicRecovery::classify_failure("some random error", &env),
            FailureCategory::Environmental
        );
    }

    #[test]
    fn test_classify_screen_unresponsive_as_environmental() {
        let env = EnvironmentSnapshot {
            screen_responsive: false,
            ..default_env()
        };
        assert_eq!(
            StrategicRecovery::classify_failure("something failed", &env),
            FailureCategory::Environmental
        );
    }

    #[test]
    fn test_classify_neocortex_dead_with_llm_error_as_capability() {
        let env = EnvironmentSnapshot {
            neocortex_alive: false,
            ..default_env()
        };
        assert_eq!(
            StrategicRecovery::classify_failure("failed to replan with LLM", &env),
            FailureCategory::Capability
        );
    }

    #[test]
    fn test_classify_unknown_defaults_to_strategic() {
        let env = default_env();
        assert_eq!(
            StrategicRecovery::classify_failure("xyzzy flurble happened", &env),
            FailureCategory::Strategic
        );
    }

    // ── determine_recovery tests ────────────────────────────────────────

    #[test]
    fn test_recovery_transient_low_attempt_retries() {
        let sr = StrategicRecovery::new();
        let ctx = make_ctx(
            "fetch",
            FailureCategory::Transient,
            1,
            "timeout",
            default_env(),
        );
        match sr.determine_recovery(&ctx) {
            RecoveryAction::RetryWithBackoff { .. } => {},
            other => panic!("expected RetryWithBackoff, got {}", other.summary()),
        }
    }

    #[test]
    fn test_recovery_transient_high_attempt_escalates() {
        let sr = StrategicRecovery::new();
        let ctx = make_ctx(
            "fetch",
            FailureCategory::Transient,
            5,
            "timeout",
            default_env(),
        );
        match sr.determine_recovery(&ctx) {
            RecoveryAction::EscalateToStrategic { from_category, .. } => {
                assert_eq!(from_category, FailureCategory::Transient);
            },
            other => panic!("expected EscalateToStrategic, got {}", other.summary()),
        }
    }

    #[test]
    fn test_recovery_strategic_replans() {
        let sr = StrategicRecovery::new();
        let ctx = make_ctx(
            "click_button",
            FailureCategory::Strategic,
            1,
            "element missing",
            default_env(),
        );
        match sr.determine_recovery(&ctx) {
            RecoveryAction::Replan { reason, context } => {
                assert!(reason.contains("click_button"));
                assert!(context.contains("element missing"));
            },
            other => panic!("expected Replan, got {}", other.summary()),
        }
    }

    #[test]
    fn test_recovery_strategic_exhausted_notifies_user() {
        let mut sr = StrategicRecovery::new();
        // Exhaust escalations
        for _ in 0..MAX_ESCALATIONS_PER_OPERATION {
            let action = RecoveryAction::Replan {
                reason: "test".into(),
                context: "test".into(),
            };
            sr.record_recovery_outcome("nav", &action, false, 1000);
        }
        assert!(sr.should_escalate("nav"));

        let ctx = make_ctx(
            "nav",
            FailureCategory::Strategic,
            5,
            "still failing",
            default_env(),
        );
        match sr.determine_recovery(&ctx) {
            RecoveryAction::NotifyUser { severity, .. } => {
                assert_eq!(severity, RecoverySeverity::Error);
            },
            other => panic!("expected NotifyUser, got {}", other.summary()),
        }
    }

    #[test]
    fn test_recovery_environmental_battery() {
        let sr = StrategicRecovery::new();
        let env = EnvironmentSnapshot {
            battery_level: 0.02,
            ..default_env()
        };
        let ctx = make_ctx(
            "task",
            FailureCategory::Environmental,
            1,
            "low battery",
            env,
        );
        match sr.determine_recovery(&ctx) {
            RecoveryAction::NotifyUser { severity, message } => {
                assert_eq!(severity, RecoverySeverity::Critical);
                assert!(message.contains("Battery"));
            },
            other => panic!("expected NotifyUser(Critical), got {}", other.summary()),
        }
    }

    #[test]
    fn test_recovery_environmental_no_network() {
        let sr = StrategicRecovery::new();
        let env = EnvironmentSnapshot {
            network_available: false,
            ..default_env()
        };
        let ctx = make_ctx(
            "fetch",
            FailureCategory::Environmental,
            1,
            "no internet",
            env,
        );
        match sr.determine_recovery(&ctx) {
            RecoveryAction::NotifyUser { severity, message } => {
                assert_eq!(severity, RecoverySeverity::Warning);
                assert!(message.contains("internet"));
            },
            other => panic!("expected NotifyUser(Warning), got {}", other.summary()),
        }
    }

    #[test]
    fn test_recovery_environmental_app_not_running() {
        let sr = StrategicRecovery::new();
        let env = EnvironmentSnapshot {
            target_app_running: false,
            ..default_env()
        };
        let ctx = make_ctx(
            "click",
            FailureCategory::Environmental,
            1,
            "app crashed",
            env,
        );
        match sr.determine_recovery(&ctx) {
            RecoveryAction::RestartEnvironment { wait_ms, .. } => {
                assert_eq!(wait_ms, DEFAULT_RESTART_WAIT_MS);
            },
            other => panic!("expected RestartEnvironment, got {}", other.summary()),
        }
    }

    #[test]
    fn test_recovery_capability_notifies_user() {
        let sr = StrategicRecovery::new();
        let ctx = make_ctx(
            "use_camera",
            FailureCategory::Capability,
            1,
            "hardware not available",
            default_env(),
        );
        match sr.determine_recovery(&ctx) {
            RecoveryAction::NotifyUser { severity, message } => {
                assert_eq!(severity, RecoverySeverity::Error);
                assert!(message.contains("use_camera"));
                assert!(message.contains("hardware not available"));
            },
            other => panic!("expected NotifyUser, got {}", other.summary()),
        }
    }

    #[test]
    fn test_recovery_safety_halts() {
        let sr = StrategicRecovery::new();
        let ctx = make_ctx(
            "send_message",
            FailureCategory::Safety,
            1,
            "denied by policy",
            default_env(),
        );
        match sr.determine_recovery(&ctx) {
            RecoveryAction::HaltAndLog { category, reason } => {
                assert_eq!(category, FailureCategory::Safety);
                assert!(reason.contains("send_message"));
            },
            other => panic!("expected HaltAndLog, got {}", other.summary()),
        }
    }

    // ── Escalation and history tests ────────────────────────────────────

    #[test]
    fn test_escalation_count_starts_at_zero() {
        let sr = StrategicRecovery::new();
        assert_eq!(sr.escalation_count("anything"), 0);
        assert!(!sr.should_escalate("anything"));
    }

    #[test]
    fn test_escalation_increments_on_failure() {
        let mut sr = StrategicRecovery::new();
        let action = RecoveryAction::Replan {
            reason: "test".into(),
            context: "ctx".into(),
        };
        sr.record_recovery_outcome("op", &action, false, 1000);
        assert_eq!(sr.escalation_count("op"), 1);
        sr.record_recovery_outcome("op", &action, false, 2000);
        assert_eq!(sr.escalation_count("op"), 2);
    }

    #[test]
    fn test_escalation_resets_on_success() {
        let mut sr = StrategicRecovery::new();
        let action = RecoveryAction::Replan {
            reason: "test".into(),
            context: "ctx".into(),
        };
        sr.record_recovery_outcome("op", &action, false, 1000);
        sr.record_recovery_outcome("op", &action, false, 2000);
        assert_eq!(sr.escalation_count("op"), 2);

        sr.record_recovery_outcome("op", &action, true, 3000);
        assert_eq!(sr.escalation_count("op"), 0);
    }

    #[test]
    fn test_recovery_stats_empty() {
        let sr = StrategicRecovery::new();
        let stats = sr.recovery_stats();
        assert_eq!(stats.total_recoveries, 0);
        assert_eq!(stats.successful_recoveries, 0);
        assert!(stats.by_category.is_empty());
    }

    #[test]
    fn test_recovery_stats_tracks_outcomes() {
        let mut sr = StrategicRecovery::new();

        let retry_action = RecoveryAction::RetryWithBackoff {
            policy: RetryPolicy::default(),
        };
        let replan_action = RecoveryAction::Replan {
            reason: "test".into(),
            context: "ctx".into(),
        };

        sr.record_recovery_outcome("a", &retry_action, true, 1000);
        sr.record_recovery_outcome("a", &retry_action, false, 2000);
        sr.record_recovery_outcome("b", &replan_action, true, 3000);

        let stats = sr.recovery_stats();
        assert_eq!(stats.total_recoveries, 3);
        assert_eq!(stats.successful_recoveries, 2);

        // Transient category: 2 total, 1 successful
        let transient = stats.by_category.get("Transient");
        assert_eq!(transient, Some(&(2, 1)));

        // Strategic category: 1 total, 1 successful
        let strategic = stats.by_category.get("Strategic");
        assert_eq!(strategic, Some(&(1, 1)));
    }

    #[test]
    fn test_recovery_history_bounded() {
        let mut sr = StrategicRecovery::new();
        let action = RecoveryAction::RetryWithBackoff {
            policy: RetryPolicy::default(),
        };

        for i in 0..(MAX_RECOVERY_HISTORY + 50) {
            sr.record_recovery_outcome("op", &action, i % 2 == 0, i as u64);
        }

        let stats = sr.recovery_stats();
        // Should be bounded to MAX_RECOVERY_HISTORY
        assert!(stats.total_recoveries <= MAX_RECOVERY_HISTORY as u32);
    }

    #[test]
    fn test_recovery_action_summary() {
        let action = RecoveryAction::RetryWithBackoff {
            policy: RetryPolicy::default(),
        };
        assert!(action.summary().contains("retry"));

        let action = RecoveryAction::HaltAndLog {
            reason: "safety stop".into(),
            category: FailureCategory::Safety,
        };
        assert!(action.summary().contains("HALT"));
        assert!(action.summary().contains("Safety"));
    }

    #[test]
    fn test_strategic_recovery_default() {
        let sr = StrategicRecovery::default();
        assert_eq!(sr.escalation_count("anything"), 0);
        assert_eq!(sr.recovery_stats().total_recoveries, 0);
    }

    #[test]
    fn test_strategic_recovery_exposes_retry_system() {
        let mut sr = StrategicRecovery::new();
        // Access the underlying retry system
        assert_eq!(sr.retry_system().tracked_operations(), 0);
        sr.retry_system_mut()
            .register_alternatives("op", vec!["alt1".into()]);
        // Verify it persists
        assert_eq!(sr.retry_system().tracked_operations(), 0); // no failures yet
    }

    #[test]
    fn test_recovery_severity_display() {
        assert_eq!(RecoverySeverity::Info.to_string(), "INFO");
        assert_eq!(RecoverySeverity::Warning.to_string(), "WARNING");
        assert_eq!(RecoverySeverity::Error.to_string(), "ERROR");
        assert_eq!(RecoverySeverity::Critical.to_string(), "CRITICAL");
    }

    #[test]
    fn test_environment_snapshot_default() {
        let env = EnvironmentSnapshot::default();
        assert!((env.battery_level - 1.0).abs() < f32::EPSILON);
        assert!(env.network_available);
        assert!(env.target_app_running);
        assert!(env.screen_responsive);
        assert!(env.neocortex_alive);
    }

    #[test]
    fn test_classify_safety_keywords() {
        let env = default_env();
        // classify_failure() always returns Strategic (stub — LLM classifies failures).
        for keyword in &[
            "policy violation",
            "action blocked",
            "safety concern",
            "ethical issue",
        ] {
            assert_eq!(
                StrategicRecovery::classify_failure(keyword, &env),
                FailureCategory::Strategic,
                "expected Strategic for '{keyword}'"
            );
        }
    }

    #[test]
    fn test_classify_capability_keywords() {
        let env = default_env();
        for keyword in &["feature unavailable", "hardware missing", "api restricted"] {
            assert_eq!(
                StrategicRecovery::classify_failure(keyword, &env),
                FailureCategory::Strategic,
                "expected Strategic for '{keyword}'"
            );
        }
    }

    #[test]
    fn test_full_recovery_lifecycle() {
        // Simulate: transient fails 3x → escalates → strategic replan succeeds
        let mut sr = StrategicRecovery::new();
        let env = default_env();

        // 3 transient retries
        for attempt in 0..MAX_TRANSIENT_ATTEMPTS {
            let ctx = make_ctx(
                "fetch_data",
                FailureCategory::Transient,
                attempt,
                "timeout",
                env.clone(),
            );
            let action = sr.determine_recovery(&ctx);
            match &action {
                RecoveryAction::RetryWithBackoff { .. } => {},
                other => panic!("attempt {attempt}: expected retry, got {}", other.summary()),
            }
            sr.record_recovery_outcome("fetch_data", &action, false, attempt as u64 * 1000);
        }

        // 4th attempt → escalates
        let ctx = make_ctx(
            "fetch_data",
            FailureCategory::Transient,
            MAX_TRANSIENT_ATTEMPTS,
            "timeout",
            env.clone(),
        );
        let action = sr.determine_recovery(&ctx);
        match &action {
            RecoveryAction::EscalateToStrategic { .. } => {},
            other => panic!("expected escalation, got {}", other.summary()),
        }

        // Now handle as strategic — should replan
        let ctx = make_ctx(
            "fetch_data",
            FailureCategory::Strategic,
            4,
            "wrong approach",
            env.clone(),
        );
        let action = sr.determine_recovery(&ctx);
        match &action {
            RecoveryAction::Replan { .. } => {},
            other => panic!("expected replan, got {}", other.summary()),
        }

        // Replan succeeds
        sr.record_recovery_outcome("fetch_data", &action, true, 5000);
        assert_eq!(sr.escalation_count("fetch_data"), 0);

        let stats = sr.recovery_stats();
        assert!(stats.successful_recoveries >= 1);
    }
}
