# ⚠️ DEPRECATED — DO NOT USE IN PRODUCTION

This monitoring stack (Prometheus/Grafana/Alertmanager) was created by
agents who misunderstood AURA's identity. AURA is a PERSONAL phone app,
not enterprise SaaS. These files are kept for reference only.

## Why Deprecated

1. **Over-engineering**: Prometheus/Grafana is enterprise SaaS monitoring
2. **Wrong platform**: AURA is Termux-based, not a cloud service
3. **Already have TelemetryEngine**: AURA has built-in metrics
4. **Single-user app**: No need for complex monitoring stack

## Simpler Alternatives

- Log-based monitoring (tail logs, grep for errors)
- Health check endpoint (/health)
- Telegram alerts (send critical errors to user)

## Files in This Directory

- `prometheus/prometheus.yml` — Prometheus scrape config
- `prometheus/alerting_rules.yml` — 27 alert rules
- `alertmanager/alertmanager.yml` — Alertmanager config
- `grafana/aura-dashboard.json` — Grafana dashboard
- `src/prometheus_exporter.rs` — HTTP /metrics endpoint
- `PERFORMANCE-ANALYSIS.md` — 10 bottlenecks identified

## History

- **Created**: 2026-04-02 by Infrastructure Department
- **Deprecated**: 2026-04-02 by Founder review
- **Reason**: Misunderstood AURA's identity (personal AGI, not enterprise SaaS)
