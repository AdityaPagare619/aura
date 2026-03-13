//! SQLite-backed offline message queue for Telegram.
//!
//! When the device is offline (or the Telegram API is unreachable), outgoing
//! messages are persisted in SQLite. When connectivity returns, messages are
//! dequeued in priority order and sent. Messages with the same `coalesce_key`
//! are merged (newer replaces older) to avoid spamming status updates.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, warn};

use bincode::config::standard as bincode_config;

use aura_types::errors::AuraError;

// ─── Types ──────────────────────────────────────────────────────────────────

/// Content of a queued message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageContent {
    /// Plain text (with optional parse_mode: "HTML" or "MarkdownV2").
    Text {
        text: String,
        parse_mode: Option<String>,
    },
    /// Photo with optional caption.
    Photo { data: Vec<u8>, caption: String },
    /// Edit an existing message.
    EditText {
        message_id: i64,
        text: String,
        parse_mode: Option<String>,
    },
}

/// Status of a queued message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueStatus {
    Pending,
    Sending,
    Sent,
    Failed,
    Expired,
}

impl QueueStatus {
    // Phase 8 wire point: as_str / from_str used by queue persistence layer
    // when serialising status to SQLite for crash recovery.
    #[allow(dead_code)]
    fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sending => "sending",
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Expired => "expired",
        }
    }

    #[allow(dead_code)]
    fn from_str(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "sending" => Self::Sending,
            "sent" => Self::Sent,
            "failed" => Self::Failed,
            "expired" => Self::Expired,
            _ => Self::Pending,
        }
    }
}

/// A message in the queue.
#[derive(Debug, Clone)]
pub struct QueuedMessage {
    pub id: i64,
    pub chat_id: i64,
    pub content: MessageContent,
    /// 0=low, 1=normal, 2=high, 3=critical.
    pub priority: u8,
    /// Unix timestamp (seconds).
    pub created_at: u64,
    /// Expire after this many seconds.
    pub ttl_seconds: u32,
    pub retry_count: u8,
    pub max_retries: u8,
    /// Messages with the same key replace each other.
    pub coalesce_key: Option<String>,
}

// ─── MessageQueue ───────────────────────────────────────────────────────────

/// Offline message queue backed by SQLite.
///
/// All SQLite operations are synchronous. Callers should wrap them in
/// `tokio::task::spawn_blocking` when called from async contexts.
pub struct MessageQueue {
    db: Connection,
}

impl MessageQueue {
    /// Open (or create) the queue in the given SQLite database.
    ///
    /// Creates the `telegram_queue` table if it doesn't exist.
    #[instrument(skip_all)]
    pub fn open(db: Connection) -> Result<Self, AuraError> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS telegram_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL,
                content BLOB NOT NULL,
                priority INTEGER DEFAULT 1,
                created_at INTEGER NOT NULL,
                ttl_seconds INTEGER DEFAULT 3600,
                retry_count INTEGER DEFAULT 0,
                max_retries INTEGER DEFAULT 3,
                coalesce_key TEXT,
                status TEXT DEFAULT 'pending'
            );
            CREATE INDEX IF NOT EXISTS idx_tq_status_priority
                ON telegram_queue(status, priority DESC, created_at ASC);
            CREATE INDEX IF NOT EXISTS idx_tq_coalesce
                ON telegram_queue(coalesce_key) WHERE coalesce_key IS NOT NULL;",
        )
        .map_err(|e| {
            AuraError::Memory(aura_types::errors::MemError::DatabaseError(format!(
                "failed to create telegram_queue: {e}"
            )))
        })?;

        debug!("telegram message queue initialized");
        Ok(Self { db })
    }

    /// Enqueue a new message. If a `coalesce_key` is set and a pending message
    /// with the same key exists, the old message is replaced.
    ///
    /// Returns the row ID.
    #[instrument(skip(self, content))]
    pub fn enqueue(
        &self,
        chat_id: i64,
        content: &MessageContent,
        priority: u8,
        ttl_seconds: u32,
        max_retries: u8,
        coalesce_key: Option<&str>,
    ) -> Result<i64, AuraError> {
        let content_bytes =
            bincode::serde::encode_to_vec(content, bincode_config()).map_err(|e| {
                AuraError::Memory(aura_types::errors::MemError::SerializationFailed(format!(
                    "queue serialize: {e}"
                )))
            })?;

        let now = unix_timestamp();

        // Coalesce: delete existing pending messages with the same key.
        if let Some(key) = coalesce_key {
            self.db
                .execute(
                    "DELETE FROM telegram_queue WHERE coalesce_key = ?1 AND status = 'pending'",
                    params![key],
                )
                .map_err(db_err)?;
        }

        self.db
            .execute(
                "INSERT INTO telegram_queue (chat_id, content, priority, created_at, ttl_seconds, max_retries, coalesce_key, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending')",
                params![
                    chat_id,
                    content_bytes,
                    priority as i32,
                    now as i64,
                    ttl_seconds as i32,
                    max_retries as i32,
                    coalesce_key,
                ],
            )
            .map_err(db_err)?;

        Ok(self.db.last_insert_rowid())
    }

    /// Dequeue a batch of pending messages, ordered by priority (highest first),
    /// then by creation time (oldest first).
    ///
    /// Marks dequeued messages as `sending`.
    #[instrument(skip(self))]
    pub fn dequeue_batch(&self, limit: usize) -> Result<Vec<QueuedMessage>, AuraError> {
        let mut stmt = self
            .db
            .prepare(
                "SELECT id, chat_id, content, priority, created_at, ttl_seconds, retry_count, max_retries, coalesce_key
                 FROM telegram_queue
                 WHERE status = 'pending'
                 ORDER BY priority DESC, created_at ASC
                 LIMIT ?1",
            )
            .map_err(db_err)?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                let id: i64 = row.get(0)?;
                let chat_id: i64 = row.get(1)?;
                let content_bytes: Vec<u8> = row.get(2)?;
                let priority: i32 = row.get(3)?;
                let created_at: i64 = row.get(4)?;
                let ttl_seconds: i32 = row.get(5)?;
                let retry_count: i32 = row.get(6)?;
                let max_retries: i32 = row.get(7)?;
                let coalesce_key: Option<String> = row.get(8)?;

                Ok((
                    id,
                    chat_id,
                    content_bytes,
                    priority,
                    created_at,
                    ttl_seconds,
                    retry_count,
                    max_retries,
                    coalesce_key,
                ))
            })
            .map_err(db_err)?;

        let mut messages = Vec::new();
        let mut ids_to_mark = Vec::new();

        for row_result in rows {
            let (
                id,
                chat_id,
                content_bytes,
                priority,
                created_at,
                ttl_seconds,
                retry_count,
                max_retries,
                coalesce_key,
            ) = row_result.map_err(db_err)?;

            let content: MessageContent =
                match bincode::serde::decode_from_slice(&content_bytes, bincode_config()) {
                    Ok((c, _)) => c,
                    Err(e) => {
                        warn!(id, "failed to deserialize queued message: {e}");
                        continue;
                    }
                };

            ids_to_mark.push(id);
            messages.push(QueuedMessage {
                id,
                chat_id,
                content,
                priority: priority as u8,
                created_at: created_at as u64,
                ttl_seconds: ttl_seconds as u32,
                retry_count: retry_count as u8,
                max_retries: max_retries as u8,
                coalesce_key,
            });
        }

        // Mark as sending.
        for id in &ids_to_mark {
            self.db
                .execute(
                    "UPDATE telegram_queue SET status = 'sending' WHERE id = ?1",
                    params![id],
                )
                .map_err(db_err)?;
        }

        debug!(count = messages.len(), "dequeued messages");
        Ok(messages)
    }

    /// Mark a message as successfully sent.
    pub fn mark_sent(&self, id: i64) -> Result<(), AuraError> {
        self.db
            .execute(
                "UPDATE telegram_queue SET status = 'sent' WHERE id = ?1",
                params![id],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Mark a message as failed. Increments retry_count. If retries exhausted,
    /// moves to `failed` status; otherwise back to `pending`.
    pub fn mark_failed(&self, id: i64) -> Result<(), AuraError> {
        self.db
            .execute(
                "UPDATE telegram_queue
                 SET retry_count = retry_count + 1,
                     status = CASE
                         WHEN retry_count + 1 >= max_retries THEN 'failed'
                         ELSE 'pending'
                     END
                 WHERE id = ?1",
                params![id],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Expire messages that have exceeded their TTL.
    ///
    /// Returns the number of expired messages.
    #[instrument(skip(self))]
    pub fn expire_old(&self) -> Result<usize, AuraError> {
        let now = unix_timestamp() as i64;
        let count = self
            .db
            .execute(
                "UPDATE telegram_queue
                 SET status = 'expired'
                 WHERE status IN ('pending', 'sending')
                   AND (created_at + ttl_seconds) < ?1",
                params![now],
            )
            .map_err(db_err)?;

        if count > 0 {
            debug!(count, "expired old messages");
        }
        Ok(count)
    }

    /// Clean up old sent/failed/expired messages older than `retention_secs`.
    pub fn cleanup(&self, retention_secs: u64) -> Result<usize, AuraError> {
        let cutoff = unix_timestamp().saturating_sub(retention_secs) as i64;
        let count = self
            .db
            .execute(
                "DELETE FROM telegram_queue
                 WHERE status IN ('sent', 'failed', 'expired')
                   AND created_at < ?1",
                params![cutoff],
            )
            .map_err(db_err)?;
        Ok(count)
    }

    /// Count of pending messages.
    pub fn pending_count(&self) -> Result<usize, AuraError> {
        let count: i64 = self
            .db
            .query_row(
                "SELECT COUNT(*) FROM telegram_queue WHERE status = 'pending'",
                [],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        Ok(count as usize)
    }

    /// Convenience: enqueue a simple text message with normal priority.
    pub fn enqueue_text(
        &self,
        chat_id: i64,
        text: &str,
        parse_mode: Option<&str>,
    ) -> Result<i64, AuraError> {
        self.enqueue(
            chat_id,
            &MessageContent::Text {
                text: text.to_string(),
                parse_mode: parse_mode.map(|s| s.to_string()),
            },
            1, // normal priority
            3600,
            3,
            None,
        )
    }

    /// Convenience: enqueue a critical message (high priority, longer TTL).
    pub fn enqueue_critical(&self, chat_id: i64, text: &str) -> Result<i64, AuraError> {
        self.enqueue(
            chat_id,
            &MessageContent::Text {
                text: text.to_string(),
                parse_mode: Some("HTML".to_string()),
            },
            3,     // critical priority
            86400, // 24h TTL
            5,
            None,
        )
    }
}

impl std::fmt::Debug for MessageQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageQueue")
            .field("pending", &self.pending_count().unwrap_or(0))
            .finish()
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn db_err(e: rusqlite::Error) -> AuraError {
    AuraError::Memory(aura_types::errors::MemError::DatabaseError(e.to_string()))
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_queue() -> MessageQueue {
        let db = Connection::open_in_memory().expect("in-memory db");
        MessageQueue::open(db).expect("open queue")
    }

    #[test]
    fn test_enqueue_and_dequeue() {
        let q = test_queue();
        let id = q.enqueue_text(42, "hello", None).unwrap();
        assert!(id > 0);
        assert_eq!(q.pending_count().unwrap(), 1);

        let batch = q.dequeue_batch(10).unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].chat_id, 42);
        match &batch[0].content {
            MessageContent::Text { text, .. } => assert_eq!(text, "hello"),
            _ => panic!("expected text message"),
        }

        // After dequeue, status is 'sending', so pending is 0.
        assert_eq!(q.pending_count().unwrap(), 0);
    }

    #[test]
    fn test_mark_sent() {
        let q = test_queue();
        let id = q.enqueue_text(42, "test", None).unwrap();
        let batch = q.dequeue_batch(10).unwrap();
        assert_eq!(batch.len(), 1);

        q.mark_sent(id).unwrap();
        // Sent messages should not reappear.
        let batch2 = q.dequeue_batch(10).unwrap();
        assert!(batch2.is_empty());
    }

    #[test]
    fn test_mark_failed_with_retry() {
        let q = test_queue();
        let id = q
            .enqueue(
                42,
                &MessageContent::Text {
                    text: "retry-me".into(),
                    parse_mode: None,
                },
                1,
                3600,
                2, // max_retries = 2
                None,
            )
            .unwrap();

        // First dequeue + fail.
        q.dequeue_batch(10).unwrap();
        q.mark_failed(id).unwrap();
        // Should be back in pending (retry_count=1, max=2).
        assert_eq!(q.pending_count().unwrap(), 1);

        // Second dequeue + fail.
        q.dequeue_batch(10).unwrap();
        q.mark_failed(id).unwrap();
        // Now retry_count=2 == max, so status='failed'.
        assert_eq!(q.pending_count().unwrap(), 0);
    }

    #[test]
    fn test_coalesce() {
        let q = test_queue();
        q.enqueue(
            42,
            &MessageContent::Text {
                text: "old".into(),
                parse_mode: None,
            },
            1,
            3600,
            3,
            Some("status"),
        )
        .unwrap();
        q.enqueue(
            42,
            &MessageContent::Text {
                text: "new".into(),
                parse_mode: None,
            },
            1,
            3600,
            3,
            Some("status"),
        )
        .unwrap();

        // Only the newest should remain.
        assert_eq!(q.pending_count().unwrap(), 1);
        let batch = q.dequeue_batch(10).unwrap();
        match &batch[0].content {
            MessageContent::Text { text, .. } => assert_eq!(text, "new"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn test_priority_ordering() {
        let q = test_queue();
        q.enqueue(
            42,
            &MessageContent::Text {
                text: "low".into(),
                parse_mode: None,
            },
            0,
            3600,
            3,
            None,
        )
        .unwrap();
        q.enqueue(
            42,
            &MessageContent::Text {
                text: "critical".into(),
                parse_mode: None,
            },
            3,
            3600,
            3,
            None,
        )
        .unwrap();
        q.enqueue(
            42,
            &MessageContent::Text {
                text: "normal".into(),
                parse_mode: None,
            },
            1,
            3600,
            3,
            None,
        )
        .unwrap();

        let batch = q.dequeue_batch(10).unwrap();
        assert_eq!(batch.len(), 3);
        // Critical first.
        match &batch[0].content {
            MessageContent::Text { text, .. } => assert_eq!(text, "critical"),
            _ => panic!("expected text"),
        }
    }
}
