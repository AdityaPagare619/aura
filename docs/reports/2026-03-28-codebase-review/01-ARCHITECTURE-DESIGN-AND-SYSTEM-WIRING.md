# AURA Architecture Design and System Wiring (Code-First)

## 1) Top-level architecture

AURA is organized as a Rust workspace with clear core crates:

- `aura-daemon`: long-running orchestration runtime.
- `aura-neocortex`: separate inference process.
- `aura-llama-sys`: native llama.cpp bindings and stub/native switch points.
- `aura-types`: shared protocol/config/error/type surfaces.
- `aura-iron-laws`: ethics constraints and policy primitives.

Reference files:

- `/home/runner/work/aura/aura/Cargo.toml`
- `/home/runner/work/aura/aura/crates/aura-daemon/src/lib.rs`

## 2) Daemon subsystem map

The daemon exposes broad subsystem modules in one crate-level surface:

- `arc`, `bridge`, `daemon_core`, `execution`, `extensions`, `goals`, `health`, `identity`, `ipc`, `memory`, `outcome_bus`, `persistence`, `pipeline`, `platform`, `policy`, `reaction`, `routing`, `screen`, `telegram`, `telemetry`, and optional `voice`.

Reference:

- `/home/runner/work/aura/aura/crates/aura-daemon/src/lib.rs:40-62`

## 3) Startup architecture (8 phases)

`startup(config)` implements phased boot with explicit timing budgets:

1. JNI load
2. Runtime init
3. Database open
4. State restore
5. Subsystem init
6. IPC bind
7. Cron schedule
8. Ready

Reference:

- `/home/runner/work/aura/aura/crates/aura-daemon/src/daemon_core/startup.rs:266+`
- Phase functions: `phase_jni_load`, `phase_runtime_init`, `phase_database_open`, `phase_state_restore`, `phase_subsystems_init`, `phase_ipc_bind`, `phase_cron_schedule`, `phase_onboarding_check`

## 4) Runtime architecture

Runtime control converges into one core loop:

- `pub async fn run(mut state: DaemonState)` with `tokio::select!` multiplexing channels/ticks.

Reference:

- `/home/runner/work/aura/aura/crates/aura-daemon/src/daemon_core/main_loop.rs:1493`
- `/home/runner/work/aura/aura/crates/aura-daemon/src/daemon_core/main_loop.rs:2133`

## 5) Process split and wiring

AURA uses daemon + neocortex separation for resilience and workload isolation:

- Daemon orchestrates policy/routing/execution/state.
- Neocortex handles model/inference path.
- IPC boundary handled through typed protocol and platform-specific transport.

Reference:

- `/home/runner/work/aura/aura/crates/aura-daemon/src/ipc/protocol.rs`
- `/home/runner/work/aura/aura/crates/aura-daemon/src/ipc/spawn.rs`
- `/home/runner/work/aura/aura/crates/aura-neocortex/src/main.rs`

## 6) Policy and ethics location

System governance and ethics controls are not ad hoc; they are represented in dedicated modules/crates:

- `crates/aura-daemon/src/policy/*`
- `crates/aura-iron-laws/src/*`

This is an explicit architecture boundary between decision execution and allowed action space.
