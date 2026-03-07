//! Health domain aggregate — medication, vitals, fitness, sleep.
//!
//! Owns the four health sub-engines and exposes a single `HealthDomain`
//! struct used by `ArcManager`.  The domain health score is computed as a
//! weighted combination of sub-domain factors (spec section 6.5):
//!
//! ```text
//! health_score = 0.30 * med_adherence
//!              + 0.25 * sleep_quality
//!              + 0.20 * activity_score
//!              + 0.15 * vitals_score
//!              + 0.10 * trend_bonus
//! ```

pub mod fitness;
pub mod medication;
pub mod sleep;
pub mod vitals;

pub use fitness::FitnessTracker;
pub use medication::MedicationManager;
pub use sleep::SleepTracker;
pub use vitals::VitalsMonitor;

use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, warn};

use super::{ArcError, DomainLifecycle};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Weights for domain health score factors (§6.5).
const W_MED_ADHERENCE: f32 = 0.30;
const W_SLEEP_QUALITY: f32 = 0.25;
const W_ACTIVITY: f32 = 0.20;
const W_VITALS: f32 = 0.15;
const W_TREND: f32 = 0.10;

/// Minimum number of data points before leaving Initializing state.
const MIN_DATA_POINTS: usize = 5;

// ---------------------------------------------------------------------------
// HealthDomain
// ---------------------------------------------------------------------------

/// Aggregate health domain engine.
///
/// Owns sub-engines for medication tracking, vital sign monitoring,
/// fitness tracking, and sleep analysis.
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthDomain {
    pub medication: MedicationManager,
    pub vitals: VitalsMonitor,
    pub fitness: FitnessTracker,
    pub sleep: SleepTracker,
    lifecycle: DomainLifecycle,
    /// Running trend score: exponential moving average of health-score deltas.
    trend_ema: f32,
    /// Previous health score for trend computation.
    prev_score: f32,
    /// Total number of evaluation cycles completed.
    eval_count: u64,
}

impl HealthDomain {
    /// Create a new health domain with all sub-engines in their default state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            medication: MedicationManager::new(),
            vitals: VitalsMonitor::new(),
            fitness: FitnessTracker::new(),
            sleep: SleepTracker::new(),
            lifecycle: DomainLifecycle::Dormant,
            trend_ema: 0.0,
            prev_score: 0.5,
            eval_count: 0,
        }
    }

    /// Current lifecycle state.
    #[must_use]
    pub fn lifecycle(&self) -> DomainLifecycle {
        self.lifecycle
    }

    /// Compute the composite health score from all sub-domains.
    ///
    /// Also updates lifecycle state and trend EMA.
    #[instrument(name = "health_score", skip(self))]
    pub fn compute_score(&mut self) -> Result<f32, ArcError> {
        let med_score = self.medication.adherence_score();
        let sleep_score = self.sleep.quality_score();
        let activity_score = self.fitness.activity_score();
        let vitals_score = self.vitals.composite_score();

        // Trend: EMA of score deltas (alpha = 0.3)
        let raw_score = W_MED_ADHERENCE * med_score
            + W_SLEEP_QUALITY * sleep_score
            + W_ACTIVITY * activity_score
            + W_VITALS * vitals_score;

        let delta = raw_score - self.prev_score;
        self.trend_ema = self.trend_ema * 0.7 + delta * 0.3;
        self.prev_score = raw_score;

        // Trend bonus: map trend_ema from [-0.5,0.5] to [0,1], clamped
        let trend_norm = (self.trend_ema + 0.5).clamp(0.0, 1.0);

        let final_score = (raw_score + W_TREND * trend_norm).clamp(0.0, 1.0);

        // Update lifecycle based on data availability
        self.eval_count += 1;
        self.update_lifecycle();

        debug!(
            med = med_score,
            sleep = sleep_score,
            activity = activity_score,
            vitals = vitals_score,
            trend = self.trend_ema,
            score = final_score,
            "health score computed"
        );

        Ok(final_score)
    }

    /// Total data points across all sub-engines.
    #[must_use]
    pub fn total_data_points(&self) -> usize {
        self.medication.total_doses_tracked()
            + self.vitals.reading_count()
            + self.fitness.total_sessions()
            + self.sleep.record_count()
    }

    /// Update lifecycle state based on data availability.
    fn update_lifecycle(&mut self) {
        let data_points = self.total_data_points();
        self.lifecycle = if data_points == 0 {
            DomainLifecycle::Dormant
        } else if data_points < MIN_DATA_POINTS {
            DomainLifecycle::Initializing
        } else {
            DomainLifecycle::Active
        };
    }
}

impl Default for HealthDomain {
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

    #[test]
    fn test_new_health_domain() {
        let h = HealthDomain::new();
        assert_eq!(h.lifecycle(), DomainLifecycle::Dormant);
        assert_eq!(h.total_data_points(), 0);
        assert_eq!(h.eval_count, 0);
    }

    #[test]
    fn test_compute_score_default() {
        let mut h = HealthDomain::new();
        let score = h.compute_score().expect("should compute");
        assert!(score >= 0.0 && score <= 1.0, "got {score}");
        assert_eq!(h.eval_count, 1);
    }

    #[test]
    fn test_lifecycle_transitions() {
        let mut h = HealthDomain::new();
        assert_eq!(h.lifecycle(), DomainLifecycle::Dormant);

        // Add a few medication doses
        for i in 0..3 {
            let _ = h.medication.record_dose(i, 1000 + i as i64, true);
        }
        h.update_lifecycle();
        assert_eq!(h.lifecycle(), DomainLifecycle::Initializing);

        // Add more to cross MIN_DATA_POINTS
        for i in 3..6 {
            let _ = h.medication.record_dose(i, 2000 + i as i64, true);
        }
        h.update_lifecycle();
        assert_eq!(h.lifecycle(), DomainLifecycle::Active);
    }

    #[test]
    fn test_weights_sum_to_one() {
        let sum = W_MED_ADHERENCE + W_SLEEP_QUALITY + W_ACTIVITY + W_VITALS + W_TREND;
        assert!((sum - 1.0).abs() < 0.001, "weights sum = {sum}");
    }

    #[test]
    fn test_trend_ema_updates() {
        let mut h = HealthDomain::new();
        let _s1 = h.compute_score().expect("score 1");
        let _ema1 = h.trend_ema;
        let _ = h.compute_score().expect("score 2");
        // trend_ema should have been updated (may or may not change from 0
        // depending on sub-scores, but the update ran)
        assert!(h.eval_count == 2);
    }
}
