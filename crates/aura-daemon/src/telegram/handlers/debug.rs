//! Debug command handlers: /trace, /dump, /perf, /etg, /goals.
//!
//! These commands are always handled locally (NOT daemon-routed). They
//! read whatever state is accessible from [`HandlerContext`] and report
//! real data where available, with honest "not accessible" messages where not.

use aura_types::errors::AuraError;
use tracing::instrument;

use super::{HandlerContext, HandlerResponse};

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Calculate uptime from startup timestamp.
fn uptime_string(startup_ms: u64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let elapsed_secs = now_ms.saturating_sub(startup_ms) / 1000;

    let days = elapsed_secs / 86400;
    let hours = (elapsed_secs % 86400) / 3600;
    let mins = (elapsed_secs % 3600) / 60;
    let secs = elapsed_secs % 60;

    if days > 0 {
        format!("{days}d {hours}h {mins}m {secs}s")
    } else if hours > 0 {
        format!("{hours}h {mins}m {secs}s")
    } else {
        format!("{mins}m {secs}s")
    }
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `/trace <module>` — Enable verbose tracing for a module.
///
/// Dynamic filter adjustment requires integration with the tracing
/// subscriber's reload handle. Reports the intent honestly.
#[instrument(skip(ctx))]
pub fn handle_trace(ctx: &HandlerContext<'_>, module: &str) -> Result<HandlerResponse, AuraError> {
    tracing::info!(
        chat_id = ctx.chat_id,
        module,
        "Trace level requested for module"
    );

    let html = format!(
        "<b>Trace Request</b>\n\n\
         Module: <code>{module}</code>\n\
         Requested level: TRACE\n\n\
         Dynamic filter adjustment requires the tracing subscriber's \
         reload handle, which is not yet exposed to Telegram handlers.\n\n\
         To enable tracing, set <code>RUST_LOG={module}=trace</code> \
         in the daemon environment or use <code>/set daemon.log_level trace</code>."
    );
    Ok(HandlerResponse::Html(html))
}

/// `/dump <component>` — Dump internal state of a component.
///
/// Reads real data from HandlerContext where available.
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
                "locked:            {locked}\n\
                 pin_set:           {pin_set}\n\
                 allowed_chats:     {allowed}\n\
                 permission_entries: {perms}"
            )
        },
        "queue" => {
            let pending = ctx.queue.pending_count().unwrap_or(0);
            format!("pending_messages:  {pending}")
        },
        "audit" => {
            let len = ctx.audit.len();
            format!("audit_entries:     {len}")
        },
        "config" => match ctx.config {
            Some(cfg) => {
                format!(
                    "daemon.version:         {}\n\
                     daemon.log_level:       {}\n\
                     daemon.data_dir:        {}\n\
                     daemon.checkpoint_s:    {}\n\
                     daemon.rss_warning_mb:  {}\n\
                     daemon.rss_ceiling_mb:  {}\n\
                     voice.enabled:          {}\n\
                     voice.tts_engine:       {}\n\
                     voice.stt_engine:       {}\n\
                     proactive.enabled:      {}\n\
                     proactive.min_conf:     {:.2}\n\
                     telegram.enabled:       {}\n\
                     telegram.poll_ms:       {}\n\
                     power.daily_budget:     {}\n\
                     policy.default_effect:  {}\n\
                     policy.log_decisions:   {}",
                    cfg.daemon.version,
                    cfg.daemon.log_level,
                    cfg.daemon.data_dir,
                    cfg.daemon.checkpoint_interval_s,
                    cfg.daemon.rss_warning_mb,
                    cfg.daemon.rss_ceiling_mb,
                    cfg.voice.enabled,
                    cfg.voice.tts_engine,
                    cfg.voice.stt_engine,
                    cfg.proactive.enabled,
                    cfg.proactive.min_confidence,
                    cfg.telegram.enabled,
                    cfg.telegram.poll_interval_ms,
                    cfg.power.daily_token_budget,
                    cfg.policy.default_effect,
                    cfg.policy.log_all_decisions,
                )
            },
            None => "AuraConfig not available in this handler context.".to_string(),
        },
        "channels" => {
            let pipeline_status = match ctx.user_command_tx {
                Some(_) => "connected (UserCommandTx available)",
                None => "disconnected (UserCommandTx not available)",
            };
            format!("daemon_pipeline:   {pipeline_status}")
        },
        unknown => {
            return Ok(HandlerResponse::text(format!(
                "Unknown component: {unknown}\n\
                 Available: security, queue, audit, config, channels"
            )));
        },
    };

    let html = format!("<b>State Dump: {component}</b>\n\n<pre>{body}</pre>");
    Ok(HandlerResponse::Html(html))
}

/// `/perf` — Show performance metrics.
///
/// Reports real data from HandlerContext: uptime, queue depth, audit count,
/// pipeline status. CPU/memory/thread metrics require OS-level integration.
#[instrument(skip(ctx))]
pub fn handle_perf(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let uptime = uptime_string(ctx.startup_time_ms);
    let pending = ctx.queue.pending_count().unwrap_or(0);
    let audit_entries = ctx.audit.len();
    let locked = ctx.security.is_locked();

    let pipeline_status = match ctx.user_command_tx {
        Some(_) => "connected",
        None => "disconnected",
    };

    let html = format!(
        "<b>Performance Metrics</b>\n\n\
         <pre>\
         Uptime:            {uptime}\n\
         Daemon pipeline:   {pipeline_status}\n\
         Security:          {}\n\
         Queue backlog:     {pending}\n\
         Audit entries:     {audit_entries}\n\
         </pre>\n\n\
         CPU, memory, and thread metrics require OS-level telemetry \
         integration (not yet available from this context).",
        if locked { "LOCKED" } else { "unlocked" },
    );
    Ok(HandlerResponse::Html(html))
}

/// `/etg [app]` — Show element tree graph (accessibility tree).
///
/// The ETG requires the A11y service running in the daemon process,
/// which is not accessible from the Telegram handler context.
#[instrument(skip(ctx))]
pub fn handle_etg(
    ctx: &HandlerContext<'_>,
    app: Option<&str>,
) -> Result<HandlerResponse, AuraError> {
    let target = app.unwrap_or("current screen");

    tracing::info!(
        chat_id = ctx.chat_id,
        target,
        "ETG requested — requires A11y service"
    );

    let html = format!(
        "<b>Element Tree Graph</b>\n\n\
         Target: <code>{target}</code>\n\n\
         The ETG (accessibility node tree) is captured by the Android \
         AccessibilityService running in the daemon process. It is not \
         directly accessible from the Telegram handler context.\n\n\
         When the daemon is fully connected, ETG queries will be routed \
         through the A11y channel."
    );
    Ok(HandlerResponse::Html(html))
}

/// `/goals` — Show active goals from the BDI planning system.
///
/// The BDI scheduler is owned by the daemon process. This handler
/// reports honestly that goals are not accessible from this context.
#[instrument(skip(ctx))]
pub fn handle_goals(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let pipeline_status = match ctx.user_command_tx {
        Some(_) => "connected",
        None => "disconnected",
    };

    tracing::info!(
        chat_id = ctx.chat_id,
        pipeline_status,
        "Goals requested — requires BDI scheduler"
    );

    Ok(HandlerResponse::Html(format!(
        "<b>Active Goals</b>\n\n\
         Daemon pipeline: <code>{pipeline_status}</code>\n\n\
         The BDI (Belief-Desire-Intention) scheduler is owned by the daemon \
         process and is not directly accessible from the Telegram handler.\n\n\
         Goals are created via <code>/plan</code> or <code>/do</code> commands \
         when routed through the daemon pipeline.\n\
         Use <code>/plan &lt;goal&gt;</code> to create a new goal."
    )))
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use aura_types::config::AuraConfig;
    use rusqlite::Connection;

    use super::*;
    use crate::telegram::{audit::AuditLog, queue::MessageQueue, security::SecurityGate};

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
    fn test_dump_security() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_dump(&ctx, "security").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("security"));
                assert!(html.contains("locked"));
                assert!(html.contains("pin_set"));
            },
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
            },
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn test_dump_config_without_config() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_dump(&ctx, "config").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("config"));
                assert!(html.contains("not available"));
            },
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_dump_config_with_config() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let cfg = AuraConfig::default();
        let ctx = HandlerContext {
            chat_id: 42,
            security: &mut sec,
            audit: &mut aud,
            queue: &q,
            startup_time_ms: 1_700_000_000_000,
            config: Some(&cfg),
            user_command_tx: None,
        };

        match handle_dump(&ctx, "config").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("daemon.version"));
                assert!(html.contains("voice.enabled"));
                assert!(html.contains("policy.default_effect"));
            },
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_dump_channels_disconnected() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_dump(&ctx, "channels").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("channels"));
                assert!(html.contains("disconnected"));
            },
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_perf_shows_real_data() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_perf(&ctx).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Performance Metrics"));
                assert!(html.contains("Uptime"));
                assert!(html.contains("disconnected")); // no channel in test
                assert!(html.contains("unlocked")); // not locked
                assert!(html.contains("0")); // 0 pending
            },
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_goals_shows_pipeline_status() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_goals(&ctx).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Active Goals"));
                assert!(html.contains("BDI"));
                assert!(html.contains("disconnected"));
            },
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_etg_shows_target() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_etg(&ctx, Some("com.android.settings")).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Element Tree Graph"));
                assert!(html.contains("com.android.settings"));
                assert!(html.contains("AccessibilityService"));
            },
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_trace_shows_module() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_trace(&ctx, "aura_daemon::neocortex").unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("Trace Request"));
                assert!(html.contains("aura_daemon::neocortex"));
            },
            other => panic!("expected Html, got {other:?}"),
        }
    }
}
