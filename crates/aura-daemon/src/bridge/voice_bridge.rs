//! Voice ↔ Daemon bridge.
//!
//! Runs the [`VoiceEngine`] processing loop, converts each completed
//! [`ProcessedUtterance`] into a [`UserCommand::Chat`] on the daemon's
//! command channel, and delivers [`DaemonResponse`] text back to the
//! voice engine's TTS for spoken output.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, error, info, warn};

use crate::bridge::{BridgeError, BridgeResult, InputChannel};
use crate::daemon_core::channels::{
    DaemonResponseRx, InputSource, UserCommand, UserCommandTx, VoiceMetadata,
};
use crate::voice::{
    ProcessedUtterance, SpeechContext, SpeechPriority, VoiceConfig, VoiceEngine, VoiceError,
};

// ---------------------------------------------------------------------------
// VoiceBridge
// ---------------------------------------------------------------------------

/// Bridge between the voice engine and the daemon pipeline.
///
/// In the `run` loop the bridge:
///
/// 1. Reads audio frames from the engine (simulated via `process_frame`).
/// 2. Converts completed utterances to [`UserCommand::Chat`].
/// 3. Monitors the response channel for text that should be spoken via TTS.
pub struct VoiceBridge {
    /// The voice engine instance.
    engine: VoiceEngine,
    /// Shared cancellation flag.
    cancel: Arc<AtomicBool>,
    /// Frame size in samples (one VAD frame).
    frame_size: usize,
}

impl VoiceBridge {
    /// Create a new voice bridge wrapping the given engine.
    pub fn new(engine: VoiceEngine, cancel: Arc<AtomicBool>) -> Self {
        // One VAD frame = 10 ms at the engine's sample rate.
        let frame_size = 160; // 16 kHz × 0.01 s
        Self {
            engine,
            cancel,
            frame_size,
        }
    }

    /// Create with explicit configuration.
    pub fn with_config(config: VoiceConfig, cancel: Arc<AtomicBool>) -> BridgeResult<Self> {
        let engine =
            VoiceEngine::with_config(config).map_err(|e| BridgeError::Upstream(e.to_string()))?;
        Ok(Self::new(engine, cancel))
    }

    /// Convert a [`ProcessedUtterance`] into a [`UserCommand::Chat`].
    fn utterance_to_command(utterance: &ProcessedUtterance) -> UserCommand {
        let emotional_valence = utterance
            .emotional_signal
            .as_ref()
            .map(|s| s.valence);
        let emotional_arousal = utterance
            .emotional_signal
            .as_ref()
            .map(|s| s.arousal);

        UserCommand::Chat {
            text: utterance.text.clone(),
            source: InputSource::Voice,
            voice_meta: Some(VoiceMetadata {
                duration_ms: utterance.duration_ms,
                emotional_valence,
                emotional_arousal,
            }),
        }
    }

    /// Speak a response via TTS.
    async fn speak_response(&mut self, text: &str) -> BridgeResult<()> {
        let ctx = SpeechContext::default();
        self.engine
            .speak(text, SpeechPriority::Normal, &ctx)
            .await
            .map_err(|e| BridgeError::Upstream(e.to_string()))
    }
}

#[async_trait]
impl InputChannel for VoiceBridge {
    fn name(&self) -> &str {
        "voice"
    }

    fn source(&self) -> InputSource {
        InputSource::Voice
    }

    async fn run(
        &mut self,
        cmd_tx: UserCommandTx,
        mut response_rx: DaemonResponseRx,
    ) -> BridgeResult<()> {
        info!("voice bridge starting");

        // Start the engine.
        self.engine
            .start()
            .await
            .map_err(|e| BridgeError::Upstream(e.to_string()))?;

        loop {
            if self.cancel.load(Ordering::Relaxed) {
                info!("voice bridge shutting down (cancel flag)");
                break;
            }

            // Check for responses to speak (non-blocking).
            match response_rx.try_recv() {
                Ok(response) => {
                    if response.destination == InputSource::Voice {
                        debug!(len = response.text.len(), "speaking daemon response");
                        if let Err(e) = self.speak_response(&response.text).await {
                            warn!(error = %e, "TTS delivery failed");
                        }
                    }
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    info!("response channel closed — voice bridge exiting");
                    break;
                }
            }

            // Process one audio frame.
            let mut frame = vec![0i16; self.frame_size];

            // Read from audio I/O (in production, this blocks until a frame
            // is available; here we yield to the runtime).
            tokio::task::yield_now().await;

            match self.engine.process_frame(&mut frame) {
                Ok(Some(utterance)) => {
                    info!(
                        text_len = utterance.text.len(),
                        duration_ms = utterance.duration_ms,
                        "voice utterance captured"
                    );

                    let cmd = Self::utterance_to_command(&utterance);
                    if cmd_tx.send(cmd).await.is_err() {
                        error!("command channel closed — voice bridge exiting");
                        break;
                    }
                }
                Ok(None) => {
                    // No complete utterance yet — continue listening.
                }
                Err(VoiceError::NotRunning) => {
                    warn!("voice engine stopped unexpectedly");
                    break;
                }
                Err(e) => {
                    warn!(error = %e, "voice frame processing error — continuing");
                }
            }
        }

        // Stop engine gracefully.
        let _ = self.engine.stop().await;
        info!("voice bridge stopped");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_core::channels::DaemonResponse;
    use crate::voice::biomarkers::EmotionalSignal;
    use tokio::sync::mpsc;

    #[test]
    fn test_utterance_to_command_with_biomarkers() {
        let utterance = ProcessedUtterance {
            text: "hello AURA".into(),
            duration_ms: 1500,
            biomarkers: None,
            emotional_signal: Some(EmotionalSignal {
                valence: 0.6,
                arousal: 0.4,
                stress: 0.3,
                fatigue: 0.1,
                confidence: 0.8,
            }),
        };

        let cmd = VoiceBridge::utterance_to_command(&utterance);
        match cmd {
            UserCommand::Chat {
                text,
                source,
                voice_meta,
            } => {
                assert_eq!(text, "hello AURA");
                assert_eq!(source, InputSource::Voice);
                let meta = voice_meta.expect("should have voice metadata");
                assert_eq!(meta.duration_ms, 1500);
                assert!((meta.emotional_valence.unwrap() - 0.6).abs() < f32::EPSILON);
                assert!((meta.emotional_arousal.unwrap() - 0.4).abs() < f32::EPSILON);
            }
            _ => panic!("expected Chat variant"),
        }
    }

    #[test]
    fn test_utterance_to_command_without_biomarkers() {
        let utterance = ProcessedUtterance {
            text: "set a timer".into(),
            duration_ms: 800,
            biomarkers: None,
            emotional_signal: None,
        };

        let cmd = VoiceBridge::utterance_to_command(&utterance);
        match cmd {
            UserCommand::Chat { voice_meta, .. } => {
                let meta = voice_meta.expect("should have voice metadata");
                assert!(meta.emotional_valence.is_none());
                assert!(meta.emotional_arousal.is_none());
            }
            _ => panic!("expected Chat variant"),
        }
    }

    #[tokio::test]
    async fn test_voice_bridge_cancel_flag() {
        let cancel = Arc::new(AtomicBool::new(true)); // Pre-set cancel.
        let engine = VoiceEngine::new().expect("engine creation");
        let mut bridge = VoiceBridge::new(engine, cancel);

        let (cmd_tx, _cmd_rx) = mpsc::channel(16);
        let (_resp_tx, resp_rx) = mpsc::channel::<DaemonResponse>(16);

        // Should exit immediately due to cancel flag.
        let result = bridge.run(cmd_tx, resp_rx).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_voice_bridge_name_and_source() {
        let cancel = Arc::new(AtomicBool::new(false));
        let engine = VoiceEngine::new().expect("engine creation");
        let bridge = VoiceBridge::new(engine, cancel);

        assert_eq!(bridge.name(), "voice");
        assert_eq!(bridge.source(), InputSource::Voice);
    }
}
