# 02 — Memory, Data Structures, and Analytics Surfaces

## 1) Canonical data contracts

`aura-types` is the contract center for inter-crate and inter-process data.

### 1.1 Envelope + protocol controls

- `AuthenticatedEnvelope<T>` embeds `protocol_version`, `session_token`, `seq`, and payload.
- `PROTOCOL_VERSION` is explicitly maintained (`3`) and version checks are first-class.

Reference: `crates/aura-types/src/ipc.rs:7-65`.

### 1.2 IPC safety controls

- `IpcRateLimitConfig` defaults: `100` req/s with `20` burst.

Reference: `crates/aura-types/src/ipc.rs:67-87`.

### 1.3 Model + context transfer types

- `ModelParams` includes context length, thread count, and model tier.
- `ContextPackage` and `FailureContext` drive richer inferencing context and recovery semantics.

Reference: `crates/aura-types/src/ipc.rs:103-119`, `:362-385`.

## 2) Memory pressure and thermal data model

Power/memory modeling is explicit and device-oriented.

### 2.1 Model memory estimates

`ModelMemoryEstimate` encodes model-family footprints and computes:

- `kv_cache_bytes(context_len)`
- runtime overhead
- total memory estimate

Reference: `crates/aura-types/src/power.rs:531-607`.

This design directly encodes the KV cache scaling concern (context-dependent memory growth), making it measurable at planning/config level.

### 2.2 Thermal and memory pressure semantics

`ThermalState`, `MemoryPressure`, and trim-memory mappings provide policy-friendly abstractions from raw platform signals.

Reference anchors:
- `crates/aura-types/src/power.rs:81-113`
- `:426-437`
- `:632` (Android-level mapping entrypoint)

## 3) Runtime boundedness patterns

The runtime uses explicit caps to avoid unbounded growth:

- IPC message max size `256 KB`
- screen cache entry and byte caps in main loop constants
- policy rate limiter key cap `256`

References:
- `crates/aura-daemon/src/ipc/protocol.rs:47-57`
- `crates/aura-daemon/src/daemon_core/main_loop.rs:176-193`
- `crates/aura-daemon/src/policy/gate.rs:28-31`

## 4) Local analytics/observability surfaces

Codebase exposes telemetry modules and counter/ring components under daemon telemetry package:

- `crates/aura-daemon/src/telemetry/counters.rs`
- `crates/aura-daemon/src/telemetry/ring.rs`
- `crates/aura-daemon/src/telemetry/mod.rs`

These indicate local metric collection surfaces suitable for operational introspection without cloud dependency.

## 5) Data lifecycle architecture (high-level)

From startup and main-loop wiring:

1. config and persisted state loaded at startup (`db`, checkpoint).
2. incoming events are parsed/scored/contextualized.
3. IPC envelopes transmit bounded context to neocortex.
4. outputs return via typed IPC and are fed into reaction/memory pathways.
5. periodic cron checkpoints persist state.

References:
- `crates/aura-daemon/src/daemon_core/startup.rs:298-347`
- `crates/aura-daemon/src/daemon_core/main_loop.rs:56-65`

## 6) Architecture assessment

Strengths:

- Contract-first types package with explicit versioning.
- Memory/thermal models are first-class and test-covered in types crate.
- Widespread boundedness constants align with mobile constraints.

Primary caution:

- Accurate runtime memory control requires end-to-end use of `ModelParams.n_ctx` and model loading parameters across orchestrator and neocortex paths; operational docs should keep this mapping explicit.
