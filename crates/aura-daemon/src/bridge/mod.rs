//! Bridge layer — connects external input/output channels to the daemon pipeline.
//!
//! Every input channel (voice, Telegram, direct JNI) implements the
//! [`InputChannel`] trait, which normalises heterogeneous input into
//! [`UserCommand`] messages on the daemon's command channel and routes
//! [`DaemonResponse`] messages back to the originating channel.
//!
//! # Architecture
//!
//! ```text
//!  Voice ──┐                    ┌── Voice TTS
//!          ├─► UserCommand ─►  daemon  ─► DaemonResponse ──┤
//! Telegram ┘   (mpsc::tx)      pipeline    (mpsc::tx)      └── Telegram queue
//! ```
//!
//! Each bridge runs as a spawned `tokio` task. The [`BridgeHandle`] returned
//! by [`InputChannel::spawn`] allows the caller to monitor bridge health.

pub mod router;
pub mod system_api;
pub mod telegram_bridge;
pub mod voice_bridge;

use async_trait::async_trait;

use crate::daemon_core::channels::{
    DaemonResponseRx, InputSource, UserCommandTx,
};
use aura_types::errors::AuraError;

// ---------------------------------------------------------------------------
// Bridge error
// ---------------------------------------------------------------------------

/// Errors that can occur during bridge operation.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// The command channel (bridge → daemon) has been closed.
    #[error("command channel closed")]
    CommandChannelClosed,

    /// The response channel (daemon → bridge) has been closed.
    #[error("response channel closed")]
    ResponseChannelClosed,

    /// The upstream engine (voice / telegram) returned an error.
    #[error("upstream error: {0}")]
    Upstream(String),

    /// Wrapped [`AuraError`].
    #[error(transparent)]
    Aura(#[from] AuraError),
}

/// Alias for bridge results.
pub type BridgeResult<T> = Result<T, BridgeError>;

// ---------------------------------------------------------------------------
// InputChannel trait
// ---------------------------------------------------------------------------

/// Trait implemented by every input channel bridge.
///
/// A bridge translates events from an external source (voice, Telegram)
/// into [`UserCommand`] messages and delivers [`DaemonResponse`] back.
#[async_trait]
pub trait InputChannel: Send {
    /// Human-readable name of this channel (e.g., `"voice"`, `"telegram"`).
    fn name(&self) -> &str;

    /// The [`InputSource`] variant this bridge produces.
    fn source(&self) -> InputSource;

    /// Start the bridge.
    ///
    /// The bridge should begin consuming events from its upstream source,
    /// translating them into [`UserCommand`] messages sent via `cmd_tx`,
    /// and routing daemon responses received on `response_rx` back to the
    /// upstream.
    ///
    /// Returns when the bridge is shut down (cancel flag, channel close, or
    /// upstream failure).
    async fn run(
        &mut self,
        cmd_tx: UserCommandTx,
        response_rx: DaemonResponseRx,
    ) -> BridgeResult<()>;
}

// ---------------------------------------------------------------------------
// BridgeHandle — monitor a spawned bridge task
// ---------------------------------------------------------------------------

/// Lightweight handle to a bridge task spawned via `tokio::spawn`.
///
/// Allows the daemon to check whether the bridge is still alive and to
/// request shutdown.
pub struct BridgeHandle {
    /// Name of the bridge (for logging).
    pub name: String,
    /// The `JoinHandle` for the spawned task.
    pub join_handle: tokio::task::JoinHandle<BridgeResult<()>>,
}

impl BridgeHandle {
    /// Check if the bridge task has finished (non-blocking).
    pub fn is_finished(&self) -> bool {
        self.join_handle.is_finished()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Spawn a bridge as a `tokio` task and return a [`BridgeHandle`].
///
/// The caller must supply the [`UserCommandTx`] for the bridge to inject
/// commands, and a [`DaemonResponseRx`] for it to receive responses.
///
/// Because bridges need their own `response_rx` (mpsc is single-consumer),
/// callers should create a **dedicated** response channel pair per bridge.
///
/// # Example
///
/// ```ignore
/// let (resp_tx, resp_rx) = tokio::sync::mpsc::channel(64);
/// let bridge = VoiceBridge::new(engine, cancel.clone());
/// let handle = spawn_bridge(bridge, cmd_tx.clone(), resp_rx);
/// ```
pub fn spawn_bridge(
    mut bridge: Box<dyn InputChannel>,
    cmd_tx: UserCommandTx,
    response_rx: DaemonResponseRx,
) -> BridgeHandle {
    let name = bridge.name().to_string();
    let join_handle = tokio::spawn(async move { bridge.run(cmd_tx, response_rx).await });
    BridgeHandle { name, join_handle }
}

// ---------------------------------------------------------------------------
// System API Bridge re-exports
// ---------------------------------------------------------------------------

pub use system_api::{SystemBridge, SystemBridgeError, SystemCommand, SystemResult};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_core::channels::{DaemonResponse, InputSource, UserCommand};
    use tokio::sync::mpsc;

    /// A trivial test bridge that sends one Chat command and exits.
    struct StubBridge;

    #[async_trait]
    impl InputChannel for StubBridge {
        fn name(&self) -> &str {
            "stub"
        }
        fn source(&self) -> InputSource {
            InputSource::Direct
        }
        async fn run(
            &mut self,
            cmd_tx: UserCommandTx,
            _response_rx: DaemonResponseRx,
        ) -> BridgeResult<()> {
            cmd_tx
                .send(UserCommand::Chat {
                    text: "hello from stub".into(),
                    source: InputSource::Direct,
                    voice_meta: None,
                })
                .await
                .map_err(|_| BridgeError::CommandChannelClosed)?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_stub_bridge_sends_command() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        let (_resp_tx, resp_rx) = mpsc::channel::<DaemonResponse>(16);

        let mut bridge = StubBridge;
        bridge.run(cmd_tx, resp_rx).await.expect("bridge should succeed");

        let cmd = cmd_rx.recv().await.expect("should receive command");
        assert!(matches!(cmd, UserCommand::Chat { text, .. } if text == "hello from stub"));
    }

    #[tokio::test]
    async fn test_spawn_bridge_handle() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        let (_resp_tx, resp_rx) = mpsc::channel::<DaemonResponse>(16);

        let handle = spawn_bridge(Box::new(StubBridge), cmd_tx, resp_rx);
        assert_eq!(handle.name, "stub");

        // Wait for completion.
        let result = handle.join_handle.await.expect("task should not panic");
        assert!(result.is_ok());

        let cmd = cmd_rx.recv().await.expect("should receive command");
        assert!(matches!(cmd, UserCommand::Chat { .. }));
    }

    #[test]
    fn test_input_source_display() {
        assert_eq!(InputSource::Direct.to_string(), "direct");
        assert_eq!(InputSource::Voice.to_string(), "voice");
        assert_eq!(
            InputSource::Telegram { chat_id: 42 }.to_string(),
            "telegram:42"
        );
    }

    #[test]
    fn test_bridge_error_display() {
        let err = BridgeError::CommandChannelClosed;
        assert_eq!(err.to_string(), "command channel closed");

        let err = BridgeError::Upstream("timeout".into());
        assert_eq!(err.to_string(), "upstream error: timeout");
    }
}
