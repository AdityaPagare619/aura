//! Handler dispatcher for Telegram bot commands.
//!
//! Routes parsed [`TelegramCommand`] variants to the appropriate category
//! handler. Security checks are performed *before* dispatch by the
//! [`TelegramEngine`](super::TelegramEngine) — handlers can assume the caller
//! is authorised.

pub mod agency;
pub mod ai;
pub mod config;
pub mod debug;
pub mod memory;
pub mod security;
pub mod system;

use aura_types::{config::AuraConfig, errors::AuraError};
use tracing::instrument;

use super::{
    audit::AuditLog,
    commands::TelegramCommand,
    queue::MessageQueue,
    security::SecurityGate,
    voice_handler::{CommunicationContext, CommunicationMode, VoiceHandler},
};
use crate::daemon_core::channels::{InputSource, UserCommand, UserCommandTx};

// ─── Handler context ────────────────────────────────────────────────────────

/// Shared mutable context passed to every handler.
///
/// Handlers use this to read daemon state and enqueue outgoing messages.
/// The `config` and `user_command_tx` fields give handlers access to the
/// live [`AuraConfig`] (for reading real personality, trust, power settings)
/// and the daemon's [`UserCommandTx`] channel (for forwarding commands to
/// the cognitive pipeline when the TelegramBridge fallback path is hit).
pub struct HandlerContext<'a> {
    /// Telegram chat ID of the requester.
    pub chat_id: i64,
    /// Security gate (for /lock, /unlock, /pin handlers).
    pub security: &'a mut SecurityGate,
    /// Audit log (for /audit handler).
    pub audit: &'a mut AuditLog,
    /// Offline message queue (for async responses).
    pub queue: &'a MessageQueue,
    /// Daemon startup time in epoch milliseconds.
    pub startup_time_ms: u64,
    /// Live AURA configuration — `None` only in tests or when config is unavailable.
    pub config: Option<&'a AuraConfig>,
    /// Channel to forward commands to the daemon's cognitive pipeline.
    /// Handlers that are fallbacks for daemon-routed commands (AI, memory,
    /// agency) use `try_send()` here to forward the request.
    /// `None` only in tests or when the channel is unavailable.
    pub user_command_tx: Option<&'a UserCommandTx>,
}

// ─── Handler response ───────────────────────────────────────────────────────

/// What a handler returns to the dispatcher.
#[derive(Debug, Clone)]
pub enum HandlerResponse {
    /// Plain text response.
    Text(String),
    /// HTML-formatted response (Telegram parse_mode = "HTML").
    Html(String),
    /// Photo with caption.
    Photo { data: Vec<u8>, caption: String },
    /// Voice/audio response (will be synthesized from text).
    Voice { text: String },
    /// No direct response — the handler already enqueued its output.
    Empty,
}

impl HandlerResponse {
    /// Convenience: wrap a string in HTML mode.
    pub fn html(s: impl Into<String>) -> Self {
        Self::Html(s.into())
    }

    /// Convenience: wrap a string in plain text mode.
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text(s.into())
    }

    /// Convenience: wrap a string as voice response.
    pub fn voice(s: impl Into<String>) -> Self {
        Self::Voice { text: s.into() }
    }

    /// Extract text content from response for voice synthesis.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) | Self::Html(s) => Some(s),
            Self::Voice { text } => Some(text),
            Self::Photo { .. } | Self::Empty => None,
        }
    }

    /// Apply smart communication mode selection to this response.
    /// Returns the response transformed based on the communication context.
    pub fn apply_smart_mode(self, context: &CommunicationContext) -> Self {
        let mode = VoiceHandler::detect_communication_mode(context);

        match (mode, self.clone()) {
            (CommunicationMode::Voice, Self::Text(text)) => Self::Voice { text },
            (CommunicationMode::Voice, Self::Html(text)) => Self::Voice { text },
            (CommunicationMode::Text, Self::Voice { text }) => Self::Text(text),
            (CommunicationMode::Voice, Self::Voice { .. }) => self,
            (CommunicationMode::Text, Self::Text(_)) => self,
            (CommunicationMode::Text, Self::Html(_)) => self,
            (_, other) => other,
        }
    }

    /// Determine if response should be spoken based on context.
    pub fn should_speak(&self, context: &CommunicationContext) -> bool {
        if let Some(text) = self.as_text() {
            VoiceHandler::should_speak(text, context)
        } else {
            false
        }
    }
}

// ─── Daemon routing ─────────────────────────────────────────────────────────

/// Returns `true` for commands that should be forwarded to the daemon's
/// cognitive pipeline (AI, memory, agency) rather than handled locally.
///
/// Mirrors the logic in [`TelegramBridge::is_daemon_routed`] — kept here
/// as a standalone function so the handler dispatcher can make the same
/// decision without depending on the bridge module.
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

/// Extract the text payload from a daemon-routed command so it can be
/// sent as a [`UserCommand::Chat`] through the pipeline.
fn command_to_text(cmd: &TelegramCommand) -> String {
    match cmd {
        TelegramCommand::Ask { question } => question.clone(),
        TelegramCommand::Think { problem } => format!("[think] {problem}"),
        TelegramCommand::Plan { goal } => format!("[plan] {goal}"),
        TelegramCommand::Explain { topic } => format!("[explain] {topic}"),
        TelegramCommand::Summarize { text } => format!("[summarize] {text}"),
        TelegramCommand::Translate { text, target_lang } => {
            format!("[translate:{target_lang}] {text}")
        }
        TelegramCommand::Remember { text } => format!("[remember] {text}"),
        TelegramCommand::Recall { query } => format!("[recall] {query}"),
        TelegramCommand::Forget { query } => format!("[forget] {query}"),
        TelegramCommand::Do { instruction } => format!("[do] {instruction}"),
        TelegramCommand::Open { app } => format!("[open] {app}"),
        TelegramCommand::Send {
            app,
            contact,
            message,
        } => format!("[send:{app}:{contact}] {message}"),
        TelegramCommand::Call { contact } => format!("[call] {contact}"),
        TelegramCommand::Schedule { event, time } => format!("[schedule:{time}] {event}"),
        TelegramCommand::Screenshot => "[screenshot]".to_string(),
        TelegramCommand::Navigate { destination } => format!("[navigate] {destination}"),
        TelegramCommand::Automate { routine } => format!("[automate] {routine}"),
        // Non-routed commands should never reach here, but be safe.
        other => format!("{other:?}"),
    }
}

/// Attempt to forward a daemon-routed command through the [`UserCommandTx`]
/// channel. Returns `Ok(Some(response))` if the command was forwarded (or
/// if an error response was generated), or `Ok(None)` if the channel is
/// unavailable and the caller should fall through to local handling.
fn try_forward_to_daemon(
    cmd: &TelegramCommand,
    ctx: &HandlerContext<'_>,
) -> Result<Option<HandlerResponse>, AuraError> {
    let Some(tx) = ctx.user_command_tx else {
        return Ok(None);
    };

    let text = command_to_text(cmd);
    let user_cmd = UserCommand::Chat {
        text,
        source: InputSource::Telegram {
            chat_id: ctx.chat_id,
        },
        voice_meta: None,
    };

    match tx.try_send(user_cmd) {
        Ok(()) => {
            tracing::debug!(chat_id = ctx.chat_id, cmd = ?cmd, "forwarded to daemon pipeline");
            Ok(Some(HandlerResponse::Html(
                "<i>Processing… response will arrive shortly.</i>".to_string(),
            )))
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            tracing::warn!(chat_id = ctx.chat_id, "daemon pipeline channel full");
            Ok(Some(HandlerResponse::text(
                "The daemon pipeline is busy. Please try again in a moment.",
            )))
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            tracing::error!(chat_id = ctx.chat_id, "daemon pipeline channel closed");
            // Fall through to local handler as graceful degradation.
            Ok(None)
        }
    }
}

// ─── Dispatcher ─────────────────────────────────────────────────────────────

/// Dispatch a parsed command to the correct category handler.
///
/// Security validation has already passed — this function assumes the caller
/// is authorised for the command.
#[instrument(skip(ctx), fields(chat_id = ctx.chat_id, category = cmd.category()))]
pub fn dispatch(
    cmd: &TelegramCommand,
    ctx: &mut HandlerContext<'_>,
) -> Result<HandlerResponse, AuraError> {
    // ── Daemon routing ──────────────────────────────────────────────
    // Commands that belong to the cognitive pipeline (AI, memory,
    // agency) are forwarded via the UserCommandTx channel when
    // available.  If the channel is unavailable or closed, we fall
    // through to the local stub handlers as graceful degradation.
    if is_daemon_routed(cmd) {
        if let Some(response) = try_forward_to_daemon(cmd, ctx)? {
            return Ok(response);
        }
        // Channel unavailable or closed — fall through to local handlers.
    }

    match cmd {
        // ── Control ──────────────────────────────────────────────────
        TelegramCommand::Start => system::handle_start(ctx),
        TelegramCommand::Stop => system::handle_stop(ctx),
        TelegramCommand::Reboot => system::handle_reboot(ctx),

        // ── System ──────────────────────────────────────────────────
        TelegramCommand::Status => system::handle_status(ctx),
        TelegramCommand::Health => system::handle_health(ctx),
        TelegramCommand::Restart => system::handle_restart(ctx),
        TelegramCommand::Logs { service, lines } => {
            system::handle_logs(ctx, service.as_deref(), *lines)
        }
        TelegramCommand::Uptime => system::handle_uptime(ctx),
        TelegramCommand::Version => system::handle_version(ctx),
        TelegramCommand::Power => system::handle_power(ctx),

        // ── AI ──────────────────────────────────────────────────────
        TelegramCommand::Ask { question } => ai::handle_ask(ctx, question),
        TelegramCommand::Think { problem } => ai::handle_think(ctx, problem),
        TelegramCommand::Plan { goal } => ai::handle_plan(ctx, goal),
        TelegramCommand::Explain { topic } => ai::handle_explain(ctx, topic),
        TelegramCommand::Summarize { text } => ai::handle_summarize(ctx, text),
        TelegramCommand::Translate { text, target_lang } => {
            ai::handle_translate(ctx, text, target_lang)
        }

        // ── Memory ─────────────────────────────────────────────────
        TelegramCommand::Remember { text } => memory::handle_remember(ctx, text),
        TelegramCommand::Recall { query } => memory::handle_recall(ctx, query),
        TelegramCommand::Forget { query } => memory::handle_forget(ctx, query),
        TelegramCommand::Memories { filter } => memory::handle_memories(ctx, filter.as_deref()),
        TelegramCommand::Consolidate => memory::handle_consolidate(ctx),
        TelegramCommand::MemoryStats => memory::handle_memory_stats(ctx),

        // ── Agency ─────────────────────────────────────────────────
        TelegramCommand::Do { instruction } => agency::handle_do(ctx, instruction),
        TelegramCommand::Open { app } => agency::handle_open(ctx, app),
        TelegramCommand::Send {
            app,
            contact,
            message,
        } => agency::handle_send(ctx, app, contact, message),
        TelegramCommand::Call { contact } => agency::handle_call(ctx, contact),
        TelegramCommand::Schedule { event, time } => agency::handle_schedule(ctx, event, time),
        TelegramCommand::Screenshot => agency::handle_screenshot(ctx),
        TelegramCommand::Navigate { destination } => agency::handle_navigate(ctx, destination),
        TelegramCommand::Automate { routine } => agency::handle_automate(ctx, routine),

        // ── Config ─────────────────────────────────────────────────
        TelegramCommand::Set { key, value } => config::handle_set(ctx, key, value),
        TelegramCommand::Get { key } => config::handle_get(ctx, key),
        TelegramCommand::Personality => config::handle_personality(ctx),
        TelegramCommand::PersonalitySet { trait_name, value } => {
            config::handle_personality_set(ctx, trait_name, *value)
        }
        TelegramCommand::Trust => config::handle_trust(ctx),
        TelegramCommand::TrustSet { level } => config::handle_trust_set(ctx, *level),
        TelegramCommand::Voice => config::handle_voice_mode(ctx),
        TelegramCommand::Chat => config::handle_chat_mode(ctx),
        TelegramCommand::Quiet => config::handle_quiet(ctx),
        TelegramCommand::Wake => config::handle_wake(ctx),

        // ── Security ───────────────────────────────────────────────
        TelegramCommand::Pin { action } => security::handle_pin(ctx, action),
        TelegramCommand::Lock => security::handle_lock(ctx),
        TelegramCommand::Unlock { pin } => security::handle_unlock(ctx, pin),
        TelegramCommand::Audit { lines } => security::handle_audit(ctx, *lines),
        TelegramCommand::Permissions => security::handle_permissions(ctx),

        // ── Debug ──────────────────────────────────────────────────
        TelegramCommand::Trace { module } => debug::handle_trace(ctx, module),
        TelegramCommand::Dump { component } => debug::handle_dump(ctx, component),
        TelegramCommand::Perf => debug::handle_perf(ctx),
        TelegramCommand::Etg { app } => debug::handle_etg(ctx, app.as_deref()),
        TelegramCommand::Goals => debug::handle_goals(ctx),

        // ── Meta ───────────────────────────────────────────────────
        TelegramCommand::Help { command } => {
            let text = match command {
                Some(name) => super::commands::command_help(name),
                None => super::commands::full_help_text(),
            };
            Ok(HandlerResponse::Html(text))
        }
        TelegramCommand::Unknown { raw } => Ok(HandlerResponse::text(format!(
            "Unknown command: {raw}\nUse /help for available commands."
        ))),
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::telegram::{audit::AuditLog, queue::MessageQueue, security::SecurityGate};

    fn test_ctx<'a>(
        security: &'a mut SecurityGate,
        audit: &'a mut AuditLog,
        queue: &'a MessageQueue,
    ) -> HandlerContext<'a> {
        HandlerContext {
            chat_id: 42,
            security,
            audit,
            queue,
            startup_time_ms: 1_700_000_000_000,
            config: None,
            user_command_tx: None,
        }
    }

    #[test]
    fn test_dispatch_help() {
        let mut security = SecurityGate::new(vec![42]);
        let mut audit = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let queue = MessageQueue::open(db).unwrap();
        let mut ctx = test_ctx(&mut security, &mut audit, &queue);

        let resp = dispatch(&TelegramCommand::Help { command: None }, &mut ctx).unwrap();
        match resp {
            HandlerResponse::Html(text) => {
                assert!(text.contains("AURA Telegram Commands"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_dispatch_unknown() {
        let mut security = SecurityGate::new(vec![42]);
        let mut audit = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let queue = MessageQueue::open(db).unwrap();
        let mut ctx = test_ctx(&mut security, &mut audit, &queue);

        let resp = dispatch(&TelegramCommand::Unknown { raw: "/foo".into() }, &mut ctx).unwrap();
        match resp {
            HandlerResponse::Text(text) => {
                assert!(text.contains("Unknown command"));
                assert!(text.contains("/foo"));
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn test_dispatch_status() {
        let mut security = SecurityGate::new(vec![42]);
        let mut audit = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let queue = MessageQueue::open(db).unwrap();
        let mut ctx = test_ctx(&mut security, &mut audit, &queue);

        let resp = dispatch(&TelegramCommand::Status, &mut ctx).unwrap();
        match resp {
            HandlerResponse::Html(text) => {
                assert!(text.contains("AURA Status"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    // ── Daemon routing tests ────────────────────────────────────────

    #[test]
    fn test_is_daemon_routed_ai_commands() {
        assert!(is_daemon_routed(&TelegramCommand::Ask {
            question: "hi".into()
        }));
        assert!(is_daemon_routed(&TelegramCommand::Think {
            problem: "x".into()
        }));
        assert!(is_daemon_routed(&TelegramCommand::Plan {
            goal: "x".into()
        }));
        assert!(is_daemon_routed(&TelegramCommand::Explain {
            topic: "x".into()
        }));
        assert!(is_daemon_routed(&TelegramCommand::Summarize {
            text: "x".into()
        }));
        assert!(is_daemon_routed(&TelegramCommand::Translate {
            text: "x".into(),
            target_lang: "en".into()
        }));
    }

    #[test]
    fn test_is_daemon_routed_memory_commands() {
        assert!(is_daemon_routed(&TelegramCommand::Remember {
            text: "x".into()
        }));
        assert!(is_daemon_routed(&TelegramCommand::Recall {
            query: "x".into()
        }));
        assert!(is_daemon_routed(&TelegramCommand::Forget {
            query: "x".into()
        }));
    }

    #[test]
    fn test_is_daemon_routed_agency_commands() {
        assert!(is_daemon_routed(&TelegramCommand::Do {
            instruction: "x".into()
        }));
        assert!(is_daemon_routed(&TelegramCommand::Open { app: "x".into() }));
        assert!(is_daemon_routed(&TelegramCommand::Screenshot));
        assert!(is_daemon_routed(&TelegramCommand::Navigate {
            destination: "x".into()
        }));
    }

    #[test]
    fn test_is_not_daemon_routed() {
        assert!(!is_daemon_routed(&TelegramCommand::Status));
        assert!(!is_daemon_routed(&TelegramCommand::Help { command: None }));
        assert!(!is_daemon_routed(&TelegramCommand::Lock));
        assert!(!is_daemon_routed(&TelegramCommand::Personality));
        assert!(!is_daemon_routed(&TelegramCommand::Perf));
    }

    #[test]
    fn test_command_to_text_plain() {
        let text = command_to_text(&TelegramCommand::Ask {
            question: "what is Rust?".into(),
        });
        assert_eq!(text, "what is Rust?");
    }

    #[test]
    fn test_command_to_text_prefixed() {
        let text = command_to_text(&TelegramCommand::Think {
            problem: "halting".into(),
        });
        assert_eq!(text, "[think] halting");

        let text = command_to_text(&TelegramCommand::Plan {
            goal: "world peace".into(),
        });
        assert_eq!(text, "[plan] world peace");

        let text = command_to_text(&TelegramCommand::Translate {
            text: "hello".into(),
            target_lang: "es".into(),
        });
        assert_eq!(text, "[translate:es] hello");
    }

    #[test]
    fn test_command_to_text_agency() {
        let text = command_to_text(&TelegramCommand::Send {
            app: "telegram".into(),
            contact: "alice".into(),
            message: "hi there".into(),
        });
        assert_eq!(text, "[send:telegram:alice] hi there");

        let text = command_to_text(&TelegramCommand::Screenshot);
        assert_eq!(text, "[screenshot]");
    }

    #[test]
    fn test_try_forward_no_channel() {
        // When user_command_tx is None, should return Ok(None) — fall through.
        let mut security = SecurityGate::new(vec![42]);
        let mut audit = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let queue = MessageQueue::open(db).unwrap();
        let ctx = HandlerContext {
            chat_id: 42,
            security: &mut security,
            audit: &mut audit,
            queue: &queue,
            startup_time_ms: 1_700_000_000_000,
            config: None,
            user_command_tx: None,
        };

        let cmd = TelegramCommand::Ask {
            question: "test".into(),
        };
        let result = try_forward_to_daemon(&cmd, &ctx).unwrap();
        assert!(result.is_none(), "should fall through when no channel");
    }

    #[test]
    fn test_try_forward_with_channel() {
        // Create a real tokio mpsc channel and verify the command is forwarded.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<UserCommand>(8);
        let mut security = SecurityGate::new(vec![42]);
        let mut audit = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let queue = MessageQueue::open(db).unwrap();
        let ctx = HandlerContext {
            chat_id: 42,
            security: &mut security,
            audit: &mut audit,
            queue: &queue,
            startup_time_ms: 1_700_000_000_000,
            config: None,
            user_command_tx: Some(&tx),
        };

        let cmd = TelegramCommand::Ask {
            question: "what is Rust?".into(),
        };
        let result = try_forward_to_daemon(&cmd, &ctx).unwrap();
        assert!(result.is_some(), "should return a response when forwarded");

        // Verify the message was actually sent through the channel.
        let received = rx.try_recv().expect("should have received a UserCommand");
        match received {
            UserCommand::Chat { text, source, .. } => {
                assert_eq!(text, "what is Rust?");
                match source {
                    InputSource::Telegram { chat_id } => assert_eq!(chat_id, 42),
                    other => panic!("expected Telegram source, got {other:?}"),
                }
            }
            other => panic!("expected Chat command, got {other:?}"),
        }
    }

    #[test]
    fn test_dispatch_forwards_ask_to_daemon() {
        // When a channel is available, /ask should be forwarded — not handled locally.
        let (tx, _rx) = tokio::sync::mpsc::channel::<UserCommand>(8);
        let mut security = SecurityGate::new(vec![42]);
        let mut audit = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let queue = MessageQueue::open(db).unwrap();
        let mut ctx = HandlerContext {
            chat_id: 42,
            security: &mut security,
            audit: &mut audit,
            queue: &queue,
            startup_time_ms: 1_700_000_000_000,
            config: None,
            user_command_tx: Some(&tx),
        };

        let cmd = TelegramCommand::Ask {
            question: "test".into(),
        };
        let resp = dispatch(&cmd, &mut ctx).unwrap();
        match resp {
            HandlerResponse::Html(text) => {
                assert!(
                    text.contains("Processing"),
                    "expected forwarding ack, got: {text}"
                );
            }
            other => panic!("expected Html forwarding ack, got {other:?}"),
        }
    }

    #[test]
    fn test_dispatch_falls_through_without_channel() {
        // When no channel is available, /ask should fall through to the local stub.
        let mut security = SecurityGate::new(vec![42]);
        let mut audit = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let queue = MessageQueue::open(db).unwrap();
        let mut ctx = test_ctx(&mut security, &mut audit, &queue);

        let cmd = TelegramCommand::Ask {
            question: "test fallback".into(),
        };
        let resp = dispatch(&cmd, &mut ctx).unwrap();
        // Should get the local stub response, not a forwarding ack.
        match &resp {
            HandlerResponse::Html(_) | HandlerResponse::Text(_) => {
                // The local ai::handle_ask stub should return something —
                // just verify it didn't return the forwarding message.
                if let Some(text) = resp.as_text() {
                    assert!(
                        !text.contains("Processing… response will arrive shortly"),
                        "should NOT get forwarding ack when no channel: {text}"
                    );
                }
            }
            other => panic!("expected text response from stub, got {other:?}"),
        }
    }
}
