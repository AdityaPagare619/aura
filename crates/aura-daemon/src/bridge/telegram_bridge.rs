//! Telegram ↔ Daemon bridge.
//!
//! Wraps the [`TelegramEngine`] and intercepts commands that should flow
//! through the daemon's processing pipeline (AI queries, task requests,
//! memory operations) rather than being handled locally by the Telegram
//! handler layer.
//!
//! Commands that are purely Telegram-local (status, security, config,
//! debug) continue to be handled by the engine's own dispatch loop.
//!
//! # Data flow
//!
//! ```text
//!  Telegram poll ──► parse ──► is_daemon_routed?
//!                                ├── yes ──► UserCommand ──► daemon pipeline
//!                                └── no  ──► local handler dispatch
//!
//!  DaemonResponse ──► destination == Telegram{chat_id} ──► queue.enqueue()
//! ```

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use async_trait::async_trait;
use tracing::{debug, info, warn};

use crate::{
    bridge::{BridgeError, BridgeResult, InputChannel},
    daemon_core::channels::{DaemonResponseRx, InputSource, UserCommand, UserCommandTx},
    telegram::{
        commands::TelegramCommand,
        queue::{MessageContent, MessageQueue},
        voice_handler::{CommunicationContext, VoiceHandler, VoiceModePreference},
        TelegramConfig,
    },
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Controls which Telegram commands are forwarded to the daemon pipeline
/// vs. handled locally by the Telegram engine.
///
/// The split is intentional: system/security/config/debug commands are
/// Telegram-specific and don't benefit from the daemon's AI pipeline.
/// AI, memory, and agency commands produce richer results when routed
/// through the daemon (which has access to the full context window,
/// memory system, and execution engine).
#[allow(dead_code)] // Phase 8: used by TelegramBridge routing decision
fn is_daemon_routed(cmd: &TelegramCommand) -> bool {
    matches!(
        cmd,
        // AI commands — benefit from full pipeline context.
        TelegramCommand::Ask { .. }
            | TelegramCommand::Think { .. }
            | TelegramCommand::Plan { .. }
            | TelegramCommand::Explain { .. }
            | TelegramCommand::Summarize { .. }
            | TelegramCommand::Translate { .. }
            // Memory commands — the daemon owns the memory system.
            | TelegramCommand::Remember { .. }
            | TelegramCommand::Recall { .. }
            | TelegramCommand::Forget { .. }
            // Agency commands — the daemon owns the execution engine.
            | TelegramCommand::Do { .. }
            | TelegramCommand::Open { .. }
            | TelegramCommand::Send { .. }
            | TelegramCommand::Call { .. }
            | TelegramCommand::Schedule { .. }
            | TelegramCommand::Screenshot
            | TelegramCommand::Navigate { .. }
            | TelegramCommand::Automate { .. }
    )
}

/// Extract the raw user payload from a daemon-routed command.
///
/// # Iron Law compliance (Law #3 — no directive injection)
///
/// This function returns ONLY the raw user-provided content.  It must NOT
/// prepend semantic directive markers such as `[think]`, `[plan]`,
/// `[remember]`, etc.  Injecting such markers into the text that reaches the
/// Neocortex would constitute prompt-engineering in Rust — the daemon telling
/// the LLM how to behave rather than letting the LLM reason from the
/// `InferenceMode` already present in the `ContextPackage`.
///
/// Routing intent is conveyed exclusively through the typed `InferenceMode`
/// field of the `ContextPackage` and the `UserCommand` variant
/// (`Chat` vs `TaskRequest`).  The LLM reads both and acts accordingly.
#[allow(dead_code)] // Phase 8: used by to_user_command for command text extraction
fn command_to_text(cmd: &TelegramCommand) -> String {
    match cmd {
        TelegramCommand::Ask { question } => question.clone(),
        TelegramCommand::Think { problem } => problem.clone(),
        TelegramCommand::Plan { goal } => goal.clone(),
        TelegramCommand::Explain { topic } => topic.clone(),
        TelegramCommand::Summarize { text } => text.clone(),
        TelegramCommand::Translate {
            text,
            target_lang: _,
        } => text.clone(),
        TelegramCommand::Remember { text } => text.clone(),
        TelegramCommand::Recall { query } => query.clone(),
        TelegramCommand::Forget { query } => query.clone(),
        TelegramCommand::Do { instruction } => instruction.clone(),
        TelegramCommand::Open { app } => app.clone(),
        TelegramCommand::Send {
            app: _,
            contact: _,
            message,
        } => message.clone(),
        TelegramCommand::Call { contact } => contact.clone(),
        TelegramCommand::Schedule { event, time: _ } => event.clone(),
        TelegramCommand::Screenshot => String::new(),
        TelegramCommand::Navigate { destination } => destination.clone(),
        TelegramCommand::Automate { routine } => routine.clone(),
        // Non-routed commands should never reach here, but be safe.
        other => format!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// TelegramBridge
// ---------------------------------------------------------------------------

/// Bridge between Telegram and the daemon processing pipeline.
///
/// Intercepts daemon-routed commands from the Telegram update stream,
/// converts them to [`UserCommand`] messages, and delivers daemon
/// responses back to the appropriate Telegram chat via the message queue.
pub struct TelegramBridge {
    /// Telegram configuration (for chat IDs and creating the queue).
    config: TelegramConfig,
    /// Shared cancellation flag.
    cancel: Arc<AtomicBool>,
    /// Offline message queue for delivering responses.
    queue: Option<MessageQueue>,
}

impl TelegramBridge {
    /// Create a new Telegram bridge.
    ///
    /// The `queue` is used to deliver daemon responses back to the
    /// Telegram chat. If `None`, responses are logged but not delivered.
    pub fn new(
        config: TelegramConfig,
        cancel: Arc<AtomicBool>,
        queue: Option<MessageQueue>,
    ) -> Self {
        Self {
            config,
            cancel,
            queue,
        }
    }

    /// The primary chat ID from config (first in the whitelist).
    fn primary_chat_id(&self) -> i64 {
        self.config.allowed_chat_ids.first().copied().unwrap_or(0)
    }

    /// Convert a parsed Telegram command to a [`UserCommand`] for the
    /// daemon pipeline, tagged with the originating chat ID.
    #[allow(dead_code)] // Phase 8: called by TelegramBridge message handler
    fn to_user_command(cmd: &TelegramCommand, chat_id: i64) -> UserCommand {
        let text = command_to_text(cmd);
        let source = InputSource::Telegram { chat_id };

        // Determine if this is a task request or a chat message.
        match cmd {
            TelegramCommand::Do { instruction } => UserCommand::TaskRequest {
                description: instruction.clone(),
                priority: 1,
                source,
            },
            _ => UserCommand::Chat {
                text,
                source,
                voice_meta: None,
            },
        }
    }

    /// Deliver a daemon response to a Telegram chat via the message queue.
    fn deliver_response(&self, chat_id: i64, text: &str) -> BridgeResult<()> {
        match &self.queue {
            Some(queue) => {
                queue
                    .enqueue(
                        chat_id,
                        &MessageContent::Text {
                            text: text.to_string(),
                            parse_mode: None,
                        },
                        0,    // priority
                        3600, // ttl_secs
                        3,    // max_retries
                        None, // coalesce_key
                    )
                    .map_err(|e| BridgeError::Upstream(format!("queue enqueue failed: {e}")))?;
                Ok(())
            }
            None => {
                debug!(chat_id, len = text.len(), "no queue — response logged only");
                Ok(())
            }
        }
    }

    /// Deliver a response with smart voice mode selection.
    /// Uses the communication context to decide whether to use voice.
    #[allow(dead_code)] // Phase 8: called by TelegramBridge response delivery path
    fn deliver_smart_response(
        &self,
        chat_id: i64,
        text: &str,
        user_preference: VoiceModePreference,
        last_was_voice: bool,
    ) -> BridgeResult<bool> {
        let context = CommunicationContext::new(chat_id)
            .with_preference(user_preference)
            .with_last_voice_status(last_was_voice);

        let should_speak = VoiceHandler::should_speak(text, &context);

        if should_speak {
            if let Some(queue) = &self.queue {
                queue
                    .enqueue(
                        chat_id,
                        &MessageContent::Text {
                            text: text.to_string(),
                            parse_mode: None,
                        },
                        1,
                        3600,
                        3,
                        Some("voice"),
                    )
                    .map_err(|e| {
                        BridgeError::Upstream(format!("voice queue enqueue failed: {e}"))
                    })?;
                debug!(chat_id, len = text.len(), "response delivered as voice");
            }
            return Ok(true);
        }

        self.deliver_response(chat_id, text)?;
        Ok(false)
    }
}

#[async_trait]
impl InputChannel for TelegramBridge {
    fn name(&self) -> &str {
        "telegram"
    }

    fn source(&self) -> InputSource {
        InputSource::Telegram {
            chat_id: self.primary_chat_id(),
        }
    }

    async fn run(
        &mut self,
        _cmd_tx: UserCommandTx,
        mut response_rx: DaemonResponseRx,
    ) -> BridgeResult<()> {
        info!(
            primary_chat = self.primary_chat_id(),
            allowed_chats = self.config.allowed_chat_ids.len(),
            "telegram bridge starting"
        );

        // The bridge's main job on the response side is to deliver
        // DaemonResponse messages back to the correct Telegram chat.
        //
        // The *input* side (polling → parse → route) is handled by the
        // TelegramEngine's run() loop. In a full integration, the engine's
        // handle_update() would call is_daemon_routed() and forward to
        // cmd_tx instead of dispatching locally. For now, the bridge
        // monitors the response channel.

        loop {
            if self.cancel.load(Ordering::Relaxed) {
                info!("telegram bridge shutting down (cancel flag)");
                break;
            }

            // Check for daemon responses to deliver.
            match response_rx.try_recv() {
                Ok(response) => {
                    // Route based on destination.
                    if let InputSource::Telegram { chat_id } = response.destination {
                        debug!(
                            chat_id,
                            len = response.text.len(),
                            "delivering daemon response"
                        );
                        if let Err(e) = self.deliver_response(chat_id, &response.text) {
                            warn!(error = %e, chat_id, "failed to deliver response");
                        }
                    }
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    // No response pending — yield and check again.
                    tokio::task::yield_now().await;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    info!("response channel closed — telegram bridge exiting");
                    break;
                }
            }
        }

        info!("telegram bridge stopped");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;
    use crate::daemon_core::channels::DaemonResponse;

    fn test_config() -> TelegramConfig {
        TelegramConfig {
            bot_token: "test:TOKEN".into(),
            allowed_chat_ids: vec![42, 99],
            ..TelegramConfig::default()
        }
    }

    // -- is_daemon_routed tests --

    #[test]
    fn test_ask_is_daemon_routed() {
        let cmd = TelegramCommand::Ask {
            question: "what time is it?".into(),
        };
        assert!(is_daemon_routed(&cmd));
    }

    #[test]
    fn test_status_is_not_daemon_routed() {
        assert!(!is_daemon_routed(&TelegramCommand::Status));
    }

    #[test]
    fn test_do_is_daemon_routed() {
        let cmd = TelegramCommand::Do {
            instruction: "open calculator".into(),
        };
        assert!(is_daemon_routed(&cmd));
    }

    #[test]
    fn test_lock_is_not_daemon_routed() {
        assert!(!is_daemon_routed(&TelegramCommand::Lock));
    }

    #[test]
    fn test_remember_is_daemon_routed() {
        let cmd = TelegramCommand::Remember {
            text: "meeting at 3pm".into(),
        };
        assert!(is_daemon_routed(&cmd));
    }

    #[test]
    fn test_help_is_not_daemon_routed() {
        let cmd = TelegramCommand::Help { command: None };
        assert!(!is_daemon_routed(&cmd));
    }

    // -- command_to_text tests --

    #[test]
    fn test_command_to_text_ask() {
        let cmd = TelegramCommand::Ask {
            question: "hello".into(),
        };
        assert_eq!(command_to_text(&cmd), "hello");
    }

    #[test]
    fn test_command_to_text_think() {
        let cmd = TelegramCommand::Think {
            problem: "optimization".into(),
        };
        assert_eq!(command_to_text(&cmd), "optimization");
    }

    #[test]
    fn test_command_to_text_translate() {
        let cmd = TelegramCommand::Translate {
            text: "hello".into(),
            target_lang: "es".into(),
        };
        assert_eq!(command_to_text(&cmd), "hello");
    }

    #[test]
    fn test_command_to_text_send() {
        let cmd = TelegramCommand::Send {
            app: "whatsapp".into(),
            contact: "John".into(),
            message: "Hi there".into(),
        };
        assert_eq!(command_to_text(&cmd), "Hi there");
    }

    #[test]
    fn test_command_to_text_screenshot() {
        assert_eq!(command_to_text(&TelegramCommand::Screenshot), "");
    }

    // -- to_user_command tests --

    #[test]
    fn test_to_user_command_ask_produces_chat() {
        let cmd = TelegramCommand::Ask {
            question: "hello world".into(),
        };
        let uc = TelegramBridge::to_user_command(&cmd, 42);
        match uc {
            UserCommand::Chat {
                text,
                source,
                voice_meta,
            } => {
                assert_eq!(text, "hello world");
                assert_eq!(source, InputSource::Telegram { chat_id: 42 });
                assert!(voice_meta.is_none());
            }
            _ => panic!("expected Chat, got {uc:?}"),
        }
    }

    #[test]
    fn test_to_user_command_do_produces_task_request() {
        let cmd = TelegramCommand::Do {
            instruction: "open calculator".into(),
        };
        let uc = TelegramBridge::to_user_command(&cmd, 99);
        match uc {
            UserCommand::TaskRequest {
                description,
                priority,
                source,
            } => {
                assert_eq!(description, "open calculator");
                assert_eq!(priority, 1);
                assert_eq!(source, InputSource::Telegram { chat_id: 99 });
            }
            _ => panic!("expected TaskRequest, got {uc:?}"),
        }
    }

    // -- TelegramBridge structural tests --

    #[test]
    fn test_bridge_name_and_source() {
        let config = test_config();
        let cancel = Arc::new(AtomicBool::new(false));
        let bridge = TelegramBridge::new(config, cancel, None);

        assert_eq!(bridge.name(), "telegram");
        assert_eq!(bridge.source(), InputSource::Telegram { chat_id: 42 });
    }

    #[test]
    fn test_primary_chat_id() {
        let config = test_config();
        let cancel = Arc::new(AtomicBool::new(false));
        let bridge = TelegramBridge::new(config, cancel, None);
        assert_eq!(bridge.primary_chat_id(), 42);
    }

    #[test]
    fn test_primary_chat_id_empty_config() {
        let config = TelegramConfig {
            allowed_chat_ids: vec![],
            ..TelegramConfig::default()
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let bridge = TelegramBridge::new(config, cancel, None);
        assert_eq!(bridge.primary_chat_id(), 0);
    }

    #[tokio::test]
    async fn test_bridge_cancel_flag_exits() {
        let config = test_config();
        let cancel = Arc::new(AtomicBool::new(true)); // Pre-set cancel.
        let mut bridge = TelegramBridge::new(config, cancel, None);

        let (cmd_tx, _cmd_rx) = mpsc::channel(16);
        let (_resp_tx, resp_rx) = mpsc::channel::<DaemonResponse>(16);

        let result = bridge.run(cmd_tx, resp_rx).await;
        assert!(
            result.is_ok(),
            "bridge.run with pre-cancelled token failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn test_bridge_response_channel_close_exits() {
        let config = test_config();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut bridge = TelegramBridge::new(config, cancel, None);

        let (cmd_tx, _cmd_rx) = mpsc::channel(16);
        let (_resp_tx, resp_rx) = mpsc::channel::<DaemonResponse>(16);

        // Drop the sender so the receiver sees Disconnected.
        drop(_resp_tx);

        let result = bridge.run(cmd_tx, resp_rx).await;
        assert!(
            result.is_ok(),
            "bridge.run with closed response channel failed: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_deliver_response_no_queue() {
        let config = test_config();
        let cancel = Arc::new(AtomicBool::new(false));
        let bridge = TelegramBridge::new(config, cancel, None);

        // Should succeed (just logs).
        let result = bridge.deliver_response(42, "hello");
        assert!(
            result.is_ok(),
            "deliver_response with no queue failed: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_deliver_response_with_queue() {
        let db = rusqlite::Connection::open_in_memory().expect("in-memory db");
        let queue = MessageQueue::open(db).expect("queue");
        let config = test_config();
        let cancel = Arc::new(AtomicBool::new(false));
        let bridge = TelegramBridge::new(config, cancel, Some(queue));

        let result = bridge.deliver_response(42, "response text");
        assert!(
            result.is_ok(),
            "deliver_response with queue failed: {:?}",
            result.err()
        );

        // Verify the message was enqueued.
        let pending = bridge
            .queue
            .as_ref()
            .unwrap()
            .pending_count()
            .expect("count");
        assert_eq!(pending, 1);
    }

    // -- Routing coverage --

    #[test]
    fn test_all_agency_commands_are_daemon_routed() {
        let cmds = vec![
            TelegramCommand::Open { app: "x".into() },
            TelegramCommand::Send {
                app: "a".into(),
                contact: "b".into(),
                message: "c".into(),
            },
            TelegramCommand::Call {
                contact: "d".into(),
            },
            TelegramCommand::Schedule {
                event: "e".into(),
                time: "f".into(),
            },
            TelegramCommand::Screenshot,
            TelegramCommand::Navigate {
                destination: "g".into(),
            },
            TelegramCommand::Automate {
                routine: "h".into(),
            },
        ];
        for cmd in &cmds {
            assert!(is_daemon_routed(cmd), "expected daemon-routed: {cmd:?}");
        }
    }

    #[test]
    fn test_all_local_commands_are_not_daemon_routed() {
        let cmds = vec![
            TelegramCommand::Status,
            TelegramCommand::Health,
            TelegramCommand::Restart,
            TelegramCommand::Uptime,
            TelegramCommand::Version,
            TelegramCommand::Power,
            TelegramCommand::Lock,
            TelegramCommand::Permissions,
            TelegramCommand::Perf,
            TelegramCommand::Goals,
            TelegramCommand::Help { command: None },
            TelegramCommand::Unknown { raw: "x".into() },
        ];
        for cmd in &cmds {
            assert!(
                !is_daemon_routed(cmd),
                "expected NOT daemon-routed: {cmd:?}"
            );
        }
    }
}
