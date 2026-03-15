//! Security command handlers: /pin, /lock, /unlock, /audit, /permissions.
//!
//! These handlers interact directly with the [`SecurityGate`] and [`AuditLog`]
//! via [`HandlerContext`], modifying security state in response to user commands.

use aura_types::errors::AuraError;
use tracing::instrument;

use super::{HandlerContext, HandlerResponse};
use crate::telegram::commands::PinAction;

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `/pin <action>` — Manage PIN (set, clear, status).
#[instrument(skip(ctx))]
pub fn handle_pin(
    ctx: &mut HandlerContext<'_>,
    action: &PinAction,
) -> Result<HandlerResponse, AuraError> {
    match action {
        PinAction::Set(value) => {
            ctx.security.set_pin(value);
            Ok(HandlerResponse::Html(
                "<b>PIN Updated</b>\n\nPIN has been set successfully.\n\
                 Use /lock to lock the bot."
                    .to_string(),
            ))
        },
        PinAction::Clear => {
            ctx.security.clear_pin();
            Ok(HandlerResponse::Html(
                "<b>PIN Cleared</b>\n\nPIN removed. Bot is now permanently unlocked \
                 until a new PIN is set."
                    .to_string(),
            ))
        },
        PinAction::Status => {
            let status = if ctx.security.is_pin_set() {
                "PIN is <b>set</b>"
            } else {
                "PIN is <b>not set</b>"
            };
            let lock = if ctx.security.is_locked() {
                "Bot is <b>LOCKED</b>"
            } else {
                "Bot is <b>unlocked</b>"
            };
            Ok(HandlerResponse::Html(format!(
                "<b>PIN Status</b>\n\n{status}\n{lock}"
            )))
        },
    }
}

/// `/lock` — Lock the bot (requires PIN to be set).
#[instrument(skip(ctx))]
pub fn handle_lock(ctx: &mut HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    match ctx.security.lock() {
        Ok(()) => Ok(HandlerResponse::Html(
            "<b>Bot Locked</b>\n\nAll commands except /unlock are now disabled.\n\
             Use /unlock &lt;pin&gt; to restore access."
                .to_string(),
        )),
        Err(e) => Ok(HandlerResponse::text(format!(
            "Cannot lock: {e}\nSet a PIN first with /pin set <value>"
        ))),
    }
}

/// `/unlock <pin>` — Unlock the bot with a PIN.
#[instrument(skip(ctx, pin))]
pub fn handle_unlock(
    ctx: &mut HandlerContext<'_>,
    pin: &str,
) -> Result<HandlerResponse, AuraError> {
    match ctx.security.unlock(pin) {
        Ok(()) => Ok(HandlerResponse::Html(
            "<b>Bot Unlocked</b>\n\nAll commands are now available.".to_string(),
        )),
        Err(e) => {
            let msg = match e {
                crate::telegram::security::SecurityError::InvalidPin => {
                    "Incorrect PIN. Try again.".to_string()
                },
                crate::telegram::security::SecurityError::PinNotConfigured => {
                    "No PIN configured. Use /pin set <value> first.".to_string()
                },
                other => format!("Unlock failed: {other}"),
            };
            Ok(HandlerResponse::text(msg))
        },
    }
}

/// `/audit [lines]` — Show recent audit log entries.
#[instrument(skip(ctx))]
pub fn handle_audit(ctx: &HandlerContext<'_>, lines: usize) -> Result<HandlerResponse, AuraError> {
    let capped = lines.min(50);
    let total = ctx.audit.len();
    let formatted = ctx.audit.format_last_n(capped);

    let html = format!(
        "<b>Audit Log</b> (showing last {capped}, {total} total)\n\n\
         <pre>{formatted}</pre>"
    );
    Ok(HandlerResponse::Html(html))
}

/// `/permissions` — Show the permission table for all known chat IDs.
#[instrument(skip(ctx))]
pub fn handle_permissions(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let perms = ctx.security.all_permissions();
    let allowed = ctx.security.allowed_chat_ids();

    let mut html = String::from("<b>Permission Table</b>\n\n");
    html.push_str("<pre>\n");
    html.push_str("Chat ID          | Level\n");
    html.push_str("-----------------+----------\n");

    for &cid in allowed {
        let level = perms.get(&cid).map_or("read-only", |l| l.as_str());
        html.push_str(&format!("{cid:<17}| {level}\n"));
    }
    html.push_str("</pre>");

    Ok(HandlerResponse::Html(html))
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::telegram::{
        audit::{AuditLog, AuditOutcome},
        queue::MessageQueue,
        security::SecurityGate,
    };

    fn make_ctx<'a>(
        sec: &'a mut SecurityGate,
        aud: &'a mut AuditLog,
        q: &'a MessageQueue,
    ) -> HandlerContext<'a> {
        HandlerContext {
            chat_id: 42,
            security: sec,
            audit: aud,
            queue: q,
            startup_time_ms: 1_700_000_000_000,
            config: None,
            user_command_tx: None,
        }
    }

    #[test]
    fn test_pin_set_and_status() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let mut ctx = make_ctx(&mut sec, &mut aud, &q);

        // Set PIN.
        let resp = handle_pin(&mut ctx, &PinAction::Set("1234".into())).unwrap();
        match resp {
            HandlerResponse::Html(html) => assert!(html.contains("PIN has been set")),
            other => panic!("expected Html, got {other:?}"),
        }

        // Check status.
        let resp = handle_pin(&mut ctx, &PinAction::Status).unwrap();
        match resp {
            HandlerResponse::Html(html) => {
                assert!(html.contains("set"));
                assert!(html.contains("unlocked"));
            },
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_lock_unlock_flow() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let mut ctx = make_ctx(&mut sec, &mut aud, &q);

        // Set PIN first.
        handle_pin(&mut ctx, &PinAction::Set("abcd".into())).unwrap();

        // Lock.
        let resp = handle_lock(&mut ctx).unwrap();
        match resp {
            HandlerResponse::Html(html) => assert!(html.contains("Bot Locked")),
            other => panic!("expected Html, got {other:?}"),
        }

        // Unlock.
        let resp = handle_unlock(&mut ctx, "abcd").unwrap();
        match resp {
            HandlerResponse::Html(html) => assert!(html.contains("Bot Unlocked")),
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_lock_without_pin() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let mut ctx = make_ctx(&mut sec, &mut aud, &q);

        let resp = handle_lock(&mut ctx).unwrap();
        match resp {
            HandlerResponse::Text(text) => {
                assert!(text.contains("Cannot lock"));
                assert!(text.contains("PIN"));
            },
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn test_audit_display() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        aud.record(42, "/status", AuditOutcome::Allowed);
        aud.record(42, "/restart", AuditOutcome::Denied("locked".into()));

        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_audit(&ctx, 10).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Audit Log"));
                assert!(html.contains("/status"));
                assert!(html.contains("2 total"));
            },
            other => panic!("expected Html, got {other:?}"),
        }
    }
}
