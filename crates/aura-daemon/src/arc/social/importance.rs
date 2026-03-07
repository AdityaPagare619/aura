//! Contact importance scoring (spec §4.2).
//!
//! Computes importance for each contact using:
//!
//! ```text
//! importance = freq × 0.3 + recency × 0.3 + depth × 0.2 + explicit × 0.2
//! ```
//!
//! Each factor is normalized to [0.0, 1.0] via sigmoid or linear scaling.
//! The resulting importance value determines the contact's tier.

use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use super::contacts::{Contact, ContactTier};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Weight for interaction frequency.
const W_FREQ: f32 = 0.3;
/// Weight for recency of last interaction.
const W_RECENCY: f32 = 0.3;
/// Weight for message depth (conversation quality).
const W_DEPTH: f32 = 0.2;
/// Weight for explicit user-assigned importance.
const W_EXPLICIT: f32 = 0.2;

/// Sigmoid midpoint for frequency normalization (interactions per 30 days).
const FREQ_SIGMOID_MID: f32 = 10.0;
/// Sigmoid steepness for frequency.
const FREQ_SIGMOID_K: f32 = 0.3;

/// Recency half-life in seconds (7 days).
const RECENCY_HALF_LIFE: f64 = 7.0 * 86400.0;

/// Depth normalization: average words per message considered "deep".
const DEPTH_NORM_DEEP: f32 = 50.0;

// ---------------------------------------------------------------------------
// ImportanceScorer
// ---------------------------------------------------------------------------

/// Computes and assigns importance scores to contacts.
#[derive(Debug, Serialize, Deserialize)]
pub struct ImportanceScorer {
    /// Last scoring run timestamp.
    last_run_at: i64,
    /// Number of scoring cycles completed.
    cycle_count: u64,
}

impl ImportanceScorer {
    /// Create a new scorer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_run_at: 0,
            cycle_count: 0,
        }
    }

    /// Compute importance for a single contact.
    ///
    /// `now` is the current unix-epoch timestamp in seconds.
    /// `observation_window_days` is how many days of data to consider for frequency.
    #[must_use]
    pub fn compute_importance(
        &self,
        contact: &Contact,
        now: i64,
        observation_window_days: u32,
    ) -> f32 {
        let freq_score = self.frequency_score(contact, observation_window_days);
        let recency_score = self.recency_score(contact, now);
        let depth_score = self.depth_score(contact);
        let explicit_score = self.explicit_score(contact);

        let importance = W_FREQ * freq_score
            + W_RECENCY * recency_score
            + W_DEPTH * depth_score
            + W_EXPLICIT * explicit_score;

        importance.clamp(0.0, 1.0)
    }

    /// Score all contacts in a slice, updating their tier and returning
    /// the number updated.
    #[instrument(name = "importance_recalc", skip(self, contacts))]
    pub fn score_all(
        &mut self,
        contacts: &mut [Contact],
        now: i64,
        observation_window_days: u32,
    ) -> usize {
        let mut updated = 0;
        for contact in contacts.iter_mut() {
            let importance = self.compute_importance(contact, now, observation_window_days);
            let new_tier = Self::tier_from_importance(importance, contact, now);
            if new_tier != contact.tier {
                contact.tier = new_tier;
                updated += 1;
            }
        }
        self.last_run_at = now;
        self.cycle_count += 1;
        debug!(
            contacts = contacts.len(),
            updated, "importance recalculated"
        );
        updated
    }

    /// Frequency score: sigmoid of interaction count per window.
    ///
    /// `sigmoid(x) = 1 / (1 + exp(-k * (x - mid)))`
    fn frequency_score(&self, contact: &Contact, window_days: u32) -> f32 {
        let window_days = window_days.max(1) as f32;
        // Normalize to interactions per 30 days
        let rate = contact.interaction_count as f32 * (30.0 / window_days);
        sigmoid(rate, FREQ_SIGMOID_MID, FREQ_SIGMOID_K)
    }

    /// Recency score: exponential decay from last interaction.
    ///
    /// `score = exp(-0.693 * elapsed / half_life)`
    fn recency_score(&self, contact: &Contact, now: i64) -> f32 {
        if contact.last_interaction_at == 0 {
            return 0.0;
        }
        let elapsed = (now - contact.last_interaction_at).max(0) as f64;
        let decay = (-0.693 * elapsed / RECENCY_HALF_LIFE).exp();
        decay as f32
    }

    /// Depth score: normalized average message depth.
    fn depth_score(&self, contact: &Contact) -> f32 {
        (contact.avg_message_depth / DEPTH_NORM_DEEP).min(1.0)
    }

    /// Explicit score: user-assigned importance or default 0.5.
    fn explicit_score(&self, contact: &Contact) -> f32 {
        contact.explicit_importance.unwrap_or(0.5)
    }

    /// Determine tier from importance score and recency.
    ///
    /// Uses both the computed importance and the time since last interaction
    /// to assign tiers matching spec thresholds.
    #[must_use]
    pub fn tier_from_importance(importance: f32, contact: &Contact, now: i64) -> ContactTier {
        let days_since = if contact.last_interaction_at > 0 {
            ((now - contact.last_interaction_at) as f64 / 86400.0) as i64
        } else {
            i64::MAX
        };

        // Tier is primarily determined by recency with importance as a modifier
        if days_since <= 7 && importance >= 0.3 {
            ContactTier::Close
        } else if days_since <= 14 && importance >= 0.2 {
            ContactTier::Friend
        } else if days_since <= 30 {
            ContactTier::Acquaintance
        } else if days_since <= 90 {
            ContactTier::Contact
        } else {
            ContactTier::Dormant
        }
    }

    /// Number of scoring cycles completed.
    #[must_use]
    pub fn cycle_count(&self) -> u64 {
        self.cycle_count
    }
}

impl Default for ImportanceScorer {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Standard sigmoid function.
///
/// `sigmoid(x) = 1 / (1 + exp(-k * (x - mid)))`
#[must_use]
fn sigmoid(x: f32, mid: f32, k: f32) -> f32 {
    1.0 / (1.0 + (-k * (x - mid)).exp())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arc::social::contacts::{Contact, ContactCategory};

    fn make_active_contact(id: u64, interactions: u64, last_at: i64, depth: f32) -> Contact {
        let mut c = Contact::new(id, format!("Contact{id}"), ContactCategory::Friend);
        c.interaction_count = interactions;
        c.last_interaction_at = last_at;
        c.avg_message_depth = depth;
        c
    }

    #[test]
    fn test_sigmoid() {
        // At midpoint, sigmoid should be 0.5
        let val = sigmoid(10.0, 10.0, 0.3);
        assert!((val - 0.5).abs() < 0.001, "got {val}");

        // Well above midpoint should approach 1.0
        let high = sigmoid(30.0, 10.0, 0.3);
        assert!(high > 0.9, "got {high}");

        // Well below midpoint should approach 0.0
        let low = sigmoid(0.0, 10.0, 0.3);
        assert!(low < 0.15, "got {low}");
    }

    #[test]
    fn test_importance_high_activity() {
        let scorer = ImportanceScorer::new();
        let now = 100_000;
        let contact = make_active_contact(1, 50, now - 3600, 30.0); // Active 1h ago

        let importance = scorer.compute_importance(&contact, now, 30);
        assert!(
            importance > 0.5,
            "active contact should be important, got {importance}"
        );
    }

    #[test]
    fn test_importance_stale_contact() {
        let scorer = ImportanceScorer::new();
        let now = 100_000;
        let contact = make_active_contact(1, 2, now - 86400 * 60, 5.0); // 60 days ago

        let importance = scorer.compute_importance(&contact, now, 30);
        assert!(
            importance < 0.4,
            "stale contact should have low importance, got {importance}"
        );
    }

    #[test]
    fn test_tier_assignment() {
        // Use a realistic timestamp large enough that subtracting 100 days
        // still yields a positive last_interaction_at (the function treats
        // last_interaction_at <= 0 as "never contacted" → Dormant).
        let now = 10_000_000;

        // Recent, active → Close
        let c1 = make_active_contact(1, 20, now - 86400 * 3, 20.0);
        let tier = ImportanceScorer::tier_from_importance(0.6, &c1, now);
        assert_eq!(tier, ContactTier::Close);

        // 10 days ago → Friend
        let c2 = make_active_contact(2, 10, now - 86400 * 10, 15.0);
        let tier = ImportanceScorer::tier_from_importance(0.4, &c2, now);
        assert_eq!(tier, ContactTier::Friend);

        // 20 days ago → Acquaintance
        let c3 = make_active_contact(3, 5, now - 86400 * 20, 10.0);
        let tier = ImportanceScorer::tier_from_importance(0.3, &c3, now);
        assert_eq!(tier, ContactTier::Acquaintance);

        // 100 days ago → Dormant
        let c4 = make_active_contact(4, 1, now - 86400 * 100, 5.0);
        let tier = ImportanceScorer::tier_from_importance(0.1, &c4, now);
        assert_eq!(tier, ContactTier::Dormant);
    }

    #[test]
    fn test_explicit_importance_override() {
        let scorer = ImportanceScorer::new();
        let now = 100_000;
        let mut contact = make_active_contact(1, 1, now - 86400 * 30, 5.0);

        // Without explicit
        let base = scorer.compute_importance(&contact, now, 30);

        // With high explicit importance
        contact.explicit_importance = Some(1.0);
        let boosted = scorer.compute_importance(&contact, now, 30);

        assert!(boosted > base, "explicit importance should boost score");
    }

    #[test]
    fn test_score_all() {
        let mut scorer = ImportanceScorer::new();
        let now = 100_000;
        let mut contacts = vec![
            make_active_contact(1, 20, now - 86400 * 2, 20.0),
            make_active_contact(2, 1, now - 86400 * 100, 5.0),
        ];

        let _updated = scorer.score_all(&mut contacts, now, 30);
        assert_eq!(scorer.cycle_count(), 1);
        // Both should have been evaluated, at least one tier may change
        assert!(
            contacts[0].tier != ContactTier::Dormant || contacts[1].tier == ContactTier::Dormant
        );
    }

    #[test]
    fn test_weights_sum() {
        let sum = W_FREQ + W_RECENCY + W_DEPTH + W_EXPLICIT;
        assert!((sum - 1.0).abs() < 0.001, "weights sum = {sum}");
    }
}
