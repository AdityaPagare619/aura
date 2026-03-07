//! Learning engine — pattern learning, Hebbian concept reinforcement, and
//! adaptive skill acquisition.
//!
//! # Architecture (SPEC-ARC §8.4)
//!
//! The learning engine implements:
//! 1. **Concepts** — abstract ideas AURA has learned about (apps, actions, preferences)
//! 2. **Associations** — weighted Hebbian links between concepts
//! 3. **Skills** — learned action sequences that improve over time
//! 4. **Interests** — an evolving user interest model
//!
//! # Consolidation (§8.4.2)
//!
//! ```text
//! consolidation_score = recency × 0.3 + frequency × 0.3 + importance × 0.4
//! threshold = 0.7
//! ```

pub mod dreaming;
pub mod dimensions;
pub mod hebbian;
pub mod interests;
pub mod patterns;
pub mod prediction;
pub mod skills;

pub use dreaming::DreamingEngine;
pub use dimensions::DimensionDiscovery;
pub use hebbian::HebbianNetwork;
pub use interests::InterestModel;
pub use patterns::PatternDetector;
pub use prediction::PredictionEngine;
pub use skills::SkillRegistry;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use super::ArcError;
use super::DomainId;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Association weight threshold for pruning during consolidation.
const PRUNE_THRESHOLD: f32 = 0.01;

/// Consolidation threshold — concepts above this are strengthened.
const CONSOLIDATION_THRESHOLD: f32 = 0.7;

/// Half-life (ms) for association decay: 7 days.
const ASSOCIATION_HALF_LIFE_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// Amount to boost importance for consolidated concepts.
const CONSOLIDATION_IMPORTANCE_BOOST: f32 = 0.02;

// ---------------------------------------------------------------------------
// Outcome
// ---------------------------------------------------------------------------

/// The result of an observed action or event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Outcome {
    /// The action succeeded.
    Success,
    /// The action failed.
    Failure,
    /// The outcome is neutral / unknown.
    Neutral,
}

// ---------------------------------------------------------------------------
// ConsolidationReport
// ---------------------------------------------------------------------------

/// Summary of a consolidation pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationReport {
    /// Number of concepts whose importance was boosted.
    pub concepts_strengthened: usize,
    /// Number of weak associations that were removed.
    pub concepts_pruned: usize,
    /// Number of associations whose weights were updated (via decay).
    pub associations_updated: usize,
}

// ---------------------------------------------------------------------------
// LearningEngine
// ---------------------------------------------------------------------------

/// Top-level learning engine aggregate.
///
/// Owns the seven sub-engines: [`HebbianNetwork`], [`InterestModel`],
/// [`SkillRegistry`], [`PatternDetector`], [`DreamingEngine`],
/// [`PredictionEngine`], and [`DimensionDiscovery`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningEngine {
    /// Hebbian concept learning network.
    pub hebbian: HebbianNetwork,
    /// User interest model.
    pub interests: InterestModel,
    /// Learned skill registry.
    pub skills: SkillRegistry,
    /// Pattern detection engine.
    pub patterns: PatternDetector,
    /// Dreaming engine for autonomous exploration.
    pub dreaming: DreamingEngine,
    /// Active Inference prediction engine (Concept Design §4.2).
    pub prediction: PredictionEngine,
    /// Emergent dimension discovery engine (Concept Design §4.1).
    pub dimensions: DimensionDiscovery,
}

impl LearningEngine {
    /// Create a new learning engine with empty sub-engines.
    #[must_use]
    pub fn new() -> Self {
        Self {
            hebbian: HebbianNetwork::new(),
            interests: InterestModel::new(),
            skills: SkillRegistry::new(),
            patterns: PatternDetector::new(),
            dreaming: DreamingEngine::new(),
            prediction: PredictionEngine::new(),
            dimensions: DimensionDiscovery::new(),
        }
    }

    /// Observe co-occurrence of two concepts with an outcome.
    ///
    /// 1. Gets or creates both concepts.
    /// 2. Activates both with the outcome.
    /// 3. Strengthens or weakens the association depending on outcome.
    #[instrument(skip_all, fields(concept_a = %concept_a, concept_b = %concept_b, ?outcome))]
    pub fn observe(
        &mut self,
        concept_a: &str,
        concept_b: &str,
        outcome: Outcome,
        now_ms: u64,
    ) -> Result<(), ArcError> {
        let id_a = self.hebbian.get_or_create_concept(concept_a, now_ms)?;
        let id_b = self.hebbian.get_or_create_concept(concept_b, now_ms)?;

        // Importance signal derived from outcome
        let importance = match outcome {
            Outcome::Success => 0.7,
            Outcome::Failure => 0.6,
            Outcome::Neutral => 0.4,
        };

        self.hebbian.activate(id_a, outcome, importance, now_ms)?;
        self.hebbian.activate(id_b, outcome, importance, now_ms)?;

        match outcome {
            Outcome::Success | Outcome::Neutral => {
                self.hebbian.strengthen_association(id_a, id_b, now_ms)?;
            }
            Outcome::Failure => {
                self.hebbian.weaken_association(id_a, id_b)?;
            }
        }

        debug!(concept_a, concept_b, ?outcome, "observation recorded");
        Ok(())
    }

    /// Run a consolidation pass over the Hebbian network.
    ///
    /// 1. Apply time-decay to all association weights.
    /// 2. For each concept, compute consolidation score.
    ///    - If ≥ threshold (0.7), boost importance slightly.
    /// 3. Prune weak associations below [`PRUNE_THRESHOLD`].
    #[instrument(skip_all, fields(now_ms))]
    pub fn consolidate(&mut self, now_ms: u64) -> Result<ConsolidationReport, ArcError> {
        // Step 1: decay
        let assoc_count_before = self.hebbian.association_count();
        self.hebbian.decay_all(now_ms, ASSOCIATION_HALF_LIFE_MS);

        // Step 2: evaluate consolidation scores
        let concept_ids = self.hebbian.concept_ids();
        let mut concepts_strengthened = 0_usize;

        for &id in &concept_ids {
            if let Some(score) = self.hebbian.consolidation_score(id, now_ms) {
                if score >= CONSOLIDATION_THRESHOLD {
                    // Boost importance for well-consolidated concepts
                    if let Some(concept) = self.hebbian.get_concept(id) {
                        let new_importance =
                            (concept.importance + CONSOLIDATION_IMPORTANCE_BOOST).min(1.0);
                        // Re-activate with boosted importance but neutral outcome
                        self.hebbian
                            .activate(id, Outcome::Neutral, new_importance, now_ms)?;
                        concepts_strengthened += 1;
                    }
                }
            }
        }

        // Step 3: prune weak associations
        let concepts_pruned = self.hebbian.prune_weak(PRUNE_THRESHOLD);

        let report = ConsolidationReport {
            concepts_strengthened,
            concepts_pruned,
            associations_updated: assoc_count_before,
        };

        info!(
            strengthened = report.concepts_strengthened,
            pruned = report.concepts_pruned,
            decayed = report.associations_updated,
            "consolidation pass complete"
        );

        Ok(report)
    }

    /// Record suggestion feedback for Hebbian learning.
    ///
    /// When a user accepts or rejects a suggestion, this creates Hebbian associations
    /// between the suggestion category and the outcome, enabling the proactive engine
    /// to learn from feedback over time.
    ///
    /// # Arguments
    /// * `category` - The domain category of the suggestion
    /// * `suggestion_text` - The text of the suggestion (used for context concepts)
    /// * `accepted` - Whether the user accepted the suggestion
    /// * `now_ms` - Current timestamp in milliseconds
    #[instrument(skip_all, fields(category = %category, accepted, text_len = suggestion_text.len()))]
    pub fn record_suggestion_feedback(
        &mut self,
        category: DomainId,
        suggestion_text: &str,
        accepted: bool,
        now_ms: u64,
    ) -> Result<(), ArcError> {
        let outcome = if accepted {
            Outcome::Success
        } else {
            Outcome::Failure
        };

        let category_name = category.to_string();
        let id_cat = self.hebbian.get_or_create_concept(&category_name, now_ms)?;

        self.hebbian.activate(id_cat, outcome, 0.8, now_ms)?;

        if accepted {
            let accepted_concept = self
                .hebbian
                .get_or_create_concept("suggestion_accepted", now_ms)?;
            self.hebbian
                .strengthen_association(id_cat, accepted_concept, now_ms)?;
        } else {
            let rejected_concept = self
                .hebbian
                .get_or_create_concept("suggestion_rejected", now_ms)?;
            self.hebbian.weaken_association(id_cat, rejected_concept)?;
        }

        let words: Vec<&str> = suggestion_text.split_whitespace().take(3).collect();
        for word in words {
            if word.len() > 2 {
                let word_id = self.hebbian.get_or_create_concept(word, now_ms)?;
                match outcome {
                    Outcome::Success => {
                        self.hebbian
                            .strengthen_association(id_cat, word_id, now_ms)?;
                    }
                    Outcome::Failure => {
                        self.hebbian.weaken_association(id_cat, word_id)?;
                    }
                    Outcome::Neutral => {}
                }
            }
        }

        debug!(
            category = %category_name,
            accepted,
            "suggestion feedback recorded in Hebbian network"
        );

        Ok(())
    }
}

impl Default for LearningEngine {
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
    fn test_new_learning_engine() {
        let engine = LearningEngine::new();
        assert_eq!(engine.hebbian.concept_count(), 0);
        assert_eq!(engine.interests.interest_count(), 0);
        assert_eq!(engine.skills.skill_count(), 0);
    }

    #[test]
    fn test_observe_success() {
        let mut engine = LearningEngine::new();
        engine
            .observe("coffee", "morning", Outcome::Success, 1000)
            .expect("observe");

        assert_eq!(engine.hebbian.concept_count(), 2);
        assert_eq!(engine.hebbian.association_count(), 1);
    }

    #[test]
    fn test_observe_failure_weakens() {
        let mut engine = LearningEngine::new();
        // Build up association first
        for i in 0..5 {
            engine
                .observe("alarm", "snooze", Outcome::Success, 1000 + i)
                .expect("ok");
        }
        let alarm_id = engine
            .hebbian
            .get_or_create_concept("alarm", 0)
            .expect("id");
        let before = engine.hebbian.get_associated(alarm_id, 0.0);
        let weight_before = before[0].1;

        // Now observe a failure
        engine
            .observe("alarm", "snooze", Outcome::Failure, 2000)
            .expect("ok");

        let id_a = engine
            .hebbian
            .get_or_create_concept("alarm", 0)
            .expect("id");
        let after = engine.hebbian.get_associated(id_a, 0.0);
        let weight_after = after[0].1;

        assert!(
            weight_after < weight_before,
            "failure should weaken: {weight_before} → {weight_after}"
        );
    }

    #[test]
    fn test_consolidate_empty() {
        let mut engine = LearningEngine::new();
        let report = engine.consolidate(1000).expect("consolidate");
        assert_eq!(report.concepts_strengthened, 0);
        assert_eq!(report.concepts_pruned, 0);
    }

    #[test]
    fn test_consolidate_with_data() {
        let mut engine = LearningEngine::new();

        // Build up concepts with many activations and high importance
        for i in 0..60 {
            engine
                .observe("phone", "unlock", Outcome::Success, 100 + i)
                .expect("ok");
        }

        // Consolidate right after (recency = high)
        let report = engine.consolidate(200).expect("consolidate");

        // Both "phone" and "unlock" should be above the threshold
        assert!(
            report.concepts_strengthened >= 1,
            "expected at least 1 strengthened, got {}",
            report.concepts_strengthened
        );
    }

    #[test]
    fn test_consolidate_prunes_weak() {
        let mut engine = LearningEngine::new();

        // Create a very weak association
        engine
            .observe("rare_a", "rare_b", Outcome::Success, 100)
            .expect("ok");

        // Decay heavily (simulate long time passing)
        // Association weight starts at 0.05 (one strengthen)
        // After massive decay, it should drop below PRUNE_THRESHOLD
        engine.hebbian.decay_all(100_000_000_000, 1000); // massive time skip

        let report = engine.consolidate(100_000_000_000).expect("consolidate");
        assert!(
            report.concepts_pruned >= 1,
            "expected at least 1 pruned, got {}",
            report.concepts_pruned
        );
    }

    #[test]
    fn test_outcome_serde_roundtrip() {
        let outcomes = [Outcome::Success, Outcome::Failure, Outcome::Neutral];
        for o in &outcomes {
            let json = serde_json::to_string(o).expect("serialize");
            let back: Outcome = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*o, back);
        }
    }

    #[test]
    fn test_consolidation_report_serde() {
        let report = ConsolidationReport {
            concepts_strengthened: 5,
            concepts_pruned: 3,
            associations_updated: 100,
        };
        let json = serde_json::to_string(&report).expect("serialize");
        let back: ConsolidationReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.concepts_strengthened, 5);
        assert_eq!(back.concepts_pruned, 3);
        assert_eq!(back.associations_updated, 100);
    }
}
