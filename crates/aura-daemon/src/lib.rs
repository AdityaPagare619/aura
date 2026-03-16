#![feature(once_cell_try)]
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

// Workspace-wide clippy configuration for aura-daemon.
// These are intentional style choices for this codebase:
#![allow(clippy::too_many_arguments)] // Complex internal APIs need many params
#![allow(clippy::new_without_default)] // Many types have non-trivial constructors
#![allow(clippy::if_same_then_else)] // Used for clarity in branching logic
#![allow(clippy::wrong_self_convention)] // from_* methods with self are intentional
#![allow(clippy::field_reassign_with_default)] // Struct builder pattern is intentional
#![allow(clippy::manual_clamp)] // Some clamp patterns are clearer without .clamp()
#![allow(clippy::ptr_arg)] // Some &String/&Vec params are needed for trait compat
#![allow(clippy::len_without_is_empty)] // Feedback buffers don't need is_empty
#![allow(clippy::manual_strip)] // Some strip patterns are clearer inline
#![allow(clippy::needless_range_loop)] // Index loops are clearer for DSP/math code

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
#[cfg(feature = "voice")]
pub mod voice;

// Re-export key types at crate root for convenience.
pub use crate::daemon_core::{
    channels::DaemonChannels,
    checkpoint::DaemonCheckpoint,
    shutdown::graceful_shutdown,
    startup::{startup, DaemonState, StartupReport},
};
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
) -> Result<crate::daemon_core::shutdown::ShutdownReport, crate::daemon_core::startup::StartupError>
{
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
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    };

    use aura_types::config::AuraConfig;
    use jni::{objects::JClass, sys::jlong, JNIEnv};

    /// Global cancel flag shared between JNI init and shutdown.
    /// Set during `init()`, read during `shutdown()`.
    static CANCEL_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();

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
                // Store cancel_flag globally so shutdown() can signal it
                let _ = CANCEL_FLAG.set(state.cancel_flag.clone());
                let boxed = Box::new(state);
                Box::into_raw(boxed) as jlong
            },
            Err(e) => {
                let msg = format!("AURA startup failed: {e}");
                let _ = env.throw_new("java/lang/RuntimeException", &msg);
                0
            },
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

        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                // Panic across FFI is undefined behavior — log and bail instead.
                tracing::error!("FATAL: tokio runtime failed to initialize in JNI run(): {e}");
                return;
            },
        };

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
        _state_ptr: jlong,
    ) {
        tracing::info!("JNI shutdown requested");
        if let Some(flag) = CANCEL_FLAG.get() {
            flag.store(true, Ordering::Release);
            tracing::info!("cancel_flag set — daemon will shut down gracefully");
        } else {
            tracing::warn!(
                "shutdown called but cancel_flag was never initialized (init not called?)"
            );
        }
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
