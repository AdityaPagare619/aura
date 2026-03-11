//! AURA v4 Memory System — 4-tier persistent, intelligent memory.
//!
//! This module ties together all four memory tiers into a single `AuraMemory`
//! orchestrator that provides a unified API for storing, querying, consolidating,
//! and reporting on the entire memory hierarchy.
//!
//! # Architecture
//!
//! | Tier     | Backend           | Budget       | Latency   | Purpose              |
//! |----------|-------------------|--------------|-----------|----------------------|
//! | Working  | RAM ring buffer   | 1MB, 1024    | <1ms      | Current context      |
//! | Episodic | SQLite WAL        | ~18MB/year   | 2-8ms     | Specific experiences |
//! | Semantic | SQLite + FTS5     | ~50MB/year   | 5-15ms    | Learned knowledge    |
//! | Archive  | Passthrough (ZSTD)| ~4MB/year    | 50-200ms  | Old memories         |
//!
//! # Invariants
//!
//! - Memory NEVER loses data — all SQLite stores use WAL mode with atomic writes.
//! - Working memory never exceeds 1024 slots.
//! - Cross-tier queries are merged by relevance and deduplicated by source_id.
//! - Consolidation cascades: micro ⊂ light ⊂ deep; emergency is its own path.

pub mod archive;
pub mod compaction;
pub mod consolidation;
pub mod embeddings;
pub mod episodic;
pub mod feedback;
pub mod hnsw;
pub mod importance;
pub mod patterns;
pub mod semantic;
pub mod working;
pub mod workflows;

// Re-export key types for ergonomic use by the rest of the daemon.
pub use archive::{ArchiveMemory, ARCHIVE_AGE_THRESHOLD_MS, ARCHIVE_IMPORTANCE_THRESHOLD};
pub use compaction::ContextCompactor;
pub use consolidation::{consolidate, ConsolidationLevel, ConsolidationReport};
pub use embeddings::{cosine_similarity, embed, jaccard_trigram_similarity, EMBED_DIM};
pub use episodic::EpisodicMemory;
pub use feedback::FeedbackLoop;
pub use importance::calculate_importance;
pub use patterns::PatternEngine;
pub use semantic::SemanticMemory;
pub use working::{WorkingMemory, WorkingResult, MAX_SLOTS};
pub use workflows::WorkflowMemory;

use std::path::Path;

use tracing::{debug, info, warn};

use aura_types::errors::MemError;
use aura_types::ipc::MemoryTier;
use aura_types::memory::{MemoryQuery, MemoryResult};

// ---------------------------------------------------------------------------
// MemoryUsageReport
// ---------------------------------------------------------------------------

/// Per-tier memory usage breakdown.
#[derive(Debug, Clone, Default)]
pub struct MemoryUsageReport {
    /// Working memory usage in bytes (RAM).
    pub working_bytes: u64,
    /// Episodic memory usage in bytes (SQLite page_count * page_size).
    pub episodic_bytes: u64,
    /// Semantic memory usage in bytes (SQLite page_count * page_size).
    pub semantic_bytes: u64,
    /// Archive memory usage in bytes (compressed, on disk).
    pub archive_bytes: u64,
    /// Archive original (uncompressed) size — useful for compression ratio.
    pub archive_original_bytes: u64,
    /// Sum of all tier bytes (working + episodic + semantic + archive).
    pub total_bytes: u64,
    /// Number of live working memory slots.
    pub working_slot_count: usize,
    /// Number of episodic episodes.
    pub episodic_count: u64,
    /// Number of semantic entries.
    pub semantic_count: u64,
    /// Number of archive blobs.
    pub archive_count: u64,
}

impl std::fmt::Display for MemoryUsageReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Memory: {:.1}KB total | Working: {:.1}KB ({} slots) | \
             Episodic: {:.1}KB ({} eps) | Semantic: {:.1}KB ({} entries) | \
             Archive: {:.1}KB ({} blobs, {:.1}KB uncompressed)",
            self.total_bytes as f64 / 1024.0,
            self.working_bytes as f64 / 1024.0,
            self.working_slot_count,
            self.episodic_bytes as f64 / 1024.0,
            self.episodic_count,
            self.semantic_bytes as f64 / 1024.0,
            self.semantic_count,
            self.archive_bytes as f64 / 1024.0,
            self.archive_count,
            self.archive_original_bytes as f64 / 1024.0,
        )
    }
}

// ---------------------------------------------------------------------------
// PredictionResult
// ---------------------------------------------------------------------------

/// Result of a cross-tier prediction query.
#[derive(Debug, Clone)]
pub struct PredictionResult {
    /// The predicted next action or event name.
    pub predicted_action: String,
    /// Confidence score [0.0, 1.0].
    pub confidence: f32,
    /// The intelligence tier that made the prediction.
    pub source_tier: String, 
}

// ---------------------------------------------------------------------------
// AuraMemory — the unified orchestrator
// ---------------------------------------------------------------------------

/// Unified orchestrator for AURA's 4-tier memory system with intelligence.
///
/// Owns all four memory stores plus pattern discovery and error→learning
/// feedback, and provides:
/// - Unified store/query API that spans tiers
/// - Consolidation delegation (with pattern integration)
/// - LLM context building
/// - Per-tier usage reporting
/// - Pattern-based prediction and error resolution
///
/// # Thread safety
///
/// `WorkingMemory`, `PatternEngine`, and `FeedbackLoop` are owned directly
/// (single-threaded access via `&mut self`).
/// `EpisodicMemory`, `SemanticMemory`, and `ArchiveMemory` each hold
/// `Arc<Mutex<Connection>>` internally, so they are `Send + Sync`.
pub struct AuraMemory {
    pub working: WorkingMemory,
    pub episodic: EpisodicMemory,
    pub semantic: SemanticMemory,
    pub archive: ArchiveMemory,
    pub workflows: WorkflowMemory,
    pub pattern_engine: PatternEngine,
    pub feedback_loop: FeedbackLoop,
}

impl AuraMemory {
    /// Open the full memory system backed by files in `data_dir`.
    ///
    /// Creates three SQLite databases in the directory:
    /// - `episodic.db` — episodic memory
    /// - `semantic.db` — semantic memory
    /// - `archive.db`  — archive memory
    ///
    /// Working memory is always RAM-only.
    pub fn new(data_dir: &Path) -> Result<Self, MemError> {
        // Ensure the data directory exists
        std::fs::create_dir_all(data_dir).map_err(|e| {
            MemError::DatabaseError(format!("failed to create data dir: {}", e))
        })?;

        let episodic = EpisodicMemory::open(&data_dir.join("episodic.db"))?;
        let semantic = SemanticMemory::open(&data_dir.join("semantic.db"))?;
        let archive = ArchiveMemory::open(&data_dir.join("archive.db"))?;
        let workflows = WorkflowMemory::open(&data_dir.join("workflows.db"))?;
        let working = WorkingMemory::new();
        let pattern_engine = PatternEngine::new();
        let feedback_loop = FeedbackLoop::new();

        info!(
            "AuraMemory initialized at {:?} — 4-tier system with intelligence ready",
            data_dir
        );

        Ok(Self {
            working,
            episodic,
            semantic,
            archive,
            workflows,
            pattern_engine,
            feedback_loop,
        })
    }

    /// Create an in-memory instance (for testing). No files are created.
    pub fn new_in_memory() -> Result<Self, MemError> {
        let episodic = EpisodicMemory::open_in_memory()?;
        let semantic = SemanticMemory::open_in_memory()?;
        let archive = ArchiveMemory::open_in_memory()?;
        let workflows = WorkflowMemory::open_in_memory()?;
        let working = WorkingMemory::new();
        let pattern_engine = PatternEngine::new();
        let feedback_loop = FeedbackLoop::new();

        debug!("AuraMemory initialized in-memory (test mode)");

        Ok(Self {
            working,
            episodic,
            semantic,
            archive,
            workflows,
            pattern_engine,
            feedback_loop,
        })
    }

    // -----------------------------------------------------------------------
    // Store
    // -----------------------------------------------------------------------

    /// Store content into working memory (immediate, <1ms).
    ///
    /// This is the primary ingestion point — all new information enters via
    /// working memory. The consolidation engine later promotes important items
    /// to episodic and beyond.
    pub fn store_working(
        &mut self,
        content: String,
        source: aura_types::events::EventSource,
        importance: f32,
        now_ms: u64,
    ) {
        self.working.push(content, source, importance, now_ms);
    }

    /// Store directly into episodic memory (for explicit user experiences).
    ///
    /// Use this when the caller knows the content should bypass working memory
    /// (e.g., a significant user interaction that must be preserved).
    pub async fn store_episodic(
        &self,
        content: String,
        emotional_valence: f32,
        base_importance: f32,
        context_tags: Vec<String>,
        now_ms: u64,
    ) -> Result<u64, MemError> {
        self.episodic
            .store(content, emotional_valence, base_importance, context_tags, now_ms)
            .await
    }

    /// Store directly into semantic memory (for explicit knowledge).
    pub async fn store_semantic(
        &self,
        concept: String,
        knowledge: String,
        confidence: f32,
        source_episodes: Vec<u64>,
        now_ms: u64,
    ) -> Result<u64, MemError> {
        self.semantic
            .store(concept, knowledge, confidence, source_episodes, now_ms)
            .await
    }

    // -----------------------------------------------------------------------
    // Query — cross-tier retrieval
    // -----------------------------------------------------------------------

    /// Query across all requested tiers, merge results by relevance descending.
    ///
    /// This is the primary retrieval API. Results from all tiers specified in
    /// `query.tiers` are fetched in parallel (episodic, semantic, archive are
    /// async; working is synchronous), then merged, deduplicated by source_id,
    /// sorted by relevance descending, and truncated to `query.max_results`.
    pub async fn query(
        &self,
        query: &MemoryQuery,
        now_ms: u64,
    ) -> Result<Vec<MemoryResult>, MemError> {
        let mut all_results: Vec<MemoryResult> = Vec::new();

        for tier in &query.tiers {
            match tier {
                MemoryTier::Working => {
                    let working_results = self.working.query(
                        &query.query_text,
                        query.max_results,
                        now_ms,
                    );
                    for wr in working_results {
                        if wr.score >= query.min_relevance {
                            all_results.push(MemoryResult {
                                content: wr.slot.content.clone(),
                                tier: MemoryTier::Working,
                                relevance: wr.score,
                                importance: wr.slot.importance,
                                timestamp_ms: wr.slot.timestamp_ms,
                                source_id: wr.index as u64,
                            });
                        }
                    }
                }
                MemoryTier::Episodic => {
                    match self
                        .episodic
                        .query(
                            &query.query_text,
                            query.max_results,
                            query.min_relevance,
                            now_ms,
                        )
                        .await
                    {
                        Ok(results) => all_results.extend(results),
                        Err(e) => {
                            warn!("episodic query failed: {}", e);
                        }
                    }
                }
                MemoryTier::Semantic => {
                    match self
                        .semantic
                        .query(
                            &query.query_text,
                            query.max_results,
                            query.min_relevance,
                            now_ms,
                        )
                        .await
                    {
                        Ok(results) => all_results.extend(results),
                        Err(e) => {
                            warn!("semantic query failed: {}", e);
                        }
                    }
                }
                MemoryTier::Archive => {
                    match self
                        .archive
                        .query(
                            &query.query_text,
                            query.max_results,
                            query.min_relevance,
                        )
                        .await
                    {
                        Ok(results) => all_results.extend(results),
                        Err(e) => {
                            warn!("archive query failed: {}", e);
                        }
                    }
                }
            }
        }

        // Deduplicate by (tier, source_id) — keep highest relevance
        all_results = dedup_results(all_results);

        // Sort by relevance descending
        all_results.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Truncate to max_results
        all_results.truncate(query.max_results);

        debug!(
            "cross-tier query '{}' returned {} results from {} tiers",
            &query.query_text[..query.query_text.len().min(40)],
            all_results.len(),
            query.tiers.len(),
        );

        Ok(all_results)
    }

    // -----------------------------------------------------------------------
    // Prediction — cross-tier next-action forecasting
    // -----------------------------------------------------------------------

    /// Predict the user's next action based on recent context and historical patterns.
    ///
    /// Aggregates predictions across memory tiers:
    /// - **Workflows**: matches recent actions against established automation sequences.
    /// - **Temporal Patterns**: checks the pattern engine for immediate A->B event correlates.
    pub async fn predict_next_action(
        &self,
        current_context: &str,
        recent_events: &[String],
    ) -> Result<Vec<PredictionResult>, MemError> {
        let mut predictions = Vec::new();

        // 1. Workflow predictions (long sequences)
        if let Ok(workflows) = self.workflows.get_all().await {
            for (_id, wf) in workflows {
                let seq_strings: Vec<String> = wf.sequence.iter().map(|a| format!("{:?}", a)).collect();
                
                // Simplistic prefix match for prediction matching
                if !recent_events.is_empty() && seq_strings.len() > recent_events.len() {
                    let mut is_prefix = true;
                    for (i, ev) in recent_events.iter().enumerate() {
                        // Very rough comparison, usually we'd want more semantically aware matching
                        if i < seq_strings.len() && !seq_strings[i].contains(ev) && !ev.contains(&seq_strings[i]) {
                            is_prefix = false;
                            break;
                        }
                    }
                    if is_prefix {
                        let next_step = seq_strings[recent_events.len()].clone();
                        let conf = (wf.frequency as f32 / 10.0).clamp(0.4, 0.95);
                        predictions.push(PredictionResult {
                            predicted_action: next_step,
                            confidence: conf,
                            source_tier: "Workflow".to_string(),
                        });
                    }
                }
            }
        }

        // 2. Temporal Pattern Engine (immediate A->B)
        if let Some(last_event) = recent_events.last() {
            let temporal_preds = self.pattern_engine.predict_next_temporal(last_event);
            for (next_event, conf) in temporal_preds {
                predictions.push(PredictionResult {
                    predicted_action: next_event,
                    confidence: conf,
                    source_tier: "TemporalPattern".to_string(),
                });
            }
        }

        // Sort by confidence descending
        predictions.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
        
        // Deduplicate by predicted_action
        let mut unique: Vec<PredictionResult> = Vec::new();
        for p in predictions {
            if !unique.iter().any(|u| u.predicted_action == p.predicted_action) {
                unique.push(p);
            }
        }
        
        // Contextual boost (if current_context contains clues, could boost related predictions - stub for M10)
        let _ = current_context;

        unique.truncate(5); // Return top 5 predictions
        Ok(unique)
    }

    // -----------------------------------------------------------------------
    // Consolidation
    // -----------------------------------------------------------------------

    /// Run consolidation at the specified level.
    ///
    /// Delegates to `consolidation::consolidate()` which cascades through
    /// lower levels as needed. Pattern discovery is integrated — successful
    /// promotions and generalizations are recorded in the pattern engine.
    pub async fn consolidate(
        &mut self,
        level: ConsolidationLevel,
        now_ms: u64,
    ) -> ConsolidationReport {
        let report = consolidation::consolidate(
            level,
            &mut self.working,
            &self.episodic,
            &self.semantic,
            &self.archive,
            &mut self.pattern_engine,
            now_ms,
        )
        .await;

        if !report.errors.is_empty() {
            warn!(
                "consolidation ({}) completed with {} errors",
                level,
                report.errors.len()
            );
        } else {
            debug!(
                "consolidation ({}) completed in {}ms — swept: {}, promoted: {}, archived: {}",
                level,
                report.duration_ms,
                report.working_slots_swept,
                report.working_to_episodic,
                report.episodes_archived,
            );
        }

        report
    }

    // -----------------------------------------------------------------------
    // LLM context building
    // -----------------------------------------------------------------------

    /// Build a prompt-ready context string from the top-k most relevant
    /// memories across working, episodic, and semantic tiers.
    ///
    /// Archive is intentionally excluded from LLM context building because
    /// of its higher latency and lower relevance for real-time conversations.
    /// The caller can include archive tier via `query()` if needed.
    ///
    /// Returns a formatted string suitable for injection into an LLM system
    /// prompt, with section headers per tier.
    pub async fn get_context_for_llm(
        &self,
        query_text: &str,
        max_items: usize,
        now_ms: u64,
    ) -> Result<String, MemError> {
        let mut sections: Vec<String> = Vec::new();

        // ----- Working memory (synchronous, fast) -----
        let working_ctx = self.working.context_for_llm(query_text, max_items, now_ms);
        if !working_ctx.is_empty() {
            sections.push(working_ctx);
        }

        // ----- Episodic memory -----
        let items_per_tier = (max_items / 3).max(2);
        match self
            .episodic
            .query(query_text, items_per_tier, 0.2, now_ms)
            .await
        {
            Ok(results) if !results.is_empty() => {
                let mut sec = String::from("[Episodic Memory]\n");
                for (i, r) in results.iter().enumerate() {
                    sec.push_str(&format!(
                        "{}. (relevance: {:.2}) {}\n",
                        i + 1,
                        r.relevance,
                        r.content,
                    ));
                }
                sections.push(sec);
            }
            Ok(_) => {} // No results
            Err(e) => {
                warn!("episodic query for LLM context failed: {}", e);
            }
        }

        // ----- Semantic memory -----
        match self
            .semantic
            .query(query_text, items_per_tier, 0.2, now_ms)
            .await
        {
            Ok(results) if !results.is_empty() => {
                let mut sec = String::from("[Semantic Memory]\n");
                for (i, r) in results.iter().enumerate() {
                    sec.push_str(&format!(
                        "{}. (confidence: {:.2}) {}\n",
                        i + 1,
                        r.importance, // importance holds confidence for semantic entries
                        r.content,
                    ));
                }
                sections.push(sec);
            }
            Ok(_) => {}
            Err(e) => {
                warn!("semantic query for LLM context failed: {}", e);
            }
        }

        let context = sections.join("\n");
        debug!(
            "built LLM context: {} chars across {} sections",
            context.len(),
            sections.len(),
        );
        Ok(context)
    }

    // -----------------------------------------------------------------------
    // Usage reporting
    // -----------------------------------------------------------------------

    /// Get per-tier memory usage statistics.
    pub async fn memory_usage(&self) -> Result<MemoryUsageReport, MemError> {
        let working_bytes = self.working.memory_usage_bytes() as u64;
        let working_slot_count = self.working.len();

        let episodic_bytes = self.episodic.storage_bytes().await?;
        let episodic_count = self.episodic.count().await?;

        let semantic_bytes = self.semantic.storage_bytes().await?;
        let semantic_count = self.semantic.count().await?;

        // storage_bytes() = SQLite db page size (actual disk footprint)
        // storage_stats() = (sum of compressed blob bytes, sum of original sizes)
        let archive_db_bytes = self.archive.storage_bytes().await?;
        let archive_count = self.archive.count().await?;
        let (_archive_compressed_blob_bytes, archive_original) =
            self.archive.storage_stats().await?;

        let total_bytes = working_bytes + episodic_bytes + semantic_bytes + archive_db_bytes;

        Ok(MemoryUsageReport {
            working_bytes,
            episodic_bytes,
            semantic_bytes,
            archive_bytes: archive_db_bytes,
            archive_original_bytes: archive_original,
            total_bytes,
            working_slot_count,
            episodic_count,
            semantic_count,
            archive_count,
        })
    }
}

// ---------------------------------------------------------------------------
// MemoryIntelligence — unified facade for intelligent memory features
// ---------------------------------------------------------------------------

/// Unified facade providing high-level access to AURA's intelligent memory
/// features: pattern discovery, error→learning feedback, spreading activation,
/// and cross-tier querying.
///
/// This struct borrows `AuraMemory` mutably and provides convenience methods
/// that coordinate across multiple subsystems. It does NOT own any state — it
/// is a short-lived lens over the existing memory system.
///
/// # Usage
///
/// ```ignore
/// let mut mem = AuraMemory::new_in_memory().unwrap();
/// let mut intel = MemoryIntelligence::new(&mut mem);
/// intel.on_action("open_file", "editor context", "file opened", true, now);
/// intel.on_error("io_error", "file not found", "disk context", now);
/// let suggestions = intel.suggest_for_error("io_error", "file not found");
/// ```
pub struct MemoryIntelligence<'a> {
    mem: &'a mut AuraMemory,
}

impl<'a> MemoryIntelligence<'a> {
    /// Create a new intelligence facade over the given memory.
    pub fn new(mem: &'a mut AuraMemory) -> Self {
        Self { mem }
    }

    // -- Pattern integration --

    /// Record an action outcome for pattern learning.
    ///
    /// This is the primary way external callers feed action→outcome data into
    /// the pattern engine. Internally calls `PatternEngine::record_outcome()`
    /// and `hebbian_update()`.
    #[tracing::instrument(skip(self), fields(action, success))]
    pub fn on_action(
        &mut self,
        action: &str,
        context: &str,
        outcome: &str,
        success: bool,
        now_ms: u64,
    ) -> Result<(), aura_types::errors::AuraError> {
        self.mem.pattern_engine.record_outcome(
            action, context, outcome, success, now_ms,
        )?;
        self.mem
            .pattern_engine
            .hebbian_update(action, context, success);

        // Also activate related working memory slots
        self.mem
            .working
            .activate_related(&format!("{} {}", action, context), now_ms);

        Ok(())
    }

    /// Predict the most likely outcome for a given action + context.
    pub fn predict_outcome(&self, action: &str, context: &str) -> Option<(String, f32)> {
        self.mem.pattern_engine.predict_outcome(action, context)
    }

    /// Get the top N strongest patterns the engine has discovered.
    pub fn strongest_patterns(
        &self,
        n: usize,
    ) -> Vec<&patterns::ActionPattern> {
        self.mem.pattern_engine.strongest_patterns(n)
    }

    // -- Error→learning integration --

    /// Record an error event for future learning.
    ///
    /// Returns the error ID which can be used later with `resolve_error()`.
    #[tracing::instrument(skip(self), fields(error_type))]
    pub fn on_error(
        &mut self,
        error_type: &str,
        error_message: &str,
        context: &str,
        now_ms: u64,
    ) -> u64 {
        let error_id = self.mem.feedback_loop.record_error(
            error_type,
            error_message,
            context,
            now_ms,
        );

        // Activate related working memory slots when an error occurs
        self.mem
            .working
            .activate_related(&format!("{} {}", error_type, error_message), now_ms);

        error_id
    }

    /// Record a resolution for a previously recorded error.
    #[tracing::instrument(skip(self), fields(error_id, success))]
    pub fn resolve_error(
        &mut self,
        error_id: u64,
        resolution: &str,
        success: bool,
        now_ms: u64,
    ) -> Result<(), aura_types::errors::AuraError> {
        self.mem
            .feedback_loop
            .resolve_error(error_id, resolution, success, now_ms)
    }

    /// Suggest resolutions for an error based on past experience.
    pub fn suggest_for_error(
        &self,
        error_type: &str,
        error_message: &str,
    ) -> Vec<&feedback::ErrorResolution> {
        self.mem
            .feedback_loop
            .suggest_resolutions(error_type, error_message)
    }

    /// Get the feedback loop's overall effectiveness (0.0..1.0).
    pub fn feedback_effectiveness(&self) -> f32 {
        self.mem.feedback_loop.effectiveness()
    }

    // -- Working memory activation --

    /// Explicitly activate working memory slots related to the given context.
    pub fn activate_context(&mut self, context: &str, now_ms: u64) {
        self.mem.working.activate_related(context, now_ms);
    }

    /// Decay all working memory activations (call periodically).
    pub fn decay_activations(&mut self, now_ms: u64) {
        self.mem.working.decay_activations(now_ms);
    }

    // -- Housekeeping --

    /// Prune old patterns and feedback data.
    ///
    /// Call periodically (e.g., during deep consolidation) to keep bounded
    /// memory usage. Uses 7-day max age for feedback loop.
    pub fn prune(&mut self, now_ms: u64) {
        self.mem.pattern_engine.prune(now_ms);
        // 7 days in ms
        self.mem
            .feedback_loop
            .prune(now_ms, 7 * 24 * 60 * 60 * 1000);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Deduplicate memory results by (tier, source_id), keeping the highest relevance.
fn dedup_results(mut results: Vec<MemoryResult>) -> Vec<MemoryResult> {
    if results.len() <= 1 {
        return results;
    }

    // Sort by (tier discriminant, source_id) to group duplicates adjacently,
    // then by relevance descending within groups so the first of each group wins.
    results.sort_by(|a, b| {
        let tier_a = tier_discriminant(&a.tier);
        let tier_b = tier_discriminant(&b.tier);
        tier_a
            .cmp(&tier_b)
            .then(a.source_id.cmp(&b.source_id))
            .then(
                b.relevance
                    .partial_cmp(&a.relevance)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    results.dedup_by(|a, b| {
        tier_discriminant(&a.tier) == tier_discriminant(&b.tier) && a.source_id == b.source_id
    });

    results
}

/// Map MemoryTier to a u8 for stable sorting/dedup (avoids relying on enum order).
fn tier_discriminant(tier: &MemoryTier) -> u8 {
    match tier {
        MemoryTier::Working => 0,
        MemoryTier::Episodic => 1,
        MemoryTier::Semantic => 2,
        MemoryTier::Archive => 3,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use aura_types::events::EventSource;
    use aura_types::ipc::MemoryTier;
    use aura_types::memory::MemoryQuery;

    // Helper: current time for tests (Jan 1 2025, 00:00:00 UTC in ms)
    const TEST_NOW_MS: u64 = 1_735_689_600_000;

    // -- Unit tests for helpers --

    #[test]
    fn test_tier_discriminant_ordering() {
        assert!(tier_discriminant(&MemoryTier::Working) < tier_discriminant(&MemoryTier::Episodic));
        assert!(
            tier_discriminant(&MemoryTier::Episodic) < tier_discriminant(&MemoryTier::Semantic)
        );
        assert!(tier_discriminant(&MemoryTier::Semantic) < tier_discriminant(&MemoryTier::Archive));
    }

    #[test]
    fn test_dedup_results_empty() {
        let results: Vec<MemoryResult> = vec![];
        let deduped = dedup_results(results);
        assert!(deduped.is_empty());
    }

    #[test]
    fn test_dedup_results_no_duplicates() {
        let results = vec![
            MemoryResult {
                content: "a".into(),
                tier: MemoryTier::Working,
                relevance: 0.9,
                importance: 0.5,
                timestamp_ms: TEST_NOW_MS,
                source_id: 1,
            },
            MemoryResult {
                content: "b".into(),
                tier: MemoryTier::Episodic,
                relevance: 0.8,
                importance: 0.6,
                timestamp_ms: TEST_NOW_MS,
                source_id: 2,
            },
        ];
        let deduped = dedup_results(results);
        assert_eq!(deduped.len(), 2);
    }

    #[test]
    fn test_dedup_results_keeps_highest_relevance() {
        let results = vec![
            MemoryResult {
                content: "a-low".into(),
                tier: MemoryTier::Episodic,
                relevance: 0.3,
                importance: 0.5,
                timestamp_ms: TEST_NOW_MS,
                source_id: 42,
            },
            MemoryResult {
                content: "a-high".into(),
                tier: MemoryTier::Episodic,
                relevance: 0.9,
                importance: 0.5,
                timestamp_ms: TEST_NOW_MS,
                source_id: 42,
            },
        ];
        let deduped = dedup_results(results);
        assert_eq!(deduped.len(), 1);
        assert!((deduped[0].relevance - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_dedup_allows_same_source_id_different_tiers() {
        let results = vec![
            MemoryResult {
                content: "working-1".into(),
                tier: MemoryTier::Working,
                relevance: 0.7,
                importance: 0.5,
                timestamp_ms: TEST_NOW_MS,
                source_id: 1,
            },
            MemoryResult {
                content: "episodic-1".into(),
                tier: MemoryTier::Episodic,
                relevance: 0.8,
                importance: 0.6,
                timestamp_ms: TEST_NOW_MS,
                source_id: 1,
            },
        ];
        let deduped = dedup_results(results);
        // Same source_id but different tiers → both kept
        assert_eq!(deduped.len(), 2);
    }

    #[test]
    fn test_memory_usage_report_display() {
        let report = MemoryUsageReport {
            working_bytes: 1024,
            episodic_bytes: 2048,
            semantic_bytes: 4096,
            archive_bytes: 512,
            archive_original_bytes: 1024,
            total_bytes: 7680,
            working_slot_count: 5,
            episodic_count: 10,
            semantic_count: 3,
            archive_count: 2,
        };
        let display = format!("{}", report);
        assert!(display.contains("Memory:"));
        assert!(display.contains("Working:"));
        assert!(display.contains("5 slots"));
        assert!(display.contains("10 eps"));
    }

    // -- Integration tests (async, in-memory) --

    #[tokio::test]
    async fn test_new_in_memory() {
        let mem = AuraMemory::new_in_memory().expect("in-memory init should work");
        assert!(mem.working.is_empty());
    }

    #[tokio::test]
    async fn test_store_and_query_working_only() {
        let mut mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;

        mem.store_working(
            "The user likes rust programming".into(),
            EventSource::UserCommand,
            0.8,
            now,
        );
        mem.store_working(
            "Weather is sunny today".into(),
            EventSource::Notification,
            0.3,
            now + 100,
        );

        let query = MemoryQuery {
            query_text: "rust programming".into(),
            max_results: 5,
            min_relevance: 0.0,
            tiers: vec![MemoryTier::Working],
            time_range: None,
        };

        let results = mem.query(&query, now + 200).await.unwrap();
        assert!(!results.is_empty());
        // The rust-related entry should be more relevant
        assert!(results[0].content.contains("rust"));
    }

    #[tokio::test]
    async fn test_store_and_query_episodic() {
        let mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;

        let id = mem
            .store_episodic(
                "User asked about machine learning basics".into(),
                0.5,
                0.7,
                vec!["ml".into(), "learning".into()],
                now,
            )
            .await
            .unwrap();
        assert!(id > 0);

        let query = MemoryQuery {
            query_text: "machine learning".into(),
            max_results: 5,
            min_relevance: 0.0,
            tiers: vec![MemoryTier::Episodic],
            time_range: None,
        };

        let results = mem.query(&query, now + 100).await.unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].tier, MemoryTier::Episodic);
    }

    #[tokio::test]
    async fn test_store_and_query_semantic() {
        let mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;

        let id = mem
            .store_semantic(
                "Rust ownership".into(),
                "Rust uses ownership with borrowing to manage memory safely".into(),
                0.85,
                vec![],
                now,
            )
            .await
            .unwrap();
        assert!(id > 0);

        let query = MemoryQuery {
            query_text: "ownership borrowing memory".into(),
            max_results: 5,
            min_relevance: 0.0,
            tiers: vec![MemoryTier::Semantic],
            time_range: None,
        };

        let results = mem.query(&query, now + 100).await.unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].tier, MemoryTier::Semantic);
    }

    #[tokio::test]
    async fn test_cross_tier_query() {
        let mut mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;

        // Populate all three live tiers
        mem.store_working(
            "working: rust patterns".into(),
            EventSource::UserCommand,
            0.6,
            now,
        );
        mem.store_episodic(
            "episodic: discussed rust design patterns".into(),
            0.3,
            0.5,
            vec!["rust".into()],
            now,
        )
        .await
        .unwrap();
        mem.store_semantic(
            "Rust patterns".into(),
            "Common Rust design patterns include builder, newtype, and typestate".into(),
            0.8,
            vec![],
            now,
        )
        .await
        .unwrap();

        let query = MemoryQuery {
            query_text: "rust patterns".into(),
            max_results: 10,
            min_relevance: 0.0,
            tiers: vec![
                MemoryTier::Working,
                MemoryTier::Episodic,
                MemoryTier::Semantic,
            ],
            time_range: None,
        };

        let results = mem.query(&query, now + 100).await.unwrap();
        // Should have results from multiple tiers
        assert!(results.len() >= 2);

        // Results should be sorted by relevance descending
        for window in results.windows(2) {
            assert!(window[0].relevance >= window[1].relevance);
        }
    }

    #[tokio::test]
    async fn test_consolidate_micro() {
        let mut mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;

        // Add a slot that will expire
        mem.working.push_with_ttl(
            "ephemeral".into(),
            EventSource::Internal,
            0.1,
            now,
            1000, // 1 second TTL
        );
        assert_eq!(mem.working.len(), 1);

        // Consolidate after TTL expires
        let report = mem
            .consolidate(ConsolidationLevel::Micro, now + 2000)
            .await;
        assert_eq!(report.working_slots_swept, 1);
        assert_eq!(mem.working.len(), 0);
    }

    #[tokio::test]
    async fn test_get_context_for_llm_empty() {
        let mem = AuraMemory::new_in_memory().unwrap();
        let ctx = mem
            .get_context_for_llm("anything", 5, TEST_NOW_MS)
            .await
            .unwrap();
        assert!(ctx.is_empty());
    }

    #[tokio::test]
    async fn test_get_context_for_llm_with_data() {
        let mut mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;

        mem.store_working(
            "User prefers dark mode".into(),
            EventSource::UserCommand,
            0.9,
            now,
        );
        mem.store_episodic(
            "User asked to enable dark mode across all apps".into(),
            0.4,
            0.7,
            vec!["ui".into(), "preferences".into()],
            now,
        )
        .await
        .unwrap();

        let ctx = mem
            .get_context_for_llm("dark mode preferences", 5, now + 100)
            .await
            .unwrap();

        assert!(ctx.contains("[Working Memory]"));
        assert!(ctx.contains("dark mode"));
    }

    #[tokio::test]
    async fn test_memory_usage_report() {
        let mut mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;

        mem.store_working(
            "test content".into(),
            EventSource::Internal,
            0.5,
            now,
        );

        let report = mem.memory_usage().await.unwrap();
        assert!(report.working_bytes > 0);
        assert_eq!(report.working_slot_count, 1);
        assert!(report.total_bytes > 0);
    }

    #[tokio::test]
    async fn test_query_max_results_truncation() {
        let mut mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;

        // Push many items
        for i in 0..20 {
            mem.store_working(
                format!("rust pattern number {}", i),
                EventSource::Internal,
                0.5 + (i as f32 * 0.01),
                now + i * 10,
            );
        }

        let query = MemoryQuery {
            query_text: "rust pattern".into(),
            max_results: 3,
            min_relevance: 0.0,
            tiers: vec![MemoryTier::Working],
            time_range: None,
        };

        let results = mem.query(&query, now + 500).await.unwrap();
        assert!(results.len() <= 3);
    }

    #[tokio::test]
    async fn test_query_min_relevance_filter() {
        let mut mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;

        mem.store_working(
            "completely unrelated content about cooking recipes".into(),
            EventSource::Internal,
            0.5,
            now,
        );

        let query = MemoryQuery {
            query_text: "quantum physics research papers".into(),
            max_results: 10,
            min_relevance: 0.9, // Very high threshold
            tiers: vec![MemoryTier::Working],
            time_range: None,
        };

        let results = mem.query(&query, now + 100).await.unwrap();
        // With a very high relevance threshold, unrelated content should be filtered
        for r in &results {
            assert!(r.relevance >= 0.9);
        }
    }

    // -- MemoryIntelligence facade tests --

    #[test]
    fn test_intelligence_on_action() {
        let mut mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;
        {
            let mut intel = MemoryIntelligence::new(&mut mem);
            intel
                .on_action("open_file", "editor", "file opened", true, now)
                .unwrap();
            intel
                .on_action("open_file", "editor", "file opened", true, now + 1000)
                .unwrap();
        }
        assert!(mem.pattern_engine.action_pattern_count() > 0);
    }

    #[test]
    fn test_intelligence_predict_outcome() {
        let mut mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;
        {
            let mut intel = MemoryIntelligence::new(&mut mem);
            // Record several outcomes to build a pattern
            for i in 0..5 {
                intel
                    .on_action(
                        "compile",
                        "rust project",
                        "success",
                        true,
                        now + i * 1000,
                    )
                    .unwrap();
            }
            let prediction = intel.predict_outcome("compile", "rust project");
            assert!(prediction.is_some(), "should predict after 5 outcomes");
            let (outcome, confidence) = prediction.unwrap();
            assert_eq!(outcome, "success");
            assert!(confidence > 0.0);
        }
    }

    #[test]
    fn test_intelligence_error_lifecycle() {
        let mut mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;
        {
            let mut intel = MemoryIntelligence::new(&mut mem);

            // Record error
            let err_id = intel.on_error("io_error", "file not found", "disk", now);

            // No suggestions yet
            let sug = intel.suggest_for_error("io_error", "file not found");
            assert!(sug.is_empty());

            // Resolve it
            intel
                .resolve_error(err_id, "check file path first", true, now + 5000)
                .unwrap();

            // Now should suggest
            let sug = intel.suggest_for_error("io_error", "file not found");
            assert!(
                !sug.is_empty(),
                "should suggest resolution after learning from it"
            );
            assert_eq!(sug[0].resolution_strategy, "check file path first");
        }
    }

    #[test]
    fn test_intelligence_feedback_effectiveness() {
        let mut mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;
        {
            let mut intel = MemoryIntelligence::new(&mut mem);

            // No data → 0.0
            assert!((intel.feedback_effectiveness() - 0.0).abs() < f32::EPSILON);

            // Record and resolve successfully
            let id = intel.on_error("test_err", "test msg", "ctx", now);
            intel
                .resolve_error(id, "fixed it", true, now + 1000)
                .unwrap();

            assert!(intel.feedback_effectiveness() > 0.0);
        }
    }

    #[test]
    fn test_intelligence_activate_context() {
        let mut mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;
        mem.store_working(
            "rust ownership and borrowing concepts".into(),
            EventSource::UserCommand,
            0.8,
            now,
        );

        {
            let mut intel = MemoryIntelligence::new(&mut mem);
            intel.activate_context("rust ownership borrowing", now + 100);
        }

        // Check that activation was applied
        let active = mem.working.active_slots();
        assert!(
            !active.is_empty(),
            "should have activated at least one slot"
        );
    }

    #[test]
    fn test_intelligence_prune() {
        let mut mem = AuraMemory::new_in_memory().unwrap();
        let now = TEST_NOW_MS;
        {
            let mut intel = MemoryIntelligence::new(&mut mem);
            intel
                .on_action("old_action", "ctx", "result", true, now)
                .unwrap();
            // Prune way in the future
            intel.prune(now + 30 * 24 * 60 * 60 * 1000);
        }
        // Should not panic, patterns with old data may be pruned
    }
}
