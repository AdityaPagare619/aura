//! Semantic memory — SQLite + FTS5 knowledge store with Reciprocal Rank Fusion.
//!
//! Budget: ~50MB/year, 5-15ms latency.
//!
//! Stores distilled knowledge and learned concepts. Uses a dual retrieval
//! strategy combining FTS5 full-text search with embedding cosine similarity
//! via Reciprocal Rank Fusion (RRF):
//!
//!   RRF_score(d) = Σ 1 / (k + rank_i(d))   where k = 60
//!
//! Also implements:
//! - Knowledge reinforcement: repeated encounters boost confidence
//! - Generalization: when 3+ episodes share a theme, create a semantic entry with confidence =
//!   min(0.95, 0.5 + num_episodes * 0.1)
//!
//! Embedding similarity search uses an HNSW approximate nearest neighbor index
//! instead of O(n) linear scan, providing sub-linear query time.

use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::Arc,
};

use aura_types::{
    errors::MemError,
    ipc::MemoryTier,
    memory::{MemoryResult, SemanticEntry},
};
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::memory::{
    embeddings::{cosine_similarity, embed, embedding_from_bytes, embedding_to_bytes, EMBED_DIM},
    hnsw::HnswIndex,
    importance,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// RRF constant k — controls how much rank differences are dampened.
/// Higher k = more dampening (less aggressive re-ranking).
const RRF_K: f32 = 60.0;

/// Minimum number of similar episodes before generalization is triggered.
const GENERALIZATION_MIN_EPISODES: usize = 3;

/// Similarity threshold for considering episodes "about the same concept."
/// Raised from 0.15→0.35 to reduce false generalizations with 384-dim
/// sign-hashed TF-IDF embeddings where unrelated content easily exceeds 0.15.
/// See: AURA-V4-BATCH7-MEMORY-INFERENCE-AUDIT §5.1 #1.
const GENERALIZATION_SIMILARITY: f32 = 0.35;

/// Maximum number of HNSW candidates to retrieve before RRF fusion.
const HNSW_SEARCH_LIMIT: usize = 100;

/// ef parameter for HNSW search (controls recall vs speed tradeoff).
const HNSW_EF_SEARCH: usize = 50;

/// Maximum buffered retrieval feedback events before oldest are dropped.
const FEEDBACK_BUFFER_CAPACITY: usize = 100;

// ---------------------------------------------------------------------------
// RetrievalFeedbackBuffer — runtime-only, not persisted in checkpoint
// ---------------------------------------------------------------------------

/// A single retrieval event recorded for consolidation weight adjustment.
#[derive(Debug, Clone)]
pub struct RetrievalEvent {
    pub stored_ms: u64,
    pub retrieved_ms: u64,
}

/// Bounded ring-buffer of retrieval events. Runtime-only — never serialized.
///
/// Filled on every successful query/find_by_concept call. Drained during
/// `cron_handle_dreaming` to drive `ConsolidationWeights::adjust_from_outcome`.
#[derive(Debug)]
pub struct RetrievalFeedbackBuffer {
    events: VecDeque<RetrievalEvent>,
}

impl Default for RetrievalFeedbackBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl RetrievalFeedbackBuffer {
    pub fn new() -> Self {
        Self {
            events: VecDeque::with_capacity(FEEDBACK_BUFFER_CAPACITY),
        }
    }

    /// Push a retrieval event. If at capacity, drops the oldest event.
    pub fn push(&mut self, event: RetrievalEvent) {
        if self.events.len() >= FEEDBACK_BUFFER_CAPACITY {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    /// Drain all buffered events for processing during consolidation.
    pub fn drain(&mut self) -> Vec<RetrievalEvent> {
        self.events.drain(..).collect()
    }

    /// Number of pending events.
    pub fn len(&self) -> usize {
        self.events.len()
    }
}

// ---------------------------------------------------------------------------
// HnswState — HNSW index + bidirectional ID maps
// ---------------------------------------------------------------------------

/// Holds the HNSW index and bidirectional maps between HNSW NodeIds and SQLite row IDs.
pub(crate) struct HnswState {
    index: HnswIndex,
    /// HNSW NodeId (u32) → SQLite row ID (u64)
    node_to_sqlite: HashMap<u32, u64>,
    /// SQLite row ID (u64) → HNSW NodeId (u32)
    sqlite_to_node: HashMap<u64, u32>,
}

impl HnswState {
    fn new() -> Self {
        Self {
            index: HnswIndex::new(EMBED_DIM),
            node_to_sqlite: HashMap::new(),
            sqlite_to_node: HashMap::new(),
        }
    }

    /// Insert an embedding and register the SQLite↔NodeId mapping.
    fn insert(&mut self, sqlite_id: u64, embedding: &[f32]) -> Result<u32, MemError> {
        let node_id = self
            .index
            .insert(embedding)
            .map_err(|e| MemError::DatabaseError(format!("HNSW insert failed: {}", e)))?;
        self.node_to_sqlite.insert(node_id, sqlite_id);
        self.sqlite_to_node.insert(sqlite_id, node_id);
        Ok(node_id)
    }

    /// Search for nearest neighbors, returning (SQLite row ID, similarity) pairs.
    fn search(&mut self, query: &[f32], k: usize, ef: usize) -> Result<Vec<(u64, f32)>, MemError> {
        let results = self
            .index
            .search(query, k, ef)
            .map_err(|e| MemError::QueryFailed(format!("HNSW search failed: {}", e)))?;

        let mut mapped = Vec::with_capacity(results.len());
        for (node_id, similarity) in results {
            if let Some(&sqlite_id) = self.node_to_sqlite.get(&node_id) {
                mapped.push((sqlite_id, similarity));
            }
        }
        Ok(mapped)
    }

    /// Remove a SQLite entry from the HNSW index (lazy delete via tombstone).
    #[allow(dead_code)]
    fn remove(&mut self, sqlite_id: u64) -> Result<(), MemError> {
        if let Some(node_id) = self.sqlite_to_node.remove(&sqlite_id) {
            self.node_to_sqlite.remove(&node_id);
            self.index
                .delete(node_id)
                .map_err(|e| MemError::DatabaseError(format!("HNSW delete failed: {}", e)))?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SemanticMemory
// ---------------------------------------------------------------------------

/// SQLite + FTS5 backed semantic memory store with HNSW similarity index.
pub struct SemanticMemory {
    conn: Arc<Mutex<Connection>>,
    hnsw: Arc<std::sync::Mutex<HnswState>>,
    /// Bounded ring-buffer of retrieval events. Drained during consolidation
    /// to drive `ConsolidationWeights::adjust_from_outcome`. Never persisted.
    pub feedback: Arc<std::sync::Mutex<RetrievalFeedbackBuffer>>,
}

impl SemanticMemory {
    /// Open (or create) a semantic memory database at the given path.
    pub fn open(db_path: &Path) -> Result<Self, MemError> {
        let conn = Connection::open(db_path)
            .map_err(|e| MemError::DatabaseError(format!("failed to open semantic db: {}", e)))?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA cache_size=-4000;",
        )
        .map_err(|e| MemError::DatabaseError(format!("pragmas failed: {}", e)))?;

        run_migrations(&conn)?;
        let hnsw_state = build_hnsw_from_db(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            hnsw: Arc::new(std::sync::Mutex::new(hnsw_state)),
            feedback: Arc::new(std::sync::Mutex::new(RetrievalFeedbackBuffer::new())),
        })
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self, MemError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| MemError::DatabaseError(format!("in-memory open failed: {}", e)))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| MemError::DatabaseError(format!("pragmas failed: {}", e)))?;

        run_migrations(&conn)?;
        let hnsw_state = build_hnsw_from_db(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            hnsw: Arc::new(std::sync::Mutex::new(hnsw_state)),
            feedback: Arc::new(std::sync::Mutex::new(RetrievalFeedbackBuffer::new())),
        })
    }

    /// Store a new semantic entry. Returns the assigned ID.
    pub async fn store(
        &self,
        concept: String,
        knowledge: String,
        confidence: f32,
        source_episodes: Vec<u64>,
        now_ms: u64,
    ) -> Result<u64, MemError> {
        let conn = self.conn.clone();
        let hnsw = self.hnsw.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            store_semantic_sync(
                &conn,
                &hnsw,
                &concept,
                &knowledge,
                confidence,
                &source_episodes,
                now_ms,
            )
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Query semantic memory using RRF (Reciprocal Rank Fusion) of FTS5 + embedding similarity.
    ///
    /// 1. FTS5 full-text search → ranked list R1
    /// 2. Embedding cosine similarity (via HNSW) → ranked list R2
    /// 3. RRF_score(d) = 1/(k + rank_R1(d)) + 1/(k + rank_R2(d))
    pub async fn query(
        &self,
        query_text: &str,
        max_results: usize,
        min_relevance: f32,
        now_ms: u64,
    ) -> Result<Vec<MemoryResult>, MemError> {
        let conn = self.conn.clone();
        let hnsw = self.hnsw.clone();
        let query_text = query_text.to_string();

        let results = tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            query_semantic_rrf(&conn, &hnsw, &query_text, max_results, min_relevance)
        })
        .await
        .map_err(|e| MemError::QueryFailed(format!("spawn_blocking failed: {}", e)))??;

        // Record retrieval feedback for consolidation weight adjustment. Non-fatal.
        if !results.is_empty() {
            if let Ok(mut buf) = self.feedback.lock() {
                for r in &results {
                    buf.push(RetrievalEvent {
                        stored_ms: r.timestamp_ms,
                        retrieved_ms: now_ms,
                    });
                }
            } else {
                tracing::warn!(
                    "semantic feedback buffer lock poisoned — skipping feedback recording"
                );
            }
        }

        Ok(results)
    }

    /// Query semantic memory with Hebbian re-ranking.
    ///
    /// Performs standard RRF retrieval via [`query`], then boosts each result
    /// whose `source_id` has a Hebbian co-occurrence with any of the
    /// `recently_retrieved` memory IDs:
    ///
    ///   final_score = rrf_score + hebbian_boost * 0.2
    ///
    /// `pattern_engine` must be the shared `PatternEngine` instance.  The
    /// method also calls `record_co_retrieval` on the engine so that this
    /// retrieval is itself Hebbianly recorded.
    ///
    /// `max_results` controls the number of results returned (same as `query`).
    pub async fn retrieve_with_hebbian(
        &self,
        query_text: &str,
        recently_retrieved: &[u64],
        max_results: usize,
        min_relevance: f32,
        now_ms: u64,
        pattern_engine: &mut crate::memory::patterns::PatternEngine,
    ) -> Result<Vec<aura_types::memory::MemoryResult>, MemError> {
        // 1. Standard RRF retrieval.
        let mut results = self
            .query(query_text, max_results, min_relevance, now_ms)
            .await?;

        // 2. Apply Hebbian boost.
        if !recently_retrieved.is_empty() {
            for result in &mut results {
                let boost =
                    pattern_engine.get_association_boost(result.source_id, recently_retrieved);
                if boost > 0.0 {
                    result.relevance = (result.relevance + boost * 0.2).clamp(0.0, 1.0);
                }
            }
            // Re-sort by updated relevance scores.
            results.sort_by(|a, b| {
                b.relevance
                    .partial_cmp(&a.relevance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        // 3. Record co-retrieval so future retrievals benefit.
        let returned_ids: Vec<u64> = results.iter().map(|r| r.source_id).collect();
        if !returned_ids.is_empty() {
            pattern_engine.record_co_retrieval(&returned_ids);
        }

        Ok(results)
    }

    /// Reinforce an existing semantic entry — increases confidence and updates
    /// last_reinforced timestamp.
    pub async fn reinforce(
        &self,
        entry_id: u64,
        additional_episode_id: Option<u64>,
        now_ms: u64,
    ) -> Result<(), MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            reinforce_sync(&conn, entry_id, additional_episode_id, now_ms)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Attempt to generalize from a set of similar episodes.
    ///
    /// If episodes share a common concept (determined by embedding clustering),
    /// creates a new semantic entry with generalized knowledge.
    ///
    /// Returns Some(id) if a new entry was created, None otherwise.
    pub async fn try_generalize(
        &self,
        episodes: &[(String, f32)], // (content, importance)
        concept_hint: &str,
        now_ms: u64,
    ) -> Result<Option<u64>, MemError> {
        if episodes.len() < GENERALIZATION_MIN_EPISODES {
            return Ok(None);
        }

        let conn = self.conn.clone();
        let hnsw = self.hnsw.clone();
        let episodes = episodes.to_vec();
        let concept_hint = concept_hint.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            try_generalize_sync(&conn, &hnsw, &episodes, &concept_hint, now_ms)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// LLM-powered generalization. Sends raw episodes to the neocortex for
    /// semantic synthesis. Falls back to the TF-IDF sync path if neocortex
    /// is unavailable.
    ///
    /// # Architecture
    /// **LLM = brain.** The neocortex synthesizes meaning; this method only
    /// ferries data to it and stores the result.
    pub async fn try_generalize_with_llm(
        &self,
        episodes: &[(String, f32)],
        concept_hint: &str,
        now_ms: u64,
        neocortex: &mut crate::ipc::NeocortexClient,
    ) -> Result<Option<u64>, MemError> {
        if episodes.len() < GENERALIZATION_MIN_EPISODES {
            return Ok(None);
        }

        let conn_guard = self.conn.lock().await;
        try_generalize_via_llm(
            &conn_guard,
            &self.hnsw,
            episodes,
            concept_hint,
            now_ms,
            neocortex,
        )
        .await
    }

    /// Get a specific semantic entry by ID.
    pub async fn get(&self, entry_id: u64) -> Result<Option<SemanticEntry>, MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            conn.query_row(
                "SELECT id, concept, knowledge, confidence, source_episodes,
                        created_ms, last_reinforced_ms, access_count
                 FROM semantic_entries WHERE id = ?1",
                params![entry_id as i64],
                row_to_semantic_entry,
            )
            .optional()
            .map_err(|e| MemError::QueryFailed(format!("get semantic failed: {}", e)))
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Count total semantic entries.
    pub async fn count(&self) -> Result<u64, MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM semantic_entries", [], |row| {
                    row.get(0)
                })
                .map_err(|e| MemError::DatabaseError(format!("count failed: {}", e)))?;
            Ok(count as u64)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Find semantic entries that match a concept (for reinforcement checking).
    pub async fn find_by_concept(
        &self,
        concept: &str,
        min_similarity: f32,
        limit: usize,
    ) -> Result<Vec<SemanticEntry>, MemError> {
        let conn = self.conn.clone();
        let hnsw = self.hnsw.clone();
        let concept = concept.to_string();

        let results = tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            find_by_concept_sync(&conn, &hnsw, &concept, min_similarity, limit)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))??;

        // Record retrieval feedback for consolidation weight adjustment. Non-fatal.
        if !results.is_empty() {
            let retrieved_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if let Ok(mut buf) = self.feedback.lock() {
                for entry in &results {
                    buf.push(RetrievalEvent {
                        stored_ms: entry.created_ms,
                        retrieved_ms,
                    });
                }
            } else {
                tracing::warn!(
                    "semantic feedback buffer lock poisoned — skipping find_by_concept feedback"
                );
            }
        }

        Ok(results)
    }

    /// Record an access to a semantic entry.
    pub async fn record_access(&self, entry_id: u64) -> Result<(), MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            conn.execute(
                "UPDATE semantic_entries SET access_count = access_count + 1 WHERE id = ?1",
                params![entry_id as i64],
            )
            .map_err(|e| MemError::DatabaseError(format!("access update failed: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Estimate storage size in bytes.
    pub async fn storage_bytes(&self) -> Result<u64, MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let page_count: i64 = conn
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .map_err(|e| MemError::DatabaseError(format!("page_count failed: {}", e)))?;
            let page_size: i64 = conn
                .query_row("PRAGMA page_size", [], |row| row.get(0))
                .map_err(|e| MemError::DatabaseError(format!("page_size failed: {}", e)))?;
            Ok((page_count * page_size) as u64)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }
}

// ---------------------------------------------------------------------------
// Migrations
// ---------------------------------------------------------------------------

fn run_migrations(conn: &Connection) -> Result<(), MemError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS semantic_entries (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            concept             TEXT NOT NULL,
            knowledge           TEXT NOT NULL,
            confidence          REAL NOT NULL DEFAULT 0.5,
            source_episodes     TEXT NOT NULL DEFAULT '[]',
            created_ms          INTEGER NOT NULL,
            last_reinforced_ms  INTEGER NOT NULL,
            access_count        INTEGER NOT NULL DEFAULT 0,
            embedding           BLOB
        );

        CREATE INDEX IF NOT EXISTS idx_semantic_confidence
            ON semantic_entries(confidence);
        CREATE INDEX IF NOT EXISTS idx_semantic_created
            ON semantic_entries(created_ms);
        CREATE INDEX IF NOT EXISTS idx_semantic_last_reinforced
            ON semantic_entries(last_reinforced_ms);",
    )
    .map_err(|e| MemError::MigrationFailed(format!("semantic table creation failed: {}", e)))?;

    // Create FTS5 virtual table for full-text search on concept + knowledge
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS semantic_fts USING fts5(
            concept,
            knowledge,
            content='semantic_entries',
            content_rowid='id'
        );

        -- Triggers to keep FTS5 in sync with the main table
        CREATE TRIGGER IF NOT EXISTS semantic_fts_insert AFTER INSERT ON semantic_entries
        BEGIN
            INSERT INTO semantic_fts(rowid, concept, knowledge)
            VALUES (new.id, new.concept, new.knowledge);
        END;

        CREATE TRIGGER IF NOT EXISTS semantic_fts_delete AFTER DELETE ON semantic_entries
        BEGIN
            INSERT INTO semantic_fts(semantic_fts, rowid, concept, knowledge)
            VALUES ('delete', old.id, old.concept, old.knowledge);
        END;

        CREATE TRIGGER IF NOT EXISTS semantic_fts_update AFTER UPDATE ON semantic_entries
        BEGIN
            INSERT INTO semantic_fts(semantic_fts, rowid, concept, knowledge)
            VALUES ('delete', old.id, old.concept, old.knowledge);
            INSERT INTO semantic_fts(rowid, concept, knowledge)
            VALUES (new.id, new.concept, new.knowledge);
        END;",
    )
    .map_err(|e| MemError::MigrationFailed(format!("FTS5 setup failed: {}", e)))?;

    info!("semantic memory: migrations complete");
    Ok(())
}

// ---------------------------------------------------------------------------
// HNSW index builder — loads all existing embeddings from SQLite
// ---------------------------------------------------------------------------

/// Build an HNSW index from all existing embeddings in the database.
/// Called once during `open()` / `open_in_memory()`.
fn build_hnsw_from_db(conn: &Connection) -> Result<HnswState, MemError> {
    let mut state = HnswState::new();

    let mut stmt = conn
        .prepare("SELECT id, embedding FROM semantic_entries WHERE embedding IS NOT NULL")
        .map_err(|e| MemError::DatabaseError(format!("HNSW rebuild prepare failed: {}", e)))?;

    let rows = stmt
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((id as u64, blob))
        })
        .map_err(|e| MemError::DatabaseError(format!("HNSW rebuild query failed: {}", e)))?;

    let mut loaded = 0u64;
    let mut skipped = 0u64;
    for row in rows {
        let (sqlite_id, blob) =
            row.map_err(|e| MemError::DatabaseError(format!("HNSW rebuild row failed: {}", e)))?;
        let emb = embedding_from_bytes(&blob);
        if emb.len() == EMBED_DIM {
            state.insert(sqlite_id, &emb)?;
            loaded += 1;
        } else {
            // Stale embedding with wrong dimensionality — skip but don't fail
            skipped += 1;
        }
    }

    if skipped > 0 {
        warn!(
            "HNSW rebuild: loaded {} embeddings, skipped {} (wrong dimension)",
            loaded, skipped
        );
    } else {
        info!("HNSW index rebuilt with {} embeddings", loaded);
    }

    Ok(state)
}

// ---------------------------------------------------------------------------
// Synchronous helpers
// ---------------------------------------------------------------------------

fn store_semantic_sync(
    conn: &Connection,
    hnsw: &std::sync::Mutex<HnswState>,
    concept: &str,
    knowledge: &str,
    confidence: f32,
    source_episodes: &[u64],
    now_ms: u64,
) -> Result<u64, MemError> {
    // Compute embedding from concept + knowledge combined
    let combined = format!("{} {}", concept, knowledge);
    let embedding = embed(&combined);
    let embedding_bytes = embedding_to_bytes(&embedding);

    let episodes_json = serde_json::to_string(source_episodes).unwrap_or_else(|_| "[]".into());

    conn.execute(
        "INSERT INTO semantic_entries (concept, knowledge, confidence, source_episodes,
                                       created_ms, last_reinforced_ms, access_count, embedding)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0, ?6)",
        params![
            concept,
            knowledge,
            confidence,
            episodes_json,
            now_ms as i64,
            embedding_bytes,
        ],
    )
    .map_err(|e| MemError::DatabaseError(format!("semantic insert failed: {}", e)))?;

    let sqlite_id = conn.last_insert_rowid() as u64;

    // Insert into HNSW index
    {
        let mut state = hnsw
            .lock()
            .map_err(|_| MemError::DatabaseError("HNSW lock poisoned".into()))?;
        state.insert(sqlite_id, &embedding)?;
    }

    debug!("stored semantic entry {} (concept: {})", sqlite_id, concept);
    Ok(sqlite_id)
}

fn query_semantic_rrf(
    conn: &Connection,
    hnsw: &std::sync::Mutex<HnswState>,
    query_text: &str,
    max_results: usize,
    min_relevance: f32,
) -> Result<Vec<MemoryResult>, MemError> {
    // ---- Rank list 1: FTS5 full-text search ----
    let fts_ranks = fts5_search(conn, query_text)?;

    // ---- Rank list 2: Embedding cosine similarity via HNSW ----
    let embedding_ranks = embedding_search_hnsw(hnsw, query_text)?;

    // ---- Combine with RRF ----
    let mut rrf_scores: HashMap<u64, f32> = HashMap::new();

    for (rank, id) in fts_ranks.iter().enumerate() {
        let score = 1.0 / (RRF_K + rank as f32 + 1.0);
        *rrf_scores.entry(*id).or_insert(0.0) += score;
    }

    for (rank, (id, _sim)) in embedding_ranks.iter().enumerate() {
        let score = 1.0 / (RRF_K + rank as f32 + 1.0);
        *rrf_scores.entry(*id).or_insert(0.0) += score;
    }

    // Normalize RRF scores to [0, 1] range
    // Max possible RRF score = 2/(k+1) when doc is rank 1 in both lists
    let max_rrf = 2.0 / (RRF_K + 1.0);

    let mut scored: Vec<(u64, f32)> = rrf_scores
        .into_iter()
        .map(|(id, score)| (id, score / max_rrf))
        .filter(|(_, score)| *score >= min_relevance)
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(max_results);

    // Fetch full entries for results
    let mut results = Vec::with_capacity(scored.len());
    for (id, rrf_score) in scored {
        if let Some(entry) = conn
            .query_row(
                "SELECT id, concept, knowledge, confidence, source_episodes,
                        created_ms, last_reinforced_ms, access_count
                 FROM semantic_entries WHERE id = ?1",
                params![id as i64],
                row_to_semantic_entry,
            )
            .optional()
            .map_err(|e| MemError::QueryFailed(format!("fetch entry failed: {}", e)))?
        {
            results.push(MemoryResult {
                content: format!("[{}] {}", entry.concept, entry.knowledge),
                tier: MemoryTier::Semantic,
                relevance: rrf_score,
                importance: entry.confidence,
                timestamp_ms: entry.created_ms,
                source_id: entry.id,
            });
        }
    }

    Ok(results)
}

/// FTS5 full-text search — returns IDs ranked by FTS5 BM25 score.
fn fts5_search(conn: &Connection, query_text: &str) -> Result<Vec<u64>, MemError> {
    // Escape FTS5 query — wrap each word in quotes for safety
    let fts_query: String = query_text
        .split_whitespace()
        .map(|word| {
            // Remove special FTS5 characters
            let clean: String = word.chars().filter(|c| c.is_alphanumeric()).collect();
            if clean.is_empty() {
                String::new()
            } else {
                format!("\"{}\"", clean)
            }
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" OR ");

    if fts_query.is_empty() {
        return Ok(Vec::new());
    }

    let mut stmt = conn
        .prepare(
            "SELECT rowid FROM semantic_fts
             WHERE semantic_fts MATCH ?1
             ORDER BY rank
             LIMIT 100",
        )
        .map_err(|e| MemError::QueryFailed(format!("FTS5 prepare failed: {}", e)))?;

    let rows = stmt
        .query_map(params![fts_query], |row| {
            let id: i64 = row.get(0)?;
            Ok(id as u64)
        })
        .map_err(|e| MemError::QueryFailed(format!("FTS5 search failed: {}", e)))?;

    let mut ids = Vec::new();
    for row in rows {
        ids.push(row.map_err(|e| MemError::QueryFailed(format!("FTS5 row failed: {}", e)))?);
    }
    Ok(ids)
}

/// Embedding similarity search via HNSW index — returns (id, similarity) ranked
/// by cosine similarity descending.
///
/// Replaces the former O(n) linear scan of all embeddings.
fn embedding_search_hnsw(
    hnsw: &std::sync::Mutex<HnswState>,
    query_text: &str,
) -> Result<Vec<(u64, f32)>, MemError> {
    let query_embedding = embed(query_text);

    let mut state = hnsw
        .lock()
        .map_err(|_| MemError::QueryFailed("HNSW lock poisoned".into()))?;

    if state.index.is_empty() {
        return Ok(Vec::new());
    }

    state.search(&query_embedding, HNSW_SEARCH_LIMIT, HNSW_EF_SEARCH)
}

fn reinforce_sync(
    conn: &Connection,
    entry_id: u64,
    additional_episode_id: Option<u64>,
    now_ms: u64,
) -> Result<(), MemError> {
    // Get current entry
    let entry = conn
        .query_row(
            "SELECT id, concept, knowledge, confidence, source_episodes,
                    created_ms, last_reinforced_ms, access_count
             FROM semantic_entries WHERE id = ?1",
            params![entry_id as i64],
            row_to_semantic_entry,
        )
        .optional()
        .map_err(|e| MemError::QueryFailed(format!("reinforce get failed: {}", e)))?;

    let entry = match entry {
        Some(e) => e,
        None => {
            return Err(MemError::NotFound(format!(
                "semantic entry {} not found",
                entry_id
            )))
        },
    };

    // Boost confidence: min(0.99, current + 0.05)
    let new_confidence = (entry.confidence + 0.05).min(0.99);

    // Add episode to source list if provided
    let mut episodes = entry.source_episodes.clone();
    if let Some(ep_id) = additional_episode_id {
        if !episodes.contains(&ep_id) {
            episodes.push(ep_id);
        }
    }
    let episodes_json = serde_json::to_string(&episodes).unwrap_or_else(|_| "[]".into());

    conn.execute(
        "UPDATE semantic_entries
         SET confidence = ?1, last_reinforced_ms = ?2, source_episodes = ?3,
             access_count = access_count + 1
         WHERE id = ?4",
        params![
            new_confidence,
            now_ms as i64,
            episodes_json,
            entry_id as i64
        ],
    )
    .map_err(|e| MemError::DatabaseError(format!("reinforce update failed: {}", e)))?;

    debug!(
        "reinforced semantic entry {} (confidence: {:.2} -> {:.2})",
        entry_id, entry.confidence, new_confidence
    );
    Ok(())
}

/// LLM-powered generalization. Sends raw episodes to the neocortex and stores
/// the returned insight as a semantic entry.
///
/// # Architecture
/// **LLM = brain.** Rust never reasons about what episodes mean. Only the LLM
/// may synthesize patterns from raw episodic text. This function is the
/// correct path for the dreaming phase.
///
/// Falls back to [`try_generalize_sync`] if neocortex IPC is unavailable.
pub(crate) async fn try_generalize_via_llm(
    conn: &Connection,
    hnsw: &std::sync::Mutex<HnswState>,
    episodes: &[(String, f32)],
    concept_hint: &str,
    now_ms: u64,
    neocortex: &mut crate::ipc::NeocortexClient,
) -> Result<Option<u64>, MemError> {
    use aura_types::ipc::{DaemonToNeocortex, NeocortexToDaemon};

    if episodes.len() < GENERALIZATION_MIN_EPISODES {
        return Ok(None);
    }

    // Build the prompt — typed data formatted for the LLM.
    // Architecture: this is serialization of facts, NOT reasoning.
    let episodes_text = episodes
        .iter()
        .map(|(content, _)| content.as_str())
        .collect::<Vec<_>>()
        .join("; ");

    let prompt = format!(
        "Given these related experiences from a user: {episodes_text}.\n\
         What general pattern or insight do these suggest about this user's \
         preferences, personality, or behavior? Be concise (1-2 sentences)."
    );

    // Ask the LLM to reason — Rust is only the messenger here.
    let response = neocortex
        .request(&DaemonToNeocortex::Summarize { prompt })
        .await;

    let knowledge = match response {
        Ok(NeocortexToDaemon::Summary { text, .. }) => {
            info!(
                concept = concept_hint,
                episodes = episodes.len(),
                "LLM generalization succeeded"
            );
            text
        },
        Ok(other) => {
            warn!(
                ?other,
                concept = concept_hint,
                "unexpected neocortex response during generalization — falling back to sync"
            );
            return try_generalize_sync(conn, hnsw, episodes, concept_hint, now_ms);
        },
        Err(e) => {
            warn!(
                error = %e,
                concept = concept_hint,
                "neocortex IPC failed during generalization — falling back to sync"
            );
            return try_generalize_sync(conn, hnsw, episodes, concept_hint, now_ms);
        },
    };

    let confidence = importance::generalization_confidence(episodes.len());
    let combined = format!("{concept_hint} {knowledge}");
    let embedding = embed(&combined);
    let embedding_bytes = embedding_to_bytes(&embedding);
    let episodes_json = "[]";

    conn.execute(
        "INSERT INTO semantic_entries (concept, knowledge, confidence, source_episodes,
                                       created_ms, last_reinforced_ms, access_count, embedding)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0, ?6)",
        params![
            concept_hint,
            knowledge,
            confidence,
            episodes_json,
            now_ms as i64,
            embedding_bytes,
        ],
    )
    .map_err(|e| MemError::DatabaseError(format!("llm generalization insert failed: {e}")))?;

    let sqlite_id = conn.last_insert_rowid() as u64;

    {
        let mut state = hnsw
            .lock()
            .map_err(|_| MemError::DatabaseError("HNSW lock poisoned".into()))?;
        state.insert(sqlite_id, &embedding)?;
    }

    info!(
        "LLM-generalized {} episodes → semantic entry {} (concept: {}, confidence: {:.2})",
        episodes.len(),
        sqlite_id,
        concept_hint,
        confidence
    );
    Ok(Some(sqlite_id))
}

fn try_generalize_sync(
    conn: &Connection,
    hnsw: &std::sync::Mutex<HnswState>,
    episodes: &[(String, f32)],
    concept_hint: &str,
    now_ms: u64,
) -> Result<Option<u64>, MemError> {
    if episodes.len() < GENERALIZATION_MIN_EPISODES {
        return Ok(None);
    }

    // Verify episodes are actually similar to each other
    let embeddings: Vec<Vec<f32>> = episodes.iter().map(|(content, _)| embed(content)).collect();

    let mut all_similar = true;
    for i in 0..embeddings.len() {
        for j in (i + 1)..embeddings.len() {
            let sim = cosine_similarity(&embeddings[i], &embeddings[j]);
            if sim < GENERALIZATION_SIMILARITY {
                all_similar = false;
                break;
            }
        }
        if !all_similar {
            break;
        }
    }

    if !all_similar {
        debug!(
            "generalization rejected: episodes not similar enough (concept: {})",
            concept_hint
        );
        return Ok(None);
    }

    // Build generalized knowledge by combining unique information
    let knowledge = format!(
        "Generalized from {} observations: {}",
        episodes.len(),
        episodes
            .iter()
            .map(|(content, _)| content.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    );

    let confidence = importance::generalization_confidence(episodes.len());

    // Store it
    let combined = format!("{} {}", concept_hint, knowledge);
    let embedding = embed(&combined);
    let embedding_bytes = embedding_to_bytes(&embedding);
    let episodes_json = "[]"; // No specific episode IDs in this API path

    conn.execute(
        "INSERT INTO semantic_entries (concept, knowledge, confidence, source_episodes,
                                       created_ms, last_reinforced_ms, access_count, embedding)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0, ?6)",
        params![
            concept_hint,
            knowledge,
            confidence,
            episodes_json,
            now_ms as i64,
            embedding_bytes,
        ],
    )
    .map_err(|e| MemError::DatabaseError(format!("generalization insert failed: {}", e)))?;

    let sqlite_id = conn.last_insert_rowid() as u64;

    // Insert into HNSW index
    {
        let mut state = hnsw
            .lock()
            .map_err(|_| MemError::DatabaseError("HNSW lock poisoned".into()))?;
        state.insert(sqlite_id, &embedding)?;
    }

    info!(
        "generalized {} episodes into semantic entry {} (concept: {}, confidence: {:.2})",
        episodes.len(),
        sqlite_id,
        concept_hint,
        confidence
    );
    Ok(Some(sqlite_id))
}

fn find_by_concept_sync(
    conn: &Connection,
    hnsw: &std::sync::Mutex<HnswState>,
    concept: &str,
    min_similarity: f32,
    limit: usize,
) -> Result<Vec<SemanticEntry>, MemError> {
    let query_embedding = embed(concept);

    // Use HNSW to find candidates (request more than limit to allow filtering)
    let candidates = {
        let mut state = hnsw
            .lock()
            .map_err(|_| MemError::QueryFailed("HNSW lock poisoned".into()))?;

        if state.index.is_empty() {
            return Ok(Vec::new());
        }

        // Request extra candidates since we'll filter by min_similarity
        let search_k = (limit * 3).max(20);
        state.search(&query_embedding, search_k, HNSW_EF_SEARCH)?
    };

    // Filter by minimum similarity and fetch full entries from SQLite
    let mut results: Vec<(SemanticEntry, f32)> = Vec::new();
    for (sqlite_id, similarity) in candidates {
        if similarity < min_similarity {
            continue;
        }
        if let Some(entry) = conn
            .query_row(
                "SELECT id, concept, knowledge, confidence, source_episodes,
                        created_ms, last_reinforced_ms, access_count
                 FROM semantic_entries WHERE id = ?1",
                params![sqlite_id as i64],
                row_to_semantic_entry,
            )
            .optional()
            .map_err(|e| MemError::QueryFailed(format!("fetch entry failed: {}", e)))?
        {
            results.push((entry, similarity));
        }
    }

    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit);
    Ok(results.into_iter().map(|(entry, _)| entry).collect())
}

fn row_to_semantic_entry(row: &rusqlite::Row) -> rusqlite::Result<SemanticEntry> {
    let id: i64 = row.get(0)?;
    let concept: String = row.get(1)?;
    let knowledge: String = row.get(2)?;
    let confidence: f64 = row.get(3)?;
    let episodes_json: String = row.get(4)?;
    let created_ms: i64 = row.get(5)?;
    let last_reinforced_ms: i64 = row.get(6)?;
    let access_count: i64 = row.get(7)?;

    let source_episodes: Vec<u64> = serde_json::from_str(&episodes_json).unwrap_or_default();

    Ok(SemanticEntry {
        id: id as u64,
        concept,
        knowledge,
        confidence: confidence as f32,
        source_episodes,
        created_ms: created_ms as u64,
        last_reinforced_ms: last_reinforced_ms as u64,
        access_count: access_count as u32,
    })
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
    fn test_open_in_memory() {
        let store = SemanticMemory::open_in_memory().unwrap();
        let rt = rt();
        let count = rt.block_on(store.count()).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_store_and_count() {
        let store = SemanticMemory::open_in_memory().unwrap();
        let rt = rt();

        let id = rt
            .block_on(store.store(
                "dark mode".into(),
                "User prefers dark mode on all applications".into(),
                0.8,
                vec![1, 2, 3],
                now(),
            ))
            .unwrap();

        assert!(id > 0);
        let count = rt.block_on(store.count()).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_store_populates_hnsw() {
        let store = SemanticMemory::open_in_memory().unwrap();
        let rt = rt();

        // Initially HNSW should be empty
        {
            let state = store.hnsw.lock().unwrap();
            assert!(state.index.is_empty());
        }

        rt.block_on(store.store(
            "dark mode".into(),
            "User prefers dark mode".into(),
            0.8,
            vec![],
            now(),
        ))
        .unwrap();

        // After store, HNSW should have 1 entry
        {
            let state = store.hnsw.lock().unwrap();
            assert_eq!(state.index.len(), 1);
            assert_eq!(state.node_to_sqlite.len(), 1);
            assert_eq!(state.sqlite_to_node.len(), 1);
        }
    }

    #[test]
    fn test_get_entry() {
        let store = SemanticMemory::open_in_memory().unwrap();
        let rt = rt();

        let id = rt
            .block_on(store.store(
                "sleep schedule".into(),
                "User typically goes to bed around 11pm".into(),
                0.7,
                vec![10, 20],
                now(),
            ))
            .unwrap();

        let entry = rt.block_on(store.get(id)).unwrap().unwrap();
        assert_eq!(entry.concept, "sleep schedule");
        assert!(entry.knowledge.contains("11pm"));
        assert!((entry.confidence - 0.7).abs() < 0.01);
        assert_eq!(entry.source_episodes, vec![10, 20]);
    }

    #[test]
    fn test_query_fts_and_embedding() {
        let store = SemanticMemory::open_in_memory().unwrap();
        let rt = rt();

        rt.block_on(store.store(
            "dark mode preference".into(),
            "User strongly prefers dark mode in all apps".into(),
            0.9,
            vec![],
            now(),
        ))
        .unwrap();

        rt.block_on(store.store(
            "morning routine".into(),
            "User checks email first thing in the morning".into(),
            0.7,
            vec![],
            now() + 1000,
        ))
        .unwrap();

        rt.block_on(store.store(
            "favorite food".into(),
            "User enjoys pasta and pizza for dinner".into(),
            0.6,
            vec![],
            now() + 2000,
        ))
        .unwrap();

        // Query for dark mode
        let results = rt
            .block_on(store.query("dark mode theme preference", 5, 0.0, now() + 3000))
            .unwrap();

        assert!(!results.is_empty());
        assert!(
            results[0].content.contains("dark mode"),
            "top result should be about dark mode, got: {}",
            results[0].content
        );
    }

    #[test]
    fn test_reinforce() {
        let store = SemanticMemory::open_in_memory().unwrap();
        let rt = rt();

        let id = rt
            .block_on(store.store(
                "dark mode".into(),
                "User prefers dark mode".into(),
                0.5,
                vec![1],
                now(),
            ))
            .unwrap();

        // Reinforce twice
        rt.block_on(store.reinforce(id, Some(2), now() + 1000))
            .unwrap();
        rt.block_on(store.reinforce(id, Some(3), now() + 2000))
            .unwrap();

        let entry = rt.block_on(store.get(id)).unwrap().unwrap();
        assert!((entry.confidence - 0.6).abs() < 0.01); // 0.5 + 0.05 + 0.05
        assert_eq!(entry.source_episodes, vec![1, 2, 3]);
        assert_eq!(entry.access_count, 2);
    }

    #[test]
    fn test_reinforce_caps_at_099() {
        let store = SemanticMemory::open_in_memory().unwrap();
        let rt = rt();

        let id = rt
            .block_on(store.store(
                "very confident".into(),
                "Known with high certainty".into(),
                0.97,
                vec![],
                now(),
            ))
            .unwrap();

        rt.block_on(store.reinforce(id, None, now() + 1000))
            .unwrap();

        let entry = rt.block_on(store.get(id)).unwrap().unwrap();
        assert!(entry.confidence <= 0.99);
    }

    #[test]
    fn test_generalization() {
        let store = SemanticMemory::open_in_memory().unwrap();
        let rt = rt();

        let episodes = vec![
            ("User opened dark mode settings".into(), 0.5),
            ("User enabled dark mode in Chrome".into(), 0.5),
            ("User switched to dark mode in VS Code".into(), 0.5),
        ];

        let result = rt
            .block_on(store.try_generalize(&episodes, "dark mode preference", now()))
            .unwrap();

        assert!(result.is_some(), "should create generalized entry");

        let id = result.unwrap();
        let entry = rt.block_on(store.get(id)).unwrap().unwrap();
        assert_eq!(entry.concept, "dark mode preference");
        assert!(entry.knowledge.contains("Generalized from 3 observations"));

        // Confidence should be generalization_confidence(3) = min(0.95, 0.5 + 0.3) = 0.8
        assert!((entry.confidence - 0.8).abs() < 0.01);

        // Generalization should also be in HNSW
        {
            let state = store.hnsw.lock().unwrap();
            assert_eq!(state.index.len(), 1);
        }
    }

    #[test]
    fn test_generalization_rejected_too_few() {
        let store = SemanticMemory::open_in_memory().unwrap();
        let rt = rt();

        let episodes = vec![
            ("User opened dark mode".into(), 0.5),
            ("User closed dark mode".into(), 0.5),
        ];

        let result = rt
            .block_on(store.try_generalize(&episodes, "dark mode", now()))
            .unwrap();

        assert!(
            result.is_none(),
            "should not generalize from only 2 episodes"
        );
    }

    #[test]
    fn test_find_by_concept() {
        let store = SemanticMemory::open_in_memory().unwrap();
        let rt = rt();

        rt.block_on(store.store(
            "dark mode".into(),
            "User prefers dark mode".into(),
            0.8,
            vec![],
            now(),
        ))
        .unwrap();

        rt.block_on(store.store(
            "cooking skills".into(),
            "User enjoys cooking Italian food".into(),
            0.6,
            vec![],
            now() + 1000,
        ))
        .unwrap();

        let results = rt
            .block_on(store.find_by_concept("dark mode theme", 0.3, 5))
            .unwrap();

        assert!(!results.is_empty());
        assert!(results[0].concept.contains("dark mode"));
    }

    #[test]
    fn test_record_access() {
        let store = SemanticMemory::open_in_memory().unwrap();
        let rt = rt();

        let id = rt
            .block_on(store.store("test".into(), "test knowledge".into(), 0.5, vec![], now()))
            .unwrap();

        rt.block_on(store.record_access(id)).unwrap();
        rt.block_on(store.record_access(id)).unwrap();

        let entry = rt.block_on(store.get(id)).unwrap().unwrap();
        assert_eq!(entry.access_count, 2);
    }

    #[test]
    fn test_storage_bytes() {
        let store = SemanticMemory::open_in_memory().unwrap();
        let rt = rt();
        let size = rt.block_on(store.storage_bytes()).unwrap();
        assert!(size > 0);
    }

    #[test]
    fn test_hnsw_rebuild_on_open() {
        // Test that opening a database with existing entries rebuilds HNSW
        let store = SemanticMemory::open_in_memory().unwrap();
        let rt = rt();

        rt.block_on(store.store(
            "topic a".into(),
            "knowledge about topic a".into(),
            0.8,
            vec![],
            now(),
        ))
        .unwrap();

        rt.block_on(store.store(
            "topic b".into(),
            "knowledge about topic b".into(),
            0.7,
            vec![],
            now() + 1000,
        ))
        .unwrap();

        // Verify HNSW has 2 entries
        {
            let state = store.hnsw.lock().unwrap();
            assert_eq!(state.index.len(), 2);
        }

        // We can't re-open an in-memory DB, but we can verify the rebuild logic
        // by directly calling build_hnsw_from_db with a connection that has data
        let conn = store.conn.try_lock().unwrap();
        let rebuilt = build_hnsw_from_db(&conn).unwrap();
        assert_eq!(rebuilt.index.len(), 2);
        assert_eq!(rebuilt.node_to_sqlite.len(), 2);
        assert_eq!(rebuilt.sqlite_to_node.len(), 2);
    }

    #[test]
    fn test_hnsw_search_returns_relevant() {
        let store = SemanticMemory::open_in_memory().unwrap();
        let rt = rt();

        // Store several different topics
        rt.block_on(store.store(
            "python programming".into(),
            "User loves coding in Python with Django".into(),
            0.8,
            vec![],
            now(),
        ))
        .unwrap();

        rt.block_on(store.store(
            "cooking pasta".into(),
            "User enjoys making homemade pasta and Italian recipes".into(),
            0.7,
            vec![],
            now() + 1000,
        ))
        .unwrap();

        rt.block_on(store.store(
            "javascript frameworks".into(),
            "User prefers React and TypeScript for frontend".into(),
            0.8,
            vec![],
            now() + 2000,
        ))
        .unwrap();

        // HNSW search for programming-related content
        let mut state = store.hnsw.lock().unwrap();
        let query_emb = embed("programming code software");
        let results = state.search(&query_emb, 3, HNSW_EF_SEARCH).unwrap();

        // Should return results (we have 3 entries)
        assert!(!results.is_empty());
        // Results should be ordered by similarity descending
        for w in results.windows(2) {
            assert!(
                w[0].1 >= w[1].1,
                "results should be sorted by similarity desc"
            );
        }
    }
}
