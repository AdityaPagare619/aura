//! IPC client module for the AURA daemon.
//!
//! Provides the daemon-side client for communicating with the Neocortex
//! process over length-prefixed bincode frames. On Android, uses Unix domain
//! sockets at an abstract address. On host/Windows, falls back to TCP on
//! `127.0.0.1:19400`.
//!
//! # Submodules
//!
//! - [`protocol`] — Wire framing, constants, encode/decode helpers.
//! - [`client`]   — Async `NeocortexClient` for sending requests and receiving responses.
//! - [`spawn`]    — Process lifecycle management for the Neocortex binary.

pub mod client;
pub mod protocol;
pub mod spawn;

// Re-export primary public types.
pub use client::NeocortexClient;
pub use protocol::{
    CONNECT_TIMEOUT, FRAME_HEADER_SIZE, MAX_MESSAGE_SIZE, REQUEST_TIMEOUT, SOCKET_ADDR,
    TCP_FALLBACK_ADDR, TCP_FALLBACK_PORT,
};
pub use spawn::NeocortexProcess;

use std::io;

/// Errors arising from IPC communication with the Neocortex process.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// Underlying I/O failure (socket read/write, connection refused, etc.).
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// A request or connection attempt exceeded its deadline.
    #[error("timeout: {context}")]
    Timeout {
        /// Human-readable description of what timed out.
        context: String,
    },

    /// Serialization or deserialization of a bincode frame failed.
    #[error("encoding error: {0}")]
    Encoding(String),

    /// The connection to the Neocortex process was lost unexpectedly.
    #[error("connection lost: {reason}")]
    ConnectionLost {
        /// Why we believe the connection dropped.
        reason: String,
    },

    /// A received or outgoing message exceeds [`MAX_MESSAGE_SIZE`].
    #[error("message too large: {size} bytes (max {max})")]
    MessageTooLarge {
        /// Actual message size in bytes.
        size: usize,
        /// Maximum allowed size.
        max: usize,
    },

    /// The Neocortex child process died or could not be spawned.
    #[error("neocortex process died: {reason}")]
    ProcessDied {
        /// Exit code or signal description.
        reason: String,
    },

    /// The client is not currently connected.
    #[error("not connected to neocortex")]
    NotConnected,

    /// Maximum restart attempts exceeded.
    #[error("max restarts ({max}) exceeded")]
    MaxRestartsExceeded {
        /// The configured maximum.
        max: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_error_display_io() {
        let err = IpcError::Io(io::Error::new(io::ErrorKind::BrokenPipe, "pipe broke"));
        let msg = format!("{err}");
        assert!(msg.contains("pipe broke"), "got: {msg}");
    }

    #[test]
    fn ipc_error_display_timeout() {
        let err = IpcError::Timeout {
            context: "connect".into(),
        };
        assert_eq!(format!("{err}"), "timeout: connect");
    }

    #[test]
    fn ipc_error_display_message_too_large() {
        let err = IpcError::MessageTooLarge {
            size: 300_000,
            max: MAX_MESSAGE_SIZE,
        };
        let msg = format!("{err}");
        assert!(msg.contains("300000"), "got: {msg}");
        assert!(msg.contains("262144"), "got: {msg}");
    }

    #[test]
    fn ipc_error_display_connection_lost() {
        let err = IpcError::ConnectionLost {
            reason: "eof".into(),
        };
        assert!(format!("{err}").contains("eof"));
    }

    #[test]
    fn ipc_error_display_process_died() {
        let err = IpcError::ProcessDied {
            reason: "exit code 137".into(),
        };
        assert!(format!("{err}").contains("137"));
    }

    #[test]
    fn ipc_error_display_not_connected() {
        let err = IpcError::NotConnected;
        assert!(format!("{err}").contains("not connected"));
    }

    #[test]
    fn ipc_error_from_io() {
        let io_err = io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
        let ipc_err: IpcError = io_err.into();
        assert!(matches!(ipc_err, IpcError::Io(_)));
    }
}
