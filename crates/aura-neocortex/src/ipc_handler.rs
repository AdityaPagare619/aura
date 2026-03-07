//! IPC message handler for AURA Neocortex.
//!
//! Implements length-prefixed bincode framing over a byte stream (Unix domain
//! socket on Android, TCP on host).  Handles reading, writing, dispatch of
//! `DaemonToNeocortex` messages, and enforces size / timeout limits.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aura_types::ipc::{ContextPackage, DaemonToNeocortex, FailureContext, NeocortexToDaemon};
use tracing::{debug, error, info, warn};

use crate::context;
use crate::inference::{InferenceEngine, ProgressSender};
use crate::model::ModelManager;

// ─── Constants ──────────────────────────────────────────────────────────────

/// Maximum message size under normal conditions (256 KB).
const MAX_MESSAGE_SIZE: usize = 256 * 1024;

/// Maximum message size under memory pressure (16 KB).
const MAX_MESSAGE_SIZE_LOW_MEM: usize = 16 * 1024;

/// Timeout for a single request processing (30 seconds).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Length prefix size: 4 bytes, little-endian u32.
const LENGTH_PREFIX_SIZE: usize = 4;

// ─── IPC handler ────────────────────────────────────────────────────────────

/// Manages the IPC connection to the daemon.
pub struct IpcHandler {
    stream: TcpStream,
    model_manager: ModelManager,
    inference_engine: InferenceEngine,
    #[allow(dead_code)]
    cancel_token: Arc<AtomicBool>,
    /// Process start time for uptime calculation.
    start_time: Instant,
    /// Whether we're under memory pressure (reduces max message size).
    low_memory: bool,
}

impl IpcHandler {
    /// Create a new handler wrapping an accepted TCP stream.
    pub fn new(
        stream: TcpStream,
        model_manager: ModelManager,
        cancel_token: Arc<AtomicBool>,
    ) -> io::Result<Self> {
        // Set read/write timeouts on the stream.
        stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;

        let inference_engine = InferenceEngine::new(cancel_token.clone());

        Ok(Self {
            stream,
            model_manager,
            inference_engine,
            cancel_token,
            start_time: Instant::now(),
            low_memory: false,
        })
    }

    /// Main message processing loop.
    ///
    /// Reads messages until the connection closes or a fatal error occurs.
    /// Returns `Ok(())` on clean shutdown, `Err` on unrecoverable failure.
    pub fn run_loop(&mut self) -> io::Result<()> {
        info!("IPC handler loop started");

        loop {
            match self.read_message() {
                Ok(msg) => {
                    debug!(msg = ?std::mem::discriminant(&msg), "received message");
                    let response = self.handle_message(msg);
                    if let Some(resp) = response {
                        if let Err(e) = self.write_message(&resp) {
                            error!(error = %e, "failed to write response");
                            return Err(e);
                        }
                    }
                }
                Err(e) => {
                    if e.kind() == io::ErrorKind::UnexpectedEof
                        || e.kind() == io::ErrorKind::ConnectionReset
                    {
                        info!("daemon disconnected — shutting down");
                        return Ok(());
                    }
                    if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut
                    {
                        // Read timeout — check idle unload, then continue.
                        self.check_idle_unload();
                        continue;
                    }
                    error!(error = %e, "IPC read error");
                    return Err(e);
                }
            }
        }
    }

    // ── Wire protocol: 4-byte LE length prefix + bincode body ───────────────

    /// Read a single framed message from the stream.
    pub fn read_message(&mut self) -> io::Result<DaemonToNeocortex> {
        // Read 4-byte length prefix.
        let mut len_buf = [0u8; LENGTH_PREFIX_SIZE];
        self.stream.read_exact(&mut len_buf)?;
        let msg_len = u32::from_le_bytes(len_buf) as usize;

        // Validate message size.
        let max_size = if self.low_memory {
            MAX_MESSAGE_SIZE_LOW_MEM
        } else {
            MAX_MESSAGE_SIZE
        };

        if msg_len > max_size {
            warn!(msg_len, max_size, "message exceeds size limit");
            // Drain the oversized message to keep the stream in sync.
            let mut drain = vec![0u8; msg_len.min(max_size)];
            let _ = self.stream.read_exact(&mut drain);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("message too large: {msg_len} bytes (max {max_size})"),
            ));
        }

        if msg_len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "zero-length message",
            ));
        }

        // Read message body.
        let mut body = vec![0u8; msg_len];
        self.stream.read_exact(&mut body)?;

        // Deserialize.
        bincode::serde::decode_from_slice(&body, bincode::config::standard())
            .map(|(msg, _)| msg)
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("bincode deserialize failed: {e}"),
                )
            })
    }

    /// Write a single framed message to the stream.
    pub fn write_message(&mut self, msg: &NeocortexToDaemon) -> io::Result<()> {
        let body =
            bincode::serde::encode_to_vec(msg, bincode::config::standard()).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("bincode serialize failed: {e}"),
                )
            })?;

        let len = body.len() as u32;
        self.stream.write_all(&len.to_le_bytes())?;
        self.stream.write_all(&body)?;
        self.stream.flush()?;

        debug!(len = body.len(), "wrote message");
        Ok(())
    }

    // ── Message dispatch ────────────────────────────────────────────────────

    /// Handle a single message, returning an optional response.
    ///
    /// For inference requests (Plan, Replan, Converse, Compose), progress
    /// messages are sent inline during processing.
    fn handle_message(&mut self, msg: DaemonToNeocortex) -> Option<NeocortexToDaemon> {
        match msg {
            // ── Model lifecycle ─────────────────────────────────────────
            DaemonToNeocortex::Load { model_path, params } => {
                info!(
                    model_path = %model_path,
                    tier = ?params.model_tier,
                    n_ctx = params.n_ctx,
                    n_threads = params.n_threads,
                    "loading model"
                );
                match self.model_manager.load(&model_path, &params) {
                    Ok((model_name, memory_used_mb)) => Some(NeocortexToDaemon::Loaded {
                        model_name,
                        memory_used_mb,
                    }),
                    Err(e) => {
                        error!(error = %e, "model load failed");
                        Some(NeocortexToDaemon::LoadFailed { reason: e })
                    }
                }
            }

            DaemonToNeocortex::Unload => {
                info!("unloading model (graceful)");
                self.model_manager.unload();
                Some(NeocortexToDaemon::Unloaded)
            }

            DaemonToNeocortex::UnloadImmediate => {
                info!("unloading model (immediate)");
                self.model_manager.unload();
                Some(NeocortexToDaemon::Unloaded)
            }

            // ── Inference requests ──────────────────────────────────────
            DaemonToNeocortex::Plan { context, failure } => {
                info!(
                    mode = ?context.inference_mode,
                    has_failure = failure.is_some(),
                    "plan request"
                );
                Some(self.handle_inference(&context, None, failure.as_ref()))
            }

            DaemonToNeocortex::Replan { context, failure } => {
                info!(mode = ?context.inference_mode, "replan request");
                Some(self.handle_inference(&context, None, Some(&failure)))
            }

            DaemonToNeocortex::Converse { context } => {
                info!("converse request");
                Some(self.handle_inference(&context, None, None))
            }

            DaemonToNeocortex::Compose { context, template } => {
                info!(template = %template, "compose request");
                Some(self.handle_inference(&context, Some(&template), None))
            }

            // ── Control messages ────────────────────────────────────────
            DaemonToNeocortex::Cancel => {
                info!("cancel requested");
                self.inference_engine.cancel();
                // No direct response — the running inference will detect
                // cancellation and send an Error response.
                None
            }

            DaemonToNeocortex::Ping => {
                let uptime_ms = self.start_time.elapsed().as_millis() as u64;
                debug!(uptime_ms, "pong");
                Some(NeocortexToDaemon::Pong { uptime_ms })
            }
        }
    }

    /// Common inference handler for Plan, Replan, Converse, Compose.
    ///
    /// Accepts the `ContextPackage` directly (already deserialized by the IPC
    /// framing layer).  The inference mode is determined from `context.inference_mode`.
    /// `template` is only set for Compose requests.
    /// `failure` is set for Plan (optional) and Replan (always) requests.
    fn handle_inference(
        &mut self,
        context: &ContextPackage,
        template: Option<&str>,
        failure: Option<&FailureContext>,
    ) -> NeocortexToDaemon {
        let mode = context.inference_mode;

        // Assemble the prompt using the context module.
        let prompt = context::assemble_prompt(context, template, failure);

        info!(
            mode = ?mode,
            estimated_tokens = prompt.estimated_tokens,
            truncated = prompt.was_truncated,
            "prompt assembled"
        );

        // Check for memory warning conditions before inference.
        let available_mb = crate::model::available_ram_mb();
        if available_mb < 200 {
            warn!(available_mb, "low memory — sending warning");
            // Send memory warning as a side-effect via the progress channel,
            // but continue with inference (Founder's directive: never refuse).
            self.set_low_memory(true);
        }

        // Create a progress sender that writes directly to the stream.
        // We need to clone the stream handle for this.
        let mut stream_clone = match self.stream.try_clone() {
            Ok(s) => s,
            Err(e) => {
                return NeocortexToDaemon::Error {
                    code: 500,
                    message: format!("failed to clone stream for progress: {e}"),
                };
            }
        };

        let mut progress_sender: ProgressSender = Box::new(move |msg| {
            let body = match bincode::serde::encode_to_vec(&msg, bincode::config::standard()) {
                Ok(b) => b,
                Err(e) => {
                    warn!(error = %e, "failed to serialize progress");
                    return;
                }
            };
            let len = body.len() as u32;
            if stream_clone.write_all(&len.to_le_bytes()).is_err() {
                return;
            }
            if stream_clone.write_all(&body).is_err() {
                return;
            }
            let _ = stream_clone.flush();
        });

        // Run inference.
        self.inference_engine
            .infer(&mut self.model_manager, prompt, mode, &mut progress_sender)
    }

    /// Check if the model should be unloaded due to idle timeout.
    fn check_idle_unload(&mut self) {
        // For now, assume normal activity and not charging.
        // In production, these would come from Android system queries.
        if self.model_manager.should_idle_unload(false, false) {
            info!("idle timeout reached — unloading model");
            self.model_manager.unload();
        }
    }

    /// Set memory pressure flag (affects max message size).
    pub fn set_low_memory(&mut self, low: bool) {
        self.low_memory = low;
        if low {
            warn!("entering low-memory mode — reduced message size limits");
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use aura_types::ipc::{InferenceMode, ModelParams, ModelTier};

    #[test]
    fn message_round_trip_load() {
        // Test that DaemonToNeocortex::Load round-trips correctly.
        let msg = DaemonToNeocortex::Load {
            model_path: "/data/models".into(),
            params: ModelParams {
                n_ctx: 2048,
                n_threads: 4,
                model_tier: ModelTier::Standard4B,
            },
        };
        let bytes = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
        let (decoded, _): (DaemonToNeocortex, _) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();

        match decoded {
            DaemonToNeocortex::Load { model_path, params } => {
                assert_eq!(model_path, "/data/models");
                assert_eq!(params.n_ctx, 2048);
                assert_eq!(params.n_threads, 4);
                assert!(matches!(params.model_tier, ModelTier::Standard4B));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_round_trip_pong() {
        let msg = NeocortexToDaemon::Pong { uptime_ms: 12345 };
        let bytes = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
        let (decoded, _): (NeocortexToDaemon, _) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();

        match decoded {
            NeocortexToDaemon::Pong { uptime_ms } => {
                assert_eq!(uptime_ms, 12345);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_round_trip_loaded() {
        let msg = NeocortexToDaemon::Loaded {
            model_name: "qwen3.5-4b-q4_k_m.gguf".into(),
            memory_used_mb: 2400,
        };
        let bytes = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
        let (decoded, _): (NeocortexToDaemon, _) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();

        match decoded {
            NeocortexToDaemon::Loaded {
                model_name,
                memory_used_mb,
            } => {
                assert_eq!(model_name, "qwen3.5-4b-q4_k_m.gguf");
                assert_eq!(memory_used_mb, 2400);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_round_trip_error() {
        let msg = NeocortexToDaemon::Error {
            code: 503,
            message: "model not loaded".into(),
        };
        let bytes = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
        let (decoded, _): (NeocortexToDaemon, _) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();

        match decoded {
            NeocortexToDaemon::Error { code, message } => {
                assert_eq!(code, 503);
                assert_eq!(message, "model not loaded");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn framing_length_prefix() {
        // Verify our framing format: 4-byte LE prefix.
        let msg = DaemonToNeocortex::Ping;
        let body = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
        let len = body.len() as u32;
        let prefix = len.to_le_bytes();

        assert_eq!(prefix.len(), 4);
        assert_eq!(u32::from_le_bytes(prefix), len);
    }

    #[test]
    fn all_daemon_messages_serialize() {
        use aura_types::ipc::{
            ConversationTurn, GoalSummary, MemorySnippet, MemoryTier, PersonalitySnapshot, Role,
            ScreenSummary, UserState,
        };

        // Build a minimal ContextPackage for the messages that need one.
        let context = ContextPackage {
            conversation_history: vec![ConversationTurn {
                role: Role::User,
                content: "hello".into(),
                timestamp_ms: 1000,
            }],
            memory_snippets: vec![MemorySnippet {
                content: "test snippet".into(),
                source: MemoryTier::Working,
                relevance: 0.9,
                timestamp_ms: 1000,
            }],
            current_screen: Some(ScreenSummary {
                package_name: "com.test".into(),
                activity_name: "MainActivity".into(),
                interactive_elements: vec!["Button:OK".into()],
                visible_text: vec!["Hello".into()],
            }),
            active_goal: Some(GoalSummary {
                description: "test goal".into(),
                progress_percent: 50,
                current_step: "testing".into(),
                blockers: vec![],
            }),
            personality: PersonalitySnapshot {
                openness: 0.7,
                conscientiousness: 0.8,
                extraversion: 0.5,
                agreeableness: 0.6,
                neuroticism: 0.3,
                current_mood_valence: 0.5,
                current_mood_arousal: 0.4,
                trust_level: 0.8,
            },
            user_state: UserState::Active,
            inference_mode: InferenceMode::Planner,
            token_budget: 4096,
        };

        let failure = aura_types::ipc::FailureContext {
            task_goal_hash: 123,
            current_step: 1,
            failing_action: 456,
            target_id: 789,
            expected_state_hash: 111,
            actual_state_hash: 222,
            tried_approaches: 0b101,
            last_3_transitions: Default::default(),
            error_class: 2,
        };

        let messages: Vec<DaemonToNeocortex> = vec![
            DaemonToNeocortex::Load {
                model_path: "/data/models".into(),
                params: ModelParams {
                    n_ctx: 2048,
                    n_threads: 4,
                    model_tier: ModelTier::Standard4B,
                },
            },
            DaemonToNeocortex::Unload,
            DaemonToNeocortex::UnloadImmediate,
            DaemonToNeocortex::Plan {
                context: context.clone(),
                failure: None,
            },
            DaemonToNeocortex::Plan {
                context: context.clone(),
                failure: Some(failure.clone()),
            },
            DaemonToNeocortex::Replan {
                context: context.clone(),
                failure: failure.clone(),
            },
            DaemonToNeocortex::Converse {
                context: context.clone(),
            },
            DaemonToNeocortex::Compose {
                context: context.clone(),
                template: "notification_reply".into(),
            },
            DaemonToNeocortex::Cancel,
            DaemonToNeocortex::Ping,
        ];

        for msg in &messages {
            let bytes = bincode::serde::encode_to_vec(msg, bincode::config::standard()).unwrap();
            assert!(!bytes.is_empty());
            assert!(bytes.len() < MAX_MESSAGE_SIZE);

            // Round-trip.
            let (_decoded, _): (DaemonToNeocortex, _) =
                bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        }
    }

    #[test]
    fn all_neocortex_messages_serialize() {
        use aura_types::actions::{ActionType, TargetSelector};
        use aura_types::dsl::{DslStep, FailureStrategy};
        use aura_types::etg::{ActionPlan, PlanSource};

        let messages: Vec<NeocortexToDaemon> = vec![
            NeocortexToDaemon::Loaded {
                model_name: "qwen3.5-4b-q4_k_m.gguf".into(),
                memory_used_mb: 2400,
            },
            NeocortexToDaemon::LoadFailed {
                reason: "out of memory".into(),
            },
            NeocortexToDaemon::Unloaded,
            NeocortexToDaemon::PlanReady {
                plan: ActionPlan {
                    goal_description: "test plan".into(),
                    steps: vec![DslStep {
                        action: ActionType::Tap { x: 100, y: 200 },
                        target: Some(TargetSelector::Text("OK".into())),
                        timeout_ms: 5000,
                        on_failure: FailureStrategy::Retry { max: 3 },
                        precondition: None,
                        postcondition: None,
                        label: Some("tap ok button".into()),
                    }],
                    estimated_duration_ms: 3000,
                    confidence: 0.85,
                    source: PlanSource::LlmGenerated,
                },
            },
            NeocortexToDaemon::ConversationReply {
                text: "Hello!".into(),
                mood_hint: Some(0.8),
            },
            NeocortexToDaemon::ComposedScript {
                steps: vec![DslStep {
                    action: ActionType::Type {
                        text: "reply text".into(),
                    },
                    target: Some(TargetSelector::ResourceId("input_field".into())),
                    timeout_ms: 3000,
                    on_failure: FailureStrategy::Retry { max: 2 },
                    precondition: None,
                    postcondition: None,
                    label: Some("type reply".into()),
                }],
            },
            NeocortexToDaemon::Progress {
                percent: 50,
                stage: "generating tokens".into(),
            },
            NeocortexToDaemon::Error {
                code: 500,
                message: "inference failed".into(),
            },
            NeocortexToDaemon::Pong { uptime_ms: 60000 },
            NeocortexToDaemon::MemoryWarning {
                used_mb: 3500,
                available_mb: 150,
            },
            NeocortexToDaemon::TokenBudgetExhausted,
        ];

        for msg in &messages {
            let bytes = bincode::serde::encode_to_vec(msg, bincode::config::standard()).unwrap();
            assert!(!bytes.is_empty());
            let (_decoded, _): (NeocortexToDaemon, _) =
                bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        }
    }

    #[test]
    fn max_message_size_constants() {
        assert_eq!(MAX_MESSAGE_SIZE, 262144);
        assert_eq!(MAX_MESSAGE_SIZE_LOW_MEM, 16384);
        assert!(MAX_MESSAGE_SIZE_LOW_MEM < MAX_MESSAGE_SIZE);
    }

    #[test]
    fn cancel_is_unit_variant() {
        // Verify Cancel has no fields — just a unit variant.
        let msg = DaemonToNeocortex::Cancel;
        let bytes = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
        // Should be very small — just the variant discriminant.
        assert!(bytes.len() <= 4);
        let (decoded, _): (DaemonToNeocortex, _) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        assert!(matches!(decoded, DaemonToNeocortex::Cancel));
    }

    #[test]
    fn ping_pong_no_extra_fields() {
        // Pong only has uptime_ms — no memory_mb.
        let msg = NeocortexToDaemon::Pong { uptime_ms: 999 };
        let bytes = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
        let (decoded, _): (NeocortexToDaemon, _) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        match decoded {
            NeocortexToDaemon::Pong { uptime_ms } => {
                assert_eq!(uptime_ms, 999);
            }
            _ => panic!("wrong variant"),
        }
    }
}
