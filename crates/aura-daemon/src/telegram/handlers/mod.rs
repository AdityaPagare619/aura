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

use aura_types::errors::AuraError;
use tracing::instrument;

use super::audit::AuditLog;
use super::commands::TelegramCommand;
use super::queue::MessageQueue;
use super::security::SecurityGate;
use super::voice_handler::{
    CommunicationContext, CommunicationMode, VoiceHandler, VoiceModePreference,
};

// ─── Handler context ────────────────────────────────────────────────────────

/// Shared mutable context passed to every handler.
///
/// Handlers use this to read daemon state and enqueue outgoing messages.
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
    use super::*;
    use crate::telegram::audit::AuditLog;
    use crate::telegram::queue::MessageQueue;
    use crate::telegram::security::SecurityGate;
    use rusqlite::Connection;

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
}
