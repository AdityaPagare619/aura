//! Memory command handlers: /remember, /recall, /forget, /memories, /consolidate, /memorystats.
//!
//! These commands interact with AURA's episodic and semantic memory systems
//! through the daemon's database write channel. Until fully wired to the
//! memory subsystem, handlers return acknowledgements and stubs.

use aura_types::errors::AuraError;
use tracing::instrument;

use super::{HandlerContext, HandlerResponse};

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `/remember <text>` — Store a memory.
#[instrument(skip(_ctx))]
pub fn handle_remember(
    _ctx: &HandlerContext<'_>,
    text: &str,
) -> Result<HandlerResponse, AuraError> {
    if text.is_empty() {
        return Ok(HandlerResponse::text("Usage: /remember <text>"));
    }

    // TODO: Send DbWriteRequest::Episode through DaemonChannels.
    Ok(HandlerResponse::Html(format!(
        "<b>Memory Stored</b>\n\n\
         <i>{}</i>\n\n\
         Saved to episodic memory. Use /recall to search.",
        escape_html(truncate(text, 500))
    )))
}

/// `/recall <query>` — Search memories.
#[instrument(skip(_ctx))]
pub fn handle_recall(_ctx: &HandlerContext<'_>, query: &str) -> Result<HandlerResponse, AuraError> {
    if query.is_empty() {
        return Ok(HandlerResponse::text("Usage: /recall <query>"));
    }

    // TODO: Query episodic memory via semantic search.
    Ok(HandlerResponse::Html(format!(
        "<b>Memory Search</b>\n\nQuery: <i>{}</i>\n\n\
         <i>Memory search not yet wired to vector store.</i>\n\
         Results will appear here when connected.",
        escape_html(truncate(query, 300))
    )))
}

/// `/forget <query>` — Delete matching memories.
///
/// This is a destructive operation (Modify permission required).
#[instrument(skip(_ctx))]
pub fn handle_forget(_ctx: &HandlerContext<'_>, query: &str) -> Result<HandlerResponse, AuraError> {
    if query.is_empty() {
        return Ok(HandlerResponse::text("Usage: /forget <query>"));
    }

    // TODO: This should trigger a dialogue flow for confirmation
    // (handled by the dialogue FSM, not directly here).
    Ok(HandlerResponse::Html(format!(
        "<b>Forget Request</b>\n\nQuery: <i>{}</i>\n\n\
         This will delete matching memories.\n\
         <i>Deletion pipeline not yet wired. Use dialogue flow for confirmation.</i>",
        escape_html(truncate(query, 300))
    )))
}

/// `/memories [filter]` — List memories.
#[instrument(skip(_ctx))]
pub fn handle_memories(
    _ctx: &HandlerContext<'_>,
    filter: Option<&str>,
) -> Result<HandlerResponse, AuraError> {
    let filter_str = filter.unwrap_or("all");

    // TODO: Query memory index for listing.
    Ok(HandlerResponse::Html(format!(
        "<b>Memories</b> (filter: {filter_str})\n\n\
         <i>Memory listing not yet wired to store.</i>\n\
         Use /memorystats for aggregate statistics."
    )))
}

/// `/consolidate` — Trigger memory consolidation.
#[instrument(skip(_ctx))]
pub fn handle_consolidate(_ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    // TODO: Send consolidation trigger via DaemonChannels.
    Ok(HandlerResponse::Html(
        "<b>Consolidation</b>\n\n\
         Memory consolidation triggered.\n\
         This merges similar episodic memories into semantic knowledge.\n\
         <i>Consolidation pipeline not yet wired.</i>"
            .to_string(),
    ))
}

/// `/memorystats` — Show memory statistics.
#[instrument(skip(_ctx))]
pub fn handle_memory_stats(_ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    // TODO: Query memory subsystem for stats.
    Ok(HandlerResponse::Html(
        "<b>Memory Statistics</b>\n\n\
         Episodic: <i>not connected</i>\n\
         Semantic: <i>not connected</i>\n\
         Working: <i>not connected</i>\n\n\
         <i>Stats will be available when memory subsystem is wired.</i>"
            .to_string(),
    ))
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
    fn test_remember_valid() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_remember(&ctx, "buy milk").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Memory Stored"));
                assert!(html.contains("buy milk"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_memorystats() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_memory_stats(&ctx).unwrap() {
            HandlerResponse::Html(html) => assert!(html.contains("Memory Statistics")),
            other => panic!("expected Html, got {other:?}"),
        }
    }
}
