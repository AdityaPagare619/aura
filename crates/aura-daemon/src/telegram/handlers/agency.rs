//! Agency command handlers: /do, /open, /send, /call, /schedule, /screenshot, /navigate, /automate.
//!
//! These commands are **daemon-routed** — when the execution pipeline is
//! available, `dispatch()` in `mod.rs` forwards them via [`UserCommandTx`]
//! and these handlers are never called.
//!
//! The functions below are **fallback handlers**: they execute only when the
//! daemon channel is unavailable. Agency commands are the MOST DANGEROUS
//! category — they control the physical device (phone calls, app launches,
//! message sending). The fallback handlers must:
//!
//! 1. **NEVER execute any action.** Only the daemon with PolicyGate can.
//! 2. **Be honest** — never claim an action was queued or is pending.
//! 3. **Log the attempt** — so the action can be reviewed when the daemon comes online.

use aura_types::errors::AuraError;
use tracing::instrument;

use super::{HandlerContext, HandlerResponse};

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `/do <instruction>` — Fallback when the execution engine is unavailable.
///
/// SAFETY: This handler NEVER executes device actions. Only the daemon
/// with PolicyGate verification can execute instructions.
#[instrument(skip(ctx))]
pub fn handle_do(
    ctx: &HandlerContext<'_>,
    instruction: &str,
) -> Result<HandlerResponse, AuraError> {
    if instruction.is_empty() {
        return Ok(HandlerResponse::text("Usage: /do <instruction>"));
    }

    tracing::warn!(
        chat_id = ctx.chat_id,
        instruction = truncate(instruction, 100),
        "Agency fallback: /do reached local handler — execution engine unavailable"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Execution Engine Unavailable</b>\n\n\
         Instruction: <i>{}</i>\n\n\
         This action requires AURA's verified execution engine with \
         PolicyGate approval. The engine is currently offline or starting up.\n\n\
         No action was taken. Please retry when the daemon is fully running.",
        escape_html(truncate(instruction, 500)),
    )))
}

/// `/open <app>` — Fallback when the execution engine is unavailable.
///
/// SAFETY: App launches require the daemon's A11y service integration.
#[instrument(skip(ctx))]
pub fn handle_open(ctx: &HandlerContext<'_>, app: &str) -> Result<HandlerResponse, AuraError> {
    if app.is_empty() {
        return Ok(HandlerResponse::text("Usage: /open <app>"));
    }

    tracing::warn!(
        chat_id = ctx.chat_id,
        app,
        "Agency fallback: /open reached local handler — execution engine unavailable"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Execution Engine Unavailable</b>\n\n\
         App: <code>{}</code>\n\n\
         App launches require the daemon's execution engine and A11y service. \
         The engine is currently offline or starting up.\n\n\
         No action was taken. Please retry when the daemon is fully running.",
        escape_html(app),
    )))
}

/// `/send <app> <contact> <message>` — Fallback when the execution engine is unavailable.
///
/// SAFETY: Sending messages is a HIGH-RISK action. The fallback handler
/// must NEVER attempt to send anything. Only the daemon with Last Mile
/// Approval can execute this.
#[instrument(skip(ctx, message))]
pub fn handle_send(
    ctx: &HandlerContext<'_>,
    app: &str,
    contact: &str,
    message: &str,
) -> Result<HandlerResponse, AuraError> {
    tracing::warn!(
        chat_id = ctx.chat_id,
        app,
        contact,
        "Agency fallback: /send reached local handler — execution engine unavailable (HIGH-RISK blocked)"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Execution Engine Unavailable</b>\n\n\
         App: <code>{}</code>\n\
         To: <code>{}</code>\n\
         Message: <i>{}</i>\n\n\
         Sending messages is a high-risk action that requires AURA's \
         verified execution engine with Last Mile Approval.\n\
         The engine is currently offline or starting up.\n\n\
         <b>No message was sent.</b> Please retry when the daemon is fully running.",
        escape_html(app),
        escape_html(contact),
        escape_html(truncate(message, 200)),
    )))
}

/// `/call <contact>` — Fallback when the execution engine is unavailable.
///
/// SAFETY: Phone calls are a HIGH-RISK action. Never execute from fallback.
#[instrument(skip(ctx))]
pub fn handle_call(ctx: &HandlerContext<'_>, contact: &str) -> Result<HandlerResponse, AuraError> {
    if contact.is_empty() {
        return Ok(HandlerResponse::text("Usage: /call <contact>"));
    }

    tracing::warn!(
        chat_id = ctx.chat_id,
        contact,
        "Agency fallback: /call reached local handler — execution engine unavailable (HIGH-RISK blocked)"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Execution Engine Unavailable</b>\n\n\
         Contact: <code>{}</code>\n\n\
         Phone calls are a high-risk action that requires AURA's \
         verified execution engine with Last Mile Approval.\n\
         The engine is currently offline or starting up.\n\n\
         <b>No call was placed.</b> Please retry when the daemon is fully running.",
        escape_html(contact),
    )))
}

/// `/schedule <event> <time>` — Fallback when the execution engine is unavailable.
///
/// SAFETY: Scheduling requires the daemon's cron system and PolicyGate.
#[instrument(skip(ctx))]
pub fn handle_schedule(
    ctx: &HandlerContext<'_>,
    event: &str,
    time: &str,
) -> Result<HandlerResponse, AuraError> {
    tracing::warn!(
        chat_id = ctx.chat_id,
        event = truncate(event, 100),
        time,
        "Agency fallback: /schedule reached local handler — execution engine unavailable"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Execution Engine Unavailable</b>\n\n\
         Event: <i>{}</i>\n\
         Time: <code>{}</code>\n\n\
         Scheduling requires the daemon's cron system and PolicyGate. \
         The engine is currently offline or starting up.\n\n\
         <b>No event was scheduled.</b> Please retry when the daemon is fully running.",
        escape_html(event),
        escape_html(time),
    )))
}

/// `/screenshot` — Fallback when the execution engine is unavailable.
///
/// Screenshots require the A11y service running in the daemon process.
#[instrument(skip(ctx))]
pub fn handle_screenshot(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    tracing::warn!(
        chat_id = ctx.chat_id,
        "Agency fallback: /screenshot reached local handler — execution engine unavailable"
    );

    Ok(HandlerResponse::Html(
        "<b>Execution Engine Unavailable</b>\n\n\
         Screen capture requires the daemon's A11y service integration. \
         The engine is currently offline or starting up.\n\n\
         No screenshot was captured. Please retry when the daemon is fully running."
            .to_string(),
    ))
}

/// `/navigate <destination>` — Fallback when the execution engine is unavailable.
///
/// SAFETY: Navigation launches an external app — requires PolicyGate.
#[instrument(skip(ctx))]
pub fn handle_navigate(
    ctx: &HandlerContext<'_>,
    destination: &str,
) -> Result<HandlerResponse, AuraError> {
    if destination.is_empty() {
        return Ok(HandlerResponse::text("Usage: /navigate <destination>"));
    }

    tracing::warn!(
        chat_id = ctx.chat_id,
        destination = truncate(destination, 100),
        "Agency fallback: /navigate reached local handler — execution engine unavailable"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Execution Engine Unavailable</b>\n\n\
         Destination: <i>{}</i>\n\n\
         Navigation requires the daemon's execution engine to launch maps. \
         The engine is currently offline or starting up.\n\n\
         No action was taken. Please retry when the daemon is fully running.",
        escape_html(truncate(destination, 300)),
    )))
}

/// `/automate <routine>` — Fallback when the execution engine is unavailable.
///
/// SAFETY: Automation routines execute multiple device actions — they
/// absolutely require the daemon's PolicyGate for safety.
#[instrument(skip(ctx))]
pub fn handle_automate(
    ctx: &HandlerContext<'_>,
    routine: &str,
) -> Result<HandlerResponse, AuraError> {
    if routine.is_empty() {
        return Ok(HandlerResponse::text("Usage: /automate <routine>"));
    }

    tracing::warn!(
        chat_id = ctx.chat_id,
        routine = truncate(routine, 100),
        "Agency fallback: /automate reached local handler — execution engine unavailable"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Execution Engine Unavailable</b>\n\n\
         Routine: <code>{}</code>\n\n\
         Automation routines execute multiple device actions and require \
         the daemon's verified execution engine with PolicyGate approval.\n\
         The engine is currently offline or starting up.\n\n\
         No action was taken. Please retry when the daemon is fully running.",
        escape_html(truncate(routine, 300)),
    )))
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &s[..end]
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::telegram::{
        audit::AuditLog, handlers::HandlerContext, queue::MessageQueue, security::SecurityGate,
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
    fn test_do_empty() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_do(&ctx, "").unwrap() {
            HandlerResponse::Text(t) => assert!(t.contains("Usage")),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn test_do_fallback_never_executes() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_do(&ctx, "turn on wifi").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Execution Engine Unavailable"));
                assert!(html.contains("turn on wifi"));
                assert!(html.contains("No action was taken"));
                // Must NOT contain misleading action language.
                assert!(!html.contains("Routing through approval"));
                assert!(!html.contains("queued"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_send_fallback_never_sends() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_send(&ctx, "whatsapp", "John", "Hello there").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Execution Engine Unavailable"));
                assert!(html.contains("whatsapp"));
                assert!(html.contains("John"));
                assert!(html.contains("No message was sent"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_call_fallback_never_calls() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_call(&ctx, "+1234567890").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Execution Engine Unavailable"));
                assert!(html.contains("+1234567890"));
                assert!(html.contains("No call was placed"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_screenshot_fallback_honest() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_screenshot(&ctx).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Execution Engine Unavailable"));
                assert!(html.contains("No screenshot was captured"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_schedule_fallback_honest() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_schedule(&ctx, "Team meeting", "3pm tomorrow").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Execution Engine Unavailable"));
                assert!(html.contains("Team meeting"));
                assert!(html.contains("No event was scheduled"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_automate_fallback_honest() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_automate(&ctx, "morning_routine").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Execution Engine Unavailable"));
                assert!(html.contains("morning_routine"));
                assert!(html.contains("No action was taken"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }
}
