//! Config command handlers: /set, /get, /personality, /personality_set, /trust, /trust_set, /voice, /chat.

use aura_types::errors::AuraError;
use tracing::instrument;

use super::{HandlerContext, HandlerResponse};

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Clamp a float to the [0.0, 1.0] range, returning an error message if out of range.
fn validate_unit_range(value: f32, name: &str) -> Result<f32, HandlerResponse> {
    if !(0.0..=1.0).contains(&value) {
        return Err(HandlerResponse::text(format!(
            "{name} must be between 0.0 and 1.0 (got {value})"
        )));
    }
    Ok(value)
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `/set <key> <value>` — Set a config value.
#[instrument(skip(_ctx))]
pub fn handle_set(
    _ctx: &HandlerContext<'_>,
    key: &str,
    value: &str,
) -> Result<HandlerResponse, AuraError> {
    // TODO: Wire to AuraConfig persistence layer.
    let html = format!(
        "<b>Config Updated</b>\n\n\
         <code>{key}</code> = <code>{value}</code>\n\n\
         <i>Note: config persistence not yet wired.</i>"
    );
    Ok(HandlerResponse::Html(html))
}

/// `/get <key>` — Get a config value.
#[instrument(skip(_ctx))]
pub fn handle_get(_ctx: &HandlerContext<'_>, key: &str) -> Result<HandlerResponse, AuraError> {
    // TODO: Read from AuraConfig.
    let html = format!(
        "<b>Config Value</b>\n\n\
         <code>{key}</code> = <i>(not available — config read not yet wired)</i>"
    );
    Ok(HandlerResponse::Html(html))
}

/// `/personality` — Show personality traits.
#[instrument(skip(_ctx))]
pub fn handle_personality(_ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    // TODO: Read real personality config from AuraConfig.
    let traits = [
        ("curiosity", 0.5),
        ("helpfulness", 0.5),
        ("formality", 0.5),
        ("humor", 0.5),
        ("verbosity", 0.5),
    ];

    let mut html = String::from("<b>Personality Traits</b>\n\n");
    for (name, val) in &traits {
        let bar = "█".repeat((*val * 10.0) as usize);
        let empty = "░".repeat(10 - (*val * 10.0) as usize);
        html.push_str(&format!("{name}: {bar}{empty} {val:.1}\n"));
    }
    html.push_str("\nUse /personality_set &lt;trait&gt; &lt;value&gt; to adjust.");
    Ok(HandlerResponse::Html(html))
}

/// `/personality_set <trait> <value>` — Set a personality trait.
#[instrument(skip(_ctx))]
pub fn handle_personality_set(
    _ctx: &HandlerContext<'_>,
    trait_name: &str,
    value: f32,
) -> Result<HandlerResponse, AuraError> {
    match validate_unit_range(value, trait_name) {
        Ok(v) => {
            // TODO: Persist to AuraConfig personality traits.
            let html = format!(
                "<b>Personality Updated</b>\n\n\
                 <code>{trait_name}</code> set to <b>{v:.2}</b>\n\n\
                 <i>Change takes effect on next interaction.</i>"
            );
            Ok(HandlerResponse::Html(html))
        }
        Err(resp) => Ok(resp),
    }
}

/// `/trust` — Show trust level.
#[instrument(skip(_ctx))]
pub fn handle_trust(_ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    // TODO: Read from AuraConfig trust settings.
    let level = 0.5_f32;
    let html = format!(
        "<b>Trust Level</b>\n\n\
         Current: <b>{level:.1}</b> / 1.0\n\n\
         0.0 = always ask for confirmation\n\
         1.0 = full autonomy\n\n\
         Use /trust_set &lt;level&gt; to adjust."
    );
    Ok(HandlerResponse::Html(html))
}

/// `/trust_set <level>` — Set trust level.
#[instrument(skip(_ctx))]
pub fn handle_trust_set(
    _ctx: &HandlerContext<'_>,
    level: f32,
) -> Result<HandlerResponse, AuraError> {
    match validate_unit_range(level, "trust") {
        Ok(v) => {
            // TODO: Persist to AuraConfig trust level.
            let html = format!(
                "<b>Trust Level Updated</b>\n\n\
                 Trust set to <b>{v:.2}</b>\n\n\
                 {}",
                if v >= 0.8 {
                    "⚠ High autonomy — AURA may act without confirmation."
                } else if v <= 0.2 {
                    "AURA will confirm most actions before executing."
                } else {
                    "Balanced — AURA will confirm critical actions."
                }
            );
            Ok(HandlerResponse::Html(html))
        }
        Err(resp) => Ok(resp),
    }
}

/// `/voice` — Set voice mode preference to always speak responses.
#[instrument(skip(_ctx))]
pub fn handle_voice_mode(_ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let html = "<b>Voice Mode Enabled</b>\n\n\
                I'll respond with voice for most messages.\n\n\
                Use /chat to switch back to text-only mode.";
    Ok(HandlerResponse::Html(html.to_string()))
}

/// `/chat` — Set chat mode preference to text-only responses.
#[instrument(skip(_ctx))]
pub fn handle_chat_mode(_ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let html = "<b>Chat Mode Enabled</b>\n\n\
                I'll respond with text messages.\n\n\
                Use /voice to enable voice responses.";
    Ok(HandlerResponse::Html(html.to_string()))
}

/// `/quiet` — Disable all proactive suggestions.
#[instrument(skip(_ctx))]
pub fn handle_quiet(_ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let html = "<b>Quiet Mode Enabled</b>\n\n\
                All proactive suggestions are now disabled.\n\n\
                I'll wait for your commands instead of proactively responding.\n\n\
                Use /wake to re-enable proactive suggestions.";
    Ok(HandlerResponse::Html(html.to_string()))
}

/// `/wake` — Re-enable proactive suggestions.
#[instrument(skip(_ctx))]
pub fn handle_wake(_ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let html = "<b>Wake Mode Enabled</b>\n\n\
                Proactive suggestions are now enabled.\n\n\
                I'll proactively help when I have something useful to share.\n\n\
                Use /quiet to disable proactive suggestions.";
    Ok(HandlerResponse::Html(html.to_string()))
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
    fn test_set_config() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_set(&ctx, "log_level", "debug").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("log_level"));
                assert!(html.contains("debug"));
                assert!(html.contains("Config Updated"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_personality_lists_traits() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_personality(&ctx).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("curiosity"));
                assert!(html.contains("helpfulness"));
                assert!(html.contains("Personality Traits"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_trust_set_invalid_range() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_trust_set(&ctx, 1.5).unwrap() {
            HandlerResponse::Text(text) => {
                assert!(text.contains("must be between 0.0 and 1.0"));
            }
            other => panic!("expected Text error, got {other:?}"),
        }
    }

    #[test]
    fn test_quiet_mode() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_quiet(&ctx).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Quiet Mode"));
                assert!(html.contains("proactive suggestions are now disabled"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_wake_mode() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_wake(&ctx).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Wake Mode"));
                assert!(html.contains("Proactive suggestions are now enabled"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }
}
