# OPERATIONAL FLOW (Code-First)

## 1) Install and bootstrap flow

Primary installer:

- `/home/runner/work/aura/aura/install.sh` (1866 lines)

Operationally, installer covers:

- environment checks,
- dependency and toolchain prep,
- model and binary setup,
- config generation,
- service/session startup wiring.

## 2) Daemon startup flow

Entrypoints:

- CLI path: `/home/runner/work/aura/aura/crates/aura-daemon/src/bin/main.rs`
- Startup orchestrator: `/home/runner/work/aura/aura/crates/aura-daemon/src/daemon_core/startup.rs`

Flow:

1. Load config
2. Initialize runtime/logging
3. Execute 8-phase startup
4. Enter main loop
5. Spawn and manage bridge/background operational channels

## 3) Command and request flow

Command/request path is centralized in main loop handlers and routing stack:

- parse -> classify -> policy gate -> route (System1/System2) -> execute/react -> publish outcome

Core files:

- `/home/runner/work/aura/aura/crates/aura-daemon/src/daemon_core/main_loop.rs`
- `/home/runner/work/aura/aura/crates/aura-daemon/src/routing/*`
- `/home/runner/work/aura/aura/crates/aura-daemon/src/reaction/*`
- `/home/runner/work/aura/aura/crates/aura-daemon/src/execution/*`

## 4) IPC and neocortex inference flow

IPC transport and protocol constants:

- Android: abstract Unix socket `@aura_ipc_v4`
- non-Android fallback: `127.0.0.1:19400`

References:

- `/home/runner/work/aura/aura/crates/aura-daemon/src/ipc/protocol.rs:38-45`
- `/home/runner/work/aura/aura/crates/aura-daemon/src/ipc/spawn.rs`

## 5) Health/degradation/circuit-breaker operational flow

Operational reliability modules reside in daemon subsystems and are wired in startup/main loop.

Review surfaces:

- `/home/runner/work/aura/aura/crates/aura-daemon/src/health/*`
- `/home/runner/work/aura/aura/crates/aura-daemon/src/arc/*`
- `/home/runner/work/aura/aura/crates/aura-daemon/src/policy/*`
- `/home/runner/work/aura/aura/crates/aura-daemon/src/daemon_core/main_loop.rs`

## 6) Shutdown flow

Shutdown path and report surfaces:

- `/home/runner/work/aura/aura/crates/aura-daemon/src/daemon_core/shutdown.rs`
- Re-exported as `graceful_shutdown` from `crates/aura-daemon/src/lib.rs`
