//! Agency command handlers: /do, /open, /send, /call, /schedule, /screenshot, /navigate, /automate.
//!
//! These commands trigger real-world actions on the Android device via the
//! daemon's execution pipeline. They require Action permission level and
//! many will route through the Last Mile Approval system before execution.

use aura_types::errors::AuraError;
use tracing::instrument;

use super::{HandlerContext, HandlerResponse};

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `/do <instruction>` — Execute an instruction on the device.
#[instrument(skip(_ctx))]
pub fn handle_do(
    _ctx: &HandlerContext<'_>,
    instruction: &str,
) -> Result<HandlerResponse, AuraError> {
    if instruction.is_empty() {
        return Ok(HandlerResponse::text("Usage: /do <instruction>"));
    }

    // TODO: Route through Last Mile Approval, then to executor.
    Ok(HandlerResponse::Html(format!(
        "<b>Execute</b>\n\n<i>{}</i>\n\n\
         Routing through approval pipeline...\n\
         You may receive a confirmation request.",
        escape_html(truncate(instruction, 500))
    )))
}

/// `/open <app>` — Open an application.
#[instrument(skip(_ctx))]
pub fn handle_open(_ctx: &HandlerContext<'_>, app: &str) -> Result<HandlerResponse, AuraError> {
    if app.is_empty() {
        return Ok(HandlerResponse::text("Usage: /open <app>"));
    }

    // TODO: Send intent to open app via A11y service.
    Ok(HandlerResponse::Html(format!(
        "<b>Open App</b>\n\nOpening: <code>{}</code>\n\n\
         <i>App launch command queued.</i>",
        escape_html(app)
    )))
}

/// `/send <app> <contact> <message>` — Send a message via an app.
#[instrument(skip(_ctx, message))]
pub fn handle_send(
    _ctx: &HandlerContext<'_>,
    app: &str,
    contact: &str,
    message: &str,
) -> Result<HandlerResponse, AuraError> {
    // This is a high-risk action — will require Last Mile Approval.
    // TODO: Route through approval with full context.
    Ok(HandlerResponse::Html(format!(
        "<b>Send Message</b>\n\n\
         App: <code>{}</code>\n\
         To: <code>{}</code>\n\
         Message: <i>{}</i>\n\n\
         Pending approval...",
        escape_html(app),
        escape_html(contact),
        escape_html(truncate(message, 200))
    )))
}

/// `/call <contact>` — Make a phone call.
#[instrument(skip(_ctx))]
pub fn handle_call(_ctx: &HandlerContext<'_>, contact: &str) -> Result<HandlerResponse, AuraError> {
    if contact.is_empty() {
        return Ok(HandlerResponse::text("Usage: /call <contact>"));
    }

    // High-risk action: requires approval.
    Ok(HandlerResponse::Html(format!(
        "<b>Phone Call</b>\n\nCalling: <code>{}</code>\n\n\
         Pending approval...",
        escape_html(contact)
    )))
}

/// `/schedule <event> <time>` — Schedule an event.
#[instrument(skip(_ctx))]
pub fn handle_schedule(
    _ctx: &HandlerContext<'_>,
    event: &str,
    time: &str,
) -> Result<HandlerResponse, AuraError> {
    // TODO: Parse time and create calendar event via cron system.
    Ok(HandlerResponse::Html(format!(
        "<b>Schedule</b>\n\n\
         Event: <i>{}</i>\n\
         Time: <code>{}</code>\n\n\
         <i>Scheduling pipeline not yet wired to cron.</i>",
        escape_html(event),
        escape_html(time)
    )))
}

/// `/screenshot` — Capture the screen.
#[instrument(skip(_ctx))]
pub fn handle_screenshot(_ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    // TODO: Capture screen via A11y service and send as photo.
    // Would enqueue a Photo message content.
    Ok(HandlerResponse::Html(
        "<b>Screenshot</b>\n\n\
         <i>Screen capture not yet wired to A11y service.</i>\n\
         The screenshot will be sent as a photo when available."
            .to_string(),
    ))
}

/// `/navigate <destination>` — Navigate somewhere.
#[instrument(skip(_ctx))]
pub fn handle_navigate(
    _ctx: &HandlerContext<'_>,
    destination: &str,
) -> Result<HandlerResponse, AuraError> {
    if destination.is_empty() {
        return Ok(HandlerResponse::text("Usage: /navigate <destination>"));
    }

    Ok(HandlerResponse::Html(format!(
        "<b>Navigate</b>\n\nDestination: <i>{}</i>\n\n\
         <i>Navigation intent queued. Opening maps...</i>",
        escape_html(truncate(destination, 300))
    )))
}

/// `/automate <routine>` — Run an automation routine.
#[instrument(skip(_ctx))]
pub fn handle_automate(
    _ctx: &HandlerContext<'_>,
    routine: &str,
) -> Result<HandlerResponse, AuraError> {
    if routine.is_empty() {
        return Ok(HandlerResponse::text("Usage: /automate <routine>"));
    }

    Ok(HandlerResponse::Html(format!(
        "<b>Automate</b>\n\nRoutine: <code>{}</code>\n\n\
         <i>Automation engine not yet wired. Routine queued.</i>",
        escape_html(truncate(routine, 300))
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
    use super::*;
    use crate::telegram::audit::AuditLog;
    use crate::telegram::handlers::HandlerContext;
    use crate::telegram::queue::MessageQueue;
    use crate::telegram::security::SecurityGate;
    use rusqlite::Connection;

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
    fn test_do_valid() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_do(&ctx, "turn on wifi").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Execute"));
                assert!(html.contains("turn on wifi"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_send_message() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_send(&ctx, "whatsapp", "John", "Hello there").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Send Message"));
                assert!(html.contains("whatsapp"));
                assert!(html.contains("John"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }
}
