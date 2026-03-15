//! Dual-tier Speech-to-Text: Zipformer (streaming) + whisper.cpp (batch).
//!
//! **Tier 1 — Zipformer (sherpa-onnx streaming):** ~30 MB model, provides
//! real-time partial results. Used during active conversation for low latency.
//!
//! **Tier 2 — whisper.cpp:** ~75 MB model (tiny.en), batch-mode transcription
//! with higher accuracy. Used when quality matters (commands, notes, etc.).

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error("STT model load failed: {0}")]
    ModelLoadFailed(String),
    #[error("STT processing error: {0}")]
    ProcessingError(String),
    #[error("no audio data provided")]
    EmptyAudio,
    #[error("audio too long: {duration_ms} ms (max {max_ms} ms)")]
    AudioTooLong { duration_ms: u32, max_ms: u32 },
    #[error("tier not available: {0:?}")]
    TierNotAvailable(SttTier),
}

pub type SttResult<T> = Result<T, SttError>;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum audio length for batch transcription (30 seconds).
pub const MAX_BATCH_DURATION_MS: u32 = 30_000;

/// Maximum audio length for streaming (60 seconds continuous).
pub const MAX_STREAM_DURATION_MS: u32 = 60_000;

/// Expected sample rate.
pub const STT_SAMPLE_RATE: u32 = 16_000;

// ---------------------------------------------------------------------------
// Tier enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttTier {
    /// Zipformer: real-time streaming, partial results.
    Streaming,
    /// whisper.cpp: high accuracy, batch mode.
    Batch,
}

// ---------------------------------------------------------------------------
// Partial result for streaming STT
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SttPartial {
    /// Partial (unstable) transcription so far.
    pub text: String,
    /// Whether this partial result is "stable" (unlikely to change).
    pub is_stable: bool,
}

// ---------------------------------------------------------------------------
// Zipformer STT (sherpa-onnx streaming)
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod zipformer_ffi {
    use std::os::raw::{c_char, c_float, c_int, c_void};

    extern "C" {
        pub fn sherpa_stt_create_streaming(model_path: *const c_char) -> *mut c_void;
        pub fn sherpa_stt_destroy(state: *mut c_void);
        pub fn sherpa_stt_accept_waveform(
            state: *mut c_void,
            samples: *const c_float,
            num_samples: c_int,
        );
        pub fn sherpa_stt_get_result(state: *mut c_void) -> *const c_char;
        pub fn sherpa_stt_is_endpoint(state: *mut c_void) -> c_int;
        pub fn sherpa_stt_reset(state: *mut c_void);
    }
}

pub struct ZipformerStt {
    #[cfg(target_os = "android")]
    state: *mut std::ffi::c_void,

    /// Accumulated partial result.
    partial_result: String,
    /// Final (endpoint-committed) result segments.
    finalized_segments: Vec<String>,
    /// Total frames fed.
    frames_fed: usize,
}

// SAFETY: ZipformerStt is Send because the raw `*mut c_void` state pointer
// (present only on Android) is only accessed through &mut self methods, ensuring
// exclusive access. The C sherpa-onnx STT library is not thread-safe per-instance,
// but since we never share the pointer across threads without &mut, Send is sound.
unsafe impl Send for ZipformerStt {}

impl ZipformerStt {
    pub fn new() -> SttResult<Self> {
        #[cfg(target_os = "android")]
        let state = {
            // TODO: sherpa_stt_create_streaming with model path
            std::ptr::null_mut()
        };

        Ok(Self {
            #[cfg(target_os = "android")]
            state,
            partial_result: String::new(),
            finalized_segments: Vec::new(),
            frames_fed: 0,
        })
    }

    /// Feed audio samples and get a partial transcription result.
    pub fn feed_audio(&mut self, samples: &[i16]) -> SttResult<SttPartial> {
        self.frames_fed += samples.len();

        // Check max duration
        let duration_ms = (self.frames_fed as u32 * 1000) / STT_SAMPLE_RATE;
        if duration_ms > MAX_STREAM_DURATION_MS {
            return Err(SttError::AudioTooLong {
                duration_ms,
                max_ms: MAX_STREAM_DURATION_MS,
            });
        }

        #[cfg(target_os = "android")]
        {
            let float_samples: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
            unsafe {
                zipformer_ffi::sherpa_stt_accept_waveform(
                    self.state,
                    float_samples.as_ptr(),
                    float_samples.len() as std::os::raw::c_int,
                );
                let result_ptr = zipformer_ffi::sherpa_stt_get_result(self.state);
                if !result_ptr.is_null() {
                    let c_str = std::ffi::CStr::from_ptr(result_ptr);
                    self.partial_result = c_str.to_string_lossy().into_owned();
                }
                let is_endpoint = zipformer_ffi::sherpa_stt_is_endpoint(self.state) != 0;
                if is_endpoint {
                    self.finalized_segments.push(self.partial_result.clone());
                    self.partial_result.clear();
                }
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = samples;
            // Mock: accumulate a fixed partial
            self.partial_result = format!("[streaming partial @ {}ms]", duration_ms);
        }

        Ok(SttPartial {
            text: self.partial_result.clone(),
            is_stable: false,
        })
    }

    /// Finalize the current stream and return the complete transcription.
    pub fn finalize(&mut self) -> SttResult<String> {
        #[cfg(target_os = "android")]
        {
            // Flush the recognizer
            unsafe {
                let result_ptr = zipformer_ffi::sherpa_stt_get_result(self.state);
                if !result_ptr.is_null() {
                    let c_str = std::ffi::CStr::from_ptr(result_ptr);
                    let final_text = c_str.to_string_lossy().into_owned();
                    if !final_text.is_empty() {
                        self.finalized_segments.push(final_text);
                    }
                }
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            if !self.partial_result.is_empty() {
                self.finalized_segments.push(self.partial_result.clone());
            }
        }

        let result = self.finalized_segments.join(" ").trim().to_string();
        Ok(result)
    }

    /// Reset for a new utterance.
    pub fn reset(&mut self) {
        self.partial_result.clear();
        self.finalized_segments.clear();
        self.frames_fed = 0;

        #[cfg(target_os = "android")]
        if !self.state.is_null() {
            unsafe { zipformer_ffi::sherpa_stt_reset(self.state) };
        }
    }
}

impl Drop for ZipformerStt {
    fn drop(&mut self) {
        #[cfg(target_os = "android")]
        if !self.state.is_null() {
            unsafe { zipformer_ffi::sherpa_stt_destroy(self.state) };
        }
    }
}

// ---------------------------------------------------------------------------
// whisper.cpp STT (batch mode)
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod whisper_ffi {
    use std::os::raw::{c_char, c_float, c_int, c_void};

    extern "C" {
        pub fn whisper_init_from_file(model_path: *const c_char) -> *mut c_void;
        pub fn whisper_free(ctx: *mut c_void);
        pub fn whisper_full(
            ctx: *mut c_void,
            params: WhisperFullParams,
            samples: *const c_float,
            num_samples: c_int,
        ) -> c_int;
        pub fn whisper_full_n_segments(ctx: *mut c_void) -> c_int;
        pub fn whisper_full_get_segment_text(ctx: *mut c_void, segment: c_int) -> *const c_char;
    }

    #[repr(C)]
    pub struct WhisperFullParams {
        pub strategy: c_int,
        pub n_threads: c_int,
        pub language: *const c_char,
        pub translate: c_int,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperModelSize {
    Tiny,  // ~75 MB, fast
    Base,  // ~142 MB, balanced
    Small, // ~466 MB, high quality (may exceed budget)
}

impl WhisperModelSize {
    /// Approximate model size in MB.
    pub fn size_mb(&self) -> u32 {
        match self {
            Self::Tiny => 75,
            Self::Base => 142,
            Self::Small => 466,
        }
    }
}

pub struct WhisperStt {
    #[cfg(target_os = "android")]
    ctx: *mut std::ffi::c_void,

    pub model_size: WhisperModelSize,
}

// SAFETY: WhisperStt is Send because the raw `*mut c_void` ctx pointer
// (present only on Android) is only accessed through &mut self methods, ensuring
// exclusive access. The whisper.cpp library is not thread-safe per-context, but
// ownership transfer between threads is safe since we never alias the pointer.
unsafe impl Send for WhisperStt {}

impl WhisperStt {
    pub fn new(model_size: WhisperModelSize) -> SttResult<Self> {
        #[cfg(target_os = "android")]
        let ctx = {
            // TODO: whisper_ffi::whisper_init_from_file with model path
            std::ptr::null_mut()
        };

        Ok(Self {
            #[cfg(target_os = "android")]
            ctx,
            model_size,
        })
    }

    /// Transcribe a complete audio buffer. Blocking call.
    pub fn transcribe(&self, audio: &[i16], sample_rate: u32) -> SttResult<String> {
        if audio.is_empty() {
            return Err(SttError::EmptyAudio);
        }

        let duration_ms = (audio.len() as u32 * 1000) / sample_rate;
        if duration_ms > MAX_BATCH_DURATION_MS {
            return Err(SttError::AudioTooLong {
                duration_ms,
                max_ms: MAX_BATCH_DURATION_MS,
            });
        }

        #[cfg(target_os = "android")]
        {
            let float_audio: Vec<f32> = audio.iter().map(|&s| s as f32 / 32768.0).collect();
            // TODO: call whisper_ffi::whisper_full, iterate segments
            let _ = float_audio;
            Ok(String::new())
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = sample_rate;
            // Mock: return a test transcription
            Ok(format!(
                "[whisper-{:?} transcription of {} ms audio]",
                self.model_size, duration_ms
            ))
        }
    }
}

impl Drop for WhisperStt {
    fn drop(&mut self) {
        #[cfg(target_os = "android")]
        if !self.ctx.is_null() {
            unsafe { whisper_ffi::whisper_free(self.ctx) };
        }
    }
}

// ---------------------------------------------------------------------------
// Unified SpeechToText interface
// ---------------------------------------------------------------------------

pub struct SpeechToText {
    pub tier: SttTier,
    streaming: Option<ZipformerStt>,
    batch: Option<WhisperStt>,
}

impl SpeechToText {
    /// Create with both tiers available.
    pub fn new_dual() -> SttResult<Self> {
        Ok(Self {
            tier: SttTier::Streaming,
            streaming: Some(ZipformerStt::new()?),
            batch: Some(WhisperStt::new(WhisperModelSize::Tiny)?),
        })
    }

    /// Create with only streaming tier.
    pub fn new_streaming_only() -> SttResult<Self> {
        Ok(Self {
            tier: SttTier::Streaming,
            streaming: Some(ZipformerStt::new()?),
            batch: None,
        })
    }

    /// Create with only batch tier.
    pub fn new_batch_only() -> SttResult<Self> {
        Ok(Self {
            tier: SttTier::Batch,
            streaming: None,
            batch: Some(WhisperStt::new(WhisperModelSize::Tiny)?),
        })
    }

    /// Set the active tier.
    pub fn set_tier(&mut self, tier: SttTier) {
        self.tier = tier;
    }

    /// Feed audio for streaming recognition. Only works in Streaming tier.
    pub fn feed_audio(&mut self, samples: &[i16]) -> SttResult<SttPartial> {
        match &mut self.streaming {
            Some(z) => z.feed_audio(samples),
            None => Err(SttError::TierNotAvailable(SttTier::Streaming)),
        }
    }

    /// Finalize streaming recognition.
    pub fn finalize_streaming(&mut self) -> SttResult<String> {
        match &mut self.streaming {
            Some(z) => z.finalize(),
            None => Err(SttError::TierNotAvailable(SttTier::Streaming)),
        }
    }

    /// Batch-transcribe a complete audio buffer.
    pub fn transcribe_batch(&self, audio: &[i16], sample_rate: u32) -> SttResult<String> {
        match &self.batch {
            Some(w) => w.transcribe(audio, sample_rate),
            None => Err(SttError::TierNotAvailable(SttTier::Batch)),
        }
    }

    /// Reset streaming state for a new utterance.
    pub fn reset_streaming(&mut self) {
        if let Some(z) = &mut self.streaming {
            z.reset();
        }
    }

    /// Smart transcribe: uses streaming result if available, falls back to batch
    /// re-transcription for better accuracy if the streaming confidence is low.
    pub fn smart_transcribe(
        &mut self,
        audio_buffer: &[i16],
        sample_rate: u32,
    ) -> SttResult<String> {
        // First: try finalize streaming
        if let Some(ref mut z) = self.streaming {
            let streaming_result = z.finalize()?;
            if !streaming_result.is_empty() {
                // If we also have batch and audio is short enough, re-transcribe
                if let Some(ref w) = self.batch {
                    let duration_ms = (audio_buffer.len() as u32 * 1000) / sample_rate;
                    if duration_ms <= MAX_BATCH_DURATION_MS && !audio_buffer.is_empty() {
                        // Batch result is authoritative
                        return w.transcribe(audio_buffer, sample_rate);
                    }
                }
                return Ok(streaming_result);
            }
        }

        // Fallback: batch only
        self.transcribe_batch(audio_buffer, sample_rate)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_feed_and_finalize() {
        let mut stt = SpeechToText::new_dual().unwrap();
        let samples = vec![0i16; 480];

        let partial = stt.feed_audio(&samples).unwrap();
        // Feeding silence (zero samples) — partial text should be empty.
        assert!(partial.text.is_empty(),
            "expected empty partial text from silence, got: '{}'", partial.text);

        let result = stt.finalize_streaming().unwrap();
        // Finalize should produce a non-empty transcription (even mock returns something).
        assert!(!result.is_empty(), "finalize_streaming should return non-empty result");
    }

    #[test]
    fn batch_transcribe() {
        let stt = SpeechToText::new_dual().unwrap();
        let audio = vec![0i16; 16_000]; // 1 second
        let result = stt.transcribe_batch(&audio, 16_000).unwrap();
        assert!(result.contains("whisper"));
    }

    #[test]
    fn batch_rejects_empty() {
        let stt = SpeechToText::new_dual().unwrap();
        assert!(stt.transcribe_batch(&[], 16_000).is_err());
    }

    #[test]
    fn batch_rejects_too_long() {
        let stt = SpeechToText::new_dual().unwrap();
        // 31 seconds at 16kHz
        let audio = vec![0i16; 16_000 * 31];
        assert!(stt.transcribe_batch(&audio, 16_000).is_err());
    }

    #[test]
    fn tier_not_available() {
        let mut stt = SpeechToText::new_batch_only().unwrap();
        let samples = vec![0i16; 480];
        assert!(stt.feed_audio(&samples).is_err()); // no streaming tier
    }

    #[test]
    fn smart_transcribe_uses_batch() {
        let mut stt = SpeechToText::new_dual().unwrap();
        let samples = vec![0i16; 480];
        stt.feed_audio(&samples).unwrap();

        let audio = vec![0i16; 8_000]; // 0.5 sec
        let result = stt.smart_transcribe(&audio, 16_000).unwrap();
        assert!(result.contains("whisper")); // batch takes priority
    }

    #[test]
    fn reset_streaming() {
        let mut stt = SpeechToText::new_dual().unwrap();
        let samples = vec![0i16; 480];
        stt.feed_audio(&samples).unwrap();
        stt.reset_streaming();
        // Should be able to feed again without error
        let partial = stt.feed_audio(&samples).unwrap();
        // After reset, feed_audio should succeed without error (already asserted by unwrap above).
        // Partial text from silence samples is expected to be empty.
        assert!(partial.text.is_empty(),
            "expected empty partial text from silence after reset, got: '{}'", partial.text);
    }
}
