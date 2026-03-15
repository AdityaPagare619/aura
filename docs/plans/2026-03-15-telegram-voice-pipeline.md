# Telegram Async Voice Pipeline — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable end-to-end voice messaging: Telegram voice → STT → LLM handler → TTS → voice reply + text transcript.

**Architecture:** Download OGG/Opus voice in the polling loop (HTTP already available), pass raw bytes through the update channel, decode/STT in the handler loop, process text through normal handler pipeline, TTS the response, encode to OGG/Opus, send both voice and text back. All CPU-bound work uses `spawn_blocking` to keep the async runtime responsive.

**Tech Stack:** `ogg` (OGG container, pure Rust), `audiopus` (Opus codec via libopus), existing Whisper STT (batch mode), existing Piper/eSpeak TTS, existing reqwest HTTP backend.

---

## Deliverable 1: Architecture & Data Flow

### Data Flow Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                     TELEGRAM BOT API                            │
│  getUpdates ←─── poll_loop() ──→ getFile + download ──→ OGG    │
│  sendVoice  ←─── flush_queue() ←── Voice(ogg_bytes)            │
│  sendMessage ←── flush_queue() ←── Text(transcript)             │
└───────────┬──────────────────────────────────────┬──────────────┘
            │ TelegramUpdate { voice_data }        │ MessageContent::Voice
            ▼                                      ▲
┌───────────────────────────────────────────────────────────────┐
│                     HANDLER LOOP (mod.rs)                      │
│                                                                │
│  update.voice_data.is_some()?                                  │
│       │                                                        │
│       ▼                                                        │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │          voice_pipeline::process_voice_input()           │   │
│  │  (runs in spawn_blocking for CPU-bound work)            │   │
│  │                                                          │   │
│  │  OGG bytes                                               │   │
│  │    ├── ogg::PacketReader → extract Opus packets          │   │
│  │    ├── audiopus::Decoder → PCM i16 @ 48kHz               │   │
│  │    ├── resample 48kHz → 16kHz (decimate by 3)            │   │
│  │    └── SpeechToText::transcribe_batch(pcm, 16000)        │   │
│  │         → transcription: String                          │   │
│  └──────────────────────────┬──────────────────────────────┘   │
│                              │                                  │
│       text = transcription   │                                  │
│       voice_input = true     │                                  │
│                              ▼                                  │
│  ┌──────────────────────────────────────────────────────┐      │
│  │    Normal handler dispatch (TelegramCommand::parse)   │      │
│  │    → HandlerResponse (Text/Html/Voice/Photo/Empty)    │      │
│  └──────────────────────────┬───────────────────────────┘      │
│                              │                                  │
│       response_text          │                                  │
│                              ▼                                  │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │        voice_pipeline::synthesize_voice_response()       │   │
│  │  (runs in spawn_blocking for CPU-bound work)            │   │
│  │                                                          │   │
│  │  response_text                                           │   │
│  │    ├── TextToSpeech::synthesize(text) → PCM @ 22050Hz    │   │
│  │    ├── resample 22050Hz → 48kHz (linear interpolation)   │   │
│  │    ├── audiopus::Encoder → Opus packets                  │   │
│  │    ├── ogg::PacketWriter → OGG bytes                     │   │
│  │    └── return Vec<u8> (OGG/Opus file)                    │   │
│  └──────────────────────────┬──────────────────────────────┘   │
│                              │                                  │
│       Queue: Voice { ogg_data, caption }  ──────────────────►  │
│       Queue: Text { transcript }          ──────────────────►  │
└────────────────────────────────────────────────────────────────┘
```

### Component Interaction

```
TelegramPoller                    TelegramEngine (handler loop)
    │                                     │
    ├── get_updates()                     │
    ├── parse_update()                    │
    │   └── extracts voice.file_id        │
    ├── get_file(file_id)                 │
    │   └── returns file_path             │
    ├── download_file(file_path)          │
    │   └── returns OGG bytes             │
    ├── tx.send(update with voice_data)──►│
    │                                     ├── voice_pipeline::process_voice_input()
    │                                     │   ├── decode_ogg_opus() 
    │                                     │   ├── resample_48k_to_16k()
    │                                     │   └── stt.transcribe_batch()
    │                                     │       → transcription text
    │                                     │
    │                                     ├── handlers::dispatch(transcribed_cmd)
    │                                     │   → response text
    │                                     │
    │                                     ├── voice_pipeline::synthesize_voice_response()
    │                                     │   ├── tts.synthesize()
    │                                     │   ├── resample_to_48k()
    │                                     │   └── encode_ogg_opus()
    │                                     │       → OGG bytes
    │                                     │
    │                                     ├── queue.enqueue(Voice { ogg_data })
    │                                     └── queue.enqueue(Text { transcript })
    │                                     
    ├── flush_queue()◄────────────────────
    │   ├── Voice → send_voice()
    │   └── Text  → send_message()
    │
```

### Sample Rate Flow

```
Telegram OGG/Opus (48kHz typically)
    │
    ▼ Opus decode
PCM i16 @ 48,000 Hz
    │
    ▼ Decimate by 3 (take every 3rd sample)
PCM i16 @ 16,000 Hz  ──► STT (Whisper expects 16kHz)
                              │
                              ▼ transcription text
                         LLM handler
                              │
                              ▼ response text
                         TTS (Piper @ 22,050 Hz)
                              │
                              ▼
                     PCM i16 @ 22,050 Hz
                              │
                              ▼ Linear interpolation upsample
                     PCM i16 @ 48,000 Hz
                              │
                              ▼ Opus encode + OGG wrap
                     OGG/Opus file bytes → sendVoice
```

---

## Deliverable 2: Code Skeletons

### Task 1: Extend `TelegramUpdate` — `polling.rs`

**Files:** Modify `crates/aura-daemon/src/telegram/polling.rs:25-33`

```rust
// polling.rs:25 — Add voice fields to TelegramUpdate
pub struct TelegramUpdate {
    pub update_id: i64,
    pub chat_id: i64,
    pub from_user_id: Option<i64>,
    pub text: Option<String>,
    pub message_id: Option<i64>,
    pub callback_data: Option<String>,
    // ── NEW: Voice message support ──
    /// Telegram file_id for the voice message OGG file.
    pub voice_file_id: Option<String>,
    /// Duration of the voice message in seconds.
    pub voice_duration: Option<i64>,
    /// Raw OGG/Opus bytes (populated by poll_loop after download).
    pub voice_data: Option<Vec<u8>>,
}
```

### Task 2: Extend `parse_update` — `polling.rs`

**Files:** Modify `crates/aura-daemon/src/telegram/polling.rs:393-433`

```rust
// polling.rs:393 — Extract voice fields from Telegram JSON
fn parse_update(raw: &serde_json::Value) -> Option<TelegramUpdate> {
    let update_id = raw.get("update_id")?.as_i64()?;

    if let Some(msg) = raw.get("message") {
        let chat_id = msg.get("chat")?.get("id")?.as_i64()?;
        let from_user_id = msg.get("from").and_then(|f| f.get("id")).and_then(|id| id.as_i64());
        let text = msg.get("text").and_then(|t| t.as_str()).map(|s| s.to_string());
        let message_id = msg.get("message_id").and_then(|m| m.as_i64());

        // ── NEW: Extract voice message metadata ──
        let (voice_file_id, voice_duration) = if let Some(voice) = msg.get("voice") {
            let file_id = voice.get("file_id").and_then(|f| f.as_str()).map(|s| s.to_string());
            let duration = voice.get("duration").and_then(|d| d.as_i64());
            (file_id, duration)
        } else {
            (None, None)
        };

        return Some(TelegramUpdate {
            update_id,
            chat_id,
            from_user_id,
            text,
            message_id,
            callback_data: None,
            voice_file_id,
            voice_duration,
            voice_data: None, // Populated later by poll_loop
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
            voice_file_id: None,
            voice_duration: None,
            voice_data: None,
        });
    }

    None
}
```

### Task 3: Add `get_file` + `download_file` + `send_voice` — `polling.rs`

**Files:** Modify `crates/aura-daemon/src/telegram/polling.rs` (add methods to `impl TelegramPoller`)

```rust
// polling.rs — Add to impl TelegramPoller, after send_photo()

    /// Telegram `getFile` API — returns the file_path for a given file_id.
    async fn get_file_path(&self, file_id: &str) -> Result<String, AuraError> {
        let url = format!("{}/getFile?file_id={}", self.base_url, file_id);
        let body = self.http.get(&url).await?;
        let resp: TelegramApiResponse<serde_json::Value> =
            serde_json::from_slice(&body).map_err(|_| {
                AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed)
            })?;

        if !resp.ok {
            return Err(AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed));
        }

        resp.result
            .and_then(|r| r.get("file_path")?.as_str().map(|s| s.to_string()))
            .ok_or_else(|| AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed))
    }

    /// Download a file from Telegram's file storage.
    async fn download_file(&self, file_path: &str) -> Result<Vec<u8>, AuraError> {
        let url = format!(
            "https://api.telegram.org/file/bot{}/{}",
            self.bot_token, file_path
        );
        self.http.get(&url).await
    }

    /// Download a voice message by file_id. Returns raw OGG/Opus bytes.
    pub async fn download_voice(&self, file_id: &str) -> Result<Vec<u8>, AuraError> {
        let file_path = self.get_file_path(file_id).await?;
        let data = self.download_file(&file_path).await?;

        // Sanity check: Telegram voice messages are typically < 1MB for 60s.
        // Reject anything over 2MB to prevent memory abuse.
        const MAX_VOICE_BYTES: usize = 2 * 1024 * 1024;
        if data.len() > MAX_VOICE_BYTES {
            return Err(AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed));
        }

        Ok(data)
    }

    /// Send a voice message (OGG/Opus) via Telegram `sendVoice`.
    pub async fn send_voice(
        &self,
        chat_id: i64,
        ogg_data: &[u8],
        caption: &str,
    ) -> Result<serde_json::Value, AuraError> {
        let url = format!("{}/sendVoice", self.base_url);
        let fields = vec![
            ("chat_id", chat_id.to_string()),
            ("caption", caption.to_string()),
        ];
        let file_field = Some(("voice", ogg_data.to_vec(), "audio/ogg"));

        let body = self.http.post_multipart(&url, fields, file_field).await?;
        let resp: TelegramApiResponse<serde_json::Value> =
            serde_json::from_slice(&body).map_err(|_| {
                AuraError::Ipc(aura_types::errors::IpcError::DeserializeFailed)
            })?;

        if !resp.ok {
            return Err(AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed));
        }

        Ok(resp.result.unwrap_or_default())
    }
```

### Task 4: Download voice in `poll_loop` — `polling.rs`

**Files:** Modify `crates/aura-daemon/src/telegram/polling.rs:167-185`

```rust
// polling.rs:167 — Inside poll_loop, after parse_update, download voice data
Ok(updates) => {
    for update in updates {
        if !self.allowed_chat_ids.contains(&update.chat_id) {
            warn!(chat_id = update.chat_id, "rejected unauthorized chat_id");
            continue;
        }

        if update.update_id >= self.offset {
            self.offset = update.update_id + 1;
        }

        // ── NEW: Download voice file before forwarding to handler ──
        let mut update = update;
        if let Some(ref file_id) = update.voice_file_id {
            match self.download_voice(file_id).await {
                Ok(data) => {
                    debug!(
                        size = data.len(),
                        duration = ?update.voice_duration,
                        "downloaded voice message"
                    );
                    update.voice_data = Some(data);
                }
                Err(e) => {
                    warn!(error = %e, "failed to download voice message — skipping");
                    // Don't forward this update; the voice data is essential.
                    continue;
                }
            }
        }

        if tx.send(update).await.is_err() {
            info!("update channel closed — exiting poll loop");
            return Ok(());
        }
    }
}
```

### Task 5: Add `Voice` variant to `MessageContent` — `queue.rs`

**Files:** Modify `crates/aura-daemon/src/telegram/queue.rs:20-34`

```rust
// queue.rs:20 — Add Voice variant
pub enum MessageContent {
    Text {
        text: String,
        parse_mode: Option<String>,
    },
    Photo { data: Vec<u8>, caption: String },
    EditText {
        message_id: i64,
        text: String,
        parse_mode: Option<String>,
    },
    // ── NEW ──
    /// Voice message (OGG/Opus) with text caption.
    Voice {
        ogg_data: Vec<u8>,
        caption: String,
    },
}
```

### Task 6: Handle `Voice` in `flush_queue` — `polling.rs`

**Files:** Modify `crates/aura-daemon/src/telegram/polling.rs:324-344`

```rust
// polling.rs:324 — Add Voice match arm in flush_queue
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
        // ── NEW ──
        MessageContent::Voice { ogg_data, caption } => {
            self.send_voice(msg.chat_id, ogg_data, caption)
                .await
                .map(|_| ())
        }
    };
    // ... rest unchanged
}
```

### Task 7: Create `voice_pipeline.rs` — NEW FILE

**Files:** Create `crates/aura-daemon/src/telegram/voice_pipeline.rs`

```rust
//! Telegram voice message pipeline.
//!
//! Handles the full round-trip:
//! 1. Decode OGG/Opus → PCM
//! 2. Resample → 16kHz for STT
//! 3. STT transcription
//! 4. TTS synthesis
//! 5. Resample → 48kHz for Opus
//! 6. Encode PCM → OGG/Opus

use crate::voice::stt::SpeechToText;
use crate::voice::tts::TextToSpeech;
use aura_types::errors::AuraError;
use tracing::{debug, warn};

/// Maximum voice message duration we'll process (seconds).
const MAX_VOICE_DURATION_SECS: u64 = 30;

/// Opus sample rate (48kHz is the Opus standard).
const OPUS_SAMPLE_RATE: u32 = 48_000;

/// STT sample rate (Whisper expects 16kHz).
const STT_SAMPLE_RATE: u32 = 16_000;

/// Opus frame size in samples at 48kHz (20ms frames).
const OPUS_FRAME_SIZE: usize = 960; // 48000 * 0.020

// ─── OGG/Opus Decoding ─────────────────────────────────────────

/// Decode OGG/Opus bytes into PCM i16 samples at 48kHz mono.
pub fn decode_ogg_opus(ogg_data: &[u8]) -> Result<Vec<i16>, AuraError> {
    use std::io::Cursor;

    let mut reader = ogg::PacketReader::new(Cursor::new(ogg_data));
    let decoder = audiopus::coder::Decoder::new(
        audiopus::SampleRate::Hz48000,
        audiopus::Channels::Mono,
    )
    .map_err(|e| {
        AuraError::Internal(format!("Opus decoder init failed: {e}"))
    })?;

    let mut pcm_out = Vec::new();
    let mut decode_buf = vec![0i16; OPUS_FRAME_SIZE * 2]; // generous buffer

    // Skip the first two OGG packets (OpusHead + OpusTags headers).
    let mut header_packets = 0u32;

    while let Some(packet) = reader
        .read_packet()
        .map_err(|e| AuraError::Internal(format!("OGG read error: {e}")))?
    {
        // Skip Opus header packets (first 2 packets in an Opus stream).
        if header_packets < 2 {
            header_packets += 1;
            continue;
        }

        let decoded = decoder
            .decode(
                Some(&packet.data),
                &mut decode_buf,
                false, // no FEC
            )
            .map_err(|e| AuraError::Internal(format!("Opus decode error: {e}")))?;

        pcm_out.extend_from_slice(&decode_buf[..decoded]);
    }

    debug!(samples = pcm_out.len(), "decoded OGG/Opus to PCM");
    Ok(pcm_out)
}

// ─── OGG/Opus Encoding ─────────────────────────────────────────

/// Encode PCM i16 samples at 48kHz mono into OGG/Opus bytes.
pub fn encode_ogg_opus(pcm_48k: &[i16]) -> Result<Vec<u8>, AuraError> {
    use std::io::Cursor;

    let encoder = audiopus::coder::Encoder::new(
        audiopus::SampleRate::Hz48000,
        audiopus::Channels::Mono,
        audiopus::Application::Voip,
    )
    .map_err(|e| {
        AuraError::Internal(format!("Opus encoder init failed: {e}"))
    })?;

    let mut ogg_buf = Vec::new();
    let mut writer = ogg::PacketWriter::new(Cursor::new(&mut ogg_buf));
    let serial = 1u32; // arbitrary stream serial number

    // Write OpusHead header.
    let opus_head = build_opus_head();
    writer
        .write_packet(opus_head, serial, ogg::PacketWriteEndInfo::EndPage, 0)
        .map_err(|e| AuraError::Internal(format!("OGG write OpusHead: {e}")))?;

    // Write OpusTags header.
    let opus_tags = build_opus_tags();
    writer
        .write_packet(opus_tags, serial, ogg::PacketWriteEndInfo::EndPage, 0)
        .map_err(|e| AuraError::Internal(format!("OGG write OpusTags: {e}")))?;

    // Encode audio data in 20ms frames.
    let mut encode_buf = vec![0u8; 4000]; // max Opus packet size
    let mut granule_pos: u64 = 0;

    for chunk in pcm_48k.chunks(OPUS_FRAME_SIZE) {
        // Pad last chunk with silence if needed.
        let frame: Vec<i16>;
        let input = if chunk.len() < OPUS_FRAME_SIZE {
            frame = {
                let mut f = chunk.to_vec();
                f.resize(OPUS_FRAME_SIZE, 0);
                f
            };
            &frame
        } else {
            chunk
        };

        let encoded_len = encoder
            .encode(input, &mut encode_buf)
            .map_err(|e| AuraError::Internal(format!("Opus encode error: {e}")))?;

        granule_pos += OPUS_FRAME_SIZE as u64;

        let is_last = chunk.len() < OPUS_FRAME_SIZE;
        let end_info = if is_last {
            ogg::PacketWriteEndInfo::EndStream
        } else {
            ogg::PacketWriteEndInfo::NormalPacket
        };

        writer
            .write_packet(
                encode_buf[..encoded_len].to_vec(),
                serial,
                end_info,
                granule_pos,
            )
            .map_err(|e| AuraError::Internal(format!("OGG write packet: {e}")))?;
    }

    // If pcm was empty, write an empty end-of-stream packet.
    if pcm_48k.is_empty() {
        writer
            .write_packet(
                vec![],
                serial,
                ogg::PacketWriteEndInfo::EndStream,
                0,
            )
            .map_err(|e| AuraError::Internal(format!("OGG write EOS: {e}")))?;
    }

    drop(writer);
    debug!(bytes = ogg_buf.len(), "encoded PCM to OGG/Opus");
    Ok(ogg_buf)
}

/// Build the OpusHead header (RFC 7845 §5.1).
fn build_opus_head() -> Vec<u8> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");  // magic signature
    head.push(1);                          // version
    head.push(1);                          // channel count (mono)
    head.extend_from_slice(&0u16.to_le_bytes()); // pre-skip
    head.extend_from_slice(&48000u32.to_le_bytes()); // input sample rate
    head.extend_from_slice(&0i16.to_le_bytes());  // output gain
    head.push(0);                          // channel mapping family
    head
}

/// Build the OpusTags header (RFC 7845 §5.2).
fn build_opus_tags() -> Vec<u8> {
    let mut tags = Vec::with_capacity(24);
    tags.extend_from_slice(b"OpusTags");     // magic signature
    let vendor = b"AURA";
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes()); // no user comments
    tags
}

// ─── Resampling ─────────────────────────────────────────────────

/// Decimate from 48kHz to 16kHz (factor of 3).
/// Simple decimation — acceptable for voice because Opus already
/// band-limits the signal.
pub fn resample_48k_to_16k(pcm_48k: &[i16]) -> Vec<i16> {
    pcm_48k.iter().step_by(3).copied().collect()
}

/// Upsample from source_rate to 48kHz using linear interpolation.
/// Used for TTS output (22050 Hz) → Opus input (48000 Hz).
pub fn resample_to_48k(pcm: &[i16], source_rate: u32) -> Vec<i16> {
    if source_rate == OPUS_SAMPLE_RATE {
        return pcm.to_vec();
    }

    let ratio = OPUS_SAMPLE_RATE as f64 / source_rate as f64;
    let out_len = (pcm.len() as f64 * ratio).ceil() as usize;
    let mut output = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_pos = i as f64 / ratio;
        let idx = src_pos as usize;
        let frac = src_pos - idx as f64;

        if idx + 1 < pcm.len() {
            let sample = pcm[idx] as f64 * (1.0 - frac) + pcm[idx + 1] as f64 * frac;
            output.push(sample.round() as i16);
        } else if idx < pcm.len() {
            output.push(pcm[idx]);
        }
    }

    output
}

// ─── Pipeline Orchestration ─────────────────────────────────────

/// Result of processing an incoming voice message.
pub struct VoiceInputResult {
    /// The transcribed text from the voice message.
    pub transcription: String,
    /// Duration of the voice message in seconds (from metadata).
    pub duration_secs: u64,
}

/// Process an incoming voice message: decode OGG → resample → STT.
///
/// This is CPU-bound and should be called inside `spawn_blocking`.
pub fn process_voice_input(
    ogg_data: &[u8],
    stt: &SpeechToText,
) -> Result<VoiceInputResult, AuraError> {
    // 1. Decode OGG/Opus → PCM @ 48kHz
    let pcm_48k = decode_ogg_opus(ogg_data)?;

    if pcm_48k.is_empty() {
        return Err(AuraError::Internal("Voice message is empty".into()));
    }

    let duration_secs = pcm_48k.len() as u64 / OPUS_SAMPLE_RATE as u64;

    if duration_secs > MAX_VOICE_DURATION_SECS {
        return Err(AuraError::Internal(format!(
            "Voice message too long: {duration_secs}s (max {MAX_VOICE_DURATION_SECS}s)"
        )));
    }

    // 2. Resample 48kHz → 16kHz for STT
    let pcm_16k = resample_48k_to_16k(&pcm_48k);
    debug!(
        samples_48k = pcm_48k.len(),
        samples_16k = pcm_16k.len(),
        "resampled for STT"
    );

    // 3. STT transcription (Whisper batch mode)
    let transcription = stt.transcribe_batch(&pcm_16k, STT_SAMPLE_RATE)?;
    debug!(text_len = transcription.len(), "STT transcription complete");

    Ok(VoiceInputResult {
        transcription,
        duration_secs,
    })
}

/// Synthesize a text response into OGG/Opus voice data.
///
/// This is CPU-bound and should be called inside `spawn_blocking`.
pub fn synthesize_voice_response(
    text: &str,
    tts: &TextToSpeech,
) -> Result<Vec<u8>, AuraError> {
    // 1. TTS → PCM (Piper @ 22050 Hz or eSpeak @ 22050 Hz)
    let audio = tts.synthesize(text)?;
    debug!(
        samples = audio.samples.len(),
        sample_rate = audio.sample_rate,
        engine = ?audio.engine,
        "TTS synthesis complete"
    );

    if audio.samples.is_empty() {
        return Err(AuraError::Internal("TTS produced no audio".into()));
    }

    // 2. Resample to 48kHz for Opus
    let pcm_48k = resample_to_48k(&audio.samples, audio.sample_rate);
    debug!(
        samples_out = pcm_48k.len(),
        "resampled for Opus encoding"
    );

    // 3. Encode to OGG/Opus
    encode_ogg_opus(&pcm_48k)
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resample_48k_to_16k() {
        let input: Vec<i16> = (0..48000).map(|i| (i % 100) as i16).collect();
        let output = resample_48k_to_16k(&input);
        assert_eq!(output.len(), 16000);
        // First sample preserved.
        assert_eq!(output[0], input[0]);
        // Every 3rd sample.
        assert_eq!(output[1], input[3]);
        assert_eq!(output[2], input[6]);
    }

    #[test]
    fn test_resample_to_48k_identity() {
        let input: Vec<i16> = vec![100, 200, 300];
        let output = resample_to_48k(&input, 48000);
        assert_eq!(output, input);
    }

    #[test]
    fn test_resample_to_48k_upsample() {
        // 16kHz → 48kHz = 3x upsample
        let input: Vec<i16> = vec![0, 300, 600];
        let output = resample_to_48k(&input, 16000);
        assert_eq!(output.len(), 9); // 3 * 3
        assert_eq!(output[0], 0);
        // Linear interpolation between 0 and 300
        assert!(output[1] > 0 && output[1] < 300);
    }

    #[test]
    fn test_opus_head_format() {
        let head = build_opus_head();
        assert_eq!(&head[..8], b"OpusHead");
        assert_eq!(head[8], 1); // version
        assert_eq!(head[9], 1); // mono
    }

    #[test]
    fn test_opus_tags_format() {
        let tags = build_opus_tags();
        assert_eq!(&tags[..8], b"OpusTags");
    }

    #[test]
    fn test_decode_empty_returns_error() {
        let result = decode_ogg_opus(&[]);
        assert!(result.is_err());
    }
}
```

### Task 8: Wire voice processing into handler loop — `mod.rs`

**Files:** Modify `crates/aura-daemon/src/telegram/mod.rs:242-247`

```rust
// mod.rs:242 — Replace the simple text extraction with voice-aware logic

                // ── Inline handle_update logic ──────────────────────────
                let chat_id = update.chat_id;
                let mut voice_input = false;

                let text = if let Some(t) = update.text {
                    // Normal text message.
                    t
                } else if let Some(ref voice_data) = update.voice_data {
                    // Voice message: decode + STT.
                    voice_input = true;

                    // Send "processing" indicator.
                    let _ = queue.enqueue(
                        chat_id,
                        &MessageContent::Text {
                            text: "🎤 Processing voice message...".into(),
                            parse_mode: None,
                        },
                        0, 60, 1, None,
                    );

                    // CPU-bound: decode OGG + STT (run in blocking context).
                    // Note: In the single-threaded handler loop, we do this inline.
                    // For true async, this would be spawn_blocking, but the handler
                    // loop is already sequential per-update.
                    match voice_pipeline::process_voice_input(voice_data, &stt) {
                        Ok(result) => {
                            debug!(
                                transcription_len = result.transcription.len(),
                                duration = result.duration_secs,
                                "voice STT complete"
                            );
                            result.transcription
                        }
                        Err(e) => {
                            warn!(error = %e, "voice processing failed");
                            let _ = queue.enqueue(
                                chat_id,
                                &MessageContent::Text {
                                    text: format!("Voice processing failed: {e}"),
                                    parse_mode: None,
                                },
                                0, 3600, 3, None,
                            );
                            continue;
                        }
                    }
                } else {
                    continue; // Ignore messages with neither text nor voice.
                };

                // ... rest of handler logic proceeds with `text` ...
                // After response is generated and enqueued:
```

### Task 9: Add voice response synthesis after handler response — `mod.rs`

**Files:** Modify `crates/aura-daemon/src/telegram/mod.rs:346-398` (inside the `Ok(response)` arm)

```rust
// mod.rs — After the existing response enqueue logic, add voice synthesis
// This goes inside the Ok(response) => { ... } block, after the match on HandlerResponse

                            // ── NEW: Voice response synthesis ──
                            // If input was voice, also synthesize and send audio response.
                            if voice_input {
                                // Extract the response text for TTS.
                                let response_text = match &response {
                                    HandlerResponse::Text(t) => Some(t.clone()),
                                    HandlerResponse::Html(t) => {
                                        // Strip HTML tags for TTS.
                                        Some(strip_html_tags(t))
                                    }
                                    HandlerResponse::Voice { text } => Some(text.clone()),
                                    _ => None,
                                };

                                if let Some(text_for_tts) = response_text {
                                    match voice_pipeline::synthesize_voice_response(
                                        &text_for_tts, &tts,
                                    ) {
                                        Ok(ogg_data) => {
                                            let _ = queue.enqueue(
                                                chat_id,
                                                &MessageContent::Voice {
                                                    ogg_data,
                                                    caption: String::new(),
                                                },
                                                0, 3600, 3, None,
                                            );
                                        }
                                        Err(e) => {
                                            warn!(error = %e, "voice synthesis failed");
                                            // Text response already sent, so this is non-fatal.
                                        }
                                    }
                                }
                            }
```

### Task 10: Add STT/TTS to TelegramEngine — `mod.rs`

**Files:** Modify `crates/aura-daemon/src/telegram/mod.rs` (struct fields + run method)

The handler_future needs access to `stt` and `tts`. These need to be fields on `TelegramEngine` or created in `run()`:

```rust
// mod.rs — Add to TelegramEngine struct
pub struct TelegramEngine {
    // ... existing fields ...
    /// STT engine for voice message transcription (batch mode).
    stt: Option<SpeechToText>,
    /// TTS engine for voice response synthesis.
    tts: Option<TextToSpeech>,
}

// In run(), destructure the new fields:
let Self {
    poller,
    security,
    audit,
    queue,
    policy_gate,
    dialogue_mgr,
    startup_time_ms,
    cancel_flag,
    primary_chat_id: _,
    aura_config,
    user_command_tx,
    stt,  // NEW
    tts,  // NEW
} = self;

// In the handler_future, stt and tts are borrowed from the destructured fields.
```

### Task 11: Register module — `telegram/mod.rs`

**Files:** Modify `crates/aura-daemon/src/telegram/mod.rs` (top of file, module declarations)

```rust
// mod.rs — Add module declaration alongside existing ones
pub mod voice_pipeline;
```

### Task 12: Utility function `strip_html_tags` — `voice_pipeline.rs` or `mod.rs`

```rust
/// Simple HTML tag stripper for TTS.
/// Removes <b>, </b>, <i>, <code>, etc. — not a full parser.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result
}
```

---

## Deliverable 3: Cargo.toml Additions

**File:** `crates/aura-daemon/Cargo.toml`

```toml
# Add after the existing reqwest line (line 23):
ogg = "0.9"
audiopus = "0.3"
```

**Notes:**
- `ogg` is pure Rust (no native deps). Provides `PacketReader` and `PacketWriter`.
- `audiopus` wraps `libopus` via `audiopus-sys` which builds from C source. This is the same pattern used by Whisper FFI, Piper FFI, etc.
- On Android (`target.'cfg(target_os = "android")'`), libopus will need to be cross-compiled. The existing build infrastructure for whisper/piper/espeak should serve as a template. Alternatively, provide a pre-built `libopus.so` in the Android JNI libs directory.
- **No `rubato` or `symphonia` needed** — resampling is done with simple arithmetic, OGG/Opus handled by `ogg` + `audiopus`.

**Workspace Cargo.toml** — check if `ogg` and `audiopus` need workspace-level entries. If the project uses workspace dependencies for all crates, add there too. Currently `reqwest` is NOT workspace-level (specified with version directly), so these can follow the same pattern.

---

## Deliverable 4: Error Handling Matrix

| Step | Error | Severity | Recovery | User Message |
|------|-------|----------|----------|-------------|
| **getFile API** | Network timeout | Transient | Retry 1x, then skip update | "Could not download voice message. Please try again." |
| **getFile API** | Invalid file_id | Permanent | Skip update | (silent — corrupted update) |
| **download_file** | Network error | Transient | Retry 1x, then skip | "Voice download failed. Please resend." |
| **download_file** | File > 2MB | Permanent | Skip | "Voice message too large." |
| **OGG parse** | Invalid OGG container | Permanent | Skip | "Could not process voice format." |
| **Opus decode** | Decoder init failure | Fatal-ish | Skip (log error) | "Voice codec error." |
| **Opus decode** | Corrupted packets | Permanent | Skip | "Voice message corrupted." |
| **Resample 48→16k** | Empty input | Permanent | Skip | (caught by empty check above) |
| **STT transcribe** | Whisper not initialized | Config | Skip voice, forward as-is | "Voice recognition not available." |
| **STT transcribe** | Audio too long (>30s) | Permanent | Truncate to 30s | "Voice message too long (max 30s)." |
| **STT transcribe** | Empty transcription | Soft | Send generic reply | "I couldn't understand that. Please try again." |
| **STT transcribe** | Whisper internal error | Transient | Skip | "Transcription failed." |
| **Handler dispatch** | Any handler error | Varies | Existing error handling | Existing error messages |
| **TTS synthesize** | Piper fails | Transient | Fallback to eSpeak (built-in) | (transparent fallback) |
| **TTS synthesize** | Both engines fail | Config | Send text only (no voice) | (text response still sent) |
| **TTS synthesize** | Text too long for TTS | Permanent | Truncate to 4096 chars | (synthesize truncated version) |
| **Resample 22→48k** | Empty input | Bug | Skip voice response | (text response still sent) |
| **Opus encode** | Encoder init failure | Fatal-ish | Skip voice response | (text response still sent) |
| **OGG write** | IO error (memory) | Fatal-ish | Skip voice response | (text response still sent) |
| **sendVoice API** | Network error | Transient | Queue retry (existing mechanism) | (retry from queue) |
| **sendVoice API** | File too large | Permanent | Send text only | "Voice response too large." |
| **Queue full** | SQLite error | Transient | Log + continue | (message dropped, logged) |

**Error philosophy:**
- **Voice input errors** → skip voice processing, notify user in text
- **Voice output errors** → non-fatal, text response was already queued
- **Never crash the handler loop** — all voice errors are caught and logged
- **Graceful degradation** — if voice pipeline fails at ANY point, the text channel still works

---

## Deliverable 5: Integration Points

### 5.1 Files Modified

| File | Lines Changed | Nature |
|------|--------------|--------|
| `telegram/polling.rs:25-33` | ~8 | Add voice fields to `TelegramUpdate` struct |
| `telegram/polling.rs:167-185` | ~20 | Download voice in poll_loop |
| `telegram/polling.rs:324-344` | ~5 | Add `Voice` match arm in flush_queue |
| `telegram/polling.rs:393-433` | ~15 | Extract voice fields in parse_update |
| `telegram/polling.rs` (new methods) | ~60 | `get_file_path`, `download_file`, `download_voice`, `send_voice` |
| `telegram/queue.rs:20-34` | ~5 | Add `Voice` variant to `MessageContent` |
| `telegram/mod.rs:1-10` | ~1 | Add `pub mod voice_pipeline;` |
| `telegram/mod.rs:242-247` | ~40 | Voice-aware text extraction in handler loop |
| `telegram/mod.rs:346-398` | ~25 | Voice synthesis after handler response |
| `telegram/mod.rs` (struct) | ~10 | Add `stt`, `tts` fields to TelegramEngine |
| `Cargo.toml` | ~2 | Add `ogg` + `audiopus` dependencies |

### 5.2 Files Created

| File | Lines | Purpose |
|------|-------|---------|
| `telegram/voice_pipeline.rs` | ~320 | Complete voice processing pipeline |

### 5.3 Existing Systems Touched

| System | How | Risk |
|--------|-----|------|
| `TelegramPoller` | New methods (additive) | Low — no existing methods modified |
| `TelegramUpdate` | New fields (additive) | Low — all new fields are `Option`, existing code unaffected |
| `MessageContent` | New variant | **Medium** — any `match` on `MessageContent` must add an arm. Check for exhaustive matches in: `flush_queue`, queue serialization, any display/debug impls |
| `TelegramEngine` | New fields + modified run() | Medium — `run()` is modified but existing flow untouched for text messages |
| `parse_update` | Extended | Low — new fields extracted from different JSON paths |
| `SpeechToText` | Read-only usage | None — using existing `transcribe_batch()` API |
| `TextToSpeech` | Read-only usage | None — using existing `synthesize()` API |
| Queue serialization | Must support `Voice` variant | **Medium** — if `MessageContent` is serialized to SQLite (check `bincode`/`serde` usage in queue.rs) |

### 5.4 Integration Test Points

1. **Unit test:** `parse_update` with voice message JSON → extracts file_id
2. **Unit test:** `decode_ogg_opus` with known good OGG file → correct PCM length
3. **Unit test:** `encode_ogg_opus` roundtrip → decode(encode(pcm)) ≈ pcm
4. **Unit test:** `resample_48k_to_16k` → correct length and values
5. **Unit test:** `resample_to_48k` → correct length and interpolation
6. **Integration test:** Full pipeline with mock HTTP → voice update processed to text
7. **Integration test:** Full synthesis → valid OGG output

### 5.5 Telegram API Endpoints Used

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `getUpdates` | GET (existing) | Now also returns voice messages |
| `getFile` | GET (new) | Get file_path for download |
| `https://api.telegram.org/file/bot<token>/<path>` | GET (new) | Download voice file |
| `sendVoice` | POST multipart (new) | Upload OGG voice response |

---

## Deliverable 6: Estimated Complexity per Component

| Component | Lines of Code | Complexity | Effort | Risk | Notes |
|-----------|--------------|------------|--------|------|-------|
| **TelegramUpdate extension** | ~15 | Trivial | 5 min | Low | Add 3 optional fields |
| **parse_update extension** | ~15 | Low | 10 min | Low | Extract from JSON, well-understood |
| **get_file_path/download_file** | ~30 | Low | 15 min | Low | Standard HTTP calls, follows send_photo pattern |
| **send_voice** | ~25 | Low | 10 min | Low | Follows send_photo pattern exactly |
| **download_voice** | ~15 | Low | 5 min | Low | Compose get_file_path + download_file |
| **poll_loop modification** | ~20 | Low | 10 min | Low | Add if-let block, follows existing pattern |
| **MessageContent::Voice** | ~5 | Trivial | 5 min | Medium | Must update ALL match sites |
| **flush_queue Voice arm** | ~5 | Trivial | 5 min | Low | One match arm |
| **decode_ogg_opus** | ~45 | **High** | 1-2 hrs | **High** | OGG packet iteration + Opus decode. Main risk: getting header skip right, handling variable frame sizes |
| **encode_ogg_opus** | ~65 | **High** | 1-2 hrs | **High** | Building correct OpusHead/OpusTags, frame chunking, granule position tracking |
| **resample_48k_to_16k** | ~3 | Trivial | 5 min | Low | Step_by(3) |
| **resample_to_48k** | ~20 | Medium | 20 min | Low | Linear interpolation, edge cases |
| **process_voice_input** | ~30 | Medium | 20 min | Medium | Orchestration, error handling |
| **synthesize_voice_response** | ~25 | Medium | 15 min | Medium | Orchestration, error handling |
| **Handler loop voice wiring** | ~50 | Medium | 30 min | Medium | Integration with existing control flow |
| **Voice response synthesis wiring** | ~30 | Medium | 20 min | Medium | Post-handler integration |
| **STT/TTS fields on Engine** | ~15 | Low | 10 min | Low | Struct modification + destructuring |
| **strip_html_tags** | ~10 | Low | 5 min | Low | Simple char iterator |
| **Tests** | ~80 | Medium | 30 min | Low | Unit tests for codec + resampling |
| **Cargo.toml** | ~2 | Trivial | 2 min | Low | Two lines |

### Summary

| Category | Total Lines | Total Effort | Primary Risk |
|----------|------------|--------------|-------------|
| Polling/API layer | ~110 | ~45 min | Low |
| Voice codec (OGG/Opus) | ~160 | **2-4 hrs** | **High** — codec correctness |
| Resampling | ~25 | ~25 min | Low |
| Pipeline orchestration | ~55 | ~35 min | Medium |
| Handler integration | ~95 | ~1 hr | Medium — touches control flow |
| Tests | ~80 | ~30 min | Low |
| **Total** | **~525** | **~5-7 hrs** | **Codec is the risk** |

### Critical Path

```
1. Cargo.toml (2 min)
   └── 2. voice_pipeline.rs codec functions (2-4 hrs) ← RISK
       └── 3. TelegramUpdate + parse_update (15 min)
           └── 4. download/send methods (25 min)
               └── 5. poll_loop download (10 min)
                   └── 6. MessageContent::Voice + flush_queue (10 min)
                       └── 7. Handler loop integration (1 hr)
                           └── 8. Tests + verification (30 min)
```

### Risk Mitigation

1. **Codec risk:** Build and test `decode_ogg_opus` and `encode_ogg_opus` in isolation first. Create a test with a real OGG file from Telegram (can be committed as a test fixture).
2. **audiopus on Android:** Verify that `audiopus-sys` cross-compiles for `aarch64-linux-android`. If not, may need to provide pre-built libopus. Fallback: use the Android MediaCodec API via JNI (like existing pattern).
3. **Memory:** A 30-second voice message at 48kHz mono i16 = 2.88MB of PCM. After resampling to 16kHz = 960KB. TTS output at 22050Hz for ~30s response = ~1.3MB. Total peak ~5MB. Acceptable for phone.
4. **Queue serialization:** Verify that `MessageContent::Voice` with `Vec<u8>` serializes correctly through the existing queue persistence layer. The `Photo` variant already stores `Vec<u8>`, so this should work.

---

## Implementation Order (Recommended)

### Phase 1: Foundation (Tasks 1-6) — ~1 hour
1. Cargo.toml additions
2. TelegramUpdate fields
3. parse_update extension
4. MessageContent::Voice variant + flush_queue arm
5. API methods (get_file_path, download_file, download_voice, send_voice)
6. poll_loop download integration

### Phase 2: Voice Pipeline (Tasks 7) — ~3 hours
7. voice_pipeline.rs — all codec + resampling + orchestration functions
8. Unit tests for codec and resampling

### Phase 3: Integration (Tasks 8-12) — ~1.5 hours
9. STT/TTS fields on TelegramEngine
10. Handler loop voice-aware text extraction
11. Voice response synthesis wiring
12. Module registration + utility functions

### Phase 4: Verification — ~30 min
13. End-to-end test with mock data
14. Verify all existing tests still pass
15. Manual test with real Telegram bot (if possible)
