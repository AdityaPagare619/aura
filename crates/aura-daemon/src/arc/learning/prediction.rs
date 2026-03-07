//! Active Inference prediction engine — generates expectations, computes
//! surprise, and drives belief updating.
//!
//! # Architecture (Concept Design §4.2 — Active Inference Framework)
//!
//! AURA minimises "free energy" — the gap between predicted and observed
//! user behaviour.  When predictions fail, AURA either:
//!
//! 1. **Updates its model** (perceptual inference — learning)
//! 2. **Acts to change the world** (active inference — proactive suggestions)
//!
//! This module sits *above* the pattern infrastructure (`patterns.rs`,
//! `hebbian.rs`, `routines.rs`) and performs three functions:
//!
//! - **Prediction**: Fuse temporal, sequential, and contextual patterns into
//!   a ranked list of expected actions for the current moment.
//! - **Surprise**: When an observation arrives, compute prediction error (the
//!   Kullback–Leibler divergence between expected and observed distributions).
//! - **Model update**: Feed surprise signals back to pattern confidence,
//!   Hebbian weights, and personality evolution.
//!
//! # Day-1 vs Year-1 Behaviour
//!
//! - **Day 1**: Zero patterns → zero predictions → zero surprise.  Engine is
//!   passive; all observations are novel and generate no prediction error.
//! - **Week 1**: ~5–10 temporal patterns discovered.  Predictions begin at low
//!   confidence (0.15–0.30).  Surprise is frequent (new behaviour).
//! - **Month 1**: 30–50 patterns with moderate confidence.  Predictions for
//!   morning/evening routines become reliable.  Surprise mostly comes from
//!   schedule changes.
//! - **Year 1**: 100+ stable patterns, sequence chains of 3–5 steps.  AURA
//!   predicts routine transitions (work→lunch→walk) and only fires surprise
//!   for genuine anomalies (missed habit, unusual app, schedule break).
//!
//! # Complexity
//!
//! - Prediction: O(P) where P = total actionable patterns (bounded by MAX_*
//!   constants in `patterns.rs`)
//! - Surprise: O(K) where K = number of predictions for the current slot
//! - Memory: O(SURPRISE_HISTORY_SIZE) for rolling surprise window
//!
//! # Side-Effect Analysis
//!
//! - **False confidence**: MIN_PREDICTIONS_FOR_PROACTIVE prevents premature
//!   action.  AURA needs ≥3 predictions with combined confidence > 0.6 before
//!   acting proactively.
//! - **Confirmation bias**: `record_miss()` actively decays pattern confidence
//!   when predictions fail, preventing model ossification.
//! - **Catastrophic forgetting**: `detect_routine_change()` uses a 7-day rolling
//!   window of surprise scores.  A sustained spike (mean surprise >
//!   ROUTINE_CHANGE_THRESHOLD for 3+ days) triggers a routine-change signal,
//!   which downstream consumers can use to accelerate pattern relearning.
//! - **Battery drain**: Predictions are computed lazily — only when the proactive
//!   engine queries, not on every observation.

use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use super::patterns::{
    ContextKey, PatternDetector, MIN_CONFIDENCE, MIN_OBSERVATIONS,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of top predictions to surface per query.
const MAX_PREDICTIONS: usize = 10;

/// Minimum combined confidence before a prediction is considered
/// proactively actionable (prevents premature action).
const PROACTIVE_CONFIDENCE_THRESHOLD: f32 = 0.6;

/// Minimum number of positive predictions before proactive action.
const MIN_PREDICTIONS_FOR_PROACTIVE: usize = 3;

/// Size of the rolling surprise history (one entry per "slot" — typically
/// each observation produces one surprise value).
const SURPRISE_HISTORY_SIZE: usize = 512;

/// Rolling window (in entries) for routine-change detection.
/// At ~20 observations/day, 140 entries ≈ 7 days.
const ROUTINE_CHANGE_WINDOW: usize = 140;

/// Mean surprise above this threshold (sustained over ROUTINE_CHANGE_WINDOW)
/// indicates the user's routine has fundamentally changed.
const ROUTINE_CHANGE_THRESHOLD: f32 = 0.7;

/// Weight for temporal predictions in the fusion.
const TEMPORAL_WEIGHT: f32 = 0.45;

/// Weight for sequential predictions in the fusion.
const SEQUENCE_WEIGHT: f32 = 0.35;

/// Weight for contextual predictions in the fusion.
const CONTEXT_WEIGHT: f32 = 0.20;

// ---------------------------------------------------------------------------
// Prediction
// ---------------------------------------------------------------------------

/// A single prediction about what the user might do next.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prediction {
    /// The predicted action (e.g. "open_whatsapp", "check_email").
    pub action: String,
    /// Fused confidence from all contributing sources [0.0, 1.0].
    pub confidence: f32,
    /// Which prediction sources contributed (for explainability).
    pub sources: Vec<PredictionSource>,
}

/// Identifies which pattern type contributed to a prediction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PredictionSource {
    /// Temporal pattern: this action occurs at this time of day.
    Temporal {
        pattern_id: u64,
        confidence: f32,
    },
    /// Sequence pattern: this action follows the recent action chain.
    Sequence {
        pattern_id: u64,
        confidence: f32,
    },
    /// Context pattern: this action correlates with current context.
    Context {
        pattern_id: u64,
        confidence: f32,
    },
}

// ---------------------------------------------------------------------------
// Surprise (prediction error)
// ---------------------------------------------------------------------------

/// The result of comparing a prediction set against an actual observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurpriseSignal {
    /// The observed action.
    pub observed_action: String,
    /// Surprise score [0.0, 1.0].
    /// - 0.0 = perfectly predicted (highest confidence prediction matched)
    /// - 1.0 = completely unexpected (no prediction matched)
    pub surprise: f32,
    /// Rank of the observed action in the prediction list (0-indexed).
    /// `None` if the action was not predicted at all.
    pub prediction_rank: Option<usize>,
    /// Number of active predictions at the time of observation.
    pub prediction_count: usize,
    /// Timestamp (ms) of the observation.
    pub timestamp_ms: u64,
}

// ---------------------------------------------------------------------------
// PredictionEngine
// ---------------------------------------------------------------------------

/// The Active Inference prediction engine.
///
/// Sits above [`PatternDetector`] and fuses temporal, sequential, and
/// contextual predictions into a unified expectation model.  Computes
/// surprise (prediction error) when observations arrive, and maintains
/// a rolling surprise history for routine-change detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionEngine {
    /// Rolling history of surprise values for trend analysis.
    surprise_history: Vec<f32>,
    /// Total predictions generated (lifetime counter).
    total_predictions: u64,
    /// Total observations processed (lifetime counter).
    total_observations: u64,
    /// Total surprise accumulated (for running mean).
    total_surprise: f64,
    /// Whether a routine change has been detected recently.
    routine_change_detected: bool,
    /// Timestamp of last routine change detection.
    last_routine_change_ms: u64,
    /// Cache of last prediction set (for surprise computation).
    #[serde(skip)]
    last_predictions: Vec<Prediction>,
}

impl PredictionEngine {
    /// Create a new prediction engine with empty state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            surprise_history: Vec::with_capacity(SURPRISE_HISTORY_SIZE),
            total_predictions: 0,
            total_observations: 0,
            total_surprise: 0.0,
            routine_change_detected: false,
            last_routine_change_ms: 0,
            last_predictions: Vec::new(),
        }
    }

    /// Generate predictions for the current moment.
    ///
    /// Fuses three prediction sources via weighted Reciprocal Rank Fusion:
    ///
    /// ```text
    /// score(action) = Σ (weight_source × confidence × 1/(rank + k))
    /// ```
    ///
    /// where `k=60` is a smoothing constant (standard RRF) that prevents
    /// top-ranked items from dominating excessively.
    ///
    /// # Arguments
    /// * `detector` — The pattern detector with all learned patterns
    /// * `minute_of_day` — Current minute (0–1439) for temporal matching
    /// * `day_of_week` — Current day (0=Mon, 6=Sun) for temporal matching
    /// * `recent_actions` — Recent action history for sequence prediction
    /// * `current_context` — Active context keys for context prediction
    #[instrument(skip_all, fields(minute = minute_of_day, day = day_of_week))]
    pub fn predict(
        &mut self,
        detector: &PatternDetector,
        minute_of_day: f32,
        day_of_week: u8,
        recent_actions: &[&str],
        current_context: &[ContextKey],
    ) -> Vec<Prediction> {
        use std::collections::HashMap;

        // --- 1. Gather predictions from each source ---

        // Temporal: Which actions are expected at this time of day?
        let temporal_preds: Vec<(String, f32, u64)> = detector
            .actionable_temporal_patterns()
            .into_iter()
            .filter(|p| p.matches_time(minute_of_day, day_of_week))
            .map(|p| (p.action.clone(), p.confidence, p.id))
            .collect();

        // Sequential: What action typically follows the recent chain?
        let sequence_preds: Vec<(String, f32, u64)> = if recent_actions.is_empty() {
            Vec::new()
        } else {
            // Use PatternDetector's predict_next_action, but we need IDs too
            // So we'll query actionable sequence patterns directly
            let mut seq_results = Vec::new();
            for pattern in detector.actionable_sequence_patterns() {
                let pat_len = pattern.actions.len();
                let recent_len = recent_actions.len();
                if recent_len >= pat_len {
                    continue;
                }
                let matches = recent_actions
                    .iter()
                    .zip(pattern.actions.iter())
                    .all(|(a, b)| *a == b.as_str());
                if matches {
                    seq_results.push((
                        pattern.actions[recent_len].clone(),
                        pattern.confidence,
                        pattern.id,
                    ));
                }
            }
            seq_results
        };

        // Contextual: What actions correlate with current context?
        let context_preds: Vec<(String, f32, u64)> = if current_context.is_empty() {
            Vec::new()
        } else {
            let mut ctx_results = Vec::new();
            for pattern in detector.actionable_context_patterns() {
                if current_context.contains(&pattern.context) {
                    ctx_results.push((
                        pattern.action.clone(),
                        pattern.confidence,
                        pattern.id,
                    ));
                }
            }
            ctx_results
        };

        // --- 2. Fuse via weighted RRF ---
        // Sort each source by confidence descending, then apply RRF
        let k = 60.0_f32; // Standard RRF smoothing constant

        let mut scores: HashMap<String, (f32, Vec<PredictionSource>)> = HashMap::new();

        // Helper: add ranked predictions from one source
        let add_source = |preds: &[(String, f32, u64)],
                          weight: f32,
                          scores: &mut HashMap<String, (f32, Vec<PredictionSource>)>,
                          make_source: &dyn Fn(u64, f32) -> PredictionSource| {
            // Already sorted by confidence from the filter
            let mut sorted = preds.to_vec();
            sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            for (rank, (action, conf, id)) in sorted.iter().enumerate() {
                let rrf_score = weight * conf * (1.0 / (rank as f32 + k));
                let entry = scores.entry(action.clone()).or_insert_with(|| {
                    (0.0, Vec::new())
                });
                entry.0 += rrf_score;
                entry.1.push(make_source(*id, *conf));
            }
        };

        add_source(&temporal_preds, TEMPORAL_WEIGHT, &mut scores, &|id, conf| {
            PredictionSource::Temporal {
                pattern_id: id,
                confidence: conf,
            }
        });

        add_source(&sequence_preds, SEQUENCE_WEIGHT, &mut scores, &|id, conf| {
            PredictionSource::Sequence {
                pattern_id: id,
                confidence: conf,
            }
        });

        add_source(&context_preds, CONTEXT_WEIGHT, &mut scores, &|id, conf| {
            PredictionSource::Context {
                pattern_id: id,
                confidence: conf,
            }
        });

        // --- 3. Build sorted prediction list ---
        let mut predictions: Vec<Prediction> = scores
            .into_iter()
            .map(|(action, (score, sources))| Prediction {
                action,
                // Normalize score to [0, 1] — max possible score is
                // (TEMPORAL_WEIGHT + SEQUENCE_WEIGHT + CONTEXT_WEIGHT) / k ≈ 0.0167
                // So multiply by k to get back to a meaningful confidence range.
                confidence: (score * k).clamp(0.0, 1.0),
                sources,
            })
            .collect();

        predictions.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        predictions.truncate(MAX_PREDICTIONS);

        self.total_predictions += predictions.len() as u64;

        debug!(
            prediction_count = predictions.len(),
            temporal = temporal_preds.len(),
            sequence = sequence_preds.len(),
            context = context_preds.len(),
            "predictions generated"
        );

        // Cache for surprise computation
        self.last_predictions = predictions.clone();

        predictions
    }

    /// Compute surprise (prediction error) for an observed action.
    ///
    /// Surprise is defined as:
    /// ```text
    /// surprise = 1.0 - confidence_of_observed_action
    /// ```
    ///
    /// If the observed action was not in the prediction set at all,
    /// surprise = 1.0 (maximum).  If it was the top prediction with
    /// confidence 0.9, surprise = 0.1 (minimal).
    ///
    /// This maps directly to the Active Inference "prediction error" signal
    /// that drives model updating (Concept Design §4.2).
    #[instrument(skip_all, fields(action = %observed_action))]
    pub fn compute_surprise(
        &mut self,
        observed_action: &str,
        now_ms: u64,
    ) -> SurpriseSignal {
        self.total_observations += 1;

        let prediction_count = self.last_predictions.len();

        // Find if this action was predicted
        let found = self
            .last_predictions
            .iter()
            .enumerate()
            .find(|(_, p)| p.action == observed_action);

        let (surprise, prediction_rank) = match found {
            Some((rank, pred)) => {
                // Action was predicted — surprise inversely proportional
                // to its confidence
                let s = 1.0 - pred.confidence;
                (s, Some(rank))
            }
            None => {
                if prediction_count == 0 {
                    // No predictions existed — this is "novel", not "surprising"
                    // Novel events have moderate surprise (0.5) rather than max
                    // because AURA shouldn't overreact during its learning phase
                    (0.5, None)
                } else {
                    // Predictions existed but none matched — maximum surprise
                    (1.0, None)
                }
            }
        };

        // Record in rolling history
        self.surprise_history.push(surprise);
        if self.surprise_history.len() > SURPRISE_HISTORY_SIZE {
            self.surprise_history.remove(0);
        }
        self.total_surprise += surprise as f64;

        // Check for routine change
        self.detect_routine_change(now_ms);

        let signal = SurpriseSignal {
            observed_action: observed_action.to_owned(),
            surprise,
            prediction_rank,
            prediction_count,
            timestamp_ms: now_ms,
        };

        debug!(
            surprise = surprise,
            rank = ?prediction_rank,
            prediction_count,
            "surprise computed"
        );

        signal
    }

    /// Detect if the user's routine has fundamentally changed.
    ///
    /// Uses a rolling window of surprise values.  If the mean surprise
    /// over the window exceeds ROUTINE_CHANGE_THRESHOLD, it means the
    /// user's behaviour has diverged from learned patterns persistently,
    /// not just transiently.
    ///
    /// This is the "spike detection" that prevents catastrophic forgetting:
    /// instead of slowly degrading old patterns, AURA recognises the shift
    /// and can accelerate relearning.
    fn detect_routine_change(&mut self, now_ms: u64) {
        let window = if self.surprise_history.len() >= ROUTINE_CHANGE_WINDOW {
            &self.surprise_history[self.surprise_history.len() - ROUTINE_CHANGE_WINDOW..]
        } else {
            return; // Not enough data yet
        };

        let mean_surprise: f32 = window.iter().sum::<f32>() / window.len() as f32;

        if mean_surprise > ROUTINE_CHANGE_THRESHOLD {
            if !self.routine_change_detected {
                info!(
                    mean_surprise,
                    window_size = ROUTINE_CHANGE_WINDOW,
                    "routine change detected — user behaviour has diverged from model"
                );
                self.routine_change_detected = true;
                self.last_routine_change_ms = now_ms;
            }
        } else if self.routine_change_detected {
            // Surprise has subsided — model has likely adapted
            info!(
                mean_surprise,
                "routine change resolved — model has adapted"
            );
            self.routine_change_detected = false;
        }
    }

    // -- Accessors -----------------------------------------------------------

    /// Whether a routine change is currently detected.
    #[must_use]
    pub fn is_routine_change_detected(&self) -> bool {
        self.routine_change_detected
    }

    /// Mean surprise over the lifetime of the engine.
    #[must_use]
    pub fn mean_surprise(&self) -> f32 {
        if self.total_observations == 0 {
            return 0.0;
        }
        (self.total_surprise / self.total_observations as f64) as f32
    }

    /// Mean surprise over the recent window (last ROUTINE_CHANGE_WINDOW entries).
    #[must_use]
    pub fn recent_mean_surprise(&self) -> f32 {
        if self.surprise_history.is_empty() {
            return 0.0;
        }
        let window_size = ROUTINE_CHANGE_WINDOW.min(self.surprise_history.len());
        let start = self.surprise_history.len() - window_size;
        let sum: f32 = self.surprise_history[start..].iter().sum();
        sum / window_size as f32
    }

    /// Number of predictions generated (lifetime).
    #[must_use]
    pub fn total_predictions(&self) -> u64 {
        self.total_predictions
    }

    /// Number of observations processed (lifetime).
    #[must_use]
    pub fn total_observations(&self) -> u64 {
        self.total_observations
    }

    /// The cached prediction set from the last call to `predict()`.
    #[must_use]
    pub fn last_predictions(&self) -> &[Prediction] {
        &self.last_predictions
    }

    /// Whether the prediction engine has enough data to make proactive
    /// suggestions.
    ///
    /// Requires at least `MIN_PREDICTIONS_FOR_PROACTIVE` predictions
    /// with combined confidence ≥ `PROACTIVE_CONFIDENCE_THRESHOLD`.
    #[must_use]
    pub fn is_proactive_ready(&self) -> bool {
        if self.last_predictions.len() < MIN_PREDICTIONS_FOR_PROACTIVE {
            return false;
        }
        let combined: f32 = self
            .last_predictions
            .iter()
            .take(MIN_PREDICTIONS_FOR_PROACTIVE)
            .map(|p| p.confidence)
            .sum();
        combined >= PROACTIVE_CONFIDENCE_THRESHOLD
    }
}

impl Default for PredictionEngine {
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
    use crate::arc::learning::patterns::{
        ContextKey, Observation, PatternDetector,
    };

    fn setup_detector_with_temporal_patterns() -> PatternDetector {
        let mut detector = PatternDetector::new();
        // Simulate "check_email" at 9am (540 min) on Monday through Friday
        // Need MIN_OBSERVATIONS (3) hits for the pattern to be actionable
        for day in 0..5u8 {
            for i in 0..4 {
                let obs = Observation {
                    action: "check_email".to_string(),
                    timestamp_ms: 1000 + (day as u64 * 86_400_000) + (i * 60_000),
                    minute_of_day: 540.0 + i as f32,
                    day_of_week: day,
                    context: vec![ContextKey::Location("office".to_string())],
                };
                let _ = detector.observe(obs);
            }
        }
        detector
    }

    #[test]
    fn test_new_engine_has_zero_state() {
        let engine = PredictionEngine::new();
        assert_eq!(engine.total_predictions(), 0);
        assert_eq!(engine.total_observations(), 0);
        assert_eq!(engine.mean_surprise(), 0.0);
        assert!(!engine.is_routine_change_detected());
        assert!(!engine.is_proactive_ready());
    }

    #[test]
    fn test_predict_with_no_patterns() {
        let mut engine = PredictionEngine::new();
        let detector = PatternDetector::new();
        let preds = engine.predict(&detector, 540.0, 0, &[], &[]);
        assert!(preds.is_empty(), "empty detector → no predictions");
    }

    #[test]
    fn test_predict_with_temporal_patterns() {
        let mut engine = PredictionEngine::new();
        let detector = setup_detector_with_temporal_patterns();

        // Predict at 9am Monday
        let preds = engine.predict(&detector, 540.0, 0, &[], &[]);
        assert!(
            !preds.is_empty(),
            "should predict check_email at 9am Monday"
        );
        assert_eq!(preds[0].action, "check_email");
        assert!(preds[0].confidence > 0.0);
    }

    #[test]
    fn test_surprise_for_expected_action() {
        let mut engine = PredictionEngine::new();
        let detector = setup_detector_with_temporal_patterns();

        // Generate predictions
        engine.predict(&detector, 540.0, 0, &[], &[]);

        // Observe the predicted action — low surprise
        let signal = engine.compute_surprise("check_email", 1000);
        assert!(
            signal.surprise < 0.5,
            "expected action should have low surprise, got {}",
            signal.surprise
        );
        assert!(signal.prediction_rank.is_some());
        assert_eq!(signal.prediction_rank.unwrap(), 0);
    }

    #[test]
    fn test_surprise_for_unexpected_action() {
        let mut engine = PredictionEngine::new();
        let detector = setup_detector_with_temporal_patterns();

        // Generate predictions
        engine.predict(&detector, 540.0, 0, &[], &[]);

        // Observe something completely unexpected — high surprise
        let signal = engine.compute_surprise("play_game", 1000);
        assert_eq!(signal.surprise, 1.0, "unexpected action → max surprise");
        assert!(signal.prediction_rank.is_none());
    }

    #[test]
    fn test_surprise_with_no_predictions_is_moderate() {
        let mut engine = PredictionEngine::new();
        let detector = PatternDetector::new();

        // No predictions exist
        engine.predict(&detector, 540.0, 0, &[], &[]);

        // Observe anything — should be "novel" (0.5), not "surprising" (1.0)
        let signal = engine.compute_surprise("anything", 1000);
        assert_eq!(
            signal.surprise, 0.5,
            "novel action with no predictions should be 0.5"
        );
    }

    #[test]
    fn test_mean_surprise_tracks_over_time() {
        let mut engine = PredictionEngine::new();
        let detector = PatternDetector::new();
        engine.predict(&detector, 0.0, 0, &[], &[]);

        // 10 novel observations
        for i in 0..10 {
            engine.compute_surprise("action", i * 1000);
        }

        assert!(
            (engine.mean_surprise() - 0.5).abs() < 0.01,
            "10 novel observations → mean surprise ≈ 0.5"
        );
    }

    #[test]
    fn test_routine_change_detection() {
        let mut engine = PredictionEngine::new();
        let detector = setup_detector_with_temporal_patterns();

        // Generate a lot of predictions and fully-surprised observations
        // to trigger routine change detection
        for i in 0..ROUTINE_CHANGE_WINDOW + 10 {
            engine.predict(&detector, 540.0, 0, &[], &[]);
            engine.compute_surprise("completely_different_action", i as u64 * 1000);
        }

        assert!(
            engine.is_routine_change_detected(),
            "sustained high surprise should trigger routine change"
        );
    }

    #[test]
    fn test_serde_roundtrip() {
        let mut engine = PredictionEngine::new();
        // Add some state
        engine.surprise_history.push(0.5);
        engine.total_observations = 10;
        engine.total_surprise = 5.0;

        let json = serde_json::to_string(&engine).expect("serialize");
        let back: PredictionEngine = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.total_observations, 10);
        assert!((back.total_surprise - 5.0).abs() < f64::EPSILON);
        assert_eq!(back.surprise_history.len(), 1);
    }
}
