//! Phone call handling via AccessibilityService screen taps.
//!
//! AURA does NOT use the telephony API. Instead, it interacts with the phone
//! app's UI via Android's AccessibilityService — finding and clicking buttons
//! like "Answer," "Reject," "End Call," "Mute," and "Speaker."

use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("no active call")]
    NoActiveCall,
    #[error("call already active")]
    CallAlreadyActive,
    #[error("accessibility action failed: {0}")]
    A11yActionFailed(String),
    #[error("UI element not found: {0}")]
    UiElementNotFound(String),
    #[error("invalid state transition: {from:?} → {to:?}")]
    InvalidTransition { from: CallState, to: CallState },
    #[error("call timeout")]
    Timeout,
}

pub type CallResult<T> = Result<T, CallError>;

// ---------------------------------------------------------------------------
// Call state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum CallState {
    /// No call in progress.
    Idle,
    /// Phone is ringing.
    Ringing {
        caller: Option<String>,
        incoming: bool,
    },
    /// Call is active (connected).
    Active {
        caller: Option<String>,
        connected_at: Instant,
        muted: bool,
        speaker: bool,
    },
    /// Call is on hold.
    OnHold {
        caller: Option<String>,
        hold_start: Instant,
    },
}

impl CallState {
    /// Get the caller name/number if available.
    pub fn caller(&self) -> Option<&str> {
        match self {
            CallState::Idle => None,
            CallState::Ringing { caller, .. } => caller.as_deref(),
            CallState::Active { caller, .. } => caller.as_deref(),
            CallState::OnHold { caller, .. } => caller.as_deref(),
        }
    }

    /// Check if a call is active or on hold.
    pub fn is_in_call(&self) -> bool {
        matches!(self, CallState::Active { .. } | CallState::OnHold { .. } | CallState::Ringing { .. })
    }

    /// Duration of the active call.
    pub fn duration(&self) -> Option<Duration> {
        match self {
            CallState::Active { connected_at, .. } => Some(connected_at.elapsed()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Call events (from system observers)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum CallEvent {
    /// Incoming call detected (from A11Y notification).
    IncomingCall { caller: Option<String> },
    /// Outgoing call initiated.
    OutgoingCall { callee: Option<String> },
    /// Call connected.
    CallConnected,
    /// Call ended (by either party).
    CallEnded,
    /// Call put on hold.
    CallHeld,
    /// Call resumed from hold.
    CallResumed,
}

// ---------------------------------------------------------------------------
// A11Y action interface (Android only)
// ---------------------------------------------------------------------------

/// Abstraction over AccessibilityService actions for call UI interaction.
#[cfg(target_os = "android")]
mod a11y_actions {
    use super::CallResult;

    extern "C" {
        /// Find a UI node by text/description and click it.
        /// Returns 0 on success, -1 if not found.
        fn a11y_click_by_text(text: *const std::os::raw::c_char) -> std::os::raw::c_int;

        /// Find a UI node by resource ID and click it.
        fn a11y_click_by_id(resource_id: *const std::os::raw::c_char) -> std::os::raw::c_int;
    }

    pub fn click_button(text: &str) -> CallResult<()> {
        use std::ffi::CString;
        let c_text = CString::new(text).unwrap();
        let result = unsafe { a11y_click_by_text(c_text.as_ptr()) };
        if result == 0 {
            Ok(())
        } else {
            Err(super::CallError::UiElementNotFound(text.to_string()))
        }
    }

    pub fn click_by_id(resource_id: &str) -> CallResult<()> {
        use std::ffi::CString;
        let c_id = CString::new(resource_id).unwrap();
        let result = unsafe { a11y_click_by_id(c_id.as_ptr()) };
        if result == 0 {
            Ok(())
        } else {
            Err(super::CallError::UiElementNotFound(resource_id.to_string()))
        }
    }
}

/// Mock A11Y actions for testing.
#[cfg(not(target_os = "android"))]
mod a11y_actions {
    use super::CallResult;

    pub fn click_button(_text: &str) -> CallResult<()> {
        // Mock: always succeeds
        Ok(())
    }

    // Phase 8 wire point: click_by_id used by accessibility-service call UI
    // automation once Accessibility API is wired in Android boot path.
    #[allow(dead_code)]
    pub fn click_by_id(_resource_id: &str) -> CallResult<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Known UI button labels (varies by phone app)
// ---------------------------------------------------------------------------

/// Known answer button labels across phone apps.
const ANSWER_LABELS: &[&str] = &["Answer", "Accept", "answer", "accept"];

/// Known reject button labels.
const REJECT_LABELS: &[&str] = &["Decline", "Reject", "decline", "reject", "Dismiss"];

/// Known end-call button labels.
const END_CALL_LABELS: &[&str] = &["End call", "Hang up", "end call", "hang up", "End"];

/// Known mute button labels.
const MUTE_LABELS: &[&str] = &["Mute", "mute", "Unmute", "unmute"];

/// Known speaker button labels.
const SPEAKER_LABELS: &[&str] = &["Speaker", "speaker", "Speakerphone"];

// ---------------------------------------------------------------------------
// CallHandler
// ---------------------------------------------------------------------------

pub struct CallHandler {
    state: CallState,
}

impl CallHandler {
    pub fn new() -> Self {
        Self {
            state: CallState::Idle,
        }
    }

    /// Get current call state.
    pub fn state(&self) -> &CallState {
        &self.state
    }

    /// Answer an incoming call by tapping the Answer button via A11Y.
    pub async fn answer_call(&mut self) -> CallResult<()> {
        match &self.state {
            CallState::Ringing { incoming: true, caller, .. } => {
                let caller = caller.clone();
                Self::try_click_labels(ANSWER_LABELS).await?;
                self.state = CallState::Active {
                    caller,
                    connected_at: Instant::now(),
                    muted: false,
                    speaker: false,
                };
                Ok(())
            }
            CallState::Ringing { incoming: false, .. } => {
                Err(CallError::A11yActionFailed("cannot answer outgoing call".into()))
            }
            _ => Err(CallError::NoActiveCall),
        }
    }

    /// Reject an incoming call.
    pub async fn reject_call(&mut self) -> CallResult<()> {
        match &self.state {
            CallState::Ringing { .. } => {
                Self::try_click_labels(REJECT_LABELS).await?;
                self.state = CallState::Idle;
                Ok(())
            }
            _ => Err(CallError::NoActiveCall),
        }
    }

    /// End an active call.
    pub async fn end_call(&mut self) -> CallResult<()> {
        match &self.state {
            CallState::Active { .. } | CallState::OnHold { .. } => {
                Self::try_click_labels(END_CALL_LABELS).await?;
                self.state = CallState::Idle;
                Ok(())
            }
            _ => Err(CallError::NoActiveCall),
        }
    }

    /// Toggle mute on active call.
    pub async fn toggle_mute(&mut self) -> CallResult<()> {
        match &mut self.state {
            CallState::Active { muted, .. } => {
                Self::try_click_labels(MUTE_LABELS).await?;
                *muted = !*muted;
                Ok(())
            }
            _ => Err(CallError::NoActiveCall),
        }
    }

    /// Toggle speaker on active call.
    pub async fn toggle_speaker(&mut self) -> CallResult<()> {
        match &mut self.state {
            CallState::Active { speaker, .. } => {
                Self::try_click_labels(SPEAKER_LABELS).await?;
                *speaker = !*speaker;
                Ok(())
            }
            _ => Err(CallError::NoActiveCall),
        }
    }

    /// Handle a call event from the system.
    pub fn on_call_event(&mut self, event: CallEvent) {
        match event {
            CallEvent::IncomingCall { caller } => {
                self.state = CallState::Ringing {
                    caller,
                    incoming: true,
                };
            }
            CallEvent::OutgoingCall { callee } => {
                self.state = CallState::Ringing {
                    caller: callee,
                    incoming: false,
                };
            }
            CallEvent::CallConnected => {
                let caller = self.state.caller().map(String::from);
                self.state = CallState::Active {
                    caller,
                    connected_at: Instant::now(),
                    muted: false,
                    speaker: false,
                };
            }
            CallEvent::CallEnded => {
                self.state = CallState::Idle;
            }
            CallEvent::CallHeld => {
                let caller = self.state.caller().map(String::from);
                self.state = CallState::OnHold {
                    caller,
                    hold_start: Instant::now(),
                };
            }
            CallEvent::CallResumed => {
                let caller = self.state.caller().map(String::from);
                self.state = CallState::Active {
                    caller,
                    connected_at: Instant::now(),
                    muted: false,
                    speaker: false,
                };
            }
        }
    }

    /// Is a call in progress?
    pub fn is_in_call(&self) -> bool {
        self.state.is_in_call()
    }

    // -- Internal -------------------------------------------------------

    /// Try clicking any of the given labels. Returns Ok on first success.
    async fn try_click_labels(labels: &[&str]) -> CallResult<()> {
        for label in labels {
            if a11y_actions::click_button(label).is_ok() {
                return Ok(());
            }
        }
        Err(CallError::UiElementNotFound(
            labels.first().unwrap_or(&"unknown").to_string(),
        ))
    }
}

impl Default for CallHandler {
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

    #[tokio::test]
    async fn answer_incoming_call() {
        let mut handler = CallHandler::new();
        handler.on_call_event(CallEvent::IncomingCall {
            caller: Some("Alice".into()),
        });

        assert!(handler.is_in_call());
        handler.answer_call().await.unwrap();

        match handler.state() {
            CallState::Active { caller, muted, speaker, .. } => {
                assert_eq!(caller.as_deref(), Some("Alice"));
                assert!(!muted);
                assert!(!speaker);
            }
            other => panic!("expected Active, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reject_incoming_call() {
        let mut handler = CallHandler::new();
        handler.on_call_event(CallEvent::IncomingCall { caller: None });
        handler.reject_call().await.unwrap();
        assert_eq!(*handler.state(), CallState::Idle);
    }

    #[tokio::test]
    async fn end_active_call() {
        let mut handler = CallHandler::new();
        handler.on_call_event(CallEvent::IncomingCall {
            caller: Some("Bob".into()),
        });
        handler.answer_call().await.unwrap();
        handler.end_call().await.unwrap();
        assert_eq!(*handler.state(), CallState::Idle);
    }

    #[tokio::test]
    async fn cannot_answer_when_idle() {
        let mut handler = CallHandler::new();
        assert!(handler.answer_call().await.is_err());
    }

    #[tokio::test]
    async fn toggle_mute_and_speaker() {
        let mut handler = CallHandler::new();
        handler.on_call_event(CallEvent::IncomingCall { caller: None });
        handler.answer_call().await.unwrap();

        handler.toggle_mute().await.unwrap();
        if let CallState::Active { muted, .. } = handler.state() {
            assert!(*muted);
        }

        handler.toggle_speaker().await.unwrap();
        if let CallState::Active { speaker, .. } = handler.state() {
            assert!(*speaker);
        }
    }

    #[test]
    fn call_event_lifecycle() {
        let mut handler = CallHandler::new();

        handler.on_call_event(CallEvent::OutgoingCall {
            callee: Some("Charlie".into()),
        });
        assert!(matches!(handler.state(), CallState::Ringing { incoming: false, .. }));

        handler.on_call_event(CallEvent::CallConnected);
        assert!(matches!(handler.state(), CallState::Active { .. }));

        handler.on_call_event(CallEvent::CallHeld);
        assert!(matches!(handler.state(), CallState::OnHold { .. }));

        handler.on_call_event(CallEvent::CallResumed);
        assert!(matches!(handler.state(), CallState::Active { .. }));

        handler.on_call_event(CallEvent::CallEnded);
        assert_eq!(*handler.state(), CallState::Idle);
    }
}
