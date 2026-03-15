//! AURA v4 Voice Engine — Module root.
//!
//! Provides the `VoiceEngine` facade that orchestrates:
//! - Audio capture/playback (Oboe)
//! - Signal processing (RNNoise denoising + AEC)
//! - Wake word detection (sherpa-onnx KWS)
//! - Voice Activity Detection (Silero VAD)
//! - Speech-to-text (dual-tier: Zipformer streaming + whisper.cpp batch)
//! - Text-to-speech (dual-tier: Piper VITS + eSpeak-NG)
//! - Voice biomarker extraction (F0, jitter, shimmer)
//! - Phone call handling (AccessibilityService)
//! - Modality state machine (Idle → Listening → Speaking → Call)
//! - Personality-driven voice parameters

pub mod audio_io;
pub mod biomarkers;
pub mod call_handler;
pub mod modality_state_machine;
pub mod personality_voice;
pub mod signal_processing;
pub mod stt;
pub mod tts;
pub mod vad;
pub mod wake_word;

// Re-exports for convenience
pub use audio_io::AudioIo;
pub use biomarkers::{BiomarkerExtractor, EmotionalSignal, VoiceBiomarkers};
pub use call_handler::{CallEvent, CallHandler, CallState};
pub use modality_state_machine::{ModalityState, ModalityStateMachine, SpeakingMode, VoiceEvent};
pub use personality_voice::{MoodState, OceanScores, SpeechContext};
pub use signal_processing::SignalProcessor;
pub use stt::{SpeechToText, SttTier};
pub use tts::{SpeechPriority, TextToSpeech, TtsParams};
pub use vad::{VadEvent, VoiceActivityDetector};
pub use wake_word::WakeWordDetector;

// ---------------------------------------------------------------------------
// Unified error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error("audio error: {0}")]
    Audio(#[from] audio_io::AudioError),
    #[error("signal processing error: {0}")]
    Signal(#[from] signal_processing::SignalError),
    #[error("VAD error: {0}")]
    Vad(#[from] vad::VadError),
    #[error("wake word error: {0}")]
    WakeWord(#[from] wake_word::WakeWordError),
    #[error("STT error: {0}")]
    Stt(#[from] stt::SttError),
    #[error("TTS error: {0}")]
    Tts(#[from] tts::TtsError),
    #[error("biomarker error: {0}")]
    Biomarker(#[from] biomarkers::BiomarkerError),
    #[error("call error: {0}")]
    Call(#[from] call_handler::CallError),
    #[error("state transition error: {0}")]
    Transition(#[from] modality_state_machine::TransitionError),
    #[error("engine not running")]
    NotRunning,
    #[error("engine already running")]
    AlreadyRunning,
}

pub type VoiceResult<T> = Result<T, VoiceError>;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the VoiceEngine.
#[derive(Debug, Clone)]
pub struct VoiceConfig {
    /// Sample rate for capture/playback (default: 16000).
    pub sample_rate: u32,
    /// VAD speech probability threshold.
    pub vad_threshold: f32,
    /// Minimum speech duration (ms) for VAD.
    pub min_speech_ms: u32,
    /// Minimum silence duration (ms) to end utterance.
    pub min_silence_ms: u32,
    /// Wake word sensitivity.
    pub wake_word_sensitivity: f32,
    /// STT tier preference.
    pub stt_tier: SttTier,
    /// Whether to extract voice biomarkers.
    pub enable_biomarkers: bool,
    /// Personality scores for TTS.
    pub personality: OceanScores,
    /// Listen timeout in seconds.
    pub listen_timeout_secs: u64,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            sample_rate: audio_io::DEFAULT_SAMPLE_RATE,
            vad_threshold: vad::DEFAULT_THRESHOLD,
            min_speech_ms: vad::DEFAULT_MIN_SPEECH_MS,
            min_silence_ms: vad::DEFAULT_MIN_SILENCE_MS,
            wake_word_sensitivity: wake_word::DEFAULT_SENSITIVITY,
            stt_tier: SttTier::Streaming,
            enable_biomarkers: true,
            personality: OceanScores::default(),
            listen_timeout_secs: 15,
        }
    }
}

// ---------------------------------------------------------------------------
// VoiceEngine
// ---------------------------------------------------------------------------

/// Main voice engine facade. Owns and orchestrates all voice subsystems.
pub struct VoiceEngine {
    /// State machine governing voice modality.
    state_machine: ModalityStateMachine,
    /// Audio I/O (capture + playback).
    audio: AudioIo,
    /// Signal processing (denoise + AEC).
    signal_proc: SignalProcessor,
    /// Voice activity detection.
    vad: VoiceActivityDetector,
    /// Wake word detector.
    wake_word: WakeWordDetector,
    /// Speech-to-text (dual-tier).
    stt: SpeechToText,
    /// Text-to-speech (dual-tier).
    tts: TextToSpeech,
    /// Voice biomarker extractor.
    biomarkers: BiomarkerExtractor,
    /// Phone call handler.
    call_handler: CallHandler,
    /// Configuration.
    config: VoiceConfig,
    /// Whether the engine is running.
    running: bool,
    /// Audio buffer for accumulating speech during active listening.
    speech_buffer: Vec<i16>,
    /// Maximum speech buffer size (10 seconds at sample rate).
    max_speech_buffer: usize,
}

impl VoiceEngine {
    /// Create a new VoiceEngine with default configuration.
    pub fn new() -> VoiceResult<Self> {
        Self::with_config(VoiceConfig::default())
    }

    /// Create with explicit configuration.
    pub fn with_config(config: VoiceConfig) -> VoiceResult<Self> {
        let max_speech_buffer = config.sample_rate as usize * 10; // 10 seconds

        Ok(Self {
            state_machine: ModalityStateMachine::new(),
            audio: AudioIo::new(),
            signal_proc: SignalProcessor::new()?,
            vad: VoiceActivityDetector::with_params(
                config.vad_threshold,
                config.min_speech_ms,
                config.min_silence_ms,
            ),
            wake_word: WakeWordDetector::new(),
            stt: SpeechToText::new_dual()?,
            tts: TextToSpeech::new_dual()?,
            biomarkers: BiomarkerExtractor::with_default_rate(),
            call_handler: CallHandler::new(),
            config,
            running: false,
            speech_buffer: Vec::new(),
            max_speech_buffer,
        })
    }

    /// Start the voice engine: begins audio capture and wake word listening.
    pub async fn start(&mut self) -> VoiceResult<()> {
        if self.running {
            return Err(VoiceError::AlreadyRunning);
        }

        self.audio.start_capture()?;
        self.state_machine.transition(VoiceEvent::EnableVoice)?;
        self.running = true;

        Ok(())
    }

    /// Stop the voice engine.
    pub async fn stop(&mut self) -> VoiceResult<()> {
        if !self.running {
            return Err(VoiceError::NotRunning);
        }

        self.audio.stop_capture()?;
        self.audio.stop_playback()?;
        self.state_machine.transition(VoiceEvent::DisableVoice)?;
        self.running = false;
        self.speech_buffer.clear();

        Ok(())
    }

    /// Speak text with given priority. Synthesizes via TTS and plays audio.
    ///
    /// `mood_hint` is the LLM's raw valence signal from `ConversationReply`
    /// (a float in [-1.0, 1.0]). Pass `None` if no hint is available.
    /// Voice parameters come from LLM mood_hint, not from personality computation.
    pub async fn speak(
        &mut self,
        text: &str,
        priority: SpeechPriority,
        context: &SpeechContext,
        mood_hint: Option<f32>,
    ) -> VoiceResult<()> {
        // Apply LLM mood_hint to TTS params — OCEAN scores are NOT used here.
        let params = personality_voice::mood_to_tts_params(mood_hint, context);
        self.tts.set_params(params);

        // Synthesize
        let audio = self.tts.synthesize(text)?;

        // Start playback if not already running
        if !self.audio.is_playing() {
            self.audio.start_playback()?;
        }

        // Feed reference to AEC so it can cancel echo
        let float_samples: Vec<f32> = audio.samples.iter().map(|&s| s as f32 / 32768.0).collect();
        self.signal_proc.aec.feed_reference(&float_samples)?;
        self.signal_proc.aec.set_active(true);

        // Enqueue for playback
        self.audio.play_samples(&audio.samples)?;

        // Update state
        let mode = match priority {
            SpeechPriority::Critical | SpeechPriority::High => SpeakingMode::Notification,
            SpeechPriority::Normal => SpeakingMode::Response,
            SpeechPriority::Low => SpeakingMode::Proactive,
        };
        let _ = self
            .state_machine
            .transition(VoiceEvent::ResponseReady { mode });

        Ok(())
    }

    /// Process one audio frame through the full pipeline.
    /// Called repeatedly from the main voice processing loop.
    ///
    /// Returns transcribed text if a complete utterance was captured and
    /// processed, along with optional biomarkers.
    pub fn process_frame(&mut self, frame: &mut [i16]) -> VoiceResult<Option<ProcessedUtterance>> {
        if !self.running {
            return Err(VoiceError::NotRunning);
        }

        // Check timeouts
        if self.state_machine.check_timeout() {
            let _ = self.state_machine.transition(VoiceEvent::Timeout);
            self.speech_buffer.clear();
            self.stt.reset_streaming();
            return Ok(None);
        }

        // Convert to float for signal processing
        let mut float_frame: Vec<f32> = frame.iter().map(|&s| s as f32 / 32768.0).collect();

        // Signal processing (AEC + denoise)
        let _voice_prob = self.signal_proc.process_frame(&mut float_frame)?;

        // Convert back to i16
        for (i, &f) in float_frame.iter().enumerate() {
            if i < frame.len() {
                frame[i] = (f * 32768.0).clamp(-32768.0, 32767.0) as i16;
            }
        }

        match self.state_machine.state().clone() {
            ModalityState::WakeWordListening => {
                // Check wake word
                if let Some(_event) = self.wake_word.process_frame(frame) {
                    self.state_machine
                        .transition(VoiceEvent::WakeWordDetected)?;
                    self.speech_buffer.clear();
                    self.stt.reset_streaming();
                }
                Ok(None)
            },

            ModalityState::ActiveListening { .. } => {
                // Run VAD
                // Pad or truncate frame to VAD frame size
                let vad_frame = if frame.len() >= vad::VAD_FRAME_SAMPLES {
                    &frame[..vad::VAD_FRAME_SAMPLES]
                } else {
                    // Pad with zeros
                    let mut padded = vec![0i16; vad::VAD_FRAME_SAMPLES];
                    padded[..frame.len()].copy_from_slice(frame);
                    return self.process_vad_frame(&padded, frame);
                };

                self.process_vad_frame(vad_frame, frame)
            },

            ModalityState::Speaking { .. } => {
                // While speaking, still monitor for barge-in via wake word
                if let Some(_event) = self.wake_word.process_frame(frame) {
                    self.state_machine
                        .transition(VoiceEvent::WakeWordDetected)?;
                    self.audio.stop_playback()?;
                    self.signal_proc.aec.set_active(false);
                    self.speech_buffer.clear();
                    self.stt.reset_streaming();
                }
                Ok(None)
            },

            _ => Ok(None),
        }
    }

    /// Process a call event.
    pub async fn handle_call_event(&mut self, event: CallEvent) -> VoiceResult<()> {
        match &event {
            CallEvent::IncomingCall { .. } | CallEvent::OutgoingCall { .. } => {
                let call_state = match &event {
                    CallEvent::IncomingCall { caller } => CallState::Ringing {
                        caller: caller.clone(),
                        incoming: true,
                    },
                    CallEvent::OutgoingCall { callee } => CallState::Ringing {
                        caller: callee.clone(),
                        incoming: false,
                    },
                    _ => unreachable!(),
                };
                self.state_machine
                    .transition(VoiceEvent::CallStarted { call_state })?;
            },
            CallEvent::CallEnded => {
                self.state_machine.transition(VoiceEvent::CallEnded)?;
            },
            _ => {},
        }

        self.call_handler.on_call_event(event);
        Ok(())
    }

    /// Get current state.
    pub fn state(&self) -> &ModalityState {
        self.state_machine.state()
    }

    /// Check if the engine is running.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Get reference to call handler.
    pub fn call_handler(&self) -> &CallHandler {
        &self.call_handler
    }

    /// Get mutable reference to call handler.
    pub fn call_handler_mut(&mut self) -> &mut CallHandler {
        &mut self.call_handler
    }

    /// Update personality scores (affects TTS voice).
    pub fn set_personality(&mut self, ocean: OceanScores) {
        self.config.personality = ocean;
    }

    // -- Internal -------------------------------------------------------

    fn process_vad_frame(
        &mut self,
        vad_frame: &[i16],
        original_frame: &[i16],
    ) -> VoiceResult<Option<ProcessedUtterance>> {
        let vad_result = self.vad.process_frame(vad_frame)?;

        match vad_result {
            VadEvent::SpeechStarted | VadEvent::SpeechContinuing => {
                // Accumulate audio
                if self.speech_buffer.len() + original_frame.len() <= self.max_speech_buffer {
                    self.speech_buffer.extend_from_slice(original_frame);
                }

                // Feed to streaming STT
                let _ = self.stt.feed_audio(original_frame);

                Ok(None)
            },

            VadEvent::SpeechEnded { duration_ms } => {
                self.state_machine.transition(VoiceEvent::SpeechEnded)?;

                // Transcribe
                let text = self
                    .stt
                    .smart_transcribe(&self.speech_buffer, self.config.sample_rate)?;

                // Extract biomarkers if enabled
                let biomarkers =
                    if self.config.enable_biomarkers && self.speech_buffer.len() >= 1600 {
                        self.biomarkers.extract(&self.speech_buffer).ok()
                    } else {
                        None
                    };

                let emotional_signal = biomarkers.as_ref().map(|b| b.to_emotional_signal());

                // Clean up
                self.speech_buffer.clear();
                self.stt.reset_streaming();

                let _ = self
                    .state_machine
                    .transition(VoiceEvent::ProcessingComplete);

                Ok(Some(ProcessedUtterance {
                    text,
                    duration_ms,
                    biomarkers,
                    emotional_signal,
                }))
            },

            VadEvent::Silence => Ok(None),
        }
    }
}

impl Default for VoiceEngine {
    fn default() -> Self {
        Self::new().expect("VoiceEngine default initialization failed")
    }
}

// ---------------------------------------------------------------------------
// Processed utterance result
// ---------------------------------------------------------------------------

/// Result of processing a complete user utterance.
#[derive(Debug)]
pub struct ProcessedUtterance {
    /// Transcribed text.
    pub text: String,
    /// Duration of the speech in milliseconds.
    pub duration_ms: u32,
    /// Voice biomarkers (if extraction was enabled and audio was long enough).
    pub biomarkers: Option<VoiceBiomarkers>,
    /// Emotional signal derived from biomarkers.
    pub emotional_signal: Option<EmotionalSignal>,
}

// ---------------------------------------------------------------------------
// Memory budget summary
// ---------------------------------------------------------------------------

/// Returns the estimated memory usage of the voice pipeline in bytes.
pub fn estimated_memory_usage() -> MemoryBudget {
    MemoryBudget {
        audio_buffers_bytes: audio_io::MAX_BUFFER_SAMPLES * 2 * 2, // input + output, i16
        rnnoise_bytes: 200_000,                                    // ~200 KB
        silero_vad_bytes: 2_000_000,                               // ~2 MB model
        wake_word_bytes: 5_000_000,                                // ~5 MB
        zipformer_bytes: 30_000_000,                               // ~30 MB
        whisper_bytes: 75_000_000,                                 // ~75 MB (tiny)
        piper_bytes: 30_000_000,                                   // ~30 MB
        espeak_bytes: 1_000_000,                                   // ~1 MB
        speech_buffer_bytes: 160_000 * 2,                          // 10s at 16kHz, i16
        total_bytes: 0,                                            // computed below
    }
}

#[derive(Debug)]
pub struct MemoryBudget {
    pub audio_buffers_bytes: usize,
    pub rnnoise_bytes: usize,
    pub silero_vad_bytes: usize,
    pub wake_word_bytes: usize,
    pub zipformer_bytes: usize,
    pub whisper_bytes: usize,
    pub piper_bytes: usize,
    pub espeak_bytes: usize,
    pub speech_buffer_bytes: usize,
    pub total_bytes: usize,
}

impl MemoryBudget {
    pub fn total(&self) -> usize {
        self.audio_buffers_bytes
            + self.rnnoise_bytes
            + self.silero_vad_bytes
            + self.wake_word_bytes
            + self.zipformer_bytes
            + self.whisper_bytes
            + self.piper_bytes
            + self.espeak_bytes
            + self.speech_buffer_bytes
    }

    pub fn total_mb(&self) -> f32 {
        self.total() as f32 / (1024.0 * 1024.0)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_creation() {
        let engine = VoiceEngine::new().unwrap();
        assert!(!engine.is_running());
        assert!(matches!(engine.state(), ModalityState::Idle));
    }

    #[tokio::test]
    async fn engine_start_stop() {
        let mut engine = VoiceEngine::new().unwrap();
        engine.start().await.unwrap();
        assert!(engine.is_running());
        assert!(matches!(engine.state(), ModalityState::WakeWordListening));

        engine.stop().await.unwrap();
        assert!(!engine.is_running());
        assert!(matches!(engine.state(), ModalityState::Idle));
    }

    #[tokio::test]
    async fn double_start_errors() {
        let mut engine = VoiceEngine::new().unwrap();
        engine.start().await.unwrap();
        assert!(engine.start().await.is_err());
    }

    #[tokio::test]
    async fn stop_when_not_running_errors() {
        let mut engine = VoiceEngine::new().unwrap();
        assert!(engine.stop().await.is_err());
    }

    #[test]
    fn memory_budget_under_limit() {
        let budget = estimated_memory_usage();
        let total = budget.total_mb();
        assert!(
            total < 232.0,
            "total memory {total:.1} MB exceeds 232 MB budget"
        );
    }

    #[test]
    fn process_frame_requires_running() {
        let mut engine = VoiceEngine::new().unwrap();
        let mut frame = vec![0i16; 160];
        assert!(engine.process_frame(&mut frame).is_err());
    }

    #[tokio::test]
    async fn call_event_handling() {
        let mut engine = VoiceEngine::new().unwrap();
        engine.start().await.unwrap();

        engine
            .handle_call_event(CallEvent::IncomingCall {
                caller: Some("Test".into()),
            })
            .await
            .unwrap();
        assert!(matches!(engine.state(), ModalityState::InCall { .. }));

        engine
            .handle_call_event(CallEvent::CallEnded)
            .await
            .unwrap();
        assert!(matches!(engine.state(), ModalityState::WakeWordListening));
    }

    #[test]
    fn personality_update() {
        let mut engine = VoiceEngine::new().unwrap();
        engine.set_personality(OceanScores {
            extraversion: 1.0,
            ..OceanScores::default()
        });
        assert!((engine.config.personality.extraversion - 1.0).abs() < f32::EPSILON);
    }
}
