//! Graceful shutdown — 5 steps with a hard timeout.
//!
//! 1. Signal all producers to stop (drop tx halves / set cancel flag).
//! 2. Drain remaining channel messages (bounded iterations).
//! 3. Flush final checkpoint to disk.
//! 4. Close the database connection.
//! 5. Report shutdown complete (JNI callback on Android).
//!
//! The entire sequence is wrapped in a timeout (default 5 s).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::daemon_core::channels::DaemonChannels;
use crate::daemon_core::checkpoint::{save_checkpoint, DaemonCheckpoint};

// ---------------------------------------------------------------------------
// Shutdown result
// ---------------------------------------------------------------------------

/// Outcome of the shutdown sequence.
#[derive(Debug)]
pub struct ShutdownReport {
    /// Total wall-clock time of shutdown.
    pub elapsed_ms: u64,
    /// Whether the checkpoint was saved successfully.
    pub checkpoint_saved: bool,
    /// Whether the database was closed cleanly.
    pub db_closed: bool,
    /// Number of channel messages drained during shutdown.
    pub messages_drained: u64,
}

// ---------------------------------------------------------------------------
// Shutdown implementation
// ---------------------------------------------------------------------------

/// Execute the 5-step graceful shutdown.
///
/// This function is **not** async — it runs synchronously after the tokio
/// runtime's main loop exits (or is called from a `spawn_blocking` context).
///
/// # Arguments
/// * `channels` — Daemon channels (we drop tx halves to signal producers).
/// * `checkpoint` — Final state to persist.
/// * `checkpoint_path` — Where to write `state.bin`.
/// * `db` — SQLite connection to close.
/// * `cancel_flag` — Shared flag; set to `true` to unblock any waiters.
/// * `timeout_secs` — Hard deadline for the whole sequence.
pub fn graceful_shutdown(
    channels: DaemonChannels,
    checkpoint: &DaemonCheckpoint,
    checkpoint_path: &Path,
    db: Option<rusqlite::Connection>,
    cancel_flag: Arc<AtomicBool>,
    timeout_secs: u64,
) -> ShutdownReport {
    let start = Instant::now();
    let deadline = start + std::time::Duration::from_secs(timeout_secs);

    let mut report = ShutdownReport {
        elapsed_ms: 0,
        checkpoint_saved: false,
        db_closed: false,
        messages_drained: 0,
    };

    // -----------------------------------------------------------------------
    // Step 1: Signal cancellation
    // -----------------------------------------------------------------------
    tracing::info!("shutdown step 1/5: signalling cancellation");
    cancel_flag.store(true, Ordering::Release);

    if Instant::now() >= deadline {
        tracing::warn!("shutdown timeout after step 1");
        report.elapsed_ms = start.elapsed().as_millis() as u64;
        return report;
    }

    // -----------------------------------------------------------------------
    // Step 2: Drain channels (drop tx halves, then drain rx)
    // -----------------------------------------------------------------------
    tracing::info!("shutdown step 2/5: draining channels");
    report.messages_drained = drain_channels(channels);
    tracing::info!(drained = report.messages_drained, "channels drained");

    if Instant::now() >= deadline {
        tracing::warn!("shutdown timeout after step 2");
        report.elapsed_ms = start.elapsed().as_millis() as u64;
        return report;
    }

    // -----------------------------------------------------------------------
    // Step 3: Flush final checkpoint
    // -----------------------------------------------------------------------
    tracing::info!("shutdown step 3/5: saving final checkpoint");
    match save_checkpoint(checkpoint, checkpoint_path) {
        Ok(()) => {
            report.checkpoint_saved = true;
            tracing::info!("final checkpoint saved");
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to save final checkpoint");
        }
    }

    if Instant::now() >= deadline {
        tracing::warn!("shutdown timeout after step 3");
        report.elapsed_ms = start.elapsed().as_millis() as u64;
        return report;
    }

    // -----------------------------------------------------------------------
    // Step 4: Close database
    // -----------------------------------------------------------------------
    tracing::info!("shutdown step 4/5: closing database");
    if let Some(conn) = db {
        match conn.close() {
            Ok(()) => {
                report.db_closed = true;
                tracing::info!("database closed cleanly");
            }
            Err((conn, e)) => {
                tracing::error!(error = %e, "database close failed — dropping connection");
                drop(conn);
            }
        }
    } else {
        report.db_closed = true; // nothing to close
    }

    // -----------------------------------------------------------------------
    // Step 5: Report completion (+ JNI callback on Android)
    // -----------------------------------------------------------------------
    report.elapsed_ms = start.elapsed().as_millis() as u64;
    tracing::info!(
        elapsed_ms = report.elapsed_ms,
        checkpoint_saved = report.checkpoint_saved,
        db_closed = report.db_closed,
        messages_drained = report.messages_drained,
        "shutdown step 5/5: complete"
    );

    #[cfg(target_os = "android")]
    {
        jni_notify_shutdown_complete(&report);
    }

    report
}

// ---------------------------------------------------------------------------
// Channel drain helper
// ---------------------------------------------------------------------------

/// Drop all tx halves (by consuming the struct), then drain each rx.
/// Returns the total number of messages drained.
fn drain_channels(channels: DaemonChannels) -> u64 {
    let DaemonChannels {
        a11y_tx,
        mut a11y_rx,
        notification_tx,
        mut notification_rx,
        user_command_tx,
        mut user_command_rx,
        ipc_outbound_tx,
        mut ipc_outbound_rx,
        ipc_inbound_tx,
        mut ipc_inbound_rx,
        db_write_tx,
        mut db_write_rx,
        cron_tick_tx,
        mut cron_tick_rx,
        ..
    } = channels;

    // Drop all senders first — this closes the channels.
    drop(a11y_tx);
    drop(notification_tx);
    drop(user_command_tx);
    drop(ipc_outbound_tx);
    drop(ipc_inbound_tx);
    drop(db_write_tx);
    drop(cron_tick_tx);

    let mut count: u64 = 0;

    // Drain each receiver (non-async `try_recv` since we're synchronous).
    while a11y_rx.try_recv().is_ok() {
        count += 1;
    }
    while notification_rx.try_recv().is_ok() {
        count += 1;
    }
    while user_command_rx.try_recv().is_ok() {
        count += 1;
    }
    while ipc_outbound_rx.try_recv().is_ok() {
        count += 1;
    }
    while ipc_inbound_rx.try_recv().is_ok() {
        count += 1;
    }
    while db_write_rx.try_recv().is_ok() {
        count += 1;
    }
    while cron_tick_rx.try_recv().is_ok() {
        count += 1;
    }

    count
}

// ---------------------------------------------------------------------------
// Android JNI callback (compile-gated)
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
fn jni_notify_shutdown_complete(report: &ShutdownReport) {
    // In a real implementation, this calls back into Kotlin via JNI
    // to inform the Android service that the native library is done.
    // For now, just log — the actual JNI env will be stored in a global
    // during JNI_OnLoad.
    tracing::info!(
        elapsed_ms = report.elapsed_ms,
        "JNI: notified Kotlin of shutdown completion"
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_core::channels::DaemonChannels;

    #[test]
    fn test_graceful_shutdown_with_empty_channels() {
        let _ = tracing_subscriber::fmt::try_init();

        let dir = tempfile::tempdir().expect("tempdir");
        let cp_path = dir.path().join("state.bin");
        let channels = DaemonChannels::new();
        let checkpoint = DaemonCheckpoint::default();
        let cancel = Arc::new(AtomicBool::new(false));

        let report = graceful_shutdown(channels, &checkpoint, &cp_path, None, cancel.clone(), 5);

        assert!(report.checkpoint_saved, "checkpoint should be saved");
        assert!(report.db_closed, "db should be marked closed (none provided)");
        assert_eq!(report.messages_drained, 0);
        assert!(report.elapsed_ms < 5000, "should complete well within timeout");
        assert!(cancel.load(Ordering::Acquire), "cancel flag should be set");
    }

    #[tokio::test]
    async fn test_shutdown_drains_pending_messages() {
        let _ = tracing_subscriber::fmt::try_init();

        let dir = tempfile::tempdir().expect("tempdir");
        let cp_path = dir.path().join("state.bin");
        let channels = DaemonChannels::new();

        // Send some messages before shutdown.
        use crate::daemon_core::channels::{InputSource, UserCommand};
        for _ in 0..5 {
            channels
                .user_command_tx
                .send(UserCommand::Chat {
                    text: "bye".to_string(),
                    source: InputSource::Direct,
                    voice_meta: None,
                })
                .await
                .expect("send");
        }

        let checkpoint = DaemonCheckpoint::default();
        let cancel = Arc::new(AtomicBool::new(false));

        let report = graceful_shutdown(channels, &checkpoint, &cp_path, None, cancel, 5);

        assert_eq!(report.messages_drained, 5);
        assert!(report.checkpoint_saved);
    }
}
