// =============================================================================
// AURA v4 Monitoring Module
// =============================================================================
// Prometheus exporter + health snapshot bridge for external monitoring.
//
// This module provides:
// - PrometheusExporter: HTTP server exposing /metrics in Prometheus text format
// - MetricsSnapshot: Lightweight health data bridge between heartbeat → exporter
// - render_metrics(): Combines TelemetryEngine output with health gauges
//
// Integration:
// 1. Start PrometheusExporter in daemon_core::startup
// 2. Update MetricsSnapshot from run_heartbeat_loop every tick
// 3. Update telemetry_prometheus from TelemetryEngine::export_prometheus()
// 4. Prometheus scrapes GET /metrics → full exposition

pub mod prometheus_exporter;

pub use prometheus_exporter::{MetricsSnapshot, PrometheusExporter, DEFAULT_METRICS_PORT};
