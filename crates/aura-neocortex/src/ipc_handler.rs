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

use aura_types::ipc::{ContextPackage, DaemonToNeocortex, FailureContext, InferenceMode, NeocortexToDaemon, ProactiveTrigger};
use tracing::{debug, error, info, warn};

use crate::context::{self, AssembledPrompt};
use crate::grammar;
use crate::inference::{InferenceEngine, ProgressSender};
use crate::model::ModelManager;
use crate::model_capabilities::ModelCapabilities;
use crate::prompts::mode_config;

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
    ///
    /// `startup_capabilities` is derived from GGUF metadata at startup and
    /// seeded into `InferenceEngine` so it is never blind to model geometry
    /// from the first millisecond. When `None`, the engine falls back to the
    /// mode's `max_tokens * 2` budget — acceptable only before any model loads.
    pub fn new(
        stream: TcpStream,
        model_manager: ModelManager,
        cancel_token: Arc<AtomicBool>,
        startup_capabilities: Option<ModelCapabilities>,
    ) -> io::Result<Self> {
        // Set read/write timeouts on the stream.
        stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;

        let inference_engine = InferenceEngine::new(cancel_token.clone(), startup_capabilities);

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
                    if e.kind() == io::ErrorKind::InvalidData {
                        // Oversized or malformed message — already drained (if
                        // applicable), stream is in sync.  Log and continue.
                        warn!(error = %e, "skipping invalid message");
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
            // Hard cap: refuse to drain absurdly large messages (>16 MB).
            // A legitimate peer should never send this much; drop the connection.
            const HARD_CAP: usize = 16 * 1024 * 1024;
            if msg_len > HARD_CAP {
                error!(
                    msg_len,
                    hard_cap = HARD_CAP,
                    "message exceeds hard cap — dropping connection"
                );
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "message absurdly large: {msg_len} bytes (hard cap {HARD_CAP}) — \
                         refusing to drain, connection dropped"
                    ),
                ));
            }

            warn!(msg_len, max_size, "message exceeds size limit — draining");

            // Drain the oversized message in chunks to keep the stream in sync.
            // Uses a small fixed buffer to avoid large allocations under memory pressure.
            const DRAIN_CHUNK: usize = 8192;
            let mut remaining = msg_len;
            let mut chunk = vec![0u8; DRAIN_CHUNK];
            while remaining > 0 {
                let to_read = remaining.min(DRAIN_CHUNK);
                self.stream.read_exact(&mut chunk[..to_read])?;
                remaining -= to_read;
            }

            debug!(msg_len, "oversized message drained successfully");
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
                    Ok((model_name, memory_used_mb)) => {
                        // Derive fresh ModelCapabilities from the newly loaded
                        // model's GGUF metadata and push them into the inference
                        // engine so prompt budgets and geometry are correct for
                        // this model, not the one that was loaded at startup.
                        if let Some(tier) = self.model_manager.current_tier() {
                            let caps = if let Some((_, meta)) =
                                self.model_manager.scanner().models.get(&tier)
                            {
                                ModelCapabilities::from_gguf(meta, None)
                            } else {
                                warn!(
                                    ?tier,
                                    "no GGUF metadata for loaded tier after load — using fallback"
                                );
                                ModelCapabilities::fallback_defaults()
                            };
                            self.inference_engine.update_capabilities(caps);
                        }
                        Some(NeocortexToDaemon::Loaded {
                            model_name,
                            memory_used_mb,
                        })
                    }
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

            // ── Embedding request ───────────────────────────────────────
            DaemonToNeocortex::Embed { text } => {
                debug!(chars = text.len(), "embed request");
                match self.model_manager.embed(&text) {
                    Ok(vector) => Some(NeocortexToDaemon::Embedding { vector }),
                    Err(reason) => {
                        warn!(error = %reason, "embed failed");
                        Some(NeocortexToDaemon::Error {
                            code: 1, // MODEL_NOT_LOADED
                            message: reason,
                        })
                    }
                }
            }

            // ── ReAct loop step ──────────────────────────────────────────
            // Daemon executed one action, captured real screen state, and sends
            // back the observation. LLM reasons whether goal is done or what
            // action to take next. This closes the ReAct loop properly.
            DaemonToNeocortex::ReActStep {
                tool_name,
                observation,
                screen_description,
                goal,
                step_index,
                max_steps,
            } => {
                info!(
                    step = step_index,
                    max = max_steps,
                    tool = %tool_name,
                    "react step — reasoning next action"
                );

                // Build a structured ReAct prompt. The LLM must output:
                //   DONE: yes|no
                //   REASONING: <one sentence>
                //   NEXT_ACTION: <tool_name(args)> or <empty if done>
                let system_prompt = format!(
                    "You are AURA's reasoning engine executing a task step-by-step.\n\
                     \n\
                     GOAL: {goal}\n\
                     STEP: {step_index} of {max_steps}\n\
                     LAST ACTION: {tool_name}\n\
                     OBSERVATION: {observation}\n\
                     CURRENT SCREEN: {screen_description}\n\
                     \n\
                     Decide if the goal is complete or what to do next.\n\
                     Respond in exactly this format:\n\
                     DONE: yes or no\n\
                     REASONING: one sentence explaining your decision\n\
                     NEXT_ACTION: tool_name(arg1, arg2) or leave blank if done\n"
                );

                let cfg = mode_config(InferenceMode::Conversational);
                let estimated_tokens = (system_prompt.len() / 4) as u32;
                let prompt = AssembledPrompt {
                    system_prompt,
                    config: cfg,
                    estimated_tokens,
                    was_truncated: false,
                    grammar_kind: None,
                    cot_enabled: false,
                    original_goal: goal.clone(),
                    high_stakes: false,
                    has_tools: false,
                    is_retry: false,
                    token_budget: context::DEFAULT_CONTEXT_BUDGET,
                    estimated_complexity: None,
                };

                // Dummy progress sender — ReAct steps do not stream tokens.
                let mut noop: ProgressSender = Box::new(|_| {});

                let response = self
                    .inference_engine
                    .infer(&mut self.model_manager, prompt, InferenceMode::Conversational, &mut noop);

                // Parse the structured response into typed ReActDecision fields.
                let (done, reasoning, next_action) = match &response {
                    NeocortexToDaemon::ConversationReply { text, .. } => {
                        parse_react_response(text)
                    }
                    // Any error from inference → treat as "not done, no action" so
                    // the daemon can decide how to escalate (e.g. abort after N failures).
                    other => {
                        warn!(?other, "unexpected inference result during react step");
                        (false, "inference error during react step".to_string(), None)
                    }
                };

                Some(NeocortexToDaemon::ReActDecision {
                    done,
                    reasoning,
                    next_action,
                    tokens_used: 0,
                })
            }

            // ── Proactive context ────────────────────────────────────────────
            // A daemon subsystem detected a proactive opportunity (goal stall,
            // social gap, health alert, memory insight, etc.) and sent typed
            // structured data. The LLM is responsible for generating the
            // natural-language message from the typed facts + user context.
            //
            // Architecture law: NO format strings encode intent here — the
            // trigger carries typed fields; the LLM turns them into language.
            DaemonToNeocortex::ProactiveContext { trigger, context } => {
                info!(trigger = ?std::mem::discriminant(&trigger), "proactive context — generating natural message");

                // Describe the trigger facts as structured text for the LLM's
                // system prompt.  This is NOT generating the user message — it
                // is converting typed data to a prompt description.  The LLM
                // turns this into language appropriate to the user's personality
                // and relationship context from the ContextPackage.
                let trigger_description = describe_trigger(&trigger);

                debug!(
                    trigger_desc_len = trigger_description.len(),
                    ocean_o = context.personality.openness,
                    ocean_c = context.personality.conscientiousness,
                    relationship_trust = context.personality.trust_level,
                    "assembling proactive inference prompt"
                );

                // Inject the proactive system framing into the context's
                // conversation history for the inference engine.
                let mut proactive_ctx = context.clone();
                proactive_ctx.inference_mode = InferenceMode::Conversational;

                // Build a standalone prompt via the shared inference path.
                // The trigger description is prepended to the context so the
                // LLM receives: "You detected X. Here is the user's context.
                // Generate a natural, empathetic message."
                let proactive_system = format!(
                    "You are AURA. A background monitoring system detected a proactive \
                     opportunity. Here is the structured data:\n\
                     \n\
                     {trigger_description}\n\
                     \n\
                     User personality (OCEAN): openness={:.2}, conscientiousness={:.2}, \
                     extraversion={:.2}, agreeableness={:.2}, neuroticism={:.2}\n\
                     Current mood: valence={:.2}, arousal={:.2}\n\
                     Relationship trust: {:.2}\n\
                     \n\
                     Generate a concise (1–3 sentence) natural message for the user. \
                     Do NOT repeat the raw field names. Speak as a caring assistant. \
                     Tone should match their personality and relationship trust level.",
                    context.personality.openness,
                    context.personality.conscientiousness,
                    context.personality.extraversion,
                    context.personality.agreeableness,
                    context.personality.neuroticism,
                    context.personality.current_mood_valence,
                    context.personality.current_mood_arousal,
                    context.personality.trust_level,
                );

                let cfg = mode_config(InferenceMode::Conversational);
                let estimated_tokens = (proactive_system.len() / 4) as u32;
                let prompt = AssembledPrompt {
                    system_prompt: proactive_system,
                    config: cfg,
                    estimated_tokens,
                    was_truncated: false,
                    grammar_kind: None,
                    cot_enabled: false,
                    original_goal: "proactive_notification".to_string(),
                    high_stakes: false,
                    has_tools: false,
                    is_retry: false,
                    token_budget: context::DEFAULT_CONTEXT_BUDGET,
                    estimated_complexity: None,
                };

                let mut noop: ProgressSender = Box::new(|_| {});
                let result = self.inference_engine.infer(
                    &mut self.model_manager,
                    prompt,
                    InferenceMode::Conversational,
                    &mut noop,
                );

                Some(result)
            }

            // ── Memory consolidation / dreaming phase summarization ───────────
            // During the dreaming phase the daemon asks the LLM to synthesize raw
            // episodic memories into semantic knowledge. Architecture law: Rust
            // NEVER reasons about what episodes mean — only the LLM may do that.
            //
            // The daemon sends a fully-formed prompt; the LLM returns a concise
            // insight. The result is stored as a SemanticMemory entry by the daemon.
            DaemonToNeocortex::Summarize { prompt } => {
                info!(prompt_chars = prompt.len(), "memory consolidation summarize request");

                let cfg = mode_config(InferenceMode::Conversational);
                let estimated_tokens = (prompt.len() / 4) as u32;
                let assembled = AssembledPrompt {
                    system_prompt: prompt,
                    config: cfg,
                    estimated_tokens,
                    was_truncated: false,
                    grammar_kind: None,
                    cot_enabled: false,
                    original_goal: "memory_consolidation".to_string(),
                    high_stakes: false,
                    has_tools: false,
                    is_retry: false,
                    token_budget: context::DEFAULT_CONTEXT_BUDGET,
                    estimated_complexity: None,
                };

                let mut noop: ProgressSender = Box::new(|_| {});
                let raw = self.inference_engine.infer(
                    &mut self.model_manager,
                    assembled,
                    InferenceMode::Conversational,
                    &mut noop,
                );

                // Extract the text from the inference result and wrap as Summary.
                let text = match raw {
                    NeocortexToDaemon::ConversationReply { text, .. } => text,
                    NeocortexToDaemon::Error { message, .. } => {
                        warn!(error = %message, "summarize inference failed");
                        return Some(NeocortexToDaemon::Error {
                            code: 2,
                            message,
                        });
                    }
                    other => {
                        warn!(?other, "unexpected result during summarize");
                        return Some(NeocortexToDaemon::Error {
                            code: 3,
                            message: "unexpected inference result type".to_string(),
                        });
                    }
                };

                Some(NeocortexToDaemon::Summary { text, tokens_used: 0 })
            }

            // ── Plan scoring ─────────────────────────────────────────────────
            // The daemon asks the LLM to evaluate a candidate ActionPlan and
            // return a quality score in [0.0, 1.0].  The LLM must output a
            // single decimal number; we parse the first float we find.
            DaemonToNeocortex::ScorePlan { plan } => {
                info!(
                    goal = %plan.goal_description,
                    steps = plan.steps.len(),
                    "plan scoring request"
                );

                let prompt = format!(
                    "You are AURA's plan quality evaluator.\n\
                     Goal: {}\n\
                     Steps ({}):\n{}\n\
                     Rate the quality of this plan on a scale from 0.0 (terrible) \
                     to 1.0 (excellent). Respond with only a decimal number, e.g. 0.75.",
                    plan.goal_description,
                    plan.steps.len(),
                    plan.steps.iter().enumerate()
                        .map(|(i, s)| format!("  {}. {:?}", i + 1, s))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );

                let cfg = mode_config(InferenceMode::Conversational);
                let estimated_tokens = (prompt.len() / 4) as u32;
                let assembled = AssembledPrompt {
                    system_prompt: prompt,
                    config: cfg,
                    estimated_tokens,
                    was_truncated: false,
                    grammar_kind: None,
                    cot_enabled: false,
                    original_goal: "score_plan".to_string(),
                    high_stakes: false,
                    has_tools: false,
                    is_retry: false,
                    token_budget: context::DEFAULT_CONTEXT_BUDGET,
                    estimated_complexity: None,
                };

                let mut noop: ProgressSender = Box::new(|_| {});
                let raw = self.inference_engine.infer(
                    &mut self.model_manager,
                    assembled,
                    InferenceMode::Conversational,
                    &mut noop,
                );

                let text = match raw {
                    NeocortexToDaemon::ConversationReply { text, .. } => text,
                    NeocortexToDaemon::Error { message, .. } => {
                        warn!(error = %message, "score_plan inference failed");
                        return Some(NeocortexToDaemon::Error { code: 4, message });
                    }
                    other => {
                        warn!(?other, "unexpected result during score_plan");
                        return Some(NeocortexToDaemon::Error {
                            code: 5,
                            message: "unexpected inference result type".to_string(),
                        });
                    }
                };

                // Parse the first float found in the response.
                let score = text
                    .split_whitespace()
                    .find_map(|tok| tok.parse::<f32>().ok())
                    .unwrap_or(0.5)
                    .clamp(0.0, 1.0);

                info!(score, "plan scored");
                Some(NeocortexToDaemon::PlanScore { score })
            }

            // ── Failure classification ────────────────────────────────────────
            // The daemon asks the LLM to classify a failure string into one of
            // the known FailureCategory labels so the DGS can adapt its strategy.
            DaemonToNeocortex::ClassifyFailure { error, context } => {
                info!(error_len = error.len(), "failure classification request");

                let prompt = format!(
                    "You are AURA's failure classifier.\n\
                     Context: {context}\n\
                     Error: {error}\n\
                     Classify this failure into exactly one of these categories:\n\
                     Transient, Strategic, Environmental, Capability, Safety.\n\
                     Respond with only the category name, e.g. Transient.",
                );

                let cfg = mode_config(InferenceMode::Conversational);
                let estimated_tokens = (prompt.len() / 4) as u32;
                let assembled = AssembledPrompt {
                    system_prompt: prompt,
                    config: cfg,
                    estimated_tokens,
                    was_truncated: false,
                    grammar_kind: None,
                    cot_enabled: false,
                    original_goal: "classify_failure".to_string(),
                    high_stakes: false,
                    has_tools: false,
                    is_retry: false,
                    token_budget: context::DEFAULT_CONTEXT_BUDGET,
                    estimated_complexity: None,
                };

                let mut noop: ProgressSender = Box::new(|_| {});
                let raw = self.inference_engine.infer(
                    &mut self.model_manager,
                    assembled,
                    InferenceMode::Conversational,
                    &mut noop,
                );

                let text = match raw {
                    NeocortexToDaemon::ConversationReply { text, .. } => text,
                    NeocortexToDaemon::Error { message, .. } => {
                        warn!(error = %message, "classify_failure inference failed");
                        return Some(NeocortexToDaemon::Error { code: 6, message });
                    }
                    other => {
                        warn!(?other, "unexpected result during classify_failure");
                        return Some(NeocortexToDaemon::Error {
                            code: 7,
                            message: "unexpected inference result type".to_string(),
                        });
                    }
                };

                // Extract the category word from the response.
                let valid = ["Transient", "Strategic", "Environmental", "Capability", "Safety"];
                let category = valid
                    .iter()
                    .find(|&&v| text.contains(v))
                    .copied()
                    .unwrap_or("Transient")
                    .to_string();

                info!(%category, "failure classified");
                Some(NeocortexToDaemon::FailureClassification { category })
            }
        }
    }

    /// Common inference handler for Plan, Replan, Converse, Compose.
    ///
    /// Accepts the `ContextPackage` directly (already deserialized by the IPC
    /// framing layer).  The inference mode is determined from `context.inference_mode`.
    /// `template` is only set for Compose requests.
    /// `failure` is set for Plan (optional) and Replan (always) requests.
    ///
    /// # Teacher stack wiring
    ///
    /// Every request passes through the full `ContextBuilder` pipeline:
    /// - `should_force_cot` → Layer 1 (chain-of-thought forcing)
    /// - `estimate_importance` → high-stakes gate (> 0.8 → `high_stakes: true`)
    /// - `GrammarKind::for_mode` → Layer 0 (GBNF grammar constraint)
    /// - Tool injection → Planner / Strategist modes only
    /// - `with_failure` → Strategist / Replan retry context
    fn handle_inference(
        &mut self,
        context: &ContextPackage,
        template: Option<&str>,
        failure: Option<&FailureContext>,
    ) -> NeocortexToDaemon {
        let mode = context.inference_mode;

        // ── Teacher stack routing ────────────────────────────────────────────
        //
        // These functions are the teacher stack's routing layer. They are NOT
        // format strings and do NOT encode intelligence — they read typed facts
        // from the ContextPackage and produce routing decisions. The LLM does
        // the actual reasoning; these just control which layers are active.

        // Layer 1: Should chain-of-thought be forced for this request?
        let force_cot = context::should_force_cot(context, failure);

        // Teacher stack routing: estimate request importance for high-stakes gate.
        // If importance > 0.8, this request warrants maximum-quality inference
        // (e.g., Best-of-N sampling at Layer 5).
        let importance = context::estimate_importance(context, failure);
        let high_stakes = importance > 0.8;

        // Layer 0: Grammar kind derived from mode — never hardcoded per-callsite.
        let grammar_kind = grammar::GrammarKind::for_mode(mode);

        // Tool injection: Planner and Strategist need to reason about available
        // actions. Conversational and Composer modes do not.
        let use_tools = matches!(
            mode,
            aura_types::ipc::InferenceMode::Planner | aura_types::ipc::InferenceMode::Strategist
        );

        debug!(
            mode = ?mode,
            force_cot,
            importance,
            high_stakes,
            grammar = ?grammar_kind,
            use_tools,
            "teacher stack routing decisions"
        );

        // ── Assemble via full ContextBuilder pipeline ────────────────────────
        //
        // The legacy `assemble_prompt()` is a thin wrapper over `ContextBuilder`
        // with no teacher stack features. We call the builder directly so all
        // layer decisions above take effect.
        let mut builder = context::ContextBuilder::new(context)
            .with_cot(force_cot);

        if let Some(gk) = grammar_kind {
            builder = builder.with_grammar(gk);
        }

        if use_tools {
            builder = builder.with_tools();
        }

        if let Some(f) = failure {
            builder = builder.with_failure(f);
        }

        if let Some(t) = template {
            builder = builder.with_template(t);
        }

        // Seed builder with live capabilities so budget is capped at the real
        // context window size. Prevents building prompts that exceed KV cache.
        if let Some(caps) = self.inference_engine.capabilities() {
            builder = builder.with_capabilities(caps);
        }

        // Build the assembled prompt — all teacher stack decisions baked in.
        let mut prompt = builder.build();

        // ── High-stakes gate ─────────────────────────────────────────────────
        //
        // `importance > 0.8` signals this request warrants Best-of-N (Layer 5).
        // The field is on `AssembledPrompt`; setting it here lets
        // `InferenceEngine::infer()` activate Layer 5 sampling without any
        // signature change.
        prompt.high_stakes = high_stakes;

        info!(
            mode = ?mode,
            estimated_tokens = prompt.estimated_tokens,
            truncated = prompt.was_truncated,
            force_cot,
            high_stakes,
            "prompt assembled via teacher stack"
        );

        // Check for memory warning conditions before inference.
        let available_mb = crate::model::available_ram_mb();
        if available_mb < 200 {
            warn!(available_mb, "low memory — sending warning");
            // Send memory warning as a side-effect via the progress channel,
            // but continue with inference (Founder's directive: never refuse).
            self.set_low_memory(true);
        } else if self.low_memory {
            info!(available_mb, "memory recovered — restoring normal message size limits");
            self.set_low_memory(false);
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

/// Parse the LLM's structured ReAct response into typed fields.
///
/// Expected format (line-by-line, case-insensitive):
/// ```text
/// DONE: yes
/// REASONING: The file was created successfully.
/// NEXT_ACTION: open_file(path=/tmp/out.txt)
/// ```
///
/// Tolerates missing lines and partial responses.
fn parse_react_response(text: &str) -> (bool, String, Option<String>) {
    let mut done = false;
    let mut reasoning = String::from("no reasoning provided");
    let mut next_action: Option<String> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        // Use lowercase key for prefix matching only; preserve original case for values.
        let lower_key = trimmed.to_ascii_lowercase();
        if let Some(rest) = lower_key.strip_prefix("done:") {
            done = rest.trim() == "yes";
        } else if lower_key.starts_with("reasoning:") {
            // Preserve original casing in reasoning content.
            let prefix_len = "reasoning:".len();
            reasoning = trimmed[prefix_len..].trim().to_string();
        } else if lower_key.starts_with("next_action:") {
            // Preserve original casing in action string (e.g. tap, type, scroll).
            let prefix_len = "next_action:".len();
            let action = trimmed[prefix_len..].trim();
            if !action.is_empty() {
                next_action = Some(action.to_string());
            }
        }
    }

    (done, reasoning, next_action)
}

/// Converts a typed [`ProactiveTrigger`] into a structured text description
/// for injection into the LLM system prompt.
///
/// This is **data assembly** — converting typed fields into a factual
/// description that the LLM reads. The LLM generates the user-facing
/// language; this function only structures the input facts.
fn describe_trigger(trigger: &ProactiveTrigger) -> String {
    match trigger {
        ProactiveTrigger::GoalStalled { goal_id, title, stalled_days } => {
            format!(
                "Trigger: goal_stalled\n\
                 Goal ID: {goal_id}\n\
                 Goal title: \"{title}\"\n\
                 Days without progress: {stalled_days}"
            )
        }
        ProactiveTrigger::GoalOverdue { goal_id, title, overdue_days } => {
            format!(
                "Trigger: goal_overdue\n\
                 Goal ID: {goal_id}\n\
                 Goal title: \"{title}\"\n\
                 Days overdue: {overdue_days}"
            )
        }
        ProactiveTrigger::SocialGap { contact_name, days_since_contact } => {
            format!(
                "Trigger: social_gap\n\
                 Contact: {contact_name}\n\
                 Days since last contact: {days_since_contact}"
            )
        }
        ProactiveTrigger::HealthAlert { metric, value, threshold } => {
            format!(
                "Trigger: health_alert\n\
                 Metric: {metric}\n\
                 Current value: {value:.3}\n\
                 Threshold: {threshold:.3}"
            )
        }
        ProactiveTrigger::MemoryInsight { summary } => {
            format!(
                "Trigger: memory_insight\n\
                 Pattern: \"{summary}\""
            )
        }
        ProactiveTrigger::TriggerRuleFired { rule_name, description } => {
            format!(
                "Trigger: rule_fired\n\
                 Rule: {rule_name}\n\
                 Details: {description}"
            )
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
            ScreenSummary, UserStateSignals,
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
                current_mood_dominance: 0.5,
                trust_level: 0.8,
            },
            user_state: UserStateSignals::default(),
            inference_mode: InferenceMode::Planner,
            token_budget: 4096,
            identity_block: None,
            mood_description: String::new(),
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
