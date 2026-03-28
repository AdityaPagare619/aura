# 00 — Executive System Architecture (Code-First)

## 1) System topology (atom-to-cosmic view)

AURA is structured as a multi-crate Rust system with Android integration and CI-driven binary distribution.

- **Core crates**: `aura-daemon`, `aura-neocortex`, `aura-llama-sys`, `aura-types`, `aura-iron-laws`.
- **Android app shell** exists in `android/` for app-side integration and ABI packaging.
- **Installer-centric deployment** for Termux uses `install.sh` as the operational entrypoint.
- **CI/CD and release surfaces** are in `.github/workflows/` and infrastructure scripts.

Codebase scale observed:

- `213` Rust source files total.
- `aura-daemon`: `182` Rust files (dominant orchestration brain/body runtime).
- `aura-neocortex`: `10` Rust files.
- `aura-types`: `17` Rust files.
- `aura-llama-sys`: `3` Rust files.

## 2) Runtime architecture and control plane

### 2.1 Daemon startup state machine (8 phases)

`aura-daemon` formalizes startup as explicit phases with phase timing telemetry:

1. JNI load
2. Runtime init
3. Database open
4. State restore
5. Subsystem init
6. IPC bind
7. Cron schedule
8. Ready

Reference: `crates/aura-daemon/src/daemon_core/startup.rs:266-370`.

This enforces deterministic boot sequencing and forms the top-level architecture contract for all subsystems.

### 2.2 Main loop execution model

`main_loop.rs` is intentionally centralized and large by design to preserve **single-writer mutable state discipline** and avoid distributed shared locking.

- Concurrency model documented directly in code comments.
- `tokio::select!` multiplexes channels and subsystem interactions.
- Policy, memory, routing, bridge, and neocortex interaction all converge in this loop.

Reference: `crates/aura-daemon/src/daemon_core/main_loop.rs:5-74`, `:93-156`, `:245-257`.

### 2.3 Cross-process inference boundary

Daemon ↔ Neocortex communication is IPC-framed and platform-aware:

- Android/Linux abstract socket: `@aura_ipc_v4`
- Host fallback TCP: `127.0.0.1:19400`
- Framing is length-prefixed with strict max message size and decode error semantics.

References:
- `crates/aura-daemon/src/ipc/protocol.rs:33-57`, `:70-119`
- `crates/aura-daemon/src/ipc/spawn.rs:144-148`
- `crates/aura-neocortex/src/main.rs:42-54`

## 3) Data contract architecture

The canonical IPC contract lives in `aura-types`:

- `PROTOCOL_VERSION = 3`
- `AuthenticatedEnvelope<T>` contains protocol version, session token, sequence, payload.
- `IpcRateLimitConfig` defines receive-side hard limits.
- `ModelParams`, `ContextPackage`, `FailureContext`, `InferenceMode` define inference exchange semantics.

Reference: `crates/aura-types/src/ipc.rs:7-65`, `:67-119`, `:362-385`.

## 4) Safety and governance architecture

`PolicyGate` implements policy decisions with first-match rules and integrated burst/rate limiting to prevent suspicious repeated actions.

- Sliding-window limiter
- bounded key tracking (`MAX_RATE_LIMITER_KEYS`)
- decision stratification (`allow`, `audit`, `confirm`, `deny` behavior)

Reference: `crates/aura-daemon/src/policy/gate.rs:1-13`, `:28-47`, `:73-117`, `:140-189`, `:195-220`.

## 5) Build and binary architecture at a glance

`aura-llama-sys/build.rs` uses target-aware conditional logic:

- Android aarch64: compile native `llama.cpp` C/C++ units and link static C++ runtime.
- host/non-android: use stub cfg path.
- Android runtime link search paths and C++ static linkage are emitted explicitly.

Reference: `crates/aura-llama-sys/build.rs:12-21`, `:38-73`, `:83-87`, `:90-183`.

## 6) CI architecture and branch confidence anchor

Mainline green run used as operational confidence anchor:

- Run `23677762816` (`CI Pipeline v2`) completed success across Check/Test/Clippy/Format/Security/Build Android.
- Corresponding head SHA: `e662aed4a98b395c8b642431f6f1cbe52eaf3979`.

Evidence source: GitHub Actions run/job metadata for run `23677762816`.

## 7) System-wide architecture conclusions

1. AURA is architected as a **runtime-orchestrated, policy-gated, IPC-separated intelligence system** with explicit boot phases.
2. Complexity is concentrated in daemon orchestration rather than llama wrapper alone.
3. Operational success depends on alignment across installer, CI artifacts, IPC endpoints, and feature gating.
4. Android behavior cannot be reasoned about from llama.cpp alone; binary lifecycle + runtime gating are equally critical.
