//! Voice Activity Detection via Silero VAD (ONNX) with energy-based fallback.
//!
//! Silero VAD is a ~2 MB ONNX model that outputs speech probability per frame.
//! On non-Android platforms (or when the model isn't available), we fall back to
//! a simple energy-based detector.

use std::time::Instant;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum VadError {
    #[error("VAD model failed to load: {0}")]
    ModelLoadFailed(String),
    #[error("invalid frame: expected {expected} samples, got {got}")]
    InvalidFrame { expected: usize, got: usize },
}

pub type VadResult<T> = Result<T, VadError>;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Silero VAD operates on 512-sample windows at 16 kHz (32 ms).
pub const VAD_FRAME_SAMPLES: usize = 512;

/// Default speech probability threshold.
pub const DEFAULT_THRESHOLD: f32 = 0.5;

/// Minimum speech duration to confirm speech (ms).
pub const DEFAULT_MIN_SPEECH_MS: u32 = 250;

/// Minimum silence after speech to declare end-of-utterance (ms).
pub const DEFAULT_MIN_SILENCE_MS: u32 = 500;

/// Frame duration in ms for 512 samples at 16 kHz.
const FRAME_DURATION_MS: u32 = 32;

// ---------------------------------------------------------------------------
// Silero VAD FFI (Android only — ONNX Runtime)
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod silero_ffi {
    use std::os::raw::{c_char, c_float, c_int, c_void};

    extern "C" {
        pub fn silero_vad_create(model_path: *const c_char) -> *mut c_void;
        pub fn silero_vad_destroy(state: *mut c_void);
        /// Process one frame. Returns speech probability [0, 1].
        pub fn silero_vad_process(
            state: *mut c_void,
            samples: *const c_float,
            num_samples: c_int,
        ) -> c_float;
        pub fn silero_vad_reset(state: *mut c_void);
    }
}

// ---------------------------------------------------------------------------
// VAD State
// ---------------------------------------------------------------------------

/// Internal state of the voice activity detector.
#[derive(Debug, Clone, PartialEq)]
pub enum VadState {
    /// No speech detected; accumulating silence duration.
    Silence { duration_ms: u32 },
    /// Speech detected; accumulating speech duration.
    Speech { duration_ms: u32 },
    /// Transitioning between states (debounce window).
    Transition { from_speech: bool, pending_ms: u32 },
}

impl Default for VadState {
    fn default() -> Self {
        VadState::Silence { duration_ms: 0 }
    }
}

/// Result of processing one audio frame.
#[derive(Debug, Clone, PartialEq)]
pub enum VadEvent {
    /// Still silence.
    Silence,
    /// Speech just started (crossed threshold + min duration).
    SpeechStarted,
    /// Speech continues.
    SpeechContinuing,
    /// Speech ended after `duration_ms` of speech followed by sufficient silence.
    SpeechEnded { duration_ms: u32 },
}

// ---------------------------------------------------------------------------
// VoiceActivityDetector
// ---------------------------------------------------------------------------

pub struct VoiceActivityDetector {
    #[cfg(target_os = "android")]
    silero_state: *mut std::ffi::c_void,

    /// Speech probability threshold.
    threshold: f32,
    /// Minimum speech duration before confirming speech (ms).
    min_speech_ms: u32,
    /// Minimum silence after speech to declare end (ms).
    min_silence_ms: u32,
    /// Current state.
    state: VadState,
    /// Accumulated speech duration in current utterance (ms).
    speech_accum_ms: u32,
    /// Accumulated silence after speech (ms).
    silence_accum_ms: u32,
    /// Last frame timestamp for timing.
    last_frame_time: Option<Instant>,
}

// SAFETY: VoiceActivityDetector is Send because the raw `*mut c_void`
// silero_state (present only on Android) is only accessed through &mut self
// methods. The Silero VAD ONNX runtime is not thread-safe per-session, but
// exclusive &mut access ensures no concurrent aliasing during cross-thread moves.
unsafe impl Send for VoiceActivityDetector {}

impl VoiceActivityDetector {
    /// Create a new VAD with default parameters.
    pub fn new() -> Self {
        Self::with_params(
            DEFAULT_THRESHOLD,
            DEFAULT_MIN_SPEECH_MS,
            DEFAULT_MIN_SILENCE_MS,
        )
    }

    /// Create with explicit parameters.
    pub fn with_params(threshold: f32, min_speech_ms: u32, min_silence_ms: u32) -> Self {
        Self {
            #[cfg(target_os = "android")]
            silero_state: std::ptr::null_mut(), // initialized on first use
            threshold,
            min_speech_ms,
            min_silence_ms,
            state: VadState::default(),
            speech_accum_ms: 0,
            silence_accum_ms: 0,
            last_frame_time: None,
        }
    }

    /// Process one frame of `VAD_FRAME_SAMPLES` i16 PCM samples.
    pub fn process_frame(&mut self, samples: &[i16]) -> VadResult<VadEvent> {
        if samples.len() != VAD_FRAME_SAMPLES {
            return Err(VadError::InvalidFrame {
                expected: VAD_FRAME_SAMPLES,
                got: samples.len(),
            });
        }

        let speech_prob = self.compute_speech_probability(samples);
        let is_speech = speech_prob >= self.threshold;

        Ok(self.update_state(is_speech))
    }

    /// Reset the VAD to initial state.
    pub fn reset(&mut self) {
        self.state = VadState::default();
        self.speech_accum_ms = 0;
        self.silence_accum_ms = 0;
        self.last_frame_time = None;

        #[cfg(target_os = "android")]
        if !self.silero_state.is_null() {
            unsafe { silero_ffi::silero_vad_reset(self.silero_state) };
        }
    }

    /// Get current VAD state.
    pub fn state(&self) -> &VadState {
        &self.state
    }

    /// Get the threshold.
    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    /// Set a new threshold.
    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold.clamp(0.0, 1.0);
    }

    // -- Internal -------------------------------------------------------

    /// Compute speech probability for a frame.
    fn compute_speech_probability(&mut self, samples: &[i16]) -> f32 {
        #[cfg(target_os = "android")]
        {
            let float_samples: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
            unsafe {
                silero_ffi::silero_vad_process(
                    self.silero_state,
                    float_samples.as_ptr(),
                    float_samples.len() as std::os::raw::c_int,
                )
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            // Fallback: energy-based VAD
            self.energy_vad(samples)
        }
    }

    /// Simple energy-based VAD for desktop/testing.
    /// Returns pseudo-probability based on RMS energy.
    #[cfg(not(target_os = "android"))]
    fn energy_vad(&self, samples: &[i16]) -> f32 {
        let rms = Self::compute_rms(samples);
        // Map RMS to [0, 1]: silence ~0-200, speech ~500-10000

        (rms / 3000.0).min(1.0)
    }

    /// Compute RMS energy of an i16 sample buffer.
    fn compute_rms(samples: &[i16]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
        (sum_sq / samples.len() as f64).sqrt() as f32
    }

    /// State machine update based on whether current frame is speech.
    fn update_state(&mut self, is_speech: bool) -> VadEvent {
        match &self.state {
            VadState::Silence { .. } => {
                if is_speech {
                    self.speech_accum_ms = FRAME_DURATION_MS;
                    self.silence_accum_ms = 0;
                    if self.speech_accum_ms >= self.min_speech_ms {
                        self.state = VadState::Speech {
                            duration_ms: self.speech_accum_ms,
                        };
                        VadEvent::SpeechStarted
                    } else {
                        self.state = VadState::Transition {
                            from_speech: false,
                            pending_ms: self.speech_accum_ms,
                        };
                        VadEvent::Silence
                    }
                } else {
                    let d = match &self.state {
                        VadState::Silence { duration_ms } => duration_ms + FRAME_DURATION_MS,
                        _ => FRAME_DURATION_MS,
                    };
                    self.state = VadState::Silence { duration_ms: d };
                    VadEvent::Silence
                }
            }

            VadState::Transition {
                from_speech,
                pending_ms,
            } => {
                let from_speech = *from_speech;
                let pending = *pending_ms;

                if !from_speech {
                    // Was in silence, tentatively detecting speech
                    if is_speech {
                        self.speech_accum_ms = pending + FRAME_DURATION_MS;
                        if self.speech_accum_ms >= self.min_speech_ms {
                            self.state = VadState::Speech {
                                duration_ms: self.speech_accum_ms,
                            };
                            VadEvent::SpeechStarted
                        } else {
                            self.state = VadState::Transition {
                                from_speech: false,
                                pending_ms: self.speech_accum_ms,
                            };
                            VadEvent::Silence
                        }
                    } else {
                        // False alarm, back to silence
                        self.speech_accum_ms = 0;
                        self.state = VadState::Silence {
                            duration_ms: FRAME_DURATION_MS,
                        };
                        VadEvent::Silence
                    }
                } else {
                    // Was in speech, tentatively detecting silence
                    if !is_speech {
                        self.silence_accum_ms = pending + FRAME_DURATION_MS;
                        if self.silence_accum_ms >= self.min_silence_ms {
                            let duration = self.speech_accum_ms;
                            self.speech_accum_ms = 0;
                            self.silence_accum_ms = 0;
                            self.state = VadState::Silence { duration_ms: 0 };
                            VadEvent::SpeechEnded {
                                duration_ms: duration,
                            }
                        } else {
                            self.state = VadState::Transition {
                                from_speech: true,
                                pending_ms: self.silence_accum_ms,
                            };
                            VadEvent::SpeechContinuing
                        }
                    } else {
                        // Speech resumed
                        self.silence_accum_ms = 0;
                        self.speech_accum_ms += FRAME_DURATION_MS;
                        self.state = VadState::Speech {
                            duration_ms: self.speech_accum_ms,
                        };
                        VadEvent::SpeechContinuing
                    }
                }
            }

            VadState::Speech { duration_ms } => {
                let d = *duration_ms;
                if is_speech {
                    self.speech_accum_ms = d + FRAME_DURATION_MS;
                    self.state = VadState::Speech {
                        duration_ms: self.speech_accum_ms,
                    };
                    VadEvent::SpeechContinuing
                } else {
                    self.silence_accum_ms = FRAME_DURATION_MS;
                    self.state = VadState::Transition {
                        from_speech: true,
                        pending_ms: self.silence_accum_ms,
                    };
                    VadEvent::SpeechContinuing
                }
            }
        }
    }
}

impl Default for VoiceActivityDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for VoiceActivityDetector {
    fn drop(&mut self) {
        #[cfg(target_os = "android")]
        if !self.silero_state.is_null() {
            unsafe { silero_ffi::silero_vad_destroy(self.silero_state) };
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_silence_frame() -> Vec<i16> {
        vec![0i16; VAD_FRAME_SAMPLES]
    }

    fn make_speech_frame(amplitude: i16) -> Vec<i16> {
        // Simulate speech with a sine-like pattern at high amplitude
        (0..VAD_FRAME_SAMPLES)
            .map(|i| {
                let t = i as f32 / VAD_FRAME_SAMPLES as f32;
                (amplitude as f32 * (t * 2.0 * std::f32::consts::PI * 5.0).sin()) as i16
            })
            .collect()
    }

    #[test]
    fn silence_stays_silent() {
        let mut vad = VoiceActivityDetector::new();
        let frame = make_silence_frame();
        for _ in 0..20 {
            let event = vad.process_frame(&frame).unwrap();
            assert_eq!(event, VadEvent::Silence);
        }
    }

    #[test]
    fn loud_speech_triggers_detection() {
        let mut vad = VoiceActivityDetector::new();
        let frame = make_speech_frame(20_000);

        let mut saw_started = false;
        // Feed enough frames to exceed min_speech_ms (250ms / 32ms ≈ 8 frames)
        for _ in 0..20 {
            let event = vad.process_frame(&frame).unwrap();
            if event == VadEvent::SpeechStarted {
                saw_started = true;
                break;
            }
        }
        assert!(saw_started, "expected SpeechStarted event");
    }

    #[test]
    fn speech_end_after_silence() {
        let mut vad = VoiceActivityDetector::new();
        let speech = make_speech_frame(20_000);
        let silence = make_silence_frame();

        // Start speech
        for _ in 0..20 {
            vad.process_frame(&speech).unwrap();
        }

        // Then feed silence until we get SpeechEnded
        let mut saw_ended = false;
        for _ in 0..50 {
            let event = vad.process_frame(&silence).unwrap();
            if let VadEvent::SpeechEnded { duration_ms } = event {
                assert!(duration_ms > 0);
                saw_ended = true;
                break;
            }
        }
        assert!(saw_ended, "expected SpeechEnded event");
    }

    #[test]
    fn reset_clears_state() {
        let mut vad = VoiceActivityDetector::new();
        let speech = make_speech_frame(20_000);
        for _ in 0..20 {
            vad.process_frame(&speech).unwrap();
        }

        vad.reset();
        assert_eq!(*vad.state(), VadState::Silence { duration_ms: 0 });
    }

    #[test]
    fn wrong_frame_size_rejected() {
        let mut vad = VoiceActivityDetector::new();
        let short_frame = vec![0i16; 100];
        assert!(vad.process_frame(&short_frame).is_err());
    }

    #[test]
    fn rms_computation() {
        // Silence
        let silence = vec![0i16; 100];
        assert_eq!(VoiceActivityDetector::compute_rms(&silence), 0.0);

        // Constant signal
        let constant = vec![1000i16; 100];
        let rms = VoiceActivityDetector::compute_rms(&constant);
        assert!((rms - 1000.0).abs() < 1.0);
    }
}
