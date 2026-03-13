//! Response fan-out router for the bridge layer.
//!
//! The daemon pipeline produces [`DaemonResponse`] messages on a single
//! `mpsc` channel. Multiple bridges (voice, Telegram, direct) each need
//! to receive only the responses addressed to *their* input source.
//!
//! [`ResponseRouter`] solves this by:
//!
//! 1. Taking **exclusive ownership** of the single `DaemonResponseRx`.
//! 2. Maintaining a registry of per-bridge output channels, keyed by
//!    [`InputSource::variant_key()`] (e.g. `"voice"`, `"telegram"`).
//! 3. Running as a long-lived `tokio` task that reads each response and
//!    forwards it to the correct bridge's dedicated channel.
//!
//! # Lifecycle
//!
//! ```text
//! DaemonChannels::new()
//!   ├── response_tx  → cloned into LoopSubsystems (handlers send responses)
//!   └── response_rx  → moved into ResponseRouter
//!
//! ResponseRouter::register("voice")   → voice_bridge_rx
//! ResponseRouter::register("telegram") → telegram_bridge_rx
//! ResponseRouter::run()               → spawned as tokio task
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

use crate::daemon_core::channels::{DaemonResponse, DaemonResponseRx};

/// Capacity for each per-bridge output channel.
const PER_BRIDGE_CAPACITY: usize = 64;

/// Receiver half given to a bridge after registration.
pub type BridgeResponseRx = mpsc::Receiver<DaemonResponse>;

/// Sender half retained by the router for fan-out delivery.
type BridgeResponseTx = mpsc::Sender<DaemonResponse>;

// ---------------------------------------------------------------------------
// RouterRegistry — shared, lock-protected map of bridge senders
// ---------------------------------------------------------------------------

/// Thread-safe registry of per-bridge output channels.
///
/// Wrapped in `Arc<Mutex<…>>` so that:
/// - Bridges can register **before** the router task starts.
/// - The router task can take a snapshot or read the registry while running.
#[derive(Debug, Default)]
struct RouterRegistry {
    /// Map from `variant_key()` → sender half of the bridge's dedicated channel.
    bridges: HashMap<&'static str, BridgeResponseTx>,
}

// ---------------------------------------------------------------------------
// ResponseRouter
// ---------------------------------------------------------------------------

/// Fan-out router that distributes [`DaemonResponse`] messages to registered
/// bridges based on [`InputSource::variant_key()`].
///
/// # Registration
///
/// Call [`register`](Self::register) for each bridge **before** calling
/// [`run`](Self::run). Registration returns a `Receiver<DaemonResponse>`
/// that the bridge uses to receive its responses.
///
/// # Running
///
/// [`run`](Self::run) consumes `self` and loops until the upstream
/// `DaemonResponseRx` closes (i.e., all `DaemonResponseTx` clones are dropped).
///
/// # Unroutable responses
///
/// If a response's `destination.variant_key()` has no registered bridge, the
/// response is logged and dropped. This is a normal condition during startup
/// or when a bridge has disconnected.
pub struct ResponseRouter {
    /// The single daemon response receiver — owned exclusively by this router.
    response_rx: DaemonResponseRx,
    /// Shared registry of per-bridge output channels.
    registry: Arc<Mutex<RouterRegistry>>,
}

impl ResponseRouter {
    /// Create a new router that owns the given `DaemonResponseRx`.
    ///
    /// The `response_rx` should come from [`DaemonChannels`] — it must **not**
    /// be shared with the main loop's `select!` (the router replaces that branch).
    pub fn new(response_rx: DaemonResponseRx) -> Self {
        Self {
            response_rx,
            registry: Arc::new(Mutex::new(RouterRegistry::default())),
        }
    }

    /// Register a bridge and return a dedicated response receiver.
    ///
    /// `variant_key` must match the value returned by
    /// [`InputSource::variant_key()`] for the bridge's source type
    /// (e.g. `"voice"`, `"telegram"`, `"direct"`).
    ///
    /// # Errors
    ///
    /// Returns `Err` if a bridge with the same key is already registered.
    /// This prevents accidental double-registration which would cause one
    /// bridge to silently lose responses.
    pub async fn register(
        &self,
        variant_key: &'static str,
    ) -> Result<BridgeResponseRx, RouterError> {
        let mut reg = self.registry.lock().await;
        if reg.bridges.contains_key(variant_key) {
            return Err(RouterError::AlreadyRegistered(variant_key));
        }
        let (tx, rx) = mpsc::channel(PER_BRIDGE_CAPACITY);
        reg.bridges.insert(variant_key, tx);
        info!(bridge = variant_key, "bridge registered with response router");
        Ok(rx)
    }

    /// Unregister a bridge, dropping its output channel.
    ///
    /// Any buffered but undelivered responses for this bridge are lost.
    /// Returns `true` if the bridge was found and removed.
    pub async fn unregister(&self, variant_key: &'static str) -> bool {
        let mut reg = self.registry.lock().await;
        let removed = reg.bridges.remove(variant_key).is_some();
        if removed {
            info!(bridge = variant_key, "bridge unregistered from response router");
        }
        removed
    }

    /// Returns the number of currently registered bridges.
    pub async fn bridge_count(&self) -> usize {
        self.registry.lock().await.bridges.len()
    }

    /// Run the fan-out loop.
    ///
    /// This method consumes `self` and loops until the upstream
    /// `DaemonResponseRx` is closed (all `DaemonResponseTx` clones dropped).
    ///
    /// For each received [`DaemonResponse`], the router:
    /// 1. Looks up the destination's `variant_key()` in the registry.
    /// 2. Attempts `try_send` on the bridge's channel.
    /// 3. On `Full` — logs a warning and drops the response (backpressure).
    /// 4. On `Closed` — removes the bridge from the registry.
    /// 5. On no match — logs a debug message and drops the response.
    pub async fn run(mut self) {
        let mut routed: u64 = 0;
        let mut dropped: u64 = 0;

        info!("response router started");

        while let Some(response) = self.response_rx.recv().await {
            let key = response.destination.variant_key();

            let mut reg = self.registry.lock().await;
            if let Some(tx) = reg.bridges.get(key) {
                match tx.try_send(response) {
                    Ok(()) => {
                        routed += 1;
                        debug!(bridge = key, total_routed = routed, "response routed");
                    }
                    Err(mpsc::error::TrySendError::Full(resp)) => {
                        dropped += 1;
                        warn!(
                            bridge = key,
                            text_len = resp.text.len(),
                            total_dropped = dropped,
                            "bridge channel full — response dropped (backpressure)"
                        );
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        dropped += 1;
                        reg.bridges.remove(key);
                        warn!(
                            bridge = key,
                            total_dropped = dropped,
                            "bridge channel closed — unregistered automatically"
                        );
                    }
                }
            } else {
                dropped += 1;
                debug!(
                    destination = key,
                    total_dropped = dropped,
                    "no bridge registered for destination — response dropped"
                );
            }
        }

        info!(
            total_routed = routed,
            total_dropped = dropped,
            "response router stopped — upstream channel closed"
        );
    }

    /// Spawn the router as a `tokio` task and return a [`RouterHandle`].
    ///
    /// This is the preferred way to start the router. The handle allows the
    /// caller to check whether the router task is still alive.
    pub fn spawn(self) -> RouterHandle {
        let join_handle = tokio::spawn(self.run());
        RouterHandle { join_handle }
    }
}

// ---------------------------------------------------------------------------
// RouterHandle — monitor the spawned router task
// ---------------------------------------------------------------------------

/// Handle to the spawned response router task.
///
/// Use [`is_finished`](Self::is_finished) to check if the router has exited,
/// or `await` the [`join_handle`](Self::join_handle) to wait for completion.
pub struct RouterHandle {
    /// The `JoinHandle` for the spawned router task.
    pub join_handle: tokio::task::JoinHandle<()>,
}

impl RouterHandle {
    /// Check if the router task has finished (non-blocking).
    pub fn is_finished(&self) -> bool {
        self.join_handle.is_finished()
    }
}

// ---------------------------------------------------------------------------
// RouterError
// ---------------------------------------------------------------------------

/// Errors specific to the response router.
#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    /// A bridge with this variant key is already registered.
    #[error("bridge already registered: {0}")]
    AlreadyRegistered(&'static str),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_core::channels::{DaemonResponse, InputSource};
    use tokio::sync::mpsc;

    /// Helper: create a (response_tx, ResponseRouter) pair.
    fn make_router() -> (mpsc::Sender<DaemonResponse>, ResponseRouter) {
        let (tx, rx) = mpsc::channel(64);
        (tx, ResponseRouter::new(rx))
    }

    #[tokio::test]
    async fn test_register_and_route_voice() {
        let (resp_tx, router) = make_router();

        let mut voice_rx = router.register("voice").await.expect("register voice");
        assert_eq!(router.bridge_count().await, 1);

        // Spawn the router.
        let handle = router.spawn();

        // Send a voice-destined response.
        resp_tx
            .send(DaemonResponse {
                destination: InputSource::Voice,
                text: "hello voice".into(),
                mood_hint: None,
            })
            .await
            .expect("send");

        // The voice bridge should receive it.
        let resp = voice_rx.recv().await.expect("voice should receive response");
        assert_eq!(resp.text, "hello voice");
        assert_eq!(resp.destination.variant_key(), "voice");

        // Cleanup: drop the tx so the router shuts down.
        drop(resp_tx);
        handle.join_handle.await.expect("router should finish");
    }

    #[tokio::test]
    async fn test_register_and_route_telegram() {
        let (resp_tx, router) = make_router();

        let mut tg_rx = router.register("telegram").await.expect("register telegram");
        let handle = router.spawn();

        resp_tx
            .send(DaemonResponse {
                destination: InputSource::Telegram { chat_id: 42 },
                text: "hi telegram".into(),
                mood_hint: None,
            })
            .await
            .expect("send");

        let resp = tg_rx.recv().await.expect("telegram should receive response");
        assert_eq!(resp.text, "hi telegram");
        assert!(matches!(resp.destination, InputSource::Telegram { chat_id: 42 }));

        drop(resp_tx);
        handle.join_handle.await.expect("router should finish");
    }

    #[tokio::test]
    async fn test_multiple_bridges_isolation() {
        let (resp_tx, router) = make_router();

        let mut voice_rx = router.register("voice").await.expect("register voice");
        let mut tg_rx = router.register("telegram").await.expect("register telegram");
        assert_eq!(router.bridge_count().await, 2);

        let handle = router.spawn();

        // Send one to voice, one to telegram.
        resp_tx
            .send(DaemonResponse {
                destination: InputSource::Voice,
                text: "for voice".into(),
                mood_hint: None,
            })
            .await
            .expect("send");

        resp_tx
            .send(DaemonResponse {
                destination: InputSource::Telegram { chat_id: 1 },
                text: "for telegram".into(),
                mood_hint: None,
            })
            .await
            .expect("send");

        // Each bridge receives only its own message.
        let v = voice_rx.recv().await.expect("voice response");
        assert_eq!(v.text, "for voice");

        let t = tg_rx.recv().await.expect("telegram response");
        assert_eq!(t.text, "for telegram");

        // Verify no cross-contamination: try_recv should be empty.
        assert!(voice_rx.try_recv().is_err());
        assert!(tg_rx.try_recv().is_err());

        drop(resp_tx);
        handle.join_handle.await.expect("router should finish");
    }

    #[tokio::test]
    async fn test_double_register_fails() {
        let (_resp_tx, router) = make_router();

        router.register("voice").await.expect("first register");
        let result = router.register("voice").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RouterError::AlreadyRegistered("voice")));
    }

    #[tokio::test]
    async fn test_unregistered_destination_dropped() {
        let (resp_tx, router) = make_router();

        // Register voice but NOT telegram.
        let mut voice_rx = router.register("voice").await.expect("register voice");
        let handle = router.spawn();

        // Send a telegram-destined response — should be silently dropped.
        resp_tx
            .send(DaemonResponse {
                destination: InputSource::Telegram { chat_id: 99 },
                text: "nobody home".into(),
                mood_hint: None,
            })
            .await
            .expect("send");

        // Send a voice response after, to prove the router is still alive.
        resp_tx
            .send(DaemonResponse {
                destination: InputSource::Voice,
                text: "still working".into(),
                mood_hint: None,
            })
            .await
            .expect("send");

        let resp = voice_rx.recv().await.expect("voice should get its message");
        assert_eq!(resp.text, "still working");

        drop(resp_tx);
        handle.join_handle.await.expect("router should finish");
    }

    #[tokio::test]
    async fn test_unregister_bridge() {
        let (_resp_tx, router) = make_router();

        router.register("voice").await.expect("register voice");
        assert_eq!(router.bridge_count().await, 1);

        assert!(router.unregister("voice").await);
        assert_eq!(router.bridge_count().await, 0);

        // Double-unregister returns false.
        assert!(!router.unregister("voice").await);
    }

    #[tokio::test]
    async fn test_router_stops_when_upstream_closes() {
        let (resp_tx, router) = make_router();
        let _voice_rx = router.register("voice").await.expect("register");

        let handle = router.spawn();

        // Drop the upstream sender — router should stop.
        drop(resp_tx);

        // The router task should complete without panic.
        handle.join_handle.await.expect("router should finish cleanly");
    }

    #[tokio::test]
    async fn test_closed_bridge_auto_unregistered() {
        let (resp_tx, router) = make_router();

        let voice_rx = router.register("voice").await.expect("register");
        // Drop the receiver — simulates bridge crash/shutdown.
        drop(voice_rx);

        let handle = router.spawn();

        // Send a response to the now-dead bridge.
        resp_tx
            .send(DaemonResponse {
                destination: InputSource::Voice,
                text: "you there?".into(),
                mood_hint: None,
            })
            .await
            .expect("send should succeed (router is alive)");

        // Give the router a moment to process.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // The bridge should have been auto-unregistered.
        // Send another message — this time it goes to "no bridge registered" path.
        resp_tx
            .send(DaemonResponse {
                destination: InputSource::Voice,
                text: "still there?".into(),
                mood_hint: None,
            })
            .await
            .expect("send");

        drop(resp_tx);
        handle.join_handle.await.expect("router should finish");
    }

    #[tokio::test]
    async fn test_router_handle_is_finished() {
        let (resp_tx, router) = make_router();
        let _voice_rx = router.register("voice").await.expect("register");
        let handle = router.spawn();

        // Router should still be running.
        assert!(!handle.is_finished());

        // Close upstream.
        drop(resp_tx);

        // Wait for completion.
        handle.join_handle.await.expect("router should finish");
    }
}
