//! Integration tests for AURA IPC subsystem.
//!
//! Tests:
//!   - IPC message serialization roundtrip
//!   - Message size limits enforced
//!   - Protocol version compatibility
//!
//! Follows the same patterns as existing integration tests in tests/.

use aura_types::ipc::*;

// ---------------------------------------------------------------------------
// IPC message serialization roundtrip
// ---------------------------------------------------------------------------

#[cfg(test)]
mod serialization_roundtrip {
    use super::*;

    /// DaemonToNeocortex::Ping should serialize and deserialize correctly.
    #[test]
    fn test_ping_roundtrip() {
        let msg = DaemonToNeocortex::Ping;
        let json = serde_json::to_string(&msg).unwrap();
        let restored: DaemonToNeocortex = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, DaemonToNeocortex::Ping));
    }

    /// DaemonToNeocortex::Load with model params roundtrip.
    #[test]
    fn test_load_roundtrip() {
        let msg = DaemonToNeocortex::Load {
            model_path: "/data/local/tmp/aura/models/qwen3-8b.gguf".to_string(),
            params: ModelParams {
                n_ctx: 4096,
                n_threads: 4,
                model_tier: ModelTier::Full8B,
            },
        };

        let json = serde_json::to_string(&msg).unwrap();
        let restored: DaemonToNeocortex = serde_json::from_str(&json).unwrap();

        match restored {
            DaemonToNeocortex::Load { model_path, params } => {
                assert_eq!(model_path, "/data/local/tmp/aura/models/qwen3-8b.gguf");
                assert_eq!(params.n_ctx, 4096);
                assert_eq!(params.n_threads, 4);
                assert_eq!(params.model_tier, ModelTier::Full8B);
            }
            _ => panic!("expected Load variant"),
        }
    }

    /// DaemonToNeocortex::Plan with full ContextPackage roundtrip.
    #[test]
    fn test_plan_roundtrip() {
        let mut ctx = ContextPackage::default();
        ctx.conversation_history.push(ConversationTurn {
            role: Role::User,
            content: "Open WhatsApp and message Alice".to_string(),
            timestamp_ms: 1_700_000_000_000,
        });
        ctx.memory_snippets.push(MemorySnippet {
            content: "Alice is a close friend".to_string(),
            source: MemoryTier::Semantic,
            relevance: 0.9,
            timestamp_ms: 1_699_900_000_000,
        });
        ctx.inference_mode = InferenceMode::Planner;

        let msg = DaemonToNeocortex::Plan {
            context: ctx,
            failure: None,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let restored: DaemonToNeocortex = serde_json::from_str(&json).unwrap();

        match restored {
            DaemonToNeocortex::Plan { context, failure } => {
                assert_eq!(context.conversation_history.len(), 1);
                assert_eq!(
                    context.conversation_history[0].content,
                    "Open WhatsApp and message Alice"
                );
                assert_eq!(context.memory_snippets.len(), 1);
                assert!(failure.is_none());
            }
            _ => panic!("expected Plan variant"),
        }
    }

    /// NeocortexToDaemon::ConversationReply roundtrip.
    #[test]
    fn test_conversation_reply_roundtrip() {
        let msg = NeocortexToDaemon::ConversationReply {
            text: "Sure, I'll open WhatsApp for you!".to_string(),
            mood_hint: Some(0.75),
            tokens_used: 42,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let restored: NeocortexToDaemon = serde_json::from_str(&json).unwrap();

        match restored {
            NeocortexToDaemon::ConversationReply {
                text,
                mood_hint,
                tokens_used,
            } => {
                assert_eq!(text, "Sure, I'll open WhatsApp for you!");
                assert!((mood_hint.unwrap() - 0.75).abs() < f32::EPSILON);
                assert_eq!(tokens_used, 42);
            }
            _ => panic!("expected ConversationReply"),
        }
    }

    /// NeocortexToDaemon::PlanReady roundtrip.
    #[test]
    fn test_plan_ready_roundtrip() {
        use aura_types::actions::ActionType;
        use aura_types::dsl::{DslStep, FailureStrategy};
        use aura_types::etg::{ActionPlan, PlanSource};

        let plan = ActionPlan {
            goal_description: "Open WhatsApp".to_string(),
            steps: vec![DslStep {
                action: ActionType::Tap { x: 100, y: 200 },
                target: None,
                timeout_ms: 5000,
                on_failure: FailureStrategy::Skip,
                precondition: None,
                postcondition: None,
                label: Some("tap icon".to_string()),
            }],
            estimated_duration_ms: 5000,
            confidence: 0.85,
            source: PlanSource::LlmGenerated,
        };

        let msg = NeocortexToDaemon::PlanReady {
            plan,
            tokens_used: 150,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let restored: NeocortexToDaemon = serde_json::from_str(&json).unwrap();

        match restored {
            NeocortexToDaemon::PlanReady { tokens_used, .. } => {
                assert_eq!(tokens_used, 150);
            }
            _ => panic!("expected PlanReady"),
        }
    }

    /// NeocortexToDaemon::Error roundtrip.
    #[test]
    fn test_error_roundtrip() {
        let msg = NeocortexToDaemon::Error {
            code: 500,
            message: "Model inference failed: out of memory".to_string(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let restored: NeocortexToDaemon = serde_json::from_str(&json).unwrap();

        match restored {
            NeocortexToDaemon::Error { code, message } => {
                assert_eq!(code, 500);
                assert!(message.contains("out of memory"));
            }
            _ => panic!("expected Error"),
        }
    }

    /// NeocortexToDaemon::MemoryWarning roundtrip.
    #[test]
    fn test_memory_warning_roundtrip() {
        let msg = NeocortexToDaemon::MemoryWarning {
            used_mb: 1800,
            available_mb: 200,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let restored: NeocortexToDaemon = serde_json::from_str(&json).unwrap();

        match restored {
            NeocortexToDaemon::MemoryWarning {
                used_mb,
                available_mb,
            } => {
                assert_eq!(used_mb, 1800);
                assert_eq!(available_mb, 200);
            }
            _ => panic!("expected MemoryWarning"),
        }
    }

    /// AuthenticatedEnvelope roundtrip with various payloads.
    #[test]
    fn test_envelope_roundtrip_various_payloads() {
        let payloads: Vec<DaemonToNeocortex> = vec![
            DaemonToNeocortex::Ping,
            DaemonToNeocortex::Unload,
            DaemonToNeocortex::UnloadImmediate,
            DaemonToNeocortex::Cancel,
            DaemonToNeocortex::Converse {
                context: ContextPackage::default(),
            },
            DaemonToNeocortex::Embed {
                text: "embed this".to_string(),
            },
        ];

        for (i, payload) in payloads.into_iter().enumerate() {
            let envelope = AuthenticatedEnvelope::new(format!("token_{:04}", i), i as u64, payload);

            let json = serde_json::to_string(&envelope)
                .unwrap_or_else(|e| panic!("serialize payload {} failed: {}", i, e));

            let restored: AuthenticatedEnvelope<DaemonToNeocortex> = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("deserialize payload {} failed: {}", i, e));

            assert_eq!(restored.protocol_version, PROTOCOL_VERSION);
            assert_eq!(restored.seq, i as u64);
        }
    }

    /// ProactiveTrigger variants roundtrip correctly.
    #[test]
    fn test_proactive_trigger_roundtrip() {
        let triggers = vec![
            ProactiveTrigger::GoalStalled {
                goal_id: 42,
                title: "Learn Rust".to_string(),
                stalled_days: 7,
            },
            ProactiveTrigger::SocialGap {
                contact_name: "Alice".to_string(),
                days_since_contact: 14,
            },
            ProactiveTrigger::HealthAlert {
                metric: "battery_level".to_string(),
                value: 10.0,
                threshold: 15.0,
            },
            ProactiveTrigger::MemoryInsight {
                summary: "User always checks email at 9am".to_string(),
            },
        ];

        for trigger in triggers {
            let json = serde_json::to_string(&trigger).unwrap();
            let _: ProactiveTrigger = serde_json::from_str(&json).unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// Message size limits enforced
// ---------------------------------------------------------------------------

#[cfg(test)]
mod message_size_limits {
    use super::*;

    /// MAX_MESSAGE_SIZE constant should be 256 KB.
    #[test]
    fn test_max_message_size_constant() {
        assert_eq!(MAX_MESSAGE_SIZE, 256 * 1024);
    }

    /// LENGTH_PREFIX_SIZE should be 4 bytes (u32).
    #[test]
    fn test_length_prefix_size() {
        assert_eq!(LENGTH_PREFIX_SIZE, 4);
        assert_eq!(FRAME_HEADER_SIZE, 4);
    }

    /// ContextPackage::MAX_SIZE should be 64 KB.
    #[test]
    fn test_context_package_max_size() {
        assert_eq!(ContextPackage::MAX_SIZE, 65536);
    }

    /// Empty context package should have a small estimated size.
    #[test]
    fn test_empty_context_package_size() {
        let ctx = ContextPackage::default();
        let size = ctx.estimated_size();
        assert!(
            size < 500,
            "empty context package should be under 500 bytes, got {}",
            size
        );
    }

    /// A context package filled with max conversation history should be bounded.
    #[test]
    fn test_max_conversation_context_size() {
        let mut ctx = ContextPackage::default();

        for _ in 0..ContextPackage::MAX_CONVERSATION_HISTORY {
            ctx.conversation_history.push(ConversationTurn {
                role: Role::User,
                content: "x".repeat(200),
                timestamp_ms: 0,
            });
        }

        let size = ctx.estimated_size();
        // Max 64 turns × ~216 bytes each ≈ 13,824 bytes + overhead
        assert!(
            size < ContextPackage::MAX_SIZE,
            "max conversation context should fit in {} bytes, got {}",
            ContextPackage::MAX_SIZE,
            size
        );
    }

    /// A context package filled with max memory snippets should be bounded.
    #[test]
    fn test_max_memory_snippets_context_size() {
        let mut ctx = ContextPackage::default();

        for _ in 0..ContextPackage::MAX_MEMORY_SNIPPETS {
            ctx.memory_snippets.push(MemorySnippet {
                content: "m".repeat(500),
                source: MemoryTier::Episodic,
                relevance: 0.8,
                timestamp_ms: 0,
            });
        }

        let size = ctx.estimated_size();
        // Max 32 snippets × ~516 bytes each ≈ 16,512 bytes + overhead
        assert!(
            size < ContextPackage::MAX_SIZE,
            "max memory snippets context should fit in {} bytes, got {}",
            ContextPackage::MAX_SIZE,
            size
        );
    }

    /// ConversationTurn with empty content should still serialize.
    #[test]
    fn test_empty_conversation_turn() {
        let turn = ConversationTurn {
            role: Role::System,
            content: String::new(),
            timestamp_ms: 0,
        };
        let json = serde_json::to_string(&turn).unwrap();
        let restored: ConversationTurn = serde_json::from_str(&json).unwrap();
        assert!(restored.content.is_empty());
    }

    /// MemorySnippet with zero relevance should serialize.
    #[test]
    fn test_zero_relevance_snippet() {
        let snippet = MemorySnippet {
            content: "test".to_string(),
            source: MemoryTier::Working,
            relevance: 0.0,
            timestamp_ms: 0,
        };
        let json = serde_json::to_string(&snippet).unwrap();
        let restored: MemorySnippet = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.relevance, 0.0);
    }
}

// ---------------------------------------------------------------------------
// Protocol version compatibility
// ---------------------------------------------------------------------------

#[cfg(test)]
mod protocol_version {
    use super::*;

    /// PROTOCOL_VERSION should be 3 (current version).
    #[test]
    fn test_protocol_version_is_current() {
        assert_eq!(PROTOCOL_VERSION, 3);
    }

    /// Envelope with matching version should pass version check.
    #[test]
    fn test_matching_version_accepted() {
        let envelope =
            AuthenticatedEnvelope::new("test_token".to_string(), 1, DaemonToNeocortex::Ping);
        assert!(envelope.version_ok());
    }

    /// Envelope with wrong version should fail version check.
    #[test]
    fn test_wrong_version_rejected() {
        let envelope = AuthenticatedEnvelope {
            protocol_version: PROTOCOL_VERSION + 1,
            session_token: "test_token".to_string(),
            seq: 1,
            payload: DaemonToNeocortex::Ping,
        };
        assert!(!envelope.version_ok());
    }

    /// Envelope with version 0 should fail version check.
    #[test]
    fn test_zero_version_rejected() {
        let envelope = AuthenticatedEnvelope {
            protocol_version: 0,
            session_token: "test_token".to_string(),
            seq: 1,
            payload: DaemonToNeocortex::Ping,
        };
        assert!(!envelope.version_ok());
    }

    /// The version check must be the FIRST thing checked on deserialization.
    /// Verify that we can construct, serialize, and deserialize an envelope
    /// with a mismatched version, and that version_ok() catches it.
    #[test]
    fn test_version_check_after_deserialization() {
        // Construct an envelope with a future version.
        let future_envelope = AuthenticatedEnvelope {
            protocol_version: 99,
            session_token: "future_token".to_string(),
            seq: 42,
            payload: DaemonToNeocortex::Ping,
        };

        // Serialize it (simulating a message from a future binary).
        let json = serde_json::to_string(&future_envelope).unwrap();

        // Deserialize (our current binary can still parse the JSON).
        let received: AuthenticatedEnvelope<DaemonToNeocortex> =
            serde_json::from_str(&json).unwrap();

        // But version_ok() must reject it.
        assert!(
            !received.version_ok(),
            "version 99 must be rejected by current protocol version {}",
            PROTOCOL_VERSION
        );
    }

    /// REQUEST_TIMEOUT should be 30 seconds.
    #[test]
    fn test_request_timeout_constant() {
        assert_eq!(REQUEST_TIMEOUT, std::time::Duration::from_secs(30));
    }

    /// Session token field must be preserved through roundtrip.
    #[test]
    fn test_session_token_preserved() {
        let token = "a]b2c3d4e5f678901234567890123456789012345678901234567890123456";
        let envelope =
            AuthenticatedEnvelope::new(token.to_string(), 100, DaemonToNeocortex::Cancel);

        let json = serde_json::to_string(&envelope).unwrap();
        let restored: AuthenticatedEnvelope<DaemonToNeocortex> =
            serde_json::from_str(&json).unwrap();

        assert_eq!(restored.session_token, token);
    }

    /// Sequence number must support u64::MAX.
    #[test]
    fn test_seq_overflow_protection() {
        let envelope =
            AuthenticatedEnvelope::new("token".to_string(), u64::MAX, DaemonToNeocortex::Ping);

        let json = serde_json::to_string(&envelope).unwrap();
        let restored: AuthenticatedEnvelope<DaemonToNeocortex> =
            serde_json::from_str(&json).unwrap();

        assert_eq!(restored.seq, u64::MAX);
    }
}

// ---------------------------------------------------------------------------
// IPC rate limiting
// ---------------------------------------------------------------------------

#[cfg(test)]
mod rate_limiting {
    use super::*;

    /// Default rate limit config should have sensible values.
    #[test]
    fn test_rate_limit_defaults() {
        let config = IpcRateLimitConfig::default();
        assert_eq!(config.max_requests_per_second, 100);
        assert_eq!(config.burst_allowance, 20);
        assert!(config.burst_allowance < config.max_requests_per_second);
    }

    /// Rate limit config values must be non-zero.
    #[test]
    fn test_rate_limit_nonzero() {
        let config = IpcRateLimitConfig::default();
        assert!(config.max_requests_per_second > 0);
        assert!(config.burst_allowance > 0);
    }
}

// ---------------------------------------------------------------------------
// Enum variant completeness
// ---------------------------------------------------------------------------

#[cfg(test)]
mod enum_variants {
    use super::*;

    /// All ModelTier variants should serialize/deserialize.
    #[test]
    fn test_model_tier_variants() {
        let tiers = [
            ModelTier::Brainstem1_5B,
            ModelTier::Standard4B,
            ModelTier::Full8B,
        ];

        for tier in &tiers {
            let json = serde_json::to_string(tier).unwrap();
            let restored: ModelTier = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, *tier);
        }
    }

    /// All Role variants should serialize/deserialize.
    #[test]
    fn test_role_variants() {
        let roles = [Role::User, Role::Assistant, Role::System];

        for role in &roles {
            let json = serde_json::to_string(role).unwrap();
            let restored: Role = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, *role);
        }
    }

    /// All MemoryTier variants should serialize/deserialize.
    #[test]
    fn test_memory_tier_variants() {
        let tiers = [
            MemoryTier::Working,
            MemoryTier::Episodic,
            MemoryTier::Semantic,
            MemoryTier::Archive,
        ];

        for tier in &tiers {
            let json = serde_json::to_string(tier).unwrap();
            let restored: MemoryTier = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, *tier);
        }
    }

    /// All InferenceMode variants produce valid temperature/top_p/max_tokens.
    #[test]
    fn test_inference_mode_properties() {
        let modes = [
            InferenceMode::Planner,
            InferenceMode::Strategist,
            InferenceMode::Composer,
            InferenceMode::Conversational,
        ];

        for mode in &modes {
            let temp = mode.temperature();
            assert!(
                temp >= 0.0 && temp <= 2.0,
                "temperature out of range: {}",
                temp
            );

            let top_p = mode.top_p();
            assert!(top_p > 0.0 && top_p <= 1.0, "top_p out of range: {}", top_p);

            let max_tokens = mode.max_tokens();
            assert!(max_tokens > 0, "max_tokens must be positive");
        }
    }

    /// UserState variants should serialize/deserialize.
    #[test]
    fn test_user_state_variants() {
        let states = [
            UserState::Active,
            UserState::Idle,
            UserState::Sleeping,
            UserState::Driving,
            UserState::InMeeting,
            UserState::Unknown,
        ];

        for state in &states {
            let json = serde_json::to_string(state).unwrap();
            let restored: UserState = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, *state);
        }
    }
}
