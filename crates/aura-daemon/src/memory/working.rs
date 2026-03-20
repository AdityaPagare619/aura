//! Working memory — RAM ring buffer with O(1) push and activation-weighted retrieval.
//!
//! Budget: 1MB max, 1024 slots, <1ms latency.
//!
//! This is AURA's "scratchpad" — the most recent context window. It uses a
//! fixed-size ring buffer with importance-weighted eviction. When full, the
//! least important expired slot is evicted first; if none are expired, the
//! lowest-importance slot is evicted.
//!
//! ## Spreading Activation
//!
//! Each slot carries an activation score (0.0..1.0) that represents how
//! recently the slot was contextually relevant. When a query or external
//! event activates related slots (cosine similarity > 0.15), their activation
//! scores are boosted. Activations decay with a 60-second half-life.
//!
//! Retrieval scoring blends similarity, recency, and activation:
//! `similarity * 0.7 + recency * 0.1 + activation * 0.2`

use std::{cmp::Ordering, collections::BinaryHeap};

use aura_types::{events::EventSource, memory::WorkingSlot};

use crate::memory::{compaction::ContextCompactor, embeddings};

/// Maximum number of slots in working memory.
pub const MAX_SLOTS: usize = 1024;

/// Default TTL for working memory slots: 5 minutes (300_000 ms).
pub const DEFAULT_TTL_MS: u64 = 300_000;

/// Half-life for spreading activation decay: 60 seconds.
const ACTIVATION_HALF_LIFE_MS: f64 = 60_000.0;

/// Minimum cosine similarity to spread activation to a slot.
const ACTIVATION_SIMILARITY_THRESHOLD: f32 = 0.15;

/// A scored result from working memory query.
#[derive(Debug, Clone)]
pub struct WorkingResult {
    pub slot: WorkingSlot,
    pub index: usize,
    pub score: f32,
}

/// Ordered wrapper for BinaryHeap (min-heap by score for top-k selection).
struct ScoredSlot {
    index: usize,
    score: f32,
}

impl PartialEq for ScoredSlot {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score
    }
}

impl Eq for ScoredSlot {}

impl PartialOrd for ScoredSlot {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScoredSlot {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap behavior (lowest score at top)
        other
            .score
            .partial_cmp(&self.score)
            .unwrap_or(Ordering::Equal)
    }
}

// ---------------------------------------------------------------------------
// WorkingMemory
// ---------------------------------------------------------------------------

/// RAM-backed ring buffer for working memory with spreading activation.
///
/// Invariants:
/// - `slots.len() <= MAX_SLOTS`
/// - `activation_scores.len() == MAX_SLOTS` (parallel to `slots`)
/// - `head` always points to the next write position
/// - `count` tracks the number of live (non-None) slots
/// - Activation scores are in [0.0, 1.0]
pub struct WorkingMemory {
    slots: Vec<Option<WorkingSlot>>,
    /// Parallel activation scores for each slot (0.0 = dormant, 1.0 = fully active).
    activation_scores: Vec<f32>,
    /// Timestamp of the last activation decay pass.
    last_activation_ms: u64,
    head: usize,
    count: usize,
    /// Compactor for long sessions.
    compactor: ContextCompactor,
}

impl WorkingMemory {
    /// Create a new empty working memory.
    pub fn new() -> Self {
        let mut slots = Vec::with_capacity(MAX_SLOTS);
        slots.resize_with(MAX_SLOTS, || None);
        Self {
            slots,
            activation_scores: vec![0.0; MAX_SLOTS],
            last_activation_ms: 0,
            head: 0,
            count: 0,
            compactor: ContextCompactor::new(),
        }
    }

    /// Push a new slot into working memory. O(1) amortized.
    ///
    /// If memory is full, evicts the least valuable slot:
    /// 1. First try to evict any expired slot (lowest importance among expired)
    /// 2. If none expired, evict the slot with the lowest importance
    pub fn push(&mut self, content: String, source: EventSource, importance: f32, now_ms: u64) {
        self.push_with_ttl(content, source, importance, now_ms, DEFAULT_TTL_MS);
    }

    /// Push with a custom TTL.
    pub fn push_with_ttl(
        &mut self,
        content: String,
        source: EventSource,
        importance: f32,
        now_ms: u64,
        ttl_ms: u64,
    ) {
        let slot = WorkingSlot {
            content,
            source,
            importance,
            timestamp_ms: now_ms,
            ttl_ms,
        };

        // If getting full (>= 90% capacity), try to compact older events.
        if self.count >= (MAX_SLOTS * 9) / 10 {
            self.compact_old_events(now_ms);
        }

        if self.count < MAX_SLOTS {
            // Below capacity — use an empty position directly.
            // Expired slots are left in place for sweep_expired() to handle,
            // ensuring consolidation passes get accurate sweep counts.
            let pos = self.find_empty_slot();
            self.slots[pos] = Some(slot);
            self.activation_scores[pos] = 0.0;
            self.count += 1;
        } else {
            // Buffer full — evict the least valuable slot.
            // find_eviction_target prefers expired slots over live ones.
            let evict_idx = self.find_eviction_target(now_ms);
            self.slots[evict_idx] = Some(slot);
            self.activation_scores[evict_idx] = 0.0;
            // count stays the same (replace, not add)
        }

        // Advance head for round-robin tracking
        self.head = (self.head + 1) % MAX_SLOTS;
    }

    /// Try to compact older working slots to free up space.
    fn compact_old_events(&mut self, now_ms: u64) {
        let live: Vec<(usize, &WorkingSlot)> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|slot| (i, slot)))
            .collect();

        // Keep at least the 100 most recent slots intact.
        let candidates = self.compactor.identify_compaction_candidates(&live, 100);

        if candidates.len() < 3 {
            return;
        }

        let mut slots_to_compact = Vec::with_capacity(candidates.len());
        for &idx in &candidates {
            if let Some(s) = &self.slots[idx] {
                slots_to_compact.push(s.clone());
            }
        }

        let refs: Vec<&WorkingSlot> = slots_to_compact.iter().collect();
        let compacted_slot = self.compactor.compact(&refs, now_ms);

        for &idx in &candidates {
            self.slots[idx] = None;
            self.activation_scores[idx] = 0.0;
            self.count -= 1;
        }

        let pos = self.find_empty_slot();
        self.slots[pos] = Some(compacted_slot);
        self.activation_scores[pos] = 0.0;
        self.count += 1;

        tracing::debug!(
            compacted_count = candidates.len(),
            new_count = self.count,
            "Working memory compacted"
        );
    }

    /// Query working memory for the top-k most relevant slots.
    ///
    /// Scoring: `similarity * 0.7 + recency * 0.1 + activation * 0.2`
    /// where recency = 1.0 - (age_ms / max_age_ms), clamped to [0, 1].
    pub fn query(&self, query_text: &str, max_results: usize, now_ms: u64) -> Vec<WorkingResult> {
        if self.count == 0 || max_results == 0 {
            return Vec::new();
        }

        let query_embedding = embeddings::embed(query_text);
        let mut heap: BinaryHeap<ScoredSlot> = BinaryHeap::with_capacity(max_results + 1);

        // Find the oldest timestamp for recency normalization
        let mut min_ts = u64::MAX;
        let mut max_ts = 0u64;
        for slot in self.slots.iter().flatten() {
            min_ts = min_ts.min(slot.timestamp_ms);
            max_ts = max_ts.max(slot.timestamp_ms);
        }
        let time_range = if max_ts > min_ts {
            (max_ts - min_ts) as f64
        } else {
            1.0 // avoid division by zero
        };

        for (idx, slot_opt) in self.slots.iter().enumerate() {
            let slot = match slot_opt {
                Some(s) => s,
                None => continue,
            };

            // Skip expired slots
            if slot.is_expired(now_ms) {
                continue;
            }

            // Compute similarity
            let slot_embedding = embeddings::embed(&slot.content);
            let similarity = embeddings::cosine_similarity(&query_embedding, &slot_embedding);

            // Compute recency (0 = oldest, 1 = newest)
            let recency = if time_range > 0.0 {
                ((slot.timestamp_ms - min_ts) as f64 / time_range) as f32
            } else {
                1.0
            };

            let score = similarity * 0.7 + recency * 0.1 + self.activation_scores[idx] * 0.2;

            heap.push(ScoredSlot { index: idx, score });
            if heap.len() > max_results {
                heap.pop(); // Remove lowest score
            }
        }

        // Extract results sorted by score descending.
        // `into_sorted_vec()` returns ascending by our reversed `Ord`,
        // which means highest real score first — exactly what we want.
        let results: Vec<WorkingResult> = heap
            .into_sorted_vec()
            .into_iter()
            .filter_map(|scored| {
                self.slots[scored.index].as_ref().map(|slot| WorkingResult {
                    slot: slot.clone(),
                    index: scored.index,
                    score: scored.score,
                })
            })
            .collect();

        results
    }

    /// Remove all expired slots and return the count of slots removed.
    pub fn sweep_expired(&mut self, now_ms: u64) -> usize {
        let mut removed = 0;
        for (idx, slot_opt) in self.slots.iter_mut().enumerate() {
            if let Some(slot) = slot_opt {
                if slot.is_expired(now_ms) {
                    *slot_opt = None;
                    self.activation_scores[idx] = 0.0;
                    self.count -= 1;
                    removed += 1;
                }
            }
        }
        removed
    }

    /// Get the number of live (non-expired, non-None) slots.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Check if working memory is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Get the total number of slots (capacity).
    pub fn capacity(&self) -> usize {
        MAX_SLOTS
    }

    /// Estimate memory usage in bytes.
    ///
    /// Rough estimate: slot overhead + average content size.
    pub fn memory_usage_bytes(&self) -> usize {
        let base = std::mem::size_of::<Self>();
        let slot_overhead = MAX_SLOTS * std::mem::size_of::<Option<WorkingSlot>>();
        let content_bytes: usize = self
            .slots
            .iter()
            .filter_map(|s| s.as_ref())
            .map(|s| s.content.len())
            .sum();
        base + slot_overhead + content_bytes
    }

    /// Get a snapshot of all live slots (for consolidation).
    pub fn snapshot(&self, now_ms: u64) -> Vec<(usize, WorkingSlot)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(idx, slot_opt)| {
                slot_opt.as_ref().and_then(|slot| {
                    if !slot.is_expired(now_ms) {
                        Some((idx, slot.clone()))
                    } else {
                        None
                    }
                })
            })
            .collect()
    }

    /// Remove a specific slot by index. Used after promoting to episodic.
    pub fn remove(&mut self, index: usize) -> Option<WorkingSlot> {
        if index < MAX_SLOTS {
            if let Some(slot) = self.slots[index].take() {
                self.activation_scores[index] = 0.0;
                self.count -= 1;
                return Some(slot);
            }
        }
        None
    }

    /// Get the slot with the highest importance (for emergency context).
    pub fn most_important(&self, now_ms: u64) -> Option<&WorkingSlot> {
        self.slots
            .iter()
            .filter_map(|s| s.as_ref())
            .filter(|s| !s.is_expired(now_ms))
            .max_by(|a, b| {
                a.importance
                    .partial_cmp(&b.importance)
                    .unwrap_or(Ordering::Equal)
            })
    }

    /// Build context string for LLM prompt injection.
    ///
    /// Returns the top `max_items` memories ranked by RRF (Reciprocal Rank
    /// Fusion) combining importance, recency, and semantic relevance to the
    /// query. Falls back to importance-only sorting when no query is provided.
    /// See: AURA-V4-BATCH7-MEMORY-INFERENCE-AUDIT §1.4.
    /// Returns the top-ranked working memory slot contents for LLM context assembly.
    ///
    /// Returns raw content strings in relevance order. Formatting (labels, numbering,
    /// section headers) is the responsibility of the neocortex/LLM context assembly layer.
    pub fn context_for_llm(&self, query: &str, max_items: usize, now_ms: u64) -> Vec<String> {
        let live: Vec<(usize, &WorkingSlot)> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|slot| (i, slot)))
            .filter(|(_, s)| !s.is_expired(now_ms))
            .collect();

        if live.is_empty() {
            return vec![];
        }

        // Build per-slot scores via RRF over three rankings.
        let query_emb = if !query.is_empty() {
            Some(embeddings::embed(query))
        } else {
            None
        };

        // 1. Importance rank (descending).
        let mut by_importance: Vec<(usize, f32)> =
            live.iter().map(|(i, s)| (*i, s.importance)).collect();
        by_importance.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));

        // 2. Recency rank (most recent first).
        let mut by_recency: Vec<(usize, u64)> =
            live.iter().map(|(i, s)| (*i, s.timestamp_ms)).collect();
        by_recency.sort_by_key(|item| std::cmp::Reverse(item.1));

        // 3. Relevance rank (highest cosine similarity first).
        let by_relevance: Vec<(usize, f32)> = match &query_emb {
            Some(qe) => {
                let mut ranked: Vec<(usize, f32)> = live
                    .iter()
                    .map(|(i, s)| {
                        let slot_emb = embeddings::embed(&s.content);
                        let sim = embeddings::cosine_similarity(qe, &slot_emb);
                        (*i, sim)
                    })
                    .collect();
                ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
                ranked
            },
            None => by_importance.clone(), // fallback: use importance as relevance
        };

        // RRF: score = sum(1 / (k + rank)) across all rankings, k = 60.
        let k = 60.0_f32;
        let mut rrf_scores: Vec<(usize, f32)> = Vec::with_capacity(live.len());
        for (idx, _) in &live {
            let rank_imp = by_importance
                .iter()
                .position(|(i, _)| i == idx)
                .unwrap_or(live.len()) as f32;
            let rank_rec = by_recency
                .iter()
                .position(|(i, _)| i == idx)
                .unwrap_or(live.len()) as f32;
            let rank_rel = by_relevance
                .iter()
                .position(|(i, _)| i == idx)
                .unwrap_or(live.len()) as f32;
            let score = 1.0 / (k + rank_imp) + 1.0 / (k + rank_rec) + 1.0 / (k + rank_rel);
            rrf_scores.push((*idx, score));
        }
        rrf_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
        rrf_scores.truncate(max_items);

        let mut items: Vec<String> = Vec::with_capacity(rrf_scores.len());
        for (slot_idx, _score) in &rrf_scores {
            if let Some(Some(slot)) = self.slots.get(*slot_idx) {
                items.push(slot.content.clone());
            }
        }
        items
    }

    // -----------------------------------------------------------------------
    // Spreading Activation
    // -----------------------------------------------------------------------

    /// Spread activation to slots related to the given query text.
    ///
    /// For each live, non-expired slot, compute cosine similarity to `query_text`.
    /// If similarity exceeds the threshold (0.7), boost the slot's activation by
    /// `similarity * 0.5` (clamped to 1.0). Applies decay first.
    pub fn activate_related(&mut self, query_text: &str, now_ms: u64) {
        self.decay_activations(now_ms);

        let query_embedding = embeddings::embed(query_text);

        for (idx, slot_opt) in self.slots.iter().enumerate() {
            let slot = match slot_opt {
                Some(s) if !s.is_expired(now_ms) => s,
                _ => continue,
            };

            let slot_embedding = embeddings::embed(&slot.content);
            let sim = embeddings::cosine_similarity(&query_embedding, &slot_embedding);

            if sim >= ACTIVATION_SIMILARITY_THRESHOLD {
                let boost = sim * 0.5;
                self.activation_scores[idx] = (self.activation_scores[idx] + boost).min(1.0);
            }
        }
    }

    /// Decay all activation scores based on elapsed time.
    ///
    /// Uses exponential decay with a 60-second half-life:
    /// `score *= 0.5^(elapsed_ms / 60_000)`
    pub fn decay_activations(&mut self, now_ms: u64) {
        if self.last_activation_ms == 0 || now_ms <= self.last_activation_ms {
            self.last_activation_ms = now_ms;
            return;
        }

        let elapsed = (now_ms - self.last_activation_ms) as f64;
        let decay_factor = (0.5_f64).powf(elapsed / ACTIVATION_HALF_LIFE_MS) as f32;

        for score in self.activation_scores.iter_mut() {
            *score *= decay_factor;
            // Clamp very small values to zero to avoid floating point dust
            if *score < 1e-6 {
                *score = 0.0;
            }
        }

        self.last_activation_ms = now_ms;
    }

    /// Get the activation score for a slot by index.
    pub fn activation(&self, index: usize) -> f32 {
        if index < MAX_SLOTS {
            self.activation_scores[index]
        } else {
            0.0
        }
    }

    /// Get a snapshot of all non-zero activation scores (index, score).
    pub fn active_slots(&self) -> Vec<(usize, f32)> {
        self.activation_scores
            .iter()
            .enumerate()
            .filter(|(_, &score)| score > 1e-6)
            .map(|(idx, &score)| (idx, score))
            .collect()
    }

    /// Get all live slots with their content (for GDPR export).
    pub fn get_all_slots(&self) -> Vec<&WorkingSlot> {
        self.slots.iter().filter_map(|s| s.as_ref()).collect()
    }

    /// Clear all working memory slots (for GDPR erasure).
    pub fn clear(&mut self) {
        for slot in self.slots.iter_mut() {
            *slot = None;
        }
        for score in self.activation_scores.iter_mut() {
            *score = 0.0;
        }
        self.count = 0;
        self.head = 0;
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Find any single expired slot (lowest importance among expired).
    #[allow(dead_code)]
    fn find_one_expired(&self, now_ms: u64) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (idx, slot_opt) in self.slots.iter().enumerate() {
            if let Some(slot) = slot_opt {
                if slot.is_expired(now_ms) {
                    match best {
                        None => best = Some((idx, slot.importance)),
                        Some((_, imp)) if slot.importance < imp => {
                            best = Some((idx, slot.importance));
                        },
                        _ => {},
                    }
                }
            }
        }
        best.map(|(idx, _)| idx)
    }

    /// Find the next empty slot position.
    fn find_empty_slot(&self) -> usize {
        // Start from head and search forward
        for i in 0..MAX_SLOTS {
            let idx = (self.head + i) % MAX_SLOTS;
            if self.slots[idx].is_none() {
                return idx;
            }
        }
        // Should not reach here if count < MAX_SLOTS
        unreachable!("find_empty_slot called but no empty slots found");
    }

    /// Find the best eviction target.
    ///
    /// Priority: expired slots (lowest importance first), then lowest importance.
    fn find_eviction_target(&self, now_ms: u64) -> usize {
        let mut best_expired: Option<(usize, f32)> = None;
        let mut best_overall: Option<(usize, f32)> = None;

        for (idx, slot_opt) in self.slots.iter().enumerate() {
            if let Some(slot) = slot_opt {
                // Track best expired candidate
                if slot.is_expired(now_ms) {
                    match best_expired {
                        None => best_expired = Some((idx, slot.importance)),
                        Some((_, imp)) if slot.importance < imp => {
                            best_expired = Some((idx, slot.importance));
                        },
                        _ => {},
                    }
                }

                // Track overall lowest importance
                match best_overall {
                    None => best_overall = Some((idx, slot.importance)),
                    Some((_, imp)) if slot.importance < imp => {
                        best_overall = Some((idx, slot.importance));
                    },
                    _ => {},
                }
            }
        }

        // Prefer evicting expired, fall back to lowest importance
        best_expired
            .or(best_overall)
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }
}

impl Default for WorkingMemory {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use aura_types::events::EventSource;

    use super::*;

    fn now() -> u64 {
        1_700_000_000_000
    }

    #[test]
    fn test_new_working_memory() {
        let wm = WorkingMemory::new();
        assert_eq!(wm.len(), 0);
        assert!(wm.is_empty());
        assert_eq!(wm.capacity(), MAX_SLOTS);
    }

    #[test]
    fn test_push_single() {
        let mut wm = WorkingMemory::new();
        wm.push("hello world".into(), EventSource::UserCommand, 0.8, now());
        assert_eq!(wm.len(), 1);
        assert!(!wm.is_empty());
    }

    #[test]
    fn test_push_multiple() {
        let mut wm = WorkingMemory::new();
        for i in 0..10 {
            wm.push(
                format!("item {}", i),
                EventSource::Notification,
                0.5,
                now() + i * 1000,
            );
        }
        assert_eq!(wm.len(), 10);
    }

    #[test]
    fn test_push_beyond_capacity_evicts() {
        let mut wm = WorkingMemory::new();
        for i in 0..MAX_SLOTS + 5 {
            wm.push(
                format!("item {}", i),
                EventSource::Notification,
                i as f32 / MAX_SLOTS as f32, // increasing importance
                now() + i as u64 * 100,
            );
        }
        // Must never exceed MAX_SLOTS.  With compaction active (fires at 90%
        // capacity), the actual count may be well below MAX_SLOTS as batches
        // of older slots are merged; the invariant is strictly ≤ MAX_SLOTS.
        assert!(
            wm.len() <= MAX_SLOTS,
            "len {} must not exceed MAX_SLOTS {}",
            wm.len(),
            MAX_SLOTS
        );
        assert!(wm.len() > 0, "should have at least one entry");
    }

    #[test]
    fn test_eviction_prefers_expired() {
        let mut wm = WorkingMemory::new();
        // Fill with slots that have very short TTL
        for i in 0..5 {
            wm.push_with_ttl(
                format!("short-lived {}", i),
                EventSource::Internal,
                1.0, // high importance but expired
                now(),
                100, // 100ms TTL
            );
        }
        // Add one that won't be expired
        wm.push_with_ttl(
            "long-lived".into(),
            EventSource::UserCommand,
            0.1, // low importance but not expired
            now(),
            1_000_000, // long TTL
        );

        // Now much later, push more — with spare capacity, an empty slot is used
        // (expired slots are left for sweep_expired to handle properly).
        let later = now() + 200; // 200ms later, short-lived are expired
        wm.push("new item".into(), EventSource::UserCommand, 0.5, later);
        assert_eq!(wm.len(), 7); // 7 because empty slot was used, not an expired one

        // Now sweep — the 5 expired should be reclaimed
        let swept = wm.sweep_expired(later);
        assert_eq!(swept, 5);
        assert_eq!(wm.len(), 2); // "long-lived" + "new item"
    }

    #[test]
    fn test_sweep_expired() {
        let mut wm = WorkingMemory::new();
        wm.push_with_ttl("ephemeral".into(), EventSource::Internal, 0.5, now(), 100);
        wm.push_with_ttl(
            "durable".into(),
            EventSource::UserCommand,
            0.8,
            now(),
            1_000_000,
        );

        assert_eq!(wm.len(), 2);

        let removed = wm.sweep_expired(now() + 200); // 200ms later
        assert_eq!(removed, 1);
        assert_eq!(wm.len(), 1);
    }

    #[test]
    fn test_query_returns_relevant() {
        let mut wm = WorkingMemory::new();
        let t = now();
        // All pushed at the same time to eliminate recency bias.
        wm.push(
            "weather forecast sunny warm temperature climate".into(),
            EventSource::UserCommand,
            0.5,
            t,
        );
        wm.push(
            "user prefers dark mode".into(),
            EventSource::UserCommand,
            0.5,
            t,
        );
        wm.push(
            "meeting at 3pm with Alice".into(),
            EventSource::Notification,
            0.5,
            t,
        );

        let results = wm.query("weather forecast temperature", 2, t + 100);
        assert!(!results.is_empty());
        // The weather slot should be the most relevant
        assert!(
            results[0].slot.content.contains("weather"),
            "expected weather result first, got: {}",
            results[0].slot.content
        );
    }

    #[test]
    fn test_query_respects_max_results() {
        let mut wm = WorkingMemory::new();
        for i in 0..10 {
            wm.push(
                format!("item {}", i),
                EventSource::Internal,
                0.5,
                now() + i * 1000,
            );
        }
        let results = wm.query("item", 3, now() + 20_000);
        assert!(results.len() <= 3);
    }

    #[test]
    fn test_query_skips_expired() {
        let mut wm = WorkingMemory::new();
        wm.push_with_ttl("expired item".into(), EventSource::Internal, 1.0, now(), 50);
        wm.push_with_ttl(
            "live item".into(),
            EventSource::UserCommand,
            0.5,
            now(),
            1_000_000,
        );

        let results = wm.query("item", 10, now() + 100);
        assert_eq!(results.len(), 1);
        assert!(results[0].slot.content.contains("live"));
    }

    #[test]
    fn test_query_empty_memory() {
        let wm = WorkingMemory::new();
        let results = wm.query("anything", 5, now());
        assert!(results.is_empty());
    }

    #[test]
    fn test_remove() {
        let mut wm = WorkingMemory::new();
        wm.push("removable".into(), EventSource::Internal, 0.5, now());

        // Find the slot index
        let snapshot = wm.snapshot(now());
        assert_eq!(snapshot.len(), 1);
        let idx = snapshot[0].0;

        let removed = wm.remove(idx);
        assert!(removed.is_some());
        assert_eq!(wm.len(), 0);
    }

    #[test]
    fn test_most_important() {
        let mut wm = WorkingMemory::new();
        wm.push("low".into(), EventSource::Internal, 0.1, now());
        wm.push("high".into(), EventSource::UserCommand, 0.9, now());
        wm.push("mid".into(), EventSource::Notification, 0.5, now());

        let best = wm.most_important(now()).unwrap();
        assert!((best.importance - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_context_for_llm() {
        let mut wm = WorkingMemory::new();
        wm.push(
            "important fact".into(),
            EventSource::UserCommand,
            0.9,
            now(),
        );
        wm.push("less important".into(), EventSource::Internal, 0.3, now());
        wm.push("medium fact".into(), EventSource::Notification, 0.6, now());

        let ctx = wm.context_for_llm("important", 2, now());
        // Returns Vec<String> of raw content — no formatting headers
        assert_eq!(ctx.len(), 2);
        assert!(ctx.iter().any(|s| s == "important fact"));
    }

    #[test]
    fn test_context_for_llm_empty() {
        let wm = WorkingMemory::new();
        let ctx = wm.context_for_llm("", 5, now());
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_snapshot() {
        let mut wm = WorkingMemory::new();
        wm.push("a".into(), EventSource::Internal, 0.5, now());
        wm.push("b".into(), EventSource::Internal, 0.5, now());
        wm.push_with_ttl("expired".into(), EventSource::Internal, 0.5, now(), 50);

        let snap = wm.snapshot(now() + 100);
        assert_eq!(snap.len(), 2); // expired one excluded
    }

    #[test]
    fn test_memory_usage() {
        let mut wm = WorkingMemory::new();
        let base = wm.memory_usage_bytes();
        wm.push("test content".into(), EventSource::Internal, 0.5, now());
        let after = wm.memory_usage_bytes();
        assert!(after > base);
    }

    #[test]
    fn test_default_trait() {
        let wm = WorkingMemory::default();
        assert_eq!(wm.len(), 0);
        assert_eq!(wm.capacity(), MAX_SLOTS);
    }

    // -- Spreading Activation tests --

    #[test]
    fn test_activation_starts_at_zero() {
        let mut wm = WorkingMemory::new();
        wm.push("hello world".into(), EventSource::UserCommand, 0.5, now());
        let snap = wm.snapshot(now());
        assert_eq!(snap.len(), 1);
        assert!((wm.activation(snap[0].0) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_activate_related_boosts_similar() {
        let mut wm = WorkingMemory::new();
        let t = now();
        wm.push(
            "the weather forecast for today is sunny and warm".into(),
            EventSource::UserCommand,
            0.5,
            t,
        );
        wm.push(
            "user prefers dark mode in the editor settings".into(),
            EventSource::UserCommand,
            0.5,
            t + 100,
        );

        // Activate with a weather-related query
        wm.activate_related("weather forecast sunny warm temperature", t + 200);

        let snap = wm.snapshot(t + 200);
        let weather_idx = snap
            .iter()
            .find(|(_, s)| s.content.contains("weather"))
            .unwrap()
            .0;
        let dark_idx = snap
            .iter()
            .find(|(_, s)| s.content.contains("dark"))
            .unwrap()
            .0;

        // Weather slot should have higher activation than dark mode slot
        assert!(
            wm.activation(weather_idx) > wm.activation(dark_idx),
            "weather activation {} should be > dark activation {}",
            wm.activation(weather_idx),
            wm.activation(dark_idx),
        );
    }

    #[test]
    fn test_activation_clamped_to_one() {
        let mut wm = WorkingMemory::new();
        let t = now();
        wm.push(
            "rust programming language ownership borrowing".into(),
            EventSource::UserCommand,
            0.5,
            t,
        );

        // Activate many times to try to exceed 1.0
        for i in 0..20 {
            wm.activate_related("rust programming language ownership borrowing", t + i * 100);
        }

        let snap = wm.snapshot(t + 2000);
        let idx = snap[0].0;
        assert!(
            wm.activation(idx) <= 1.0,
            "activation {} should be <= 1.0",
            wm.activation(idx)
        );
    }

    #[test]
    fn test_decay_halves_after_half_life() {
        let mut wm = WorkingMemory::new();
        let t = now();
        wm.push(
            "rust programming language".into(),
            EventSource::UserCommand,
            0.5,
            t,
        );

        // Activate to get a known score
        wm.activate_related("rust programming language", t);
        let snap = wm.snapshot(t);
        let idx = snap[0].0;
        let initial = wm.activation(idx);
        assert!(initial > 0.0, "should have been activated");

        // Decay after one half-life (60 seconds)
        wm.decay_activations(t + 60_000);
        let after_one_hl = wm.activation(idx);
        let expected = initial * 0.5;
        assert!(
            (after_one_hl - expected).abs() < 0.01,
            "after 1 half-life: got {}, expected ~{}",
            after_one_hl,
            expected,
        );
    }

    #[test]
    fn test_decay_clears_to_zero_after_long_time() {
        let mut wm = WorkingMemory::new();
        let t = now();
        // Use a long TTL so the slot survives past the 20-minute decay window.
        wm.push_with_ttl(
            "something memorable".into(),
            EventSource::UserCommand,
            0.5,
            t,
            2_000_000, // ~33 minutes
        );
        wm.activate_related("something memorable", t);

        // 20 minutes later — 20 half-lives, decay factor = 0.5^20 ≈ 9.5e-7
        wm.decay_activations(t + 1_200_000);
        let snap = wm.snapshot(t + 1_200_000);
        assert!(
            !snap.is_empty(),
            "slot should still be alive with extended TTL"
        );
        let idx = snap[0].0;
        assert!(
            wm.activation(idx) < 1e-5,
            "should be effectively zero after 20 min, got {}",
            wm.activation(idx)
        );
    }

    #[test]
    fn test_query_scores_include_activation() {
        let mut wm = WorkingMemory::new();
        let t = now();

        // Two slots with identical content & timing — only activation differs
        wm.push(
            "rust programming concepts and patterns".into(),
            EventSource::UserCommand,
            0.5,
            t,
        );
        wm.push(
            "rust development patterns and idioms".into(),
            EventSource::UserCommand,
            0.5,
            t,
        );

        // Activate only the first one
        let snap = wm.snapshot(t);
        let idx0 = snap[0].0;
        // Manually set activation on slot 0 only
        wm.activation_scores[idx0] = 0.8;

        let results = wm.query("rust programming patterns", 2, t + 100);
        assert_eq!(results.len(), 2);

        // The activated slot should score higher due to 0.3 activation weight
        let activated_score = results
            .iter()
            .find(|r| r.index == idx0)
            .map(|r| r.score)
            .unwrap_or(0.0);
        let other_score = results
            .iter()
            .find(|r| r.index != idx0)
            .map(|r| r.score)
            .unwrap_or(0.0);
        assert!(
            activated_score > other_score,
            "activated slot score {} should be > {}",
            activated_score,
            other_score,
        );
    }

    #[test]
    fn test_active_slots_returns_nonzero() {
        let mut wm = WorkingMemory::new();
        let t = now();
        wm.push("a".into(), EventSource::Internal, 0.5, t);
        wm.push("b".into(), EventSource::Internal, 0.5, t);

        assert!(wm.active_slots().is_empty());

        // Set one activation manually
        wm.activation_scores[0] = 0.5;
        let active = wm.active_slots();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].0, 0);
    }

    #[test]
    fn test_remove_resets_activation() {
        let mut wm = WorkingMemory::new();
        let t = now();
        wm.push(
            "removable slot content".into(),
            EventSource::Internal,
            0.5,
            t,
        );
        let snap = wm.snapshot(t);
        let idx = snap[0].0;
        wm.activation_scores[idx] = 0.9;

        wm.remove(idx);
        assert!((wm.activation(idx) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sweep_resets_activation() {
        let mut wm = WorkingMemory::new();
        let t = now();
        wm.push_with_ttl("ephemeral".into(), EventSource::Internal, 0.5, t, 100);
        let snap = wm.snapshot(t);
        let idx = snap[0].0;
        wm.activation_scores[idx] = 0.8;

        wm.sweep_expired(t + 200);
        assert!((wm.activation(idx) - 0.0).abs() < f32::EPSILON);
    }
}
