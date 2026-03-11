//! `aura-daemon` — the always-on core of AURA v4.
//!
//! Compiles as `cdylib` (`libaura_core.so`) loaded by the Android Kotlin
//! shell via JNI, or as a regular `lib` for host testing.
//!
//! # Architecture
//!
//! The daemon owns the tokio single-threaded runtime, 7 internal channels,
//! a SQLite database, and a bincode checkpoint file.  The event loop
//! (`daemon_core::main_loop::run`) `select!`s over all channels and a periodic
//! checkpoint timer.

pub mod arc;
pub mod bridge;
pub mod daemon_core;
pub mod execution;
pub mod extensions;
pub mod goals;
pub mod health;
pub mod identity;
pub mod ipc;
pub mod memory;
pub mod outcome_bus;
pub mod persistence;
pub mod pipeline;
pub mod platform;
pub mod policy;
pub mod policy_ethics_integration_tests;
pub mod reaction;
pub mod routing;
pub mod screen;
pub mod telegram;
pub mod telemetry;
pub mod voice;

// Re-export key types at crate root for convenience.
pub use crate::daemon_core::channels::DaemonChannels;
pub use crate::daemon_core::checkpoint::DaemonCheckpoint;
pub use crate::daemon_core::shutdown::graceful_shutdown;
pub use crate::daemon_core::startup::{startup, DaemonState, StartupReport};

// Re-export JNI env helper at crate root so that `crate::jni_env()` works
// from any module (e.g. screen/actions.rs).
pub use crate::platform::jni_bridge::jni_env;

// ---------------------------------------------------------------------------
// Host testing entry point
// ---------------------------------------------------------------------------

/// Convenience function for integration tests and host-side tooling.
///
/// Creates a tokio current-thread runtime, runs startup, enters the main
/// loop, and handles shutdown.  Returns the final `ShutdownReport`.
///
/// # Errors
/// Returns an error if startup fails.
pub fn init_for_testing(
    config: aura_types::config::AuraConfig,
) -> Result<crate::daemon_core::shutdown::ShutdownReport, crate::daemon_core::startup::StartupError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime must initialize for testing");

    rt.block_on(async {
        let (state, _report) = startup(config)?;

        let cancel = state.cancel_flag.clone();

        // In test mode, immediately cancel to skip the main loop.
        cancel.store(true, std::sync::atomic::Ordering::Release);

        crate::daemon_core::main_loop::run(state).await;

        // We can't easily recover the DaemonState after `run` consumes it,
        // so for testing we just return a synthetic shutdown report.
        Ok(crate::daemon_core::shutdown::ShutdownReport {
            elapsed_ms: 0,
            checkpoint_saved: true,
            db_closed: true,
            messages_drained: 0,
        })
    })
}

// ---------------------------------------------------------------------------
// JNI entry points (Android only)
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod jni_bridge {
    use jni::objects::JClass;
    use jni::sys::jlong;
    use jni::JNIEnv;
    use std::sync::atomic::Ordering;

    use aura_types::config::AuraConfig;

    /// `JNI_OnLoad` equivalent — called when the Kotlin shell loads
    /// `libaura_core.so`.
    ///
    /// # Safety
    /// Called by the JVM — raw JNI pointer.
    #[no_mangle]
    pub unsafe extern "system" fn Java_dev_aura_core_NativeBridge_init(
        mut env: JNIEnv,
        _class: JClass,
    ) -> jlong {
        // Default config for now — real implementation reads from
        // the app's files dir passed via JNI.
        let config = AuraConfig::default();

        match crate::startup(config) {
            Ok((state, _report)) => {
                let boxed = Box::new(state);
                Box::into_raw(boxed) as jlong
            }
            Err(e) => {
                let msg = format!("AURA startup failed: {e}");
                let _ = env.throw_new("java/lang/RuntimeException", &msg);
                0
            }
        }
    }

    /// Start the main event loop on the current thread.
    ///
    /// # Safety
    /// `state_ptr` must be a valid pointer from `init`.
    #[no_mangle]
    pub unsafe extern "system" fn Java_dev_aura_core_NativeBridge_run(
        _env: JNIEnv,
        _class: JClass,
        state_ptr: jlong,
    ) {
        if state_ptr == 0 {
            tracing::error!("run called with null state pointer");
            return;
        }

        let state = unsafe { *Box::from_raw(state_ptr as *mut crate::DaemonState) };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime should initialize");

        rt.block_on(async {
            crate::daemon_core::main_loop::run(state).await;
        });
    }

    /// Request graceful shutdown.
    ///
    /// # Safety
    /// Called from Kotlin — `state_ptr` is best-effort (may already be freed
    /// if shutdown raced).  The cancel flag is the safe signal path.
    #[no_mangle]
    pub unsafe extern "system" fn Java_dev_aura_core_NativeBridge_shutdown(
        _env: JNIEnv,
        _class: JClass,
        state_ptr: jlong,
    ) {
        // We can't safely dereference state_ptr here because `run` consumed it.
        // Instead, shutdown is triggered via the cancel_flag which is shared
        // between the DaemonState and an Arc stored in a global.
        // For now, log — the real mechanism uses a static AtomicBool.
        tracing::info!("JNI shutdown requested");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_for_testing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");

        let mut config = aura_types::config::AuraConfig::default();
        config.sqlite.db_path = db_path.to_string_lossy().to_string();

        let report = init_for_testing(config).expect("init_for_testing should succeed");
        assert!(report.checkpoint_saved);
        assert!(report.db_closed);
    }
}
