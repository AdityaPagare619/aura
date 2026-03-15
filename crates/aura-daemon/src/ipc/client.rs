//! Async IPC client for communicating with the Neocortex process.
//!
//! [`NeocortexClient`] manages a single persistent connection to the Neocortex
//! process, providing typed send/receive over length-prefixed bincode frames.
//! It handles automatic reconnection with exponential backoff and enforces
//! per-request timeouts.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use aura_types::ipc::{DaemonToNeocortex, NeocortexToDaemon};
use tracing::{debug, error, info, instrument, warn};

use super::{
    protocol::{self, IpcStream},
    IpcError,
};

// ─── Backoff configuration ──────────────────────────────────────────────────

/// Exponential backoff state for reconnection attempts.
#[derive(Debug)]
struct ExponentialBackoff {
    /// Current delay before the next attempt.
    current: Duration,
    /// Minimum delay (first attempt).
    min: Duration,
    /// Maximum delay (cap).
    max: Duration,
    /// Multiplicative factor per attempt.
    factor: u32,
    /// Number of consecutive failures.
    attempts: u32,
}

impl ExponentialBackoff {
    fn new(min: Duration, max: Duration, factor: u32) -> Self {
        Self {
            current: min,
            min,
            max,
            factor,
            attempts: 0,
        }
    }

    /// Return the next backoff duration and advance state.
    fn next_backoff(&mut self) -> Duration {
        let delay = self.current;
        self.attempts = self.attempts.saturating_add(1);
        self.current = (self.current * self.factor).min(self.max);
        delay
    }

    /// Reset after a successful connection.
    fn reset(&mut self) {
        self.current = self.min;
        self.attempts = 0;
    }

    /// How many consecutive failures so far.
    fn attempts(&self) -> u32 {
        self.attempts
    }
}

// ─── NeocortexClient ────────────────────────────────────────────────────────

/// Async IPC client that communicates with the Neocortex process.
///
/// The client maintains a single connection (Unix socket on Android, TCP on
/// host) and provides `send`, `recv`, and `request` (send + await response)
/// methods.  On connection loss it can automatically reconnect with
/// exponential backoff.
///
/// # Concurrency
///
/// This client is **not** `Sync` — it owns a mutable stream.  The intended
/// usage is from a single task that bridges the daemon's internal channels
/// to the IPC socket.
pub struct NeocortexClient {
    /// Active connection to the Neocortex process; `None` when disconnected.
    stream: Option<IpcStream>,

    /// Monotonically increasing request counter for diagnostics / tracing.
    request_counter: AtomicU64,

    /// Reconnection backoff state.
    reconnect_backoff: ExponentialBackoff,
}

impl std::fmt::Debug for NeocortexClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NeocortexClient")
            .field("connected", &self.stream.is_some())
            .field(
                "request_counter",
                &self.request_counter.load(Ordering::Relaxed),
            )
            .field("reconnect_attempts", &self.reconnect_backoff.attempts())
            .finish()
    }
}

impl NeocortexClient {
    /// Attempt to connect to the Neocortex process.
    ///
    /// Respects [`protocol::CONNECT_TIMEOUT`].  On success the client is
    /// immediately ready for `send` / `recv`.
    ///
    /// # Errors
    ///
    /// Returns [`IpcError::Timeout`] or [`IpcError::Io`] if the connection
    /// cannot be established.
    #[instrument(name = "ipc_client_connect", skip_all)]
    pub async fn connect() -> Result<Self, IpcError> {
        info!("connecting to neocortex");
        let stream = protocol::connect_stream().await?;
        info!("connected to neocortex");

        Ok(Self {
            stream: Some(stream),
            request_counter: AtomicU64::new(0),
            reconnect_backoff: ExponentialBackoff::new(
                Duration::from_secs(1),
                Duration::from_secs(30),
                2,
            ),
        })
    }

    /// Create a client in a disconnected state.
    ///
    /// Useful when you want to manage connection timing yourself via
    /// [`reconnect`](Self::reconnect).
    pub fn disconnected() -> Self {
        Self {
            stream: None,
            request_counter: AtomicU64::new(0),
            reconnect_backoff: ExponentialBackoff::new(
                Duration::from_secs(1),
                Duration::from_secs(30),
                2,
            ),
        }
    }

    /// Whether the client currently holds an open connection.
    ///
    /// Note: this only checks local state; the remote end may have closed
    /// without us knowing until the next I/O attempt.
    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// How many requests have been sent during this client's lifetime.
    pub fn request_count(&self) -> u64 {
        self.request_counter.load(Ordering::Relaxed)
    }

    /// Send a message to the Neocortex process.
    ///
    /// # Errors
    ///
    /// - [`IpcError::NotConnected`] if no connection is active.
    /// - [`IpcError::Encoding`] if serialization fails.
    /// - [`IpcError::MessageTooLarge`] if the encoded frame exceeds the limit.
    /// - [`IpcError::ConnectionLost`] / [`IpcError::Io`] on write failure.
    #[instrument(name = "ipc_send", skip_all)]
    pub async fn send(&mut self, msg: &DaemonToNeocortex) -> Result<(), IpcError> {
        let stream = self.stream.as_mut().ok_or(IpcError::NotConnected)?;
        let frame = protocol::encode_frame(msg)?;
        protocol::write_frame(stream, &frame).await.map_err(|e| {
            // On write failure, mark disconnected.
            error!(error = %e, "send failed — marking disconnected");
            self.stream = None;
            e
        })
    }

    /// Receive the next message from the Neocortex process.
    ///
    /// Blocks (asynchronously) until a complete frame arrives.  No timeout
    /// is applied here — use [`request`](Self::request) for timeout-bounded
    /// round-trips.
    ///
    /// # Errors
    ///
    /// - [`IpcError::NotConnected`] if no connection is active.
    /// - [`IpcError::ConnectionLost`] on EOF or reset.
    /// - [`IpcError::Encoding`] if deserialization fails.
    #[instrument(name = "ipc_recv", skip_all)]
    pub async fn recv(&mut self) -> Result<NeocortexToDaemon, IpcError> {
        let stream = self.stream.as_mut().ok_or(IpcError::NotConnected)?;
        protocol::decode_frame(stream).await.map_err(|e| {
            if matches!(e, IpcError::ConnectionLost { .. }) {
                error!(error = %e, "recv detected connection loss");
                self.stream = None;
            }
            e
        })
    }

    /// Send a request and wait for the response, with [`REQUEST_TIMEOUT`].
    ///
    /// This is the primary high-level method. It sends `msg`, then waits up
    /// to 30 seconds for a response frame.  The request counter is
    /// incremented atomically for tracing correlation.
    ///
    /// # Errors
    ///
    /// Any error from [`send`](Self::send) or [`recv`](Self::recv), plus
    /// [`IpcError::Timeout`] if no response arrives within the deadline.
    #[instrument(name = "ipc_request", skip_all, fields(
        req_id = self.request_counter.load(Ordering::Relaxed),
    ))]
    pub async fn request(
        &mut self,
        msg: &DaemonToNeocortex,
    ) -> Result<NeocortexToDaemon, IpcError> {
        let req_id = self.request_counter.fetch_add(1, Ordering::Relaxed);
        debug!(req_id, "sending request");

        self.send(msg).await?;

        let result = tokio::time::timeout(protocol::REQUEST_TIMEOUT, self.recv()).await;

        match result {
            Ok(inner) => {
                let resp = inner?;
                debug!(req_id, resp = ?std::mem::discriminant(&resp), "received response");
                Ok(resp)
            },
            Err(_elapsed) => {
                warn!(
                    req_id,
                    "request timed out after {:?}",
                    protocol::REQUEST_TIMEOUT
                );
                // Timeout likely means the Neocortex process is stuck.
                // Mark disconnected so the next call triggers reconnect.
                self.stream = None;
                Err(IpcError::Timeout {
                    context: format!(
                        "request {req_id} timed out after {:?}",
                        protocol::REQUEST_TIMEOUT
                    ),
                })
            },
        }
    }

    /// Attempt to re-establish the connection with exponential backoff.
    ///
    /// Sleeps for the current backoff duration, then tries to connect.
    /// On success, resets the backoff. On failure, advances the backoff
    /// and returns the error.
    ///
    /// # Errors
    ///
    /// Returns the connection error if the attempt fails.
    #[instrument(name = "ipc_reconnect", skip_all, fields(
        attempt = self.reconnect_backoff.attempts(),
    ))]
    pub async fn reconnect(&mut self) -> Result<(), IpcError> {
        // Drop the old stream if any.
        self.stream = None;

        let delay = self.reconnect_backoff.next_backoff();
        info!(
            delay_ms = delay.as_millis() as u64,
            attempt = self.reconnect_backoff.attempts(),
            "reconnecting after backoff"
        );
        tokio::time::sleep(delay).await;

        match protocol::connect_stream().await {
            Ok(stream) => {
                info!("reconnected to neocortex");
                self.stream = Some(stream);
                self.reconnect_backoff.reset();
                Ok(())
            },
            Err(e) => {
                warn!(
                    error = %e,
                    attempt = self.reconnect_backoff.attempts(),
                    "reconnect failed"
                );
                Err(e)
            },
        }
    }

    /// Drop the current connection (if any) without attempting reconnect.
    pub fn disconnect(&mut self) {
        if self.stream.take().is_some() {
            info!("disconnected from neocortex");
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_progression() {
        let mut backoff =
            ExponentialBackoff::new(Duration::from_secs(1), Duration::from_secs(30), 2);
        assert_eq!(backoff.next_backoff(), Duration::from_secs(1));
        assert_eq!(backoff.next_backoff(), Duration::from_secs(2));
        assert_eq!(backoff.next_backoff(), Duration::from_secs(4));
        assert_eq!(backoff.next_backoff(), Duration::from_secs(8));
        assert_eq!(backoff.next_backoff(), Duration::from_secs(16));
        // Should cap at 30s.
        assert_eq!(backoff.next_backoff(), Duration::from_secs(30));
        assert_eq!(backoff.next_backoff(), Duration::from_secs(30));
    }

    #[test]
    fn backoff_reset() {
        let mut backoff =
            ExponentialBackoff::new(Duration::from_secs(1), Duration::from_secs(30), 2);
        let _ = backoff.next_backoff(); // 1s
        let _ = backoff.next_backoff(); // 2s
        assert_eq!(backoff.attempts(), 2);
        backoff.reset();
        assert_eq!(backoff.attempts(), 0);
        assert_eq!(backoff.next_backoff(), Duration::from_secs(1));
    }

    #[test]
    fn disconnected_client_reports_not_connected() {
        let client = NeocortexClient::disconnected();
        assert!(!client.is_connected());
        assert_eq!(client.request_count(), 0);
    }

    #[tokio::test]
    async fn send_on_disconnected_returns_not_connected() {
        let mut client = NeocortexClient::disconnected();
        let result = client.send(&DaemonToNeocortex::Ping).await;
        assert!(matches!(result, Err(IpcError::NotConnected)));
    }

    #[tokio::test]
    async fn recv_on_disconnected_returns_not_connected() {
        let mut client = NeocortexClient::disconnected();
        let result = client.recv().await;
        assert!(matches!(result, Err(IpcError::NotConnected)));
    }

    #[tokio::test]
    async fn request_on_disconnected_returns_not_connected() {
        let mut client = NeocortexClient::disconnected();
        let result = client.request(&DaemonToNeocortex::Ping).await;
        assert!(matches!(result, Err(IpcError::NotConnected)));
    }

    #[test]
    fn debug_format() {
        let client = NeocortexClient::disconnected();
        let dbg = format!("{client:?}");
        assert!(dbg.contains("NeocortexClient"));
        assert!(dbg.contains("connected: false"));
    }

    #[test]
    fn disconnect_is_idempotent() {
        let mut client = NeocortexClient::disconnected();
        client.disconnect();
        client.disconnect();
        assert!(!client.is_connected());
    }
}
