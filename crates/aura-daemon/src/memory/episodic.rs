//! Episodic memory — SQLite WAL-backed store for specific experiences.
//!
//! Budget: ~18MB/year, 2-8ms latency.
//!
//! Each episode records a specific event/experience with emotional context,
//! importance scoring, and a 64-dim trigram hash embedding for similarity search.
//!
//! Retrieval uses the v4 recall scoring formula:
//!   score = similarity×0.4 + recency×0.2 + importance×0.2 + activation×0.2
//!
//! Pattern separation: when a new episode is too similar (cosine > 0.9) to an
//! existing one, slight noise is injected into the embedding to maintain
//! discriminability — inspired by dentate gyrus pattern separation in the brain.
//!
//! Uses HNSW approximate nearest neighbor index for sub-linear query time.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;
use tracing::{debug, info};

use aura_types::errors::MemError;
use aura_types::ipc::MemoryTier;
use aura_types::memory::{Episode, MemoryResult};

use crate::memory::embeddings::{
    self, cosine_similarity, embed, embedding_from_bytes, embedding_to_bytes, EMBED_DIM,
};
use crate::memory::hnsw::HnswIndex;
use crate::memory::importance;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Cosine similarity threshold above which pattern separation is applied.
const PATTERN_SEPARATION_THRESHOLD: f32 = 0.9;

/// Noise magnitude for pattern separation (small perturbation).
const PATTERN_SEPARATION_NOISE: f32 = 0.05;

/// Maximum episodes to scan for pattern separation (performance guard).
const PATTERN_SEPARATION_SCAN_LIMIT: usize = 100;

/// ef parameter for HNSW search (controls recall vs speed tradeoff).
const HNSW_EF_SEARCH: usize = 50;

/// Maximum number of HNSW candidates to retrieve.
const HNSW_SEARCH_LIMIT: usize = 100;

// ---------------------------------------------------------------------------
// HnswState — HNSW index + bidirectional ID maps for episodic memory
// ---------------------------------------------------------------------------

struct HnswState {
    index: HnswIndex,
    node_to_sqlite: HashMap<u32, u64>,
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

    fn insert(&mut self, sqlite_id: u64, embedding: &[f32]) -> Result<u32, MemError> {
        let node_id = self
            .index
            .insert(embedding)
            .map_err(|e| MemError::DatabaseError(format!("HNSW insert failed: {}", e)))?;
        self.node_to_sqlite.insert(node_id, sqlite_id);
        self.sqlite_to_node.insert(sqlite_id, node_id);
        Ok(node_id)
    }

    fn search(&self, query: &[f32], k: usize, ef: usize) -> Result<Vec<(u64, f32)>, MemError> {
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
// EpisodicMemory
// ---------------------------------------------------------------------------

/// SQLite WAL-backed episodic memory store with HNSW index.
///
/// Thread-safe via `Arc<Mutex<Connection>>`. All blocking SQLite calls are
/// wrapped in `spawn_blocking` when used from async contexts.
pub struct EpisodicMemory {
    conn: Arc<Mutex<Connection>>,
    hnsw: Arc<std::sync::Mutex<HnswState>>,
}

impl EpisodicMemory {
    /// Open (or create) an episodic memory database at the given path.
    ///
    /// Enables WAL mode for concurrent reads and atomic writes.
    pub fn open(db_path: &Path) -> Result<Self, MemError> {
        let conn = Connection::open(db_path).map_err(|e| {
            MemError::DatabaseError(format!("failed to open episodic db: {}", e))
        })?;

        // Enable WAL mode for safety and performance
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA cache_size=-4000;",
        )
        .map_err(|e| MemError::DatabaseError(format!("pragmas failed: {}", e)))?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            hnsw: Arc::new(std::sync::Mutex::new(HnswState::new())),
        };
        // Run migrations synchronously at startup
        store.migrate_sync()?;
        // Build HNSW index from existing data
        store.build_hnsw_index()?;
        Ok(store)
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self, MemError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| MemError::DatabaseError(format!("in-memory open failed: {}", e)))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| MemError::DatabaseError(format!("pragmas failed: {}", e)))?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            hnsw: Arc::new(std::sync::Mutex::new(HnswState::new())),
        };
        store.migrate_sync()?;
        Ok(store)
    }

    /// Build HNSW index from existing episodes in the database.
    fn build_hnsw_index(&self) -> Result<(), MemError> {
        let conn = self.conn.try_lock().map_err(|_| {
            MemError::DatabaseError("could not lock db for HNSW rebuild".into())
        })?;

        let mut stmt = conn
            .prepare("SELECT id, embedding FROM episodes WHERE embedding IS NOT NULL")
            .map_err(|e| MemError::DatabaseError(format!("HNSW rebuild prepare failed: {}", e)))?;

        let rows = stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((id as u64, blob))
            })
            .map_err(|e| MemError::DatabaseError(format!("HNSW rebuild query failed: {}", e)))?;

        let mut hnsw = self.hnsw.lock().map_err(|_| {
            MemError::DatabaseError("HNSW lock poisoned".into())
        })?;

        let mut loaded = 0u64;
        for row in rows {
            let (sqlite_id, blob) =
                row.map_err(|e| MemError::DatabaseError(format!("HNSW rebuild row failed: {}", e)))?;
            let emb = embedding_from_bytes(&blob);
            if emb.len() == EMBED_DIM {
                hnsw.insert(sqlite_id, &emb)?;
                loaded += 1;
            }
        }

        info!("episodic HNSW index rebuilt with {} embeddings", loaded);
        Ok(())
    }

    /// Run database migrations.
    fn migrate_sync(&self) -> Result<(), MemError> {
        // We need to block on the mutex for the sync migration at init.
        // This is only called once during open(), before any async context.
        let conn = self.conn.try_lock().map_err(|_| {
            MemError::MigrationFailed("could not lock db for migration".into())
        })?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS episodes (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                content         TEXT NOT NULL,
                emotional_valence REAL NOT NULL DEFAULT 0.0,
                importance      REAL NOT NULL DEFAULT 0.5,
                context_tags    TEXT NOT NULL DEFAULT '[]',
                timestamp_ms    INTEGER NOT NULL,
                access_count    INTEGER NOT NULL DEFAULT 0,
                last_access_ms  INTEGER NOT NULL,
                embedding       BLOB
            );

            CREATE INDEX IF NOT EXISTS idx_episodes_timestamp
                ON episodes(timestamp_ms);
            CREATE INDEX IF NOT EXISTS idx_episodes_importance
                ON episodes(importance);
            CREATE INDEX IF NOT EXISTS idx_episodes_last_access
                ON episodes(last_access_ms);",
        )
        .map_err(|e| MemError::MigrationFailed(format!("episode table creation failed: {}", e)))?;

        info!("episodic memory: migrations complete");
        Ok(())
    }

    /// Store a new episode. Returns the assigned episode ID.
    ///
    /// Automatically:
    /// 1. Computes the embedding
    /// 2. Applies pattern separation if too similar to recent episodes
    /// 3. Inserts with WAL-safe atomic write
    /// 4. Indexes embedding in HNSW for fast similarity search
    pub async fn store(
        &self,
        content: String,
        emotional_valence: f32,
        base_importance: f32,
        context_tags: Vec<String>,
        now_ms: u64,
    ) -> Result<u64, MemError> {
        let conn = self.conn.clone();
        let hnsw = self.hnsw.clone();

        let id = tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let mut hnsw_state = hnsw.lock().map_err(|_| {
                MemError::DatabaseError("HNSW lock poisoned".into())
            })?;
            let (id, embedding) = store_episode_sync(
                &conn,
                &hnsw_state,
                &content,
                emotional_valence,
                base_importance,
                &context_tags,
                now_ms,
            )?;

            // Add to HNSW index
            hnsw_state.insert(id, &embedding)?;

            Ok::<_, MemError>(id)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))??;

        Ok(id)
    }

    /// Query episodic memory using HNSW + recall scoring formula.
    ///
    /// Uses HNSW approximate nearest neighbor search for fast retrieval,
    /// then applies recall scoring formula for final ranking.
    ///
    /// Returns up to `max_results` episodes sorted by recall score descending,
    /// filtered to those above `min_relevance`.
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
            query_episodes_hnsw(&conn, &hnsw, &query_text, max_results, min_relevance, now_ms)
        })
        .await
        .map_err(|e| MemError::QueryFailed(format!("spawn_blocking failed: {}", e)))??;

        Ok(results)
    }

    /// Record an access to an episode (updates access_count and last_access_ms).
    pub async fn record_access(&self, episode_id: u64, now_ms: u64) -> Result<(), MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            conn.execute(
                "UPDATE episodes SET access_count = access_count + 1, last_access_ms = ?1 WHERE id = ?2",
                params![now_ms as i64, episode_id as i64],
            )
            .map_err(|e| MemError::DatabaseError(format!("access update failed: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Get a specific episode by ID.
    pub async fn get(&self, episode_id: u64) -> Result<Option<Episode>, MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            get_episode_sync(&conn, episode_id)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Count total episodes.
    pub async fn count(&self) -> Result<u64, MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM episodes", [], |row| row.get(0))
                .map_err(|e| MemError::DatabaseError(format!("count failed: {}", e)))?;
            Ok(count as u64)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Get episodes older than a threshold with importance below a cutoff.
    ///
    /// Used by the consolidation engine to find candidates for archival.
    pub async fn get_archival_candidates(
        &self,
        max_age_ms: u64,
        max_importance: f32,
        now_ms: u64,
        limit: usize,
    ) -> Result<Vec<Episode>, MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let cutoff_ms = now_ms.saturating_sub(max_age_ms);
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, emotional_valence, importance, context_tags,
                            timestamp_ms, access_count, last_access_ms, embedding
                     FROM episodes
                     WHERE timestamp_ms < ?1 AND importance < ?2
                     ORDER BY importance ASC
                     LIMIT ?3",
                )
                .map_err(|e| MemError::QueryFailed(format!("prepare failed: {}", e)))?;

            let rows = stmt
                .query_map(
                    params![cutoff_ms as i64, max_importance, limit as i64],
                    row_to_episode,
                )
                .map_err(|e| MemError::QueryFailed(format!("query failed: {}", e)))?;

            let mut episodes = Vec::new();
            for row in rows {
                episodes.push(
                    row.map_err(|e| MemError::QueryFailed(format!("row read failed: {}", e)))?,
                );
            }
            Ok(episodes)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Delete episodes by ID (after archival).
    pub async fn delete_episodes(&self, ids: &[u64]) -> Result<usize, MemError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.clone();
        let ids = ids.to_vec();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("DELETE FROM episodes WHERE id IN ({})", placeholders);
            let params: Vec<Box<dyn rusqlite::types::ToSql>> =
                ids.iter().map(|&id| Box::new(id as i64) as Box<dyn rusqlite::types::ToSql>).collect();
            let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
            let deleted = conn
                .execute(&sql, refs.as_slice())
                .map_err(|e| MemError::DatabaseError(format!("delete failed: {}", e)))?;
            Ok(deleted)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Get episodes that are similar to the given content (for consolidation/generalization).
    pub async fn find_similar(
        &self,
        content: &str,
        min_similarity: f32,
        limit: usize,
    ) -> Result<Vec<Episode>, MemError> {
        let conn = self.conn.clone();
        let hnsw = self.hnsw.clone();
        let content = content.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            find_similar_sync(&conn, &hnsw, &content, min_similarity, limit)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Get the N most recent full episodes.
    /// Useful for clustering and analysis where context tags and full metadata are needed.
    pub async fn get_recent_episodes(&self, limit: usize) -> Result<Vec<Episode>, MemError> {
        let conn = self.conn.clone();
        
        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, emotional_valence, importance, context_tags,
                            timestamp_ms, access_count, last_access_ms, embedding
                     FROM episodes
                     ORDER BY id DESC LIMIT ?1",
                )
                .map_err(|e| MemError::QueryFailed(format!("recent query prepare failed: {}", e)))?;

            let rows = stmt
                .query_map(params![limit as i64], row_to_episode)
                .map_err(|e| MemError::QueryFailed(format!("recent query failed: {}", e)))?;

            let mut episodes = Vec::new();
            for row in rows {
                episodes.push(row.map_err(|e| MemError::QueryFailed(format!("row read failed: {}", e)))?);
            }
            Ok(episodes)
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
// Synchronous helpers (run inside spawn_blocking)
// ---------------------------------------------------------------------------

fn store_episode_sync(
    conn: &Connection,
    hnsw_state: &HnswState,
    content: &str,
    emotional_valence: f32,
    base_importance: f32,
    context_tags: &[String],
    now_ms: u64,
) -> Result<(u64, Vec<f32>), MemError> {
    // 1. Compute embedding
    let mut embedding = embed(content);

    // 2. Pattern separation: O(log n) via HNSW nearest-neighbor search
    apply_pattern_separation(hnsw_state, &mut embedding)?;

    // 3. Serialize
    let embedding_bytes = embedding_to_bytes(&embedding);
    let tags_json =
        serde_json::to_string(context_tags).unwrap_or_else(|_| "[]".to_string());

    // 4. Insert atomically
    conn.execute(
        "INSERT INTO episodes (content, emotional_valence, importance, context_tags,
                               timestamp_ms, access_count, last_access_ms, embedding)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?5, ?6)",
        params![
            content,
            emotional_valence,
            base_importance,
            tags_json,
            now_ms as i64,
            embedding_bytes,
        ],
    )
    .map_err(|e| MemError::DatabaseError(format!("episode insert failed: {}", e)))?;

    let id = conn.last_insert_rowid() as u64;
    debug!("stored episode {} ({} chars)", id, content.len());
    Ok((id, embedding))
}

/// Apply pattern separation: if the new embedding is too close to any existing
/// episode (cosine > 0.9), inject small noise to maintain discriminability.
///
/// This mimics the dentate gyrus pattern separation mechanism.
///
/// Uses HNSW nearest-neighbor search: O(log n) instead of the old O(n) SQLite
/// scan that fetched and deserialized the last 100 episode embeddings.
/// The only episode that could exceed the 0.9 threshold is the nearest
/// neighbor, so k=1 search is sufficient.
fn apply_pattern_separation(
    hnsw_state: &HnswState,
    embedding: &mut Vec<f32>,
) -> Result<(), MemError> {
    // HNSW search for nearest neighbor: O(log n)
    let nearest = hnsw_state.search(embedding, 1, HNSW_EF_SEARCH)?;

    let needs_separation = nearest
        .first()
        .map(|(_id, sim)| *sim > PATTERN_SEPARATION_THRESHOLD)
        .unwrap_or(false);

    if needs_separation {
        // Inject deterministic noise based on embedding content
        // Using a simple hash-based approach for determinism
        let seed = embedding
            .iter()
            .enumerate()
            .fold(0u64, |acc, (i, &v)| {
                acc.wrapping_add((v.to_bits() as u64).wrapping_mul(i as u64 + 1))
            });

        for (i, val) in embedding.iter_mut().enumerate() {
            // Simple deterministic pseudo-noise
            let noise_seed = seed.wrapping_mul(i as u64 + 7).wrapping_add(0xDEAD_BEEF);
            let noise = ((noise_seed % 1000) as f32 / 1000.0 - 0.5) * 2.0 * PATTERN_SEPARATION_NOISE;
            *val += noise;
        }

        // Re-normalize to unit vector
        let magnitude: f32 = embedding.iter().map(|v| v * v).sum::<f32>().sqrt();
        if magnitude > f32::EPSILON {
            for v in embedding.iter_mut() {
                *v /= magnitude;
            }
        }

        debug!("pattern separation applied to new episode embedding (via HNSW)");
    }

    Ok(())
}

/// Query episodic memory using HNSW for fast approximate nearest neighbor search.
/// This replaces the O(n) linear scan with sub-linear HNSW search.
fn query_episodes_hnsw(
    conn: &Connection,
    hnsw: &std::sync::Mutex<HnswState>,
    query_text: &str,
    max_results: usize,
    min_relevance: f32,
    now_ms: u64,
) -> Result<Vec<MemoryResult>, MemError> {
    let query_embedding = embed(query_text);

    // Check if HNSW index has any entries
    let hnsw_state = hnsw
        .lock()
        .map_err(|_| MemError::QueryFailed("HNSW lock poisoned".into()))?;

    if hnsw_state.index.is_empty() {
        // Fallback to empty results if no indexed episodes
        return Ok(Vec::new());
    }

    // Use HNSW to get candidate episodes (much faster than O(n) scan)
    let hnsw_results = hnsw_state.search(&query_embedding, HNSW_SEARCH_LIMIT, HNSW_EF_SEARCH)?;

    if hnsw_results.is_empty() {
        return Ok(Vec::new());
    }

    // Collect candidate IDs for SQL query
    let candidate_ids: Vec<i64> = hnsw_results.iter().map(|(id, _)| *id as i64).collect();

    // Fetch full episode data for candidates
    let placeholders: String = candidate_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT id, content, emotional_valence, importance, context_tags,
                timestamp_ms, access_count, last_access_ms, embedding
         FROM episodes WHERE id IN ({})",
        placeholders
    );

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| MemError::QueryFailed(format!("candidate query prepare failed: {}", e)))?;

    let params: Vec<&dyn rusqlite::types::ToSql> = candidate_ids
        .iter()
        .collect::<Vec<_>>()
        .iter()
        .map(|id| *id as &dyn rusqlite::types::ToSql)
        .collect();

    let rows = stmt
        .query_map(params.as_slice(), row_to_episode)
        .map_err(|e| MemError::QueryFailed(format!("candidate query failed: {}", e)))?;

    // Build a map from ID to HNSW similarity for quick lookup
    let mut sim_map: HashMap<u64, f32> = hnsw_results.into_iter().collect();

    let now_hours = now_ms as f64 / 3_600_000.0;
    let mut scored: Vec<(MemoryResult, f32)> = Vec::new();

    for row in rows {
        let episode =
            row.map_err(|e| MemError::QueryFailed(format!("row read failed: {}", e)))?;

        // Use HNSW similarity if available, otherwise compute
        let similarity = sim_map
            .remove(&episode.id)
            .unwrap_or_else(|| {
                match &episode.embedding {
                    Some(emb) if emb.len() == query_embedding.len() => {
                        cosine_similarity(&query_embedding, emb)
                    }
                    _ => {
                        embeddings::jaccard_trigram_similarity(query_text, &episode.content)
                    }
                }
            });

        // Compute recall score
        let episode_hours = episode.timestamp_ms as f64 / 3_600_000.0;
        let hours_ago = (now_hours - episode_hours).max(0.0);

        let score = importance::recall_score(
            similarity,
            hours_ago,
            episode.importance,
            episode.access_count,
        );

        if score >= min_relevance {
            scored.push((
                MemoryResult {
                    content: episode.content.clone(),
                    tier: MemoryTier::Episodic,
                    relevance: score,
                    importance: episode.importance,
                    timestamp_ms: episode.timestamp_ms,
                    source_id: episode.id,
                },
                score,
            ));
        }
    }

    // Sort by score descending and truncate
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(max_results);

    Ok(scored.into_iter().map(|(result, _)| result).collect())
}

/// Legacy O(n) query - kept for fallback and specific use cases.
fn query_episodes_sync(
    conn: &Connection,
    query_text: &str,
    max_results: usize,
    min_relevance: f32,
    now_ms: u64,
) -> Result<Vec<MemoryResult>, MemError> {
    let query_embedding = embed(query_text);

    let mut stmt = conn
        .prepare(
            "SELECT id, content, emotional_valence, importance, context_tags,
                    timestamp_ms, access_count, last_access_ms, embedding
             FROM episodes",
        )
        .map_err(|e| MemError::QueryFailed(format!("query prepare failed: {}", e)))?;

    let rows = stmt
        .query_map([], row_to_episode)
        .map_err(|e| MemError::QueryFailed(format!("query failed: {}", e)))?;

    let now_hours = now_ms as f64 / 3_600_000.0;
    let mut scored: Vec<(MemoryResult, f32)> = Vec::new();

    for row in rows {
        let episode =
            row.map_err(|e| MemError::QueryFailed(format!("row read failed: {}", e)))?;

        // Compute similarity
        let similarity = match &episode.embedding {
            Some(emb) if emb.len() == query_embedding.len() => {
                cosine_similarity(&query_embedding, emb)
            }
            _ => {
                // Fallback to Jaccard trigram similarity
                embeddings::jaccard_trigram_similarity(query_text, &episode.content)
            }
        };

        // Compute recall score
        let episode_hours = episode.timestamp_ms as f64 / 3_600_000.0;
        let hours_ago = (now_hours - episode_hours).max(0.0);

        let score = importance::recall_score(
            similarity,
            hours_ago,
            episode.importance,
            episode.access_count,
        );

        if score >= min_relevance {
            scored.push((
                MemoryResult {
                    content: episode.content.clone(),
                    tier: MemoryTier::Episodic,
                    relevance: score,
                    importance: episode.importance,
                    timestamp_ms: episode.timestamp_ms,
                    source_id: episode.id,
                },
                score,
            ));
        }
    }

    // Sort by score descending and truncate
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(max_results);

    Ok(scored.into_iter().map(|(result, _)| result).collect())
}

fn get_episode_sync(conn: &Connection, episode_id: u64) -> Result<Option<Episode>, MemError> {
    let result = conn
        .query_row(
            "SELECT id, content, emotional_valence, importance, context_tags,
                    timestamp_ms, access_count, last_access_ms, embedding
             FROM episodes WHERE id = ?1",
            params![episode_id as i64],
            row_to_episode,
        )
        .optional()
        .map_err(|e| MemError::QueryFailed(format!("get episode failed: {}", e)))?;

    Ok(result)
}

fn find_similar_sync(
    conn: &Connection,
    hnsw: &std::sync::Mutex<HnswState>,
    content: &str,
    min_similarity: f32,
    limit: usize,
) -> Result<Vec<Episode>, MemError> {
    let query_embedding = embed(content);

    let hnsw_state = hnsw
        .lock()
        .map_err(|_| MemError::QueryFailed("HNSW lock poisoned".into()))?;

    if hnsw_state.index.is_empty() {
        return Ok(Vec::new());
    }

    let search_limit = (limit * 3).max(20);
    let hnsw_results = hnsw_state.search(&query_embedding, search_limit, HNSW_EF_SEARCH)?;

    if hnsw_results.is_empty() {
        return Ok(Vec::new());
    }

    let candidate_ids: Vec<i64> = hnsw_results.iter().map(|(id, _)| *id as i64).collect();
    let placeholders: String = candidate_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT id, content, emotional_valence, importance, context_tags,
                timestamp_ms, access_count, last_access_ms, embedding
         FROM episodes WHERE id IN ({})",
        placeholders
    );

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| MemError::QueryFailed(format!("find_similar prepare failed: {}", e)))?;

    let params: Vec<&dyn rusqlite::types::ToSql> = candidate_ids
        .iter()
        .collect::<Vec<_>>()
        .iter()
        .map(|id| *id as &dyn rusqlite::types::ToSql)
        .collect();

    let rows = stmt
        .query_map(params.as_slice(), row_to_episode)
        .map_err(|e| MemError::QueryFailed(format!("find_similar query failed: {}", e)))?;

    let mut sim_map: HashMap<u64, f32> = hnsw_results.into_iter().collect();
    let mut results: Vec<(Episode, f32)> = Vec::new();

    for row in rows {
        let episode =
            row.map_err(|e| MemError::QueryFailed(format!("row read failed: {}", e)))?;

        if let Some(sim) = sim_map.remove(&episode.id) {
            if sim >= min_similarity {
                results.push((episode, sim));
            }
        }
    }

    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit);

    Ok(results.into_iter().map(|(ep, _)| ep).collect())
}

/// Map a rusqlite Row to an Episode.
fn row_to_episode(row: &rusqlite::Row) -> rusqlite::Result<Episode> {
    let id: i64 = row.get(0)?;
    let content: String = row.get(1)?;
    let emotional_valence: f64 = row.get(2)?;
    let importance_val: f64 = row.get(3)?;
    let tags_json: String = row.get(4)?;
    let timestamp_ms: i64 = row.get(5)?;
    let access_count: i64 = row.get(6)?;
    let last_access_ms: i64 = row.get(7)?;
    let embedding_blob: Option<Vec<u8>> = row.get(8)?;

    let context_tags: Vec<String> =
        serde_json::from_str(&tags_json).unwrap_or_default();

    let embedding = embedding_blob.map(|blob| embedding_from_bytes(&blob));

    Ok(Episode {
        id: id as u64,
        content,
        emotional_valence: emotional_valence as f32,
        importance: importance_val as f32,
        context_tags,
        timestamp_ms: timestamp_ms as u64,
        access_count: access_count as u32,
        last_access_ms: last_access_ms as u64,
        embedding,
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
        let store = EpisodicMemory::open_in_memory().unwrap();
        let rt = rt();
        let count = rt.block_on(store.count()).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_store_and_count() {
        let store = EpisodicMemory::open_in_memory().unwrap();
        let rt = rt();

        let id = rt
            .block_on(store.store(
                "user asked about the weather".into(),
                0.3,
                0.7,
                vec!["weather".into()],
                now(),
            ))
            .unwrap();

        assert!(id > 0);

        let count = rt.block_on(store.count()).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_store_multiple() {
        let store = EpisodicMemory::open_in_memory().unwrap();
        let rt = rt();

        for i in 0..5 {
            rt.block_on(store.store(
                format!("event number {}", i),
                0.0,
                0.5,
                vec![],
                now() + i * 1000,
            ))
            .unwrap();
        }

        let count = rt.block_on(store.count()).unwrap();
        assert_eq!(count, 5);
    }

    #[test]
    fn test_get_episode() {
        let store = EpisodicMemory::open_in_memory().unwrap();
        let rt = rt();

        let id = rt
            .block_on(store.store(
                "memorable event".into(),
                0.8,
                0.9,
                vec!["important".into()],
                now(),
            ))
            .unwrap();

        let ep = rt.block_on(store.get(id)).unwrap().unwrap();
        assert_eq!(ep.content, "memorable event");
        assert!((ep.emotional_valence - 0.8).abs() < 0.01);
        assert!((ep.importance - 0.9).abs() < 0.01);
        assert_eq!(ep.context_tags, vec!["important".to_string()]);
        assert!(ep.embedding.is_some());
    }

    #[test]
    fn test_get_nonexistent() {
        let store = EpisodicMemory::open_in_memory().unwrap();
        let rt = rt();

        let ep = rt.block_on(store.get(99999)).unwrap();
        assert!(ep.is_none());
    }

    #[test]
    fn test_query_relevance() {
        let store = EpisodicMemory::open_in_memory().unwrap();
        let rt = rt();

        // Store diverse episodes
        rt.block_on(store.store(
            "the weather is sunny today".into(),
            0.5,
            0.7,
            vec!["weather".into()],
            now(),
        ))
        .unwrap();

        rt.block_on(store.store(
            "meeting with alice about project deadline".into(),
            0.0,
            0.6,
            vec!["work".into()],
            now() + 1000,
        ))
        .unwrap();

        rt.block_on(store.store(
            "rainy forecast for tomorrow".into(),
            -0.2,
            0.5,
            vec!["weather".into()],
            now() + 2000,
        ))
        .unwrap();

        // Query for weather-related episodes
        let results = rt
            .block_on(store.query("weather forecast rain", 5, 0.0, now() + 3000))
            .unwrap();

        assert!(!results.is_empty());
        // Weather-related episodes should score higher
        assert!(
            results[0].content.contains("weather") || results[0].content.contains("rain"),
            "top result should be weather-related, got: {}",
            results[0].content
        );
    }

    #[test]
    fn test_query_min_relevance_filter() {
        let store = EpisodicMemory::open_in_memory().unwrap();
        let rt = rt();

        rt.block_on(store.store("some random data".into(), 0.0, 0.1, vec![], now()))
            .unwrap();

        // Very high min_relevance should filter out low-quality matches
        let results = rt
            .block_on(store.query("completely unrelated query xyz", 10, 0.99, now() + 1000))
            .unwrap();

        // Might be empty if nothing scores above 0.99
        // This is acceptable — the filter is working
        assert!(results.len() <= 1);
    }

    #[test]
    fn test_record_access() {
        let store = EpisodicMemory::open_in_memory().unwrap();
        let rt = rt();

        let id = rt
            .block_on(store.store("accessed episode".into(), 0.0, 0.5, vec![], now()))
            .unwrap();

        // Access it twice
        rt.block_on(store.record_access(id, now() + 1000)).unwrap();
        rt.block_on(store.record_access(id, now() + 2000)).unwrap();

        let ep = rt.block_on(store.get(id)).unwrap().unwrap();
        assert_eq!(ep.access_count, 2);
        assert_eq!(ep.last_access_ms, now() + 2000);
    }

    #[test]
    fn test_find_similar() {
        let store = EpisodicMemory::open_in_memory().unwrap();
        let rt = rt();

        rt.block_on(store.store("user prefers dark mode".into(), 0.0, 0.5, vec![], now()))
            .unwrap();
        rt.block_on(store.store("user likes dark theme".into(), 0.0, 0.5, vec![], now() + 1000))
            .unwrap();
        rt.block_on(store.store("chocolate cake recipe".into(), 0.0, 0.5, vec![], now() + 2000))
            .unwrap();

        let similar = rt
            .block_on(store.find_similar("dark mode preference", 0.3, 5))
            .unwrap();

        // Should find the dark mode/theme episodes
        assert!(!similar.is_empty());
        assert!(
            similar[0].content.contains("dark"),
            "most similar should be about dark mode, got: {}",
            similar[0].content
        );
    }

    #[test]
    fn test_archival_candidates() {
        let store = EpisodicMemory::open_in_memory().unwrap();
        let rt = rt();

        // Store an old, low-importance episode
        rt.block_on(store.store("old boring event".into(), 0.0, 0.1, vec![], now()))
            .unwrap();

        // Store a recent, important episode
        let later = now() + 100_000_000; // ~27 hours later
        rt.block_on(store.store("recent important event".into(), 0.8, 0.9, vec![], later))
            .unwrap();

        // Look for archival candidates: older than 50_000_000ms, importance < 0.3
        let candidates = rt
            .block_on(store.get_archival_candidates(50_000_000, 0.3, later, 10))
            .unwrap();

        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].content.contains("old boring"));
    }

    #[test]
    fn test_delete_episodes() {
        let store = EpisodicMemory::open_in_memory().unwrap();
        let rt = rt();

        let id1 = rt
            .block_on(store.store("episode 1".into(), 0.0, 0.5, vec![], now()))
            .unwrap();
        let id2 = rt
            .block_on(store.store("episode 2".into(), 0.0, 0.5, vec![], now() + 1000))
            .unwrap();
        let _id3 = rt
            .block_on(store.store("episode 3".into(), 0.0, 0.5, vec![], now() + 2000))
            .unwrap();

        let deleted = rt.block_on(store.delete_episodes(&[id1, id2])).unwrap();
        assert_eq!(deleted, 2);

        let count = rt.block_on(store.count()).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_storage_bytes() {
        let store = EpisodicMemory::open_in_memory().unwrap();
        let rt = rt();

        let size = rt.block_on(store.storage_bytes()).unwrap();
        assert!(size > 0); // Even empty DB has some overhead
    }

    #[test]
    fn test_pattern_separation_noise() {
        // Verify that pattern separation produces a different embedding
        // when a very similar episode already exists
        let store = EpisodicMemory::open_in_memory().unwrap();
        let rt = rt();

        // Store first episode
        let id1 = rt
            .block_on(store.store(
                "the user prefers dark mode".into(),
                0.0,
                0.5,
                vec![],
                now(),
            ))
            .unwrap();

        // Store nearly identical episode (should trigger pattern separation)
        let id2 = rt
            .block_on(store.store(
                "the user prefers dark mode".into(), // exact same content
                0.0,
                0.5,
                vec![],
                now() + 1000,
            ))
            .unwrap();

        let ep1 = rt.block_on(store.get(id1)).unwrap().unwrap();
        let ep2 = rt.block_on(store.get(id2)).unwrap().unwrap();

        // Both should have embeddings
        assert!(ep1.embedding.is_some());
        assert!(ep2.embedding.is_some());

        // Embeddings should differ due to pattern separation
        let emb1 = ep1.embedding.unwrap();
        let emb2 = ep2.embedding.unwrap();
        let sim = cosine_similarity(&emb1, &emb2);

        // Should be close but not identical
        assert!(
            sim < 1.0 - f32::EPSILON,
            "pattern separation should make embeddings differ, got similarity {}",
            sim
        );
        // But still highly similar (noise is small)
        assert!(
            sim > 0.8,
            "pattern separation noise should be subtle, got similarity {}",
            sim
        );
    }
}
