//! Prometheus metrics exporter for AURA v4 daemon.
//!
//! Bridges the existing [`TelemetryEngine`] (ring buffer + counters) to
//! Prometheus-compatible text exposition format. Serves metrics over HTTP
//! on a configurable port (default 9090).
//!
//! # Architecture
//!
//! ```text
//! TelemetryEngine ──→ PrometheusExporter ──→ HTTP /metrics
//! HealthMonitor   ──→         │
//! MemorySystem    ──→         │
//! ```
//!
//! The exporter reads from three sources:
//! 1. **TelemetryEngine** — counters + ring buffer metrics (existing `export_prometheus()`)
//! 2. **HealthMonitor** — health status gauges (battery, thermal, error rate, neocortex)
//! 3. **AuraMemory** — per-tier memory usage (working slots, episodic/semantic counts)
//!
//! # Usage
//!
//! ```ignore
//! let exporter = PrometheusExporter::new(9090);
//! exporter.start(telemetry, health_monitor, memory).await?;
//! // GET http://localhost:9090/metrics returns Prometheus text format
//! ```

use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Default metrics HTTP port.
pub const DEFAULT_METRICS_PORT: u16 = 9090;

/// Health snapshot data for Prometheus exposition.
/// Mirrors the fields from `HealthReport` but in a lightweight form.
#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    pub neocortex_alive: bool,
    pub memory_accessible: bool,
    pub memory_usage_bytes: u64,
    pub battery_level: f32,
    pub thermal_state: u8, // 0=Normal, 1=Warm, 2=Hot, 3=Critical, 4=Shutdown
    pub thermal_celsius: Option<f32>,
    pub a11y_connected: bool,
    pub error_rate_last_hour: f32,
    pub overall_status: u8, // 0=Healthy, 1=Degraded, 2=Critical
    pub session_number: u64,
    pub consecutive_healthy: u32,
    pub low_power_mode: bool,
    pub neocortex_restart_attempts: u32,
    // Memory system metrics
    pub working_memory_bytes: u64,
    pub working_slot_count: usize,
    pub episodic_memory_bytes: u64,
    pub episodic_count: u64,
    pub semantic_memory_bytes: u64,
    pub semantic_count: u64,
    pub archive_memory_bytes: u64,
    pub archive_count: u64,
}

/// Prometheus metrics exporter — serves `/metrics` over HTTP.
pub struct PrometheusExporter {
    port: u16,
}

impl PrometheusExporter {
    /// Create a new exporter bound to the given port.
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    /// Create with default port.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_METRICS_PORT)
    }

    /// Start the HTTP metrics server.
    ///
    /// Binds to `0.0.0.0:{port}` and serves Prometheus text exposition on
    /// `GET /metrics`. The server runs until the returned handle is dropped
    /// or the cancellation token fires.
    ///
    /// # Arguments
    ///
    /// * `telemetry_prometheus` — Pre-rendered Prometheus text from `TelemetryEngine::export_prometheus()`.
    ///   Updated periodically by the caller (e.g., every scrape_interval).
    /// * `health_snapshot` — Shared health data updated by the heartbeat loop.
    pub async fn start(
        &self,
        telemetry_prometheus: Arc<RwLock<String>>,
        health_snapshot: Arc<RwLock<MetricsSnapshot>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let addr = format!("0.0.0.0:{}", self.port);
        let listener = TcpListener::bind(&addr).await?;
        info!(port = self.port, "Prometheus metrics server listening");

        loop {
            match listener.accept().await {
                Ok((mut stream, peer)) => {
                    debug!(%peer, "metrics request received");

                    let telemetry = telemetry_prometheus.read().await.clone();
                    let health = health_snapshot.read().await.clone();
                    let body = render_metrics(&telemetry, &health);

                    let response = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: text/plain; version=0.0.4; charset=utf-8\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\
                         \r\n\
                         {}",
                        body.len(),
                        body
                    );

                    if let Err(e) = stream.write_all(response.as_bytes()).await {
                        warn!(error = %e, "failed to write metrics response");
                    }
                }
                Err(e) => {
                    error!(error = %e, "failed to accept metrics connection");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
}

/// Render the full Prometheus exposition text.
///
/// Combines telemetry counters/ring metrics with health gauges and memory stats.
fn render_metrics(telemetry: &str, health: &MetricsSnapshot) -> String {
    let mut out = String::with_capacity(4096);

    // ── Telemetry (counters + ring buffer) ──
    out.push_str("# Telemetry counters and ring buffer metrics\n");
    out.push_str(telemetry);
    if !telemetry.ends_with('\n') {
        out.push('\n');
    }

    // ── Health Status Gauges ──
    out.push_str("\n# HELP aura_neocortex_alive Whether the neocortex inference engine is responding (1=yes, 0=no).\n");
    out.push_str("# TYPE aura_neocortex_alive gauge\n");
    out.push_str(&format!(
        "aura_neocortex_alive {}\n",
        if health.neocortex_alive { 1 } else { 0 }
    ));

    out.push_str("\n# HELP aura_memory_accessible Whether shared memory region is accessible (1=yes, 0=no).\n");
    out.push_str("# TYPE aura_memory_accessible gauge\n");
    out.push_str(&format!(
        "aura_memory_accessible {}\n",
        if health.memory_accessible { 1 } else { 0 }
    ));

    out.push_str(
        "\n# HELP aura_memory_pressure_bytes Process RSS in bytes (from health monitor).\n",
    );
    out.push_str("# TYPE aura_memory_pressure_bytes gauge\n");
    out.push_str(&format!(
        "aura_memory_pressure_bytes {}\n",
        health.memory_usage_bytes
    ));

    out.push_str("\n# HELP aura_battery_pct Battery charge level as percentage (0-100).\n");
    out.push_str("# TYPE aura_battery_pct gauge\n");
    out.push_str(&format!(
        "aura_battery_pct {}\n",
        (health.battery_level * 100.0) as u8
    ));

    out.push_str("\n# HELP aura_thermal_level Thermal state level (0=Normal, 1=Warm, 2=Hot, 3=Critical, 4=Shutdown).\n");
    out.push_str("# TYPE aura_thermal_level gauge\n");
    out.push_str(&format!("aura_thermal_level {}\n", health.thermal_state));

    if let Some(celsius) = health.thermal_celsius {
        out.push_str(
            "\n# HELP aura_thermal_celsius Thermal zone temperature in degrees Celsius.\n",
        );
        out.push_str("# TYPE aura_thermal_celsius gauge\n");
        out.push_str(&format!("aura_thermal_celsius {}\n", celsius));
    }

    out.push_str("\n# HELP aura_a11y_connected Whether the Android accessibility service is connected (1=yes, 0=no).\n");
    out.push_str("# TYPE aura_a11y_connected gauge\n");
    out.push_str(&format!(
        "aura_a11y_connected {}\n",
        if health.a11y_connected { 1 } else { 0 }
    ));

    out.push_str(
        "\n# HELP aura_error_rate_last_hour Error rate over the last hour as fraction (0.0-1.0).\n",
    );
    out.push_str("# TYPE aura_error_rate_last_hour gauge\n");
    out.push_str(&format!(
        "aura_error_rate_last_hour {}\n",
        health.error_rate_last_hour
    ));

    out.push_str(
        "\n# HELP aura_overall_health_status Overall health (0=Healthy, 1=Degraded, 2=Critical).\n",
    );
    out.push_str("# TYPE aura_overall_health_status gauge\n");
    out.push_str(&format!(
        "aura_overall_health_status {}\n",
        health.overall_status
    ));

    out.push_str("\n# HELP aura_session_number Current daemon session number.\n");
    out.push_str("# TYPE aura_session_number gauge\n");
    out.push_str(&format!("aura_session_number {}\n", health.session_number));

    out.push_str(
        "\n# HELP aura_consecutive_healthy_checks Number of consecutive healthy checks.\n",
    );
    out.push_str("# TYPE aura_consecutive_healthy_checks gauge\n");
    out.push_str(&format!(
        "aura_consecutive_healthy_checks {}\n",
        health.consecutive_healthy
    ));

    out.push_str("\n# HELP aura_low_power_mode Whether low-power mode is active (1=yes, 0=no).\n");
    out.push_str("# TYPE aura_low_power_mode gauge\n");
    out.push_str(&format!(
        "aura_low_power_mode {}\n",
        if health.low_power_mode { 1 } else { 0 }
    ));

    out.push_str("\n# HELP aura_neocortex_restart_attempts Number of neocortex restart attempts this session.\n");
    out.push_str("# TYPE aura_neocortex_restart_attempts gauge\n");
    out.push_str(&format!(
        "aura_neocortex_restart_attempts {}\n",
        health.neocortex_restart_attempts
    ));

    // ── Memory System Gauges ──
    out.push_str("\n# HELP aura_working_memory_bytes Working memory usage in bytes.\n");
    out.push_str("# TYPE aura_working_memory_bytes gauge\n");
    out.push_str(&format!(
        "aura_working_memory_bytes {}\n",
        health.working_memory_bytes
    ));

    out.push_str("\n# HELP aura_working_memory_slots Number of active working memory slots.\n");
    out.push_str("# TYPE aura_working_memory_slots gauge\n");
    out.push_str(&format!(
        "aura_working_memory_slots {}\n",
        health.working_slot_count
    ));

    out.push_str("\n# HELP aura_episodic_memory_bytes Episodic memory storage in bytes.\n");
    out.push_str("# TYPE aura_episodic_memory_bytes gauge\n");
    out.push_str(&format!(
        "aura_episodic_memory_bytes {}\n",
        health.episodic_memory_bytes
    ));

    out.push_str("\n# HELP aura_episodic_memory_count Number of episodic memory entries.\n");
    out.push_str("# TYPE aura_episodic_memory_count gauge\n");
    out.push_str(&format!(
        "aura_episodic_memory_count {}\n",
        health.episodic_count
    ));

    out.push_str("\n# HELP aura_semantic_memory_bytes Semantic memory storage in bytes.\n");
    out.push_str("# TYPE aura_semantic_memory_bytes gauge\n");
    out.push_str(&format!(
        "aura_semantic_memory_bytes {}\n",
        health.semantic_memory_bytes
    ));

    out.push_str("\n# HELP aura_semantic_memory_count Number of semantic memory entries.\n");
    out.push_str("# TYPE aura_semantic_memory_count gauge\n");
    out.push_str(&format!(
        "aura_semantic_memory_count {}\n",
        health.semantic_count
    ));

    out.push_str("\n# HELP aura_archive_memory_bytes Archive memory storage in bytes.\n");
    out.push_str("# TYPE aura_archive_memory_bytes gauge\n");
    out.push_str(&format!(
        "aura_archive_memory_bytes {}\n",
        health.archive_memory_bytes
    ));

    out.push_str("\n# HELP aura_archive_memory_count Number of archive blobs.\n");
    out.push_str("# TYPE aura_archive_memory_count gauge\n");
    out.push_str(&format!(
        "aura_archive_memory_count {}\n",
        health.archive_count
    ));

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_metrics_includes_telemetry() {
        let telemetry = "aura_events_received{kind=\"counter\"} 42 1000\n";
        let health = MetricsSnapshot::default();
        let output = render_metrics(telemetry, &health);
        assert!(output.contains("aura_events_received"));
    }

    #[test]
    fn test_render_metrics_includes_health_gauges() {
        let health = MetricsSnapshot {
            neocortex_alive: true,
            battery_level: 0.75,
            thermal_state: 1,
            thermal_celsius: Some(42.5),
            error_rate_last_hour: 0.05,
            overall_status: 0,
            ..Default::default()
        };
        let output = render_metrics("", &health);
        assert!(output.contains("aura_neocortex_alive 1"));
        assert!(output.contains("aura_battery_pct 75"));
        assert!(output.contains("aura_thermal_level 1"));
        assert!(output.contains("aura_thermal_celsius 42.5"));
        assert!(output.contains("aura_error_rate_last_hour 0.05"));
        assert!(output.contains("aura_overall_health_status 0"));
    }

    #[test]
    fn test_render_metrics_includes_memory_tiers() {
        let health = MetricsSnapshot {
            working_memory_bytes: 1024,
            working_slot_count: 5,
            episodic_count: 100,
            semantic_count: 50,
            archive_count: 10,
            ..Default::default()
        };
        let output = render_metrics("", &health);
        assert!(output.contains("aura_working_memory_bytes 1024"));
        assert!(output.contains("aura_working_memory_slots 5"));
        assert!(output.contains("aura_episodic_memory_count 100"));
        assert!(output.contains("aura_semantic_memory_count 50"));
        assert!(output.contains("aura_archive_memory_count 10"));
    }

    #[test]
    fn test_render_metrics_neocortex_down() {
        let health = MetricsSnapshot {
            neocortex_alive: false,
            ..Default::default()
        };
        let output = render_metrics("", &health);
        assert!(output.contains("aura_neocortex_alive 0"));
    }

    #[test]
    fn test_render_metrics_no_thermal_celsius() {
        let health = MetricsSnapshot {
            thermal_celsius: None,
            ..Default::default()
        };
        let output = render_metrics("", &health);
        assert!(!output.contains("aura_thermal_celsius"));
    }

    #[test]
    fn test_render_metrics_prometheus_format() {
        let health = MetricsSnapshot::default();
        let output = render_metrics("", &health);
        // Verify Prometheus text format headers
        assert!(output.contains("# HELP aura_neocortex_alive"));
        assert!(output.contains("# TYPE aura_neocortex_alive gauge"));
        assert!(output.contains("# HELP aura_battery_pct"));
        assert!(output.contains("# TYPE aura_battery_pct gauge"));
    }
}
