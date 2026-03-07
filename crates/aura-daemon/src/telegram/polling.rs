//! Pure HTTP long-polling client for the Telegram Bot API.
//!
//! No teloxide dependency — we use raw HTTP requests via `ureq` (sync) or
//! `reqwest` (async). Since the daemon already has `tokio`, we implement
//! async polling with `tokio::time` for the long-poll timeout and manual
//! HTTP via `std::net::TcpStream` + TLS, or mock for testing.
//!
//! In production this would use `reqwest`. For compilation without extra deps,
//! we define the API surface and use a trait for the HTTP backend.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, info, instrument, warn};

use aura_types::errors::AuraError;

use super::queue::{MessageContent, MessageQueue};

// ─── Telegram API types ─────────────────────────────────────────────────────

/// Subset of Telegram's `Update` object that we care about.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub chat_id: i64,
    pub from_user_id: Option<i64>,
    pub text: Option<String>,
    pub message_id: Option<i64>,
    /// Callback query data (from inline keyboards).
    pub callback_data: Option<String>,
}

/// Result wrapper matching Telegram's `{ ok: bool, result: T }` envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramApiResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
}

/// Telegram message send result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentMessage {
    pub message_id: i64,
    pub chat_id: i64,
}

// ─── HTTP Backend trait ─────────────────────────────────────────────────────

/// Abstraction over the HTTP client for testability.
///
/// In production, this is implemented with `reqwest`. In tests, we use
/// a mock that returns canned responses.
#[async_trait::async_trait]
pub trait HttpBackend: Send + Sync {
    /// GET request returning the response body as bytes.
    async fn get(&self, url: &str) -> Result<Vec<u8>, AuraError>;

    /// POST request with JSON body, returning response body as bytes.
    async fn post_json(&self, url: &str, body: &[u8]) -> Result<Vec<u8>, AuraError>;

    /// POST multipart (for photo upload).
    async fn post_multipart(
        &self,
        url: &str,
        fields: Vec<(&str, String)>,
        file_field: Option<(&str, Vec<u8>, &str)>,
    ) -> Result<Vec<u8>, AuraError>;
}

/// Stub HTTP backend that returns network errors.
///
/// Used as the default when no real HTTP client is configured.
/// Replace with `reqwest`-based implementation in production.
pub struct StubHttpBackend;

#[async_trait::async_trait]
impl HttpBackend for StubHttpBackend {
    async fn get(&self, _url: &str) -> Result<Vec<u8>, AuraError> {
        Err(AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed))
    }

    async fn post_json(&self, _url: &str, _body: &[u8]) -> Result<Vec<u8>, AuraError> {
        Err(AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed))
    }

    async fn post_multipart(
        &self,
        _url: &str,
        _fields: Vec<(&str, String)>,
        _file_field: Option<(&str, Vec<u8>, &str)>,
    ) -> Result<Vec<u8>, AuraError> {
        Err(AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed))
    }
}

// ─── TelegramPoller ─────────────────────────────────────────────────────────

/// Telegram Bot API long-polling client.
///
/// Manages the poll loop, outgoing message queue flushing, and raw API calls.
pub struct TelegramPoller {
    bot_token: String,
    base_url: String,
    offset: i64,
    allowed_chat_ids: Vec<i64>,
    http: Box<dyn HttpBackend>,
    /// Long-poll timeout in seconds.
    poll_timeout: u32,
}

/// Telegram message limit (UTF-8 code points).
pub const MAX_MESSAGE_LENGTH: usize = 4096;

impl TelegramPoller {
    /// Create a new poller.
    ///
    /// `bot_token` is the Telegram Bot API token (e.g., `123456:ABC-DEF...`).
    pub fn new(
        bot_token: String,
        allowed_chat_ids: Vec<i64>,
        http: Box<dyn HttpBackend>,
    ) -> Self {
        let base_url = format!("https://api.telegram.org/bot{bot_token}");
        Self {
            bot_token,
            base_url,
            offset: 0,
            allowed_chat_ids,
            http,
            poll_timeout: 30,
        }
    }

    /// Run the main polling loop.
    ///
    /// Incoming updates (from whitelisted chat IDs) are sent to `tx`.
    /// Outgoing messages are flushed from `queue` each iteration.
    ///
    /// This loop runs forever until the channel is closed or an unrecoverable
    /// error occurs.
    #[instrument(skip_all, name = "telegram_poll_loop")]
    pub async fn poll_loop(
        &mut self,
        tx: &mpsc::Sender<TelegramUpdate>,
        queue: &MessageQueue,
    ) -> Result<(), AuraError> {
        info!(
            poll_timeout = self.poll_timeout,
            allowed_chats = self.allowed_chat_ids.len(),
            "starting Telegram poll loop"
        );

        loop {
            // 1. Flush outgoing queue.
            if let Err(e) = self.flush_queue(queue).await {
                warn!(error = %e, "failed to flush outgoing queue");
            }

            // 2. Long-poll for updates.
            match self.get_updates().await {
                Ok(updates) => {
                    for update in updates {
                        // Security Layer 1: chat_id whitelist (fast reject).
                        if !self.allowed_chat_ids.contains(&update.chat_id) {
                            warn!(chat_id = update.chat_id, "rejected unauthorized chat_id");
                            continue;
                        }

                        // Advance offset to acknowledge this update.
                        if update.update_id >= self.offset {
                            self.offset = update.update_id + 1;
                        }

                        // Forward to handler.
                        if tx.send(update).await.is_err() {
                            info!("update channel closed — exiting poll loop");
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "getUpdates failed — will retry after backoff");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }

    // ─── Raw API calls ──────────────────────────────────────────────────

    /// Call `getUpdates` with long-polling.
    async fn get_updates(&self) -> Result<Vec<TelegramUpdate>, AuraError> {
        let url = format!(
            "{}/getUpdates?offset={}&timeout={}&allowed_updates=[\"message\",\"callback_query\"]",
            self.base_url, self.offset, self.poll_timeout
        );

        let body = self.http.get(&url).await?;
        let resp: TelegramApiResponse<Vec<serde_json::Value>> =
            serde_json::from_slice(&body).map_err(|_e| {
                AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed)
            })?;

        if !resp.ok {
            return Err(AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed));
        }

        let raw_updates = resp.result.unwrap_or_default();
        let mut updates = Vec::with_capacity(raw_updates.len());

        for raw in raw_updates {
            if let Some(update) = parse_update(&raw) {
                updates.push(update);
            }
        }

        debug!(count = updates.len(), "received updates");
        Ok(updates)
    }

    /// Send a text message to a chat.
    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        parse_mode: Option<&str>,
    ) -> Result<SentMessage, AuraError> {
        let truncated = truncate_message(text);

        let mut payload = serde_json::json!({
            "chat_id": chat_id,
            "text": truncated,
        });
        if let Some(pm) = parse_mode {
            payload["parse_mode"] = serde_json::Value::String(pm.to_string());
        }

        let body_bytes = serde_json::to_vec(&payload).map_err(|_| {
            AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed)
        })?;

        let url = format!("{}/sendMessage", self.base_url);
        let resp_bytes = self.http.post_json(&url, &body_bytes).await?;

        let resp: TelegramApiResponse<SentMessage> =
            serde_json::from_slice(&resp_bytes).map_err(|_| {
                AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed)
            })?;

        resp.result.ok_or_else(|| {
            AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed)
        })
    }

    /// Edit an existing message.
    pub async fn edit_message(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        parse_mode: Option<&str>,
    ) -> Result<(), AuraError> {
        let truncated = truncate_message(text);

        let mut payload = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": truncated,
        });
        if let Some(pm) = parse_mode {
            payload["parse_mode"] = serde_json::Value::String(pm.to_string());
        }

        let body_bytes = serde_json::to_vec(&payload).map_err(|_| {
            AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed)
        })?;

        let url = format!("{}/editMessageText", self.base_url);
        let _ = self.http.post_json(&url, &body_bytes).await?;
        Ok(())
    }

    /// Send a photo with caption.
    pub async fn send_photo(
        &self,
        chat_id: i64,
        photo_data: &[u8],
        caption: &str,
    ) -> Result<SentMessage, AuraError> {
        let url = format!("{}/sendPhoto", self.base_url);
        let fields = vec![
            ("chat_id", chat_id.to_string()),
            ("caption", caption.to_string()),
        ];
        let file = Some(("photo", photo_data.to_vec(), "screenshot.png"));

        let resp_bytes = self.http.post_multipart(&url, fields, file).await?;

        let resp: TelegramApiResponse<SentMessage> =
            serde_json::from_slice(&resp_bytes).map_err(|_| {
                AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed)
            })?;

        resp.result.ok_or_else(|| {
            AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed)
        })
    }

    // ─── Queue flushing ─────────────────────────────────────────────────

    /// Send all pending messages from the offline queue.
    async fn flush_queue(&self, queue: &MessageQueue) -> Result<(), AuraError> {
        // Use spawn_blocking for the SQLite read.
        // In real code: let batch = tokio::task::spawn_blocking(|| queue.dequeue_batch(10)).await??;
        // For now, since MessageQueue isn't Send, we call directly (acceptable for single-threaded runtime).
        let batch = queue.dequeue_batch(10)?;

        for msg in &batch {
            let result = match &msg.content {
                MessageContent::Text { text, parse_mode } => {
                    self.send_message(msg.chat_id, text, parse_mode.as_deref())
                        .await
                        .map(|_| ())
                }
                MessageContent::Photo { data, caption } => {
                    self.send_photo(msg.chat_id, data, caption)
                        .await
                        .map(|_| ())
                }
                MessageContent::EditText {
                    message_id,
                    text,
                    parse_mode,
                } => {
                    self.edit_message(msg.chat_id, *message_id, text, parse_mode.as_deref())
                        .await
                }
            };

            match result {
                Ok(()) => {
                    queue.mark_sent(msg.id)?;
                    debug!(id = msg.id, "message sent from queue");
                }
                Err(e) => {
                    warn!(id = msg.id, error = %e, "failed to send queued message");
                    queue.mark_failed(msg.id)?;
                }
            }
        }

        // Expire old messages.
        queue.expire_old()?;

        Ok(())
    }

    /// Get the current offset (for persistence across restarts).
    pub fn offset(&self) -> i64 {
        self.offset
    }

    /// Set the offset (restore from checkpoint).
    pub fn set_offset(&mut self, offset: i64) {
        self.offset = offset;
    }

    /// Check if a chat ID is in the whitelist.
    pub fn is_allowed(&self, chat_id: i64) -> bool {
        self.allowed_chat_ids.contains(&chat_id)
    }
}

impl std::fmt::Debug for TelegramPoller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramPoller")
            .field("offset", &self.offset)
            .field("allowed_chats", &self.allowed_chat_ids.len())
            .field("poll_timeout", &self.poll_timeout)
            .finish()
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Parse a raw Telegram update JSON into our simplified struct.
fn parse_update(raw: &serde_json::Value) -> Option<TelegramUpdate> {
    let update_id = raw.get("update_id")?.as_i64()?;

    // Try message first, then callback_query.
    if let Some(msg) = raw.get("message") {
        let chat_id = msg.get("chat")?.get("id")?.as_i64()?;
        let from_user_id = msg.get("from").and_then(|f| f.get("id")).and_then(|id| id.as_i64());
        let text = msg.get("text").and_then(|t| t.as_str()).map(|s| s.to_string());
        let message_id = msg.get("message_id").and_then(|m| m.as_i64());

        return Some(TelegramUpdate {
            update_id,
            chat_id,
            from_user_id,
            text,
            message_id,
            callback_data: None,
        });
    }

    if let Some(cb) = raw.get("callback_query") {
        let chat_id = cb.get("message")?.get("chat")?.get("id")?.as_i64()?;
        let from_user_id = cb.get("from").and_then(|f| f.get("id")).and_then(|id| id.as_i64());
        let data = cb.get("data").and_then(|d| d.as_str()).map(|s| s.to_string());
        let message_id = cb
            .get("message")
            .and_then(|m| m.get("message_id"))
            .and_then(|m| m.as_i64());

        return Some(TelegramUpdate {
            update_id,
            chat_id,
            from_user_id,
            text: None,
            message_id,
            callback_data: data,
        });
    }

    None
}

/// Truncate a message to Telegram's 4096-character limit.
fn truncate_message(text: &str) -> &str {
    if text.len() <= MAX_MESSAGE_LENGTH {
        text
    } else {
        // Find a safe UTF-8 boundary.
        let mut end = MAX_MESSAGE_LENGTH - 20;
        while !text.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &text[..end]
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_message_update() {
        let raw = serde_json::json!({
            "update_id": 100,
            "message": {
                "message_id": 1,
                "chat": { "id": 42 },
                "from": { "id": 42 },
                "text": "/status"
            }
        });

        let update = parse_update(&raw).unwrap();
        assert_eq!(update.update_id, 100);
        assert_eq!(update.chat_id, 42);
        assert_eq!(update.text, Some("/status".to_string()));
        assert_eq!(update.message_id, Some(1));
    }

    #[test]
    fn test_parse_callback_query() {
        let raw = serde_json::json!({
            "update_id": 200,
            "callback_query": {
                "id": "abc",
                "from": { "id": 42 },
                "message": {
                    "message_id": 5,
                    "chat": { "id": 42 }
                },
                "data": "approve_123"
            }
        });

        let update = parse_update(&raw).unwrap();
        assert_eq!(update.update_id, 200);
        assert_eq!(update.chat_id, 42);
        assert_eq!(update.callback_data, Some("approve_123".to_string()));
    }

    #[test]
    fn test_parse_invalid_update() {
        let raw = serde_json::json!({ "foo": "bar" });
        assert!(parse_update(&raw).is_none());
    }

    #[test]
    fn test_truncate_message_short() {
        let text = "hello";
        assert_eq!(truncate_message(text), "hello");
    }

    #[test]
    fn test_truncate_message_long() {
        let text = "a".repeat(5000);
        let truncated = truncate_message(&text);
        assert!(truncated.len() <= MAX_MESSAGE_LENGTH);
    }

    #[test]
    fn test_poller_creation() {
        let poller = TelegramPoller::new(
            "123:ABC".to_string(),
            vec![42, 100],
            Box::new(StubHttpBackend),
        );
        assert_eq!(poller.offset(), 0);
        assert!(poller.is_allowed(42));
        assert!(!poller.is_allowed(999));
    }

    #[test]
    fn test_poller_offset_persistence() {
        let mut poller = TelegramPoller::new(
            "123:ABC".to_string(),
            vec![42],
            Box::new(StubHttpBackend),
        );
        poller.set_offset(500);
        assert_eq!(poller.offset(), 500);
    }
}
