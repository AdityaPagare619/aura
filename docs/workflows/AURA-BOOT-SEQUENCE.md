# AURA Boot Sequence

Complete startup trace from Android process creation to ready state.

## Overview

**File:** `crates/aura-daemon/src/daemon_core/startup.rs`

AURA boots in 8 sequential phases with a total budget of **<800ms**. Each phase is individually timed. The sequence distinguishes critical subsystems (failure = abort) from non-critical subsystems (failure = degraded mode).

## Phase Timeline

```
 0ms                                                              800ms
  ├────┬──┬────────┬───┬──────────┬──┬──┬─┤
  │ P1 │P2│  P3    │P4 │   P5     │P6│P7│8│
  │JNI │RT│  DB    │CP │  Subs    │IP│Cr│R│
  │    │  │        │   │          │C │on│ │
  │100 │20│ 150    │50 │  200     │10│20│5│
  └────┴──┴────────┴───┴──────────┴──┴──┴─┘
                                     Total: <555ms typical
```

## Phase Details

### Phase 1: JNI Load (<100ms)

```
Android ForegroundService.onCreate()
    │
    ▼
System.loadLibrary("aura_daemon")    // Load Rust .so via JNI
    │
    ▼
JNI_OnLoad() → Rust entry point
    │
    ▼
Validate JNI environment version
Store JavaVM pointer for callbacks
```

**Failure mode:** Fatal. If the native library can't load, the service crashes. Android will show a "AURA stopped" dialog.

### Phase 2: Tokio Runtime (<20ms)

```
Build tokio multi-thread runtime
    │
    ├── Worker threads: min(CPU cores, 4)
    ├── Blocking pool: default
    └── Enable IO + timer drivers
```

**Failure mode:** Fatal. No async runtime = no event processing possible.

### Phase 3: Database Open (<150ms)

```
Open SQLite database
    │
    ├── WAL mode (Write-Ahead Logging)
    │   └── Enables concurrent reads during writes
    │
    ├── mmap size: 4MB
    │   └── Memory-mapped I/O for read performance
    │
    ├── Create tables if first run
    │   ├── etg_nodes, etg_edges
    │   ├── episodes, semantic_facts
    │   ├── patterns, temporal_patterns
    │   ├── policy_rules
    │   └── checkpoints
    │
    └── Set pragmas: journal_size_limit, cache_size, synchronous=NORMAL
```

**Failure mode:** Fatal. Database is the persistence backbone for ETG, memory, and policy.

### Phase 4: Checkpoint Restore (<50ms)

```
Load last DaemonState checkpoint
    │
    ├── Restore ETG graph (in-memory from SQLite)
    │   ├── Load nodes (up to 10,000)
    │   └── Load edges (up to 50,000)
    │
    ├── Restore HNSW index (binary deserialization)
    │
    ├── Restore working memory (last session context)
    │
    └── Restore pattern engine state
```

**Failure mode:** Non-fatal. If checkpoint is corrupted or missing, start with empty state. Log warning.

### Phase 5: Subsystem Init (<200ms)

This is the most complex phase. Subsystems are initialized in dependency order and categorized as critical or non-critical.

```
SubSystems struct initialization
    │
    ├── CRITICAL (failure = abort startup)
    │   ├── Memory system
    │   │   ├── Working tier (RAM ring, 1024 slots)
    │   │   ├── Episodic tier (SQLite handle)
    │   │   ├── Semantic tier (FTS5 index)
    │   │   └── Archive tier (ZSTD codec)
    │   │
    │   ├── Identity system
    │   │   ├── Load user identity profile
    │   │   └── Initialize ethics gate (blocked_patterns, audit_keywords)
    │   │
    │   └── Executor
    │       ├── ReAct engine
    │       ├── Selector cascade (L0-L7)
    │       └── AccessibilityService binding
    │
    ├── NON-CRITICAL (failure = degraded mode, set to None)
    │   ├── Pipeline (chat processing pipeline)
    │   ├── Routing (RouteClassifier, System1/System2 dispatchers)
    │   ├── Goals (long-term goal tracking)
    │   ├── Platform (device info, sensors)
    │   ├── IPC (neocortex connection)
    │   └── ARC (Adaptive Resource Controller)
    │
    └── Each non-critical subsystem: try init, on error → log + set to None
```

**Degraded mode behavior:**
- No Pipeline → can't process chat commands, only accessibility events
- No Routing → all tasks go to System1 default path
- No IPC → System2 unavailable, System1-only operation
- No Goals → no proactive task scheduling
- No ARC → no adaptive resource management, use fixed defaults

### Phase 6: IPC Bind (<10ms)

```
Bind abstract Unix socket for neocortex communication
    │
    ├── Socket name: "@aura-ipc" (abstract namespace, Android-specific)
    ├── Non-blocking mode
    └── Register with tokio runtime
```

**Failure mode:** Non-critical. If IPC fails, System2 is unavailable. Daemon operates in System1-only mode.

### Phase 7: Cron Schedule (<20ms)

```
Register scheduled tasks
    │
    ├── Consolidation timers
    │   ├── Light consolidation: periodic (e.g., every 15 min)
    │   └── Deep consolidation: on idle + charging
    │
    ├── ETG maintenance
    │   ├── Edge pruning (reliability < 0.3)
    │   └── LRU eviction check
    │
    ├── Checkpoint save (periodic state persistence)
    │
    └── User-defined scheduled automations
```

**Failure mode:** Non-critical. Without cron, no proactive tasks. Reactive operation continues normally.

### Phase 8: Ready (<5ms)

```
Set DaemonState = Running
    │
    ▼
Post-startup checks
    │
    ├── Onboarding status check:
    │   ├── FirstRun → trigger onboarding flow
    │   ├── Interrupted → resume onboarding
    │   └── Completed → normal operation
    │
    ▼
Enter main_loop() ──► tokio::select! over 8 channels
```

## Error Handling Summary

| Phase | On Failure | Recovery |
|-------|-----------|----------|
| P1: JNI | Abort | Android restarts service (if persistent) |
| P2: Runtime | Abort | Same as P1 |
| P3: Database | Abort | Same as P1 |
| P4: Checkpoint | Continue (empty state) | Rebuilds state from scratch over time |
| P5: Critical sub | Abort | Same as P1 |
| P5: Non-crit sub | Degraded (None) | Feature unavailable until restart |
| P6: IPC | Degraded | System1-only operation |
| P7: Cron | Degraded | No proactive tasks |
| P8: Ready | N/A | Always succeeds |

## Startup Metrics

The startup sequence logs timing for each phase. Typical cold boot on a mid-range device (Snapdragon 6-series):

| Phase | Budget | Typical |
|-------|--------|---------|
| P1: JNI Load | <100ms | 40-60ms |
| P2: Tokio | <20ms | 5-10ms |
| P3: DB Open | <150ms | 80-120ms |
| P4: Checkpoint | <50ms | 10-30ms |
| P5: Subsystems | <200ms | 100-150ms |
| P6: IPC | <10ms | 2-5ms |
| P7: Cron | <20ms | 5-10ms |
| P8: Ready | <5ms | <2ms |
| **Total** | **<555ms** | **250-390ms** |

## References

- `crates/aura-daemon/src/daemon_core/startup.rs` — Full boot sequence, phase timing, subsystem init
- `crates/aura-daemon/src/daemon_core/main_loop.rs` — Main loop entered after boot
- `crates/aura-daemon/src/execution/etg.rs` — ETG restore from SQLite
- `crates/aura-daemon/src/memory/hnsw.rs` — HNSW binary deserialization
