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

use aura_types::errors::AuraError;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, info, instrument, warn};

use super::queue::{MessageContent, MessageQueue};

// ─── Telegram API types ─────────────────────────────────────────────────────

/// Type of non-text content in a Telegram message.
///
/// Used to give honest UX feedback when AURA receives media it can't process yet,
/// and to route voice messages through the voice pipeline once available.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NonTextContent {
    /// Telegram voice message (OGG/Opus). Includes `file_id` for download.
    Voice { file_id: String, duration_secs: u32 },
    /// Audio file (music, podcast, etc.). Includes `file_id` for download.
    Audio { file_id: String },
    /// Photo (one or more sizes). Includes `file_id` of the largest size.
    Photo { file_id: String },
    /// Video file. Includes `file_id`.
    Video { file_id: String },
    /// Video note (round video). Includes `file_id`.
    VideoNote { file_id: String },
    /// Document / file attachment. Includes `file_id`.
    Document { file_id: String },
    /// Sticker. Includes emoji if available.
    Sticker { emoji: Option<String> },
    /// Contact shared.
    Contact,
    /// Location shared.
    Location,
    /// Any other non-text content we don't specifically handle.
    Other,
}

impl NonTextContent {
    /// Human-readable description for UX feedback messages.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Voice { .. } => "voice message",
            Self::Audio { .. } => "audio file",
            Self::Photo { .. } => "photo",
            Self::Video { .. } => "video",
            Self::VideoNote { .. } => "video note",
            Self::Document { .. } => "file",
            Self::Sticker { .. } => "sticker",
            Self::Contact => "contact",
            Self::Location => "location",
            Self::Other => "media",
        }
    }
}

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
    /// Non-text content type, if the message contains media instead of text.
    /// `None` means pure text (or callback). `Some(...)` means the user sent media.
    pub non_text_content: Option<NonTextContent>,
    /// Pre-downloaded voice data (OGG/Opus bytes).
    /// Populated by `poll_loop` when a voice message is detected, so the
    /// handler can process it without needing access to the HTTP backend.
    #[serde(skip)]
    pub voice_data: Option<Vec<u8>>,
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
        Err(AuraError::Ipc(
            aura_types::errors::IpcError::ConnectionFailed,
        ))
    }

    async fn post_json(&self, _url: &str, _body: &[u8]) -> Result<Vec<u8>, AuraError> {
        Err(AuraError::Ipc(
            aura_types::errors::IpcError::ConnectionFailed,
        ))
    }

    async fn post_multipart(
        &self,
        _url: &str,
        _fields: Vec<(&str, String)>,
        _file_field: Option<(&str, Vec<u8>, &str)>,
    ) -> Result<Vec<u8>, AuraError> {
        Err(AuraError::Ipc(
            aura_types::errors::IpcError::ConnectionFailed,
        ))
    }
}

// ─── TelegramPoller ─────────────────────────────────────────────────────────

/// Telegram Bot API long-polling client.
///
/// Manages the poll loop, outgoing message queue flushing, and raw API calls.
// Phase 8 wire point: bot_token read by polling loop once Telegram bridge is
// activated from the JNI boot path.
#[allow(dead_code)]
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
    pub fn new(bot_token: String, allowed_chat_ids: Vec<i64>, http: Box<dyn HttpBackend>) -> Self {
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

                        // Pre-download voice data so the handler can process it
                        // without needing direct access to the HTTP backend.
                        let mut update = update;
                        if let Some(NonTextContent::Voice { ref file_id, .. }) =
                            update.non_text_content
                        {
                            match self.download_file(file_id).await {
                                Ok(bytes) => {
                                    debug!(
                                        file_id = %file_id,
                                        size_bytes = bytes.len(),
                                        "pre-downloaded voice message"
                                    );
                                    update.voice_data = Some(bytes);
                                }
                                Err(e) => {
                                    warn!(error = %e, "failed to download voice file — handler will use text fallback");
                                    // voice_data stays None — handler falls back gracefully.
                                }
                            }
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
        let resp: TelegramApiResponse<Vec<serde_json::Value>> = serde_json::from_slice(&body)
            .map_err(|_e| AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed))?;

        if !resp.ok {
            return Err(AuraError::Ipc(
                aura_types::errors::IpcError::ConnectionFailed,
            ));
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

        let body_bytes = serde_json::to_vec(&payload)
            .map_err(|_| AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed))?;

        let url = format!("{}/sendMessage", self.base_url);
        let resp_bytes = self.http.post_json(&url, &body_bytes).await?;

        let resp: TelegramApiResponse<SentMessage> = serde_json::from_slice(&resp_bytes)
            .map_err(|_| AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed))?;

        resp.result.ok_or(AuraError::Ipc(
            aura_types::errors::IpcError::ConnectionFailed,
        ))
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

        let body_bytes = serde_json::to_vec(&payload)
            .map_err(|_| AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed))?;

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

        let resp: TelegramApiResponse<SentMessage> = serde_json::from_slice(&resp_bytes)
            .map_err(|_| AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed))?;

        resp.result.ok_or(AuraError::Ipc(
            aura_types::errors::IpcError::ConnectionFailed,
        ))
    }

    /// Send an OGG/Opus voice message.
    pub async fn send_voice(
        &self,
        chat_id: i64,
        voice_data: &[u8],
        duration_secs: u32,
        caption: Option<&str>,
    ) -> Result<SentMessage, AuraError> {
        let url = format!("{}/sendVoice", self.base_url);
        let mut fields = vec![
            ("chat_id", chat_id.to_string()),
            ("duration", duration_secs.to_string()),
        ];
        if let Some(cap) = caption {
            fields.push(("caption", cap.to_string()));
        }
        let file = Some(("voice", voice_data.to_vec(), "voice.ogg"));

        let resp_bytes = self.http.post_multipart(&url, fields, file).await?;

        let resp: TelegramApiResponse<SentMessage> = serde_json::from_slice(&resp_bytes)
            .map_err(|_| AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed))?;

        resp.result.ok_or(AuraError::Ipc(
            aura_types::errors::IpcError::ConnectionFailed,
        ))
    }

    /// Download a file from Telegram by file_id.
    ///
    /// Two-step: getFile → download from file_path.
    pub async fn download_file(&self, file_id: &str) -> Result<Vec<u8>, AuraError> {
        // Step 1: getFile to get the file_path.
        let url = format!("{}/getFile", self.base_url);
        let payload = serde_json::json!({ "file_id": file_id });
        let body_bytes = serde_json::to_vec(&payload)
            .map_err(|_| AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed))?;
        let resp_bytes = self.http.post_json(&url, &body_bytes).await?;

        // Parse the file path from the response.
        let resp: serde_json::Value = serde_json::from_slice(&resp_bytes)
            .map_err(|_| AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed))?;
        let file_path = resp["result"]["file_path"].as_str().ok_or(AuraError::Ipc(
            aura_types::errors::IpcError::ConnectionFailed,
        ))?;

        // Step 2: Download the actual file content.
        // base_url is like https://api.telegram.org/bot<TOKEN>
        // file download is https://api.telegram.org/file/bot<TOKEN>/<file_path>
        let download_url = self.base_url.replace("/bot", "/file/bot");
        let download_url = format!("{}/{}", download_url, file_path);
        let file_bytes = self.http.get(&download_url).await?;

        Ok(file_bytes)
    }

    // ─── Queue flushing ─────────────────────────────────────────────────

    /// Send all pending messages from the offline queue.
    async fn flush_queue(&self, queue: &MessageQueue) -> Result<(), AuraError> {
        // Use spawn_blocking for the SQLite read.
        // In real code: let batch = tokio::task::spawn_blocking(||
        // queue.dequeue_batch(10)).await??; For now, since MessageQueue isn't Send, we call
        // directly (acceptable for single-threaded runtime).
        let batch = queue.dequeue_batch(10)?;

        for msg in &batch {
            let result = match &msg.content {
                MessageContent::Text { text, parse_mode } => self
                    .send_message(msg.chat_id, text, parse_mode.as_deref())
                    .await
                    .map(|_| ()),
                MessageContent::Photo { data, caption } => self
                    .send_photo(msg.chat_id, data, caption)
                    .await
                    .map(|_| ()),
                MessageContent::EditText {
                    message_id,
                    text,
                    parse_mode,
                } => {
                    self.edit_message(msg.chat_id, *message_id, text, parse_mode.as_deref())
                        .await
                }
                MessageContent::Voice {
                    data,
                    duration_secs,
                    caption,
                } => self
                    .send_voice(msg.chat_id, data, *duration_secs, caption.as_deref())
                    .await
                    .map(|_| ()),
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
        let from_user_id = msg
            .get("from")
            .and_then(|f| f.get("id"))
            .and_then(|id| id.as_i64());
        let text = msg
            .get("text")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());
        let message_id = msg.get("message_id").and_then(|m| m.as_i64());

        // Detect non-text content types from the Telegram message object.
        // Order matters: voice is most actionable (future voice pipeline), then
        // photo/video (common), then the rest.
        let non_text_content = if let Some(voice) = msg.get("voice") {
            Some(NonTextContent::Voice {
                file_id: voice
                    .get("file_id")
                    .and_then(|f| f.as_str())
                    .unwrap_or_default()
                    .to_string(),
                duration_secs: voice.get("duration").and_then(|d| d.as_u64()).unwrap_or(0) as u32,
            })
        } else if let Some(audio) = msg.get("audio") {
            Some(NonTextContent::Audio {
                file_id: audio
                    .get("file_id")
                    .and_then(|f| f.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
        } else if let Some(photos) = msg.get("photo") {
            // Telegram sends an array of photo sizes; take the last (largest).
            let file_id = photos
                .as_array()
                .and_then(|arr| arr.last())
                .and_then(|p| p.get("file_id"))
                .and_then(|f| f.as_str())
                .unwrap_or_default()
                .to_string();
            Some(NonTextContent::Photo { file_id })
        } else if let Some(video) = msg.get("video") {
            Some(NonTextContent::Video {
                file_id: video
                    .get("file_id")
                    .and_then(|f| f.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
        } else if let Some(vn) = msg.get("video_note") {
            Some(NonTextContent::VideoNote {
                file_id: vn
                    .get("file_id")
                    .and_then(|f| f.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
        } else if let Some(doc) = msg.get("document") {
            Some(NonTextContent::Document {
                file_id: doc
                    .get("file_id")
                    .and_then(|f| f.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
        } else if let Some(sticker) = msg.get("sticker") {
            Some(NonTextContent::Sticker {
                emoji: sticker
                    .get("emoji")
                    .and_then(|e| e.as_str())
                    .map(|s| s.to_string()),
            })
        } else if msg.get("contact").is_some() {
            Some(NonTextContent::Contact)
        } else if msg.get("location").is_some() {
            Some(NonTextContent::Location)
        } else {
            // Pure text message, or a type we don't enumerate (poll, dice, etc.)
            // If text is also None, this becomes an "Other" non-text.
            if text.is_none() {
                Some(NonTextContent::Other)
            } else {
                None
            }
        };

        return Some(TelegramUpdate {
            update_id,
            chat_id,
            from_user_id,
            text,
            message_id,
            callback_data: None,
            non_text_content,
            voice_data: None,
        });
    }

    if let Some(cb) = raw.get("callback_query") {
        let chat_id = cb.get("message")?.get("chat")?.get("id")?.as_i64()?;
        let from_user_id = cb
            .get("from")
            .and_then(|f| f.get("id"))
            .and_then(|id| id.as_i64());
        let data = cb
            .get("data")
            .and_then(|d| d.as_str())
            .map(|s| s.to_string());
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
            non_text_content: None,
            voice_data: None,
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
        assert!(
            update.non_text_content.is_none(),
            "pure text should have no non_text_content"
        );
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
        assert!(update.non_text_content.is_none());
    }

    #[test]
    fn test_parse_voice_message() {
        let raw = serde_json::json!({
            "update_id": 300,
            "message": {
                "message_id": 10,
                "chat": { "id": 42 },
                "from": { "id": 42 },
                "voice": {
                    "file_id": "AwACAgIAAxkBAAI_voice_id",
                    "duration": 5,
                    "mime_type": "audio/ogg"
                }
            }
        });

        let update = parse_update(&raw).unwrap();
        assert_eq!(update.chat_id, 42);
        assert!(update.text.is_none());
        match &update.non_text_content {
            Some(NonTextContent::Voice {
                file_id,
                duration_secs,
            }) => {
                assert_eq!(file_id, "AwACAgIAAxkBAAI_voice_id");
                assert_eq!(*duration_secs, 5);
            }
            other => panic!("expected Voice, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_photo_message() {
        let raw = serde_json::json!({
            "update_id": 301,
            "message": {
                "message_id": 11,
                "chat": { "id": 42 },
                "from": { "id": 42 },
                "photo": [
                    { "file_id": "small_id", "width": 90, "height": 90 },
                    { "file_id": "large_id", "width": 800, "height": 600 }
                ]
            }
        });

        let update = parse_update(&raw).unwrap();
        assert!(update.text.is_none());
        match &update.non_text_content {
            Some(NonTextContent::Photo { file_id }) => {
                assert_eq!(file_id, "large_id", "should pick the largest photo");
            }
            other => panic!("expected Photo, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sticker_message() {
        let raw = serde_json::json!({
            "update_id": 302,
            "message": {
                "message_id": 12,
                "chat": { "id": 42 },
                "from": { "id": 42 },
                "sticker": {
                    "file_id": "sticker_id",
                    "emoji": "😀",
                    "type": "regular"
                }
            }
        });

        let update = parse_update(&raw).unwrap();
        match &update.non_text_content {
            Some(NonTextContent::Sticker { emoji }) => {
                assert_eq!(emoji.as_deref(), Some("😀"));
            }
            other => panic!("expected Sticker, got {:?}", other),
        }
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
        let mut poller =
            TelegramPoller::new("123:ABC".to_string(), vec![42], Box::new(StubHttpBackend));
        poller.set_offset(500);
        assert_eq!(poller.offset(), 500);
    }
}
