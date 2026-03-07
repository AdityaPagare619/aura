//! Hebbian concept learning network — fire-together wire-together.
//!
//! This is the **core learning algorithm** for AURA v4.  It maintains a bounded
//! graph of [`Concept`] nodes linked by weighted [`Association`] edges.
//!
//! # Algorithm (SPEC-ARC §8.4)
//!
//! ```text
//! concept_score = success_rate × 0.5 + importance × 0.3 + |valence| × 0.2
//! ```
//!
//! Associations are strengthened when two concepts co-activate (Hebbian rule)
//! and weakened on failure (anti-Hebbian).  An exponential decay is applied
//! periodically so stale links fade naturally.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use super::super::ArcError;
use super::Outcome;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of concepts the network can hold.
pub const MAX_CONCEPTS: usize = 2048;

/// Maximum number of associations (edges) the network can hold.
pub const MAX_ASSOCIATIONS: usize = 8192;

/// Maximum length of a concept name in bytes.
const MAX_NAME_LEN: usize = 128;

/// Weight increment per co-activation.
const STRENGTHEN_DELTA: f32 = 0.05;

/// Weight decrement per anti-Hebbian event.
const WEAKEN_DELTA: f32 = 0.03;

/// Natural log of 2 — used for half-life decay.
const LN2: f64 = std::f64::consts::LN_2;

// ---------------------------------------------------------------------------
// FNV-1a hash (inline, no external crate)
// ---------------------------------------------------------------------------

const FNV_OFFSET_BASIS: u64 = 14_695_981_039_346_656_037;
const FNV_PRIME: u64 = 1_099_511_628_211;

/// Compute a 64-bit FNV-1a hash of `data`.
#[must_use]
fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Public wrapper around FNV-1a hash for use by sibling modules (e.g. patterns).
#[must_use]
pub fn fnv1a_hash_public(data: &[u8]) -> u64 {
    fnv1a_hash(data)
}

// ---------------------------------------------------------------------------
// Concept
// ---------------------------------------------------------------------------

/// A single learned concept — an abstract idea AURA knows about.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Concept {
    /// Unique identifier (FNV-1a hash of name).
    pub id: u64,
    /// Human-readable name (max [`MAX_NAME_LEN`] bytes).
    pub name: String,
    /// Number of successful activations.
    pub success_count: u32,
    /// Number of failed activations.
    pub failure_count: u32,
    /// Total activation count (success + failure + neutral).
    pub total_activations: u32,
    /// Domain importance weight (0.0–1.0).
    pub importance: f32,
    /// Emotional colouring (−1.0 to +1.0).
    pub valence: f32,
    /// Timestamp (ms) of last activation.
    pub last_activated_ms: u64,
    /// Timestamp (ms) of creation.
    pub created_ms: u64,
}

impl Concept {
    /// Success rate: `success / total`.  Returns 0.0 when no activations.
    #[must_use]
    pub fn success_rate(&self) -> f32 {
        if self.total_activations == 0 {
            return 0.0;
        }
        self.success_count as f32 / self.total_activations as f32
    }
}

// ---------------------------------------------------------------------------
// Association
// ---------------------------------------------------------------------------

/// A weighted link between two concepts (Hebbian edge).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Association {
    /// Hebbian strength (0.0–1.0).
    pub weight: f32,
    /// Number of co-activation events.
    pub co_activations: u32,
    /// Timestamp (ms) of last update.
    pub last_updated_ms: u64,
}

// ---------------------------------------------------------------------------
// HebbianNetwork
// ---------------------------------------------------------------------------

/// The Hebbian concept learning network.
///
/// Bounded to [`MAX_CONCEPTS`] concepts and [`MAX_ASSOCIATIONS`] association edges.
/// When full, the concept with the lowest [`concept_score`] is evicted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HebbianNetwork {
    /// Concept store keyed by FNV-1a hash of name.
    concepts: HashMap<u64, Concept>,
    /// Association edges keyed by ordered `(id_lo, id_hi)`.
    associations: HashMap<(u64, u64), Association>,
    /// Monotonic counter for diagnostics (not used as concept ID — we use name hash).
    concept_counter: u64,
}

impl HebbianNetwork {
    /// Create an empty network.
    #[must_use]
    pub fn new() -> Self {
        Self {
            concepts: HashMap::with_capacity(64),
            associations: HashMap::with_capacity(256),
            concept_counter: 0,
        }
    }

    // -- accessors ----------------------------------------------------------

    /// Number of concepts currently stored.
    #[must_use]
    pub fn concept_count(&self) -> usize {
        self.concepts.len()
    }

    /// Number of associations currently stored.
    #[must_use]
    pub fn association_count(&self) -> usize {
        self.associations.len()
    }

    /// Immutable reference to a concept by id.
    #[must_use]
    pub fn get_concept(&self, id: u64) -> Option<&Concept> {
        self.concepts.get(&id)
    }

    // -- core API -----------------------------------------------------------

    /// Look up a concept by name, or create it if absent.
    ///
    /// When the network is at [`MAX_CONCEPTS`], the concept with the lowest
    /// score is evicted first.
    #[instrument(skip_all, fields(name = %name))]
    pub fn get_or_create_concept(&mut self, name: &str, now_ms: u64) -> Result<u64, ArcError> {
        let trimmed = if name.len() > MAX_NAME_LEN {
            &name[..MAX_NAME_LEN]
        } else {
            name
        };
        let id = fnv1a_hash(trimmed.as_bytes());

        if self.concepts.contains_key(&id) {
            return Ok(id);
        }

        // Capacity check — evict lowest-scoring concept if full.
        if self.concepts.len() >= MAX_CONCEPTS {
            self.evict_lowest_score()?;
        }

        self.concept_counter += 1;
        let concept = Concept {
            id,
            name: trimmed.to_owned(),
            success_count: 0,
            failure_count: 0,
            total_activations: 0,
            importance: 0.5,
            valence: 0.0,
            last_activated_ms: now_ms,
            created_ms: now_ms,
        };
        self.concepts.insert(id, concept);
        debug!(concept_id = id, "created concept");
        Ok(id)
    }

    /// Activate a concept with an outcome, updating stats.
    #[instrument(skip_all, fields(concept_id, ?outcome))]
    pub fn activate(
        &mut self,
        concept_id: u64,
        outcome: Outcome,
        importance: f32,
        now_ms: u64,
    ) -> Result<(), ArcError> {
        let concept = self
            .concepts
            .get_mut(&concept_id)
            .ok_or_else(|| ArcError::NotFound {
                entity: "concept".into(),
                id: concept_id,
            })?;

        concept.total_activations = concept.total_activations.saturating_add(1);
        match outcome {
            Outcome::Success => {
                concept.success_count = concept.success_count.saturating_add(1);
                // Nudge valence positively
                concept.valence = (concept.valence + 0.05).clamp(-1.0, 1.0);
            }
            Outcome::Failure => {
                concept.failure_count = concept.failure_count.saturating_add(1);
                concept.valence = (concept.valence - 0.05).clamp(-1.0, 1.0);
            }
            Outcome::Neutral => {}
        }

        // Exponential moving average for importance
        concept.importance = (concept.importance * 0.8 + importance * 0.2).clamp(0.0, 1.0);
        concept.last_activated_ms = now_ms;
        Ok(())
    }

    /// Strengthen the association between two concepts (Hebbian rule).
    ///
    /// Creates the association if it does not exist.  Bounded by
    /// [`MAX_ASSOCIATIONS`].
    #[instrument(skip_all, fields(a, b))]
    pub fn strengthen_association(&mut self, a: u64, b: u64, now_ms: u64) -> Result<(), ArcError> {
        if !self.concepts.contains_key(&a) {
            return Err(ArcError::NotFound {
                entity: "concept".into(),
                id: a,
            });
        }
        if !self.concepts.contains_key(&b) {
            return Err(ArcError::NotFound {
                entity: "concept".into(),
                id: b,
            });
        }

        let key = ordered_pair(a, b);

        if let Some(assoc) = self.associations.get_mut(&key) {
            assoc.weight = (assoc.weight + STRENGTHEN_DELTA).min(1.0);
            assoc.co_activations = assoc.co_activations.saturating_add(1);
            assoc.last_updated_ms = now_ms;
        } else {
            // New association — check capacity.
            if self.associations.len() >= MAX_ASSOCIATIONS {
                return Err(ArcError::CapacityExceeded {
                    collection: "associations".into(),
                    max: MAX_ASSOCIATIONS,
                });
            }
            self.associations.insert(
                key,
                Association {
                    weight: STRENGTHEN_DELTA,
                    co_activations: 1,
                    last_updated_ms: now_ms,
                },
            );
        }
        Ok(())
    }

    /// Weaken the association between two concepts (anti-Hebbian).
    #[instrument(skip_all, fields(a, b))]
    pub fn weaken_association(&mut self, a: u64, b: u64) -> Result<(), ArcError> {
        let key = ordered_pair(a, b);
        if let Some(assoc) = self.associations.get_mut(&key) {
            assoc.weight = (assoc.weight - WEAKEN_DELTA).max(0.0);
            Ok(())
        } else {
            // No association to weaken — not an error, just a no-op.
            Ok(())
        }
    }

    /// Compute the concept score:
    ///
    /// `success_rate × 0.5 + importance × 0.3 + |valence| × 0.2`
    #[must_use]
    pub fn concept_score(&self, id: u64) -> Option<f32> {
        let c = self.concepts.get(&id)?;
        Some(c.success_rate() * 0.5 + c.importance * 0.3 + c.valence.abs() * 0.2)
    }

    /// Return all concepts associated with `concept_id` whose edge weight ≥
    /// `min_weight`, as `(other_id, weight)` pairs.
    #[must_use]
    pub fn get_associated(&self, concept_id: u64, min_weight: f32) -> Vec<(u64, f32)> {
        let mut result = Vec::new();
        for (&(a, b), assoc) in &self.associations {
            if assoc.weight < min_weight {
                continue;
            }
            if a == concept_id {
                result.push((b, assoc.weight));
            } else if b == concept_id {
                result.push((a, assoc.weight));
            }
        }
        result
    }

    /// Remove all associations with weight below `threshold`.
    /// Returns the number of associations pruned.
    #[instrument(skip_all, fields(threshold))]
    pub fn prune_weak(&mut self, threshold: f32) -> usize {
        let before = self.associations.len();
        self.associations
            .retain(|_, assoc| assoc.weight >= threshold);
        let pruned = before - self.associations.len();
        if pruned > 0 {
            info!(pruned, "pruned weak associations");
        }
        pruned
    }

    /// Apply exponential decay to all association weights.
    ///
    /// Uses the formula `w' = w × 2^(−Δt / half_life)`.
    #[instrument(skip_all, fields(now_ms, half_life_ms))]
    pub fn decay_all(&mut self, now_ms: u64, half_life_ms: u64) {
        if half_life_ms == 0 {
            warn!("decay_all called with half_life_ms=0, skipping");
            return;
        }
        for assoc in self.associations.values_mut() {
            let dt = now_ms.saturating_sub(assoc.last_updated_ms) as f64;
            let decay_factor = (-LN2 * dt / half_life_ms as f64).exp() as f32;
            assoc.weight = (assoc.weight * decay_factor).max(0.0);
        }
    }

    /// Compute the consolidation score for a concept:
    ///
    /// `recency × 0.3 + frequency × 0.3 + importance × 0.4`
    ///
    /// - `recency`: 1.0 if activated within the last hour, decays to 0.0 over 7 days.
    /// - `frequency`: min(total_activations / 100, 1.0)
    #[must_use]
    pub fn consolidation_score(&self, id: u64, now_ms: u64) -> Option<f32> {
        let c = self.concepts.get(&id)?;

        // Recency: exponential decay, half-life = 1 day (86_400_000 ms)
        let dt = now_ms.saturating_sub(c.last_activated_ms) as f64;
        let recency = (-LN2 * dt / 86_400_000.0).exp() as f32;

        // Frequency: normalized activation count (cap at 100)
        let frequency = (c.total_activations as f32 / 100.0).min(1.0);

        Some(recency * 0.3 + frequency * 0.3 + c.importance * 0.4)
    }

    /// Collect all concept IDs.
    #[must_use]
    pub fn concept_ids(&self) -> Vec<u64> {
        self.concepts.keys().copied().collect()
    }

    // -- internals ----------------------------------------------------------

    /// Evict the concept with the lowest score to make room.
    fn evict_lowest_score(&mut self) -> Result<(), ArcError> {
        let victim_id = self
            .concepts
            .keys()
            .copied()
            .min_by(|&a, &b| {
                let sa = self.concept_score(a).unwrap_or(0.0);
                let sb = self.concept_score(b).unwrap_or(0.0);
                sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .ok_or_else(|| ArcError::CapacityExceeded {
                collection: "concepts".into(),
                max: MAX_CONCEPTS,
            })?;

        // Remove the concept and all its associations.
        self.concepts.remove(&victim_id);
        self.associations
            .retain(|&(a, b), _| a != victim_id && b != victim_id);

        debug!(victim_id, "evicted lowest-scoring concept");
        Ok(())
    }
}

impl Default for HebbianNetwork {
    fn default() -> Self {
        Self::new()
    }
}

/// Produce a canonical ordered pair `(min, max)` to avoid duplicate edges.
#[must_use]
fn ordered_pair(a: u64, b: u64) -> (u64, u64) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

// ---------------------------------------------------------------------------
// Spreading Activation Constants (ported from V3 hebbian_self_correction.py)
// ---------------------------------------------------------------------------

/// Activation energy decay per hop during spreading activation.
const SPREAD_ACTIVATION_DECAY: f32 = 0.5;

/// Minimum activation level to be considered "fired".
const ACTIVATION_THRESHOLD: f32 = 0.3;

/// Stronger strengthen factor used for outcome-based learning (V3 §success).
const SUCCESS_STRENGTHEN_FACTOR: f32 = 0.2;

/// Stronger weaken factor used for outcome-based learning (V3 §failure).
const FAILURE_WEAKEN_FACTOR: f32 = 0.3;

/// Maximum depth for spreading activation traversal.
const MAX_SPREAD_DEPTH: usize = 5;

/// Maximum entries in a `LocalActivationMap`.
const MAX_ACTIVATION_ENTRIES: usize = 512;

/// User correction strengthen factor — stronger than normal success.
const USER_CORRECTION_STRENGTHEN: f32 = 0.35;

/// User correction weaken factor — stronger than normal failure.
const USER_CORRECTION_WEAKEN: f32 = 0.40;

// ---------------------------------------------------------------------------
// ActivationEntry
// ---------------------------------------------------------------------------

/// A single entry in the local activation map — tracks transient activation
/// level for a concept during spreading activation or a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivationEntry {
    /// The concept this activation belongs to.
    pub concept_id: u64,
    /// Current activation level (0.0–1.0+). Can temporarily exceed 1.0
    /// during multi-source activation before clamping.
    pub activation_level: f32,
    /// Timestamp of last activation update.
    pub last_activated_ms: u64,
    /// Whether the activation has crossed the firing threshold.
    pub fired: bool,
    /// How many times this concept was activated in this map's lifetime.
    pub activation_count: u32,
}

// ---------------------------------------------------------------------------
// LocalActivationMap — lightweight per-session activation tracking
// ---------------------------------------------------------------------------

/// A lightweight, bounded activation map for transient per-session Hebbian
/// learning. Ported from V3's `LocalActivationMap` class.
///
/// This is **not** the persistent `HebbianNetwork` — it is a short-lived map
/// used during spreading activation, dreaming sessions, or single-request
/// context windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalActivationMap {
    /// Active concept entries keyed by concept ID.
    entries: HashMap<u64, ActivationEntry>,
    /// Maximum number of entries before eviction.
    max_entries: usize,
}

impl LocalActivationMap {
    /// Create an empty activation map with the default capacity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::with_capacity(64),
            max_entries: MAX_ACTIVATION_ENTRIES,
        }
    }

    /// Create with a custom max-entry limit.
    #[must_use]
    pub fn with_capacity(max_entries: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(max_entries.min(64)),
            max_entries,
        }
    }

    /// Number of active entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the map is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Activate a concept — adds energy to its activation level.
    ///
    /// If the concept isn't tracked yet, creates a new entry.
    /// If at capacity, evicts the entry with the lowest activation level.
    pub fn activate(&mut self, concept_id: u64, energy: f32, now_ms: u64) {
        if let Some(entry) = self.entries.get_mut(&concept_id) {
            entry.activation_level += energy;
            entry.last_activated_ms = now_ms;
            entry.activation_count = entry.activation_count.saturating_add(1);
            if entry.activation_level >= ACTIVATION_THRESHOLD {
                entry.fired = true;
            }
        } else {
            // Evict if at capacity.
            if self.entries.len() >= self.max_entries {
                self.evict_lowest();
            }
            let fired = energy >= ACTIVATION_THRESHOLD;
            self.entries.insert(
                concept_id,
                ActivationEntry {
                    concept_id,
                    activation_level: energy,
                    last_activated_ms: now_ms,
                    fired,
                    activation_count: 1,
                },
            );
        }
    }

    /// Get the current activation level of a concept, or 0.0 if not tracked.
    #[must_use]
    pub fn get_activation(&self, concept_id: u64) -> f32 {
        self.entries
            .get(&concept_id)
            .map(|e| e.activation_level)
            .unwrap_or(0.0)
    }

    /// Get the full activation entry, if present.
    #[must_use]
    pub fn get_entry(&self, concept_id: u64) -> Option<&ActivationEntry> {
        self.entries.get(&concept_id)
    }

    /// Apply uniform decay to all entries, reducing activation levels.
    ///
    /// `decay_factor` is multiplied against each level (e.g., 0.9 = 10% decay).
    pub fn decay(&mut self, decay_factor: f32) {
        let factor = decay_factor.clamp(0.0, 1.0);
        for entry in self.entries.values_mut() {
            entry.activation_level *= factor;
            if entry.activation_level < ACTIVATION_THRESHOLD {
                entry.fired = false;
            }
        }
    }

    /// Return all entries whose activation level is at or above `threshold`,
    /// sorted descending by activation level.
    #[must_use]
    pub fn above_threshold(&self, threshold: f32) -> Vec<(u64, f32)> {
        let mut result: Vec<(u64, f32)> = self
            .entries
            .values()
            .filter(|e| e.activation_level >= threshold)
            .map(|e| (e.concept_id, e.activation_level))
            .collect();
        result.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        result
    }

    /// Return all entries that have crossed the firing threshold.
    #[must_use]
    pub fn fired_concepts(&self) -> Vec<(u64, f32)> {
        let mut result: Vec<(u64, f32)> = self
            .entries
            .values()
            .filter(|e| e.fired)
            .map(|e| (e.concept_id, e.activation_level))
            .collect();
        result.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        result
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Evict the entry with the lowest activation level.
    fn evict_lowest(&mut self) {
        if let Some((&victim_id, _)) = self.entries.iter().min_by(|a, b| {
            a.1.activation_level
                .partial_cmp(&b.1.activation_level)
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            self.entries.remove(&victim_id);
        }
    }
}

impl Default for LocalActivationMap {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// ActionRecommendation — result from spreading activation analysis
// ---------------------------------------------------------------------------

/// How a recommendation was derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecommendationSource {
    /// Directly associated concept.
    Direct,
    /// Found via spreading activation through the concept graph.
    SpreadingActivation,
    /// Found as an alternative after a failure.
    Alternative,
    /// Provided by an explicit user correction.
    UserCorrection,
}

/// A recommended action/concept with confidence and provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRecommendation {
    /// The recommended concept ID.
    pub concept_id: u64,
    /// Human-readable concept name.
    pub concept_name: String,
    /// Confidence score (0.0–1.0).
    pub confidence: f32,
    /// Path of concept IDs traversed to reach this recommendation.
    pub path: Vec<u64>,
    /// How this recommendation was derived.
    pub source: RecommendationSource,
}

// ---------------------------------------------------------------------------
// HebbianNetwork — spreading activation extensions
// ---------------------------------------------------------------------------

impl HebbianNetwork {
    /// Perform spreading activation from a source concept.
    ///
    /// Energy starts at `initial_energy` and decays by [`SPREAD_ACTIVATION_DECAY`]
    /// per hop. Traversal stops at `max_depth` or when energy falls below a
    /// minimum threshold (0.01).
    ///
    /// Returns a [`LocalActivationMap`] containing all concepts that received
    /// activation energy.
    ///
    /// # Algorithm
    ///
    /// Breadth-first spreading: each hop transfers `energy × edge_weight × decay`
    /// to neighbours. This mirrors V3's `_find_alternative_paths()` logic.
    #[instrument(skip_all, fields(source_id, initial_energy, max_depth))]
    pub fn spread_activation(
        &self,
        source_id: u64,
        initial_energy: f32,
        max_depth: usize,
        now_ms: u64,
    ) -> Result<LocalActivationMap, ArcError> {
        if !self.concepts.contains_key(&source_id) {
            return Err(ArcError::NotFound {
                entity: "concept".into(),
                id: source_id,
            });
        }

        let depth = max_depth.min(MAX_SPREAD_DEPTH);
        let mut activation_map = LocalActivationMap::new();
        activation_map.activate(source_id, initial_energy, now_ms);

        // BFS frontier: (concept_id, energy_at_this_node, current_depth)
        let mut frontier: Vec<(u64, f32, usize)> = vec![(source_id, initial_energy, 0)];
        let mut visited: HashMap<u64, f32> = HashMap::new();
        visited.insert(source_id, initial_energy);

        while let Some((current_id, current_energy, current_depth)) = frontier.pop() {
            if current_depth >= depth {
                continue;
            }

            let neighbours = self.get_associated(current_id, 0.01);
            for (neighbour_id, edge_weight) in neighbours {
                let propagated = current_energy * edge_weight * SPREAD_ACTIVATION_DECAY;
                if propagated < 0.01 {
                    continue;
                }

                // Only propagate if this path delivers more energy than any
                // previous path to this node.
                let existing = visited.get(&neighbour_id).copied().unwrap_or(0.0);
                if propagated > existing {
                    visited.insert(neighbour_id, propagated);
                    activation_map.activate(neighbour_id, propagated, now_ms);
                    frontier.push((neighbour_id, propagated, current_depth + 1));
                }
            }
        }

        debug!(
            source_id,
            activated = activation_map.len(),
            "spreading activation complete"
        );
        Ok(activation_map)
    }

    /// Find alternative concepts when `failed_concept` has failed, using
    /// spreading activation from the context concepts.
    ///
    /// This is the Rust port of V3's `HebbianSelfCorrector._find_alternative_paths()`.
    ///
    /// 1. Spreads activation from each context concept.
    /// 2. Collects all concepts that fired above threshold.
    /// 3. Filters out the failed concept.
    /// 4. Ranks by activation level × concept score.
    /// 5. Returns the top `max_alternatives` recommendations.
    #[instrument(skip_all, fields(failed_concept, num_context = context_concepts.len()))]
    pub fn find_alternative_paths(
        &self,
        failed_concept: u64,
        context_concepts: &[u64],
        max_alternatives: usize,
        now_ms: u64,
    ) -> Result<Vec<ActionRecommendation>, ArcError> {
        let mut combined_map = LocalActivationMap::new();

        for &ctx_id in context_concepts {
            if !self.concepts.contains_key(&ctx_id) {
                continue;
            }
            let local = self.spread_activation(ctx_id, 1.0, 3, now_ms)?;
            // Merge into combined map
            for (cid, level) in local.above_threshold(0.0) {
                combined_map.activate(cid, level, now_ms);
            }
        }

        // Collect candidates — above a relaxed threshold, excluding the failed concept.
        // We use half the normal firing threshold because spreading activation
        // naturally attenuates through multi-hop paths, so strict thresholds
        // would eliminate viable alternatives.
        let alt_threshold = ACTIVATION_THRESHOLD * 0.5;
        let mut candidates: Vec<(u64, f32)> = combined_map
            .above_threshold(alt_threshold)
            .into_iter()
            .filter(|&(id, _)| id != failed_concept)
            .collect();

        // Rank by activation × concept_score.
        candidates.sort_by(|a, b| {
            let score_a = a.1 * self.concept_score(a.0).unwrap_or(0.0);
            let score_b = b.1 * self.concept_score(b.0).unwrap_or(0.0);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let recommendations: Vec<ActionRecommendation> = candidates
            .into_iter()
            .take(max_alternatives)
            .filter_map(|(cid, activation)| {
                let concept = self.concepts.get(&cid)?;
                let confidence =
                    (activation * self.concept_score(cid).unwrap_or(0.5)).clamp(0.0, 1.0);
                Some(ActionRecommendation {
                    concept_id: cid,
                    concept_name: concept.name.clone(),
                    confidence,
                    path: vec![cid],
                    source: RecommendationSource::Alternative,
                })
            })
            .collect();

        debug!(
            failed_concept,
            alternatives = recommendations.len(),
            "alternative paths found"
        );
        Ok(recommendations)
    }

    /// Get the best action recommendation for a concept in a given context.
    ///
    /// Returns the concept itself (if healthy) with confidence based on its
    /// score, plus alternatives found via spreading activation from context.
    ///
    /// This mirrors V3's `HebbianSelfCorrector.get_action_recommendation()`.
    pub fn get_action_recommendation(
        &self,
        concept_id: u64,
        context: &[u64],
        now_ms: u64,
    ) -> Result<Option<ActionRecommendation>, ArcError> {
        let concept = match self.concepts.get(&concept_id) {
            Some(c) => c,
            None => return Ok(None),
        };

        let score = self.concept_score(concept_id).unwrap_or(0.0);
        let success_rate = concept.success_rate();

        // If the concept has a reasonable track record, recommend it directly.
        if success_rate >= 0.5 || concept.total_activations < 3 {
            return Ok(Some(ActionRecommendation {
                concept_id,
                concept_name: concept.name.clone(),
                confidence: score,
                path: vec![concept_id],
                source: RecommendationSource::Direct,
            }));
        }

        // Otherwise, find an alternative via spreading activation.
        let alternatives = self.find_alternative_paths(concept_id, context, 1, now_ms)?;
        if let Some(alt) = alternatives.into_iter().next() {
            Ok(Some(alt))
        } else {
            // No alternative found — still return the original but with lower confidence.
            Ok(Some(ActionRecommendation {
                concept_id,
                concept_name: concept.name.clone(),
                confidence: score * 0.5,
                path: vec![concept_id],
                source: RecommendationSource::Direct,
            }))
        }
    }

    /// Record the outcome of an action, using V3-style stronger factors.
    ///
    /// On success: strengthens all associations between `concept_id` and each
    /// context concept by [`SUCCESS_STRENGTHEN_FACTOR`].
    ///
    /// On failure: weakens all associations by [`FAILURE_WEAKEN_FACTOR`] and
    /// nudges the concept's valence negatively.
    #[instrument(skip_all, fields(concept_id, ?outcome, context_len = context.len()))]
    pub fn record_outcome(
        &mut self,
        concept_id: u64,
        outcome: Outcome,
        context: &[u64],
        now_ms: u64,
    ) -> Result<(), ArcError> {
        // Activate the concept.
        let importance = match outcome {
            Outcome::Success => 0.8,
            Outcome::Failure => 0.6,
            Outcome::Neutral => 0.5,
        };
        self.activate(concept_id, outcome, importance, now_ms)?;

        // Strengthen or weaken associations with context.
        for &ctx_id in context {
            if !self.concepts.contains_key(&ctx_id) {
                continue;
            }
            let key = ordered_pair(concept_id, ctx_id);
            match outcome {
                Outcome::Success => {
                    if let Some(assoc) = self.associations.get_mut(&key) {
                        assoc.weight = (assoc.weight + SUCCESS_STRENGTHEN_FACTOR).min(1.0);
                        assoc.co_activations = assoc.co_activations.saturating_add(1);
                        assoc.last_updated_ms = now_ms;
                    } else if self.associations.len() < MAX_ASSOCIATIONS {
                        self.associations.insert(
                            key,
                            Association {
                                weight: SUCCESS_STRENGTHEN_FACTOR,
                                co_activations: 1,
                                last_updated_ms: now_ms,
                            },
                        );
                    }
                }
                Outcome::Failure => {
                    if let Some(assoc) = self.associations.get_mut(&key) {
                        assoc.weight = (assoc.weight - FAILURE_WEAKEN_FACTOR).max(0.0);
                        assoc.last_updated_ms = now_ms;
                    }
                }
                Outcome::Neutral => {
                    // Light strengthen for neutral — same as original delta.
                    if let Some(assoc) = self.associations.get_mut(&key) {
                        assoc.weight = (assoc.weight + STRENGTHEN_DELTA).min(1.0);
                        assoc.last_updated_ms = now_ms;
                    }
                }
            }
        }

        debug!(concept_id, ?outcome, "outcome recorded with context");
        Ok(())
    }

    /// Record an explicit user correction: "I meant X, not Y."
    ///
    /// This strongly strengthens the correct concept and strongly weakens
    /// the incorrect one, plus adjusts their mutual associations with context.
    ///
    /// Mirrors V3's `HebbianSelfCorrector.record_user_correction()`.
    #[instrument(skip_all, fields(incorrect_id, correct_id))]
    pub fn record_user_correction(
        &mut self,
        incorrect_id: u64,
        correct_id: u64,
        context: &[u64],
        now_ms: u64,
    ) -> Result<(), ArcError> {
        // Validate both concepts exist.
        if !self.concepts.contains_key(&incorrect_id) {
            return Err(ArcError::NotFound {
                entity: "concept".into(),
                id: incorrect_id,
            });
        }
        if !self.concepts.contains_key(&correct_id) {
            return Err(ArcError::NotFound {
                entity: "concept".into(),
                id: correct_id,
            });
        }

        // Penalise the incorrect concept.
        self.activate(incorrect_id, Outcome::Failure, 0.3, now_ms)?;
        // Reward the correct concept.
        self.activate(correct_id, Outcome::Success, 0.9, now_ms)?;

        // Weaken incorrect ↔ context associations.
        for &ctx_id in context {
            let key = ordered_pair(incorrect_id, ctx_id);
            if let Some(assoc) = self.associations.get_mut(&key) {
                assoc.weight = (assoc.weight - USER_CORRECTION_WEAKEN).max(0.0);
                assoc.last_updated_ms = now_ms;
            }
        }

        // Strengthen correct ↔ context associations.
        for &ctx_id in context {
            if !self.concepts.contains_key(&ctx_id) {
                continue;
            }
            let key = ordered_pair(correct_id, ctx_id);
            if let Some(assoc) = self.associations.get_mut(&key) {
                assoc.weight = (assoc.weight + USER_CORRECTION_STRENGTHEN).min(1.0);
                assoc.co_activations = assoc.co_activations.saturating_add(1);
                assoc.last_updated_ms = now_ms;
            } else if self.associations.len() < MAX_ASSOCIATIONS {
                self.associations.insert(
                    key,
                    Association {
                        weight: USER_CORRECTION_STRENGTHEN,
                        co_activations: 1,
                        last_updated_ms: now_ms,
                    },
                );
            }
        }

        // Weaken incorrect ↔ correct direct association.
        let direct_key = ordered_pair(incorrect_id, correct_id);
        if let Some(assoc) = self.associations.get_mut(&direct_key) {
            assoc.weight = (assoc.weight - USER_CORRECTION_WEAKEN).max(0.0);
            assoc.last_updated_ms = now_ms;
        }

        info!(incorrect_id, correct_id, "user correction recorded");
        Ok(())
    }

    /// Get a mutable reference to a concept by id.
    pub fn get_concept_mut(&mut self, id: u64) -> Option<&mut Concept> {
        self.concepts.get_mut(&id)
    }

    /// Iterate over all concepts.
    pub fn concepts_iter(&self) -> impl Iterator<Item = (&u64, &Concept)> {
        self.concepts.iter()
    }

    /// Compute the concept ID for a given name (FNV-1a hash).
    #[must_use]
    pub fn concept_id_for_name(name: &str) -> u64 {
        let trimmed = if name.len() > MAX_NAME_LEN {
            &name[..MAX_NAME_LEN]
        } else {
            name
        };
        fnv1a_hash(trimmed.as_bytes())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_net() -> HebbianNetwork {
        HebbianNetwork::new()
    }

    #[test]
    fn test_fnv1a_deterministic() {
        let h1 = fnv1a_hash(b"hello");
        let h2 = fnv1a_hash(b"hello");
        assert_eq!(h1, h2);
        assert_ne!(h1, fnv1a_hash(b"world"));
    }

    #[test]
    fn test_create_concept() {
        let mut net = make_net();
        let id = net.get_or_create_concept("music", 1000).expect("create");
        assert_eq!(net.concept_count(), 1);
        let c = net.get_concept(id).expect("lookup");
        assert_eq!(c.name, "music");
        assert_eq!(c.created_ms, 1000);
    }

    #[test]
    fn test_create_concept_idempotent() {
        let mut net = make_net();
        let id1 = net.get_or_create_concept("music", 1000).expect("first");
        let id2 = net.get_or_create_concept("music", 2000).expect("second");
        assert_eq!(id1, id2);
        assert_eq!(net.concept_count(), 1);
    }

    #[test]
    fn test_activate_success() {
        let mut net = make_net();
        let id = net.get_or_create_concept("music", 100).expect("create");
        net.activate(id, Outcome::Success, 0.8, 200)
            .expect("activate");
        let c = net.get_concept(id).expect("lookup");
        assert_eq!(c.success_count, 1);
        assert_eq!(c.total_activations, 1);
        assert!(c.valence > 0.0);
    }

    #[test]
    fn test_activate_failure() {
        let mut net = make_net();
        let id = net.get_or_create_concept("alarm", 100).expect("create");
        net.activate(id, Outcome::Failure, 0.5, 200)
            .expect("activate");
        let c = net.get_concept(id).expect("lookup");
        assert_eq!(c.failure_count, 1);
        assert!(c.valence < 0.0);
    }

    #[test]
    fn test_activate_not_found() {
        let mut net = make_net();
        let result = net.activate(99999, Outcome::Success, 0.5, 100);
        assert!(result.is_err());
    }

    #[test]
    fn test_strengthen_association() {
        let mut net = make_net();
        let a = net.get_or_create_concept("coffee", 100).expect("a");
        let b = net.get_or_create_concept("morning", 100).expect("b");

        net.strengthen_association(a, b, 200).expect("strengthen");
        assert_eq!(net.association_count(), 1);

        let assocs = net.get_associated(a, 0.0);
        assert_eq!(assocs.len(), 1);
        assert_eq!(assocs[0].0, b);
        assert!((assocs[0].1 - STRENGTHEN_DELTA).abs() < f32::EPSILON);
    }

    #[test]
    fn test_strengthen_repeated() {
        let mut net = make_net();
        let a = net.get_or_create_concept("coffee", 100).expect("a");
        let b = net.get_or_create_concept("morning", 100).expect("b");

        net.strengthen_association(a, b, 200).expect("1");
        net.strengthen_association(a, b, 300).expect("2");
        net.strengthen_association(a, b, 400).expect("3");

        let assocs = net.get_associated(a, 0.0);
        assert_eq!(assocs.len(), 1);
        let expected = STRENGTHEN_DELTA * 3.0;
        assert!((assocs[0].1 - expected).abs() < f32::EPSILON);
    }

    #[test]
    fn test_weaken_association() {
        let mut net = make_net();
        let a = net.get_or_create_concept("coffee", 100).expect("a");
        let b = net.get_or_create_concept("morning", 100).expect("b");

        // Strengthen a few times then weaken
        for _ in 0..5 {
            net.strengthen_association(a, b, 200).expect("strengthen");
        }
        let before = net.get_associated(a, 0.0)[0].1;
        net.weaken_association(a, b).expect("weaken");
        let after = net.get_associated(a, 0.0)[0].1;
        assert!(after < before);
    }

    #[test]
    fn test_weaken_nonexistent_is_ok() {
        let mut net = make_net();
        // Weaken an association that doesn't exist — should be a no-op
        let result = net.weaken_association(1, 2);
        assert!(result.is_ok());
    }

    #[test]
    fn test_concept_score_formula() {
        let mut net = make_net();
        let id = net.get_or_create_concept("test", 100).expect("create");

        // Activate 4 times: 3 success, 1 failure
        for _ in 0..3 {
            net.activate(id, Outcome::Success, 0.9, 200).expect("ok");
        }
        net.activate(id, Outcome::Failure, 0.9, 300).expect("ok");

        let c = net.get_concept(id).expect("lookup");
        let _rate = c.success_rate(); // 3/4 = 0.75

        let score = net.concept_score(id).expect("score");
        // score = 0.75 * 0.5 + importance * 0.3 + |valence| * 0.2
        // importance evolved via EMA, valence evolved too — just verify range
        assert!(score > 0.0 && score <= 1.0, "score = {score}");
        // Verify success_rate component dominates
        assert!(score > 0.3, "expected score > 0.3, got {score}");
    }

    #[test]
    fn test_concept_score_not_found() {
        let net = make_net();
        assert!(net.concept_score(12345).is_none());
    }

    #[test]
    fn test_prune_weak() {
        let mut net = make_net();
        let a = net.get_or_create_concept("x", 100).expect("a");
        let b = net.get_or_create_concept("y", 100).expect("b");
        let c = net.get_or_create_concept("z", 100).expect("c");

        net.strengthen_association(a, b, 200).expect("ab");
        // a-c: strengthen 10 times to make it strong
        for _ in 0..10 {
            net.strengthen_association(a, c, 200).expect("ac");
        }

        // a-b is weak (0.05), a-c is strong (0.50)
        let pruned = net.prune_weak(0.1);
        assert_eq!(pruned, 1);
        assert_eq!(net.association_count(), 1);
    }

    #[test]
    fn test_decay_all() {
        let mut net = make_net();
        let a = net.get_or_create_concept("x", 0).expect("a");
        let b = net.get_or_create_concept("y", 0).expect("b");

        // Strengthen 20 times at t=0 → weight = 1.0 (capped)
        for _ in 0..20 {
            net.strengthen_association(a, b, 0).expect("ok");
        }

        let before = net.get_associated(a, 0.0)[0].1;
        assert!((before - 1.0).abs() < f32::EPSILON);

        // Decay with half_life = 1000ms, now = 1000ms → factor ≈ 0.5
        net.decay_all(1000, 1000);
        let after = net.get_associated(a, 0.0)[0].1;
        assert!(
            (after - 0.5).abs() < 0.01,
            "expected ~0.5 after one half-life, got {after}"
        );
    }

    #[test]
    fn test_decay_zero_half_life() {
        let mut net = make_net();
        // Should not panic — just warn and return
        net.decay_all(1000, 0);
    }

    #[test]
    fn test_get_associated_min_weight() {
        let mut net = make_net();
        let a = net.get_or_create_concept("hub", 100).expect("a");
        let b = net.get_or_create_concept("spoke1", 100).expect("b");
        let c = net.get_or_create_concept("spoke2", 100).expect("c");

        net.strengthen_association(a, b, 200).expect("ab"); // 0.05
        for _ in 0..10 {
            net.strengthen_association(a, c, 200).expect("ac"); // 0.50
        }

        // Only strong associations
        let strong = net.get_associated(a, 0.3);
        assert_eq!(strong.len(), 1);
        assert_eq!(strong[0].0, c);

        // All associations
        let all = net.get_associated(a, 0.0);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_eviction_on_capacity() {
        let mut net = make_net();
        // Fill to MAX_CONCEPTS
        for i in 0..MAX_CONCEPTS {
            let name = format!("concept_{i}");
            net.get_or_create_concept(&name, 100).expect("create");
        }
        assert_eq!(net.concept_count(), MAX_CONCEPTS);

        // One more should succeed via eviction
        let new_id = net
            .get_or_create_concept("overflow_concept", 200)
            .expect("create after eviction");
        assert_eq!(net.concept_count(), MAX_CONCEPTS);
        assert!(net.get_concept(new_id).is_some());
    }

    #[test]
    fn test_consolidation_score() {
        let mut net = make_net();
        let id = net.get_or_create_concept("recent", 1000).expect("create");

        // Activate a bunch of times to build frequency
        for i in 0..50 {
            net.activate(id, Outcome::Success, 0.9, 1000 + i)
                .expect("ok");
        }

        // Consolidation right now (recency = ~1.0)
        let score = net.consolidation_score(id, 1050).expect("score");
        // recency ≈ 1.0, frequency = 50/100 = 0.5, importance ≈ 0.9
        // score ≈ 1.0*0.3 + 0.5*0.3 + 0.9*0.4 = 0.3 + 0.15 + 0.36 = 0.81
        assert!(
            score > 0.7,
            "expected score > 0.7 for consolidation, got {score}"
        );
    }

    #[test]
    fn test_name_truncation() {
        let mut net = make_net();
        let long_name = "a".repeat(300);
        let id = net.get_or_create_concept(&long_name, 100).expect("create");
        let c = net.get_concept(id).expect("lookup");
        assert_eq!(c.name.len(), MAX_NAME_LEN);
    }

    #[test]
    fn test_ordered_pair_symmetry() {
        assert_eq!(ordered_pair(1, 2), ordered_pair(2, 1));
        assert_eq!(ordered_pair(5, 5), (5, 5));
    }

    #[test]
    fn test_association_symmetry() {
        let mut net = make_net();
        let a = net.get_or_create_concept("left", 100).expect("a");
        let b = net.get_or_create_concept("right", 100).expect("b");

        // Strengthen a→b
        net.strengthen_association(a, b, 200).expect("ab");
        // Query b→a should find the same association
        let from_b = net.get_associated(b, 0.0);
        assert_eq!(from_b.len(), 1);
        assert_eq!(from_b[0].0, a);
    }

    // =======================================================================
    // Spreading activation / LocalActivationMap tests
    // =======================================================================

    #[test]
    fn test_local_activation_map_new() {
        let map = LocalActivationMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn test_local_activation_map_activate() {
        let mut map = LocalActivationMap::new();
        map.activate(1, 0.5, 1000);
        assert_eq!(map.len(), 1);
        assert!((map.get_activation(1) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_local_activation_map_accumulates() {
        let mut map = LocalActivationMap::new();
        map.activate(1, 0.2, 1000);
        map.activate(1, 0.3, 1001);
        // Should sum to 0.5
        assert!((map.get_activation(1) - 0.5).abs() < f32::EPSILON);
        // Should have fired (0.5 >= 0.3 threshold)
        let entry = map.get_entry(1).expect("entry");
        assert!(entry.fired);
        assert_eq!(entry.activation_count, 2);
    }

    #[test]
    fn test_local_activation_map_decay() {
        let mut map = LocalActivationMap::new();
        map.activate(1, 1.0, 1000);
        map.decay(0.5);
        assert!((map.get_activation(1) - 0.5).abs() < f32::EPSILON);
        // After decay below threshold, fired should be false
        map.decay(0.5); // now 0.25
        let entry = map.get_entry(1).expect("entry");
        assert!(!entry.fired);
    }

    #[test]
    fn test_local_activation_map_above_threshold() {
        let mut map = LocalActivationMap::new();
        map.activate(1, 0.1, 1000); // below default threshold
        map.activate(2, 0.5, 1000); // above
        map.activate(3, 0.8, 1000); // above
        let above = map.above_threshold(0.3);
        assert_eq!(above.len(), 2);
        // Should be sorted descending
        assert_eq!(above[0].0, 3);
        assert_eq!(above[1].0, 2);
    }

    #[test]
    fn test_local_activation_map_fired_concepts() {
        let mut map = LocalActivationMap::new();
        map.activate(1, 0.1, 1000); // below threshold
        map.activate(2, 0.5, 1000); // above
        let fired = map.fired_concepts();
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].0, 2);
    }

    #[test]
    fn test_local_activation_map_eviction() {
        let mut map = LocalActivationMap::with_capacity(2);
        map.activate(1, 0.1, 1000);
        map.activate(2, 0.5, 1000);
        // At capacity — adding third should evict lowest (id=1, level=0.1)
        map.activate(3, 0.8, 1000);
        assert_eq!(map.len(), 2);
        assert!((map.get_activation(1) - 0.0).abs() < f32::EPSILON); // evicted
        assert!((map.get_activation(2) - 0.5).abs() < f32::EPSILON);
        assert!((map.get_activation(3) - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn test_local_activation_map_clear() {
        let mut map = LocalActivationMap::new();
        map.activate(1, 0.5, 1000);
        map.activate(2, 0.5, 1000);
        map.clear();
        assert!(map.is_empty());
    }

    #[test]
    fn test_spread_activation_basic() {
        let mut net = make_net();
        let a = net.get_or_create_concept("hub", 100).expect("a");
        let b = net.get_or_create_concept("spoke1", 100).expect("b");
        let c = net.get_or_create_concept("spoke2", 100).expect("c");

        // Build strong associations: hub → spoke1, hub → spoke2
        for _ in 0..10 {
            net.strengthen_association(a, b, 200).expect("ab");
            net.strengthen_association(a, c, 200).expect("ac");
        }

        let map = net.spread_activation(a, 1.0, 2, 300).expect("spread");
        // Hub itself should be activated
        assert!(map.get_activation(a) > 0.0);
        // Spokes should have received propagated energy
        assert!(map.get_activation(b) > 0.0, "spoke1 should be activated");
        assert!(map.get_activation(c) > 0.0, "spoke2 should be activated");
    }

    #[test]
    fn test_spread_activation_decays_with_depth() {
        let mut net = make_net();
        let a = net.get_or_create_concept("a", 100).expect("a");
        let b = net.get_or_create_concept("b", 100).expect("b");
        let c = net.get_or_create_concept("c", 100).expect("c");

        // Build chain: a → b → c with strong weights
        for _ in 0..20 {
            net.strengthen_association(a, b, 200).expect("ab");
            net.strengthen_association(b, c, 200).expect("bc");
        }

        let map = net.spread_activation(a, 1.0, 3, 300).expect("spread");
        let level_b = map.get_activation(b);
        let level_c = map.get_activation(c);

        // c should receive less energy than b (one more hop of decay)
        assert!(
            level_b > level_c,
            "energy should decay: b={level_b}, c={level_c}"
        );
    }

    #[test]
    fn test_spread_activation_not_found() {
        let net = make_net();
        let result = net.spread_activation(99999, 1.0, 2, 100);
        assert!(result.is_err());
    }

    #[test]
    fn test_spread_activation_depth_zero() {
        let mut net = make_net();
        let a = net.get_or_create_concept("lonely", 100).expect("a");
        let b = net.get_or_create_concept("friend", 100).expect("b");
        for _ in 0..10 {
            net.strengthen_association(a, b, 200).expect("ab");
        }

        // Depth 0 — should only activate source
        let map = net.spread_activation(a, 1.0, 0, 300).expect("spread");
        assert!(map.get_activation(a) > 0.0);
        assert!(
            (map.get_activation(b) - 0.0).abs() < f32::EPSILON,
            "depth 0 should not spread to neighbours"
        );
    }

    #[test]
    fn test_find_alternative_paths() {
        let mut net = make_net();
        // Setup: "morning" → "alarm" (failed), "morning" → "gentle_music" (alternative)
        let morning = net.get_or_create_concept("morning", 100).expect("morning");
        let alarm = net.get_or_create_concept("alarm", 100).expect("alarm");
        let gentle = net
            .get_or_create_concept("gentle_music", 100)
            .expect("gentle");

        // Build associations
        for _ in 0..10 {
            net.strengthen_association(morning, alarm, 200).expect("ok");
            net.strengthen_association(morning, gentle, 200)
                .expect("ok");
        }

        // Make gentle_music have a good track record
        for _ in 0..5 {
            net.activate(gentle, Outcome::Success, 0.8, 200)
                .expect("ok");
        }

        let alts = net
            .find_alternative_paths(alarm, &[morning], 3, 300)
            .expect("alts");

        // Should find gentle_music as an alternative
        assert!(!alts.is_empty(), "should find at least one alternative");
        let found_gentle = alts.iter().any(|r| r.concept_id == gentle);
        assert!(found_gentle, "gentle_music should be among alternatives");
    }

    #[test]
    fn test_find_alternative_paths_excludes_failed() {
        let mut net = make_net();
        let ctx = net.get_or_create_concept("context", 100).expect("ctx");
        let failed = net.get_or_create_concept("failed", 100).expect("failed");

        for _ in 0..10 {
            net.strengthen_association(ctx, failed, 200).expect("ok");
        }

        let alts = net
            .find_alternative_paths(failed, &[ctx], 5, 300)
            .expect("alts");

        // Failed concept should not appear in alternatives
        assert!(
            !alts.iter().any(|r| r.concept_id == failed),
            "failed concept should be excluded from alternatives"
        );
    }

    #[test]
    fn test_get_action_recommendation_healthy() {
        let mut net = make_net();
        let id = net.get_or_create_concept("open_app", 100).expect("id");

        // Build a good track record
        for _ in 0..5 {
            net.activate(id, Outcome::Success, 0.8, 200).expect("ok");
        }

        let rec = net
            .get_action_recommendation(id, &[], 300)
            .expect("rec")
            .expect("should have recommendation");

        assert_eq!(rec.concept_id, id);
        assert_eq!(rec.source, RecommendationSource::Direct);
        assert!(rec.confidence > 0.0);
    }

    #[test]
    fn test_get_action_recommendation_not_found() {
        let net = make_net();
        let rec = net.get_action_recommendation(99999, &[], 100).expect("rec");
        assert!(rec.is_none());
    }

    #[test]
    fn test_record_outcome_success() {
        let mut net = make_net();
        let action = net.get_or_create_concept("tap_button", 100).expect("a");
        let ctx = net.get_or_create_concept("settings_app", 100).expect("c");

        net.record_outcome(action, Outcome::Success, &[ctx], 200)
            .expect("ok");

        // Should create association with SUCCESS_STRENGTHEN_FACTOR weight
        let assocs = net.get_associated(action, 0.0);
        assert_eq!(assocs.len(), 1);
        assert!(
            (assocs[0].1 - SUCCESS_STRENGTHEN_FACTOR).abs() < f32::EPSILON,
            "expected weight {}, got {}",
            SUCCESS_STRENGTHEN_FACTOR,
            assocs[0].1
        );
    }

    #[test]
    fn test_record_outcome_failure_weakens() {
        let mut net = make_net();
        let action = net.get_or_create_concept("tap_button", 100).expect("a");
        let ctx = net.get_or_create_concept("settings_app", 100).expect("c");

        // First establish a strong association
        for _ in 0..5 {
            net.strengthen_association(action, ctx, 100).expect("ok");
        }
        let before = net.get_associated(action, 0.0)[0].1;

        // Record failure
        net.record_outcome(action, Outcome::Failure, &[ctx], 200)
            .expect("ok");

        let after = net.get_associated(action, 0.0)[0].1;
        assert!(
            after < before,
            "failure should weaken: before={before}, after={after}"
        );
        let expected = before - FAILURE_WEAKEN_FACTOR;
        assert!(
            (after - expected.max(0.0)).abs() < f32::EPSILON,
            "expected {}, got {after}",
            expected.max(0.0)
        );
    }

    #[test]
    fn test_record_user_correction() {
        let mut net = make_net();
        let incorrect = net.get_or_create_concept("wrong_action", 100).expect("i");
        let correct = net.get_or_create_concept("right_action", 100).expect("c");
        let ctx = net.get_or_create_concept("context", 100).expect("ctx");

        // Build up associations
        for _ in 0..5 {
            net.strengthen_association(incorrect, ctx, 100).expect("ok");
        }

        net.record_user_correction(incorrect, correct, &[ctx], 200)
            .expect("correction");

        // Incorrect should have failure recorded
        let inc_c = net.get_concept(incorrect).expect("lookup");
        assert_eq!(inc_c.failure_count, 1);

        // Correct should have success recorded
        let cor_c = net.get_concept(correct).expect("lookup");
        assert_eq!(cor_c.success_count, 1);

        // Correct ↔ context should have a new/strengthened association
        let cor_assocs = net.get_associated(correct, 0.0);
        assert!(
            !cor_assocs.is_empty(),
            "correct concept should have association with context"
        );
    }

    #[test]
    fn test_record_user_correction_not_found() {
        let mut net = make_net();
        let valid = net.get_or_create_concept("valid", 100).expect("v");

        // Incorrect concept doesn't exist
        let result = net.record_user_correction(99999, valid, &[], 200);
        assert!(result.is_err());

        // Correct concept doesn't exist
        let result = net.record_user_correction(valid, 99999, &[], 200);
        assert!(result.is_err());
    }

    #[test]
    fn test_concept_id_for_name() {
        let name_id = HebbianNetwork::concept_id_for_name("music");
        let mut net = make_net();
        let created_id = net.get_or_create_concept("music", 100).expect("create");
        assert_eq!(name_id, created_id);
    }

    #[test]
    fn test_get_concept_mut() {
        let mut net = make_net();
        let id = net.get_or_create_concept("mutable", 100).expect("create");
        let concept = net.get_concept_mut(id).expect("mut ref");
        concept.importance = 0.99;
        assert!((net.get_concept(id).expect("lookup").importance - 0.99).abs() < f32::EPSILON);
    }

    #[test]
    fn test_concepts_iter() {
        let mut net = make_net();
        net.get_or_create_concept("alpha", 100).expect("a");
        net.get_or_create_concept("beta", 100).expect("b");
        let count = net.concepts_iter().count();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_recommendation_source_serde() {
        let sources = [
            RecommendationSource::Direct,
            RecommendationSource::SpreadingActivation,
            RecommendationSource::Alternative,
            RecommendationSource::UserCorrection,
        ];
        for src in &sources {
            let json = serde_json::to_string(src).expect("serialize");
            let back: RecommendationSource = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*src, back);
        }
    }

    #[test]
    fn test_action_recommendation_serde() {
        let rec = ActionRecommendation {
            concept_id: 42,
            concept_name: "test_action".to_string(),
            confidence: 0.85,
            path: vec![42],
            source: RecommendationSource::Direct,
        };
        let json = serde_json::to_string(&rec).expect("serialize");
        let back: ActionRecommendation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.concept_id, 42);
        assert_eq!(back.concept_name, "test_action");
        assert!((back.confidence - 0.85).abs() < f32::EPSILON);
    }
}
