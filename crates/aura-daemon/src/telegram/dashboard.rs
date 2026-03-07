//! HTML health dashboard generator for the Telegram bot.
//!
//! Generates a rich HTML snapshot of AURA's current state, suitable for
//! sending as a Telegram message with `parse_mode: "HTML"`. The dashboard
//! is intentionally self-contained (no external CSS/JS) so it renders
//! correctly in Telegram's limited HTML subset.

use tracing::instrument;

use super::audit::AuditLog;
use super::queue::MessageQueue;
use super::security::SecurityGate;

// ─── Dashboard data ─────────────────────────────────────────────────────────

/// Snapshot of system state used to render the dashboard.
pub struct DashboardSnapshot {
    /// Daemon uptime as a formatted string.
    pub uptime: String,
    /// Whether the bot is locked.
    pub locked: bool,
    /// Whether a PIN is configured.
    pub pin_set: bool,
    /// Number of allowed chat IDs.
    pub allowed_chats: usize,
    /// Number of pending messages in the queue.
    pub queue_pending: usize,
    /// Number of audit log entries.
    pub audit_entries: usize,
    /// Number of denied audit entries (security violations).
    pub audit_denied: usize,
    /// Build version string.
    pub version: String,
    /// Architecture string.
    pub arch: String,
    /// Build profile (debug/release).
    pub profile: String,
}

impl DashboardSnapshot {
    /// Collect a snapshot from the current system state.
    #[instrument(skip_all)]
    pub fn collect(
        security: &SecurityGate,
        audit: &AuditLog,
        queue: &MessageQueue,
        startup_time_ms: u64,
    ) -> Self {
        let uptime = format_uptime(startup_time_ms);
        let queue_pending = queue.pending_count().unwrap_or(0);

        Self {
            uptime,
            locked: security.is_locked(),
            pin_set: security.is_pin_set(),
            allowed_chats: security.allowed_chat_ids().len(),
            queue_pending,
            audit_entries: audit.len(),
            audit_denied: audit.denied().len(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            arch: std::env::consts::ARCH.to_string(),
            profile: if cfg!(debug_assertions) {
                "debug".to_string()
            } else {
                "release".to_string()
            },
        }
    }
}

// ─── Rendering ──────────────────────────────────────────────────────────────

/// Render a full HTML dashboard from a snapshot.
///
/// Output uses only the HTML tags supported by Telegram:
/// `<b>`, `<i>`, `<code>`, `<pre>`, `<a>`.
#[instrument(skip(snap))]
pub fn render_dashboard(snap: &DashboardSnapshot) -> String {
    let health_icon = if snap.queue_pending < 50 && snap.audit_denied == 0 {
        "OK"
    } else if snap.queue_pending < 200 {
        "DEGRADED"
    } else {
        "CRITICAL"
    };

    let lock_label = if snap.locked { "LOCKED" } else { "unlocked" };
    let pin_label = if snap.pin_set { "set" } else { "not set" };

    let mut html = String::with_capacity(1024);

    // Header.
    html.push_str("<b>AURA Health Dashboard</b>\n");
    html.push_str(&format!("Status: <b>{health_icon}</b>\n\n"));

    // System section.
    html.push_str("<b>System</b>\n");
    html.push_str(&format!("  Uptime:   {}\n", snap.uptime));
    html.push_str(&format!(
        "  Version:  {} ({})\n",
        snap.version, snap.profile
    ));
    html.push_str(&format!("  Arch:     {}\n\n", snap.arch));

    // Security section.
    html.push_str("<b>Security</b>\n");
    html.push_str(&format!("  Lock:     {lock_label}\n"));
    html.push_str(&format!("  PIN:      {pin_label}\n"));
    html.push_str(&format!("  Allowed:  {} chats\n", snap.allowed_chats));
    html.push_str(&format!("  Denied:   {} attempts\n\n", snap.audit_denied));

    // Queue section.
    html.push_str("<b>Message Queue</b>\n");
    html.push_str(&format!("  Pending:  {}\n", snap.queue_pending));

    let queue_health = if snap.queue_pending == 0 {
        "idle"
    } else if snap.queue_pending < 10 {
        "healthy"
    } else if snap.queue_pending < 50 {
        "busy"
    } else {
        "backlogged"
    };
    html.push_str(&format!("  Status:   {queue_health}\n\n"));

    // Audit section.
    html.push_str("<b>Audit</b>\n");
    html.push_str(&format!("  Entries:  {}\n", snap.audit_entries));
    html.push_str(&format!("  Denied:   {}\n\n", snap.audit_denied));

    // Footer.
    html.push_str("Use /health for a quick check, /audit for details.");

    html
}

/// Render a compact one-line health check.
pub fn render_health_oneliner(snap: &DashboardSnapshot) -> String {
    let status = if snap.queue_pending < 50 {
        "OK"
    } else {
        "DEGRADED"
    };
    format!(
        "{status} | up {} | q:{} | audit:{} | {}",
        snap.uptime,
        snap.queue_pending,
        snap.audit_entries,
        if snap.locked { "LOCKED" } else { "unlocked" },
    )
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn format_uptime(startup_ms: u64) -> String {
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
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m {secs}s")
    } else {
        format!("{mins}m {secs}s")
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_snapshot() -> DashboardSnapshot {
        DashboardSnapshot {
            uptime: "1h 23m 45s".to_string(),
            locked: false,
            pin_set: true,
            allowed_chats: 2,
            queue_pending: 5,
            audit_entries: 42,
            audit_denied: 1,
            version: "0.4.0".to_string(),
            arch: "aarch64".to_string(),
            profile: "release".to_string(),
        }
    }

    #[test]
    fn test_render_dashboard_contains_sections() {
        let snap = test_snapshot();
        let html = render_dashboard(&snap);

        assert!(html.contains("AURA Health Dashboard"));
        assert!(html.contains("System"));
        assert!(html.contains("Security"));
        assert!(html.contains("Message Queue"));
        assert!(html.contains("Audit"));
        assert!(html.contains("1h 23m 45s"));
    }

    #[test]
    fn test_health_status_ok() {
        let snap = test_snapshot();
        let html = render_dashboard(&snap);
        // queue_pending=5 and audit_denied=1 > 0, so status should be DEGRADED
        // Actually: queue_pending < 50 && audit_denied == 0 is false (denied=1),
        // but queue_pending < 200 is true, so DEGRADED.
        assert!(html.contains("DEGRADED"));
    }

    #[test]
    fn test_health_status_critical() {
        let mut snap = test_snapshot();
        snap.queue_pending = 500;
        let html = render_dashboard(&snap);
        assert!(html.contains("CRITICAL"));
    }

    #[test]
    fn test_oneliner() {
        let snap = test_snapshot();
        let line = render_health_oneliner(&snap);
        assert!(line.contains("OK"));
        assert!(line.contains("1h 23m 45s"));
        assert!(line.contains("q:5"));
    }

    #[test]
    fn test_dashboard_locked_state() {
        let mut snap = test_snapshot();
        snap.locked = true;
        let html = render_dashboard(&snap);
        assert!(html.contains("LOCKED"));
    }
}
