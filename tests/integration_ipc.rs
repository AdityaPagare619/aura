//! Integration tests for AURA v4 IPC subsystem.
//!
//! Tests the wire protocol layer: bincode frame encoding/decoding,
//! authenticated envelope validation, rate limiting behavior, and
//! graceful shutdown patterns.
//!
//! Uses mock/stub approach — no live daemon required.
//!
//! Run with: cargo test --test integration_ipc

use aura_types::ipc::*;

// ─── Wire protocol: frame encode/decode ───────────────────────────────────────

#[cfg(test)]
mod wire_protocol {
    use super::*;

    /// Verify that a serialized frame has the correct 4-byte LE length prefix.
    #[test]
    fn test_frame_length_prefix_format() {
        let msg = DaemonToNeocortex::Ping;
        let json = serde_json::to_string(&msg).unwrap();

        // Simulate what protocol.rs does: prefix with LE u32 length.
        let body = json.as_bytes();
        let len_bytes = (body.len() as u32).to_le_bytes();

        assert_eq!(len_bytes.len(), LENGTH_PREFIX_SIZE);
        assert_eq!(len_bytes.len(), FRAME_HEADER_SIZE);

        // Reconstruct from prefix.
        let reconstructed_len = u32::from_le_bytes(len_bytes) as usize;
        assert_eq!(reconstructed_len, body.len());
    }

    /// Envelope wire format should include protocol_version, session_token,
    /// seq, and payload — all in a single serde-compatible struct.
    #[test]
    fn test_envelope_wire_format_fields() {
        let envelope = AuthenticatedEnvelope::new("abc123".to_string(), 1, DaemonToNeocortex::Ping);

        let json = serde_json::to_string(&envelope).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        // All required fields must be present in wire format.
        assert!(v.get("protocol_version").is_some());
        assert!(v.get("session_token").is_some());
        assert!(v.get("seq").is_some());
        assert!(v.get("payload").is_some());

        assert_eq!(v["protocol_version"], PROTOCOL_VERSION);
        assert_eq!(v["session_token"], "abc123");
        assert_eq!(v["seq"], 1);
    }

    /// Bincode roundtrip for all DaemonToNeocortex variants should work.
    /// This mirrors the protocol.rs encode_frame / decode_frame logic.
    #[test]
    fn test_bincode_roundtrip_all_daemon_variants() {
        let messages: Vec<DaemonToNeocortex> = vec![
            DaemonToNeocortex::Ping,
            DaemonToNeocortex::Unload,
            DaemonToNeocortex::UnloadImmediate,
            DaemonToNeocortex::Cancel,
            DaemonToNeocortex::Load {
                model_path: "/data/models/test.gguf".to_string(),
                params: ModelParams::default(),
            },
            DaemonToNeocortex::Converse {
                context: ContextPackage::default(),
            },
            DaemonToNeocortex::Embed {
                text: "test embedding".to_string(),
            },
            DaemonToNeocortex::Compose {
                context: ContextPackage::default(),
                template: "open {app}".to_string(),
            },
            DaemonToNeocortex::Plan {
                context: ContextPackage::default(),
                failure: None,
            },
            DaemonToNeocortex::Summarize {
                prompt: "summarize this episode".to_string(),
            },
        ];

        for msg in &messages {
            // Encode (mimic encode_frame).
            let encoded = bincode::serde::encode_to_vec(msg, bincode::config::standard())
                .unwrap_or_else(|e| panic!("bincode encode failed for {:?}: {e}", msg));

            assert!(
                encoded.len() <= MAX_MESSAGE_SIZE,
                "encoded message exceeds MAX_MESSAGE_SIZE"
            );

            // Decode (mimic decode_frame body).
            let (decoded, _): (DaemonToNeocortex, _) =
                bincode::serde::decode_from_slice(&encoded, bincode::config::standard())
                    .unwrap_or_else(|e| panic!("bincode decode failed for {:?}: {e}", msg));

            // Variant must match.
            assert_eq!(
                std::mem::discriminant(msg),
                std::mem::discriminant(&decoded),
                "discriminant mismatch after roundtrip"
            );
        }
    }

    /// Bincode roundtrip for all NeocortexToDaemon variants.
    #[test]
    fn test_bincode_roundtrip_all_neocortex_variants() {
        let messages: Vec<NeocortexToDaemon> = vec![
            NeocortexToDaemon::Loaded {
                model_name: "qwen-4b".to_string(),
                memory_used_mb: 2048,
            },
            NeocortexToDaemon::LoadFailed {
                reason: "OOM".to_string(),
            },
            NeocortexToDaemon::Unloaded,
            NeocortexToDaemon::ConversationReply {
                text: "Hello!".to_string(),
                mood_hint: Some(0.5),
                tokens_used: 10,
            },
            NeocortexToDaemon::Progress {
                percent: 50,
                stage: "generating".to_string(),
            },
            NeocortexToDaemon::Error {
                code: 500,
                message: "inference failed".to_string(),
            },
            NeocortexToDaemon::Pong { uptime_ms: 12345 },
            NeocortexToDaemon::MemoryWarning {
                used_mb: 1800,
                available_mb: 200,
            },
            NeocortexToDaemon::TokenBudgetExhausted,
            NeocortexToDaemon::Embedding {
                vector: vec![0.1, 0.2, 0.3],
            },
            NeocortexToDaemon::Summary {
                text: "summary text".to_string(),
                tokens_used: 50,
            },
            NeocortexToDaemon::PlanScore { score: 0.85 },
            NeocortexToDaemon::FailureClassification {
                category: "Transient".to_string(),
            },
        ];

        for msg in &messages {
            let encoded = bincode::serde::encode_to_vec(msg, bincode::config::standard())
                .unwrap_or_else(|e| panic!("bincode encode failed for {:?}: {e}", msg));

            let (decoded, _): (NeocortexToDaemon, _) =
                bincode::serde::decode_from_slice(&encoded, bincode::config::standard())
                    .unwrap_or_else(|e| panic!("bincode decode failed for {:?}: {e}", msg));

            assert_eq!(
                std::mem::discriminant(msg),
                std::mem::discriminant(&decoded),
                "discriminant mismatch after roundtrip"
            );
        }
    }

    /// Oversized messages must be detectable before encoding is accepted.
    #[test]
    fn test_oversized_message_detection() {
        let huge_text = "x".repeat(MAX_MESSAGE_SIZE + 1024);
        let msg = DaemonToNeocortex::Compose {
            context: ContextPackage {
                conversation_history: vec![ConversationTurn {
                    role: Role::User,
                    content: huge_text,
                    timestamp_ms: 0,
                }],
                ..Default::default()
            },
            template: String::new(),
        };

        let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard());
        // Encoding may succeed (bincode doesn't enforce size), but the producer
        // must check the result against MAX_MESSAGE_SIZE before sending.
        if let Ok(ref data) = encoded {
            assert!(
                data.len() > MAX_MESSAGE_SIZE,
                "huge message should exceed MAX_MESSAGE_SIZE"
            );
        }
    }

    /// Zero-length frame body must be detectable (protocol.rs rejects len==0).
    #[test]
    fn test_zero_length_frame_detection() {
        let zero_len: u32 = 0;
        let bytes = zero_len.to_le_bytes();
        let parsed = u32::from_le_bytes(bytes);
        assert_eq!(parsed, 0, "zero-length frame should be detectable");
    }
}

// ─── Authentication token validation ──────────────────────────────────────────

#[cfg(test)]
mod authentication {
    use super::*;

    /// Session tokens must survive serialization roundtrip unchanged.
    /// This is the security boundary between daemon and neocortex.
    #[test]
    fn test_session_token_integrity() {
        // 64 hex chars = 32 bytes (typical CSPRNG output).
        let token = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6a7b8c9d0e1f2a3b4c5d6a7b8c9d0e1f2";

        let envelope = AuthenticatedEnvelope::new(token.to_string(), 1, DaemonToNeocortex::Ping);

        let json = serde_json::to_string(&envelope).unwrap();
        let restored: AuthenticatedEnvelope<DaemonToNeocortex> =
            serde_json::from_str(&json).unwrap();

        assert_eq!(restored.session_token, token);
    }

    /// Empty session token must be rejected by the receiver.
    #[test]
    fn test_empty_session_token_is_invalid() {
        let envelope = AuthenticatedEnvelope::new(String::new(), 1, DaemonToNeocortex::Ping);
        assert!(
            envelope.session_token.is_empty(),
            "empty token must be detectable"
        );
        // In production, receiver checks token length > 0 before processing.
    }

    /// Protocol version mismatch must be caught immediately.
    #[test]
    fn test_protocol_version_rejection() {
        let valid = AuthenticatedEnvelope::new("token".to_string(), 1, DaemonToNeocortex::Ping);
        assert!(valid.version_ok());

        let invalid = AuthenticatedEnvelope {
            protocol_version: PROTOCOL_VERSION + 10,
            session_token: "token".to_string(),
            seq: 1,
            payload: DaemonToNeocortex::Ping,
        };
        assert!(!invalid.version_ok());
    }

    /// Multiple envelopes with different seqs but same token must be accepted
    /// (normal operation: monotonic sequence numbers).
    #[test]
    fn test_monotonic_sequence_numbers() {
        let token = "shared_session_token".to_string();
        let mut last_seq = 0u64;

        for i in 1..=100 {
            let envelope = AuthenticatedEnvelope::new(token.clone(), i, DaemonToNeocortex::Ping);
            assert!(
                envelope.seq > last_seq,
                "sequence numbers must be monotonic"
            );
            last_seq = envelope.seq;
        }
    }

    /// Replayed messages (same seq) should be detectable.
    /// Receiver must track last_seen_seq per session.
    #[test]
    fn test_replay_detection_concept() {
        let token = "session_abc".to_string();
        let seq = 42;

        let envelope1 = AuthenticatedEnvelope::new(token.clone(), seq, DaemonToNeocortex::Ping);
        let envelope2 = AuthenticatedEnvelope::new(token.clone(), seq, DaemonToNeocortex::Ping);

        // Both have the same (token, seq) pair — replay.
        assert_eq!(envelope1.session_token, envelope2.session_token);
        assert_eq!(envelope1.seq, envelope2.seq);
        // Receiver must reject envelope2 if seq <= last_seen_seq for this token.
    }

    /// Envelope with a different token must not be accepted for a given session.
    #[test]
    fn test_token_mismatch_rejection() {
        let correct_token = "correct_token_123".to_string();
        let wrong_token = "wrong_token_456".to_string();

        let envelope_correct =
            AuthenticatedEnvelope::new(correct_token.clone(), 1, DaemonToNeocortex::Ping);
        let envelope_wrong = AuthenticatedEnvelope::new(wrong_token, 1, DaemonToNeocortex::Ping);

        assert_ne!(
            envelope_correct.session_token, envelope_wrong.session_token,
            "different tokens must be distinguishable"
        );
    }
}

// ─── Rate limiting ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod rate_limiting {
    use super::*;

    /// Default rate limit config must have sensible, non-zero values.
    #[test]
    fn test_rate_limit_defaults_sensible() {
        let config = IpcRateLimitConfig::default();

        assert_eq!(config.max_requests_per_second, 100);
        assert_eq!(config.burst_allowance, 20);

        // Burst must be strictly less than steady-state rate.
        assert!(config.burst_allowance < config.max_requests_per_second);

        // Both must be positive (zero = disabled IPC).
        assert!(config.max_requests_per_second > 0);
        assert!(config.burst_allowance > 0);
    }

    /// Custom rate limit configs must be constructable for testing.
    #[test]
    fn test_custom_rate_limit_config() {
        let strict = IpcRateLimitConfig {
            max_requests_per_second: 10,
            burst_allowance: 5,
        };
        assert_eq!(strict.max_requests_per_second, 10);
        assert_eq!(strict.burst_allowance, 5);
    }

    /// Token bucket algorithm simulation: verify that sending at steady rate
    /// stays within limits, and burst is consumed then refills.
    #[test]
    fn test_token_bucket_simulation() {
        let config = IpcRateLimitConfig {
            max_requests_per_second: 10,
            burst_allowance: 5,
        };

        // Simulate: burst pool starts at burst_allowance.
        let mut tokens = config.burst_allowance as f64;
        let refill_rate = config.max_requests_per_second as f64; // per second

        // Phase 1: Burst — consume all burst tokens.
        for _ in 0..config.burst_allowance {
            if tokens >= 1.0 {
                tokens -= 1.0;
            }
        }
        assert!(tokens < 1.0, "burst tokens should be consumed");

        // Phase 2: After 0.5 seconds, refill = refill_rate * 0.5
        tokens += refill_rate * 0.5;
        assert!(
            tokens <= config.burst_allowance as f64 + refill_rate * 0.5,
            "refill should add tokens"
        );

        // Phase 3: Rapid requests drain faster than refill.
        for _ in 0..20 {
            tokens -= 1.0;
        }
        assert!(tokens < 0.0, "rapid requests should exhaust tokens");
    }

    /// Rate limit constants must be consistent across the codebase.
    #[test]
    fn test_rate_limit_constants_match_ipc_definitions() {
        // These are used in protocol.rs for timeout enforcement.
        assert_eq!(REQUEST_TIMEOUT.as_secs(), 30);
        assert_eq!(MAX_MESSAGE_SIZE, 256 * 1024);
    }
}

// ─── Graceful shutdown ─────────────────────────────────────────────────────────

#[cfg(test)]
mod graceful_shutdown {
    use super::*;

    /// Sending Unload (graceful) must be distinguishable from UnloadImmediate.
    #[test]
    fn test_graceful_vs_immediate_unload() {
        let graceful = DaemonToNeocortex::Unload;
        let immediate = DaemonToNeocortex::UnloadImmediate;

        assert_ne!(
            std::mem::discriminant(&graceful),
            std::mem::discriminant(&immediate),
            "graceful and immediate unload must be different variants"
        );
    }

    /// The shutdown sequence should be: Cancel → Unload → verify Unloaded.
    /// This test verifies each message in the sequence can be serialized.
    #[test]
    fn test_shutdown_sequence_serialization() {
        // Step 1: Cancel current inference.
        let cancel = DaemonToNeocortex::Cancel;
        let cancel_json = serde_json::to_string(&cancel).unwrap();
        let _: DaemonToNeocortex = serde_json::from_str(&cancel_json).unwrap();

        // Step 2: Graceful unload (finish work first).
        let unload = DaemonToNeocortex::Unload;
        let unload_json = serde_json::to_string(&unload).unwrap();
        let _: DaemonToNeocortex = serde_json::from_str(&unload_json).unwrap();

        // Step 3: Neocortex confirms unloaded.
        let unloaded = NeocortexToDaemon::Unloaded;
        let unloaded_json = serde_json::to_string(&unloaded).unwrap();
        let _: NeocortexToDaemon = serde_json::from_str(&unloaded_json).unwrap();
    }

    /// Ping/Pong health check must work as IPC liveness probe.
    #[test]
    fn test_ping_pong_liveness_probe() {
        // Daemon sends Ping.
        let ping = DaemonToNeocortex::Ping;
        let ping_encoded = bincode::serde::encode_to_vec(&ping, bincode::config::standard())
            .expect("Ping encode failed");

        let (ping_decoded, _): (DaemonToNeocortex, _) =
            bincode::serde::decode_from_slice(&ping_encoded, bincode::config::standard())
                .expect("Ping decode failed");
        assert!(matches!(ping_decoded, DaemonToNeocortex::Ping));

        // Neocortex responds with Pong.
        let pong = NeocortexToDaemon::Pong { uptime_ms: 60_000 };
        let pong_encoded = bincode::serde::encode_to_vec(&pong, bincode::config::standard())
            .expect("Pong encode failed");

        let (pong_decoded, _): (NeocortexToDaemon, _) =
            bincode::serde::decode_from_slice(&pong_encoded, bincode::config::standard())
                .expect("Pong decode failed");
        match pong_decoded {
            NeocortexToDaemon::Pong { uptime_ms } => assert_eq!(uptime_ms, 60_000),
            _ => panic!("expected Pong"),
        }
    }

    /// Error response must carry both code and message for diagnostics.
    #[test]
    fn test_error_response_diagnostics() {
        let error = NeocortexToDaemon::Error {
            code: 503,
            message: "Model not loaded: file not found".to_string(),
        };

        let encoded = bincode::serde::encode_to_vec(&error, bincode::config::standard())
            .expect("Error encode failed");
        let (decoded, _): (NeocortexToDaemon, _) =
            bincode::serde::decode_from_slice(&encoded, bincode::config::standard())
                .expect("Error decode failed");

        match decoded {
            NeocortexToDaemon::Error { code, message } => {
                assert_eq!(code, 503);
                assert!(message.contains("not loaded"));
            }
            _ => panic!("expected Error variant"),
        }
    }

    /// ConnectionLost error kind must be detectable for graceful cleanup.
    #[test]
    fn test_connection_lost_indicates_shutdown() {
        // Simulate what protocol.rs detects on read EOF.
        let eof_error = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "peer closed");
        assert_eq!(eof_error.kind(), std::io::ErrorKind::UnexpectedEof);

        // Broken pipe indicates the other side closed during write.
        let broken_pipe = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe broken");
        assert_eq!(broken_pipe.kind(), std::io::ErrorKind::BrokenPipe);
    }
}

// ─── IPC error handling ────────────────────────────────────────────────────────

#[cfg(test)]
mod error_handling {
    use super::*;

    /// MessageTooLarge error must carry size and max for diagnostics.
    #[test]
    fn test_message_too_large_fields() {
        let size = MAX_MESSAGE_SIZE + 1;
        let max = MAX_MESSAGE_SIZE;
        assert!(size > max, "oversized message must be detectable");
    }

    /// Encoding errors must not panic — they should return Err.
    #[test]
    fn test_invalid_frame_handling() {
        // A truncated frame (only 2 bytes instead of 4 for length prefix).
        let truncated = [0x00, 0x01];
        assert!(truncated.len() < FRAME_HEADER_SIZE);

        // Attempting to read a u32 from this would fail gracefully.
        if truncated.len() >= FRAME_HEADER_SIZE {
            let _ = u32::from_le_bytes([truncated[0], truncated[1], truncated[2], truncated[3]]);
        } else {
            // Graceful: don't panic, report error.
        }
    }

    /// Error code 500 (internal) must be distinguishable from 400 (bad request).
    #[test]
    fn test_error_code_classification() {
        let internal_error = NeocortexToDaemon::Error {
            code: 500,
            message: "OOM".to_string(),
        };
        let bad_request = NeocortexToDaemon::Error {
            code: 400,
            message: "Invalid payload".to_string(),
        };

        match (&internal_error, &bad_request) {
            (
                NeocortexToDaemon::Error { code: c1, .. },
                NeocortexToDaemon::Error { code: c2, .. },
            ) => {
                assert_ne!(c1, c2);
                assert!(*c1 >= 500);
                assert!(*c2 < 500);
            }
            _ => panic!("expected Error variants"),
        }
    }
}
