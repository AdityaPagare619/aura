//! Wire protocol for AURA daemon ↔ Neocortex IPC.
//!
//! Framing: `[4-byte LE length][bincode payload]`
//!
//! Both encode and decode enforce [`MAX_MESSAGE_SIZE`] to prevent OOM on
//! malformed data.  Serialization uses `bincode 2.0` with
//! `bincode::config::standard()` and the serde compatibility layer, matching
//! the server-side implementation in `aura-neocortex`.

use std::time::Duration;

use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::debug;

use super::IpcError;

// ─── Platform-specific stream type ──────────────────────────────────────────────────

/// On Android, the daemon↔neocortex link uses Unix domain sockets (abstract
/// namespace) for lower latency and no TCP overhead.
///
/// On all other platforms (Linux desktop, macOS, Windows) the link uses TCP
/// to `127.0.0.1:19400`, matching the neocortex server's `TcpListener`.
#[cfg(target_os = "android")]
pub type IpcStream = tokio::net::UnixStream;

#[cfg(not(target_os = "android"))]
pub type IpcStream = tokio::net::TcpStream;

// ─── Constants ──────────────────────────────────────────────────────────

/// Abstract socket address used on Android / Linux.
///
/// On Linux the leading `@` denotes an abstract namespace address (the kernel
/// replaces it with a NUL byte).  On non-Unix platforms this constant is
/// defined but unused; we connect via TCP instead.
pub const SOCKET_ADDR: &str = "@aura_ipc_v4";

/// TCP fallback address for host-side development (Windows / macOS without
/// Unix socket support).
pub const TCP_FALLBACK_ADDR: &str = "127.0.0.1";

/// TCP fallback port.
pub const TCP_FALLBACK_PORT: u16 = 19400;

/// Maximum allowed message payload size (256 KB).
pub const MAX_MESSAGE_SIZE: usize = 256 * 1024;

/// Timeout for establishing the initial connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Timeout for a single request→response round-trip.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Size of the length-prefix header (u32 little-endian).
pub const FRAME_HEADER_SIZE: usize = 4;

// ─── Encoding ───────────────────────────────────────────────────────────

/// Serialize `msg` into a length-prefixed frame ready for sending.
///
/// Returns a `Vec<u8>` consisting of a 4-byte little-endian length header
/// followed by the bincode-encoded payload.
///
/// # Errors
///
/// - [`IpcError::Encoding`] if bincode serialization fails.
/// - [`IpcError::MessageTooLarge`] if the serialized body exceeds [`MAX_MESSAGE_SIZE`].
pub fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>, IpcError> {
    let body = bincode::serde::encode_to_vec(msg, bincode::config::standard())
        .map_err(|e| IpcError::Encoding(format!("bincode serialize failed: {e}")))?;

    if body.len() > MAX_MESSAGE_SIZE {
        return Err(IpcError::MessageTooLarge {
            size: body.len(),
            max: MAX_MESSAGE_SIZE,
        });
    }

    let len = body.len() as u32;
    let mut frame = Vec::with_capacity(FRAME_HEADER_SIZE + body.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&body);

    debug!(
        payload_len = body.len(),
        frame_len = frame.len(),
        "encoded frame"
    );
    Ok(frame)
}

/// Read and deserialize a single length-prefixed frame from `stream`.
///
/// # Errors
///
/// - [`IpcError::Io`] on read failure or unexpected EOF.
/// - [`IpcError::MessageTooLarge`] if the declared length exceeds [`MAX_MESSAGE_SIZE`].
/// - [`IpcError::Encoding`] if bincode deserialization fails.
/// - [`IpcError::ConnectionLost`] if the stream yields zero bytes (clean disconnect).
pub async fn decode_frame<T: DeserializeOwned>(stream: &mut IpcStream) -> Result<T, IpcError> {
    // Read the 4-byte length prefix.
    let mut len_buf = [0u8; FRAME_HEADER_SIZE];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {},
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(IpcError::ConnectionLost {
                reason: "peer closed connection (EOF during header read)".into(),
            });
        },
        Err(e) => return Err(IpcError::Io(e)),
    }

    let msg_len = u32::from_le_bytes(len_buf) as usize;

    if msg_len == 0 {
        return Err(IpcError::Encoding("zero-length message".into()));
    }

    if msg_len > MAX_MESSAGE_SIZE {
        return Err(IpcError::MessageTooLarge {
            size: msg_len,
            max: MAX_MESSAGE_SIZE,
        });
    }

    // Read the payload body.
    let mut body = vec![0u8; msg_len];
    match stream.read_exact(&mut body).await {
        Ok(_) => {},
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(IpcError::ConnectionLost {
                reason: format!(
                    "peer closed connection (EOF during body read, expected {msg_len} bytes)"
                ),
            });
        },
        Err(e) => return Err(IpcError::Io(e)),
    }

    // Deserialize.
    let (msg, _) = bincode::serde::decode_from_slice(&body, bincode::config::standard())
        .map_err(|e| IpcError::Encoding(format!("bincode deserialize failed: {e}")))?;

    debug!(payload_len = msg_len, "decoded frame");
    Ok(msg)
}

/// Write an already-encoded frame to the stream, flushing afterward.
///
/// # Errors
///
/// - [`IpcError::Io`] if the write or flush fails.
/// - [`IpcError::ConnectionLost`] on broken-pipe / reset errors.
pub async fn write_frame(stream: &mut IpcStream, frame: &[u8]) -> Result<(), IpcError> {
    match stream.write_all(frame).await {
        Ok(()) => {},
        Err(e)
            if e.kind() == std::io::ErrorKind::BrokenPipe
                || e.kind() == std::io::ErrorKind::ConnectionReset =>
        {
            return Err(IpcError::ConnectionLost {
                reason: format!("write failed: {e}"),
            });
        },
        Err(e) => return Err(IpcError::Io(e)),
    }
    stream.flush().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::BrokenPipe
            || e.kind() == std::io::ErrorKind::ConnectionReset
        {
            IpcError::ConnectionLost {
                reason: format!("flush failed: {e}"),
            }
        } else {
            IpcError::Io(e)
        }
    })?;
    Ok(())
}

/// Establish a platform-appropriate async stream to the Neocortex process.
///
/// - **Android:** Unix domain socket at abstract address `@aura_ipc_v4`.
/// - **Non-Android (Linux, macOS, Windows):** TCP to `127.0.0.1:19400`.
///
/// The connection attempt is bounded by [`CONNECT_TIMEOUT`].
///
/// # Errors
///
/// - [`IpcError::Timeout`] if the connection is not established within [`CONNECT_TIMEOUT`].
/// - [`IpcError::Io`] for other connection failures.
pub async fn connect_stream() -> Result<IpcStream, IpcError> {
    // Android: abstract Unix domain socket.
    // On target_os = "android", SocketAddrExt lives in std::os::android::net
    // (NOT std::os::linux -- Android has its own target triple).
    #[cfg(target_os = "android")]
    {
        use std::os::{android::net::SocketAddrExt, unix::net::SocketAddr as StdSocketAddr};

        let addr = StdSocketAddr::from_abstract_name(b"aura_ipc_v4").map_err(|e| {
            IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid abstract socket name: {e}"),
            ))
        })?;

        let stream = tokio::time::timeout(CONNECT_TIMEOUT, async {
            let std_stream = std::os::unix::net::UnixStream::connect_addr(&addr)?;
            std_stream.set_nonblocking(true)?;
            tokio::net::UnixStream::from_std(std_stream)
        })
        .await
        .map_err(|_| IpcError::Timeout {
            context: format!("connect to {SOCKET_ADDR} timed out after {CONNECT_TIMEOUT:?}"),
        })?
        .map_err(IpcError::Io)?;

        Ok(stream)
    }

    // Non-Android: TCP to the standard IPC port.
    //
    // The neocortex server binds a `std::net::TcpListener` on this address.
    // On Linux desktop, macOS, and Windows, TCP is used for simplicity and
    // because the neocortex IpcHandler is built around `std::net::TcpStream`.
    #[cfg(not(target_os = "android"))]
    {
        let addr = format!("{TCP_FALLBACK_ADDR}:{TCP_FALLBACK_PORT}");

        let stream = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(&addr))
            .await
            .map_err(|_| IpcError::Timeout {
                context: format!("connect to {addr} timed out after {CONNECT_TIMEOUT:?}"),
            })?
            .map_err(IpcError::Io)?;

        // Disable Nagle's algorithm for low-latency IPC.
        stream.set_nodelay(true).map_err(IpcError::Io)?;

        Ok(stream)
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use aura_types::ipc::{DaemonToNeocortex, ModelParams, ModelTier, NeocortexToDaemon};

    use super::*;

    #[test]
    fn encode_ping_frame() {
        let frame = encode_frame(&DaemonToNeocortex::Ping).expect("encode Ping");
        // Must start with a 4-byte LE length prefix.
        assert!(frame.len() > FRAME_HEADER_SIZE);
        let payload_len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
        assert_eq!(payload_len, frame.len() - FRAME_HEADER_SIZE);
    }

    #[test]
    fn encode_decode_round_trip_ping() {
        let original = DaemonToNeocortex::Ping;
        let frame = encode_frame(&original).expect("encode");
        let body = &frame[FRAME_HEADER_SIZE..];
        let (decoded, _): (DaemonToNeocortex, _) =
            bincode::serde::decode_from_slice(body, bincode::config::standard()).expect("decode");
        assert!(matches!(decoded, DaemonToNeocortex::Ping));
    }

    #[test]
    fn encode_decode_round_trip_load() {
        let original = DaemonToNeocortex::Load {
            model_path: "/data/models/qwen-4b.gguf".into(),
            params: ModelParams {
                n_ctx: 4096,
                n_threads: 8,
                model_tier: ModelTier::Full8B,
            },
        };
        let frame = encode_frame(&original).expect("encode");
        let body = &frame[FRAME_HEADER_SIZE..];
        let (decoded, _): (DaemonToNeocortex, _) =
            bincode::serde::decode_from_slice(body, bincode::config::standard()).expect("decode");
        match decoded {
            DaemonToNeocortex::Load { model_path, params } => {
                assert_eq!(model_path, "/data/models/qwen-4b.gguf");
                assert_eq!(params.n_ctx, 4096);
                assert_eq!(params.n_threads, 8);
                assert!(matches!(params.model_tier, ModelTier::Full8B));
            },
            other => panic!("expected Load, got {other:?}"),
        }
    }

    #[test]
    fn encode_decode_round_trip_response() {
        let original = NeocortexToDaemon::Pong { uptime_ms: 42_000 };
        let frame = encode_frame(&original).expect("encode");
        let body = &frame[FRAME_HEADER_SIZE..];
        let (decoded, _): (NeocortexToDaemon, _) =
            bincode::serde::decode_from_slice(body, bincode::config::standard()).expect("decode");
        match decoded {
            NeocortexToDaemon::Pong { uptime_ms } => assert_eq!(uptime_ms, 42_000),
            other => panic!("expected Pong, got {other:?}"),
        }
    }

    #[test]
    fn encode_rejects_oversized_message() {
        // Create a message with a huge string that exceeds MAX_MESSAGE_SIZE.
        let huge = DaemonToNeocortex::Compose {
            context: aura_types::ipc::ContextPackage {
                conversation_history: vec![aura_types::ipc::ConversationTurn {
                    role: aura_types::ipc::Role::User,
                    content: "x".repeat(MAX_MESSAGE_SIZE + 1),
                    timestamp_ms: 0,
                }],
                ..Default::default()
            },
            template: String::new(),
        };
        let result = encode_frame(&huge);
        assert!(result.is_err());
        if let Err(IpcError::MessageTooLarge { size, max }) = result {
            assert!(size > MAX_MESSAGE_SIZE);
            assert_eq!(max, MAX_MESSAGE_SIZE);
        } else {
            panic!("expected MessageTooLarge error");
        }
    }

    #[test]
    fn constants_are_correct() {
        assert_eq!(MAX_MESSAGE_SIZE, 256 * 1024);
        assert_eq!(FRAME_HEADER_SIZE, 4);
        assert_eq!(CONNECT_TIMEOUT, Duration::from_secs(5));
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
        assert_eq!(TCP_FALLBACK_PORT, 19400);
    }

    #[tokio::test]
    async fn decode_frame_from_in_memory_stream() {
        // Build a valid frame in memory, pipe it through a duplex stream,
        // and verify decode_frame recovers the original message.
        let original = DaemonToNeocortex::Ping;
        let frame = encode_frame(&original).expect("encode");

        let (mut client, mut server) = tokio::io::duplex(4096);

        // Write the frame from the server side.
        let write_handle = tokio::spawn(async move {
            server.write_all(&frame).await.expect("write");
            server.flush().await.expect("flush");
            // Keep `server` alive briefly so the reader doesn't get EOF before
            // reading the frame.
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(server);
        });

        // Wrap the client half in our IpcStream type (TcpStream on non-unix,
        // but duplex is a DuplexStream — so we test via raw read/write).
        // Since duplex isn't our IpcStream, test the decode logic manually.
        let mut len_buf = [0u8; FRAME_HEADER_SIZE];
        client.read_exact(&mut len_buf).await.expect("read header");
        let msg_len = u32::from_le_bytes(len_buf) as usize;
        assert!(msg_len <= MAX_MESSAGE_SIZE);

        let mut body = vec![0u8; msg_len];
        client.read_exact(&mut body).await.expect("read body");

        let (decoded, _): (DaemonToNeocortex, _) =
            bincode::serde::decode_from_slice(&body, bincode::config::standard()).expect("decode");
        assert!(matches!(decoded, DaemonToNeocortex::Ping));

        write_handle.await.expect("writer task");
    }
}
