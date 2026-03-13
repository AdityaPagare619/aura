//! Routine deviation detection — the circadian awareness layer.
//!
//! A *routine* is any recurring behaviour the user repeats at consistent times
//! (morning coffee, evening walk, weekly stand-up). `RoutineWindow` learns the
//! expected time-of-day for each routine using **Welford's online algorithm**
//! for numerically stable incremental mean/variance — no stored history
//! required, O(1) memory regardless of how many observations arrive.
//!
//! # Day-zero law
//! On day zero (fewer than `MIN_SAMPLES` observations) `check_deviation`
//! returns `None`. AURA must never generate alerts from an empty baseline.
//!
//! # Thread safety
//! `RoutineWindow` is `Send + Sync` — it holds no interior mutability.
//! Callers are responsible for synchronisation (e.g., wrap in a `Mutex`).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Minimum number of observations before deviation detection activates.
/// Below this threshold `check_deviation` always returns `None` — day-zero
/// law compliance.
const MIN_SAMPLES: u32 = 7;

/// Sigma multiplier: deviations beyond this many standard deviations are
/// reported. 2.0 ≈ 95th percentile of a normal distribution.
const DEVIATION_SIGMA_THRESHOLD: f64 = 2.0;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of a deviation check.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviationResult {
    /// How many standard deviations the observed time is from the learned mean.
    pub sigma: f64,
    /// The learned mean time-of-day (minutes since midnight).
    pub expected_minutes: f64,
    /// The observed time-of-day (minutes since midnight).
    pub observed_minutes: f64,
}

/// A single learned routine window for one recurring behaviour.
///
/// Tracks the expected time-of-day (in **minutes since midnight**, float)
/// using Welford's online mean/variance algorithm. Optionally stratified by
/// weekday so Monday routines don't pollute weekend baselines.
///
/// # Example
/// ```ignore
/// let mut window = RoutineWindow::new(Some(1)); // Monday only
/// for &t in &[480.0_f64, 485.0, 478.0, 482.0, 479.0, 481.0, 483.0] {
///     window.observe(t);
/// }
/// let result = window.check_deviation(530.0); // 8:50 AM — late
/// assert!(result.is_some());
/// assert!(result.unwrap().sigma > 2.0);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutineWindow {
    /// Optional weekday filter (0 = Sunday … 6 = Saturday).
    /// `None` = all days treated identically.
    pub day_of_week: Option<u8>,

    /// Number of observations accumulated so far.
    count: u32,

    /// Welford's running mean (minutes since midnight).
    mean: f64,

    /// Welford's running M₂ — sum of squared deviations from the current mean.
    /// Variance = M₂ / (count - 1) when count ≥ 2.
    m2: f64,
}

impl RoutineWindow {
    /// Create a new, empty window.
    ///
    /// Pass `Some(weekday)` (0–6) to restrict this window to a specific day.
    pub fn new(day_of_week: Option<u8>) -> Self {
        Self {
            day_of_week,
            count: 0,
            mean: 0.0,
            m2: 0.0,
        }
    }

    /// Feed a new observation (minutes since midnight, e.g. `8 * 60 + 30` = 510.0).
    ///
    /// Uses Welford's online algorithm for numerically stable incremental
    /// mean and variance. O(1) time and memory.
    pub fn observe(&mut self, minutes_since_midnight: f64) {
        self.count += 1;
        let delta = minutes_since_midnight - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = minutes_since_midnight - self.mean;
        self.m2 += delta * delta2;
    }

    /// Number of observations accumulated.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Current learned mean (minutes since midnight).
    /// Returns `None` if no observations have been made yet.
    pub fn mean(&self) -> Option<f64> {
        if self.count == 0 {
            None
        } else {
            Some(self.mean)
        }
    }

    /// Sample variance of accumulated observations.
    /// Returns `None` if fewer than 2 observations have been made.
    pub fn variance(&self) -> Option<f64> {
        if self.count < 2 {
            None
        } else {
            Some(self.m2 / (self.count - 1) as f64)
        }
    }

    /// Sample standard deviation.
    /// Returns `None` if fewer than 2 observations have been made.
    pub fn std_dev(&self) -> Option<f64> {
        self.variance().map(f64::sqrt)
    }

    /// Check whether `observed_minutes` deviates significantly from the
    /// learned baseline.
    ///
    /// Returns:
    /// - `None` — fewer than `MIN_SAMPLES` observations (day-zero law).
    /// - `None` — deviation is within `DEVIATION_SIGMA_THRESHOLD` σ (normal).
    /// - `Some(DeviationResult)` — observed time is anomalous.
    ///
    /// `observed_minutes` — minutes since midnight for the current occurrence
    /// (use real system clock; this module is a pure sensor — it reasons nothing).
    pub fn check_deviation(&self, observed_minutes: f64) -> Option<DeviationResult> {
        // Day-zero law: refuse to fire alerts without a baseline.
        if self.count < MIN_SAMPLES {
            return None;
        }

        let std = match self.std_dev() {
            Some(s) if s > f64::EPSILON => s,
            // Near-zero variance: user is extremely consistent.
            // Use a 1-minute floor so we never divide by zero and still
            // detect meaningful deviations (> 2 minutes = 2 sigma).
            _ => 1.0,
        };

        let sigma = (observed_minutes - self.mean).abs() / std;

        if sigma >= DEVIATION_SIGMA_THRESHOLD {
            Some(DeviationResult {
                sigma,
                expected_minutes: self.mean,
                observed_minutes,
            })
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(window: &mut RoutineWindow, observations: &[f64]) {
        for &t in observations {
            window.observe(t);
        }
    }

    #[test]
    fn day_zero_returns_none() {
        let w = RoutineWindow::new(None);
        // 0 observations — must return None regardless of input.
        assert!(w.check_deviation(480.0).is_none());
    }

    #[test]
    fn below_min_samples_returns_none() {
        let mut w = RoutineWindow::new(None);
        // MIN_SAMPLES - 1 = 6 observations — still below threshold.
        feed(&mut w, &[480.0, 481.0, 479.0, 482.0, 478.0, 480.5]);
        assert!(w.check_deviation(600.0).is_none()); // wildly different time, still None
    }

    #[test]
    fn normal_observation_no_deviation() {
        let mut w = RoutineWindow::new(None);
        // 7 tightly-clustered observations around 480 min (8:00 AM).
        feed(&mut w, &[480.0, 481.0, 479.0, 480.5, 480.0, 481.5, 479.5]);
        // 481 min (8:01 AM) — within 2σ threshold, should be None.
        assert!(w.check_deviation(481.0).is_none());
    }

    #[test]
    fn large_deviation_detected() {
        let mut w = RoutineWindow::new(None);
        // 7 observations tightly around 480 min (8:00 AM), std ≈ 1 min.
        feed(&mut w, &[480.0, 481.0, 479.0, 480.5, 480.0, 481.5, 479.5]);
        // 510 min (8:30 AM) — 30 min late → >> 2σ.
        let result = w.check_deviation(510.0);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.sigma > DEVIATION_SIGMA_THRESHOLD);
        assert!((r.expected_minutes - 480.285).abs() < 0.1);
    }

    #[test]
    fn welford_mean_is_accurate() {
        let mut w = RoutineWindow::new(None);
        feed(&mut w, &[100.0, 200.0, 300.0]);
        let mean = w.mean().expect("should have mean after 3 observations");
        assert!((mean - 200.0).abs() < f64::EPSILON * 10.0);
    }
}
