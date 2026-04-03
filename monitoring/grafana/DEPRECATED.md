# ⚠️ DEPRECATED — Grafana Dashboard

## Why Deprecated

Grafana is a visualization platform for enterprise monitoring stacks. It requires:
- A running Grafana server (~100MB+ RAM)
- A Prometheus data source (also deprecated)
- Network access and port exposure

AURA is a **personal, local AGI** that runs entirely on-device. There is no
dashboard server, no Grafana instance, and no need for one.

## What to Use Instead

- **TelemetryEngine** — AURA's built-in metrics, queryable programmatically
- **Health check script** — `scripts/health_check.sh` — outputs JSON status
- **Telegram** — The primary interface for status and alerts
- **Log files** — Human-readable logs in `/data/local/tmp/aura/logs/`

## History

- **Created**: 2026-04-02 by Infrastructure Department
- **Deprecated**: 2026-04-03 by Architecture department
- **Reason**: Grafana is enterprise visualization — AURA is a personal phone app
