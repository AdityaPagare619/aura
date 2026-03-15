//! User interest model — tracks evolving user interests over time.
//!
//! The interest model observes user behaviour and maintains a bounded map
//! of topic → [`InterestEntry`] records.  Interests decay over time so the
//! model naturally forgets stale preferences.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, warn};

use super::super::{ArcError, DomainId};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of tracked interests.
pub const MAX_INTERESTS: usize = 256;

/// Maximum number of domain affinity entries (one per [`DomainId`]).
const MAX_DOMAIN_WEIGHTS: usize = 10;

/// Natural log of 2.
const LN2: f64 = std::f64::consts::LN_2;

/// EMA alpha for score updates.
const SCORE_ALPHA: f32 = 0.3;

/// EMA alpha for trend updates.
const TREND_ALPHA: f32 = 0.2;

/// Default half-life for interest decay: 30 days in milliseconds.
pub(crate) const DEFAULT_UPDATE_HALF_LIFE_MS: u64 = 30 * 24 * 3600 * 1000;

// ---------------------------------------------------------------------------
// InterestEntry
// ---------------------------------------------------------------------------

/// A single tracked user interest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterestEntry {
    /// Current interest score (0.0–1.0).
    pub score: f32,
    /// Number of times this interest was observed.
    pub observations: u32,
    /// Timestamp (ms) of most recent observation.
    pub last_observed_ms: u64,
    /// Rising (+) or falling (−) trend (−1.0 to +1.0).
    pub trend: f32,
}

// ---------------------------------------------------------------------------
// InterestModel
// ---------------------------------------------------------------------------

/// Bounded model of user interests with time-decay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterestModel {
    /// Topic → interest entry.
    interests: HashMap<String, InterestEntry>,
    /// Per-domain affinity weights.
    category_weights: HashMap<DomainId, f32>,

    /// Configurable half-life (ms) for interest score decay.
    /// Default: 30 days ([`DEFAULT_UPDATE_HALF_LIFE_MS`]).
    #[serde(default = "default_update_half_life_ms")]
    pub(crate) update_half_life_ms: u64,
}

fn default_update_half_life_ms() -> u64 {
    DEFAULT_UPDATE_HALF_LIFE_MS
}

impl InterestModel {
    /// Create an empty interest model with default domain weights.
    #[must_use]
    pub fn new() -> Self {
        let mut category_weights = HashMap::with_capacity(MAX_DOMAIN_WEIGHTS);
        for &d in &DomainId::ALL {
            category_weights.insert(d, d.default_weight());
        }
        Self {
            interests: HashMap::with_capacity(32),
            category_weights,
            update_half_life_ms: DEFAULT_UPDATE_HALF_LIFE_MS,
        }
    }

    /// Number of tracked interests.
    #[must_use]
    pub fn interest_count(&self) -> usize {
        self.interests.len()
    }

    /// The configured half-life (ms) for interest score decay.
    #[must_use]
    pub(crate) fn update_half_life_ms(&self) -> u64 {
        self.update_half_life_ms
    }

    /// Record an observation of user interest in `topic`.
    ///
    /// `strength` is a 0.0–1.0 signal of how strong this observation is
    /// (e.g., 1.0 = explicit user action, 0.3 = passive exposure).
    #[instrument(skip_all, fields(topic = %topic, strength))]
    pub fn observe_interest(
        &mut self,
        topic: &str,
        strength: f32,
        now_ms: u64,
    ) -> Result<(), ArcError> {
        let strength = strength.clamp(0.0, 1.0);

        if let Some(entry) = self.interests.get_mut(topic) {
            let old_score = entry.score;
            // EMA update
            entry.score =
                (entry.score * (1.0 - SCORE_ALPHA) + strength * SCORE_ALPHA).clamp(0.0, 1.0);
            // Trend: direction of change
            let delta = entry.score - old_score;
            entry.trend =
                (entry.trend * (1.0 - TREND_ALPHA) + delta.signum() * TREND_ALPHA).clamp(-1.0, 1.0);
            entry.observations = entry.observations.saturating_add(1);
            entry.last_observed_ms = now_ms;
            debug!(
                topic,
                score = entry.score,
                trend = entry.trend,
                "interest updated"
            );
        } else {
            // New interest — check capacity
            if self.interests.len() >= MAX_INTERESTS {
                self.evict_weakest()?;
            }
            self.interests.insert(
                topic.to_owned(),
                InterestEntry {
                    score: strength,
                    observations: 1,
                    last_observed_ms: now_ms,
                    trend: 0.0,
                },
            );
            debug!(topic, score = strength, "new interest tracked");
        }
        Ok(())
    }

    /// Get the top `n` interests by score, in descending order.
    #[must_use]
    pub fn get_top_interests(&self, n: usize) -> Vec<(&str, f32)> {
        let mut entries: Vec<(&str, f32)> = self
            .interests
            .iter()
            .map(|(k, v)| (k.as_str(), v.score))
            .collect();
        entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        entries.truncate(n);
        entries
    }

    /// Apply exponential decay to all interests.
    ///
    /// `half_life_ms`: the time in milliseconds for an interest score to
    /// halve in the absence of new observations.
    #[instrument(skip_all, fields(now_ms, half_life_ms))]
    pub fn decay(&mut self, now_ms: u64, half_life_ms: u64) {
        if half_life_ms == 0 {
            warn!("decay called with half_life_ms=0, skipping");
            return;
        }
        for entry in self.interests.values_mut() {
            let dt = now_ms.saturating_sub(entry.last_observed_ms) as f64;
            let factor = (-LN2 * dt / half_life_ms as f64).exp() as f32;
            entry.score = (entry.score * factor).clamp(0.0, 1.0);
        }
    }

    /// Get the affinity weight for a specific domain.
    #[must_use]
    pub fn get_domain_affinity(&self, domain: DomainId) -> f32 {
        self.category_weights.get(&domain).copied().unwrap_or(0.0)
    }

    /// Set the affinity weight for a domain.
    pub fn set_domain_affinity(&mut self, domain: DomainId, weight: f32) {
        self.category_weights.insert(domain, weight.clamp(0.0, 1.0));
    }

    /// Look up a specific interest by topic name.
    #[must_use]
    pub fn get_interest(&self, topic: &str) -> Option<&InterestEntry> {
        self.interests.get(topic)
    }

    // -- internals ----------------------------------------------------------

    /// Evict the interest with the lowest score.
    fn evict_weakest(&mut self) -> Result<(), ArcError> {
        let weakest = self
            .interests
            .iter()
            .min_by(|a, b| {
                a.1.score
                    .partial_cmp(&b.1.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(k, _)| k.clone());

        match weakest {
            Some(key) => {
                self.interests.remove(&key);
                debug!(evicted = %key, "evicted weakest interest");
                Ok(())
            },
            None => Err(ArcError::CapacityExceeded {
                collection: "interests".into(),
                max: MAX_INTERESTS,
            }),
        }
    }
}

impl Default for InterestModel {
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
    fn test_new_interest_model() {
        let model = InterestModel::new();
        assert_eq!(model.interest_count(), 0);
        // Should have default weights for all 10 domains
        for &d in &DomainId::ALL {
            assert!(model.get_domain_affinity(d) > 0.0);
        }
    }

    #[test]
    fn test_observe_interest_new() {
        let mut model = InterestModel::new();
        model
            .observe_interest("rust_programming", 0.8, 1000)
            .expect("observe");
        assert_eq!(model.interest_count(), 1);
        let entry = model.get_interest("rust_programming").expect("lookup");
        assert!((entry.score - 0.8).abs() < f32::EPSILON);
        assert_eq!(entry.observations, 1);
    }

    #[test]
    fn test_observe_interest_update() {
        let mut model = InterestModel::new();
        model.observe_interest("music", 0.6, 1000).expect("first");
        model.observe_interest("music", 0.9, 2000).expect("second");

        let entry = model.get_interest("music").expect("lookup");
        assert_eq!(entry.observations, 2);
        // Score should have moved toward 0.9 via EMA
        assert!(
            entry.score > 0.6,
            "score should have increased: {}",
            entry.score
        );
    }

    #[test]
    fn test_get_top_interests() {
        let mut model = InterestModel::new();
        model.observe_interest("low", 0.1, 100).expect("ok");
        model.observe_interest("mid", 0.5, 100).expect("ok");
        model.observe_interest("high", 0.9, 100).expect("ok");

        let top = model.get_top_interests(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, "high");
        assert_eq!(top[1].0, "mid");
    }

    #[test]
    fn test_decay_interests() {
        let mut model = InterestModel::new();
        model.observe_interest("fading", 1.0, 0).expect("ok");

        // After one half-life, score should be ≈ 0.5
        model.decay(1000, 1000);
        let entry = model.get_interest("fading").expect("lookup");
        assert!(
            (entry.score - 0.5).abs() < 0.05,
            "expected ~0.5 after one half-life, got {}",
            entry.score
        );
    }

    #[test]
    fn test_decay_zero_half_life() {
        let mut model = InterestModel::new();
        model.observe_interest("test", 0.8, 0).expect("ok");
        // Should not panic
        model.decay(1000, 0);
        // Score should be unchanged
        let entry = model.get_interest("test").expect("lookup");
        assert!((entry.score - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn test_domain_affinity() {
        let mut model = InterestModel::new();
        let orig = model.get_domain_affinity(DomainId::Health);
        model.set_domain_affinity(DomainId::Health, 0.95);
        assert!((model.get_domain_affinity(DomainId::Health) - 0.95).abs() < f32::EPSILON);
        assert!((orig - 1.0).abs() < f32::EPSILON); // Health default = 1.0
    }

    #[test]
    fn test_eviction_on_capacity() {
        let mut model = InterestModel::new();
        // Fill to MAX_INTERESTS
        for i in 0..MAX_INTERESTS {
            model
                .observe_interest(&format!("topic_{i}"), 0.5, 100)
                .expect("fill");
        }
        assert_eq!(model.interest_count(), MAX_INTERESTS);

        // One more should succeed via eviction
        model
            .observe_interest("overflow_topic", 0.9, 200)
            .expect("overflow");
        assert_eq!(model.interest_count(), MAX_INTERESTS);
        assert!(model.get_interest("overflow_topic").is_some());
    }

    #[test]
    fn test_strength_clamping() {
        let mut model = InterestModel::new();
        model.observe_interest("clamped", 5.0, 100).expect("clamp");
        let entry = model.get_interest("clamped").expect("lookup");
        assert!(
            (entry.score - 1.0).abs() < f32::EPSILON,
            "should clamp to 1.0"
        );
    }
}
