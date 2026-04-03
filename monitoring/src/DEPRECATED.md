# ⚠️ DEPRECATED — Prometheus Exporter (Rust)

## Why Deprecated

This module exports AURA metrics in Prometheus format via an HTTP `/metrics`
endpoint. It was designed for enterprise monitoring stacks.

AURA is a **personal, local AGI** — not a cloud service. Running an HTTP
metrics endpoint:
1. Exposes internal state unnecessarily
2. Consumes RAM and battery for a feature no one will use
3. Conflicts with the "everything stays on device" principle
4. Requires network binding on a personal phone

## What to Use Instead

- **TelemetryEngine** — AURA's built-in, in-process metrics system
- **Health check script** — `scripts/health_check.sh` — outputs JSON to stdout
- **`aura status` CLI** — Direct status queries without HTTP overhead

## Code Status

This module is kept for reference only. Do not import or use it in new code.
Any `#[cfg(feature = "prometheus")]` gates should default to disabled.

## History

- **Created**: 2026-04-02 by Infrastructure Department
- **Deprecated**: 2026-04-03 by Architecture department
- **Reason**: HTTP metrics endpoint is unnecessary for a local, single-user app
