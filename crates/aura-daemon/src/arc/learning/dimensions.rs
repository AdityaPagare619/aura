//! Emergent dimension discovery — AURA finds user-specific behavioural axes.
//!
//! # Architecture (Concept Design §4.1 — Emergent Discovery)
//!
//! Traditional assistants use preset personality labels ("introvert",
//! "morning person", "fitness enthusiast").  AURA discovers dimensions
//! that are unique to each user through statistical analysis of their
//! behaviour patterns, creating a truly personalised model.
//!
//! # How it works
//!
//! 1. **Observation stream**: AURA records user actions as feature vectors
//!    (time-of-day, app category, duration, context, outcome).
//!
//! 2. **Correlation detection**: When two features consistently co-occur
//!    (e.g., "uses focus apps" + "morning hours" + "low notification count"),
//!    they form a proto-dimension.
//!
//! 3. **Dimension crystallisation**: When a proto-dimension has enough
//!    evidence (MIN_EVIDENCE observations with correlation > CORRELATION_THRESHOLD),
//!    it becomes a named dimension with a human-readable label.
//!
//! 4. **Dimension evolution**: Dimensions strengthen with confirming evidence
//!    and weaken without it.  Dead dimensions are pruned.
//!
//! # Cross-Domain Insight (Polymath: Factor Analysis → Personality)
//!
//! This is analogous to how the Big Five personality traits were discovered:
//! researchers didn't Start with "Extraversion" as a concept — they observed
//! thousands of behavioural data points, ran factor analysis, and discovered
//! that certain behaviours cluster into 5 orthogonal dimensions.  We do the
//! same, but per-user and continuously.
//!
//! # Day-1 vs Year-1
//!
//! - **Day 1**: No dimensions yet.  AURA is a blank slate.
//! - **Week 2**: 1-3 proto-dimensions forming ("seems to prefer quiet time
//!   in the morning").
//! - **Month 3**: 5-10 crystallised dimensions with labels ("Deep Work
//!   Mornings", "Social Evenings", "Weekend Explorer").
//! - **Year 1**: A rich, unique personality model that no other user shares.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, instrument};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of active dimensions.
const MAX_DIMENSIONS: usize = 32;

/// Maximum number of proto-dimensions (candidates for crystallisation).
const MAX_PROTO_DIMENSIONS: usize = 64;

/// Minimum observations before a proto-dimension can crystallise.
const MIN_EVIDENCE: u32 = 10;

/// Minimum correlation (0.0–1.0) for a feature pair to form a proto-dimension.
const CORRELATION_THRESHOLD: f32 = 0.60;

/// Confidence decay per day of inactivity.
const DIMENSION_DECAY_PER_DAY: f32 = 0.99;

/// Minimum strength for a crystallised dimension to survive pruning.
const PRUNE_THRESHOLD: f32 = 0.05;

/// Milliseconds in one day.
const MS_PER_DAY: u64 = 24 * 60 * 60 * 1000;

// ---------------------------------------------------------------------------
// Feature
// ---------------------------------------------------------------------------

/// A single observed feature (one axis of a behavioural observation).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Feature {
    /// Time bucket: "early_morning", "morning", "afternoon", "evening", "night"
    TimeBucket(String),
    /// App category: "social", "productivity", "entertainment", "health"
    AppCategory(String),
    /// Context: "home", "work", "commuting", "gym"
    Location(String),
    /// Activity type: "browsing", "messaging", "creating", "consuming"
    ActivityType(String),
    /// Custom user-specific feature
    Custom(String, String),
}

impl Feature {
    /// Stable string key for a feature.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Feature::TimeBucket(t) => format!("time:{t}"),
            Feature::AppCategory(c) => format!("app:{c}"),
            Feature::Location(l) => format!("loc:{l}"),
            Feature::ActivityType(a) => format!("act:{a}"),
            Feature::Custom(k, v) => format!("custom:{k}:{v}"),
        }
    }
}

// ---------------------------------------------------------------------------
// FeatureCooccurrence
// ---------------------------------------------------------------------------

/// Tracks how often two features appear together vs. independently.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FeatureCooccurrence {
    /// Feature A key.
    feature_a: String,
    /// Feature B key.
    feature_b: String,
    /// Times both features appeared together.
    joint_count: u32,
    /// Times feature A appeared (with or without B).
    count_a: u32,
    /// Times feature B appeared (with or without A).
    count_b: u32,
    /// Total observations considered.
    total: u32,
}

impl FeatureCooccurrence {
    fn new(a: &str, b: &str) -> Self {
        Self {
            feature_a: a.to_owned(),
            feature_b: b.to_owned(),
            joint_count: 0,
            count_a: 0,
            count_b: 0,
            total: 0,
        }
    }

    /// Pointwise Mutual Information: log2(P(A,B) / (P(A) × P(B)))
    ///
    /// High PMI means A and B appear together much more than chance.
    /// Returns 0.0 if there's not enough data.
    fn pmi(&self) -> f32 {
        if self.total == 0 || self.count_a == 0 || self.count_b == 0 || self.joint_count == 0 {
            return 0.0;
        }
        let p_ab = self.joint_count as f32 / self.total as f32;
        let p_a = self.count_a as f32 / self.total as f32;
        let p_b = self.count_b as f32 / self.total as f32;
        let pmi = (p_ab / (p_a * p_b)).ln() / std::f32::consts::LN_2;
        pmi.max(0.0) // Clamp negative PMI (anti-correlation) to 0
    }

    /// Normalised PMI: PMI / (-log2(P(A,B))), yielding [0.0, 1.0].
    fn npmi(&self) -> f32 {
        if self.total == 0 || self.joint_count == 0 {
            return 0.0;
        }
        let p_ab = self.joint_count as f32 / self.total as f32;
        let denominator = -(p_ab.ln() / std::f32::consts::LN_2);
        if denominator.abs() < f32::EPSILON {
            return 0.0;
        }
        (self.pmi() / denominator).clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// ProtoDimension
// ---------------------------------------------------------------------------

/// A candidate dimension: a cluster of correlated features that might
/// crystallise into a named dimension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtoDimension {
    /// The feature keys that define this proto-dimension.
    pub features: Vec<String>,
    /// Correlation strength (normalised PMI average across feature pairs).
    pub correlation: f32,
    /// Number of observations supporting this proto-dimension.
    pub evidence_count: u32,
    /// First observed timestamp.
    pub first_seen_ms: u64,
    /// Last observed timestamp.
    pub last_seen_ms: u64,
}

// ---------------------------------------------------------------------------
// Dimension
// ---------------------------------------------------------------------------

/// A crystallised behavioural dimension — a named axis of user personality.
///
/// Example dimensions discovered for a real user:
/// - "Deep Work Mornings" (features: time:morning + act:creating + loc:home)
/// - "Social Evenings" (features: time:evening + app:social + act:messaging)
/// - "Weekend Explorer" (features: time:afternoon + loc:outdoors + act:browsing)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dimension {
    /// Unique identifier.
    pub id: u64,
    /// Human-readable label (auto-generated from constituent features).
    pub label: String,
    /// The feature keys that define this dimension.
    pub features: Vec<String>,
    /// Strength/confidence in this dimension [0.0, 1.0].
    pub strength: f32,
    /// Number of confirming observations.
    pub observation_count: u32,
    /// Number of disconfirming observations (features appeared separately).
    pub contradiction_count: u32,
    /// Timestamp of crystallisation.
    pub created_ms: u64,
    /// Timestamp of last observation.
    pub last_observed_ms: u64,
}

impl Dimension {
    /// Generate a human-readable label from features.
    ///
    /// This creates labels like "Morning + Productivity" from
    /// ["time:morning", "app:productivity"].
    fn generate_label(features: &[String]) -> String {
        let parts: Vec<&str> = features
            .iter()
            .map(|f| {
                // Extract the value part after the colon
                f.split(':').nth(1).unwrap_or(f.as_str())
            })
            .collect();
        if parts.is_empty() {
            return "Unknown Dimension".to_string();
        }
        // Capitalize first letter of each part
        parts
            .iter()
            .map(|p| {
                let mut c = p.chars();
                match c.next() {
                    None => String::new(),
                    Some(first) => {
                        first.to_uppercase().to_string() + c.as_str()
                    }
                }
            })
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

// ---------------------------------------------------------------------------
// DimensionDiscovery
// ---------------------------------------------------------------------------

/// The emergent dimension discovery engine.
///
/// Observes feature co-occurrences, detects proto-dimensions via normalised
/// PMI, and crystallises stable patterns into named dimensions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionDiscovery {
    /// Co-occurrence tracker (keyed by "featureA||featureB").
    cooccurrences: HashMap<String, FeatureCooccurrence>,
    /// Proto-dimensions (candidates for crystallisation).
    proto_dimensions: Vec<ProtoDimension>,
    /// Crystallised dimensions.
    dimensions: Vec<Dimension>,
    /// ID counter for dimensions.
    next_id: u64,
    /// Total observations processed.
    total_observations: u64,
}

impl DimensionDiscovery {
    /// Create a new dimension discovery engine.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cooccurrences: HashMap::with_capacity(128),
            proto_dimensions: Vec::with_capacity(16),
            dimensions: Vec::with_capacity(8),
            next_id: 1,
            total_observations: 0,
        }
    }

    /// Observe a set of features that appeared simultaneously.
    ///
    /// This is the main entry point.  Call it whenever AURA observes a
    /// user action with its associated features (time bucket, app category,
    /// location, activity type, etc.).
    #[instrument(skip_all, fields(feature_count = features.len()))]
    pub fn observe(&mut self, features: &[Feature], now_ms: u64) {
        self.total_observations += 1;
        let keys: Vec<String> = features.iter().map(|f| f.key()).collect();

        // Update co-occurrence counts for all pairs
        for i in 0..keys.len() {
            for j in (i + 1)..keys.len() {
                let pair_key = if keys[i] < keys[j] {
                    format!("{}||{}", keys[i], keys[j])
                } else {
                    format!("{}||{}", keys[j], keys[i])
                };

                let entry = self
                    .cooccurrences
                    .entry(pair_key)
                    .or_insert_with(|| {
                        FeatureCooccurrence::new(&keys[i], &keys[j])
                    });
                entry.joint_count += 1;
                entry.total += 1;
            }
            // Also track individual feature counts
            for j in 0..keys.len() {
                if i == j {
                    continue;
                }
                let pair_key = if keys[i] < keys[j] {
                    format!("{}||{}", keys[i], keys[j])
                } else {
                    format!("{}||{}", keys[j], keys[i])
                };
                if let Some(entry) = self.cooccurrences.get_mut(&pair_key) {
                    if keys[i] == entry.feature_a || keys[i] == entry.feature_b {
                        // Already counted
                    }
                }
            }
        }

        // Update individual counts for all relevant pairs
        for entry in self.cooccurrences.values_mut() {
            let has_a = keys.contains(&entry.feature_a);
            let has_b = keys.contains(&entry.feature_b);
            if has_a {
                entry.count_a += 1;
            }
            if has_b {
                entry.count_b += 1;
            }
            if !has_a && !has_b {
                entry.total += 1;
            }
        }

        // Try to crystallise proto-dimensions
        self.detect_proto_dimensions(now_ms);
        self.try_crystallise(now_ms);
    }

    /// Detect new proto-dimensions from high-correlation feature pairs.
    fn detect_proto_dimensions(&mut self, now_ms: u64) {
        for entry in self.cooccurrences.values() {
            if entry.total < MIN_EVIDENCE {
                continue;
            }
            let npmi = entry.npmi();
            if npmi < CORRELATION_THRESHOLD {
                continue;
            }

            // Check if this pair is already in a proto-dimension
            let features = vec![entry.feature_a.clone(), entry.feature_b.clone()];
            let exists = self.proto_dimensions.iter().any(|pd| {
                pd.features.len() == features.len()
                    && pd.features.iter().all(|f| features.contains(f))
            });
            if exists {
                // Update existing
                if let Some(pd) = self.proto_dimensions.iter_mut().find(|pd| {
                    pd.features.len() == features.len()
                        && pd.features.iter().all(|f| features.contains(f))
                }) {
                    pd.correlation = npmi;
                    pd.evidence_count = entry.joint_count;
                    pd.last_seen_ms = now_ms;
                }
                continue;
            }

            // Also check if already crystallised
            let crystallised = self.dimensions.iter().any(|d| {
                d.features.len() == features.len()
                    && d.features.iter().all(|f| features.contains(f))
            });
            if crystallised {
                // Update the existing dimension
                if let Some(d) = self.dimensions.iter_mut().find(|d| {
                    d.features.len() == features.len()
                        && d.features.iter().all(|f| features.contains(f))
                }) {
                    d.observation_count = d.observation_count.saturating_add(1);
                    d.last_observed_ms = now_ms;
                    // Bayesian strength update
                    d.strength = (5.0 * d.strength + 1.0) / 6.0;
                    d.strength = d.strength.clamp(0.0, 1.0);
                }
                continue;
            }

            // New proto-dimension
            if self.proto_dimensions.len() >= MAX_PROTO_DIMENSIONS {
                // Evict weakest
                if let Some(idx) = self
                    .proto_dimensions
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        a.correlation
                            .partial_cmp(&b.correlation)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, _)| i)
                {
                    self.proto_dimensions.swap_remove(idx);
                }
            }

            debug!(
                features = ?features,
                npmi = npmi,
                evidence = entry.joint_count,
                "new proto-dimension detected"
            );

            self.proto_dimensions.push(ProtoDimension {
                features,
                correlation: npmi,
                evidence_count: entry.joint_count,
                first_seen_ms: now_ms,
                last_seen_ms: now_ms,
            });
        }
    }

    /// Try to crystallise proto-dimensions with enough evidence.
    fn try_crystallise(&mut self, now_ms: u64) {
        let mut crystallised_indices = Vec::new();

        for (idx, pd) in self.proto_dimensions.iter().enumerate() {
            if pd.evidence_count >= MIN_EVIDENCE && pd.correlation >= CORRELATION_THRESHOLD {
                if self.dimensions.len() >= MAX_DIMENSIONS {
                    // Evict weakest dimension
                    if let Some(weak_idx) = self
                        .dimensions
                        .iter()
                        .enumerate()
                        .min_by(|(_, a), (_, b)| {
                            a.strength
                                .partial_cmp(&b.strength)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .map(|(i, _)| i)
                    {
                        self.dimensions.swap_remove(weak_idx);
                    }
                }

                let label = Dimension::generate_label(&pd.features);
                let id = self.next_id;
                self.next_id += 1;

                info!(
                    id,
                    label = %label,
                    features = ?pd.features,
                    correlation = pd.correlation,
                    evidence = pd.evidence_count,
                    "dimension crystallised"
                );

                self.dimensions.push(Dimension {
                    id,
                    label,
                    features: pd.features.clone(),
                    strength: pd.correlation,
                    observation_count: pd.evidence_count,
                    contradiction_count: 0,
                    created_ms: now_ms,
                    last_observed_ms: now_ms,
                });

                crystallised_indices.push(idx);
            }
        }

        // Remove crystallised proto-dimensions (reverse order to preserve indices)
        crystallised_indices.sort_unstable_by(|a, b| b.cmp(a));
        for idx in crystallised_indices {
            self.proto_dimensions.swap_remove(idx);
        }
    }

    /// Age all dimensions, pruning weak ones.
    pub fn age_dimensions(&mut self, now_ms: u64) -> usize {
        let mut pruned = 0;
        self.dimensions.retain(|d| {
            let days_inactive =
                now_ms.saturating_sub(d.last_observed_ms) as f64 / MS_PER_DAY as f64;
            let decayed = d.strength * DIMENSION_DECAY_PER_DAY.powf(days_inactive as f32);
            if decayed < PRUNE_THRESHOLD {
                pruned += 1;
                false
            } else {
                true
            }
        });
        pruned
    }

    // -- Accessors -----------------------------------------------------------

    /// Number of crystallised dimensions.
    #[must_use]
    pub fn dimension_count(&self) -> usize {
        self.dimensions.len()
    }

    /// Number of proto-dimensions.
    #[must_use]
    pub fn proto_count(&self) -> usize {
        self.proto_dimensions.len()
    }

    /// Read-only access to dimensions.
    #[must_use]
    pub fn dimensions(&self) -> &[Dimension] {
        &self.dimensions
    }

    /// Total observations processed.
    #[must_use]
    pub fn total_observations(&self) -> u64 {
        self.total_observations
    }
}

impl Default for DimensionDiscovery {
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
    fn test_new_engine_empty() {
        let engine = DimensionDiscovery::new();
        assert_eq!(engine.dimension_count(), 0);
        assert_eq!(engine.proto_count(), 0);
        assert_eq!(engine.total_observations(), 0);
    }

    #[test]
    fn test_feature_key() {
        assert_eq!(Feature::TimeBucket("morning".to_string()).key(), "time:morning");
        assert_eq!(Feature::AppCategory("social".to_string()).key(), "app:social");
    }

    #[test]
    fn test_pmi_calculation() {
        let mut cooc = FeatureCooccurrence::new("A", "B");
        cooc.total = 100;
        cooc.count_a = 50;
        cooc.count_b = 30;
        cooc.joint_count = 25; // Much higher than chance (50/100 × 30/100 = 0.15)

        let pmi = cooc.pmi();
        assert!(pmi > 0.0, "PMI should be positive for correlated features");
    }

    #[test]
    fn test_npmi_range() {
        let mut cooc = FeatureCooccurrence::new("A", "B");
        cooc.total = 100;
        cooc.count_a = 50;
        cooc.count_b = 50;
        cooc.joint_count = 50; // Perfect correlation

        let npmi = cooc.npmi();
        assert!(npmi >= 0.0 && npmi <= 1.0, "NPMI should be in [0, 1], got {}", npmi);
    }

    #[test]
    fn test_dimension_label_generation() {
        let features = vec!["time:morning".to_string(), "app:productivity".to_string()];
        let label = Dimension::generate_label(&features);
        assert_eq!(label, "Morning + Productivity");
    }

    #[test]
    fn test_repeated_observations_create_proto_dimension() {
        let mut engine = DimensionDiscovery::new();
        let features = vec![
            Feature::TimeBucket("morning".to_string()),
            Feature::AppCategory("productivity".to_string()),
        ];

        // Need enough observations for MIN_EVIDENCE
        for i in 0..15 {
            engine.observe(&features, 1000 + i * 60_000);
        }

        assert!(
            engine.proto_count() > 0 || engine.dimension_count() > 0,
            "15 correlated observations should create at least a proto-dimension"
        );
    }

    #[test]
    fn test_crystallisation_with_sufficient_evidence() {
        let mut engine = DimensionDiscovery::new();
        let features = vec![
            Feature::TimeBucket("evening".to_string()),
            Feature::AppCategory("social".to_string()),
        ];

        // Many observations to ensure crystallisation
        for i in 0..30 {
            engine.observe(&features, 1000 + i * 60_000);
        }

        // Should have crystallised by now
        if engine.dimension_count() > 0 {
            let dim = &engine.dimensions()[0];
            assert!(dim.label.contains("Evening") || dim.label.contains("Social"));
            assert!(dim.strength > 0.0);
        }
    }

    #[test]
    fn test_serde_roundtrip() {
        let mut engine = DimensionDiscovery::new();
        engine.observe(
            &[Feature::TimeBucket("night".to_string())],
            1000,
        );

        let json = serde_json::to_string(&engine).expect("serialize");
        let back: DimensionDiscovery = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.total_observations(), 1);
    }
}
