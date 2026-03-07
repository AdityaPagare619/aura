//! Debug command handlers: /trace, /dump, /perf, /etg, /goals.

use aura_types::errors::AuraError;
use tracing::instrument;

use super::{HandlerContext, HandlerResponse};

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `/trace <module>` — Enable verbose tracing for a module.
#[instrument(skip(_ctx))]
pub fn handle_trace(_ctx: &HandlerContext<'_>, module: &str) -> Result<HandlerResponse, AuraError> {
    // TODO: Wire to tracing subscriber dynamic filter.
    let html = format!(
        "<b>Tracing Enabled</b>\n\n\
         Module: <code>{module}</code>\n\
         Level: TRACE\n\n\
         <i>Dynamic filter update not yet wired to tracing subscriber.\n\
         Use /logs {module} to view output.</i>"
    );
    Ok(HandlerResponse::Html(html))
}

/// `/dump <component>` — Dump internal state of a component.
#[instrument(skip(ctx))]
pub fn handle_dump(
    ctx: &HandlerContext<'_>,
    component: &str,
) -> Result<HandlerResponse, AuraError> {
    let body = match component {
        "security" => {
            let locked = ctx.security.is_locked();
            let pin_set = ctx.security.is_pin_set();
            let allowed = ctx.security.allowed_chat_ids().len();
            let perms = ctx.security.all_permissions().len();
            format!(
                "locked: {locked}\n\
                 pin_set: {pin_set}\n\
                 allowed_chats: {allowed}\n\
                 permission_entries: {perms}"
            )
        }
        "queue" => {
            let pending = ctx.queue.pending_count().unwrap_or(0);
            format!("pending_messages: {pending}")
        }
        "audit" => {
            let len = ctx.audit.len();
            format!("audit_entries: {len}")
        }
        "config" => {
            // TODO: Dump AuraConfig fields.
            "config dump not yet wired to AuraConfig".to_string()
        }
        "channels" => {
            // TODO: Dump DaemonChannels capacity / backpressure info.
            "channels dump not yet wired to DaemonChannels".to_string()
        }
        unknown => {
            return Ok(HandlerResponse::text(format!(
                "Unknown component: {unknown}\n\
                 Available: security, queue, audit, config, channels"
            )));
        }
    };

    let html = format!("<b>State Dump: {component}</b>\n\n<pre>{body}</pre>");
    Ok(HandlerResponse::Html(html))
}

/// `/perf` — Show performance metrics.
#[instrument(skip(ctx))]
pub fn handle_perf(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let pending = ctx.queue.pending_count().unwrap_or(0);
    let audit_entries = ctx.audit.len();

    // TODO: Wire to actual performance counters (CPU, memory, thread count).
    let html = format!(
        "<b>Performance Metrics</b>\n\n\
         <pre>\
         CPU:             N/A (not yet wired)\n\
         Memory:          N/A\n\
         Threads:         N/A\n\
         Queue backlog:   {pending}\n\
         Audit entries:   {audit_entries}\n\
         GC cycles:       N/A\n\
         </pre>\n\
         <i>Full perf counters require integration with telemetry engine.</i>"
    );
    Ok(HandlerResponse::Html(html))
}

/// `/etg [app]` — Show element tree graph (accessibility tree).
#[instrument(skip(_ctx))]
pub fn handle_etg(
    _ctx: &HandlerContext<'_>,
    app: Option<&str>,
) -> Result<HandlerResponse, AuraError> {
    // TODO: Wire to A11y service for real element tree.
    let target = app.unwrap_or("current screen");
    let html = format!(
        "<b>Element Tree Graph</b>\n\n\
         Target: <code>{target}</code>\n\n\
         <i>ETG capture not yet wired to accessibility service.\n\
         On Android, this queries the AccessibilityNodeInfo tree.</i>"
    );
    Ok(HandlerResponse::Html(html))
}

/// `/goals` — Show active goals from the planning system.
#[instrument(skip(_ctx))]
pub fn handle_goals(_ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    // TODO: Wire to goal / planning system.
    Ok(HandlerResponse::Html(
        "<b>Active Goals</b>\n\n\
         <i>No active goals.</i>\n\n\
         Goals are created via /plan or /do commands.\n\
         Use /plan &lt;goal&gt; to create a new goal."
            .to_string(),
    ))
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telegram::audit::AuditLog;
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
    fn test_dump_security() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_dump(&ctx, "security").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("security"));
                assert!(html.contains("locked: false"));
                assert!(html.contains("pin_set: false"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_dump_unknown() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_dump(&ctx, "foobar").unwrap() {
            HandlerResponse::Text(text) => {
                assert!(text.contains("Unknown component"));
                assert!(text.contains("foobar"));
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn test_perf_placeholder() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_perf(&ctx).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Performance Metrics"));
                assert!(html.contains("CPU"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }
}
