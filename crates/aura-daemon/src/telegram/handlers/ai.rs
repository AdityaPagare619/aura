//! AI command handlers: /ask, /think, /plan, /explain, /summarize, /translate.
//!
//! These commands forward requests to the Neocortex (AURA's LLM backend)
//! via the daemon's IPC channels. Since the Neocortex call is async and
//! potentially slow, handlers enqueue a "thinking..." placeholder and
//! update it when the response arrives.
//!
//! Until the IPC integration is wired, handlers return stub responses
//! acknowledging the request.

use aura_types::errors::AuraError;
use tracing::instrument;

use super::{HandlerContext, HandlerResponse};

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `/ask <question>` — Ask AURA a question.
#[instrument(skip(ctx))]
pub fn handle_ask(ctx: &HandlerContext<'_>, question: &str) -> Result<HandlerResponse, AuraError> {
    if question.is_empty() {
        return Ok(HandlerResponse::text("Usage: /ask <question>"));
    }

    // TODO: Forward to Neocortex via IPC channel.
    // For now, enqueue a placeholder and return acknowledgement.
    let _ = ctx.queue.enqueue_text(
        ctx.chat_id,
        &format!("Thinking about: {}", truncate(question, 200)),
        None,
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Question received</b>\n\n<i>{}</i>\n\n\
         Processing via Neocortex... response will follow.",
        escape_html(truncate(question, 500))
    )))
}

/// `/think <problem>` — Deep reasoning on a problem.
#[instrument(skip(_ctx))]
pub fn handle_think(
    _ctx: &HandlerContext<'_>,
    problem: &str,
) -> Result<HandlerResponse, AuraError> {
    if problem.is_empty() {
        return Ok(HandlerResponse::text("Usage: /think <problem>"));
    }

    // TODO: Forward to Neocortex with reasoning mode.
    Ok(HandlerResponse::Html(format!(
        "<b>Deep Reasoning</b>\n\n<i>{}</i>\n\n\
         Engaging extended reasoning... this may take a moment.",
        escape_html(truncate(problem, 500))
    )))
}

/// `/plan <goal>` — Generate a plan for a goal.
#[instrument(skip(_ctx))]
pub fn handle_plan(_ctx: &HandlerContext<'_>, goal: &str) -> Result<HandlerResponse, AuraError> {
    if goal.is_empty() {
        return Ok(HandlerResponse::text("Usage: /plan <goal>"));
    }

    // TODO: Forward to Neocortex planning pipeline.
    Ok(HandlerResponse::Html(format!(
        "<b>Plan Generation</b>\n\nGoal: <i>{}</i>\n\n\
         Generating action plan... response will follow.",
        escape_html(truncate(goal, 500))
    )))
}

/// `/explain <topic>` — Explain a topic.
#[instrument(skip(_ctx))]
pub fn handle_explain(
    _ctx: &HandlerContext<'_>,
    topic: &str,
) -> Result<HandlerResponse, AuraError> {
    if topic.is_empty() {
        return Ok(HandlerResponse::text("Usage: /explain <topic>"));
    }

    Ok(HandlerResponse::Html(format!(
        "<b>Explanation</b>\n\nTopic: <i>{}</i>\n\n\
         Preparing explanation... response will follow.",
        escape_html(truncate(topic, 500))
    )))
}

/// `/summarize <text>` — Summarize text.
#[instrument(skip(_ctx))]
pub fn handle_summarize(
    _ctx: &HandlerContext<'_>,
    text: &str,
) -> Result<HandlerResponse, AuraError> {
    if text.is_empty() {
        return Ok(HandlerResponse::text("Usage: /summarize <text>"));
    }

    let word_count = text.split_whitespace().count();
    Ok(HandlerResponse::Html(format!(
        "<b>Summarize</b>\n\nInput: {word_count} words\n\n\
         Generating summary... response will follow."
    )))
}

/// `/translate <text> <lang>` — Translate text.
#[instrument(skip(_ctx))]
pub fn handle_translate(
    _ctx: &HandlerContext<'_>,
    text: &str,
    target_lang: &str,
) -> Result<HandlerResponse, AuraError> {
    if text.is_empty() {
        return Ok(HandlerResponse::text("Usage: /translate <text> <lang>"));
    }

    Ok(HandlerResponse::Html(format!(
        "<b>Translate</b>\n\nTarget: {target_lang}\n\
         Input: <i>{}</i>\n\n\
         Translating... response will follow.",
        escape_html(truncate(text, 300))
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

/// Escape HTML special characters for Telegram's HTML parse mode.
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
    fn test_ask_empty_input() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_ask(&ctx, "").unwrap() {
            HandlerResponse::Text(t) => assert!(t.contains("Usage")),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn test_ask_valid() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_ask(&ctx, "what is rust?").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Question received"));
                assert!(html.contains("what is rust?"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_escape_html() {
        assert_eq!(escape_html("<b>test&</b>"), "&lt;b&gt;test&amp;&lt;/b&gt;");
    }
}
