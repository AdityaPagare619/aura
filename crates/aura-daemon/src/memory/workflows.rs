//! Persistent storage for observed workflows.
//!
//! Stores patterns extracted by `WorkflowObserver` for long-term retention
//! and automation synthesis. This forms the foundation for M9.

use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use rusqlite::{params, Connection};
use tracing::{info, debug};
use aura_types::errors::MemError;
use crate::execution::learning::workflows::WorkflowPattern;

/// Stores recurring workflow patterns extracted from execution traces.
#[derive(Clone)]
pub struct WorkflowMemory {
    conn: Arc<Mutex<Connection>>,
}

impl WorkflowMemory {
    /// Open a SQLite database for workflow patterns backed by a file.
    pub fn open(path: &Path) -> Result<Self, MemError> {
        let conn = Connection::open(path)
            .map_err(|e| MemError::DatabaseError(format!("workflow db open failed: {}", e)))?;

        Self::migrate(&conn)?;

        info!("workflow memory initialized at {:?}", path);
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self, MemError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| MemError::DatabaseError(format!("in-memory workflow open failed: {}", e)))?;

        Self::migrate(&conn)?;

        debug!("workflow memory initialized in-memory (test mode)");
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    fn migrate(conn: &Connection) -> Result<(), MemError> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS workflow_patterns (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 sequence_json TEXT NOT NULL UNIQUE,
                 frequency INTEGER NOT NULL,
                 avg_time_ms INTEGER NOT NULL,
                 last_observed_ms INTEGER NOT NULL
             );"
        )
        .map_err(|e| MemError::MigrationFailed(format!("workflow db migration failed: {}", e)))
    }

    /// Store or update an observed workflow pattern.
    pub async fn store(&self, pattern: &WorkflowPattern, now_ms: u64) -> Result<u64, MemError> {
        let conn = self.conn.clone();
        let p = pattern.clone();
        
        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();
            let seq_json = serde_json::to_string(&p.sequence)
                .map_err(|e| MemError::DatabaseError(format!("json serialized failed: {}", e)))?;

            // Insert new pattern or update existing (adding frequencies and updating moving average)
            conn.execute(
                "INSERT INTO workflow_patterns (sequence_json, frequency, avg_time_ms, last_observed_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(sequence_json) DO UPDATE SET
                    frequency = frequency + ?2,
                    avg_time_ms = (avg_time_ms + ?3) / 2,
                    last_observed_ms = MAX(last_observed_ms, ?4)",
                params![seq_json, p.frequency as i64, p.avg_time_ms as i64, now_ms as i64]
            ).map_err(|e| MemError::DatabaseError(format!("workflow store failed: {}", e)))?;

            let correct_id: i64 = conn.query_row(
                "SELECT id FROM workflow_patterns WHERE sequence_json = ?1",
                params![seq_json],
                |row| row.get(0)
            ).map_err(|e| MemError::DatabaseError(format!("failed to fetch id: {}", e)))?;

            debug!("stored workflow pattern {} (freq {})", correct_id, p.frequency);
            Ok(correct_id as u64)
        }).await.map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }

    /// Retrieve all stored workflow patterns.
    pub async fn get_all(&self) -> Result<Vec<(u64, WorkflowPattern)>, MemError> {
         let conn = self.conn.clone();
         
         tokio::task::spawn_blocking(move || {
             let conn = conn.blocking_lock();
             let mut stmt = conn.prepare(
                 "SELECT id, sequence_json, frequency, avg_time_ms FROM workflow_patterns ORDER BY frequency DESC"
             ).map_err(|e| MemError::QueryFailed(format!("workflow query prepare failed: {}", e)))?;

             let rows = stmt.query_map([], |row| {
                 let id: i64 = row.get(0)?;
                 let seq_json: String = row.get(1)?;
                 let frequency: i64 = row.get(2)?;
                 let avg_time_ms: i64 = row.get(3)?;
                 Ok((id as u64, seq_json, frequency as u32, avg_time_ms as u64))
             }).map_err(|e| MemError::QueryFailed(format!("workflow query failed: {}", e)))?;

             let mut results = Vec::new();
             for r in rows {
                 let (id, seq_json, frequency, avg_time_ms) = r.map_err(|e| MemError::QueryFailed(e.to_string()))?;
                 if let Ok(sequence) = serde_json::from_str(&seq_json) {
                     results.push((id, WorkflowPattern { sequence, frequency, avg_time_ms }));
                 }
             }
             Ok(results)
         }).await.map_err(|e| MemError::DatabaseError(format!("spawn_blocking failed: {}", e)))?
    }
}
