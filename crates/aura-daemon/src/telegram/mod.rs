//! Telegram bot module — remote command center for AURA.
//!
//! # Architecture
//!
//! - [`TelegramEngine`] — top-level facade that owns all subsystems.
//! - [`polling`] — Pure HTTP long-polling (no teloxide dependency).
//! - [`commands`] — 43-command parser with aliases and categories.
//! - [`handlers`] — Command dispatch and per-category handler modules.
//! - [`security`] — 5-layer security gate (whitelist, PIN, permissions,
//!   rate-limit, audit).
//! - [`audit`] — Ring-buffer audit trail for all command attempts.
//! - [`queue`] — SQLite offline message queue with priority and coalesce.
//! - [`dashboard`] — HTML health dashboard generator.
//! - [`approval`] — Last Mile Approval via PolicyGate.
//! - [`dialogue`] — FSM multi-step conversation flows.
//!
//! # Security Layers
//!
//! Every incoming Telegram message passes through 5 security layers before
//! any handler executes:
//!
//! 1. **Chat ID whitelist** — fast reject of unknown senders (in poller).
//! 2. **Argon2id PIN** — optional lock requiring constant-time verification.
//! 3. **Per-command permissions** — role-based access control.
//! 4. **Rate limiting** — sliding-window per chat ID.
//! 5. **Audit trail** — every attempt logged with outcome.

pub mod approval;
pub mod audit;
pub mod commands;
pub mod dashboard;
pub mod dialogue;
pub mod handlers;
pub mod polling;
pub mod queue;
pub mod reqwest_backend;
pub mod security;
pub mod voice_handler;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use rusqlite::Connection;
use tokio::sync::mpsc;
use tracing::{info, instrument, warn};

use aura_types::config::AuraConfig;
use aura_types::errors::AuraError;

use crate::daemon_core::channels::UserCommandTx;

use self::approval::PolicyGate;
use self::audit::{AuditLog, AuditOutcome};
use self::commands::TelegramCommand;
use self::dashboard::DashboardSnapshot;
use self::dialogue::DialogueManager;
use self::handlers::{HandlerContext, HandlerResponse};
use self::polling::{HttpBackend, StubHttpBackend, TelegramPoller, TelegramUpdate};
use self::queue::{MessageContent, MessageQueue};
use self::security::SecurityGate;

// ─── Configuration ──────────────────────────────────────────────────────────

/// Configuration for the Telegram bot module.
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    /// Telegram Bot API token.
    pub bot_token: String,
    /// Allowed chat IDs (first is primary / admin).
    pub allowed_chat_ids: Vec<i64>,
    /// Default trust level for the PolicyGate (0.0–1.0).
    pub trust_level: f32,
    /// Audit log capacity (number of entries).
    pub audit_capacity: usize,
    /// Rate limit: max commands per minute.
    pub rate_per_minute: u32,
    /// Rate limit: max commands per hour.
    pub rate_per_hour: u32,
    /// Approval request TTL in seconds.
    pub approval_ttl_secs: u64,
    /// Dialogue timeout in seconds.
    pub dialogue_timeout_secs: u64,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            bot_token: String::new(),
            allowed_chat_ids: Vec::new(),
            trust_level: 0.5,
            audit_capacity: 1000,
            rate_per_minute: 30,
            rate_per_hour: 300,
            approval_ttl_secs: 300,
            dialogue_timeout_secs: 120,
        }
    }
}

// ─── TelegramEngine ─────────────────────────────────────────────────────────

/// Top-level Telegram bot engine.
///
/// Owns all subsystems and orchestrates the message processing pipeline:
/// poll → security → dialogue/dispatch → respond → audit.
pub struct TelegramEngine {
    /// The long-polling client.
    poller: TelegramPoller,
    /// 5-layer security gate.
    security: SecurityGate,
    /// Audit log.
    audit: AuditLog,
    /// Offline message queue.
    queue: MessageQueue,
    /// PolicyGate for last-mile approval.
    policy_gate: PolicyGate,
    /// FSM dialogue manager.
    dialogue_mgr: DialogueManager,
    /// Daemon startup time (epoch ms).
    startup_time_ms: u64,
    /// Cancellation flag (shared with daemon).
    cancel_flag: Arc<AtomicBool>,
    /// Primary chat ID for unsolicited messages (alerts, approvals).
    primary_chat_id: i64,
    /// Live AURA configuration snapshot — gives handlers access to real
    /// personality, trust, power, and other config values.
    aura_config: Option<AuraConfig>,
    /// Sender half of the daemon's user-command channel. Handlers use this
    /// to forward daemon-routed commands (AI, memory, agency) that reach
    /// the fallback path instead of being intercepted by TelegramBridge.
    user_command_tx: Option<UserCommandTx>,
}

impl TelegramEngine {
    /// Create a new Telegram engine from config.
    ///
    /// The `queue_db` connection is used for the offline message queue.
    /// The `http` backend is used for Telegram API calls.
    #[instrument(skip(config, queue_db, http, cancel_flag))]
    pub fn new(
        config: TelegramConfig,
        queue_db: Connection,
        http: Box<dyn HttpBackend>,
        startup_time_ms: u64,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<Self, AuraError> {
        let primary_chat_id = config
            .allowed_chat_ids
            .first()
            .copied()
            .unwrap_or(0);

        let queue = MessageQueue::open(queue_db)?;
        let security = SecurityGate::new(config.allowed_chat_ids.clone());
        let audit = AuditLog::new(config.audit_capacity);
        let poller = TelegramPoller::new(
            config.bot_token.clone(),
            config.allowed_chat_ids.clone(),
            http,
        );
        let policy_gate = PolicyGate::new(config.approval_ttl_secs);
        let dialogue_mgr = DialogueManager::new(config.dialogue_timeout_secs);

        info!(
            allowed_chats = config.allowed_chat_ids.len(),
            trust = config.trust_level,
            "Telegram engine initialized"
        );

        Ok(Self {
            poller,
            security,
            audit,
            queue,
            policy_gate,
            dialogue_mgr,
            startup_time_ms,
            cancel_flag,
            primary_chat_id,
            aura_config: None,
            user_command_tx: None,
        })
    }

    /// Create an engine with the stub HTTP backend (for testing / offline).
    pub fn with_stub(
        config: TelegramConfig,
        queue_db: Connection,
        startup_time_ms: u64,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<Self, AuraError> {
        Self::new(
            config,
            queue_db,
            Box::new(StubHttpBackend),
            startup_time_ms,
            cancel_flag,
        )
    }

    /// Run the main processing loop.
    ///
    /// 1. Starts the poll loop in a background task.
    /// 2. Processes incoming updates through the security → handler pipeline.
    /// 3. Runs until the cancel flag is set.
    #[instrument(skip(self), name = "telegram_engine_run")]
    pub async fn run(&mut self) -> Result<(), AuraError> {
        let (tx, mut rx) = mpsc::channel::<TelegramUpdate>(256);

        info!("Telegram engine starting main loop");

        // Destructure self into disjoint field borrows so we can run the
        // poll loop (which borrows poller + queue) concurrently with the
        // handler loop (which borrows the remaining fields).
        let Self {
            poller,
            security,
            audit,
            queue,
            policy_gate,
            dialogue_mgr,
            startup_time_ms,
            cancel_flag,
            primary_chat_id: _,
            aura_config,
            user_command_tx,
        } = self;

        let poll_future = poller.poll_loop(&tx, queue);

        let handler_future = async {
            while let Some(update) = rx.recv().await {
                if cancel_flag.load(std::sync::atomic::Ordering::Relaxed) {
                    info!("cancel flag set — stopping Telegram engine");
                    break;
                }

                // Expire stale dialogues and approval requests.
                dialogue_mgr.expire_stale();
                policy_gate.expire_stale();

                // ── Inline handle_update logic ──────────────────────────
                let chat_id = update.chat_id;
                let text = match &update.text {
                    Some(t) => t.clone(),
                    None => continue, // Ignore non-text messages.
                };

                // Check for active dialogue first.
                if dialogue_mgr.has_active(chat_id) {
                    use dialogue::DialogueOutcome;
                    match dialogue_mgr.process_input(chat_id, &text) {
                        DialogueOutcome::Continue(prompt) => {
                            let _ = queue.enqueue(
                                chat_id,
                                &MessageContent::Text {
                                    text: prompt,
                                    parse_mode: Some("HTML".into()),
                                },
                                0,
                                3600,
                                3,
                                None,
                            );
                        }
                        DialogueOutcome::Completed { kind: _, responses } => {
                            for resp in responses {
                                let _ = queue.enqueue(
                                    chat_id,
                                    &MessageContent::Text {
                                        text: resp,
                                        parse_mode: None,
                                    },
                                    0,
                                    3600,
                                    3,
                                    None,
                                );
                            }
                        }
                        DialogueOutcome::InvalidInput(hint) => {
                            let _ = queue.enqueue(
                                chat_id,
                                &MessageContent::Text {
                                    text: hint,
                                    parse_mode: None,
                                },
                                0,
                                3600,
                                3,
                                None,
                            );
                        }
                        DialogueOutcome::TimedOut => {
                            let _ = queue.enqueue(
                                chat_id,
                                &MessageContent::Text {
                                    text: "Dialogue timed out.".into(),
                                    parse_mode: None,
                                },
                                0,
                                3600,
                                3,
                                None,
                            );
                        }
                        DialogueOutcome::Cancelled => {
                            let _ = queue.enqueue(
                                chat_id,
                                &MessageContent::Text {
                                    text: "Dialogue cancelled.".into(),
                                    parse_mode: None,
                                },
                                0,
                                3600,
                                3,
                                None,
                            );
                        }
                    }
                    continue;
                }

                // Parse the command.
                let cmd = TelegramCommand::parse(&text);
                let audit_summary = cmd.audit_summary();
                let required_perm = cmd.required_permission();
                let is_unlock = matches!(cmd, TelegramCommand::Unlock { .. });

                // Security check (layers 1–4).
                match security.check(chat_id, required_perm, is_unlock) {
                    Ok(()) => {
                        let result = {
                            let mut ctx = HandlerContext {
                                chat_id,
                                security,
                                audit,
                                queue,
                                startup_time_ms: *startup_time_ms,
                                config: aura_config.as_ref(),
                                user_command_tx: user_command_tx.as_ref(),
                            };
                            handlers::dispatch(&cmd, &mut ctx)
                        };

                        match result {
                            Ok(response) => {
                                audit.record(chat_id, &audit_summary, AuditOutcome::Allowed);
                                // Inline enqueue_response logic.
                                match response {
                                    HandlerResponse::Text(text) => {
                                        let _ = queue.enqueue(
                                            chat_id,
                                            &MessageContent::Text { text, parse_mode: None },
                                            0,
                                            3600,
                                            3,
                                            None,
                                        );
                                    }
                                    HandlerResponse::Html(text) => {
                                        let _ = queue.enqueue(
                                            chat_id,
                                            &MessageContent::Text {
                                                text,
                                                parse_mode: Some("HTML".into()),
                                            },
                                            0,
                                            3600,
                                            3,
                                            None,
                                        );
                                    }
                                    HandlerResponse::Photo { data, caption } => {
                                        let _ = queue.enqueue(
                                            chat_id,
                                            &MessageContent::Photo { data, caption },
                                            0,
                                            3600,
                                            3,
                                            None,
                                        );
                                    }
                                    HandlerResponse::Voice { text } => {
                                        let _ = queue.enqueue(
                                            chat_id,
                                            &MessageContent::Text {
                                                text,
                                                parse_mode: None,
                                            },
                                            1,
                                            3600,
                                            3,
                                            Some("voice"),
                                        );
                                    }
                                    HandlerResponse::Empty => {}
                                }
                            }
                            Err(e) => {
                                audit.record(
                                    chat_id,
                                    &audit_summary,
                                    AuditOutcome::Failed(e.to_string()),
                                );
                                let _ = queue.enqueue(
                                    chat_id,
                                    &MessageContent::Text {
                                        text: format!("Command failed: {e}"),
                                        parse_mode: None,
                                    },
                                    0,
                                    3600,
                                    3,
                                    None,
                                );
                            }
                        }
                    }
                    Err(sec_err) => {
                        audit.record(
                            chat_id,
                            &audit_summary,
                            AuditOutcome::Denied(sec_err.to_string()),
                        );
                        let _ = queue.enqueue(
                            chat_id,
                            &MessageContent::Text {
                                text: format!("Access denied: {sec_err}"),
                                parse_mode: None,
                            },
                            0,
                            3600,
                            3,
                            None,
                        );
                    }
                }
            }
        };

        // Run both futures concurrently. When either finishes, cancel the other.
        tokio::select! {
            poll_result = poll_future => {
                if let Err(e) = poll_result {
                    tracing::error!(error = %e, "Telegram poll loop exited with error");
                }
            }
            _ = handler_future => {
                tracing::info!("Telegram handler loop exited");
            }
        }

        info!("Telegram engine stopped");
        Ok(())
    }

    /// Process a single incoming update through the full pipeline.
    // Phase 8 wire point: called by Telegram polling loop once bridge is
    // activated from the JNI boot path.
    #[instrument(skip(self, update), fields(chat_id = update.chat_id, update_id = update.update_id))]
    #[allow(dead_code)]
    async fn handle_update(&mut self, update: TelegramUpdate) {
        let chat_id = update.chat_id;
        let text = match &update.text {
            Some(t) => t.clone(),
            None => return, // Ignore non-text messages for now.
        };

        // Check if there's an active dialogue for this chat.
        if self.dialogue_mgr.has_active(chat_id) {
            self.handle_dialogue_input(chat_id, &text);
            return;
        }

        // Parse the command.
        let cmd = TelegramCommand::parse(&text);
        let audit_summary = cmd.audit_summary();
        let required_perm = cmd.required_permission();
        let is_unlock = matches!(cmd, TelegramCommand::Unlock { .. });

        // Security check (layers 1–4).
        match self.security.check(chat_id, required_perm, is_unlock) {
            Ok(()) => {
                // Dispatch to handler. Scope the HandlerContext so the mutable
                // borrows on self.security/self.audit are released before we
                // call self.audit.record() below.
                let result = {
                    let mut ctx = HandlerContext {
                        chat_id,
                        security: &mut self.security,
                        audit: &mut self.audit,
                        queue: &self.queue,
                        startup_time_ms: self.startup_time_ms,
                        config: self.aura_config.as_ref(),
                        user_command_tx: self.user_command_tx.as_ref(),
                    };
                    handlers::dispatch(&cmd, &mut ctx)
                };

                match result {
                    Ok(response) => {
                        // Layer 5: audit (success).
                        self.audit.record(chat_id, &audit_summary, AuditOutcome::Allowed);
                        self.enqueue_response(chat_id, response);
                    }
                    Err(e) => {
                        // Layer 5: audit (failure).
                        self.audit.record(
                            chat_id,
                            &audit_summary,
                            AuditOutcome::Failed(e.to_string()),
                        );
                        let _ = self.queue.enqueue(
                            chat_id,
                            &MessageContent::Text {
                                text: format!("Command failed: {e}"),
                                parse_mode: None,
                            },
                            0,
                            3600,
                            3,
                            None,
                        );
                    }
                }
            }
            Err(sec_err) => {
                // Layer 5: audit (denied).
                self.audit.record(
                    chat_id,
                    &audit_summary,
                    AuditOutcome::Denied(sec_err.to_string()),
                );
                let _ = self.queue.enqueue(
                    chat_id,
                    &MessageContent::Text {
                        text: format!("Access denied: {sec_err}"),
                        parse_mode: None,
                    },
                    0,
                    3600,
                    3,
                    None,
                );
            }
        }
    }

    /// Handle input directed to an active dialogue flow.
    // Phase 8 wire point: called by handle_update dialogue routing branch.
    #[allow(dead_code)]
    fn handle_dialogue_input(&mut self, chat_id: i64, text: &str) {
        use dialogue::DialogueOutcome;

        match self.dialogue_mgr.process_input(chat_id, text) {
            DialogueOutcome::Continue(prompt) => {
                let _ = self.queue.enqueue(
                    chat_id,
                    &MessageContent::Text {
                        text: prompt,
                        parse_mode: Some("HTML".into()),
                    },
                    0,
                    3600,
                    3,
                    None,
                );
            }
            DialogueOutcome::Completed { kind, responses } => {
                let msg = format!(
                    "Dialogue completed: {:?}\nResponses: {:?}",
                    kind, responses
                );
                // TODO: Execute the action associated with the completed dialogue.
                let _ = self.queue.enqueue(
                    chat_id,
                    &MessageContent::Text {
                        text: msg,
                        parse_mode: None,
                    },
                    0,
                    3600,
                    3,
                    None,
                );
            }
            DialogueOutcome::InvalidInput(hint) => {
                let _ = self.queue.enqueue(
                    chat_id,
                    &MessageContent::Text {
                        text: hint,
                        parse_mode: Some("HTML".into()),
                    },
                    0,
                    3600,
                    3,
                    None,
                );
            }
            DialogueOutcome::TimedOut => {
                let _ = self.queue.enqueue(
                    chat_id,
                    &MessageContent::Text {
                        text: "Dialogue timed out. Start over if needed.".into(),
                        parse_mode: None,
                    },
                    0,
                    3600,
                    3,
                    None,
                );
            }
            DialogueOutcome::Cancelled => {
                let _ = self.queue.enqueue(
                    chat_id,
                    &MessageContent::Text {
                        text: "Dialogue cancelled.".into(),
                        parse_mode: None,
                    },
                    0,
                    3600,
                    3,
                    None,
                );
            }
        }
    }

    /// Enqueue a handler response for sending.
    // Phase 8 wire point: called by handle_update command handler dispatch.
    #[allow(dead_code)]
    fn enqueue_response(&self, chat_id: i64, response: HandlerResponse) {
        match response {
            HandlerResponse::Text(text) => {
                let _ = self.queue.enqueue(
                    chat_id,
                    &MessageContent::Text {
                        text,
                        parse_mode: None,
                    },
                    0,
                    3600,
                    3,
                    None,
                );
            }
            HandlerResponse::Html(html) => {
                let _ = self.queue.enqueue(
                    chat_id,
                    &MessageContent::Text {
                        text: html,
                        parse_mode: Some("HTML".into()),
                    },
                    0,
                    3600,
                    3,
                    None,
                );
            }
            HandlerResponse::Photo { data, caption } => {
                let _ = self.queue.enqueue(
                    chat_id,
                    &MessageContent::Photo { data, caption },
                    0,
                    3600,
                    3,
                    None,
                );
            }
            HandlerResponse::Voice { text } => {
                let _ = self.queue.enqueue(
                    chat_id,
                    &MessageContent::Text {
                        text,
                        parse_mode: None,
                    },
                    1,
                    3600,
                    3,
                    Some("voice"),
                );
            }
            HandlerResponse::Empty => {}
        }
    }

    /// Generate a health dashboard snapshot.
    pub fn dashboard_snapshot(&self) -> DashboardSnapshot {
        DashboardSnapshot::collect(
            &self.security,
            &self.audit,
            &self.queue,
            self.startup_time_ms,
        )
    }

    /// Render the full HTML health dashboard.
    pub fn render_dashboard(&self) -> String {
        let snap = self.dashboard_snapshot();
        dashboard::render_dashboard(&snap)
    }

    // ── Accessors ───────────────────────────────────────────────────────

    pub fn security(&self) -> &SecurityGate {
        &self.security
    }

    pub fn security_mut(&mut self) -> &mut SecurityGate {
        &mut self.security
    }

    pub fn audit(&self) -> &AuditLog {
        &self.audit
    }

    pub fn queue(&self) -> &MessageQueue {
        &self.queue
    }

    pub fn policy_gate(&self) -> &PolicyGate {
        &self.policy_gate
    }

    pub fn policy_gate_mut(&mut self) -> &mut PolicyGate {
        &mut self.policy_gate
    }

    pub fn dialogue_manager(&self) -> &DialogueManager {
        &self.dialogue_mgr
    }

    /// Inject the live AURA config snapshot so handlers can read real values.
    pub fn set_aura_config(&mut self, config: AuraConfig) {
        self.aura_config = Some(config);
    }

    /// Inject the user-command channel so fallback handlers can forward
    /// commands to the cognitive pipeline.
    pub fn set_user_command_tx(&mut self, tx: UserCommandTx) {
        self.user_command_tx = Some(tx);
    }
}

impl std::fmt::Debug for TelegramEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramEngine")
            .field("security", &self.security)
            .field("audit", &self.audit)
            .field("policy_gate", &self.policy_gate)
            .field("dialogue_mgr", &self.dialogue_mgr)
            .field("primary_chat_id", &self.primary_chat_id)
            .finish()
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> TelegramConfig {
        TelegramConfig {
            bot_token: "test:TOKEN".into(),
            allowed_chat_ids: vec![42, 99],
            trust_level: 0.5,
            audit_capacity: 100,
            rate_per_minute: 30,
            rate_per_hour: 300,
            approval_ttl_secs: 300,
            dialogue_timeout_secs: 120,
        }
    }

    #[test]
    fn test_engine_creation() {
        let config = test_config();
        let db = Connection::open_in_memory().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let engine = TelegramEngine::with_stub(config, db, 1_700_000_000_000, cancel).unwrap();

        assert!(!engine.security().is_locked());
        assert!(engine.security().is_allowed(42));
        assert!(engine.security().is_allowed(99));
        assert!(!engine.security().is_allowed(1));
    }

    #[test]
    fn test_engine_dashboard() {
        let config = test_config();
        let db = Connection::open_in_memory().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let engine = TelegramEngine::with_stub(config, db, 1_700_000_000_000, cancel).unwrap();

        let html = engine.render_dashboard();
        assert!(html.contains("AURA Health Dashboard"));
        assert!(html.contains("Security"));
        assert!(html.contains("2 chats")); // 42 + 99
    }

    #[test]
    fn test_enqueue_response_text() {
        let config = test_config();
        let db = Connection::open_in_memory().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let engine = TelegramEngine::with_stub(config, db, 1_700_000_000_000, cancel).unwrap();

        engine.enqueue_response(42, HandlerResponse::text("hello"));
        let pending = engine.queue().pending_count().unwrap();
        assert_eq!(pending, 1);
    }

    #[test]
    fn test_enqueue_response_empty() {
        let config = test_config();
        let db = Connection::open_in_memory().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let engine = TelegramEngine::with_stub(config, db, 1_700_000_000_000, cancel).unwrap();

        engine.enqueue_response(42, HandlerResponse::Empty);
        let pending = engine.queue().pending_count().unwrap();
        assert_eq!(pending, 0);
    }

    #[tokio::test]
    async fn test_handle_update_unauthorized() {
        let config = test_config();
        let db = Connection::open_in_memory().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut engine = TelegramEngine::with_stub(config, db, 1_700_000_000_000, cancel).unwrap();

        let update = TelegramUpdate {
            update_id: 1,
            chat_id: 999, // Not in allowed list, but handle_update doesn't re-check whitelist
            from_user_id: Some(999),
            text: Some("/status".into()),
            message_id: Some(1),
            callback_data: None,
        };

        engine.handle_update(update).await;

        // Should be denied — check audit log.
        let entries = engine.audit().last_n(1);
        assert_eq!(entries.len(), 1);
        assert!(matches!(
            entries[0].outcome,
            AuditOutcome::Denied(_)
        ));
    }

    #[tokio::test]
    async fn test_handle_update_authorized() {
        let config = test_config();
        let db = Connection::open_in_memory().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut engine = TelegramEngine::with_stub(config, db, 1_700_000_000_000, cancel).unwrap();

        let update = TelegramUpdate {
            update_id: 1,
            chat_id: 42,
            from_user_id: Some(42),
            text: Some("/status".into()),
            message_id: Some(1),
            callback_data: None,
        };

        engine.handle_update(update).await;

        // Should be allowed — check audit log.
        let entries = engine.audit().last_n(1);
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].outcome, AuditOutcome::Allowed));

        // Response should be queued.
        let pending = engine.queue().pending_count().unwrap();
        assert!(pending >= 1);
    }
}
