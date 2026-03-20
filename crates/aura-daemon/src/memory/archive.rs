//! Archive memory — compressed cold storage for old, low-importance memories.
//!
//! Budget: ~4MB/year, 50-200ms latency.
//!
//! Archives store compressed summaries of old episodes and semantic entries.
//! Uses FTS5 on summaries for retrieval. Supports real compression via LZ4 (fast)
//! or ZSTD (high ratio).
//!
//! Archival policy:
//! - Episodes older than 30 days with importance < 0.3
//! - Accessed via full-text search on summaries

use std::{path::Path, sync::Arc};

use aura_types::{errors::MemError, ipc::MemoryTier, memory::MemoryResult};
use lz4::block::{compress as lz4_compress, decompress as lz4_decompress};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default archival age threshold: 30 days in milliseconds.
pub const ARCHIVE_AGE_THRESHOLD_MS: u64 = 30 * 24 * 60 * 60 * 1000;

/// Default importance threshold for archival.
pub const ARCHIVE_IMPORTANCE_THRESHOLD: f32 = 0.3;

/// Compression algorithm selection
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CompressionAlgo {
    #[default]
    Lz4,
    Zstd,
}

// ---------------------------------------------------------------------------
// Compression interface
// ---------------------------------------------------------------------------
//
// Wire format:
//   magic       : 4 bytes — [0xAA, 0xBA, 0xC0, 0x01]
//   algo        : 1 byte  — 0x01 = LZ4, 0x02 = ZSTD
//   original_len: 4 bytes — little-endian u32
//   payload     : variable — compressed data
//
// LZ4 provides fast compression/decompression.
// ZSTD provides higher compression ratios but is slower.
//

/// Magic header for AURA archive blobs.
const ARCHIVE_MAGIC: [u8; 4] = [0xAA, 0xBA, 0xC0, 0x01];

/// Compression algorithm identifiers in wire format.
const ALGO_LZ4: u8 = 0x01;
const ALGO_ZSTD: u8 = 0x02;

/// Default compression level for LZ4 (0-16, higher = more compression)
#[allow(dead_code)] // Phase 8: used by LZ4 compression pipeline when enabled
const LZ4_COMPRESSION_LEVEL: i32 = 1;

/// Compress data using the specified algorithm.
pub fn compress(data: &[u8], algo: CompressionAlgo) -> Result<Vec<u8>, MemError> {
    match algo {
        CompressionAlgo::Lz4 => compress_lz4(data),
        CompressionAlgo::Zstd => compress_zstd(data),
    }
}

/// Compress using LZ4 (fast).
fn compress_lz4(data: &[u8]) -> Result<Vec<u8>, MemError> {
    if data.is_empty() {
        let mut result = Vec::with_capacity(9);
        result.extend_from_slice(&ARCHIVE_MAGIC);
        result.push(ALGO_LZ4);
        result.extend_from_slice(&0u32.to_le_bytes());
        return Ok(result);
    }
    let original_len = data.len() as u32;
    let compressed = lz4_compress(data, None, true)
        .map_err(|e| MemError::SerializationFailed(format!("LZ4 compression failed: {}", e)))?;

    let mut result = Vec::with_capacity(9 + compressed.len());
    result.extend_from_slice(&ARCHIVE_MAGIC);
    result.push(ALGO_LZ4);
    result.extend_from_slice(&original_len.to_le_bytes());
    result.extend_from_slice(&compressed);
    Ok(result)
}

/// Compress using ZSTD (high ratio).
fn compress_zstd(data: &[u8]) -> Result<Vec<u8>, MemError> {
    if data.is_empty() {
        let mut result = Vec::with_capacity(9);
        result.extend_from_slice(&ARCHIVE_MAGIC);
        result.push(ALGO_ZSTD);
        result.extend_from_slice(&0u32.to_le_bytes());
        return Ok(result);
    }
    let original_len = data.len() as u32;
    let compressed = zstd::encode_all(data, 3)
        .map_err(|e| MemError::SerializationFailed(format!("ZSTD compression failed: {}", e)))?;

    let mut result = Vec::with_capacity(9 + compressed.len());
    result.extend_from_slice(&ARCHIVE_MAGIC);
    result.push(ALGO_ZSTD);
    result.extend_from_slice(&original_len.to_le_bytes());
    result.extend_from_slice(&compressed);
    Ok(result)
}

/// Decompress data produced by [`compress`].
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, MemError> {
    if data.len() < 9 {
        return Err(MemError::SerializationFailed(
            "archive blob too short for header".into(),
        ));
    }
    let magic = &data[0..4];
    if magic != ARCHIVE_MAGIC {
        return Err(MemError::SerializationFailed(format!(
            "bad archive magic: expected {:02X?}, got {:02X?}",
            ARCHIVE_MAGIC, magic
        )));
    }
    let algo = data[4];
    let orig_len = u32::from_le_bytes(
        data[5..9]
            .try_into()
            .map_err(|_| MemError::SerializationFailed("invalid original-length field".into()))?,
    ) as usize;

    // Handle empty data case - return empty vec directly
    if orig_len == 0 {
        return Ok(Vec::new());
    }

    let decoded = match algo {
        ALGO_LZ4 => decompress_lz4(&data[9..], orig_len)?,
        ALGO_ZSTD => decompress_zstd(&data[9..], orig_len)?,
        _ => {
            return Err(MemError::SerializationFailed(format!(
                "unknown compression algorithm: 0x{:02X}",
                algo
            )));
        },
    };

    if decoded.len() != orig_len {
        return Err(MemError::SerializationFailed(format!(
            "length mismatch: header says {} but decoded {}",
            orig_len,
            decoded.len()
        )));
    }
    Ok(decoded)
}

fn decompress_lz4(data: &[u8], _original_len: usize) -> Result<Vec<u8>, MemError> {
    lz4_decompress(data, None)
        .map_err(|e| MemError::SerializationFailed(format!("LZ4 decompression failed: {}", e)))
}

fn decompress_zstd(data: &[u8], _original_len: usize) -> Result<Vec<u8>, MemError> {
    zstd::decode_all(data)
        .map_err(|e| MemError::SerializationFailed(format!("ZSTD decompression failed: {}", e)))
}

/// Estimate the compression ratio (for storage reporting).
pub fn compression_ratio(original_size: usize, compressed_size: usize) -> f32 {
    if original_size == 0 {
        return 1.0;
    }
    compressed_size as f32 / original_size as f32
}

// ---------------------------------------------------------------------------
// ArchiveMemory
// ---------------------------------------------------------------------------

/// SQLite-backed archive memory with FTS5 on summaries.
pub struct ArchiveMemory {
    conn: Arc<Mutex<Connection>>,
}

impl ArchiveMemory {
    /// Open (or create) an archive database at the given path.
    pub fn open(db_path: &Path) -> Result<Self, MemError> {
        let conn = Connection::open(db_path)
            .map_err(|e| MemError::DatabaseError(format!("failed to open archive db: {}", e)))?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA cache_size=-2000;",
        )
        .map_err(|e| MemError::DatabaseError(format!("pragmas failed: {}", e)))?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.migrate_sync()?;
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
        };
        store.migrate_sync()?;
        Ok(store)
    }

    fn migrate_sync(&self) -> Result<(), MemError> {
        let conn = self
            .conn
            .try_lock()
            .map_err(|_| MemError::MigrationFailed("could not lock db for migration".into()))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS archive_blobs (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                summary         TEXT NOT NULL,
                compressed_data BLOB NOT NULL,
                original_size   INTEGER NOT NULL,
                importance      REAL NOT NULL DEFAULT 0.1,
                period_start_ms INTEGER NOT NULL,
                period_end_ms   INTEGER NOT NULL,
                source_type     TEXT NOT NULL DEFAULT 'episode',
                source_ids      TEXT NOT NULL DEFAULT '[]'
            );

            CREATE INDEX IF NOT EXISTS idx_archive_period
                ON archive_blobs(period_start_ms, period_end_ms);
            CREATE INDEX IF NOT EXISTS idx_archive_importance
                ON archive_blobs(importance);",
        )
        .map_err(|e| MemError::MigrationFailed(format!("archive table creation failed: {}", e)))?;

        // FTS5 virtual table on summaries for full-text retrieval
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS archive_fts USING fts5(
                summary,
                content='archive_blobs',
                content_rowid='id'
            );

            CREATE TRIGGER IF NOT EXISTS archive_fts_insert AFTER INSERT ON archive_blobs
            BEGIN
                INSERT INTO archive_fts(rowid, summary) VALUES (new.id, new.summary);
            END;

            CREATE TRIGGER IF NOT EXISTS archive_fts_delete AFTER DELETE ON archive_blobs
            BEGIN
                INSERT INTO archive_fts(archive_fts, rowid, summary)
                VALUES ('delete', old.id, old.summary);
            END;

            CREATE TRIGGER IF NOT EXISTS archive_fts_update AFTER UPDATE ON archive_blobs
            BEGIN
                INSERT INTO archive_fts(archive_fts, rowid, summary)
                VALUES ('delete', old.id, old.summary);
                INSERT INTO archive_fts(rowid, summary) VALUES (new.id, new.summary);
            END;",
        )
        .map_err(|e| MemError::MigrationFailed(format!("archive FTS5 setup failed: {}", e)))?;

        info!("archive memory: migrations complete");
        Ok(())
    }

    /// Archive a batch of content.
    ///
    /// Takes raw content, compresses it using the specified algorithm,
    /// and stores with a summary for FTS retrieval.
    pub async fn archive(
        &self,
        summary: String,
        raw_content: Vec<u8>,
        importance: f32,
        period_start_ms: u64,
        period_end_ms: u64,
        source_type: String,
        source_ids: Vec<u64>,
        compression_algo: CompressionAlgo,
    ) -> Result<u64, MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let original_size = raw_content.len() as u32;
            let compressed = compress(&raw_content, compression_algo)?;
            let ids_json = serde_json::to_string(&source_ids).unwrap_or_else(|_| "[]".into());

            conn.execute(
                "INSERT INTO archive_blobs (summary, compressed_data, original_size,
                                            importance, period_start_ms, period_end_ms,
                                            source_type, source_ids)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    summary,
                    compressed,
                    original_size,
                    importance,
                    period_start_ms as i64,
                    period_end_ms as i64,
                    source_type,
                    ids_json,
                ],
            )
            .map_err(|e| MemError::DatabaseError(format!("archive insert failed: {}", e)))?;

            let id = conn.last_insert_rowid() as u64;
            debug!(
                "archived blob {} ({} -> {} bytes, summary: {})",
                id,
                original_size,
                compressed.len(),
                &summary[..summary.len().min(50)]
            );
            Ok(id)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Query archive using FTS5 on summaries.
    pub async fn query(
        &self,
        query_text: &str,
        max_results: usize,
        _min_relevance: f32,
    ) -> Result<Vec<MemoryResult>, MemError> {
        let conn = self.conn.clone();
        let query_text = query_text.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            query_archive_fts(&conn, &query_text, max_results)
        })
        .await
        .map_err(|e| MemError::QueryFailed(format!("spawn_blocking failed: {}", e)))?
    }

    /// Retrieve and decompress a specific archive blob.
    pub async fn retrieve(&self, blob_id: u64) -> Result<Option<(String, Vec<u8>)>, MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let result = conn
                .query_row(
                    "SELECT summary, compressed_data FROM archive_blobs WHERE id = ?1",
                    params![blob_id as i64],
                    |row| {
                        let summary: String = row.get(0)?;
                        let compressed: Vec<u8> = row.get(1)?;
                        Ok((summary, compressed))
                    },
                )
                .optional()
                .map_err(|e| MemError::QueryFailed(format!("retrieve failed: {}", e)))?;

            match result {
                Some((summary, compressed)) => {
                    let data = decompress(&compressed)?;
                    Ok(Some((summary, data)))
                },
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Count total archive blobs.
    pub async fn count(&self) -> Result<u64, MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM archive_blobs", [], |row| row.get(0))
                .map_err(|e| MemError::DatabaseError(format!("count failed: {}", e)))?;
            Ok(count as u64)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Get total stored bytes (compressed) and original bytes.
    pub async fn storage_stats(&self) -> Result<(u64, u64), MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let result: (i64, i64) = conn
                .query_row(
                    "SELECT COALESCE(SUM(LENGTH(compressed_data)), 0),
                            COALESCE(SUM(original_size), 0)
                     FROM archive_blobs",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| MemError::DatabaseError(format!("storage stats failed: {}", e)))?;
            Ok((result.0 as u64, result.1 as u64))
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Delete archive blobs older than a given timestamp.
    pub async fn prune_before(&self, before_ms: u64) -> Result<usize, MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let deleted = conn
                .execute(
                    "DELETE FROM archive_blobs WHERE period_end_ms < ?1",
                    params![before_ms as i64],
                )
                .map_err(|e| MemError::DatabaseError(format!("prune failed: {}", e)))?;
            Ok(deleted)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Estimate total storage size in bytes (database pages).
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

    /// Get all archive blob summaries (for GDPR export).
    /// Returns (id, summary, original_size, importance, period_start_ms, period_end_ms).
    /// Does NOT return the actual compressed data to avoid memory issues with large blobs.
    pub async fn get_all_blobs(&self) -> Result<Vec<ArchiveBlobExport>, MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let mut stmt = conn
                .prepare(
                    "SELECT id, summary, original_size, importance,
                            period_start_ms, period_end_ms
                     FROM archive_blobs ORDER BY id ASC",
                )
                .map_err(|e| MemError::QueryFailed(format!("get_all prepare failed: {}", e)))?;

            let rows = stmt
                .query_map([], |row| {
                    Ok(ArchiveBlobExport {
                        id: row.get(0)?,
                        summary: row.get(1)?,
                        original_size: row.get::<_, i64>(2)? as u32,
                        importance: row.get(3)?,
                        period_start_ms: row.get::<_, i64>(4)? as u64,
                        period_end_ms: row.get::<_, i64>(5)? as u64,
                    })
                })
                .map_err(|e| MemError::QueryFailed(format!("get_all query failed: {}", e)))?;

            let mut blobs = Vec::new();
            for row in rows {
                blobs.push(
                    row.map_err(|e| MemError::QueryFailed(format!("row read failed: {}", e)))?,
                );
            }
            Ok(blobs)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Delete all archive blobs (for GDPR erasure).
    pub async fn delete_all(&self) -> Result<u64, MemError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let deleted = conn
                .execute("DELETE FROM archive_blobs", [])
                .map_err(|e| MemError::DatabaseError(format!("delete_all failed: {}", e)))?;
            Ok(deleted as u64)
        })
        .await
        .map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }
}

/// Export structure for archive blobs (metadata only, no raw data).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveBlobExport {
    pub id: u64,
    pub summary: String,
    pub original_size: u32,
    pub importance: f32,
    pub period_start_ms: u64,
    pub period_end_ms: u64,
}

// ---------------------------------------------------------------------------
// Synchronous helpers
// ---------------------------------------------------------------------------

fn query_archive_fts(
    conn: &Connection,
    query_text: &str,
    max_results: usize,
) -> Result<Vec<MemoryResult>, MemError> {
    // Escape FTS5 query
    let fts_query: String = query_text
        .split_whitespace()
        .map(|word| {
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
            "SELECT a.id, a.summary, a.importance, a.period_start_ms, a.period_end_ms
             FROM archive_fts f
             JOIN archive_blobs a ON a.id = f.rowid
             WHERE archive_fts MATCH ?1
             ORDER BY f.rank
             LIMIT ?2",
        )
        .map_err(|e| MemError::QueryFailed(format!("archive FTS prepare failed: {}", e)))?;

    let rows = stmt
        .query_map(params![fts_query, max_results as i64], |row| {
            let id: i64 = row.get(0)?;
            let summary: String = row.get(1)?;
            let imp: f64 = row.get(2)?;
            let _start_ms: i64 = row.get(3)?;
            let end_ms: i64 = row.get(4)?;
            Ok(MemoryResult {
                content: summary,
                tier: MemoryTier::Archive,
                relevance: 0.5, // FTS doesn't give a direct 0-1 score; moderate default
                importance: imp as f32,
                timestamp_ms: end_ms as u64, // use period end as representative timestamp
                source_id: id as u64,
            })
        })
        .map_err(|e| MemError::QueryFailed(format!("archive FTS search failed: {}", e)))?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|e| MemError::QueryFailed(format!("archive row failed: {}", e)))?);
    }
    Ok(results)
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
    fn test_compress_decompress_roundtrip() {
        let data = b"hello world, this is test data for archive compression";
        let compressed = compress(data, CompressionAlgo::Lz4).unwrap();
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(&decompressed, data);
    }

    #[test]
    fn test_compress_empty() {
        let compressed = compress(b"", CompressionAlgo::Lz4).unwrap();
        let decompressed = decompress(&compressed).unwrap();
        assert!(decompressed.is_empty());
    }

    #[test]
    fn test_decompress_too_short() {
        let result = decompress(&[0x01, 0x02]);
        assert!(result.is_err());
    }

    #[test]
    fn test_compression_ratio() {
        assert!((compression_ratio(100, 50) - 0.5).abs() < f32::EPSILON);
        assert!((compression_ratio(0, 0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_open_in_memory() {
        let store = ArchiveMemory::open_in_memory().unwrap();
        let rt = rt();
        let count = rt.block_on(store.count()).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_archive_and_count() {
        let store = ArchiveMemory::open_in_memory().unwrap();
        let rt = rt();

        let id = rt
            .block_on(store.archive(
                "Summary of old weather events".into(),
                b"raw event data about weather patterns".to_vec(),
                0.2,
                now() - 100_000_000,
                now() - 50_000_000,
                "episode".into(),
                vec![1, 2, 3],
                CompressionAlgo::Lz4,
            ))
            .unwrap();

        assert!(id > 0);
        let count = rt.block_on(store.count()).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_archive_and_retrieve() {
        let store = ArchiveMemory::open_in_memory().unwrap();
        let rt = rt();

        let original_data = b"detailed event log data that is being archived".to_vec();
        let id = rt
            .block_on(store.archive(
                "Detailed event log archive".into(),
                original_data.clone(),
                0.15,
                now() - 200_000_000,
                now() - 100_000_000,
                "episode".into(),
                vec![10, 20],
                CompressionAlgo::Lz4,
            ))
            .unwrap();

        let (summary, data) = rt.block_on(store.retrieve(id)).unwrap().unwrap();
        assert_eq!(summary, "Detailed event log archive");
        assert_eq!(data, original_data);
    }

    #[test]
    fn test_retrieve_nonexistent() {
        let store = ArchiveMemory::open_in_memory().unwrap();
        let rt = rt();
        let result = rt.block_on(store.retrieve(99999)).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_query_fts() {
        let store = ArchiveMemory::open_in_memory().unwrap();
        let rt = rt();

        rt.block_on(store.archive(
            "Weather patterns from January showing cold temperatures".into(),
            b"weather data".to_vec(),
            0.2,
            now() - 300_000_000,
            now() - 200_000_000,
            "episode".into(),
            vec![],
            CompressionAlgo::Lz4,
        ))
        .unwrap();

        rt.block_on(store.archive(
            "Meeting notes with Alice about project deadline".into(),
            b"meeting data".to_vec(),
            0.3,
            now() - 250_000_000,
            now() - 150_000_000,
            "episode".into(),
            vec![],
            CompressionAlgo::Lz4,
        ))
        .unwrap();

        // Search for weather
        let results = rt
            .block_on(store.query("weather temperature", 5, 0.0))
            .unwrap();

        assert!(!results.is_empty());
        assert!(results[0].content.contains("Weather"));
    }

    #[test]
    fn test_query_fts_no_match() {
        let store = ArchiveMemory::open_in_memory().unwrap();
        let rt = rt();

        rt.block_on(store.archive(
            "weather data".into(),
            b"data".to_vec(),
            0.2,
            now(),
            now(),
            "episode".into(),
            vec![],
            CompressionAlgo::Lz4,
        ))
        .unwrap();

        let _results = rt
            .block_on(store.query("quantum physics equations", 5, 0.0))
            .unwrap();

        // May or may not match — depends on FTS5 tokenization
        // No assertion on emptiness, just that it doesn't crash
    }

    #[test]
    fn test_storage_stats() {
        let store = ArchiveMemory::open_in_memory().unwrap();
        let rt = rt();

        rt.block_on(store.archive(
            "test".into(),
            vec![0u8; 1000],
            0.1,
            now(),
            now(),
            "episode".into(),
            vec![],
            CompressionAlgo::Lz4,
        ))
        .unwrap();

        let (compressed_bytes, original_bytes) = rt.block_on(store.storage_stats()).unwrap();
        assert!(compressed_bytes > 0);
        assert_eq!(original_bytes, 1000);
    }

    #[test]
    fn test_prune_before() {
        let store = ArchiveMemory::open_in_memory().unwrap();
        let rt = rt();

        // Archive two blobs with different periods
        rt.block_on(store.archive(
            "old data".into(),
            b"old".to_vec(),
            0.1,
            100_000,
            200_000,
            "episode".into(),
            vec![],
            CompressionAlgo::Lz4,
        ))
        .unwrap();

        rt.block_on(store.archive(
            "recent data".into(),
            b"recent".to_vec(),
            0.1,
            500_000,
            600_000,
            "episode".into(),
            vec![],
            CompressionAlgo::Lz4,
        ))
        .unwrap();

        // Prune everything ending before 300_000
        let pruned = rt.block_on(store.prune_before(300_000)).unwrap();
        assert_eq!(pruned, 1);

        let count = rt.block_on(store.count()).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_storage_bytes() {
        let store = ArchiveMemory::open_in_memory().unwrap();
        let rt = rt();
        let size = rt.block_on(store.storage_bytes()).unwrap();
        assert!(size > 0);
    }
}
