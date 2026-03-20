//! Memory command handlers: /remember, /recall, /forget, /memories, /consolidate, /memorystats.
//!
//! **Daemon-routed commands** (`/remember`, `/recall`, `/forget`):
//! These are forwarded to the daemon's memory system via [`UserCommandTx`]
//! when the channel is available. The functions below are **fallback handlers**
//! that only execute when the pipeline is unavailable. They must be HONEST —
//! never claim a memory was stored when it wasn't.
//!
//! **Always-local commands** (`/memories`, `/consolidate`, `/memorystats`):
//! These are NOT daemon-routed and always execute locally. They report
//! whatever state is accessible from [`HandlerContext`].

use aura_types::errors::AuraError;
use tracing::instrument;

use super::{HandlerContext, HandlerResponse};

// ─── Daemon-routed fallback handlers ────────────────────────────────────────

/// `/remember <text>` — Fallback when the memory pipeline is unavailable.
///
/// This handler only runs if the daemon channel is closed or absent.
/// It must NOT claim the memory was stored — that would be a lie.
#[instrument(skip(ctx))]
pub fn handle_remember(ctx: &HandlerContext<'_>, text: &str) -> Result<HandlerResponse, AuraError> {
    if text.is_empty() {
        return Ok(HandlerResponse::text("Usage: /remember <text>"));
    }

    tracing::warn!(
        chat_id = ctx.chat_id,
        text_len = text.len(),
        "Memory fallback: /remember reached local handler — daemon pipeline unavailable"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Memory Service Unavailable</b>\n\n\
         Your note: <i>{}</i>\n\n\
         The memory engine is currently offline or starting up. \
         Your note was <b>not</b> saved.\n\n\
         Please retry when the daemon is fully running:\n\
         <code>/remember {}</code>",
        escape_html(truncate(text, 500)),
        escape_html(truncate(text, 100)),
    )))
}

/// `/recall <query>` — Fallback when the memory pipeline is unavailable.
///
/// Memory search requires the daemon's vector store and cognitive engine.
#[instrument(skip(ctx))]
pub fn handle_recall(ctx: &HandlerContext<'_>, query: &str) -> Result<HandlerResponse, AuraError> {
    if query.is_empty() {
        return Ok(HandlerResponse::text("Usage: /recall <query>"));
    }

    tracing::warn!(
        chat_id = ctx.chat_id,
        query = truncate(query, 100),
        "Memory fallback: /recall reached local handler — daemon pipeline unavailable"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Memory Service Unavailable</b>\n\n\
         Search query: <i>{}</i>\n\n\
         Memory search requires the cognitive engine, which is currently \
         offline or starting up. No search was performed.\n\n\
         Please retry shortly.",
        escape_html(truncate(query, 300)),
    )))
}

/// `/forget <query>` — Fallback when the memory pipeline is unavailable.
///
/// Forget is a destructive operation that requires the daemon's safety
/// verification (PolicyGate). The fallback handler must NEVER delete anything.
#[instrument(skip(ctx))]
pub fn handle_forget(ctx: &HandlerContext<'_>, query: &str) -> Result<HandlerResponse, AuraError> {
    if query.is_empty() {
        return Ok(HandlerResponse::text("Usage: /forget <query>"));
    }

    tracing::warn!(
        chat_id = ctx.chat_id,
        query = truncate(query, 100),
        "Memory fallback: /forget reached local handler — daemon pipeline unavailable"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Memory Service Unavailable</b>\n\n\
         Forget query: <i>{}</i>\n\n\
         Forget requests require safety verification through the daemon's \
         PolicyGate. The engine is currently offline or starting up.\n\n\
         No memories were deleted. Please retry when the daemon is fully running.",
        escape_html(truncate(query, 300)),
    )))
}

// ─── Always-local handlers ──────────────────────────────────────────────────

/// `/memories [filter]` — List memories (always local).
///
/// This command is NOT daemon-routed, so it always executes locally.
/// Without direct access to the memory store from HandlerContext,
/// we report what we can.
#[instrument(skip(ctx))]
pub fn handle_memories(
    ctx: &HandlerContext<'_>,
    filter: Option<&str>,
) -> Result<HandlerResponse, AuraError> {
    let filter_str = filter.unwrap_or("all");
    let pending = ctx.queue.pending_count().unwrap_or(0);

    Ok(HandlerResponse::Html(format!(
        "<b>Memories</b> (filter: {filter_str})\n\n\
         The memory store is not directly accessible from the Telegram \
         handler context. Memory listing requires a daemon query.\n\n\
         <b>Queue status:</b> {pending} pending messages\n\n\
         Use <code>/recall &lt;query&gt;</code> to search memories via the \
         cognitive pipeline.\n\
         Use <code>/memorystats</code> for aggregate statistics."
    )))
}

/// `/consolidate` — Trigger memory consolidation (always local).
///
/// Consolidation merges episodic memories into semantic knowledge.
/// This requires the daemon's memory subsystem.
#[instrument(skip(ctx))]
pub fn handle_consolidate(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    tracing::info!(
        chat_id = ctx.chat_id,
        "Consolidation requested — requires daemon memory subsystem"
    );

    Ok(HandlerResponse::Html(
        "<b>Memory Consolidation</b>\n\n\
         Consolidation (merging episodic memories into semantic knowledge) \
         requires the daemon's memory subsystem, which is not directly \
         accessible from this handler.\n\n\
         This operation will be available when the full memory pipeline \
         is connected."
            .to_string(),
    ))
}

/// `/memorystats` — Show memory statistics (always local).
///
/// Reports whatever state is accessible from HandlerContext.
#[instrument(skip(ctx))]
pub fn handle_memory_stats(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let pending = ctx.queue.pending_count().unwrap_or(0);
    let audit_entries = ctx.audit.len();

    // Check if we have a channel to the daemon (indicates pipeline health).
    let pipeline_status = match ctx.user_command_tx {
        Some(_) => "connected",
        None => "disconnected",
    };

    Ok(HandlerResponse::Html(format!(
        "<b>Memory Statistics</b>\n\n\
         <pre>\
         Daemon pipeline:  {pipeline_status}\n\
         Message queue:    {pending} pending\n\
         Audit entries:    {audit_entries}\n\
         </pre>\n\n\
         Episodic and semantic memory statistics require direct access \
         to the memory store, which is managed by the daemon process.\n\n\
         The counts above reflect Telegram-side state only."
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
    fn test_remember_empty() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_remember(&ctx, "").unwrap() {
            HandlerResponse::Text(t) => assert!(t.contains("Usage")),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn test_remember_fallback_is_honest() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_remember(&ctx, "buy milk").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Memory Service Unavailable"));
                assert!(html.contains("buy milk"));
                assert!(html.contains("not"));
                // Must NOT claim memory was stored.
                assert!(!html.contains("Memory Stored"));
                assert!(!html.contains("Saved to episodic"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_recall_fallback_is_honest() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_recall(&ctx, "meetings").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Memory Service Unavailable"));
                assert!(html.contains("meetings"));
                assert!(html.contains("No search was performed"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_forget_fallback_never_deletes() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_forget(&ctx, "old stuff").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Memory Service Unavailable"));
                assert!(html.contains("No memories were deleted"));
                assert!(html.contains("PolicyGate"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_memorystats_shows_real_data() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_memory_stats(&ctx).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Memory Statistics"));
                assert!(html.contains("disconnected")); // no channel in test
                assert!(html.contains("0 pending"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_memories_local_handler() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_memories(&ctx, Some("recent")).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Memories"));
                assert!(html.contains("recent"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }
}
