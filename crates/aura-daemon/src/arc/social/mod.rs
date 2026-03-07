//! Social domain aggregate — contacts, relationships, social graph.
//!
//! Owns the six social sub-engines and exposes a single `SocialDomain`
//! struct used by `ArcManager`.

pub mod birthday;
pub mod contacts;
pub mod gap;
pub mod graph;
pub mod health;
pub mod importance;

pub use birthday::BirthdayTracker;
pub use contacts::ContactStore;
pub use gap::GapDetector;
pub use graph::SocialGraph;
pub use health::RelationshipHealthEngine;
pub use importance::ImportanceScorer;

use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use super::{ArcError, DomainLifecycle};

// ---------------------------------------------------------------------------
// SocialDomain
// ---------------------------------------------------------------------------

/// Aggregate social domain engine.
///
/// Owns sub-engines for contact management, importance scoring, relationship
/// health, gap detection, birthday tracking, and social graph analysis.
#[derive(Debug, Serialize, Deserialize)]
pub struct SocialDomain {
    pub contacts: ContactStore,
    pub importance: ImportanceScorer,
    pub relationship_health: RelationshipHealthEngine,
    pub gap_detector: GapDetector,
    pub birthdays: BirthdayTracker,
    pub graph: SocialGraph,
    lifecycle: DomainLifecycle,
    eval_count: u64,
}

impl SocialDomain {
    /// Create a new social domain with all sub-engines in their default state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            contacts: ContactStore::new(),
            importance: ImportanceScorer::new(),
            relationship_health: RelationshipHealthEngine::new(),
            gap_detector: GapDetector::new(),
            birthdays: BirthdayTracker::new(),
            graph: SocialGraph::new(),
            lifecycle: DomainLifecycle::Dormant,
            eval_count: 0,
        }
    }

    /// Current lifecycle state.
    #[must_use]
    pub fn lifecycle(&self) -> DomainLifecycle {
        self.lifecycle
    }

    /// Compute the composite social health score.
    ///
    /// Weights: relationship_health 0.35 + gap_score 0.25 + contact_coverage 0.20
    /// + social_diversity 0.20
    #[instrument(name = "social_score", skip(self))]
    pub fn compute_score(&mut self) -> Result<f32, ArcError> {
        let rel_health = self.relationship_health.average_health();
        let gap_score = self.gap_detector.health_score();
        let contact_coverage = self.contact_coverage();
        let diversity = self.graph.diversity_score();

        let score =
            rel_health * 0.35 + gap_score * 0.25 + contact_coverage * 0.20 + diversity * 0.20;

        self.eval_count += 1;
        self.update_lifecycle();

        debug!(
            rel_health,
            gap_score, contact_coverage, diversity, score, "social score computed"
        );

        Ok(score.clamp(0.0, 1.0))
    }

    /// How well-covered is the user's social life (have contacts across categories).
    fn contact_coverage(&self) -> f32 {
        let total = self.contacts.total_contacts();
        if total == 0 {
            return 0.0;
        }
        // Simple: more contacts (up to 50) = better coverage
        (total as f32 / 50.0).min(1.0)
    }

    /// Update lifecycle based on data availability.
    fn update_lifecycle(&mut self) {
        let total = self.contacts.total_contacts();
        self.lifecycle = if total == 0 {
            DomainLifecycle::Dormant
        } else if total < 3 {
            DomainLifecycle::Initializing
        } else {
            DomainLifecycle::Active
        };
    }
}

impl Default for SocialDomain {
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
    fn test_new_social_domain() {
        let s = SocialDomain::new();
        assert_eq!(s.lifecycle(), DomainLifecycle::Dormant);
        assert_eq!(s.eval_count, 0);
    }

    #[test]
    fn test_compute_score_empty() {
        let mut s = SocialDomain::new();
        let score = s.compute_score().expect("compute");
        assert!(score >= 0.0 && score <= 1.0, "got {score}");
    }

    #[test]
    fn test_contact_coverage() {
        let s = SocialDomain::new();
        assert!((s.contact_coverage() - 0.0).abs() < 0.001);
    }
}
