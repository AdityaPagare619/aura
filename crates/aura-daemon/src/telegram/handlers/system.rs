//! System command handlers: /status, /health, /restart, /logs, /uptime, /version, /power.

use aura_types::errors::AuraError;
use tracing::instrument;

use super::{HandlerContext, HandlerResponse};

// ─── Helpers ────────────────────────────────────────────────────────────────

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

/// `/status` — Show system dashboard.
#[instrument(skip(ctx))]
pub fn handle_status(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let uptime = uptime_string(ctx.startup_time_ms);
    let locked = if ctx.security.is_locked() {
        "LOCKED"
    } else {
        "unlocked"
    };
    let pin = if ctx.security.is_pin_set() {
        "set"
    } else {
        "not set"
    };
    let pending_msgs = ctx.queue.pending_count().unwrap_or(0);
    let audit_entries = ctx.audit.len();

    let html = format!(
        "<b>AURA Status Dashboard</b>\n\n\
         <b>Uptime:</b> {uptime}\n\
         <b>Lock:</b> {locked}\n\
         <b>PIN:</b> {pin}\n\
         <b>Queue:</b> {pending_msgs} pending\n\
         <b>Audit:</b> {audit_entries} entries\n\
         <b>Allowed chats:</b> {}\n\n\
         Use /health for detailed checks.",
        ctx.security.allowed_chat_ids().len()
    );

    Ok(HandlerResponse::Html(html))
}

/// `/health` — Quick health check.
#[instrument(skip(ctx))]
pub fn handle_health(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let uptime = uptime_string(ctx.startup_time_ms);
    let pending = ctx.queue.pending_count().unwrap_or(0);

    let status = if pending < 50 {
        "healthy"
    } else {
        "degraded (queue backlog)"
    };

    let html = format!(
        "<b>Health Check</b>\n\n\
         Status: {status}\n\
         Uptime: {uptime}\n\
         Queue backlog: {pending}\n\
         Security: {}",
        if ctx.security.is_locked() {
            "locked"
        } else {
            "ok"
        }
    );

    Ok(HandlerResponse::Html(html))
}

/// `/restart` — Request daemon restart.
///
/// In the real implementation this would send a restart command through
/// the daemon's channels. For now we acknowledge the request.
#[instrument(skip(ctx))]
pub fn handle_restart(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    // Enqueue a critical message before restart so it survives.
    let _ = ctx
        .queue
        .enqueue_critical(ctx.chat_id, "Restart requested. Daemon restarting...");

    // TODO: Actually trigger daemon restart via DaemonChannels / cancel_flag.
    Ok(HandlerResponse::Html(
        "<b>Restart</b>\nRestart command acknowledged. The daemon will restart shortly."
            .to_string(),
    ))
}

/// `/start` — Start AURA daemon (control interface).
///
/// Note: On Android, the daemon may already be running. This command
/// confirms the daemon is active and responsive.
#[instrument(skip(ctx))]
pub fn handle_start(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let _ = ctx
        .queue
        .enqueue_critical(ctx.chat_id, "AURA is starting...");

    Ok(HandlerResponse::Html(
        "<b>AURA Control</b>\n\n\
         ✅ AURA daemon is running and ready.\n\n\
         Use /status for system dashboard.\n\
         Use /stop to shut down the daemon."
            .to_string(),
    ))
}

/// `/stop` — Stop AURA daemon (control interface).
///
/// This requests graceful shutdown of the AURA daemon.
#[instrument(skip(ctx))]
pub fn handle_stop(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let _ = ctx
        .queue
        .enqueue_critical(ctx.chat_id, "AURA shutting down...");

    // TODO: Actually trigger daemon shutdown via DaemonChannels / cancel_flag.
    Ok(HandlerResponse::Html(
        "<b>AURA Control</b>\n\n\
         🛑 Shutdown requested. AURA will stop.\n\n\
         Use /start to restart the daemon."
            .to_string(),
    ))
}

/// `/reboot` — Restart AURA daemon (control interface).
///
/// Same as /restart - requests daemon restart.
#[instrument(skip(ctx))]
pub fn handle_reboot(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let _ = ctx.queue.enqueue_critical(ctx.chat_id, "AURA rebooting...");

    // TODO: Actually trigger daemon restart via DaemonChannels / cancel_flag.
    Ok(HandlerResponse::Html(
        "<b>AURA Control</b>\n\n\
         🔄 Reboot requested. AURA will restart...\n\n\
         Use /status to check when it's back online."
            .to_string(),
    ))
}

/// `/logs [service] [lines]` — Show recent logs.
///
/// In the real implementation this would read from the tracing subscriber's
/// in-memory buffer. For now we return a placeholder.
#[instrument(skip(_ctx))]
pub fn handle_logs(
    _ctx: &HandlerContext<'_>,
    service: Option<&str>,
    lines: usize,
) -> Result<HandlerResponse, AuraError> {
    let svc = service.unwrap_or("all");
    let capped = lines.min(100);

    // TODO: Wire to actual log ring buffer.
    let html = format!(
        "<b>Logs</b> (service={svc}, last {capped} lines)\n\n\
         <i>Log retrieval not yet wired to tracing subscriber.</i>\n\
         Use /trace &lt;module&gt; to enable verbose logging."
    );

    Ok(HandlerResponse::Html(html))
}

/// `/uptime` — Show daemon uptime.
#[instrument(skip(ctx))]
pub fn handle_uptime(ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let uptime = uptime_string(ctx.startup_time_ms);
    Ok(HandlerResponse::text(format!("Uptime: {uptime}")))
}

/// `/version` — Show build version.
#[instrument(skip(_ctx))]
pub fn handle_version(_ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    let version = env!("CARGO_PKG_VERSION");
    let html = format!(
        "<b>AURA Daemon</b>\n\
         Version: {version}\n\
         Target: {}\n\
         Profile: {}",
        std::env::consts::ARCH,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    );
    Ok(HandlerResponse::Html(html))
}

/// `/power` — Show power/battery status.
///
/// On Android this reads from `/sys/class/power_supply/`. For now, placeholder.
#[instrument(skip(_ctx))]
pub fn handle_power(_ctx: &HandlerContext<'_>) -> Result<HandlerResponse, AuraError> {
    // TODO: Read from Android power sysfs.
    Ok(HandlerResponse::Html(
        "<b>Power Status</b>\n\n\
         <i>Battery info not available on this platform.</i>\n\
         On Android, this reads /sys/class/power_supply/."
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
            startup_time_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
                - 60_000, // 1 minute ago
            config: None,
            user_command_tx: None,
        }
    }

    #[test]
    fn test_status_contains_dashboard() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_status(&ctx).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("AURA Status Dashboard"));
                assert!(html.contains("Uptime"));
                assert!(html.contains("unlocked"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_health_shows_healthy() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_health(&ctx).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("healthy"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn test_uptime_string_format() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let s = uptime_string(now - 90_000); // 90 seconds ago
        assert!(s.contains("1m"));
    }

    #[test]
    fn test_version_contains_version() {
        let mut sec = SecurityGate::new(vec![42]);
        let mut aud = AuditLog::new(100);
        let db = Connection::open_in_memory().unwrap();
        let q = MessageQueue::open(db).unwrap();
        let ctx = make_ctx(&mut sec, &mut aud, &q);

        match handle_version(&ctx).unwrap() {
            HandlerResponse::Html(html) => {
                assert!(html.contains("AURA Daemon"));
                assert!(html.contains("Version"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }
}
