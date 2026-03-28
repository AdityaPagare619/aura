# Memory, Data Structures, and Data Analytics (Code-First)

## 1) Memory architecture surfaces

Core memory container and tiers:

- `AuraMemory`: `/home/runner/work/aura/aura/crates/aura-daemon/src/memory/mod.rs`
- `WorkingMemory`: `/home/runner/work/aura/aura/crates/aura-daemon/src/memory/working.rs`
- `EpisodicMemory`: `/home/runner/work/aura/aura/crates/aura-daemon/src/memory/episodic.rs`
- `SemanticMemory`: `/home/runner/work/aura/aura/crates/aura-daemon/src/memory/semantic.rs`
- `ArchiveMemory`: `/home/runner/work/aura/aura/crates/aura-daemon/src/memory/archive.rs`

## 2) Primary state/data structures

Daemon runtime state:

- `DaemonState` in startup core.
- loop-managed runtime context in `main_loop.rs`.

Shared protocol/config/errors:

- `/home/runner/work/aura/aura/crates/aura-types/src/*`

## 3) Persistence and queueing

Persistence stack:

- `/home/runner/work/aura/aura/crates/aura-daemon/src/persistence/*`

Messaging and queue surfaces:

- `/home/runner/work/aura/aura/crates/aura-daemon/src/telegram/queue.rs`
- `/home/runner/work/aura/aura/crates/aura-daemon/src/daemon_core/channels.rs`

## 4) Analytics and telemetry paths

Telemetry/events/health surfaces:

- `/home/runner/work/aura/aura/crates/aura-daemon/src/telemetry/*`
- `/home/runner/work/aura/aura/crates/aura-daemon/src/health/*`
- `/home/runner/work/aura/aura/crates/aura-daemon/src/outcome_bus/*`

## 5) Android memory risk relevance

Android-relevant memory pressure touchpoints identified in code paths:

- model/inference boundaries (`aura-neocortex` + `aura-llama-sys`),
- SQLite mmap/WAL config at startup,
- channel and queue capacities in daemon runtime,
- build/runtime split between stub and native LLM backend paths.

References:

- `/home/runner/work/aura/aura/crates/aura-daemon/src/daemon_core/startup.rs`
- `/home/runner/work/aura/aura/crates/aura-daemon/src/ipc/protocol.rs`
- `/home/runner/work/aura/aura/crates/aura-llama-sys/build.rs`
