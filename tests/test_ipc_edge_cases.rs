//! Tests for IPC protocol edge cases.
//!
//! Covers: malformed frames, timeout handling, encoding errors,
//! connection loss simulation, and boundary conditions.

#[cfg(test)]
mod malformed_frames {
    use aura_daemon::ipc::protocol::{decode_frame, encode_frame};
    use aura_types::ipc::{DaemonToNeocortex, MAX_MESSAGE_SIZE};

    #[test]
    fn test_decode_empty_buffer() {
        let result = decode_frame::<DaemonToNeocortex>(&[]);
        assert!(result.is_err(), "empty buffer should fail to decode");
    }

    #[test]
    fn test_decode_truncated_header() {
        // Only 2 bytes instead of 4 for length prefix
        let result = decode_frame::<DaemonToNeocortex>(&[0x00, 0x01]);
        assert!(result.is_err(), "truncated header should fail");
    }

    #[test]
    fn test_decode_body_shorter_than_header() {
        // Header says 100 bytes, but only 5 bytes of body
        let mut buf = (100u32).to_le_bytes().to_vec();
        buf.extend_from_slice(b"short");
        let result = decode_frame::<DaemonToNeocortex>(&buf);
        assert!(
            result.is_err(),
            "body shorter than header length should fail"
        );
    }

    #[test]
    fn test_decode_invalid_json_body() {
        let body = b"not valid json";
        let len = (body.len() as u32).to_le_bytes();
        let mut buf = len.to_vec();
        buf.extend_from_slice(body);
        let result = decode_frame::<DaemonToNeocortex>(&buf);
        assert!(result.is_err(), "invalid JSON body should fail");
    }

    #[test]
    fn test_decode_zero_length_body() {
        let len = (0u32).to_le_bytes();
        let result = decode_frame::<DaemonToNeocortex>(&len.to_vec());
        assert!(result.is_err(), "zero-length body should fail");
    }

    #[test]
    fn test_encode_decode_ping_roundtrip() {
        let msg = DaemonToNeocortex::Ping;
        let encoded = encode_frame(&msg).expect("encode Ping");
        let decoded = decode_frame::<DaemonToNeocortex>(&encoded).expect("decode Ping");
        assert!(matches!(decoded, DaemonToNeocortex::Ping));
    }

    #[test]
    fn test_encode_decode_cancel_roundtrip() {
        let msg = DaemonToNeocortex::Cancel;
        let encoded = encode_frame(&msg).expect("encode Cancel");
        let decoded = decode_frame::<DaemonToNeocortex>(&encoded).expect("decode Cancel");
        assert!(matches!(decoded, DaemonToNeocortex::Cancel));
    }

    #[test]
    fn test_encode_decode_unload_roundtrip() {
        let msg = DaemonToNeocortex::Unload;
        let encoded = encode_frame(&msg).expect("encode Unload");
        let decoded = decode_frame::<DaemonToNeocortex>(&encoded).expect("decode Unload");
        assert!(matches!(decoded, DaemonToNeocortex::Unload));
    }

    #[test]
    fn test_encode_large_but_valid_message() {
        // Create a message close to but under the limit
        let text = "x".repeat(MAX_MESSAGE_SIZE - 1024);
        let msg = DaemonToNeocortex::Embed { text };
        let encoded = encode_frame(&msg);
        assert!(
            encoded.is_ok(),
            "message under MAX_MESSAGE_SIZE should encode"
        );
        assert!(encoded.unwrap().len() <= MAX_MESSAGE_SIZE);
    }
}

#[cfg(test)]
mod timeout_handling {
    use aura_types::ipc::REQUEST_TIMEOUT;
    use std::time::Duration;

    #[test]
    fn test_request_timeout_is_30_seconds() {
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn test_request_timeout_not_zero() {
        assert!(REQUEST_TIMEOUT > Duration::ZERO);
    }

    #[test]
    fn test_request_timeout_reasonable_range() {
        assert!(
            REQUEST_TIMEOUT >= Duration::from_secs(5)
                && REQUEST_TIMEOUT <= Duration::from_secs(300),
            "REQUEST_TIMEOUT should be between 5s and 5min, got {:?}",
            REQUEST_TIMEOUT
        );
    }
}

#[cfg(test)]
mod connection_error_simulation {
    use aura_daemon::ipc::IpcError;
    use std::io;

    #[test]
    fn test_ipc_error_io_from_connection_refused() {
        let io_err = io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused");
        let ipc_err: IpcError = io_err.into();
        assert!(matches!(ipc_err, IpcError::Io(_)));
    }

    #[test]
    fn test_ipc_error_io_from_timed_out() {
        let io_err = io::Error::new(io::ErrorKind::TimedOut, "operation timed out");
        let ipc_err: IpcError = io_err.into();
        assert!(matches!(ipc_err, IpcError::Io(_)));
    }

    #[test]
    fn test_ipc_error_encoding() {
        let err = IpcError::Encoding("bincode failed".into());
        let msg = format!("{err}");
        assert!(msg.contains("bincode failed"));
    }

    #[test]
    fn test_ipc_error_max_restarts() {
        let err = IpcError::MaxRestartsExceeded { max: 5 };
        let msg = format!("{err}");
        assert!(msg.contains("5"));
        assert!(msg.contains("max restarts"));
    }

    #[test]
    fn test_ipc_error_not_connected_display() {
        let err = IpcError::NotConnected;
        let msg = format!("{err}");
        assert!(msg.contains("not connected"));
    }
}

#[cfg(test)]
mod neocortex_process {
    use aura_daemon::ipc::spawn::NeocortexProcess;

    #[test]
    fn test_neocortex_process_creation() {
        // On host, this should create a process handle (may fail to spawn)
        let result = NeocortexProcess::spawn();
        // Either Ok or Err is fine — we're testing it doesn't panic
        match result {
            Ok(_) => {}
            Err(e) => {
                let msg = format!("{e}");
                assert!(!msg.is_empty());
            }
        }
    }
}
