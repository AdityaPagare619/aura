# ⚠️ DEPRECATED — Alertmanager Configuration

## Why Deprecated

Alertmanager is part of the Prometheus ecosystem for routing, deduplicating,
and silencing alerts across large-scale infrastructure. It assumes:
- Multiple alert sources (Prometheus, custom exporters, etc.)
- Multiple notification channels (PagerDuty, Slack, email, etc.)
- Alert grouping and inhibition rules for team operations

AURA is a **single-user, single-device** application. It doesn't need:
- Alert routing — there's only one user
- Deduplication — there's only one alert source
- Silencing rules — there's no on-call rotation

## What to Use Instead

- **Telegram alerts** — Critical errors sent directly to the user's Telegram
- **Health check script** — `scripts/health_check.sh` — simple JSON health status
- **TelemetryEngine** — Built-in metrics with configurable thresholds
- **Log monitoring** — `tail` + `grep` for error patterns

## History

- **Created**: 2026-04-02 by Infrastructure Department
- **Deprecated**: 2026-04-03 by Architecture department
- **Reason**: Enterprise alert routing is overkill for a personal AGI
