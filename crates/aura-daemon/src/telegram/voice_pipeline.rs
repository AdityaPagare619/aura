//! Telegram voice message processing pipeline.
//!
//! Handles the full voice round-trip:
//! 1. **Decode:** OGG/Opus → PCM i16 @ 48kHz
//! 2. **Resample:** 48kHz → 16kHz (for STT models)
//! 3. **STT:** PCM → transcribed text (currently stubbed — wired when FFI ready)
//! 4. **TTS:** response text → PCM (currently stubbed — wired when FFI ready)
//! 5. **Resample:** TTS sample rate → 48kHz (Opus standard)
//! 6. **Encode:** PCM → OGG/Opus bytes
//!
//! All CPU-bound work runs in `spawn_blocking` to keep the async runtime free.
//!
//! Architecture note: the pipeline extracts sensory data (Rust body) and feeds
//! it to the LLM (brain) for reasoning. No intent classification happens here.

use tracing::{debug, warn};

// ─── Constants ──────────────────────────────────────────────────────────────

/// Opus operates at 48kHz natively.
const OPUS_SAMPLE_RATE: u32 = 48_000;

/// Most STT models expect 16kHz mono.
const STT_SAMPLE_RATE: u32 = 16_000;

/// Opus frame duration in ms (20ms is standard).
const OPUS_FRAME_MS: u32 = 20;

/// Max Opus frame size in bytes (for 48kHz, 20ms stereo — generous).
const OPUS_MAX_FRAME_BYTES: usize = 4000;

/// Opus samples per frame at 48kHz, 20ms = 960 samples.
const OPUS_FRAME_SAMPLES: usize = (OPUS_SAMPLE_RATE * OPUS_FRAME_MS / 1000) as usize;

// ─── Error type ─────────────────────────────────────────────────────────────

/// Errors during voice pipeline processing.
#[derive(Debug, thiserror::Error)]
pub enum VoicePipelineError {
    #[error("OGG decode failed: {0}")]
    OggDecode(String),

    #[error("Opus decode failed: {0}")]
    OpusDecode(String),

    #[error("Opus encode failed: {0}")]
    OpusEncode(String),

    #[error("OGG encode failed: {0}")]
    OggEncode(String),

    #[error("Audio too short (need ≥ {min_ms}ms, got {actual_ms}ms)")]
    TooShort { min_ms: u32, actual_ms: u32 },

    #[error("Audio too long (max {max_secs}s, got {actual_secs}s)")]
    TooLong { max_secs: u32, actual_secs: u32 },

    #[error("STT not yet available (FFI stubs pending)")]
    SttUnavailable,

    #[error("TTS not yet available (FFI stubs pending)")]
    TtsUnavailable,

    #[error("Empty transcription")]
    EmptyTranscription,
}

// ─── OGG/Opus Decoding ─────────────────────────────────────────────────────

/// Decode an OGG/Opus byte stream into PCM i16 samples at 48kHz mono.
///
/// This is CPU-bound — call from `spawn_blocking`.
pub fn decode_ogg_opus(ogg_bytes: &[u8]) -> Result<Vec<i16>, VoicePipelineError> {
    use std::io::Cursor;

    let cursor = Cursor::new(ogg_bytes);
    let mut packet_reader = ogg::PacketReader::new(cursor);

    // Create Opus decoder: 48kHz, mono.
    let decoder =
        audiopus::coder::Decoder::new(audiopus::SampleRate::Hz48000, audiopus::Channels::Mono)
            .map_err(|e| VoicePipelineError::OpusDecode(format!("init: {e}")))?;

    let mut pcm_out = Vec::new();
    let mut decode_buf = vec![0i16; OPUS_FRAME_SAMPLES * 2]; // generous buffer
    let mut packet_count: u32 = 0;

    loop {
        match packet_reader.read_packet() {
            Ok(Some(packet)) => {
                packet_count += 1;
                // Skip the first two OGG packets (Opus header + comment).
                if packet_count <= 2 {
                    continue;
                }

                let decoded_samples = decoder
                    .decode(
                        Some(&packet.data),
                        &mut decode_buf,
                        false, // no FEC
                    )
                    .map_err(|e| VoicePipelineError::OpusDecode(format!("frame: {e}")))?;

                pcm_out.extend_from_slice(&decode_buf[..decoded_samples]);
            },
            Ok(None) => break, // End of stream.
            Err(e) => {
                // Some OGG streams have minor issues — log and continue.
                warn!(error = %e, "OGG packet read error (continuing)");
                break;
            },
        }
    }

    if pcm_out.is_empty() {
        return Err(VoicePipelineError::OggDecode(
            "no audio data decoded".into(),
        ));
    }

    let duration_ms = (pcm_out.len() as u64 * 1000) / OPUS_SAMPLE_RATE as u64;
    debug!(
        samples = pcm_out.len(),
        duration_ms = duration_ms,
        packets = packet_count,
        "OGG/Opus decoded"
    );

    Ok(pcm_out)
}

// ─── Resampling ─────────────────────────────────────────────────────────────

/// Resample PCM from 48kHz → 16kHz by simple decimation (factor 3).
///
/// For voice this is sufficient — no anti-aliasing filter needed because
/// the Opus decoder already band-limits the signal.
pub fn resample_48k_to_16k(pcm_48k: &[i16]) -> Vec<i16> {
    // 48000 / 16000 = 3
    pcm_48k.iter().step_by(3).copied().collect()
}

/// Resample PCM from a source rate to 48kHz using linear interpolation.
///
/// Handles common TTS output rates (22050Hz, 24000Hz, 16000Hz).
pub fn resample_to_48k(pcm: &[i16], source_rate: u32) -> Vec<i16> {
    if source_rate == OPUS_SAMPLE_RATE {
        return pcm.to_vec();
    }

    let ratio = OPUS_SAMPLE_RATE as f64 / source_rate as f64;
    let output_len = (pcm.len() as f64 * ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 / ratio;
        let src_idx = src_pos as usize;
        let frac = src_pos - src_idx as f64;

        let sample = if src_idx + 1 < pcm.len() {
            let a = pcm[src_idx] as f64;
            let b = pcm[src_idx + 1] as f64;
            (a + (b - a) * frac) as i16
        } else if src_idx < pcm.len() {
            pcm[src_idx]
        } else {
            0
        };
        output.push(sample);
    }

    output
}

// ─── OGG/Opus Encoding ─────────────────────────────────────────────────────

/// Encode PCM i16 samples at 48kHz mono into OGG/Opus bytes.
///
/// This is CPU-bound — call from `spawn_blocking`.
pub fn encode_ogg_opus(pcm_48k: &[i16]) -> Result<Vec<u8>, VoicePipelineError> {
    use std::io::Cursor;

    let encoder = audiopus::coder::Encoder::new(
        audiopus::SampleRate::Hz48000,
        audiopus::Channels::Mono,
        audiopus::Application::Voip,
    )
    .map_err(|e| VoicePipelineError::OpusEncode(format!("init: {e}")))?;

    let mut ogg_buf = Vec::new();
    let cursor = Cursor::new(&mut ogg_buf);
    let serial = 1u32; // Single-stream OGG.
    let mut packet_writer = ogg::PacketWriter::new(cursor);

    // Write Opus header packet (OpusHead).
    let opus_head = build_opus_head();
    packet_writer
        .write_packet(
            opus_head.into(),
            serial,
            ogg::writing::PacketWriteEndInfo::EndPage,
            0, // granule position
        )
        .map_err(|e| VoicePipelineError::OggEncode(format!("header: {e}")))?;

    // Write Opus comment packet (OpusTags).
    let opus_tags = build_opus_tags();
    packet_writer
        .write_packet(
            opus_tags.into(),
            serial,
            ogg::writing::PacketWriteEndInfo::EndPage,
            0,
        )
        .map_err(|e| VoicePipelineError::OggEncode(format!("tags: {e}")))?;

    // Encode audio data in 20ms frames.
    let mut encode_buf = vec![0u8; OPUS_MAX_FRAME_BYTES];
    let mut granule_pos: u64 = 0;
    let total_frames = pcm_48k.len() / OPUS_FRAME_SAMPLES;

    for frame_idx in 0..total_frames {
        let start = frame_idx * OPUS_FRAME_SAMPLES;
        let frame = &pcm_48k[start..start + OPUS_FRAME_SAMPLES];

        let encoded_len = encoder
            .encode(frame, &mut encode_buf)
            .map_err(|e| VoicePipelineError::OpusEncode(format!("frame {frame_idx}: {e}")))?;

        granule_pos += OPUS_FRAME_SAMPLES as u64;
        let is_last = frame_idx == total_frames - 1;
        let end_info = if is_last {
            ogg::writing::PacketWriteEndInfo::EndStream
        } else {
            ogg::writing::PacketWriteEndInfo::NormalPacket
        };

        packet_writer
            .write_packet(
                encode_buf[..encoded_len].to_vec().into(),
                serial,
                end_info,
                granule_pos,
            )
            .map_err(|e| VoicePipelineError::OggEncode(format!("frame {frame_idx}: {e}")))?;
    }

    // Handle remaining samples (pad with silence if < one frame).
    let remaining = pcm_48k.len() % OPUS_FRAME_SAMPLES;
    if remaining > 0 && total_frames > 0 {
        let mut last_frame = vec![0i16; OPUS_FRAME_SAMPLES];
        let start = total_frames * OPUS_FRAME_SAMPLES;
        last_frame[..remaining].copy_from_slice(&pcm_48k[start..]);

        let encoded_len = encoder
            .encode(&last_frame, &mut encode_buf)
            .map_err(|e| VoicePipelineError::OpusEncode(format!("final: {e}")))?;

        granule_pos += OPUS_FRAME_SAMPLES as u64;
        packet_writer
            .write_packet(
                encode_buf[..encoded_len].to_vec().into(),
                serial,
                ogg::writing::PacketWriteEndInfo::EndStream,
                granule_pos,
            )
            .map_err(|e| VoicePipelineError::OggEncode(format!("final: {e}")))?;
    }

    drop(packet_writer);

    debug!(
        output_bytes = ogg_buf.len(),
        frames = total_frames,
        "OGG/Opus encoded"
    );

    Ok(ogg_buf)
}

/// Build the OpusHead identification header (RFC 7845 §5.1).
fn build_opus_head() -> Vec<u8> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead"); // Magic signature
    head.push(1); // Version
    head.push(1); // Channel count (mono)
    head.extend_from_slice(&0u16.to_le_bytes()); // Pre-skip
    head.extend_from_slice(&48000u32.to_le_bytes()); // Input sample rate
    head.extend_from_slice(&0i16.to_le_bytes()); // Output gain
    head.push(0); // Channel mapping family
    head
}

/// Build the OpusTags comment header (RFC 7845 §5.2).
fn build_opus_tags() -> Vec<u8> {
    let vendor = b"AURA v4";
    let mut tags = Vec::new();
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes()); // No user comments.
    tags
}

// ─── High-level Pipeline Functions ──────────────────────────────────────────

/// Process incoming voice message: decode → resample → prepare for STT.
///
/// Returns PCM at 16kHz (STT-ready) and estimated duration in seconds.
///
/// Note: STT itself is not called here — the caller decides whether to
/// invoke STT (when FFI is wired) or route metadata to the LLM.
pub fn process_voice_input(ogg_bytes: &[u8]) -> Result<VoiceInputResult, VoicePipelineError> {
    // Validate minimum size (an empty OGG is ~200 bytes of headers).
    if ogg_bytes.len() < 256 {
        return Err(VoicePipelineError::TooShort {
            min_ms: 100,
            actual_ms: 0,
        });
    }

    // Decode OGG/Opus → PCM 48kHz.
    let pcm_48k = decode_ogg_opus(ogg_bytes)?;

    // Duration at 48kHz.
    let duration_secs = pcm_48k.len() as f32 / OPUS_SAMPLE_RATE as f32;

    // Validate duration bounds.
    if duration_secs < 0.1 {
        return Err(VoicePipelineError::TooShort {
            min_ms: 100,
            actual_ms: (duration_secs * 1000.0) as u32,
        });
    }
    if duration_secs > 300.0 {
        return Err(VoicePipelineError::TooLong {
            max_secs: 300,
            actual_secs: duration_secs as u32,
        });
    }

    // Resample for STT.
    let pcm_16k = resample_48k_to_16k(&pcm_48k);

    debug!(
        duration_secs = duration_secs,
        pcm_16k_samples = pcm_16k.len(),
        "voice input processed"
    );

    Ok(VoiceInputResult {
        pcm_16k,
        duration_secs,
    })
}

/// Result of processing an incoming voice message.
pub struct VoiceInputResult {
    /// PCM samples at 16kHz mono, ready for STT.
    pub pcm_16k: Vec<i16>,
    /// Duration of the voice message in seconds.
    pub duration_secs: f32,
}

/// Synthesize a voice response from text.
///
/// Currently returns Err(TtsUnavailable) — will be wired when Android TTS
/// JNI bridge or Piper/eSpeak FFI is ready. The caller should fall back to
/// text-only response.
///
/// When wired, the flow will be:
/// 1. TTS engine produces PCM at its native rate (e.g., 22050Hz)
/// 2. Resample to 48kHz
/// 3. Encode to OGG/Opus
/// 4. Return bytes ready for Telegram sendVoice
pub fn synthesize_voice_response(_text: &str) -> Result<Vec<u8>, VoicePipelineError> {
    // TODO(voice-alpha): Wire to Android TTS via JNI or Piper/eSpeak FFI.
    // When ready:
    //   let pcm = tts.synthesize(text)?;
    //   let pcm_48k = resample_to_48k(&pcm, tts.sample_rate());
    //   let ogg = encode_ogg_opus(&pcm_48k)?;
    //   Ok(ogg)
    Err(VoicePipelineError::TtsUnavailable)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resample_48k_to_16k_factor_3() {
        let input: Vec<i16> = (0..9600).map(|i| (i % 1000) as i16).collect();
        let output = resample_48k_to_16k(&input);
        assert_eq!(output.len(), 3200); // 9600 / 3
        assert_eq!(output[0], 0);
        assert_eq!(output[1], 3); // input[3]
    }

    #[test]
    fn test_resample_to_48k_passthrough() {
        let input: Vec<i16> = vec![100, 200, 300];
        let output = resample_to_48k(&input, 48000);
        assert_eq!(output, input);
    }

    #[test]
    fn test_resample_to_48k_upsample() {
        let input: Vec<i16> = vec![0, 1000];
        let output = resample_to_48k(&input, 24000);
        // 24000 → 48000 = 2x, so ~4 output samples.
        assert_eq!(output.len(), 4);
        assert_eq!(output[0], 0); // exact first sample
    }

    #[test]
    fn test_opus_head_structure() {
        let head = build_opus_head();
        assert_eq!(&head[..8], b"OpusHead");
        assert_eq!(head[8], 1); // version
        assert_eq!(head[9], 1); // mono
    }

    #[test]
    fn test_opus_tags_structure() {
        let tags = build_opus_tags();
        assert_eq!(&tags[..8], b"OpusTags");
        let vendor_len = u32::from_le_bytes(tags[8..12].try_into().unwrap());
        assert_eq!(vendor_len, 7); // "AURA v4"
    }

    #[test]
    fn test_process_voice_input_too_small() {
        let result = process_voice_input(&[0u8; 100]);
        assert!(matches!(result, Err(VoicePipelineError::TooShort { .. })));
    }

    #[test]
    fn test_synthesize_voice_response_stub() {
        let result = synthesize_voice_response("Hello");
        assert!(matches!(result, Err(VoicePipelineError::TtsUnavailable)));
    }
}
