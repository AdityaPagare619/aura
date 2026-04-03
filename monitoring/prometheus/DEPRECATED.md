# ⚠️ DEPRECATED — Prometheus Configuration

## Why Deprecated

Prometheus is an enterprise-grade time-series database designed for monitoring
cloud infrastructure with thousands of targets. AURA is a **personal, local AGI**
running on a single Android phone via Termux. Running Prometheus on-device would:

1. **Consume excessive RAM** — Prometheus needs ~200-500MB just idle, which is
   more than AURA's entire RSS ceiling (30MB)
2. **Require persistent storage** — TSDB writes add I/O overhead on flash storage
3. **Serve no purpose** — AURA already has TelemetryEngine for self-monitoring
4. **Break the identity** — Personal AGI ≠ enterprise SaaS

## What to Use Instead

- **TelemetryEngine** — AURA's built-in metrics system (lightweight, in-process)
- **Health check script** — `scripts/health_check.sh` outputs JSON status
- **Telegram alerts** — Critical errors sent directly to the user
- **Log tailing** — `tail -f /data/local/tmp/aura/logs/aura.log | grep ERROR`

## History

- **Created**: 2026-04-02 by Infrastructure Department
- **Deprecated**: 2026-04-03 by Architecture department
- **Reason**: Over-engineered for a single-user, on-device application
