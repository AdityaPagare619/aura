//! Consolidation engine — 4-level memory management inspired by sleep-stage memory consolidation.
//!
//! Levels:
//! - **Micro** (<1ms): Working memory sweep — remove expired slots
//! - **Light** (≤60s): Promote important working slots to episodic, reinforce semantic entries
//! - **Deep** (≤30min): Generalize from episode clusters, archive old low-importance episodes
//! - **Emergency** (<5s): Free 2.8-3.6MB by aggressive sweep + archival
//!
//! Deep consolidation uses k-means clustering on episode embeddings to discover
//! natural topic clusters instead of hardcoded sample queries. Pattern outcomes
//! are recorded into the PatternEngine for Hebbian learning.
//!
//! Consolidation scoring formula:
//!   score = recency_factor(0.3) + frequency_factor(0.3) + importance_factor(0.4)
//!   where recency uses a 7-day half-life exponential decay.

use tracing::{debug, info};

use crate::memory::archive::{ArchiveMemory, ARCHIVE_AGE_THRESHOLD_MS, ARCHIVE_IMPORTANCE_THRESHOLD, CompressionAlgo};
use crate::memory::embeddings::{cosine_similarity, embed};
use crate::memory::episodic::EpisodicMemory;
use crate::memory::importance;
use crate::memory::patterns::PatternEngine;
use crate::memory::semantic::SemanticMemory;
use crate::memory::working::WorkingMemory;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Half-life for recency scoring (7 days in milliseconds).
const RECENCY_HALF_LIFE_MS: f64 = 7.0 * 24.0 * 3600.0 * 1000.0;

/// Maximum episodes to fetch for clustering.
const CLUSTER_EPISODE_LIMIT: usize = 100;

/// Minimum cluster size for generalization attempt.
const MIN_CLUSTER_SIZE: usize = 3;

/// Number of k-means iterations.
const KMEANS_ITERATIONS: usize = 10;

/// Number of clusters to try for k-means (auto-limited by episode count).
const KMEANS_K: usize = 8;

/// Similarity threshold for assigning an episode to a cluster centroid.
const CLUSTER_ASSIGNMENT_THRESHOLD: f32 = 0.3;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Consolidation level (from the v4 spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsolidationLevel {
    /// <1ms — sweep expired working memory slots.
    Micro,
    /// ≤60s — promote working → episodic, reinforce semantic.
    Light,
    /// ≤30min — generalize episodes → semantic, archive old episodes.
    Deep,
    /// <5s — emergency free 2.8-3.6 MB.
    Emergency,
}

/// Report of what a consolidation pass accomplished.
#[derive(Debug, Clone, Default)]
pub struct ConsolidationReport {
    pub level: Option<ConsolidationLevel>,
    /// Number of expired working memory slots swept.
    pub working_slots_swept: usize,
    /// Number of working slots promoted to episodic.
    pub working_to_episodic: usize,
    /// Number of semantic entries reinforced.
    pub semantic_reinforced: usize,
    /// Number of new semantic entries created via generalization.
    pub semantic_generalized: usize,
    /// Number of episodes archived.
    pub episodes_archived: usize,
    /// Number of patterns recorded during this consolidation.
    pub patterns_recorded: usize,
    /// Estimated bytes freed.
    pub bytes_freed: u64,
    /// Duration of the consolidation pass in milliseconds.
    pub duration_ms: u64,
    /// Any errors encountered (non-fatal).
    pub errors: Vec<String>,
}

impl std::fmt::Display for ConsolidationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Micro => write!(f, "micro"),
            Self::Light => write!(f, "light"),
            Self::Deep => write!(f, "deep"),
            Self::Emergency => write!(f, "emergency"),
        }
    }
}

// ---------------------------------------------------------------------------
// Consolidation scoring
// ---------------------------------------------------------------------------

/// Compute a consolidation priority score for an item.
///
/// Formula: recency(0.3) + frequency(0.3) + importance(0.4)
///
/// - recency: exponential decay with 7-day half-life
/// - frequency: log-scaled access count (capped at 1.0)
/// - importance: raw importance score (already 0..1)
fn consolidation_priority(
    age_ms: u64,
    access_count: u32,
    importance_score: f32,
) -> f32 {
    // Recency: exp(-ln(2) * age / half_life)
    let recency = (-(std::f64::consts::LN_2 * age_ms as f64 / RECENCY_HALF_LIFE_MS)).exp() as f32;

    // Frequency: log2(access_count + 1) / log2(32), capped at 1.0
    let frequency = ((access_count as f32 + 1.0).log2() / 5.0).min(1.0);

    // Weighted combination
    0.3 * recency + 0.3 * frequency + 0.4 * importance_score.clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// Consolidation engine
// ---------------------------------------------------------------------------

/// Run consolidation at the specified level.
///
/// Each higher level includes the work of lower levels:
/// - Emergency does Light + aggressive archival
/// - Deep does Light + generalization + archival
/// - Light does Micro + promotion + reinforcement
/// - Micro is just a sweep
pub async fn consolidate(
    level: ConsolidationLevel,
    working: &mut WorkingMemory,
    episodic: &EpisodicMemory,
    semantic: &SemanticMemory,
    archive: &ArchiveMemory,
    patterns: &mut PatternEngine,
    now_ms: u64,
) -> ConsolidationReport {
    let start = std::time::Instant::now();
    let mut report = ConsolidationReport {
        level: Some(level),
        ..Default::default()
    };

    match level {
        ConsolidationLevel::Micro => {
            run_micro(working, now_ms, &mut report);
        }
        ConsolidationLevel::Light => {
            run_micro(working, now_ms, &mut report);
            run_light(working, episodic, semantic, patterns, now_ms, &mut report).await;
        }
        ConsolidationLevel::Deep => {
            run_micro(working, now_ms, &mut report);
            run_light(working, episodic, semantic, patterns, now_ms, &mut report).await;
            run_deep(episodic, semantic, archive, patterns, now_ms, &mut report).await;
        }
        ConsolidationLevel::Emergency => {
            run_emergency(working, episodic, archive, now_ms, &mut report).await;
        }
    }

    report.duration_ms = start.elapsed().as_millis() as u64;

    info!(
        "consolidation [{}] complete in {}ms: swept={}, promoted={}, reinforced={}, generalized={}, archived={}, patterns={}, freed={}B",
        level,
        report.duration_ms,
        report.working_slots_swept,
        report.working_to_episodic,
        report.semantic_reinforced,
        report.semantic_generalized,
        report.episodes_archived,
        report.patterns_recorded,
        report.bytes_freed,
    );

    report
}

// ---------------------------------------------------------------------------
// Level implementations
// ---------------------------------------------------------------------------

/// Micro consolidation: sweep expired working memory slots.
/// Target: <1ms.
fn run_micro(
    working: &mut WorkingMemory,
    now_ms: u64,
    report: &mut ConsolidationReport,
) {
    let swept = working.sweep_expired(now_ms);
    report.working_slots_swept += swept;
    if swept > 0 {
        debug!("micro: swept {} expired working slots", swept);
    }
}

/// Light consolidation: promote important working slots to episodic,
/// check for semantic reinforcement opportunities, record patterns.
/// Target: ≤60s.
async fn run_light(
    working: &mut WorkingMemory,
    episodic: &EpisodicMemory,
    semantic: &SemanticMemory,
    patterns: &mut PatternEngine,
    now_ms: u64,
    report: &mut ConsolidationReport,
) {
    // 1. Promote high-value working slots to episodic
    let snapshot = working.snapshot(now_ms);
    let mut to_remove: Vec<usize> = Vec::new();

    for (idx, slot) in &snapshot {
        // Use the new consolidation priority scoring
        let age_ms = now_ms.saturating_sub(slot.timestamp_ms);
        let priority = consolidation_priority(age_ms, 0, slot.importance);

        // Also check the legacy importance-based scoring for backward compat
        let hours_ago = age_ms as f64 / 3_600_000.0;
        let consol_score = importance::consolidation_score(hours_ago, 0, slot.importance);

        // Promote if either scoring says yes
        if priority >= 0.5 || consol_score >= 0.7 || slot.importance >= 0.7 {
            match episodic
                .store(
                    slot.content.clone(),
                    0.0, // neutral emotional valence by default
                    slot.importance,
                    vec![], // no tags from working memory
                    slot.timestamp_ms,
                )
                .await
            {
                Ok(ep_id) => {
                    debug!(
                        "light: promoted working slot {} to episode {} (importance: {:.2}, priority: {:.2})",
                        idx, ep_id, slot.importance, priority
                    );
                    to_remove.push(*idx);
                    report.working_to_episodic += 1;

                    // Record pattern: successful promotion
                    if let Err(e) = patterns.record_outcome(
                        "consolidation:promote",
                        &format!("importance:{:.1}", slot.importance),
                        "promoted_to_episodic",
                        true,
                        now_ms,
                    ) {
                        report.errors.push(format!("pattern record failed: {}", e));
                    } else {
                        report.patterns_recorded += 1;
                    }
                }
                Err(e) => {
                    report
                        .errors
                        .push(format!("promote slot {} failed: {}", idx, e));
                }
            }
        }
    }

    // Remove promoted slots from working memory
    for idx in to_remove {
        working.remove(idx);
    }

    // 2. Check for semantic reinforcement
    // Look at recently stored episodes and see if they match existing semantic entries
    let recent_snapshot = working.snapshot(now_ms);
    for (_idx, slot) in &recent_snapshot {
        match semantic.find_by_concept(&slot.content, 0.6, 1).await {
            Ok(entries) if !entries.is_empty() => {
                let entry = &entries[0];
                if let Err(e) = semantic.reinforce(entry.id, None, now_ms).await {
                    report.errors.push(format!(
                        "reinforce semantic {} failed: {}",
                        entry.id, e
                    ));
                } else {
                    report.semantic_reinforced += 1;
                    debug!(
                        "light: reinforced semantic entry {} (concept: {})",
                        entry.id, entry.concept
                    );

                    // Record pattern: reinforcement
                    if let Err(e) = patterns.record_outcome(
                        "consolidation:reinforce",
                        &entry.concept,
                        "semantic_reinforced",
                        true,
                        now_ms,
                    ) {
                        report.errors.push(format!("pattern record failed: {}", e));
                    } else {
                        report.patterns_recorded += 1;
                    }
                }
            }
            Err(e) => {
                report
                    .errors
                    .push(format!("semantic concept search failed: {}", e));
            }
            _ => {} // No match — nothing to reinforce
        }
    }
}

/// Deep consolidation: generalize from episode clusters, archive old episodes.
/// Target: ≤30min.
async fn run_deep(
    episodic: &EpisodicMemory,
    semantic: &SemanticMemory,
    archive: &ArchiveMemory,
    patterns: &mut PatternEngine,
    now_ms: u64,
    report: &mut ConsolidationReport,
) {
    // 1. Find episodes that could be generalized via k-means clustering
    run_generalization(episodic, semantic, patterns, now_ms, report).await;

    // 2. Archive old, low-importance episodes
    run_archival(episodic, archive, now_ms, report).await;
}

/// Emergency consolidation: aggressively free memory.
/// Target: <5s, free 2.8-3.6 MB.
async fn run_emergency(
    working: &mut WorkingMemory,
    episodic: &EpisodicMemory,
    archive: &ArchiveMemory,
    now_ms: u64,
    report: &mut ConsolidationReport,
) {
    info!("EMERGENCY consolidation triggered — freeing memory aggressively");

    // 1. Sweep ALL expired working slots
    let swept = working.sweep_expired(now_ms);
    report.working_slots_swept += swept;

    // 2. Remove low-importance working slots (below 0.3)
    let snapshot = working.snapshot(now_ms);
    let mut low_imp_removed = 0;
    for (idx, slot) in &snapshot {
        if slot.importance < 0.3 {
            working.remove(*idx);
            low_imp_removed += 1;
        }
    }
    report.working_slots_swept += low_imp_removed;

    // Estimate freed bytes from working memory
    report.bytes_freed += (swept + low_imp_removed) as u64 * 512; // ~512 bytes per slot estimate

    // 3. Aggressively archive episodes
    // Lower thresholds: importance < 0.5 (instead of 0.3) and age > 7 days (instead of 30)
    let emergency_age_threshold = 7 * 24 * 60 * 60 * 1000; // 7 days
    let emergency_importance_threshold = 0.5;

    match episodic
        .get_archival_candidates(emergency_age_threshold, emergency_importance_threshold, now_ms, 200)
        .await
    {
        Ok(candidates) => {
            let mut archived_ids: Vec<u64> = Vec::new();
            let mut total_content_bytes: u64 = 0;

            for episode in &candidates {
                let content_bytes = episode.content.as_bytes().to_vec();
                total_content_bytes += content_bytes.len() as u64;

                match archive
                    .archive(
                        truncate_summary(&episode.content, 200),
                        content_bytes,
                        episode.importance,
                        episode.timestamp_ms,
                        episode.timestamp_ms,
                        "episode".into(),
                        vec![episode.id],
                        CompressionAlgo::Lz4,
                    )
                    .await
                {
                    Ok(_) => {
                        archived_ids.push(episode.id);
                    }
                    Err(e) => {
                        report
                            .errors
                            .push(format!("emergency archive ep {} failed: {}", episode.id, e));
                    }
                }
            }

            if !archived_ids.is_empty() {
                match episodic.delete_episodes(&archived_ids).await {
                    Ok(deleted) => {
                        report.episodes_archived += deleted;
                        report.bytes_freed += total_content_bytes;
                    }
                    Err(e) => {
                        report
                            .errors
                            .push(format!("emergency delete episodes failed: {}", e));
                    }
                }
            }
        }
        Err(e) => {
            report
                .errors
                .push(format!("emergency archival candidates failed: {}", e));
        }
    }

    info!(
        "EMERGENCY consolidation: freed ~{}KB (swept={}, archived={})",
        report.bytes_freed / 1024,
        report.working_slots_swept,
        report.episodes_archived,
    );
}

// ---------------------------------------------------------------------------
// K-means clustering for episode generalization
// ---------------------------------------------------------------------------

/// Compute embeddings for a list of episode contents and run k-means
/// to find natural topic clusters.
fn cluster_episodes(
    contents: &[String],
    k: usize,
) -> Vec<Vec<usize>> {
    if contents.is_empty() || k == 0 {
        return Vec::new();
    }

    let embeddings: Vec<Vec<f32>> = contents.iter().map(|c| embed(c)).collect();
    let n = embeddings.len();
    let actual_k = k.min(n);

    if actual_k <= 1 {
        // A single cluster only makes sense if it has enough members.
        if n >= MIN_CLUSTER_SIZE {
            return vec![(0..n).collect()];
        }
        return Vec::new();
    }

    // Initialize centroids using k-means++ (D²-weighted probabilistic selection).
    // This yields better cluster quality than first-k init, converges faster,
    // and is robust to input ordering. See: Arthur & Vassilvitskii 2007.
    let dim = embeddings[0].len();
    let mut centroids: Vec<Vec<f32>> = Vec::with_capacity(actual_k);

    // First centroid: deterministic pick based on embedding content hash.
    let first_seed = embeddings
        .iter()
        .flat_map(|e| e.iter())
        .fold(0u64, |acc, &v| acc.wrapping_add(v.to_bits() as u64));
    centroids.push(embeddings[(first_seed as usize) % n].clone());

    // Subsequent centroids: pick proportional to D² (squared min-distance).
    for _ in 1..actual_k {
        let mut distances: Vec<f32> = embeddings
            .iter()
            .map(|emb| {
                centroids
                    .iter()
                    .map(|c| {
                        let sim = cosine_similarity(emb, c);
                        // Distance² = (1 - similarity)² in cosine space
                        let d = 1.0 - sim;
                        d * d
                    })
                    .fold(f32::INFINITY, f32::min) // min distance to any centroid
            })
            .collect();

        // Normalize to probability distribution
        let total: f32 = distances.iter().sum();
        if total < f32::EPSILON {
            break; // All points are at centroids, can't spread further
        }
        for d in distances.iter_mut() {
            *d /= total;
        }

        // Deterministic weighted selection using cumulative distribution
        let pick_seed = first_seed.wrapping_mul(centroids.len() as u64 + 31);
        let threshold = ((pick_seed % 10000) as f32) / 10000.0;
        let mut cumulative = 0.0_f32;
        let mut picked = 0;
        for (i, &d) in distances.iter().enumerate() {
            cumulative += d;
            if cumulative >= threshold {
                picked = i;
                break;
            }
        }
        centroids.push(embeddings[picked].clone());
    }

    let mut assignments = vec![0usize; n];

    for _iter in 0..KMEANS_ITERATIONS {
        // Assignment step: assign each embedding to nearest centroid
        let mut changed = false;
        for (i, emb) in embeddings.iter().enumerate() {
            let mut best_cluster = 0;
            let mut best_sim = f32::NEG_INFINITY;

            for (c, centroid) in centroids.iter().enumerate() {
                let sim = cosine_similarity(emb, centroid);
                if sim > best_sim {
                    best_sim = sim;
                    best_cluster = c;
                }
            }

            // Only assign if similarity exceeds threshold
            if best_sim >= CLUSTER_ASSIGNMENT_THRESHOLD && assignments[i] != best_cluster {
                assignments[i] = best_cluster;
                changed = true;
            }
        }

        if !changed {
            break; // Converged
        }

        // Update step: recompute centroids as mean of assigned embeddings
        let mut new_centroids = vec![vec![0.0f32; dim]; actual_k];
        let mut counts = vec![0usize; actual_k];

        for (i, emb) in embeddings.iter().enumerate() {
            let c = assignments[i];
            counts[c] += 1;
            for (j, &val) in emb.iter().enumerate() {
                new_centroids[c][j] += val;
            }
        }

        for c in 0..actual_k {
            if counts[c] > 0 {
                let count = counts[c] as f32;
                for j in 0..dim {
                    new_centroids[c][j] /= count;
                }
                // Normalize centroid to unit length for cosine similarity
                let norm: f32 = new_centroids[c].iter().map(|v| v * v).sum::<f32>().sqrt();
                if norm > 1e-9 {
                    for j in 0..dim {
                        new_centroids[c][j] /= norm;
                    }
                }
                centroids[c] = new_centroids[c].clone();
            }
        }
    }

    // Group episodes by cluster
    let mut clusters: Vec<Vec<usize>> = vec![Vec::new(); actual_k];
    for (i, &c) in assignments.iter().enumerate() {
        clusters[c].push(i);
    }

    // Filter out clusters that are too small for generalization
    clusters.retain(|c| c.len() >= MIN_CLUSTER_SIZE);
    clusters
}

/// Extract a concept hint from a cluster of episode contents.
/// Uses the most common significant words across the cluster.
fn extract_concept_hint(contents: &[&str]) -> String {
    use std::collections::HashMap;

    let mut word_counts: HashMap<&str, usize> = HashMap::new();

    for content in contents {
        // Deduplicate words per document to get document frequency
        let mut seen = std::collections::HashSet::new();
        for word in content.split_whitespace() {
            let clean = word.trim_matches(|c: char| !c.is_alphanumeric());
            let lower = clean.to_lowercase();
            if lower.len() >= 3 && seen.insert(lower.clone()) {
                // Use the cleaned word (can't store &str from local, but count by position)
                *word_counts.entry(clean).or_insert(0) += 1;
            }
        }
    }

    // Get words that appear in majority of cluster documents
    let threshold = (contents.len() as f32 * 0.5).ceil() as usize;
    let mut common_words: Vec<(&str, usize)> = word_counts
        .into_iter()
        .filter(|(_, count)| *count >= threshold)
        .collect();

    common_words.sort_by(|a, b| b.1.cmp(&a.1));

    let hint: String = common_words
        .iter()
        .take(4)
        .map(|(w, _)| *w)
        .collect::<Vec<_>>()
        .join(" ");

    if hint.is_empty() {
        "general pattern".into()
    } else {
        hint
    }
}

// ---------------------------------------------------------------------------
// Sub-routines
// ---------------------------------------------------------------------------

/// Try to generalize similar episodes into semantic entries using k-means clustering.
async fn run_generalization(
    episodic: &EpisodicMemory,
    semantic: &SemanticMemory,
    patterns: &mut PatternEngine,
    now_ms: u64,
    report: &mut ConsolidationReport,
) {
    // Fetch recent episodes — use a broad query to get a diverse sample
    let episodes = match episodic
        .query("", CLUSTER_EPISODE_LIMIT, 0.0, now_ms)
        .await
    {
        Ok(results) => results,
        Err(_) => {
            // Fallback: try find_similar with very low threshold
            match episodic.find_similar("", 0.0, CLUSTER_EPISODE_LIMIT).await {
                Ok(eps) => {
                    // Convert Episode to something we can work with
                    let mut results = Vec::new();
                    for ep in eps {
                        results.push(aura_types::memory::MemoryResult {
                            content: ep.content,
                            tier: aura_types::ipc::MemoryTier::Episodic,
                            relevance: 0.5,
                            importance: ep.importance,
                            timestamp_ms: ep.timestamp_ms,
                            source_id: ep.id,
                        });
                    }
                    results
                }
                Err(e) => {
                    report.errors.push(format!("episode fetch for clustering failed: {}", e));
                    return;
                }
            }
        }
    };

    if episodes.len() < MIN_CLUSTER_SIZE {
        debug!("deep: not enough episodes for clustering ({})", episodes.len());
        return;
    }

    // Extract contents for clustering
    let contents: Vec<String> = episodes.iter().map(|e| e.content.clone()).collect();

    // Run k-means clustering
    let clusters = cluster_episodes(&contents, KMEANS_K);

    debug!(
        "deep: k-means found {} clusters from {} episodes",
        clusters.len(),
        episodes.len()
    );

    // Attempt generalization for each cluster
    for cluster_indices in &clusters {
        let cluster_contents: Vec<&str> = cluster_indices
            .iter()
            .map(|&i| contents[i].as_str())
            .collect();

        let concept_hint = extract_concept_hint(&cluster_contents);

        let episode_data: Vec<(String, f32)> = cluster_indices
            .iter()
            .map(|&i| (contents[i].clone(), episodes[i].importance))
            .collect();

        match semantic
            .try_generalize(&episode_data, &concept_hint, now_ms)
            .await
        {
            Ok(Some(id)) => {
                report.semantic_generalized += 1;
                debug!(
                    "deep: generalized {} episodes into semantic entry {} (concept: {})",
                    cluster_indices.len(),
                    id,
                    concept_hint
                );

                // Record successful generalization pattern
                if let Err(e) = patterns.record_outcome(
                    "consolidation:generalize",
                    &concept_hint,
                    &format!("created_semantic_{}", id),
                    true,
                    now_ms,
                ) {
                    report.errors.push(format!("pattern record failed: {}", e));
                } else {
                    report.patterns_recorded += 1;
                }
            }
            Ok(None) => {
                // Rejected — episodes not similar enough to each other
                debug!("deep: generalization rejected for cluster (concept: {})", concept_hint);
            }
            Err(e) => {
                report
                    .errors
                    .push(format!("generalization failed for '{}': {}", concept_hint, e));
            }
        }
    }
}

/// Archive old, low-importance episodes.
async fn run_archival(
    episodic: &EpisodicMemory,
    archive: &ArchiveMemory,
    now_ms: u64,
    report: &mut ConsolidationReport,
) {
    match episodic
        .get_archival_candidates(ARCHIVE_AGE_THRESHOLD_MS, ARCHIVE_IMPORTANCE_THRESHOLD, now_ms, 50)
        .await
    {
        Ok(candidates) => {
            if candidates.is_empty() {
                return;
            }

            debug!(
                "deep: found {} archival candidates",
                candidates.len()
            );

            let mut archived_ids: Vec<u64> = Vec::new();

            for episode in &candidates {
                let content_bytes = episode.content.as_bytes().to_vec();

                match archive
                    .archive(
                        truncate_summary(&episode.content, 200),
                        content_bytes,
                        episode.importance,
                        episode.timestamp_ms,
                        episode.timestamp_ms,
                        "episode".into(),
                        vec![episode.id],
                        CompressionAlgo::Lz4,
                    )
                    .await
                {
                    Ok(archive_id) => {
                        archived_ids.push(episode.id);
                        debug!(
                            "deep: archived episode {} -> archive blob {}",
                            episode.id, archive_id
                        );
                    }
                    Err(e) => {
                        report.errors.push(format!(
                            "archive episode {} failed: {}",
                            episode.id, e
                        ));
                    }
                }
            }

            // Delete archived episodes from episodic store
            if !archived_ids.is_empty() {
                match episodic.delete_episodes(&archived_ids).await {
                    Ok(deleted) => {
                        report.episodes_archived += deleted;
                    }
                    Err(e) => {
                        report
                            .errors
                            .push(format!("delete archived episodes failed: {}", e));
                    }
                }
            }
        }
        Err(e) => {
            report
                .errors
                .push(format!("get archival candidates failed: {}", e));
        }
    }
}

/// Truncate text to a max character length for summaries.
fn truncate_summary(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(max_chars - 3).collect();
        format!("{}...", truncated)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> u64 {
        1_700_000_000_000
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn test_truncate_summary_short() {
        assert_eq!(truncate_summary("short", 200), "short");
    }

    #[test]
    fn test_truncate_summary_long() {
        let long = "a".repeat(300);
        let result = truncate_summary(&long, 200);
        assert!(result.len() <= 200);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_consolidation_level_display() {
        assert_eq!(format!("{}", ConsolidationLevel::Micro), "micro");
        assert_eq!(format!("{}", ConsolidationLevel::Light), "light");
        assert_eq!(format!("{}", ConsolidationLevel::Deep), "deep");
        assert_eq!(format!("{}", ConsolidationLevel::Emergency), "emergency");
    }

    #[test]
    fn test_consolidation_priority_scoring() {
        // Fresh, accessed, important item should score high
        let high = consolidation_priority(0, 10, 0.9);
        assert!(high > 0.7, "high priority item scored {}", high);

        // Old, never accessed, unimportant item should score low
        let low = consolidation_priority(30 * 24 * 3600 * 1000, 0, 0.1);
        assert!(low < 0.3, "low priority item scored {}", low);

        // Recency matters: same item, one fresh, one 14 days old
        let fresh = consolidation_priority(0, 5, 0.5);
        let old = consolidation_priority(14 * 24 * 3600 * 1000, 5, 0.5);
        assert!(fresh > old, "fresh ({}) should beat old ({})", fresh, old);
    }

    #[test]
    fn test_consolidation_priority_half_life() {
        // At exactly 7 days, recency factor should be ~0.5
        let half_life_ms = 7 * 24 * 3600 * 1000;
        let score_at_half_life = consolidation_priority(half_life_ms, 0, 0.0);
        // recency_factor at half-life = 0.5, so score = 0.3 * 0.5 + 0.3 * log2(1)/5 + 0.0 = 0.15
        assert!(
            (score_at_half_life - 0.15).abs() < 0.02,
            "score at half-life should be ~0.15, got {}",
            score_at_half_life
        );
    }

    #[test]
    fn test_cluster_episodes_basic() {
        // Create 6 episodes in 2 clear clusters
        let contents = vec![
            "user prefers dark mode in apps".into(),
            "user enabled dark mode in Chrome".into(),
            "user switched to dark theme everywhere".into(),
            "user enjoys cooking pasta for dinner".into(),
            "user made homemade pasta recipe".into(),
            "user cooked Italian pasta with sauce".into(),
        ];

        let clusters = cluster_episodes(&contents, 4);
        // Should find at least 1 cluster (the similarity-based ones)
        // Due to TF-IDF embeddings, clustering may vary, but we should get non-empty results
        assert!(!clusters.is_empty() || contents.len() < MIN_CLUSTER_SIZE,
            "should find at least one cluster from clearly related content");
    }

    #[test]
    fn test_cluster_episodes_empty() {
        let clusters = cluster_episodes(&[], 3);
        assert!(clusters.is_empty());
    }

    #[test]
    fn test_cluster_episodes_too_few() {
        let contents = vec!["just one episode".into()];
        let clusters = cluster_episodes(&contents, 3);
        // With only 1 episode, no cluster can reach MIN_CLUSTER_SIZE
        assert!(clusters.is_empty());
    }

    #[test]
    fn test_extract_concept_hint() {
        let contents = vec![
            "user prefers dark mode settings",
            "user enabled dark mode in Chrome",
            "user set dark mode in VS Code",
        ];
        let hint = extract_concept_hint(&contents);
        // Should extract something related to the common words
        assert!(!hint.is_empty());
    }

    #[test]
    fn test_extract_concept_hint_empty() {
        let hint = extract_concept_hint(&[]);
        assert_eq!(hint, "general pattern");
    }

    #[test]
    fn test_micro_consolidation() {
        let rt = rt();
        let episodic = EpisodicMemory::open_in_memory().unwrap();
        let semantic = SemanticMemory::open_in_memory().unwrap();
        let archive = ArchiveMemory::open_in_memory().unwrap();
        let mut working = WorkingMemory::new();
        let mut patterns = PatternEngine::new();

        // Add some expired slots
        working.push_with_ttl(
            "expired 1".into(),
            aura_types::events::EventSource::Internal,
            0.5,
            now(),
            100,
        );
        working.push_with_ttl(
            "expired 2".into(),
            aura_types::events::EventSource::Internal,
            0.5,
            now(),
            100,
        );
        working.push(
            "still alive".into(),
            aura_types::events::EventSource::UserCommand,
            0.8,
            now() + 200,
        );

        let report = rt.block_on(consolidate(
            ConsolidationLevel::Micro,
            &mut working,
            &episodic,
            &semantic,
            &archive,
            &mut patterns,
            now() + 300,
        ));

        assert_eq!(report.working_slots_swept, 2);
        assert_eq!(working.len(), 1);
    }

    #[test]
    fn test_light_consolidation_promotes() {
        let rt = rt();
        let episodic = EpisodicMemory::open_in_memory().unwrap();
        let semantic = SemanticMemory::open_in_memory().unwrap();
        let archive = ArchiveMemory::open_in_memory().unwrap();
        let mut working = WorkingMemory::new();
        let mut patterns = PatternEngine::new();

        // Add a high-importance slot that should be promoted
        working.push(
            "user said they love dark mode".into(),
            aura_types::events::EventSource::UserCommand,
            0.9, // high importance
            now(),
        );

        // Add a low-importance slot that should NOT be promoted
        working.push(
            "system ping".into(),
            aura_types::events::EventSource::Internal,
            0.1,
            now(),
        );

        let report = rt.block_on(consolidate(
            ConsolidationLevel::Light,
            &mut working,
            &episodic,
            &semantic,
            &archive,
            &mut patterns,
            now() + 1000,
        ));

        assert!(report.working_to_episodic >= 1, "should promote at least 1 slot");

        // Verify the episode was stored
        let count = rt.block_on(episodic.count()).unwrap();
        assert!(count >= 1);

        // Verify pattern was recorded
        assert!(report.patterns_recorded > 0, "should record promotion pattern");
    }

    #[test]
    fn test_emergency_consolidation() {
        let rt = rt();
        let episodic = EpisodicMemory::open_in_memory().unwrap();
        let semantic = SemanticMemory::open_in_memory().unwrap();
        let archive = ArchiveMemory::open_in_memory().unwrap();
        let mut working = WorkingMemory::new();
        let mut patterns = PatternEngine::new();

        // Fill working memory with low-importance slots
        for i in 0..10 {
            working.push(
                format!("low importance item {}", i),
                aura_types::events::EventSource::Internal,
                0.1, // below 0.3 threshold
                now(),
            );
        }

        // Add some old episodes that should be archived
        for i in 0..5 {
            rt.block_on(episodic.store(
                format!("old episode {}", i),
                0.0,
                0.2, // low importance
                vec![],
                now() - 10 * 24 * 60 * 60 * 1000, // 10 days ago
            ))
            .unwrap();
        }

        let report = rt.block_on(consolidate(
            ConsolidationLevel::Emergency,
            &mut working,
            &episodic,
            &semantic,
            &archive,
            &mut patterns,
            now(),
        ));

        // Should have swept working slots
        assert!(report.working_slots_swept > 0);
        // Should have archived old episodes
        assert!(report.episodes_archived > 0);
        assert!(report.bytes_freed > 0);
    }

    #[test]
    fn test_report_default() {
        let report = ConsolidationReport::default();
        assert!(report.level.is_none());
        assert_eq!(report.working_slots_swept, 0);
        assert_eq!(report.working_to_episodic, 0);
        assert_eq!(report.patterns_recorded, 0);
        assert!(report.errors.is_empty());
    }
}
