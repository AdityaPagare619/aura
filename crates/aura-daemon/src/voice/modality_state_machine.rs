//! Voice modality state machine.
//!
//! Governs transitions between voice pipeline states:
//! Idle → WakeWordListening → ActiveListening → Processing → Speaking → (loop)
//! Any state can transition to InCall on phone events.

use std::time::{Duration, Instant};

use super::call_handler::CallState;

// ---------------------------------------------------------------------------
// State definitions
// ---------------------------------------------------------------------------

/// Top-level modality state for the voice engine.
#[derive(Debug, Clone, PartialEq)]
pub enum ModalityState {
    /// Voice engine is off or suspended.
    Idle,
    /// Waiting for wake word ("Hey AURA").
    WakeWordListening,
    /// Recording user speech after wake word.
    ActiveListening {
        started_at: Instant,
        /// Max listen duration before auto-finalize.
        timeout: Duration,
    },
    /// STT is processing recorded audio.
    Processing { started_at: Instant },
    /// TTS is speaking a response.
    Speaking {
        mode: SpeakingMode,
        started_at: Instant,
    },
    /// Phone call is active — voice engine hands off to call handler.
    InCall {
        call_state: CallState,
        /// State to return to after call ends.
        previous_state: Box<ModalityState>,
    },
}

/// What kind of speech is being output.
#[derive(Debug, Clone, PartialEq)]
pub enum SpeakingMode {
    /// Normal conversational response.
    Response,
    /// Notification read-out.
    Notification,
    /// Proactive insight from AURA.
    Proactive,
}

// ---------------------------------------------------------------------------
// Transition events
// ---------------------------------------------------------------------------

/// Events that trigger state transitions.
#[derive(Debug, Clone)]
pub enum VoiceEvent {
    /// User enabled voice / AURA decided to listen.
    EnableVoice,
    /// User disabled voice.
    DisableVoice,
    /// Wake word detected.
    WakeWordDetected,
    /// User started speaking (VAD speech start).
    SpeechStarted,
    /// User stopped speaking (VAD speech end).
    SpeechEnded,
    /// STT processing complete.
    ProcessingComplete,
    /// TTS response ready to speak.
    ResponseReady { mode: SpeakingMode },
    /// TTS finished speaking.
    SpeakingComplete,
    /// Incoming/outgoing phone call.
    CallStarted { call_state: CallState },
    /// Phone call ended.
    CallEnded,
    /// Timeout expired.
    Timeout,
}

// ---------------------------------------------------------------------------
// Transition errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum TransitionError {
    #[error("invalid transition: {event:?} in state {state}")]
    Invalid { state: String, event: String },
}

// ---------------------------------------------------------------------------
// ModalityStateMachine
// ---------------------------------------------------------------------------

pub struct ModalityStateMachine {
    state: ModalityState,
    /// Default listen timeout.
    listen_timeout: Duration,
    /// History of state transitions for debugging.
    history: Vec<(Instant, ModalityState)>,
    /// Max history entries.
    max_history: usize,
}

impl ModalityStateMachine {
    pub fn new() -> Self {
        Self {
            state: ModalityState::Idle,
            listen_timeout: Duration::from_secs(15),
            history: Vec::new(),
            max_history: 50,
        }
    }

    /// Get current state.
    pub fn state(&self) -> &ModalityState {
        &self.state
    }

    /// Set the listen timeout.
    pub fn set_listen_timeout(&mut self, timeout: Duration) {
        self.listen_timeout = timeout;
    }

    /// Process an event and transition state. Returns the new state.
    pub fn transition(&mut self, event: VoiceEvent) -> Result<&ModalityState, TransitionError> {
        let new_state = self.compute_transition(&event)?;
        self.record_transition();
        self.state = new_state;
        Ok(&self.state)
    }

    /// Check if the current state has timed out.
    pub fn check_timeout(&self) -> bool {
        match &self.state {
            ModalityState::ActiveListening {
                started_at,
                timeout,
            } => started_at.elapsed() >= *timeout,
            ModalityState::Processing { started_at } => {
                started_at.elapsed() >= Duration::from_secs(30) // 30s processing timeout
            },
            _ => false,
        }
    }

    /// Is AURA currently listening for user input?
    pub fn is_listening(&self) -> bool {
        matches!(
            self.state,
            ModalityState::WakeWordListening | ModalityState::ActiveListening { .. }
        )
    }

    /// Is AURA currently speaking?
    pub fn is_speaking(&self) -> bool {
        matches!(self.state, ModalityState::Speaking { .. })
    }

    /// Is a call active?
    pub fn is_in_call(&self) -> bool {
        matches!(self.state, ModalityState::InCall { .. })
    }

    /// Get state transition history.
    pub fn history(&self) -> &[(Instant, ModalityState)] {
        &self.history
    }

    // -- Internal -------------------------------------------------------

    fn compute_transition(&self, event: &VoiceEvent) -> Result<ModalityState, TransitionError> {
        // Call events override any state
        if let VoiceEvent::CallStarted { call_state } = event {
            return Ok(ModalityState::InCall {
                call_state: call_state.clone(),
                previous_state: Box::new(self.state.clone()),
            });
        }

        match (&self.state, event) {
            // Idle transitions
            (ModalityState::Idle, VoiceEvent::EnableVoice) => Ok(ModalityState::WakeWordListening),

            // WakeWordListening transitions
            (ModalityState::WakeWordListening, VoiceEvent::WakeWordDetected) => {
                Ok(ModalityState::ActiveListening {
                    started_at: Instant::now(),
                    timeout: self.listen_timeout,
                })
            },
            (ModalityState::WakeWordListening, VoiceEvent::DisableVoice) => Ok(ModalityState::Idle),

            // ActiveListening transitions
            (ModalityState::ActiveListening { .. }, VoiceEvent::SpeechEnded) => {
                Ok(ModalityState::Processing {
                    started_at: Instant::now(),
                })
            },
            (ModalityState::ActiveListening { .. }, VoiceEvent::Timeout) => {
                Ok(ModalityState::WakeWordListening)
            },
            (ModalityState::ActiveListening { .. }, VoiceEvent::DisableVoice) => {
                Ok(ModalityState::Idle)
            },

            // Processing transitions
            (ModalityState::Processing { .. }, VoiceEvent::ResponseReady { mode }) => {
                Ok(ModalityState::Speaking {
                    mode: mode.clone(),
                    started_at: Instant::now(),
                })
            },
            (ModalityState::Processing { .. }, VoiceEvent::ProcessingComplete) => {
                // Processing done but no response to speak → go back to listening
                Ok(ModalityState::WakeWordListening)
            },
            (ModalityState::Processing { .. }, VoiceEvent::Timeout) => {
                Ok(ModalityState::WakeWordListening)
            },

            // Speaking transitions
            (ModalityState::Speaking { .. }, VoiceEvent::SpeakingComplete) => {
                Ok(ModalityState::WakeWordListening)
            },
            // Barge-in: user speaks while AURA is speaking
            (ModalityState::Speaking { .. }, VoiceEvent::WakeWordDetected) => {
                Ok(ModalityState::ActiveListening {
                    started_at: Instant::now(),
                    timeout: self.listen_timeout,
                })
            },

            // InCall transitions
            (ModalityState::InCall { previous_state, .. }, VoiceEvent::CallEnded) => {
                // Return to previous state, or WakeWordListening if previous was Idle
                let prev = previous_state.as_ref().clone();
                match prev {
                    ModalityState::Idle => Ok(ModalityState::WakeWordListening),
                    other => Ok(other),
                }
            },

            // Any state → Idle on disable
            (_, VoiceEvent::DisableVoice) => Ok(ModalityState::Idle),

            // Invalid
            (state, event) => Err(TransitionError::Invalid {
                state: format!("{state:?}"),
                event: format!("{event:?}"),
            }),
        }
    }

    fn record_transition(&mut self) {
        if self.history.len() >= self.max_history {
            self.history.remove(0);
        }
        self.history.push((Instant::now(), self.state.clone()));
    }
}

impl Default for ModalityStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_happy_path() {
        let mut sm = ModalityStateMachine::new();

        // Idle → WakeWordListening
        sm.transition(VoiceEvent::EnableVoice).unwrap();
        assert!(matches!(sm.state(), ModalityState::WakeWordListening));

        // WakeWordListening → ActiveListening
        sm.transition(VoiceEvent::WakeWordDetected).unwrap();
        assert!(matches!(sm.state(), ModalityState::ActiveListening { .. }));

        // ActiveListening → Processing
        sm.transition(VoiceEvent::SpeechEnded).unwrap();
        assert!(matches!(sm.state(), ModalityState::Processing { .. }));

        // Processing → Speaking
        sm.transition(VoiceEvent::ResponseReady {
            mode: SpeakingMode::Response,
        })
        .unwrap();
        assert!(matches!(sm.state(), ModalityState::Speaking { .. }));

        // Speaking → WakeWordListening
        sm.transition(VoiceEvent::SpeakingComplete).unwrap();
        assert!(matches!(sm.state(), ModalityState::WakeWordListening));
    }

    #[test]
    fn call_interrupts_and_restores() {
        let mut sm = ModalityStateMachine::new();
        sm.transition(VoiceEvent::EnableVoice).unwrap();

        // Call arrives while listening
        sm.transition(VoiceEvent::CallStarted {
            call_state: CallState::Ringing {
                caller: Some("Alice".into()),
                incoming: true,
            },
        })
        .unwrap();
        assert!(sm.is_in_call());

        // Call ends → should restore WakeWordListening
        sm.transition(VoiceEvent::CallEnded).unwrap();
        assert!(matches!(sm.state(), ModalityState::WakeWordListening));
    }

    #[test]
    fn disable_from_any_state() {
        let mut sm = ModalityStateMachine::new();
        sm.transition(VoiceEvent::EnableVoice).unwrap();
        sm.transition(VoiceEvent::WakeWordDetected).unwrap();
        // In ActiveListening → disable
        sm.transition(VoiceEvent::DisableVoice).unwrap();
        assert!(matches!(sm.state(), ModalityState::Idle));
    }

    #[test]
    fn barge_in_during_speaking() {
        let mut sm = ModalityStateMachine::new();
        sm.transition(VoiceEvent::EnableVoice).unwrap();
        sm.transition(VoiceEvent::WakeWordDetected).unwrap();
        sm.transition(VoiceEvent::SpeechEnded).unwrap();
        sm.transition(VoiceEvent::ResponseReady {
            mode: SpeakingMode::Response,
        })
        .unwrap();

        // Barge-in: wake word while speaking
        sm.transition(VoiceEvent::WakeWordDetected).unwrap();
        assert!(matches!(sm.state(), ModalityState::ActiveListening { .. }));
    }

    #[test]
    fn listen_timeout() {
        let mut sm = ModalityStateMachine::new();
        sm.set_listen_timeout(Duration::from_millis(1));
        sm.transition(VoiceEvent::EnableVoice).unwrap();
        sm.transition(VoiceEvent::WakeWordDetected).unwrap();

        // Simulate timeout
        sm.transition(VoiceEvent::Timeout).unwrap();
        assert!(matches!(sm.state(), ModalityState::WakeWordListening));
    }

    #[test]
    fn invalid_transition_rejected() {
        let mut sm = ModalityStateMachine::new();
        // Can't detect wake word when Idle (must enable first)
        assert!(sm.transition(VoiceEvent::WakeWordDetected).is_err());
    }

    #[test]
    fn history_recording() {
        let mut sm = ModalityStateMachine::new();
        sm.transition(VoiceEvent::EnableVoice).unwrap();
        sm.transition(VoiceEvent::WakeWordDetected).unwrap();
        assert_eq!(sm.history().len(), 2);
    }
}
