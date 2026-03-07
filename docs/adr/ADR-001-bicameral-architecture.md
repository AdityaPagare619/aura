# ADR-001: Bicameral Architecture (System1 Daemon + System2 Neocortex)

**Status:** Accepted  
**Date:** 2026-03-01  
**Deciders:** AURA Core Team

## Context

AURA is an on-device Android automation agent. It must:

1. Respond to accessibility events and user commands in real-time (<100ms for common tasks)
2. Handle novel, multi-step tasks requiring planning and reasoning
3. Run 24/7 on battery-constrained mobile hardware
4. Minimize cloud dependency and LLM inference cost

A single-process architecture forces a choice: either always load an LLM (expensive, slow startup, battery drain) or never use one (limited to scripted actions). Neither extreme works for a general-purpose agent.

## Decision

Split the agent into two cooperating processes inspired by dual-process cognitive theory:

```
┌─────────────────────────────────────────────────┐
│                   Android OS                     │
│                                                  │
│  ┌──────────────────────┐  ┌──────────────────┐ │
│  │   DAEMON (System1)   │  │ NEOCORTEX (Sys2) │ │
│  │                      │  │                  │ │
│  │  ForegroundService   │  │  Separate Process │ │
│  │  Rust via JNI        │  │  llama.cpp        │ │
│  │  Always-on           │  │  On-demand        │ │
│  │  ETG plan cache      │  │  LLM inference    │ │
│  │  <10ms response      │  │  1-30s response   │ │
│  │  ~80% of tasks       │  │  ~20% of tasks    │ │
│  │                      │  │                  │ │
│  │  crates/aura-daemon/ │  │ crates/aura-     │ │
│  │                      │  │   neocortex/     │ │
│  └──────────┬───────────┘  └───────┬──────────┘ │
│             │    IPC (abstract      │            │
│             │◄──  Unix socket  ────►│            │
│             │    on Android)        │            │
└─────────────────────────────────────────────────┘
```

### System1 — The Daemon

- **Location:** `crates/aura-daemon/`
- **Runtime:** Rust binary loaded via JNI into an Android ForegroundService
- **Role:** Fast, deterministic execution of known tasks
- **Mechanism:** Execution Trace Graph (ETG) provides cached action plans. Plan cache holds max 256 entries with 0.70 confidence threshold (`routing/system1.rs`)
- **Execution modes** (`daemon_core/react.rs`):
  - `ExecutionMode::Dgs` — Document-Guided Scripting for template-matched tasks
  - `ExecutionMode::SemanticReact` — ReAct loop for tasks requiring observation cycles
- **Handles:** accessibility events, notifications, cron triggers, simple ack patterns

### System2 — The Neocortex

- **Location:** `crates/aura-neocortex/`
- **Runtime:** Separate OS process with `llama.cpp` via `aura-llama-sys` FFI bindings
- **Role:** Novel task planning, multi-step reasoning, conversation
- **Connection:** Abstract Unix socket IPC on Android
- **Lifecycle:** Started on-demand, can be killed to reclaim memory

### Routing

The `RouteClassifier` (`routing/classifier.rs`) examines each incoming event and assigns one of four routes:

| Route | Meaning | Example |
|-------|---------|---------|
| `System1` | ETG cache hit, execute locally | "Open WhatsApp" (seen before) |
| `System2` | Needs LLM planning | "Summarize my last 5 emails" |
| `DaemonOnly` | Internal bookkeeping | Checkpoint, cron tick |
| `Hybrid` | System1 starts, escalates if stuck | Multi-step task with partial cache |

### Strategy Escalation

The ReAct engine (`daemon_core/react.rs`) uses monotonic escalation — never downgrades:

```
Direct → Exploratory → Cautious → Recovery
```

Mid-execution escalation tiers when DGS execution encounters friction:

```
DgsSuccess → RetryAdjusted → Brainstem (0.8B model) → FullNeocortex
```

### Graceful Degradation

If the neocortex process is unreachable (crashed, not started, OOM-killed), System2 requests fall back to System1 with degraded capabilities rather than failing entirely.

## Consequences

### Positive

- **Battery efficiency:** LLM process only alive when needed. System1 handles ~80% of tasks at near-zero compute cost
- **Latency:** Common tasks execute in <10ms via ETG cache, no model loading
- **Memory isolation:** Neocortex OOM doesn't crash the daemon. Android can reclaim neocortex memory under pressure
- **Progressive learning:** Novel tasks (System2) get recorded in ETG, becoming System1 cache hits over time. The ratio shifts toward System1 as usage grows

### Negative

- **IPC complexity:** Two-process communication adds serialization overhead and failure modes (socket errors, message ordering)
- **State synchronization:** Daemon and neocortex have separate memory spaces; context must be explicitly passed via IPC messages
- **Debugging difficulty:** Traces span two processes; need correlated logging

## Alternatives Considered

### 1. Single Process with Embedded LLM
- **Rejected:** Keeping a 0.8-7B model loaded at all times on a phone wastes 500MB-4GB RAM and drains battery even when idle. Startup time balloons past 800ms budget.

### 2. Cloud-Only LLM
- **Rejected:** Adds network latency (200-2000ms), requires internet, raises privacy concerns for an on-device agent accessing personal data.

### 3. Plugin Architecture (Single Process, Dynamic Loading)
- **Rejected:** Dynamic loading of LLM libraries in a single process doesn't solve the memory problem. Android's OOM killer would take down the entire agent, not just the LLM component.

## References

- `crates/aura-daemon/src/daemon_core/main_loop.rs` — Main event loop with 8-channel `tokio::select!`
- `crates/aura-daemon/src/daemon_core/react.rs` — ReAct engine, execution modes, strategy escalation
- `crates/aura-daemon/src/routing/system1.rs` — System1 fast path, ETG plan cache
- `crates/aura-daemon/src/routing/system2.rs` — System2 slow path, IPC dispatch
- `crates/aura-daemon/src/routing/classifier.rs` — RouteClassifier
