//! WorldModel — AURA's unified representation of user state.
//!
//! # Architecture (Phase 1A — Nervous System Wiring)
//!
//! The WorldModel is the integration layer that sits *above* the seven
//! learning sub-engines and fuses their outputs into a single coherent
//! view of the user's current state, predicted next actions, and
//! confidence levels.
//!
//! # Why this exists
//!
//! Without the WorldModel, each sub-engine operates in isolation:
//! - [`PatternDetector`] knows temporal/sequential/contextual patterns
//! - [`PredictionEngine`] knows what might happen next
//! - [`DimensionDiscovery`] knows discovered behavioral axes
//! - [`HebbianNetwork`] knows concept associations
//! - [`InterestModel`] knows interest weights
//! - [`SkillRegistry`] knows learned capabilities
//! - [`DreamingEngine`] knows app topography
//!
//! The WorldModel reads from ALL of them and produces a unified snapshot
//! that downstream systems (routing, proactive engine, user profile,
//! ARC manager) can consume.
//!
//! # Active Inference (Concept Design §4.2)
//!
//! The WorldModel IS the internal model of Active Inference.  It holds:
//! - Current beliefs about user state (`UserStateSnapshot`)
//! - Predictions about what happens next (`predict_next`)
//! - Prediction error when reality deviates (`compute_prediction_error`)
//!
//! The prediction-error signal drives belief updating — AURA learns by
//! being surprised.
//!
//! # Design principle
//!
//! WorldModel does NOT replace sub-engines.  It reads their outputs and
//! fuses them.  Sub-engines remain specialized; WorldModel is the
//! integration layer — like the thalamus routing signals across brain regions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, instrument};

use super::dimensions::{Dimension, DimensionDiscovery};
use super::hebbian::HebbianNetwork;
use super::interests::InterestModel;
use super::patterns::{ContextKey, PatternDetector};
use super::prediction::{Prediction, PredictionEngine, SurpriseSignal};
use super::skills::SkillRegistry;
use super::super::DomainId;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of top interests to include in a snapshot.
const MAX_SNAPSHOT_INTERESTS: usize = 10;

/// Maximum number of predictions to include in a snapshot.
const MAX_SNAPSHOT_PREDICTIONS: usize = 5;

/// Maximum number of active dimensions to include in a snapshot.
const MAX_SNAPSHOT_DIMENSIONS: usize = 10;

/// Maximum number of top Hebbian concepts to include.
const MAX_SNAPSHOT_CONCEPTS: usize = 15;

/// Minimum strength for a dimension to be considered "active" in the snapshot.
const ACTIVE_DIMENSION_THRESHOLD: f32 = 0.3;

/// Confidence threshold below which a prediction is not included.
const PREDICTION_CONFIDENCE_FLOOR: f32 = 0.10;

// ---------------------------------------------------------------------------
// UserStateSnapshot — the fused output
// ---------------------------------------------------------------------------

/// A point-in-time unified view of everything AURA knows about the user.
///
/// This is the output of [`WorldModel::fuse`].  Downstream systems read this
/// instead of querying individual sub-engines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserStateSnapshot {
    /// Timestamp when this snapshot was generated.
    pub generated_at_ms: u64,

    // -- Predictions (what we expect to happen next) -------------------------

    /// Top predictions for the user's next action, with confidence.
    pub predictions: Vec<SnapshotPrediction>,

    /// Whether the prediction engine has enough data to be proactive.
    pub proactive_ready: bool,

    /// Current adaptive fusion weights [temporal, sequence, context].
    pub prediction_weights: [f32; 3],

    // -- Dimensions (who this user IS) ---------------------------------------

    /// Active discovered behavioral dimensions with strength.
    pub active_dimensions: Vec<SnapshotDimension>,

    // -- Interests (what this user CARES about) ------------------------------

    /// Top interests by current score.
    pub top_interests: Vec<SnapshotInterest>,

    /// Domain affinity map (which life domains matter most).
    pub domain_affinities: HashMap<String, f32>,

    // -- Skills (what AURA CAN DO for this user) -----------------------------

    /// Number of reliable skills AURA has learned.
    pub reliable_skill_count: usize,

    /// Total registered skills.
    pub total_skill_count: usize,

    // -- Concepts (what AURA KNOWS about this user's world) ------------------

    /// Top Hebbian concepts by activation recency and importance.
    pub top_concepts: Vec<SnapshotConcept>,

    /// Total concept count in the Hebbian network.
    pub total_concept_count: usize,

    // -- Meta ----------------------------------------------------------------

    /// Total observations processed by the learning engine.
    pub total_learning_observations: u64,

    /// Mean surprise level from recent predictions (0.0 = perfect,
    /// 1.0 = completely unexpected).  Indicates how well AURA
    /// understands the user.
    pub mean_recent_surprise: f32,

    /// Whether a routine change has been detected.
    pub routine_change_detected: bool,
}

/// A prediction entry in the snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotPrediction {
    /// Predicted action label (e.g., "open_spotify").
    pub action: String,
    /// Fused confidence [0.0, 1.0].
    pub confidence: f32,
    /// Which source contributed most (temporal/sequence/context).
    pub primary_source: String,
}

/// A discovered dimension in the snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotDimension {
    /// Human-readable label (e.g., "Deep Work Mornings").
    pub label: String,
    /// Strength/confidence [0.0, 1.0].
    pub strength: f32,
    /// Feature keys that define this dimension.
    pub features: Vec<String>,
    /// Number of confirming observations.
    pub observation_count: u32,
}

/// An interest entry in the snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInterest {
    /// Topic name.
    pub topic: String,
    /// Current score [0.0, 1.0].
    pub score: f32,
}

/// A Hebbian concept in the snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotConcept {
    /// Concept name.
    pub name: String,
    /// Importance score [0.0, 1.0].
    pub importance: f32,
    /// Number of activations.
    pub activation_count: u32,
}

// ---------------------------------------------------------------------------
// WorldModel
// ---------------------------------------------------------------------------

/// The unified world model that fuses all learning subsystem outputs.
///
/// # Usage
///
/// ```ignore
/// let mut world_model = WorldModel::new();
///
/// // After each observation cycle:
/// let snapshot = world_model.fuse(
///     &mut learning_engine.prediction,
///     &learning_engine.patterns,
///     &learning_engine.dimensions,
///     &learning_engine.interests,
///     &learning_engine.skills,
///     &learning_engine.hebbian,
///     minute_of_day,
///     day_of_week,
///     &recent_actions,
///     &current_context,
///     now_ms,
/// );
///
/// // Downstream: routing, proactive engine, user profile all read `snapshot`
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldModel {
    /// Most recent snapshot (cached).
    last_snapshot: Option<UserStateSnapshot>,

    /// Timestamp of last fusion.
    last_fuse_ms: u64,

    /// Rolling surprise average (EMA with α=0.1).
    surprise_ema: f32,

    /// Count of consecutive high-surprise observations.
    /// When this exceeds a threshold, it signals a routine change.
    high_surprise_streak: u32,

    /// Total fusions performed (for diagnostics).
    total_fusions: u64,
}

impl WorldModel {
    /// Create a new, empty world model.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_snapshot: None,
            last_fuse_ms: 0,
            surprise_ema: 0.5, // Start neutral
            high_surprise_streak: 0,
            total_fusions: 0,
        }
    }

    /// Fuse all learning subsystem outputs into a unified snapshot.
    ///
    /// This is the main entry point.  Call it periodically (e.g., on each
    /// observation) to keep the world model current.
    ///
    /// # Arguments
    ///
    /// * `prediction` — Mutable reference to generate predictions
    /// * `patterns` — Pattern detector for temporal/sequential/contextual data
    /// * `dimensions` — Dimension discovery for behavioral axes
    /// * `interests` — Interest model for topic weights
    /// * `skills` — Skill registry for capability assessment
    /// * `hebbian` — Hebbian network for concept associations
    /// * `minute_of_day` — Current minute of day (0.0–1439.0)
    /// * `day_of_week` — Current day (0=Mon, 6=Sun)
    /// * `recent_actions` — Recent user action labels for sequence prediction
    /// * `current_context` — Current environmental context keys
    /// * `now_ms` — Current timestamp in milliseconds
    #[instrument(skip_all, fields(minute_of_day, day_of_week))]
    #[allow(clippy::too_many_arguments)]
    pub fn fuse(
        &mut self,
        prediction: &mut PredictionEngine,
        patterns: &PatternDetector,
        dimensions: &DimensionDiscovery,
        interests: &InterestModel,
        skills: &SkillRegistry,
        hebbian: &HebbianNetwork,
        minute_of_day: f32,
        day_of_week: u8,
        recent_actions: &[&str],
        current_context: &[ContextKey],
        now_ms: u64,
    ) -> UserStateSnapshot {
        // 1. Generate predictions
        let raw_predictions = prediction.predict(
            patterns,
            minute_of_day,
            day_of_week,
            recent_actions,
            current_context,
        );

        let predictions: Vec<SnapshotPrediction> = raw_predictions
            .iter()
            .filter(|p| p.confidence >= PREDICTION_CONFIDENCE_FLOOR)
            .take(MAX_SNAPSHOT_PREDICTIONS)
            .map(|p| SnapshotPrediction {
                action: p.action.clone(),
                confidence: p.confidence,
                primary_source: p.sources.first()
                    .map(|s| format!("{:?}", s))
                    .unwrap_or_else(|| "unknown".to_string()),
            })
            .collect();

        let proactive_ready = prediction.is_proactive_ready();
        let prediction_weights = prediction.current_weights();

        // 2. Gather active dimensions
        let active_dimensions: Vec<SnapshotDimension> = dimensions
            .dimensions()
            .iter()
            .filter(|d| d.strength >= ACTIVE_DIMENSION_THRESHOLD)
            .take(MAX_SNAPSHOT_DIMENSIONS)
            .map(|d| SnapshotDimension {
                label: d.label.clone(),
                strength: d.strength,
                features: d.features.clone(),
                observation_count: d.observation_count,
            })
            .collect();

        // 3. Gather top interests
        let raw_interests = interests.get_top_interests(MAX_SNAPSHOT_INTERESTS);
        let top_interests: Vec<SnapshotInterest> = raw_interests
            .into_iter()
            .map(|(topic, score)| SnapshotInterest {
                topic: topic.to_string(),
                score,
            })
            .collect();

        // 4. Domain affinities
        let mut domain_affinities = HashMap::new();
        for domain in DomainId::all() {
            let affinity = interests.get_domain_affinity(domain);
            if (affinity - 1.0).abs() > f32::EPSILON {
                // Only include non-default affinities
                domain_affinities.insert(domain.to_string(), affinity);
            }
        }

        // 5. Skill summary
        let reliable_skills = skills.get_reliable_skills(0.7);
        let reliable_skill_count = reliable_skills.len();
        let total_skill_count = skills.skill_count();

        // 6. Top Hebbian concepts
        let concept_ids = hebbian.concept_ids();
        let mut concept_entries: Vec<SnapshotConcept> = concept_ids
            .iter()
            .filter_map(|&id| {
                hebbian.get_concept(id).map(|c| SnapshotConcept {
                    name: c.name.clone(),
                    importance: c.importance,
                    activation_count: c.total_activations,
                })
            })
            .collect();

        // Sort by importance descending, take top N
        concept_entries.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        concept_entries.truncate(MAX_SNAPSHOT_CONCEPTS);

        let total_concept_count = hebbian.concept_count();

        // 7. Meta — surprise and observation counts
        let total_learning_observations = dimensions.total_observations();

        let routine_change_detected = self.high_surprise_streak >= 3;

        let snapshot = UserStateSnapshot {
            generated_at_ms: now_ms,
            predictions,
            proactive_ready,
            prediction_weights,
            active_dimensions,
            top_interests,
            domain_affinities,
            reliable_skill_count,
            total_skill_count,
            top_concepts: concept_entries,
            total_concept_count,
            total_learning_observations,
            mean_recent_surprise: self.surprise_ema,
            routine_change_detected,
        };

        self.last_snapshot = Some(snapshot.clone());
        self.last_fuse_ms = now_ms;
        self.total_fusions += 1;

        debug!(
            predictions = snapshot.predictions.len(),
            dimensions = snapshot.active_dimensions.len(),
            interests = snapshot.top_interests.len(),
            skills = snapshot.reliable_skill_count,
            concepts = snapshot.top_concepts.len(),
            surprise = snapshot.mean_recent_surprise,
            "world model fused"
        );

        snapshot
    }

    /// Update the surprise EMA after computing prediction error.
    ///
    /// Call this after each [`PredictionEngine::compute_surprise`] to
    /// keep the world model's surprise estimate current.
    pub fn update_surprise(&mut self, surprise: &SurpriseSignal) {
        const EMA_ALPHA: f32 = 0.1;
        const HIGH_SURPRISE_THRESHOLD: f32 = 0.7;

        self.surprise_ema =
            EMA_ALPHA * surprise.surprise + (1.0 - EMA_ALPHA) * self.surprise_ema;

        if surprise.surprise >= HIGH_SURPRISE_THRESHOLD {
            self.high_surprise_streak += 1;
        } else {
            self.high_surprise_streak = 0;
        }

        debug!(
            surprise = surprise.surprise,
            ema = self.surprise_ema,
            streak = self.high_surprise_streak,
            "surprise EMA updated"
        );
    }

    /// Get the most recent snapshot, if available.
    #[must_use]
    pub fn last_snapshot(&self) -> Option<&UserStateSnapshot> {
        self.last_snapshot.as_ref()
    }

    /// Timestamp of the last fusion.
    #[must_use]
    pub fn last_fuse_ms(&self) -> u64 {
        self.last_fuse_ms
    }

    /// Mean surprise level (EMA).
    #[must_use]
    pub fn mean_surprise(&self) -> f32 {
        self.surprise_ema
    }

    /// Whether a routine change has been detected.
    ///
    /// True when 3+ consecutive observations had surprise ≥ 0.7,
    /// indicating the user's behaviour has shifted.
    #[must_use]
    pub fn routine_change_detected(&self) -> bool {
        self.high_surprise_streak >= 3
    }

    /// Total number of times fuse() has been called.
    #[must_use]
    pub fn total_fusions(&self) -> u64 {
        self.total_fusions
    }
}

impl Default for WorldModel {
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
    use super::super::{
        DimensionDiscovery, HebbianNetwork, InterestModel,
        PatternDetector, PredictionEngine, SkillRegistry,
    };

    fn make_engines() -> (
        PredictionEngine,
        PatternDetector,
        DimensionDiscovery,
        InterestModel,
        SkillRegistry,
        HebbianNetwork,
    ) {
        (
            PredictionEngine::new(),
            PatternDetector::new(),
            DimensionDiscovery::new(),
            InterestModel::new(),
            SkillRegistry::new(),
            HebbianNetwork::new(),
        )
    }

    #[test]
    fn test_new_world_model_is_empty() {
        let wm = WorldModel::new();
        assert!(wm.last_snapshot().is_none());
        assert_eq!(wm.total_fusions(), 0);
        assert!(!wm.routine_change_detected());
    }

    #[test]
    fn test_fuse_with_empty_engines() {
        let mut wm = WorldModel::new();
        let (mut pred, pat, dim, int, skills, heb) = make_engines();

        let snapshot = wm.fuse(
            &mut pred, &pat, &dim, &int, &skills, &heb,
            480.0, // 8:00 AM
            1,     // Tuesday
            &[],
            &[],
            1_000_000,
        );

        assert_eq!(snapshot.predictions.len(), 0);
        assert_eq!(snapshot.active_dimensions.len(), 0);
        assert_eq!(snapshot.top_interests.len(), 0);
        assert_eq!(snapshot.reliable_skill_count, 0);
        assert_eq!(snapshot.total_concept_count, 0);
        assert!(!snapshot.proactive_ready);

        assert_eq!(wm.total_fusions(), 1);
        assert!(wm.last_snapshot().is_some());
    }

    #[test]
    fn test_fuse_with_interests() {
        let mut wm = WorldModel::new();
        let (mut pred, pat, dim, mut int, skills, heb) = make_engines();

        // Add some interests
        int.observe_interest("programming", 0.9, 1000).unwrap();
        int.observe_interest("music", 0.7, 1000).unwrap();
        int.observe_interest("cooking", 0.3, 1000).unwrap();

        let snapshot = wm.fuse(
            &mut pred, &pat, &dim, &int, &skills, &heb,
            480.0, 1, &[], &[], 2000,
        );

        assert!(snapshot.top_interests.len() >= 2);
        // Highest interest should be first
        assert_eq!(snapshot.top_interests[0].topic, "programming");
    }

    #[test]
    fn test_fuse_with_concepts() {
        let mut wm = WorldModel::new();
        let (mut pred, pat, dim, int, skills, mut heb) = make_engines();

        // Create some Hebbian concepts
        let id_a = heb.get_or_create_concept("coffee", 1000).unwrap();
        let id_b = heb.get_or_create_concept("morning", 1000).unwrap();
        heb.activate(id_a, super::super::Outcome::Success, 0.9, 1000).unwrap();
        heb.activate(id_b, super::super::Outcome::Success, 0.5, 1000).unwrap();

        let snapshot = wm.fuse(
            &mut pred, &pat, &dim, &int, &skills, &heb,
            480.0, 1, &[], &[], 2000,
        );

        assert_eq!(snapshot.total_concept_count, 2);
        assert!(snapshot.top_concepts.len() >= 1);
    }

    #[test]
    fn test_surprise_ema_updates() {
        let mut wm = WorldModel::new();

        // Start at 0.5 (neutral)
        assert!((wm.mean_surprise() - 0.5).abs() < f32::EPSILON);

        // Low surprise signal
        let low_surprise = SurpriseSignal {
            observed_action: "check_email".to_string(),
            surprise: 0.1,
            prediction_rank: Some(0),
            prediction_count: 3,
            timestamp_ms: 1000,
        };
        wm.update_surprise(&low_surprise);

        // EMA should decrease toward 0.1
        assert!(wm.mean_surprise() < 0.5);
        assert!(!wm.routine_change_detected());
    }

    #[test]
    fn test_routine_change_detection() {
        let mut wm = WorldModel::new();

        let high_surprise = SurpriseSignal {
            observed_action: "unexpected_action".to_string(),
            surprise: 0.9,
            prediction_rank: None,
            prediction_count: 3,
            timestamp_ms: 1000,
        };

        // 1st high surprise
        wm.update_surprise(&high_surprise);
        assert!(!wm.routine_change_detected());

        // 2nd high surprise
        wm.update_surprise(&high_surprise);
        assert!(!wm.routine_change_detected());

        // 3rd high surprise — NOW routine change is detected
        wm.update_surprise(&high_surprise);
        assert!(wm.routine_change_detected());
    }

    #[test]
    fn test_routine_change_resets_on_low_surprise() {
        let mut wm = WorldModel::new();

        let high = SurpriseSignal {
            observed_action: "unexpected".to_string(),
            surprise: 0.9,
            prediction_rank: None,
            prediction_count: 3,
            timestamp_ms: 1000,
        };
        let low = SurpriseSignal {
            observed_action: "expected".to_string(),
            surprise: 0.2,
            prediction_rank: Some(0),
            prediction_count: 3,
            timestamp_ms: 2000,
        };

        wm.update_surprise(&high);
        wm.update_surprise(&high);
        // 2 high — not yet routine change
        assert!(!wm.routine_change_detected());

        // Low surprise breaks the streak
        wm.update_surprise(&low);
        assert!(!wm.routine_change_detected());
    }

    #[test]
    fn test_snapshot_serde_roundtrip() {
        let mut wm = WorldModel::new();
        let (mut pred, pat, dim, int, skills, heb) = make_engines();

        let snapshot = wm.fuse(
            &mut pred, &pat, &dim, &int, &skills, &heb,
            480.0, 1, &[], &[], 1000,
        );

        let json = serde_json::to_string(&snapshot).expect("serialize");
        let back: UserStateSnapshot =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.generated_at_ms, 1000);
    }

    #[test]
    fn test_world_model_serde_roundtrip() {
        let wm = WorldModel::new();
        let json = serde_json::to_string(&wm).expect("serialize");
        let back: WorldModel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.total_fusions(), 0);
    }
}
