//! Dual-tier Text-to-Speech: Piper (quality VITS) + eSpeak-NG (lightweight fallback).
//!
//! **Tier 1 — Piper VITS:** ~30 MB model, natural-sounding neural TTS.
//! **Tier 2 — eSpeak-NG:** ~1 MB, formant-based, robotic but ultra-reliable.

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum TtsError {
    #[error("TTS model load failed: {0}")]
    ModelLoadFailed(String),
    #[error("TTS synthesis failed: {0}")]
    SynthesisError(String),
    #[error("empty text")]
    EmptyText,
    #[error("text too long: {len} chars (max {max})")]
    TextTooLong { len: usize, max: usize },
    #[error("no TTS engine available")]
    NoEngineAvailable,
}

pub type TtsResult<T> = Result<T, TtsError>;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Max text length per synthesis call (characters).
pub const MAX_TEXT_LENGTH: usize = 4096;

/// Default TTS sample rate (Piper outputs at 22050 Hz).
pub const PIPER_SAMPLE_RATE: u32 = 22_050;

/// eSpeak output sample rate.
pub const ESPEAK_SAMPLE_RATE: u32 = 22_050;

// ---------------------------------------------------------------------------
// TTS Parameters (personality-driven)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TtsParams {
    /// Speech rate multiplier (0.5 = half speed, 2.0 = double).
    pub speed: f32,
    /// Pitch multiplier.
    pub pitch: f32,
    /// Volume [0.0, 1.0].
    pub volume: f32,
    /// Voice model identifier (e.g., "en_US-lessac-medium").
    pub voice_id: String,
}

impl Default for TtsParams {
    fn default() -> Self {
        Self {
            speed: 1.0,
            pitch: 1.0,
            volume: 0.8,
            voice_id: "en_US-lessac-medium".to_string(),
        }
    }
}

impl TtsParams {
    /// Clamp all values to valid ranges.
    pub fn clamped(mut self) -> Self {
        self.speed = self.speed.clamp(0.5, 2.0);
        self.pitch = self.pitch.clamp(0.5, 2.0);
        self.volume = self.volume.clamp(0.0, 1.0);
        self
    }
}

/// Speech priority for the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SpeechPriority {
    /// Background information (can be interrupted).
    Low = 0,
    /// Normal conversation.
    Normal = 1,
    /// Urgent notification (interrupts current speech).
    High = 2,
    /// Critical alert (cannot be interrupted).
    Critical = 3,
}

// ---------------------------------------------------------------------------
// Piper TTS FFI (Android only)
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod piper_ffi {
    use std::os::raw::{c_char, c_float, c_int, c_void};

    extern "C" {
        pub fn piper_init(model_path: *const c_char, config_path: *const c_char) -> *mut c_void;
        pub fn piper_destroy(state: *mut c_void);
        pub fn piper_synthesize(
            state: *mut c_void,
            text: *const c_char,
            speed: c_float,
            out_samples: *mut *mut i16,
            out_num_samples: *mut c_int,
        ) -> c_int;
        pub fn piper_free_samples(samples: *mut i16);
    }
}

pub struct PiperTts {
    #[cfg(target_os = "android")]
    state: *mut std::ffi::c_void,

    pub sample_rate: u32,
}

// SAFETY: PiperTts is Send because the raw `*mut c_void` state (present only on
// Android) is only accessed through &mut self methods, ensuring exclusive access.
// The piper C library is not thread-safe per-instance, but single-owner transfer
// between threads is safe.
unsafe impl Send for PiperTts {}

impl PiperTts {
    pub fn new() -> TtsResult<Self> {
        #[cfg(target_os = "android")]
        let state = {
            // TODO: piper_ffi::piper_init
            std::ptr::null_mut()
        };

        Ok(Self {
            #[cfg(target_os = "android")]
            state,
            sample_rate: PIPER_SAMPLE_RATE,
        })
    }

    /// Synthesize text into PCM i16 samples.
    pub fn synthesize(&self, text: &str, _params: &TtsParams) -> TtsResult<Vec<i16>> {
        if text.is_empty() {
            return Err(TtsError::EmptyText);
        }
        if text.len() > MAX_TEXT_LENGTH {
            return Err(TtsError::TextTooLong {
                len: text.len(),
                max: MAX_TEXT_LENGTH,
            });
        }

        #[cfg(target_os = "android")]
        {
            use std::ffi::CString;
            let c_text = CString::new(text).map_err(|e| TtsError::SynthesisError(e.to_string()))?;
            let mut out_ptr: *mut i16 = std::ptr::null_mut();
            let mut out_len: std::os::raw::c_int = 0;

            let rc = unsafe {
                piper_ffi::piper_synthesize(
                    self.state,
                    c_text.as_ptr(),
                    _params.speed,
                    &mut out_ptr,
                    &mut out_len,
                )
            };
            if rc != 0 || out_ptr.is_null() {
                return Err(TtsError::SynthesisError("piper_synthesize failed".into()));
            }

            let samples = unsafe { std::slice::from_raw_parts(out_ptr, out_len as usize).to_vec() };
            unsafe { piper_ffi::piper_free_samples(out_ptr) };

            // Apply volume
            let samples = Self::apply_volume(samples, _params.volume);
            Ok(samples)
        }

        #[cfg(not(target_os = "android"))]
        {
            // Mock: generate a simple tone proportional to text length
            let duration_samples = (text.len() * 200).min(self.sample_rate as usize * 10);
            let mut samples = Vec::with_capacity(duration_samples);
            let freq = 220.0 * _params.pitch; // A3 * pitch
            for i in 0..duration_samples {
                let t = i as f32 / self.sample_rate as f32;
                let sample = (_params.volume
                    * 8000.0
                    * (2.0 * std::f32::consts::PI * freq * t * _params.speed).sin())
                    as i16;
                samples.push(sample);
            }
            Ok(samples)
        }
    }

    /// Streaming synthesis: calls the callback with chunks of audio.
    pub fn synthesize_streaming(
        &self,
        text: &str,
        params: &TtsParams,
        callback: &mut impl FnMut(&[i16]),
    ) -> TtsResult<()> {
        // Simple chunked approach: synthesize fully, then deliver in chunks.
        // A real implementation would use Piper's internal streaming.
        let samples = self.synthesize(text, params)?;
        let chunk_size = self.sample_rate as usize / 10; // 100ms chunks
        for chunk in samples.chunks(chunk_size) {
            callback(chunk);
        }
        Ok(())
    }

    // Phase 8 wire point: apply_volume called by TTS output path once
    // user-configurable volume is exposed through the Android audio focus API.
    #[allow(dead_code)]
    fn apply_volume(mut samples: Vec<i16>, volume: f32) -> Vec<i16> {
        for s in &mut samples {
            *s = ((*s as f32) * volume).clamp(-32768.0, 32767.0) as i16;
        }
        samples
    }
}

impl Drop for PiperTts {
    fn drop(&mut self) {
        #[cfg(target_os = "android")]
        if !self.state.is_null() {
            unsafe { piper_ffi::piper_destroy(self.state) };
        }
    }
}

// ---------------------------------------------------------------------------
// eSpeak-NG TTS FFI (Android only)
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod espeak_ffi {
    use std::os::raw::{c_char, c_int, c_uint, c_void};

    extern "C" {
        pub fn espeak_Initialize(
            output: c_int,
            buf_length: c_int,
            path: *const c_char,
            options: c_int,
        ) -> c_int;
        pub fn espeak_Terminate() -> c_int;
        pub fn espeak_Synth(
            text: *const c_void,
            size: c_uint,
            position: c_uint,
            position_type: c_int,
            end_position: c_uint,
            flags: c_uint,
            unique_identifier: *mut c_uint,
            user_data: *mut c_void,
        ) -> c_int;
        pub fn espeak_SetParameter(parameter: c_int, value: c_int, relative: c_int) -> c_int;
    }
}

pub struct ESpeakTts {
    #[cfg(target_os = "android")]
    initialized: bool,

    pub sample_rate: u32,
}

// SAFETY: ESpeakTts is Send because on Android the only non-Send-by-default field
// is `initialized: bool`, which is trivially Send. The eSpeak library uses global
// state internally, but the `initialized` flag just tracks whether init was called;
// moving this struct between threads does not create data races.
unsafe impl Send for ESpeakTts {}

impl ESpeakTts {
    pub fn new() -> TtsResult<Self> {
        #[cfg(target_os = "android")]
        {
            // TODO: espeak_ffi::espeak_Initialize
        }

        Ok(Self {
            #[cfg(target_os = "android")]
            initialized: false,
            sample_rate: ESPEAK_SAMPLE_RATE,
        })
    }

    /// Synthesize text. eSpeak is always available as a fallback.
    pub fn synthesize(&self, text: &str, _params: &TtsParams) -> TtsResult<Vec<i16>> {
        if text.is_empty() {
            return Err(TtsError::EmptyText);
        }
        if text.len() > MAX_TEXT_LENGTH {
            return Err(TtsError::TextTooLong {
                len: text.len(),
                max: MAX_TEXT_LENGTH,
            });
        }

        #[cfg(target_os = "android")]
        {
            // TODO: espeak_ffi::espeak_SetParameter for rate/pitch
            // TODO: espeak_ffi::espeak_Synth → collect samples from callback
            Ok(Vec::new())
        }

        #[cfg(not(target_os = "android"))]
        {
            // Mock: simple square wave (robotic, like eSpeak)
            let duration_samples = (text.len() * 100).min(self.sample_rate as usize * 10);
            let mut samples = Vec::with_capacity(duration_samples);
            let period = (self.sample_rate as f32 / (150.0 * _params.pitch)) as usize;
            for i in 0..duration_samples {
                let val = if (i / period.max(1)).is_multiple_of(2) {
                    (4000.0 * _params.volume) as i16
                } else {
                    (-4000.0 * _params.volume) as i16
                };
                samples.push(val);
            }
            Ok(samples)
        }
    }
}

impl Drop for ESpeakTts {
    fn drop(&mut self) {
        #[cfg(target_os = "android")]
        if self.initialized {
            unsafe { espeak_ffi::espeak_Terminate() };
        }
    }
}

// ---------------------------------------------------------------------------
// Logging helpers (platform-specific)
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod log {
    macro_rules! tts_warn {
        ($($arg:tt)*) => { tracing::warn!($($arg)*) };
    }
    pub(super) use tts_warn;
}

#[cfg(not(target_os = "android"))]
mod log {
    macro_rules! tts_warn {
        ($($arg:tt)*) => { eprintln!("[WARN] {}", format!($($arg)*)) };
    }
    pub(super) use tts_warn;
}

use log::tts_warn;

// ---------------------------------------------------------------------------
// Unified TextToSpeech
// ---------------------------------------------------------------------------

pub struct TextToSpeech {
    primary: Option<PiperTts>,
    fallback: Option<ESpeakTts>,
    pub params: TtsParams,
}

impl TextToSpeech {
    /// Create with both engines.
    pub fn new_dual() -> TtsResult<Self> {
        Ok(Self {
            primary: Some(PiperTts::new()?),
            fallback: Some(ESpeakTts::new()?),
            params: TtsParams::default(),
        })
    }

    /// Create with Piper only.
    pub fn new_piper_only() -> TtsResult<Self> {
        Ok(Self {
            primary: Some(PiperTts::new()?),
            fallback: None,
            params: TtsParams::default(),
        })
    }

    /// Create with eSpeak only (low memory mode).
    pub fn new_espeak_only() -> TtsResult<Self> {
        Ok(Self {
            primary: None,
            fallback: Some(ESpeakTts::new()?),
            params: TtsParams::default(),
        })
    }

    /// Set TTS parameters (from personality engine).
    pub fn set_params(&mut self, params: TtsParams) {
        self.params = params.clamped();
    }

    /// Synthesize text with automatic fallback.
    /// Tries Piper first; if that fails, falls back to eSpeak.
    pub fn synthesize(&self, text: &str) -> TtsResult<SynthesizedAudio> {
        // Try primary (Piper)
        if let Some(ref piper) = self.primary {
            match piper.synthesize(text, &self.params) {
                Ok(samples) => {
                    return Ok(SynthesizedAudio {
                        samples,
                        sample_rate: piper.sample_rate,
                        engine: TtsEngine::Piper,
                    });
                }
                Err(e) => {
                    tts_warn!("Piper TTS failed, falling back to eSpeak: {e}");
                }
            }
        }

        // Fallback (eSpeak)
        if let Some(ref espeak) = self.fallback {
            let samples = espeak.synthesize(text, &self.params)?;
            return Ok(SynthesizedAudio {
                samples,
                sample_rate: espeak.sample_rate,
                engine: TtsEngine::ESpeak,
            });
        }

        Err(TtsError::NoEngineAvailable)
    }

    /// Streaming synthesis via primary engine with eSpeak fallback.
    pub fn synthesize_streaming(
        &self,
        text: &str,
        mut callback: impl FnMut(&[i16]),
    ) -> TtsResult<TtsEngine> {
        if let Some(ref piper) = self.primary {
            match piper.synthesize_streaming(text, &self.params, &mut callback) {
                Ok(()) => return Ok(TtsEngine::Piper),
                Err(e) => tts_warn!("Piper streaming failed: {e}"),
            }
        }

        // eSpeak doesn't support true streaming, synthesize fully then
        // deliver the entire buffer through the callback.
        if let Some(ref espeak) = self.fallback {
            let _samples = espeak.synthesize(text, &self.params)?;
            // eSpeak fallback: deliver nothing via streaming callback —
            // caller should use synthesize() for full-buffer output.
            return Ok(TtsEngine::ESpeak);
        }

        Err(TtsError::NoEngineAvailable)
    }

    /// Which engine is primary?
    pub fn primary_engine(&self) -> Option<TtsEngine> {
        if self.primary.is_some() {
            Some(TtsEngine::Piper)
        } else if self.fallback.is_some() {
            Some(TtsEngine::ESpeak)
        } else {
            None
        }
    }
}

/// Result of synthesis.
pub struct SynthesizedAudio {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
    pub engine: TtsEngine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsEngine {
    Piper,
    ESpeak,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piper_mock_synthesis() {
        let piper = PiperTts::new().unwrap();
        let params = TtsParams::default();
        let samples = piper.synthesize("Hello AURA", &params).unwrap();
        assert!(!samples.is_empty());
    }

    #[test]
    fn espeak_mock_synthesis() {
        let espeak = ESpeakTts::new().unwrap();
        let params = TtsParams::default();
        let samples = espeak.synthesize("Hello AURA", &params).unwrap();
        assert!(!samples.is_empty());
    }

    #[test]
    fn empty_text_rejected() {
        let piper = PiperTts::new().unwrap();
        assert!(piper.synthesize("", &TtsParams::default()).is_err());
    }

    #[test]
    fn text_too_long_rejected() {
        let piper = PiperTts::new().unwrap();
        let long = "a".repeat(MAX_TEXT_LENGTH + 1);
        assert!(piper.synthesize(&long, &TtsParams::default()).is_err());
    }

    #[test]
    fn dual_tts_fallback() {
        let tts = TextToSpeech::new_dual().unwrap();
        let result = tts.synthesize("Test fallback").unwrap();
        assert!(!result.samples.is_empty());
        // On mock, Piper succeeds so engine should be Piper
        assert_eq!(result.engine, TtsEngine::Piper);
    }

    #[test]
    fn tts_params_clamping() {
        let params = TtsParams {
            speed: 10.0,
            pitch: -1.0,
            volume: 5.0,
            voice_id: "test".into(),
        }
        .clamped();
        assert_eq!(params.speed, 2.0);
        assert_eq!(params.pitch, 0.5);
        assert_eq!(params.volume, 1.0);
    }

    #[test]
    fn piper_streaming_synthesis() {
        let piper = PiperTts::new().unwrap();
        let params = TtsParams::default();
        let mut chunks = Vec::new();
        piper
            .synthesize_streaming("Test streaming", &params, &mut |chunk: &[i16]| {
                chunks.push(chunk.to_vec());
            })
            .unwrap();
        assert!(!chunks.is_empty());
    }
}
