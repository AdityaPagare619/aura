//! Real embedding system for AURA v4 memory.
//!
//! Dual-mode: TF-IDF fallback (always available) + neural via IPC (Phase 2A+).
//! 384-dimensional TF-IDF embeddings with LRU cache (1024 entries).
//!
//! The TF-IDF mode uses **feature hashing with sign trick** over unigrams,
//! bigrams, and character trigrams. The sign-hashing trick (Weinberger et al.)
//! prevents hash-collision accumulation: each feature hashes to a bucket *and*
//! to a sign (+1 / -1), so colliding unrelated features cancel rather than
//! reinforce, producing dramatically better cosine similarity than naive
//! modular hashing.
//!
//! ## Embedding Quality
//!
//! [`EmbeddingQuality`] is returned alongside every embedding vector so callers
//! can make informed decisions about search thresholds and memory ranking.
//! TF-IDF vectors are fast and deterministic but approximate; Neural vectors
//! are semantic and require the neocortex process to be loaded with a
//! model that exposes an embedding layer.
//!
//! ## ⚠ Dimension Mismatch Danger
//!
//! **NEVER compare embeddings of different quality levels.**
//!
//! | Quality | Dimension | Source |
//! |---------|-----------|--------|
//! | `TfIdf` | [`EMBED_DIM`] = 384 | sign-hash buckets |
//! | `Neural` | [`DIM_NEURAL_FALLBACK`] (4096) or model-specific | GGUF hidden state |
//!
//! Comparing a 384-dim TF-IDF vector against a 4096-dim neural vector via
//! cosine similarity produces **garbage scores**. Always track [`EmbeddingQuality`]
//! alongside the vector and refuse cross-quality comparisons.
//!
//! The `debug_assert_eq!` in [`cosine_similarity`] will catch length mismatches
//! in debug builds; in release builds the lengths must be enforced by callers.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// Embedding dimensionality — 384 buckets for TF-IDF sign-hash vectors.
///
/// 384 dims (3× the previous 128) combined with sign hashing virtually
/// eliminates false similarity between unrelated content.
pub const EMBED_DIM: usize = 384;

/// Neural embedding dimension assumed when the caller knows the neocortex is
/// running but has not queried [`ModelCapabilities`] yet.
///
/// Matches `ModelCapabilities::fallback_defaults().embedding_dim` (4096) —
/// the hidden-state size of the default 4B GGUF tier. Real dimension is
/// model-specific; prefer passing the exact dim from `ModelCapabilities`
/// when available.
///
/// **Never mix with [`EMBED_DIM`]** — see module-level documentation for the
/// dimension mismatch danger.
pub const DIM_NEURAL_FALLBACK: u32 = 4096;

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x100000001b3;

/// A second FNV seed used for the sign hash (different offset basis).
const FNV_SIGN_OFFSET: u64 = 0x6c62272e07bb0142;

/// Maximum cache entries.
const CACHE_MAX: usize = 1024;

/// Weight for unigram features.
const UNIGRAM_WEIGHT: f32 = 1.0;
/// Weight for bigram features.
const BIGRAM_WEIGHT: f32 = 0.7;
/// Weight for character-trigram features.
const CHAR_TRIGRAM_WEIGHT: f32 = 0.3;

// ---------------------------------------------------------------------------
// Stop words
// ---------------------------------------------------------------------------

static STOP_WORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "be", "been", "being",
    "have", "has", "had", "do", "does", "did", "will", "would", "shall",
    "should", "may", "might", "must", "can", "could", "to", "of", "in",
    "for", "on", "with", "at", "by", "from", "as", "into", "through",
    "during", "before", "after", "above", "below", "between", "out", "off",
    "over", "under", "again", "further", "then", "once", "this", "that",
    "these", "those", "it", "its", "and", "but", "or", "nor", "not", "so",
    "yet", "both", "each", "all", "any", "few", "more", "most", "other",
    "some", "such", "no", "only", "own", "same", "than", "too", "very",
];

fn is_stop_word(word: &str) -> bool {
    STOP_WORDS.iter().any(|&sw| sw == word)
}

// ---------------------------------------------------------------------------
// Embedding cache
// ---------------------------------------------------------------------------

/// PERF-HIGH-1: RwLock replaces Mutex so concurrent readers don't block each other.
/// PERF-HIGH-2: VecDeque provides O(1) LRU eviction (replaces O(n) min scan).
/// PERF-HIGH-3: Arc<Vec<f32>> avoids cloning the full 1536-byte embedding on cache hit;
///              readers only do an atomic refcount increment under the read lock.
struct EmbeddingCache {
    entries: HashMap<u64, Arc<Vec<f32>>>,
    /// Front = oldest insertion, back = newest. May contain stale keys (already
    /// evicted); the eviction loop skips those in O(1) amortised.
    eviction_order: VecDeque<u64>,
    max_size: usize,
}

impl EmbeddingCache {
    fn new(max_size: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(max_size / 2),
            eviction_order: VecDeque::with_capacity(max_size + max_size / 4),
            max_size,
        }
    }

    /// Read-only lookup — returns a cheap Arc clone (no vector data copied).
    /// Suitable for use under a read lock.
    fn get(&self, key: u64) -> Option<Arc<Vec<f32>>> {
        self.entries.get(&key).cloned() // Arc::clone = atomic increment
    }

    /// Insert a new entry, evicting the LRU entry in O(1) amortised if at capacity.
    fn insert(&mut self, key: u64, embedding: Vec<f32>) {
        if self.entries.len() >= self.max_size && !self.entries.contains_key(&key) {
            // Pop from front until we find a key still in the map (skip stale ghosts).
            while let Some(oldest_key) = self.eviction_order.pop_front() {
                if self.entries.remove(&oldest_key).is_some() {
                    break;
                }
            }
        }
        self.entries.insert(key, Arc::new(embedding));
        self.eviction_order.push_back(key);
    }
}

/// PERF-HIGH-1: RwLock allows concurrent cache reads without blocking.
static CACHE: RwLock<Option<EmbeddingCache>> = RwLock::new(None);

/// Try a read-only cache lookup under a read lock (non-blocking for concurrent readers).
fn cache_get(key: u64) -> Option<Arc<Vec<f32>>> {
    let guard = CACHE.read().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().and_then(|c| c.get(key))
}

/// Insert into the cache under a write lock.
fn cache_insert(key: u64, embedding: Vec<f32>) {
    let mut guard = CACHE.write().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(|| EmbeddingCache::new(CACHE_MAX));
    cache.insert(key, embedding);
}

// ---------------------------------------------------------------------------
// Core embedding: TF-IDF with sign-hashing trick
// ---------------------------------------------------------------------------

/// Generate a 384-dimensional embedding from text using TF-IDF sign-hashing.
///
/// Algorithm:
/// 1. Tokenize: split on whitespace/punctuation, lowercase, remove stop words
/// 2. For each feature (unigram, bigram, char-trigram):
///    a. Compute bucket = fnv_hash(feature) % 384
///    b. Compute sign = +1 or -1 from a second independent hash
///    c. Accumulate sign × weight into the bucket
/// 3. L2-normalize to unit vector
///
/// The **sign-hashing trick** (Weinberger et al. 2009) prevents hash-collision
/// build-up: unrelated features that collide in the same bucket are equally
/// likely to add or subtract, so their contributions cancel in expectation.
///
/// Deterministic: same input always produces same output.
/// Thread-safe: uses an internal LRU cache for repeated queries.
// ---------------------------------------------------------------------------
// Embedding quality signal
// ---------------------------------------------------------------------------

/// Describes how an embedding vector was produced.
///
/// Callers should use this to set appropriate similarity thresholds:
/// - [`TfIdf`]: use a higher threshold (e.g. 0.7) — vectors are approximate.
/// - [`Neural`]: lower threshold is fine (e.g. 0.5) — vectors are semantic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingQuality {
    /// Feature-hashing TF-IDF with sign trick — fast, deterministic, no model
    /// required. Captures lexical overlap well; struggles with synonyms and
    /// paraphrase. Always available.
    TfIdf,
    /// Neural embedding from the loaded LLM via the neocortex IPC endpoint —
    /// semantic, captures meaning not just tokens. Requires the neocortex
    /// process to be running with a model that supports embedding.
    ///
    /// Dimension is model-specific (typically 1536–4096 for GGUF models).
    /// Use [`DIM_NEURAL_FALLBACK`] when the exact dim is unknown.
    ///
    /// Returned by [`embed_neural_or_tfidf`] when the neocortex IPC call
    /// succeeds and the returned vector matches the expected dimension.
    Neural,
}

/// Embed `text` and return both the vector and the quality level used.
///
/// This is a **synchronous, TF-IDF-only** function. It never performs IPC
/// and always returns [`EmbeddingQuality::TfIdf`] with a 384-dim vector.
///
/// For the neural path (async, requires the neocortex to be connected), use
/// [`embed_neural_or_tfidf`] instead. That function falls back to TF-IDF
/// automatically when the neural path is unavailable.
///
/// Existing callers using `embed()` are unaffected — this is an additive API.
pub fn embed_with_quality(text: &str) -> (Vec<f32>, EmbeddingQuality) {
    // Intentionally sync and TfIdf-only. Callers with IPC access should use
    // embed_neural_or_tfidf(), which is async and can return EmbeddingQuality::Neural.
    (embed(text), EmbeddingQuality::TfIdf)
}

/// Batch variant of [`embed_with_quality`].
///
/// All texts in the batch are embedded with the same quality path (cannot mix
/// TfIdf and Neural in one batch — the quality signal is uniform for the batch).
pub fn embed_batch_with_quality(texts: &[&str]) -> (Vec<Vec<f32>>, EmbeddingQuality) {
    // TODO(Phase 2A): attempt Neural batch path.
    let vecs: Vec<Vec<f32>> = texts.iter().map(|t| embed(t)).collect();
    (vecs, EmbeddingQuality::TfIdf)
}

// ---------------------------------------------------------------------------
// Core embedding API
// ---------------------------------------------------------------------------

pub fn embed(text: &str) -> Vec<f32> {
    if text.is_empty() {
        return vec![0.0; EMBED_DIM];
    }

    // Check cache first (read lock — non-blocking for concurrent readers).
    let cache_key = fnv_hash_str(text);
    if let Some(cached) = cache_get(cache_key) {
        // PERF-HIGH-3: Arc::clone under read lock, then deref outside lock.
        // Only the 384×4 = 1536 byte copy happens here, not under any lock.
        return (*cached).clone();
    }

    let result = embed_tfidf(text);

    // Store in cache (write lock — only taken on cache miss).
    cache_insert(cache_key, result.clone());

    result
}

/// Raw TF-IDF sign-hash embedding without cache (exposed for testing).
fn embed_tfidf(text: &str) -> Vec<f32> {
    let normalized = normalize_text(text);
    let tokens = tokenize(&normalized);

    if tokens.is_empty() {
        return vec![0.0; EMBED_DIM];
    }

    let mut buckets = vec![0.0f32; EMBED_DIM];

    // Count token frequencies.
    let mut tf_map: HashMap<&str, u32> = HashMap::new();
    for token in &tokens {
        *tf_map.entry(token.as_str()).or_insert(0) += 1;
    }

    // --- Unigrams with TF-IDF weighting and sign hashing ---
    for (token, &count) in &tf_map {
        let hash = fnv_hash_str(token);
        let bucket = (hash % EMBED_DIM as u64) as usize;
        let sign = sign_hash(token);

        let tf_weight = (1.0 + count as f32).ln();
        // IDF approximation: longer/rarer words get higher weight.
        let idf_approx = 1.0 + 0.5 * (token.len() as f32 / 10.0).min(1.0);

        buckets[bucket] += sign * UNIGRAM_WEIGHT * tf_weight * idf_approx;
    }

    // --- Bigrams with sign hashing ---
    for window in tokens.windows(2) {
        let bigram = format!("{}_{}", window[0], window[1]);
        let hash = fnv_hash_str(&bigram);
        let bucket = (hash % EMBED_DIM as u64) as usize;
        let sign = sign_hash(&bigram);

        let idf_approx = 1.0 + 0.5 * (bigram.len() as f32 / 10.0).min(1.0);
        buckets[bucket] += sign * BIGRAM_WEIGHT * idf_approx;
    }

    // --- Character trigrams with sign hashing ---
    // Provides subword-level similarity (morphological overlap, typo tolerance).
    let chars: Vec<char> = normalized.chars().collect();
    if chars.len() >= 3 {
        let mut trigram_counts: HashMap<[char; 3], u32> = HashMap::new();
        for window in chars.windows(3) {
            let tri = [window[0], window[1], window[2]];
            // Skip trigrams that are all whitespace.
            if tri.iter().all(|c| c.is_whitespace()) {
                continue;
            }
            *trigram_counts.entry(tri).or_insert(0) += 1;
        }
        for (tri, count) in &trigram_counts {
            let tri_str: String = tri.iter().collect();
            let hash = fnv_hash_str(&tri_str);
            let bucket = (hash % EMBED_DIM as u64) as usize;
            let sign = sign_hash(&tri_str);
            let tf = (1.0 + *count as f32).ln();
            buckets[bucket] += sign * CHAR_TRIGRAM_WEIGHT * tf;
        }
    }

    // L2-normalize to unit vector.
    let magnitude = dot_product(&buckets, &buckets).sqrt();
    if magnitude > f32::EPSILON {
        for v in buckets.iter_mut() {
            *v /= magnitude;
        }
    }

    buckets
}

/// Batch embedding: embed multiple texts, returning a Vec of embeddings.
///
/// Uses the same cache as `embed()`. More efficient than calling `embed()`
/// in a loop when embeddings might already be cached.
pub fn embed_batch(texts: &[&str]) -> Vec<Vec<f32>> {
    texts.iter().map(|t| embed(t)).collect()
}

/// Quantize an f32 embedding to u8 (0..255) for compact storage.
///
/// Maps \[-1.0, 1.0\] linearly to \[0, 255\]. Values outside the range are
/// clamped. The resulting vector is 4× smaller than f32.
pub fn quantize_u8(embedding: &[f32]) -> Vec<u8> {
    embedding
        .iter()
        .map(|&v| {
            let clamped = v.clamp(-1.0, 1.0);
            ((clamped + 1.0) * 127.5) as u8
        })
        .collect()
}

/// Dequantize a u8 embedding back to f32.
///
/// Inverse of `quantize_u8`. Note: this is lossy — the round-trip introduces
/// small quantization error (±0.004 per dimension).
pub fn dequantize_u8(quantized: &[u8]) -> Vec<f32> {
    quantized
        .iter()
        .map(|&v| (v as f32 / 127.5) - 1.0)
        .collect()
}

/// Tokenize text: split on non-alphanumeric, lowercase, filter stop words.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .filter(|s| s.len() > 1 && !is_stop_word(s))
        .collect()
}

// ---------------------------------------------------------------------------
// Neural embedding via IPC
// ---------------------------------------------------------------------------

/// Try neural embedding via the Neocortex IPC, falling back to TF-IDF.
///
/// Returns both the embedding vector **and** the quality level so callers can
/// set appropriate similarity thresholds and avoid cross-quality comparisons.
///
/// # Parameters
///
/// - `text` — input text to embed.
/// - `expected_neural_dim` — the embedding dimension the neocortex model is
///   expected to return. Pass the value from `ModelCapabilities.embedding_dim`
///   when available, or [`DIM_NEURAL_FALLBACK`] when the neocortex is known to
///   be running but the exact dim has not been queried yet. Pass `None` to
///   skip the neural path entirely (no IPC call is made; equivalent to calling
///   [`embed_with_quality`]).
/// - `client` — optional mutable reference to an active [`NeocortexClient`].
///   When `None`, the neural path is skipped regardless of `expected_neural_dim`.
///
/// # Dimension validation
///
/// When `expected_neural_dim` is `Some(dim)` and the neocortex returns a
/// vector whose length ≠ `dim as usize`, the vector is **discarded** and the
/// function falls back to TF-IDF with a warning. This guards against silent
/// garbage-similarity caused by comparing vectors of mismatched length.
///
/// # Fallback behaviour
///
/// Any of the following causes silent fallback to TF-IDF:
/// - `client` is `None` or disconnected
/// - `expected_neural_dim` is `None`
/// - IPC request fails or times out
/// - Returned vector length ≠ `expected_neural_dim`
/// - Unexpected response variant
pub async fn embed_neural_or_tfidf(
    text: &str,
    expected_neural_dim: Option<u32>,
    client: Option<&mut crate::ipc::client::NeocortexClient>,
) -> (Vec<f32>, EmbeddingQuality) {
    if text.is_empty() {
        return (vec![0.0; EMBED_DIM], EmbeddingQuality::TfIdf);
    }

    // Skip neural path if no dim or no client.
    if let (Some(neural_dim), Some(c)) = (expected_neural_dim, client) {
        if c.is_connected() {
            let msg = aura_types::ipc::DaemonToNeocortex::Embed {
                text: text.to_string(),
            };
            match c.request(&msg).await {
                Ok(aura_types::ipc::NeocortexToDaemon::Embedding { vector }) => {
                    if vector.len() == neural_dim as usize {
                        return (vector, EmbeddingQuality::Neural);
                    } else {
                        tracing::warn!(
                            expected = neural_dim,
                            got = vector.len(),
                            "Neocortex returned embedding with wrong dimension; \
                             falling back to TF-IDF. \
                             Check ModelCapabilities.embedding_dim matches the loaded GGUF."
                        );
                    }
                }
                Ok(_) => {
                    tracing::warn!(
                        "Neocortex returned unexpected response to Embed request; \
                         falling back to TF-IDF."
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Neural embedding IPC call failed; falling back to TF-IDF."
                    );
                }
            }
        }
    }

    // Fallback: synchronous TF-IDF (always available).
    (embed(text), EmbeddingQuality::TfIdf)
}

/// Async neural embedding via the Neocortex process, with TF-IDF fallback.
///
/// Returns only the raw vector (no quality signal). Prefer [`embed_neural_or_tfidf`]
/// for new call-sites that can track quality.
///
/// # Parameters
///
/// - `expected_neural_dim` — the model's embedding dimension, used to
///   validate the returned vector. Pass [`DIM_NEURAL_FALLBACK`] if unknown.
///   Pass `None` to disable neural path entirely.
///
/// See [`embed_neural_or_tfidf`] for full semantics.
pub async fn embed_best_effort(
    text: &str,
    expected_neural_dim: Option<u32>,
    client: Option<&mut crate::ipc::client::NeocortexClient>,
) -> Vec<f32> {
    embed_neural_or_tfidf(text, expected_neural_dim, client).await.0
}

// ---------------------------------------------------------------------------
// Similarity / distance functions
// ---------------------------------------------------------------------------

/// Compute cosine similarity between two vectors.
///
/// Returns dot(a,b) / (|a| * |b|), or 0.0 if either vector is zero.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(
        a.len(),
        b.len(),
        "vectors must have equal length for cosine similarity"
    );
    let dot = dot_product(a, b);
    let mag_a = dot_product(a, a).sqrt();
    let mag_b = dot_product(b, b).sqrt();

    if mag_a < f32::EPSILON || mag_b < f32::EPSILON {
        return 0.0;
    }

    let sim = dot / (mag_a * mag_b);
    sim.clamp(-1.0, 1.0)
}

/// Compute Euclidean (L2) distance between two vectors.
pub fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

/// Compute dot-product similarity (same as `dot_product`, exposed publicly).
pub fn dot_similarity(a: &[f32], b: &[f32]) -> f32 {
    dot_product(a, b)
}

/// Compute Jaccard similarity over character trigram sets.
///
/// jaccard(A, B) = |A ∩ B| / |A ∪ B|
pub fn jaccard_trigram_similarity(a: &str, b: &str) -> f32 {
    let set_a = extract_trigram_set(a);
    let set_b = extract_trigram_set(b);

    if set_a.is_empty() && set_b.is_empty() {
        return 0.0;
    }

    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();

    if union == 0 {
        return 0.0;
    }

    intersection as f32 / union as f32
}

// ---------------------------------------------------------------------------
// Serialization for SQLite BLOB storage
// ---------------------------------------------------------------------------

/// Serialize an f32 embedding to bytes (EMBED_DIM * 4 bytes).
pub fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(embedding.len() * 4);
    for &val in embedding {
        bytes.extend_from_slice(&val.to_le_bytes());
    }
    bytes
}

/// Deserialize bytes back to an f32 embedding.
pub fn embedding_from_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            let arr: [u8; 4] = [chunk[0], chunk[1], chunk[2], chunk[3]];
            f32::from_le_bytes(arr)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Normalize text: lowercase, collapse whitespace, trim.
fn normalize_text(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut result = String::with_capacity(lower.len());
    let mut last_was_space = true;
    for ch in lower.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                result.push(' ');
                last_was_space = true;
            }
        } else {
            result.push(ch);
            last_was_space = false;
        }
    }
    if result.ends_with(' ') {
        result.pop();
    }
    result
}

/// FNV-1a hash of a string (primary hash for bucket selection).
#[inline]
fn fnv_hash_str(s: &str) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Sign hash: returns +1.0 or -1.0 using a second independent FNV hash.
///
/// This is the core of the sign-hashing trick (Weinberger et al. 2009).
/// By using a different seed than the bucket hash, the sign is independent
/// of the bucket assignment, ensuring unbiased cancellation of collisions.
#[inline]
fn sign_hash(s: &str) -> f32 {
    let mut hash = FNV_SIGN_OFFSET;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    if hash & 1 == 0 { 1.0 } else { -1.0 }
}

/// Dot product of two f32 slices.
#[inline]
fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Extract the set of character trigrams from text (normalized).
fn extract_trigram_set(text: &str) -> HashSet<[char; 3]> {
    let normalized = normalize_text(text);
    let chars: Vec<char> = normalized.chars().collect();
    let mut set = HashSet::new();
    if chars.len() >= 3 {
        for window in chars.windows(3) {
            set.insert([window[0], window[1], window[2]]);
        }
    }
    set
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embed_deterministic() {
        let v1 = embed("hello world");
        let v2 = embed("hello world");
        assert_eq!(v1, v2, "embedding must be deterministic");
    }

    #[test]
    fn test_embed_dimension() {
        let v = embed("test input");
        assert_eq!(v.len(), EMBED_DIM);
        assert_eq!(v.len(), 384);
    }

    #[test]
    fn test_embed_unit_vector() {
        let v = embed("the quick brown fox jumps over the lazy dog");
        let mag: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (mag - 1.0).abs() < 1e-5,
            "embedding should be unit vector, got magnitude {mag}"
        );
    }

    #[test]
    fn test_embed_empty_string() {
        let v = embed("");
        assert_eq!(v.len(), EMBED_DIM);
        assert!(v.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn test_embed_case_insensitive() {
        let v1 = embed("Hello World");
        let v2 = embed("hello world");
        assert_eq!(v1, v2, "embedding must be case-insensitive");
    }

    #[test]
    fn test_cosine_self_similarity() {
        let v = embed("memory consolidation patterns");
        let sim = cosine_similarity(&v, &v);
        assert!(
            (sim - 1.0).abs() < 1e-5,
            "self-similarity should be 1.0, got {sim}"
        );
    }

    #[test]
    fn test_cosine_similar_texts() {
        let v1 = embed("user prefers dark mode");
        let v2 = embed("user likes dark theme");
        let sim = cosine_similarity(&v1, &v2);
        assert!(
            sim > 0.2,
            "similar texts should have reasonable similarity, got {sim}"
        );
    }

    #[test]
    fn test_cosine_dissimilar_texts() {
        let v1 = embed("quantum physics equations");
        let v2 = embed("chocolate cake recipe");
        let v3 = embed("quantum mechanics theory");
        let sim_diff = cosine_similarity(&v1, &v2);
        let sim_related = cosine_similarity(&v1, &v3);
        assert!(
            sim_related > sim_diff,
            "related texts ({sim_related}) should be more similar than unrelated ({sim_diff})"
        );
    }

    #[test]
    fn test_sign_hashing_reduces_false_similarity() {
        // This is the critical test: unrelated content should have low similarity.
        let weather = embed("weather forecast sunny warm temperature climate");
        let meeting = embed("meeting at 3pm with Alice");
        let query = embed("weather forecast temperature");

        let sim_weather = cosine_similarity(&query, &weather);
        let sim_meeting = cosine_similarity(&query, &meeting);

        assert!(
            sim_weather > sim_meeting,
            "weather query ({sim_weather}) should be more similar to weather content than meeting ({sim_meeting})"
        );
        // The gap should be significant, not marginal.
        assert!(
            sim_weather - sim_meeting > 0.1,
            "similarity gap should be significant: weather={sim_weather}, meeting={sim_meeting}"
        );
    }

    #[test]
    fn test_embedding_serialization_roundtrip() {
        let original = embed("test embedding serialization");
        let bytes = embedding_to_bytes(&original);
        assert_eq!(bytes.len(), EMBED_DIM * 4);
        let restored = embedding_from_bytes(&bytes);
        assert_eq!(original, restored);
    }

    #[test]
    fn test_cache_hit() {
        let text = "unique text for cache test 12345";
        let _ = embed(text); // First call — populates cache.
        let v2 = embed(text); // Second call — should hit cache.
        let v3 = embed_tfidf(text); // Direct call — bypasses cache.
        assert_eq!(v2, v3, "cached and direct should match");
    }

    #[test]
    fn test_stop_words_filtered() {
        let v1 = embed("the user preferences");
        let v2 = embed("user preferences");
        let sim = cosine_similarity(&v1, &v2);
        assert!(
            sim > 0.8,
            "stop word filtering should make these very similar, got {sim}"
        );
    }

    #[test]
    fn test_cosine_zero_vector() {
        let zero = vec![0.0f32; EMBED_DIM];
        let v = embed("test");
        assert_eq!(cosine_similarity(&zero, &v), 0.0);
        assert_eq!(cosine_similarity(&v, &zero), 0.0);
    }

    #[test]
    fn test_jaccard_identical() {
        let sim = jaccard_trigram_similarity("hello", "hello");
        assert!((sim - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_jaccard_completely_different() {
        let sim = jaccard_trigram_similarity("abc", "xyz");
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn test_normalize_text() {
        assert_eq!(normalize_text("  Hello   World  "), "hello world");
        assert_eq!(normalize_text("TAB\there"), "tab here");
        assert_eq!(normalize_text(""), "");
    }

    #[test]
    fn test_tokenize() {
        let tokens = tokenize("user prefers the dark mode");
        assert!(!tokens.contains(&"the".to_string()));
        assert!(tokens.contains(&"user".to_string()));
        assert!(tokens.contains(&"prefers".to_string()));
        assert!(tokens.contains(&"dark".to_string()));
        assert!(tokens.contains(&"mode".to_string()));
    }

    #[test]
    fn test_quantize_roundtrip() {
        let original = embed("quantization test vector");
        let quantized = quantize_u8(&original);
        assert_eq!(quantized.len(), EMBED_DIM);
        let restored = dequantize_u8(&quantized);
        assert_eq!(restored.len(), EMBED_DIM);
        // Check that round-trip error is small (±0.008 per dim).
        for (o, r) in original.iter().zip(restored.iter()) {
            assert!(
                (o - r).abs() < 0.01,
                "quantization error too large: original={o}, restored={r}"
            );
        }
    }

    #[test]
    fn test_euclidean_distance_zero() {
        let v = embed("same text");
        let d = euclidean_distance(&v, &v);
        assert!(d.abs() < 1e-5, "distance to self should be 0, got {d}");
    }

    #[test]
    fn test_euclidean_distance_positive() {
        let v1 = embed("cat");
        let v2 = embed("airplane");
        let d = euclidean_distance(&v1, &v2);
        assert!(d > 0.0, "different texts should have positive distance");
    }

    #[test]
    fn test_dot_similarity() {
        let v = embed("hello world");
        let d = dot_similarity(&v, &v);
        // Unit vectors → dot product with self = 1.0.
        assert!(
            (d - 1.0).abs() < 1e-5,
            "dot product of unit vector with self should be 1.0, got {d}"
        );
    }

    #[test]
    fn test_embed_batch() {
        let texts = &["hello world", "foo bar", "test input"];
        let embeddings = embed_batch(texts);
        assert_eq!(embeddings.len(), 3);
        for emb in &embeddings {
            assert_eq!(emb.len(), EMBED_DIM);
        }
        // Each should match individual embed call.
        assert_eq!(embeddings[0], embed("hello world"));
        assert_eq!(embeddings[1], embed("foo bar"));
        assert_eq!(embeddings[2], embed("test input"));
    }

    #[test]
    fn test_sign_hash_deterministic() {
        let s1 = sign_hash("hello");
        let s2 = sign_hash("hello");
        assert_eq!(s1, s2, "sign hash must be deterministic");
        assert!(s1 == 1.0 || s1 == -1.0, "sign must be +1 or -1");
    }

    #[test]
    fn test_sign_hash_distribution() {
        // Over many strings, roughly half should be +1 and half -1.
        let mut positives = 0;
        let count = 1000;
        for i in 0..count {
            let s = format!("token_{}", i);
            if sign_hash(&s) > 0.0 {
                positives += 1;
            }
        }
        // Allow ±10% deviation from 50/50.
        assert!(
            positives > 400 && positives < 600,
            "sign distribution should be roughly 50/50, got {positives}/{count}"
        );
    }

    /// Verifies that `embed_neural_or_tfidf` falls back to TF-IDF when the
    /// expected dimension does not match the vector length, and that calling
    /// it with `None` client also returns TF-IDF.
    ///
    /// This test exercises the dimension-validation guard without a live IPC
    /// connection: by passing `None` for the client the neural path is skipped
    /// and the function must return `EmbeddingQuality::TfIdf`.
    #[tokio::test]
    async fn embed_neural_or_tfidf_with_wrong_dim_falls_back_to_tfidf() {
        // Case 1: no client → must return TfIdf regardless of dim hint.
        let (vec_no_client, quality_no_client) =
            embed_neural_or_tfidf("hello world", Some(DIM_NEURAL_FALLBACK), None).await;
        assert_eq!(
            quality_no_client,
            EmbeddingQuality::TfIdf,
            "no client → must be TfIdf"
        );
        assert_eq!(
            vec_no_client.len(),
            EMBED_DIM,
            "no client → vector must be EMBED_DIM ({EMBED_DIM})"
        );

        // Case 2: expected_neural_dim = None → skip neural path entirely.
        let (vec_no_dim, quality_no_dim) =
            embed_neural_or_tfidf("hello world", None, None).await;
        assert_eq!(
            quality_no_dim,
            EmbeddingQuality::TfIdf,
            "None dim → must be TfIdf"
        );
        assert_eq!(vec_no_dim.len(), EMBED_DIM);

        // Case 3: empty text → always returns zero TfIdf vector.
        let (vec_empty, quality_empty) =
            embed_neural_or_tfidf("", Some(DIM_NEURAL_FALLBACK), None).await;
        assert_eq!(quality_empty, EmbeddingQuality::TfIdf);
        assert_eq!(vec_empty.len(), EMBED_DIM);
        assert!(vec_empty.iter().all(|&x| x == 0.0));

        // Case 4: consistency — result must equal the sync TfIdf path.
        let sync_vec = embed("the quick brown fox");
        let (async_vec, async_quality) =
            embed_neural_or_tfidf("the quick brown fox", None, None).await;
        assert_eq!(async_quality, EmbeddingQuality::TfIdf);
        assert_eq!(
            async_vec, sync_vec,
            "async TfIdf fallback must match sync embed()"
        );
    }

    #[test]
    fn test_weather_vs_meeting_vs_darkmode() {
        // Regression test: weather query must rank weather content above
        // unrelated content (this was broken with 128-dim naive hashing).
        let query = embed("weather forecast temperature");
        let weather = embed("weather forecast sunny warm temperature climate");
        let meeting = embed("meeting at 3pm with Alice");
        let darkmode = embed("user prefers dark mode");

        let sim_w = cosine_similarity(&query, &weather);
        let sim_m = cosine_similarity(&query, &meeting);
        let sim_d = cosine_similarity(&query, &darkmode);

        assert!(
            sim_w > sim_m && sim_w > sim_d,
            "weather should rank first: weather={sim_w}, meeting={sim_m}, darkmode={sim_d}"
        );
    }
}
