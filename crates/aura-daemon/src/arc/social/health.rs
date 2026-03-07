//! Relationship health tracking (spec §4.3).
//!
//! Computes per-relationship health using:
//! - Expected Contact Interval (ECI) learned via Exponential Moving Average (alpha=0.15)
//! - Gaussian pattern adherence: `adherence = exp(-(excess^2) / (2 * tolerance^2))`
//! - Composite health = adherence * sentiment_modifier * category_weight

use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use super::contacts::ContactCategory;
use crate::arc::ArcError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// EMA alpha for ECI learning.
const ECI_ALPHA: f64 = 0.15;

/// Minimum ECI in seconds (1 day — can't expect contact more than daily).
const MIN_ECI_SECS: f64 = 86400.0;

/// Maximum ECI in seconds (180 days).
const MAX_ECI_SECS: f64 = 180.0 * 86400.0;

/// Default ECI in seconds when no history exists (14 days).
const DEFAULT_ECI_SECS: f64 = 14.0 * 86400.0;

/// Maximum relationship health entries.
const MAX_RELATIONSHIP_ENTRIES: usize = 500;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Health trend direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthTrend {
    Improving,
    Stable,
    Declining,
    Critical,
}

/// Sentiment modifier based on recent interaction quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SentimentLevel {
    Positive,
    Neutral,
    Negative,
}

impl SentimentLevel {
    /// Multiplier for composite health.
    #[must_use]
    pub fn modifier(self) -> f32 {
        match self {
            SentimentLevel::Positive => 1.1,
            SentimentLevel::Neutral => 1.0,
            SentimentLevel::Negative => 0.8,
        }
    }
}

/// Per-relationship health record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipHealth {
    /// Contact ID.
    pub contact_id: u64,
    /// Learned Expected Contact Interval (seconds), via EMA.
    pub eci_secs: f64,
    /// Number of ECI observations for learning.
    pub eci_observations: u32,
    /// Current health score (0.0 to 1.0).
    pub health: f32,
    /// Trend direction.
    pub trend: HealthTrend,
    /// Recent sentiment.
    pub sentiment: SentimentLevel,
    /// Category weight for composite scoring.
    pub category_weight: f32,
    /// Timestamp of last health evaluation.
    pub last_eval_at: i64,
}

impl RelationshipHealth {
    /// Create a new relationship health record with defaults.
    #[must_use]
    pub fn new(contact_id: u64, category: ContactCategory) -> Self {
        Self {
            contact_id,
            eci_secs: DEFAULT_ECI_SECS,
            eci_observations: 0,
            health: 0.5,
            trend: HealthTrend::Stable,
            sentiment: SentimentLevel::Neutral,
            category_weight: category_weight(category),
            last_eval_at: 0,
        }
    }

    /// Update ECI with a new observed interval using EMA.
    ///
    /// `observed_interval_secs` is the time between the last two interactions.
    pub fn update_eci(&mut self, observed_interval_secs: f64) {
        let clamped = observed_interval_secs.clamp(MIN_ECI_SECS, MAX_ECI_SECS);

        if self.eci_observations == 0 {
            self.eci_secs = clamped;
        } else {
            // EMA: eci = alpha * observed + (1 - alpha) * eci
            self.eci_secs = ECI_ALPHA * clamped + (1.0 - ECI_ALPHA) * self.eci_secs;
        }
        self.eci_observations = self.eci_observations.saturating_add(1);
        self.eci_secs = self.eci_secs.clamp(MIN_ECI_SECS, MAX_ECI_SECS);
    }
}

// ---------------------------------------------------------------------------
// Gaussian adherence
// ---------------------------------------------------------------------------

/// Compute Gaussian pattern adherence.
///
/// `adherence = exp(-(excess^2) / (2 * tolerance^2))`
///
/// Where:
/// - `excess` = max(0, actual_gap - expected_gap), i.e., how much overdue
/// - `tolerance` = expected_gap * tolerance_fraction
///
/// A gap shorter than expected returns 1.0 (perfect).
#[must_use]
pub fn gaussian_adherence(
    actual_gap_secs: f64,
    expected_gap_secs: f64,
    tolerance_fraction: f64,
) -> f32 {
    if expected_gap_secs <= 0.0 {
        return 1.0;
    }

    let excess = (actual_gap_secs - expected_gap_secs).max(0.0);
    if excess <= 0.0 {
        return 1.0;
    }

    let tolerance = expected_gap_secs * tolerance_fraction;
    if tolerance <= 0.0 {
        return 0.0;
    }

    let adherence = (-excess * excess / (2.0 * tolerance * tolerance)).exp();
    adherence as f32
}

// ---------------------------------------------------------------------------
// Category weights
// ---------------------------------------------------------------------------

/// Weight for composite scoring by relationship category.
#[must_use]
fn category_weight(category: ContactCategory) -> f32 {
    match category {
        ContactCategory::Partner => 1.0,
        ContactCategory::Family => 0.9,
        ContactCategory::CloseFriend => 0.8,
        ContactCategory::Friend => 0.6,
        ContactCategory::Colleague => 0.4,
        ContactCategory::Professional => 0.3,
        ContactCategory::Acquaintance => 0.2,
        ContactCategory::Service => 0.1,
        ContactCategory::Other => 0.2,
    }
}

// ---------------------------------------------------------------------------
// RelationshipHealthEngine
// ---------------------------------------------------------------------------

/// Engine that manages per-relationship health scores.
#[derive(Debug, Serialize, Deserialize)]
pub struct RelationshipHealthEngine {
    entries: Vec<RelationshipHealth>,
}

impl RelationshipHealthEngine {
    /// Create a new empty engine.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::with_capacity(64),
        }
    }

    /// Get or create health entry for a contact.
    pub fn get_or_create(
        &mut self,
        contact_id: u64,
        category: ContactCategory,
    ) -> Result<&mut RelationshipHealth, ArcError> {
        let idx = self.entries.iter().position(|e| e.contact_id == contact_id);
        if let Some(i) = idx {
            return Ok(&mut self.entries[i]);
        }

        if self.entries.len() >= MAX_RELATIONSHIP_ENTRIES {
            return Err(ArcError::CapacityExceeded {
                collection: "relationship_health".into(),
                max: MAX_RELATIONSHIP_ENTRIES,
            });
        }

        self.entries
            .push(RelationshipHealth::new(contact_id, category));
        let last = self.entries.len() - 1;
        Ok(&mut self.entries[last])
    }

    /// Evaluate health for a contact given current gap.
    ///
    /// Returns the updated health score.
    #[instrument(name = "rel_health_eval", skip(self))]
    pub fn evaluate(
        &mut self,
        contact_id: u64,
        category: ContactCategory,
        last_interaction_at: i64,
        now: i64,
    ) -> Result<f32, ArcError> {
        let entry = self.get_or_create(contact_id, category)?;

        let gap_secs = (now - last_interaction_at).max(0) as f64;
        let adherence = gaussian_adherence(gap_secs, entry.eci_secs, 1.0);

        // Composite health = adherence * sentiment_modifier * category_weight
        let raw_health = adherence * entry.sentiment.modifier() * entry.category_weight;

        // Determine trend
        let prev_health = entry.health;
        entry.trend = if raw_health > prev_health + 0.05 {
            HealthTrend::Improving
        } else if raw_health < prev_health - 0.1 {
            if raw_health < 0.2 {
                HealthTrend::Critical
            } else {
                HealthTrend::Declining
            }
        } else {
            HealthTrend::Stable
        };

        entry.health = raw_health.clamp(0.0, 1.0);
        entry.last_eval_at = now;

        debug!(
            contact_id,
            adherence,
            health = entry.health,
            trend = ?entry.trend,
            "relationship health evaluated"
        );

        Ok(entry.health)
    }

    /// Record a new interaction and update ECI.
    pub fn record_interaction(
        &mut self,
        contact_id: u64,
        category: ContactCategory,
        previous_interaction_at: i64,
        current_interaction_at: i64,
    ) -> Result<(), ArcError> {
        let entry = self.get_or_create(contact_id, category)?;
        let interval = (current_interaction_at - previous_interaction_at).max(0) as f64;
        entry.update_eci(interval);
        Ok(())
    }

    /// Update sentiment for a relationship.
    pub fn update_sentiment(
        &mut self,
        contact_id: u64,
        category: ContactCategory,
        sentiment: SentimentLevel,
    ) -> Result<(), ArcError> {
        let entry = self.get_or_create(contact_id, category)?;
        entry.sentiment = sentiment;
        Ok(())
    }

    /// Average health across all tracked relationships (weighted by category).
    #[must_use]
    pub fn average_health(&self) -> f32 {
        if self.entries.is_empty() {
            return 0.5;
        }

        let mut weighted_sum = 0.0_f32;
        let mut weight_sum = 0.0_f32;

        for entry in &self.entries {
            weighted_sum += entry.health * entry.category_weight;
            weight_sum += entry.category_weight;
        }

        if weight_sum > 0.0 {
            (weighted_sum / weight_sum).clamp(0.0, 1.0)
        } else {
            0.5
        }
    }

    /// Get health for a specific contact.
    #[must_use]
    pub fn get_health(&self, contact_id: u64) -> Option<&RelationshipHealth> {
        self.entries.iter().find(|e| e.contact_id == contact_id)
    }

    /// Number of tracked relationships.
    #[must_use]
    pub fn tracked_count(&self) -> usize {
        self.entries.len()
    }
}

impl Default for RelationshipHealthEngine {
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
    fn test_gaussian_adherence_on_time() {
        // Gap equals expected → adherence = 1.0 (excess = 0)
        let a = gaussian_adherence(86400.0 * 7.0, 86400.0 * 7.0, 1.0);
        assert!((a - 1.0).abs() < 0.001, "got {a}");
    }

    #[test]
    fn test_gaussian_adherence_early() {
        // Gap shorter than expected → adherence = 1.0
        let a = gaussian_adherence(86400.0 * 3.0, 86400.0 * 7.0, 1.0);
        assert!((a - 1.0).abs() < 0.001, "got {a}");
    }

    #[test]
    fn test_gaussian_adherence_overdue() {
        // Gap = 2x expected, tolerance = 1.0
        // excess = 7d, tolerance = 7d
        // adherence = exp(-(7^2) / (2*7^2)) = exp(-0.5) ≈ 0.607
        let a = gaussian_adherence(86400.0 * 14.0, 86400.0 * 7.0, 1.0);
        assert!((a - 0.607).abs() < 0.01, "got {a}");
    }

    #[test]
    fn test_gaussian_adherence_very_overdue() {
        // Gap = 3x expected → adherence ≈ exp(-2) ≈ 0.135
        let a = gaussian_adherence(86400.0 * 21.0, 86400.0 * 7.0, 1.0);
        assert!((a - 0.135).abs() < 0.02, "got {a}");
    }

    #[test]
    fn test_eci_learning() {
        let mut rh = RelationshipHealth::new(1, ContactCategory::Friend);
        assert!((rh.eci_secs - DEFAULT_ECI_SECS).abs() < 1.0);

        // First observation: 7 days
        rh.update_eci(7.0 * 86400.0);
        assert!((rh.eci_secs - 7.0 * 86400.0).abs() < 1.0);

        // Second observation: 3 days → EMA moves toward 3d
        rh.update_eci(3.0 * 86400.0);
        // 0.15 * 3d + 0.85 * 7d = 0.45d + 5.95d = 6.4d
        let expected = 6.4 * 86400.0;
        assert!(
            (rh.eci_secs - expected).abs() < 86400.0,
            "got {} days",
            rh.eci_secs / 86400.0
        );
    }

    #[test]
    fn test_evaluate_recent_contact() {
        let mut engine = RelationshipHealthEngine::new();
        let now = 100_000;
        // Last contact 2 days ago (within default ECI of 14 days)
        let health = engine
            .evaluate(1, ContactCategory::Friend, now - 86400 * 2, now)
            .expect("eval");
        assert!(
            health > 0.3,
            "recent contact should be healthy, got {health}"
        );
    }

    #[test]
    fn test_evaluate_overdue_contact() {
        let mut engine = RelationshipHealthEngine::new();
        let now = 100_000;
        // Last contact 60 days ago (way past 14-day ECI)
        let health = engine
            .evaluate(1, ContactCategory::Friend, now - 86400 * 60, now)
            .expect("eval");
        assert!(
            health < 0.3,
            "overdue contact should be unhealthy, got {health}"
        );
    }

    #[test]
    fn test_sentiment_modifier() {
        assert!(SentimentLevel::Positive.modifier() > SentimentLevel::Neutral.modifier());
        assert!(SentimentLevel::Neutral.modifier() > SentimentLevel::Negative.modifier());
    }

    #[test]
    fn test_category_weights() {
        assert!(
            category_weight(ContactCategory::Partner) > category_weight(ContactCategory::Friend)
        );
        assert!(
            category_weight(ContactCategory::Family) > category_weight(ContactCategory::Colleague)
        );
    }

    #[test]
    fn test_average_health_empty() {
        let engine = RelationshipHealthEngine::new();
        assert!((engine.average_health() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_record_interaction_updates_eci() {
        let mut engine = RelationshipHealthEngine::new();
        engine
            .record_interaction(1, ContactCategory::Friend, 0, 86400 * 5)
            .expect("record");
        let entry = engine.get_health(1).expect("entry exists");
        assert_eq!(entry.eci_observations, 1);
    }
}
