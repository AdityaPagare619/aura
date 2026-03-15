//! Telemetry module — daemon self-monitoring via metrics ring buffer and
//! named atomic counters.
//!
//! # Architecture
//!
//! - [`MetricsRing`] — fixed-size circular buffer for time-series metrics. O(1) push, zero heap
//!   allocation on hot path.
//! - [`CounterSet`] — up to 64 named `AtomicU64` counters for lock-free event counting from any
//!   thread.
//! - [`TelemetryEngine`] — aggregate that owns both and provides a convenient recording API.

pub mod counters;
pub mod ring;

use std::time::{Duration, Instant};

pub use counters::{CounterError, CounterSet, CounterSnapshot, PREDEFINED_COUNTERS};
pub use ring::{LabelSummary, MetricEntry, MetricKind, MetricsRing, RingError};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// TelemetryEngine
// ---------------------------------------------------------------------------

/// Aggregate telemetry engine owning the ring buffer and counters.
///
/// Provides a unified API for recording metrics, incrementing counters,
/// measuring function execution time, and querying summaries.
pub struct TelemetryEngine {
    ring: MetricsRing<4096>,
    counters: CounterSet,
    started_at: Instant,
}

/// Summary of telemetry state, suitable for serialization / logging.
#[derive(Debug, Clone)]
pub struct TelemetrySummary {
    pub uptime_secs: f64,
    pub ring_entries: usize,
    pub ring_capacity: usize,
    pub counter_snapshots: Vec<CounterSnapshot>,
    pub metric_summaries: Vec<LabelSummary>,
}

/// Errors from the telemetry engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelemetryError {
    /// Ring buffer creation failed.
    RingInit(RingError),
    /// Counter operation failed.
    Counter(CounterError),
}

impl std::fmt::Display for TelemetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TelemetryError::RingInit(e) => write!(f, "telemetry ring init failed: {e}"),
            TelemetryError::Counter(e) => write!(f, "telemetry counter error: {e}"),
        }
    }
}

impl std::error::Error for TelemetryError {}

impl From<RingError> for TelemetryError {
    fn from(e: RingError) -> Self {
        TelemetryError::RingInit(e)
    }
}

impl From<CounterError> for TelemetryError {
    fn from(e: CounterError) -> Self {
        TelemetryError::Counter(e)
    }
}

impl TelemetryEngine {
    /// Create a new telemetry engine with default ring (4096) and all
    /// predefined counters.
    ///
    /// # Errors
    /// Returns `TelemetryError::RingInit` if the ring buffer cannot be
    /// allocated.
    pub fn new() -> Result<Self, TelemetryError> {
        let ring = MetricsRing::<4096>::new()?;
        let counters = CounterSet::with_defaults(0);
        debug!(
            ring_capacity = 4096,
            counters = counters.count(),
            "telemetry engine initialized"
        );
        Ok(Self {
            ring,
            counters,
            started_at: Instant::now(),
        })
    }

    /// Elapsed milliseconds since the engine was created.
    fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    // ── Recording ───────────────────────────────────────────────────────

    /// Record a metric entry into the ring buffer.
    pub fn record(&mut self, kind: MetricKind, label: &str, value: f64) {
        let ts = self.elapsed_ms();
        self.ring.record(ts, kind, label, value);
    }

    /// Increment a named counter by 1. Lock-free, safe from any thread
    /// (as long as `&self` is available via `Arc` or similar).
    ///
    /// Silently logs a warning if the counter doesn't exist.
    pub fn increment(&self, counter_name: &str) {
        if let Err(e) = self.counters.increment(counter_name) {
            warn!(counter = counter_name, error = %e, "failed to increment counter");
        }
    }

    /// Increment a named counter by a delta.
    ///
    /// Silently logs a warning if the counter doesn't exist.
    pub fn increment_by(&self, counter_name: &str, delta: u64) {
        if let Err(e) = self.counters.increment_by(counter_name, delta) {
            warn!(counter = counter_name, error = %e, "failed to increment counter");
        }
    }

    /// Record a gauge (point-in-time) metric.
    pub fn gauge(&mut self, label: &str, value: f64) {
        self.record(MetricKind::Gauge, label, value);
    }

    /// Measure the execution time of a closure and record it as a
    /// `Duration` metric (value in microseconds).
    ///
    /// Returns the closure's return value.
    pub fn time<F, R>(&mut self, label: &str, f: F) -> R
    where
        F: FnOnce() -> R, {
        let start = Instant::now();
        let result = f();
        let elapsed_us = start.elapsed().as_micros() as f64;
        self.record(MetricKind::Duration, label, elapsed_us);
        result
    }

    // ── Queries ─────────────────────────────────────────────────────────

    /// Produce a summary of all metrics within the given time window (ms).
    pub fn summary(&self, window_ms: u64) -> TelemetrySummary {
        let now = self.elapsed_ms();
        TelemetrySummary {
            uptime_secs: self.daemon_uptime().as_secs_f64(),
            ring_entries: self.ring.len(),
            ring_capacity: self.ring.capacity(),
            counter_snapshots: self.counters.snapshot(),
            metric_summaries: self.ring.summary(now, window_ms),
        }
    }

    /// How long the daemon has been running.
    pub fn daemon_uptime(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Access the underlying ring for advanced queries.
    pub fn ring(&self) -> &MetricsRing<4096> {
        &self.ring
    }

    /// Access the underlying counter set for direct operations.
    pub fn counters(&self) -> &CounterSet {
        &self.counters
    }

    /// Mutable access to counters for registration.
    pub fn counters_mut(&mut self) -> &mut CounterSet {
        &mut self.counters
    }

    /// Export ring contents in Prometheus text format.
    pub fn export_prometheus(&self) -> String {
        self.ring.export_prometheus()
    }
}

impl std::fmt::Debug for TelemetryEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelemetryEngine")
            .field("ring_len", &self.ring.len())
            .field("counter_count", &self.counters.count())
            .field("uptime", &self.started_at.elapsed())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation() {
        let engine = TelemetryEngine::new().expect("engine");
        assert!(engine.ring().is_empty());
        assert_eq!(engine.counters().count(), PREDEFINED_COUNTERS.len());
    }

    #[test]
    fn test_record_and_query() {
        let mut engine = TelemetryEngine::new().expect("engine");
        engine.record(MetricKind::Counter, "test_metric", 42.0);
        engine.record(MetricKind::Counter, "test_metric", 43.0);

        assert_eq!(engine.ring().len(), 2);
        let last = engine.ring().query_last(1);
        assert_eq!(last.len(), 1);
        assert!((last[0].value - 43.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_increment_predefined_counter() {
        let engine = TelemetryEngine::new().expect("engine");
        engine.increment("events_received");
        engine.increment("events_received");

        let c = engine.counters().get("events_received").expect("counter");
        assert_eq!(c.get(), 2);
    }

    #[test]
    fn test_gauge_recording() {
        let mut engine = TelemetryEngine::new().expect("engine");
        engine.gauge("cpu_usage", 67.5);

        let entries = engine.ring().query_by_label("cpu_usage");
        assert_eq!(entries.len(), 1);
        assert!((entries[0].value - 67.5).abs() < f64::EPSILON);
        assert_eq!(entries[0].kind, MetricKind::Gauge);
    }

    #[test]
    fn test_time_measurement() {
        let mut engine = TelemetryEngine::new().expect("engine");
        let result = engine.time("test_op", || {
            // Simulate a tiny computation.
            let mut sum = 0u64;
            for i in 0..1000 {
                sum += i;
            }
            sum
        });
        assert_eq!(result, 499_500);

        let entries = engine.ring().query_by_label("test_op");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, MetricKind::Duration);
        // Duration should be >= 0 microseconds.
        assert!(entries[0].value >= 0.0);
    }

    #[test]
    fn test_summary() {
        let mut engine = TelemetryEngine::new().expect("engine");
        engine.record(MetricKind::Gauge, "mem", 100.0);
        engine.record(MetricKind::Gauge, "mem", 200.0);
        engine.increment("events_received");

        let summary = engine.summary(60_000);
        assert!(summary.uptime_secs >= 0.0);
        assert_eq!(summary.ring_entries, 2);
        assert_eq!(summary.ring_capacity, 4096);
        assert!(!summary.counter_snapshots.is_empty());
    }

    #[test]
    fn test_daemon_uptime() {
        let engine = TelemetryEngine::new().expect("engine");
        std::thread::sleep(Duration::from_millis(10));
        let uptime = engine.daemon_uptime();
        assert!(uptime.as_millis() >= 10);
    }

    #[test]
    fn test_export_prometheus() {
        let mut engine = TelemetryEngine::new().expect("engine");
        engine.record(MetricKind::Counter, "test_prom", 1.0);
        let output = engine.export_prometheus();
        assert!(output.contains("aura_test_prom"));
    }

    #[test]
    fn test_increment_unknown_counter_does_not_panic() {
        let engine = TelemetryEngine::new().expect("engine");
        // Should just warn, not panic.
        engine.increment("nonexistent_counter");
    }
}
