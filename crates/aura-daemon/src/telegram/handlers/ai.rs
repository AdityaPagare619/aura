//! AI command handlers: /ask, /think, /plan, /explain, /summarize, /translate.
//!
//! These commands are **daemon-routed** — when the cognitive pipeline is
//! available, `dispatch()` in `mod.rs` forwards them via [`UserCommandTx`]
//! and these handlers are never called.
//!
//! The functions below are **fallback handlers**: they execute only when the
//! daemon channel is unavailable (channel closed, daemon starting up, or
//! running in degraded mode). They must be HONEST — never pretend the LLM
//! is processing when it isn't.

use aura_types::errors::AuraError;
use tracing::instrument;

use super::{HandlerContext, HandlerResponse};

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `/ask <question>` — Fallback when the Neocortex pipeline is unavailable.
///
/// This handler only runs if the daemon channel is closed or absent.
/// It acknowledges the user's question honestly and explains the situation.
#[instrument(skip(ctx))]
pub fn handle_ask(ctx: &HandlerContext<'_>, question: &str) -> Result<HandlerResponse, AuraError> {
    if question.is_empty() {
        return Ok(HandlerResponse::text("Usage: /ask <question>"));
    }

    tracing::warn!(
        chat_id = ctx.chat_id,
        question = truncate(question, 100),
        "AI fallback: /ask reached local handler — daemon pipeline unavailable"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Cognitive Engine Unavailable</b>\n\n\
         Your question: <i>{}</i>\n\n\
         AURA's reasoning engine is not yet connected. \
         This happens during startup or if the inference process is loading.\n\n\
         Your question was <b>not</b> processed. Please retry in a moment with:\n\
         <code>/ask {}</code>",
        escape_html(truncate(question, 500)),
        escape_html(truncate(question, 100)),
    )))
}

/// `/think <problem>` — Fallback when the Neocortex pipeline is unavailable.
#[instrument(skip(ctx))]
pub fn handle_think(
    ctx: &HandlerContext<'_>,
    problem: &str,
) -> Result<HandlerResponse, AuraError> {
    if problem.is_empty() {
        return Ok(HandlerResponse::text("Usage: /think <problem>"));
    }

    tracing::warn!(
        chat_id = ctx.chat_id,
        problem = truncate(problem, 100),
        "AI fallback: /think reached local handler — daemon pipeline unavailable"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Cognitive Engine Unavailable</b>\n\n\
         Problem: <i>{}</i>\n\n\
         Deep reasoning requires the inference engine, which is currently \
         offline or starting up.\n\n\
         Your request was <b>not</b> processed. Please retry shortly.",
        escape_html(truncate(problem, 500)),
    )))
}

/// `/plan <goal>` — Fallback when the Neocortex pipeline is unavailable.
#[instrument(skip(ctx))]
pub fn handle_plan(ctx: &HandlerContext<'_>, goal: &str) -> Result<HandlerResponse, AuraError> {
    if goal.is_empty() {
        return Ok(HandlerResponse::text("Usage: /plan <goal>"));
    }

    tracing::warn!(
        chat_id = ctx.chat_id,
        goal = truncate(goal, 100),
        "AI fallback: /plan reached local handler — daemon pipeline unavailable"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Cognitive Engine Unavailable</b>\n\n\
         Goal: <i>{}</i>\n\n\
         Plan generation requires the inference engine, which is currently \
         offline or starting up.\n\n\
         Your request was <b>not</b> processed. Please retry shortly.",
        escape_html(truncate(goal, 500)),
    )))
}

/// `/explain <topic>` — Fallback when the Neocortex pipeline is unavailable.
#[instrument(skip(ctx))]
pub fn handle_explain(
    ctx: &HandlerContext<'_>,
    topic: &str,
) -> Result<HandlerResponse, AuraError> {
    if topic.is_empty() {
        return Ok(HandlerResponse::text("Usage: /explain <topic>"));
    }

    tracing::warn!(
        chat_id = ctx.chat_id,
        topic = truncate(topic, 100),
        "AI fallback: /explain reached local handler — daemon pipeline unavailable"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Cognitive Engine Unavailable</b>\n\n\
         Topic: <i>{}</i>\n\n\
         Explanations require the inference engine, which is currently \
         offline or starting up.\n\n\
         Your request was <b>not</b> processed. Please retry shortly.",
        escape_html(truncate(topic, 500)),
    )))
}

/// `/summarize <text>` — Fallback when the Neocortex pipeline is unavailable.
#[instrument(skip(ctx))]
pub fn handle_summarize(
    ctx: &HandlerContext<'_>,
    text: &str,
) -> Result<HandlerResponse, AuraError> {
    if text.is_empty() {
        return Ok(HandlerResponse::text("Usage: /summarize <text>"));
    }

    let word_count = text.split_whitespace().count();
    tracing::warn!(
        chat_id = ctx.chat_id,
        word_count,
        "AI fallback: /summarize reached local handler — daemon pipeline unavailable"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Cognitive Engine Unavailable</b>\n\n\
         Input: {word_count} words\n\n\
         Summarization requires the inference engine, which is currently \
         offline or starting up.\n\n\
         Your text was <b>not</b> processed. Please retry shortly."
    )))
}

/// `/translate <text> <lang>` — Fallback when the Neocortex pipeline is unavailable.
#[instrument(skip(ctx))]
pub fn handle_translate(
    ctx: &HandlerContext<'_>,
    text: &str,
    target_lang: &str,
) -> Result<HandlerResponse, AuraError> {
    if text.is_empty() {
        return Ok(HandlerResponse::text("Usage: /translate <text> <lang>"));
    }

    tracing::warn!(
        chat_id = ctx.chat_id,
        target_lang,
        text_len = text.len(),
        "AI fallback: /translate reached local handler — daemon pipeline unavailable"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Cognitive Engine Unavailable</b>\n\n\
         Target language: <code>{target_lang}</code>\n\
         Input: <i>{}</i>\n\n\
         Translation requires the inference engine, which is currently \
         offline or starting up.\n\n\
         Your text was <b>not</b> processed. Please retry shortly.",
        escape_html(truncate(text, 300)),
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
            config: None,
            user_command_tx: None,
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
    fn test_ask_fallback_is_honest() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_ask(&ctx, "what is rust?").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Cognitive Engine Unavailable"));
                assert!(html.contains("what is rust?"));
                assert!(html.contains("not"));
                // Must NOT contain misleading "Processing" or "Thinking"
                assert!(!html.contains("Processing via Neocortex"));
                assert!(!html.contains("Thinking about"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_think_fallback_is_honest() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_think(&ctx, "halting problem").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Cognitive Engine Unavailable"));
                assert!(html.contains("halting problem"));
                assert!(html.contains("not"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_translate_fallback_includes_lang() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_translate(&ctx, "hello world", "es").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Cognitive Engine Unavailable"));
                assert!(html.contains("es"));
                assert!(html.contains("hello world"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_escape_html() {
        assert_eq!(escape_html("<b>test&</b>"), "&lt;b&gt;test&amp;&lt;/b&gt;");
    }
}
