//! AURA v4 Monitoring — Prometheus exporter for daemon telemetry.
//!
//! This crate provides the HTTP metrics endpoint that bridges AURA's internal
//! TelemetryEngine (ring buffer + atomic counters) to Prometheus scrape format.
//!
//! # Quick Start
//!
//! ```bash
//! # Start Prometheus scraping AURA
//! prometheus --config.file=monitoring/prometheus/prometheus.yml
//!
//! # Import Grafana dashboard
//! # Upload monitoring/grafana/aura-dashboard.json via Grafana UI
//! ```

pub mod prometheus_exporter;

pub use prometheus_exporter::{MetricsSnapshot, PrometheusExporter, DEFAULT_METRICS_PORT};
