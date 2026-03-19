//! Importance scoring engine — the Amygdala of memory.
//!
//! This module computes and updates the importance score for memories using
//! the v4 formula:
//!   importance = source_weight * recency_decay * access_bonus * domain_priority
//!
//! Source weights, recency decay constants, and domain priorities are all from
//! the AURA-V4-ENGINEERING-BLUEPRINT.md §4.1.2.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Event source for importance weighting (memory-specific, distinct from pipeline
/// EventSource to allow finer-grained weights).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventSource {
    UserExplicit,
    Conversation,
    Notification,
    SystemEvent,
    Cron,
}

impl EventSource {
    /// Base weight per source type.
    pub fn weight(self) -> f32 {
        match self {
            Self::UserExplicit => 1.0,
            Self::Conversation => 0.8,
            Self::Notification => 0.5,
            Self::SystemEvent => 0.3,
            Self::Cron => 0.2,
        }
    }
}

/// Map from pipeline EventSource to memory EventSource.
impl From<aura_types::events::EventSource> for EventSource {
    fn from(src: aura_types::events::EventSource) -> Self {
        match src {
            aura_types::events::EventSource::UserCommand => Self::UserExplicit,
            aura_types::events::EventSource::Notification => Self::Notification,
            aura_types::events::EventSource::Accessibility => Self::SystemEvent,
            aura_types::events::EventSource::Cron => Self::Cron,
            aura_types::events::EventSource::Internal => Self::SystemEvent,
        }
    }
}

/// Events that adjust an existing importance score.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImportanceEvent {
    /// User explicitly referenced this memory.
    UserReferenced,
    /// User corrected or contradicted this memory.
    UserCorrected,
    /// Passage of time.
    TimePassed { hours: f64 },
    /// A related memory was strengthened.
    RelatedStrengthened,
}

// ---------------------------------------------------------------------------
// Hebbian memory trace
// ---------------------------------------------------------------------------

/// Forgetting rate constants for Hebbian traces (fraction of strength lost per hour).
///
/// Episodic memories (tied to specific events) forget faster than semantic ones
/// (general knowledge). Values chosen so episodic half-life ≈ 4 days,
/// semantic half-life ≈ 35 days.
const HEBBIAN_EPISODIC_RATE: f64 = 0.007_200; // h⁻¹  ≈ 1/139 h
const HEBBIAN_SEMANTIC_RATE: f64 = 0.000_825; // h⁻¹  ≈ 1/1212 h

/// A Hebbian connection-strength trace attached to an individual memory.
///
/// Each time a memory is retrieved, its trace is *strengthened* (fire-together
/// wire-together). During idle time the trace decays exponentially, modelling
/// biological forgetting.
///
/// `effective_strength` applies time-elapsed decay lazily — the stored
/// `strength` is the *peak* after the last retrieval; actual current value
/// must always be computed via `effective_strength(hours_since_last_retrieval)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HebbianTrace {
    /// Connection strength in [0.0, 1.0]. 1.0 = maximally consolidated.
    pub strength: f32,
    /// Whether this trace belongs to episodic or semantic memory.
    /// Episodic decays faster; semantic decays slower.
    pub is_episodic: bool,
}

impl HebbianTrace {
    /// Create a brand-new trace (day-zero safe — no prior retrieval required).
    pub fn new(is_episodic: bool) -> Self {
        Self {
            strength: 0.5,
            is_episodic,
        }
    }

    /// Strengthen the trace on retrieval (Hebbian LTP analogue).
    ///
    /// Formula: `min(strength * 1.3 + 0.05, 1.0)`
    /// — a proportional boost plus a small absolute floor so even weak traces
    /// recover somewhat from retrieval.
    pub fn on_retrieved(&mut self) {
        self.strength = (self.strength * 1.3 + 0.05).min(1.0);
    }

    /// Return the *current* effective strength, accounting for exponential
    /// forgetting since the last retrieval.
    ///
    /// `hours_elapsed` — wall-clock hours since the last `on_retrieved` call
    /// (or since creation for a brand-new trace).
    ///
    /// Returns a value in (0.0, 1.0]. Never zero — even ancient memories
    /// leave a faint biological trace.
    pub fn effective_strength(&self, hours_elapsed: f64) -> f32 {
        let rate = if self.is_episodic {
            HEBBIAN_EPISODIC_RATE
        } else {
            HEBBIAN_SEMANTIC_RATE
        };
        let decayed = self.strength as f64 * (-rate * hours_elapsed).exp();
        decayed.clamp(1e-6, 1.0) as f32
    }
}

// ---------------------------------------------------------------------------
// Core scoring
// ---------------------------------------------------------------------------

/// Calculate base importance for a NEW memory.
///
/// Formula: `source_weight * recency_decay * access_bonus * domain_priority`
///
/// `domain_priority` should be 1.0 by default. The LLM layer is responsible for
/// determining domain-based priority and may pass a non-1.0 multiplier if it has
/// classified the memory domain. Rust must not attempt to classify domain from text.
///
/// For new memories, `hours_old` is typically 0 and `access_count` is 0,
/// so the effective formula simplifies to `source_weight * 1.0 * 1.0 * domain_priority`.
pub fn calculate_importance(
    source: EventSource,
    hours_old: f64,
    access_count: u32,
    domain_priority: f32,
) -> f32 {
    let source_weight = source.weight();
    let recency = recency_decay(hours_old);
    let access = access_bonus(access_count);

    let raw = source_weight * recency * access * domain_priority;
    raw.clamp(0.0, 2.0)
}

/// Update an existing importance value based on an event.
///
/// Returns the new importance, clamped to [0.0, 2.0].
pub fn update_importance(current: f32, event: ImportanceEvent) -> f32 {
    let adjusted = match event {
        ImportanceEvent::UserReferenced => current + 0.1,
        ImportanceEvent::UserCorrected => current - 0.2,
        ImportanceEvent::TimePassed { hours } => {
            // Apply recency decay multiplicatively
            current * recency_decay(hours)
        },
        ImportanceEvent::RelatedStrengthened => current + 0.05,
    };
    adjusted.clamp(0.0, 2.0)
}

/// Recency decay function: e^(-0.001 * hours).
///
/// Very slow decay — memories stay relevant for weeks.
/// Half-life: ln(2) / 0.001 = ~693 hours ≈ 29 days.
#[inline]
pub fn recency_decay(hours: f64) -> f32 {
    (-0.001 * hours).exp() as f32
}

/// Access bonus: min(2.0, 1.0 + 0.1 * access_count).
///
/// More accessed = more important, capped at 2x.
#[inline]
pub fn access_bonus(access_count: u32) -> f32 {
    (1.0 + 0.1 * access_count as f32).min(2.0)
}

/// Compute the recall score for ranking retrieval results.
///
/// score = similarity*0.25 + recency*0.20 + activation*0.20 + emotional_valence*0.15 + goal_relevance*0.10 + novelty_score*0.10
///
/// Where:
/// - similarity: cosine similarity between query and memory embedding [0, 1]
/// - recency: exp(-0.1 * hours_ago) — ~7 hour half-life
/// - activation: access_bonus / 2.0 (normalized to [0, 1])
/// - emotional_valence: emotional significance from episodic memory [0, 1]
/// - goal_relevance: how related this memory is to current goals [0, 1]
/// - novelty_score: how different this memory is from existing memories [0, 1]
pub fn recall_score(
    similarity: f32,
    hours_ago: f64,
    access_count: u32,
    emotional_valence: f32,
    goal_relevance: f32,
    novelty_score: f32,
) -> f32 {
    let recency = (-0.1 * hours_ago).exp() as f32;
    let activation = (access_bonus(access_count) / 2.0).clamp(0.0, 1.0);
    let norm_valence = emotional_valence.clamp(0.0, 1.0);
    let norm_goal = goal_relevance.clamp(0.0, 1.0);
    let norm_novelty = novelty_score.clamp(0.0, 1.0);

    similarity * 0.25 + recency * 0.20 + activation * 0.20 + norm_valence * 0.15 + norm_goal * 0.10 + norm_novelty * 0.10
}

/// Consolidation score for deciding what to promote between tiers.
///
/// score = recency*0.3 + frequency*0.3 + base_importance*0.4
///
/// Threshold for promotion is typically 0.7.
pub fn consolidation_score(hours_ago: f64, access_count: u32, base_importance: f32) -> f32 {
    let recency = (-0.1 * hours_ago).exp() as f32;
    let frequency = (access_count as f32 / 10.0).min(1.0); // normalize: 10 accesses = 1.0
    let norm_importance = (base_importance / 2.0).clamp(0.0, 1.0);

    recency * 0.3 + frequency * 0.3 + norm_importance * 0.4
}

/// Initial confidence for a new semantic entry.
///
/// confidence = min(0.9, 0.3 + importance * 0.4)
pub fn initial_semantic_confidence(importance: f32) -> f32 {
    (0.3 + importance * 0.4).min(0.9)
}

/// Generalization confidence for semantic entries derived from episode clusters.
///
/// confidence = min(0.95, 0.5 + num_episodes * 0.1)
pub fn generalization_confidence(num_episodes: usize) -> f32 {
    (0.5 + num_episodes as f32 * 0.1).min(0.95)
}

// ---------------------------------------------------------------------------
// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_weights() {
        assert_eq!(EventSource::UserExplicit.weight(), 1.0);
        assert_eq!(EventSource::Conversation.weight(), 0.8);
        assert_eq!(EventSource::Notification.weight(), 0.5);
        assert_eq!(EventSource::SystemEvent.weight(), 0.3);
        assert_eq!(EventSource::Cron.weight(), 0.2);
    }

    #[test]
    fn test_calculate_importance_new_memory() {
        // New memory: hours_old=0, access_count=0, neutral domain_priority=1.0
        let imp = calculate_importance(EventSource::UserExplicit, 0.0, 0, 1.0);
        // 1.0 * exp(0) * min(2.0, 1.0 + 0) * 1.0 = 1.0
        assert!((imp - 1.0).abs() < 1e-5, "expected 1.0, got {}", imp);
    }

    #[test]
    fn test_calculate_importance_with_domain_priority() {
        // LLM-provided domain_priority of 1.2 (e.g., health-classified by LLM)
        let imp = calculate_importance(EventSource::UserExplicit, 0.0, 0, 1.2);
        assert!((imp - 1.2).abs() < 1e-5, "expected 1.2, got {}", imp);
    }

    #[test]
    fn test_calculate_importance_old_memory() {
        // 720 hours old (30 days), 5 accesses, neutral domain_priority=1.0
        let imp = calculate_importance(EventSource::Conversation, 720.0, 5, 1.0);
        let expected = 0.8 * (-0.001 * 720.0_f64).exp() as f32 * (1.0 + 0.5) * 1.0;
        assert!(
            (imp - expected).abs() < 1e-4,
            "expected {}, got {}",
            expected,
            imp
        );
    }

    #[test]
    fn test_recency_decay() {
        assert!((recency_decay(0.0) - 1.0).abs() < f32::EPSILON);
        // At 693 hours (half-life), should be ~0.5
        let half = recency_decay(693.0);
        assert!(
            (half - 0.5).abs() < 0.01,
            "half-life decay should be ~0.5, got {}",
            half
        );
        // Very old should approach 0
        let old = recency_decay(10000.0);
        assert!(old < 0.001);
    }

    #[test]
    fn test_access_bonus() {
        assert_eq!(access_bonus(0), 1.0);
        assert_eq!(access_bonus(5), 1.5);
        assert_eq!(access_bonus(10), 2.0);
        assert_eq!(access_bonus(100), 2.0); // capped
    }

    #[test]
    fn test_recall_score() {
        // Perfect match, just created, high valence, many accesses, high goal/novelty
        let score = recall_score(1.0, 0.0, 10, 1.0, 1.0, 1.0);
        // 1.0*0.25 + 1.0*0.20 + 1.0*0.20 + 1.0*0.15 + 1.0*0.10 + 1.0*0.10 = 1.0
        assert!(
            (score - 1.0).abs() < 1e-5,
            "max recall score should be 1.0, got {}",
            score
        );
    }

    #[test]
    fn test_recall_score_old_low_importance() {
        let score = recall_score(0.5, 24.0, 1, 0.3, 0.5, 0.5);
        // similarity=0.5*0.25=0.125, recency≈0.09*0.20=0.018, activation≈0.55*0.20=0.11,
        // valence=0.3*0.15=0.045, goal=0.5*0.10=0.05, novelty=0.5*0.10=0.05
        assert!(score > 0.0 && score < 1.0);
    }

    #[test]
    fn test_consolidation_score() {
        // Fresh, frequently accessed, important
        let score = consolidation_score(0.0, 10, 2.0);
        // 1.0*0.3 + 1.0*0.3 + 1.0*0.4 = 1.0
        assert!((score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_consolidation_score_below_threshold() {
        // Old, rarely accessed, low importance
        let score = consolidation_score(100.0, 0, 0.2);
        assert!(
            score < 0.7,
            "should be below promotion threshold, got {}",
            score
        );
    }

    #[test]
    fn test_update_importance_user_referenced() {
        let new = update_importance(0.5, ImportanceEvent::UserReferenced);
        assert!((new - 0.6).abs() < f32::EPSILON);
    }

    #[test]
    fn test_update_importance_user_corrected() {
        let new = update_importance(0.5, ImportanceEvent::UserCorrected);
        assert!((new - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_update_importance_clamp() {
        // Should not go below 0
        let new = update_importance(0.1, ImportanceEvent::UserCorrected);
        assert_eq!(new, 0.0);
        // Should not go above 2.0
        let new = update_importance(1.95, ImportanceEvent::UserReferenced);
        assert_eq!(new, 2.0);
    }

    #[test]
    fn test_initial_semantic_confidence() {
        assert!((initial_semantic_confidence(0.0) - 0.3).abs() < f32::EPSILON);
        assert!((initial_semantic_confidence(1.0) - 0.7).abs() < f32::EPSILON);
        assert!((initial_semantic_confidence(2.0) - 0.9).abs() < f32::EPSILON); // capped
        assert!((initial_semantic_confidence(5.0) - 0.9).abs() < f32::EPSILON); // capped
    }

    #[test]
    fn test_generalization_confidence() {
        assert!((generalization_confidence(3) - 0.8).abs() < f32::EPSILON);
        assert!((generalization_confidence(5) - 0.95).abs() < f32::EPSILON); // capped
        assert!((generalization_confidence(10) - 0.95).abs() < f32::EPSILON); // capped
    }

    #[test]
    fn test_pipeline_event_source_conversion() {
        let src = aura_types::events::EventSource::UserCommand;
        let mem_src: EventSource = src.into();
        assert_eq!(mem_src, EventSource::UserExplicit);
    }
}
