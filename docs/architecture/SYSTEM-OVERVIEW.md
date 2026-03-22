# AURA v4 System Overview

**Document**: `docs/architecture/SYSTEM-OVERVIEW.md`  
**Version**: 4.0.0-stable  
**Date**: 2026-03-22  
**Status**: ACTIVE  
**Owner**: Documentation Charter

---

## 1. Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              AURA v4.0 ARCHITECTURE                          │
└─────────────────────────────────────────────────────────────────────────────┘

                              ┌─────────────────────┐
                              │   Android Device    │
                              │   ┌─────────────┐   │
                              │   │   Shell App  │   │  PID: App Process
                              │   │  (Kotlin +   │   │  • UI Layer
                              │   │  Accessibility)  │  • AccessibilityService
                              │   └──────┬──────┘   │  • JNI Bridge
                              └──────────┼──────────┘
                                         │ JNI
                              ┌──────────▼──────────┐
                              │   aura-daemon       │  PID: 1 (Main)
                              │   ┌─────────────┐   │
                              │   │  Event Loop │   │  • Tokio async runtime
                              │   │  (Main Loop) │   │  • 7 input channels
                              │   └──────┬──────┘   │
                              │          │           │  ┌──────────────────┐
                              │   ┌──────▼──────┐   │  │ Subsystems       │
                              │   │  Routing     │   │  │ • Memory System  │
                              │   │  System1/2  │   │  │ • Identity       │
                              │   └──────┬──────┘   │  │ • Policy Gate    │
                              │          │           │  │ • Execution      │
                              │   ┌──────▼──────┐   │  │ • BDI Goals      │
                              │   │ OutcomeBus  │   │  │ • ARC System     │
                              │   │  (5 subs)   │   │  │ • Neocortex IPC  │
                              │   └─────────────┘   │  └──────────────────┘
                              └──────────┬──────────┘
                                         │ Unix Socket
                              ┌──────────▼──────────┐
                              │  aura-neocortex     │  PID: 2 (LLM)
                              │  ┌─────────────┐   │
                              │  │  IPC Server │   │  • llama.cpp process
                              │  │  (TCP Sync) │   │  • LLM inference
                              │  └──────┬──────┘   │
                              │          │           │  • FFI via aura-llama-sys
                              │   ┌──────▼──────┐   │
                              │   │   llama.cpp │   │
                              │   │  (4-bit Q4) │   │
                              │   └─────────────┘   │
                              └─────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│                            EXTERNAL INTERFACES                               │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌─────────────┐     ┌─────────────┐     ┌─────────────┐                   │
│  │  Telegram   │     │   System    │     │  Developer  │                   │
│  │   Bot API   │     │  Apps UI    │     │    CLI      │                   │
│  │  (MTProto)  │     │ (A11ySvc)   │     │  (debug)    │                   │
│  └──────┬──────┘     └──────┬──────┘     └──────┬──────┘                   │
│         │                    │                   │                           │
│         │ HTTPS              │ A11y Events       │ Shell commands            │
│         ▼                    ▼                   ▼                           │
│  ┌─────────────────────────────────────────────────────────────────┐        │
│  │                    Android OS Layer                              │        │
│  │  ┌───────────┐  ┌───────────┐  ┌───────────┐  ┌───────────┐    │        │
│  │  │  Network  │  │  Screen   │  │  Sensors  │  │   File    │    │        │
│  │  │  Stack    │  │  Capture  │  │  (GPS,    │  │   System  │    │        │
│  │  │           │  │           │  │   Battry) │  │  ($PREFIX)│    │        │
│  │  └───────────┘  └───────────┘  └───────────┘  └───────────┘    │        │
│  └─────────────────────────────────────────────────────────────────┘        │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Core Components

### 2.1 Process Architecture

| Process | PID | Language | Purpose | Restart Policy |
|---------|-----|----------|---------|---------------|
| `aura-daemon` | 1 | Rust | Main event loop, all subsystems | Persistent |
| `aura-neocortex` | 2 | C/Rust | LLM inference via llama.cpp | Killable (LMK-safe) |

**Isolation Principle**: If the LLM process is killed by Android's Low Memory Killer, the daemon survives and can reconnect to a new neocortex instance.

### 2.2 Daemon Subsystems

```
aura-daemon
├── memory/          Memory hierarchy (14 modules)
│   ├── working/     RAM ring buffer (1024 slots)
│   ├── episodic/    SQLite-backed event store
│   ├── semantic/     FTS5 indexed facts
│   ├── archive/     ZSTD compressed cold storage
│   ├── hnsw/        Approximate nearest neighbor index
│   └── embeddings/  Vector embeddings
│
├── identity/        User profile & ethics (12 modules)
│   ├── ocean/       Big Five personality traits
│   ├── vad/         Valence-Arousal-Dominance
│   ├── ethics/      15 hardcoded ethics rules
│   └── relationship/ Interaction history
│
├── policy/          Security & boundaries (8 modules)
│   ├── rules/       Deny-by-default allowlist
│   ├── sandbox/     Execution isolation
│   └── audit/       Action logging
│
├── execution/       Action system
│   ├── executor/    ReAct loop execution
│   ├── planner/     HTN goal decomposition
│   ├── etg/         Learned template cache
│   └── monitor/     Retry & health tracking
│
├── goals/           BDI agent model
│   ├── registry/    Goal definitions
│   └── scheduler/  Intent selection
│
├── arc/             Adaptive Resource Controller
│   ├── health/      System health monitoring
│   ├── learning/    Usage pattern learning
│   └── proactive/   Proactive behavior engine
│
└── platform/        Device integration
    ├── sensors/     Battery, GPS, network state
    └── a11y/       AccessibilityService bridge
```

---

## 3. Data Flow

### 3.1 Request Processing (Telegram → LLM → Action)

```
Telegram Message
       │
       ▼
┌──────────────────┐
│ 1. JNI Receive   │  Shell app receives via MTProto
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│ 2. Event Loop    │  tokio::select! over 7 channels
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│ 3. Routing       │  System1 (ETG cache) or System2 (LLM)
└────────┬─────────┘
         │
    ┌────┴────┐
    │         │
    ▼         ▼
System1    System2
  │           │
  │           ▼
  │    ┌──────────────┐
  │    │ IPC to       │
  │    │ aura-neocortex│
  │    └───────┬──────┘
  │            │ Unix Socket (bincode)
  │            ▼
  │    ┌──────────────┐
  │    │ llama.cpp    │
  │    │ Inference    │
  │    └───────┬──────┘
  │            │
  │            ▼
  │    ┌──────────────┐
  │    │ ReAct Decision│
  │    │ or Reply     │
  │    └──────────────┘
  │
  ▼
┌──────────────────┐
│ 4. Execution     │  Action via AccessibilityService
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│ 5. OutcomeBus    │  Publish to 5 subscribers
└────────┬─────────┘
         │
    ┌────┼────┬────┬────┐
    ▼    ▼    ▼    ▼    ▼
  Memory Goals ARC Audit Profile
```

### 3.2 Persistence Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    AURA Data Directory                   │
│                    ~/.aura/                             │
├─────────────────────────────────────────────────────────┤
│                                                         │
│  ┌─────────────────┐  ┌─────────────────┐             │
│  │   SQLite DB     │  │  Checkpoints/    │             │
│  │   aura.db       │  │  bincode files  │             │
│  ├─────────────────┤  ├─────────────────┤             │
│  │ • episodes      │  │ • daemon.state  │             │
│  │ • semantic_facts│  │ • etg_graph.bin │             │
│  │ • etg_nodes     │  │ • hnsw.bin      │             │
│  │ • etg_edges     │  │ • patterns.bin  │             │
│  │ • policy_rules  │  │ • identity.bin  │             │
│  │ • checkpoints   │  │                 │             │
│  │ • temporal_pat  │  │                 │             │
│  └─────────────────┘  └─────────────────┘             │
│                                                         │
│  ┌─────────────────┐  ┌─────────────────┐             │
│  │   Logs/         │  │  Models/        │             │
│  │   text files    │  │  (user-managed) │             │
│  ├─────────────────┤  ├─────────────────┤             │
│  │ • boot.log      │  │ • llama-model   │             │
│  │ • crash-DATE.txt│  │   (downloaded)  │             │
│  │ • audit.log     │  │                 │             │
│  └─────────────────┘  └─────────────────┘             │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

---

## 4. Communication Protocols

### 4.1 JNI Bridge (App → Daemon)

| Method | Direction | Purpose |
|--------|-----------|---------|
| `startDaemon()` | Java → Rust | Start aura-daemon service |
| `sendMessage()` | Java → Rust | Queue Telegram message for processing |
| `getStatus()` | Java → Rust | Query daemon health |
| `onAccessibilityEvent()` | Java → Rust | Forward screen events |

### 4.2 IPC Protocol (Daemon → Neocortex)

| Field | Type | Description |
|-------|------|-------------|
| `version` | u32 | Protocol version (current: 1) |
| `kind` | enum | InferenceRequest, HealthCheck, Shutdown |
| `payload` | Vec<u8> | Bincode-encoded request |
| `timeout_ms` | u64 | Request timeout |

**Transport**: Unix domain socket on Android, TCP localhost on host testing.

---

## 5. Security Boundaries

```
┌─────────────────────────────────────────────────────────┐
│                    TRUST BOUNDARY                        │
├─────────────────────────────────────────────────────────┤
│                                                          │
│   USER DATA          AURA CODE         SYSTEM            │
│   (on-device)        (Rust binary)      (Android OS)    │
│                                                          │
│  ┌──────────┐     ┌──────────────┐    ┌──────────┐     │
│  │ SQLite   │     │   Memory     │    │ Network  │     │
│  │ Memory   │────▶│   Sandbox    │───▶│ Stack    │     │
│  │ Store    │     │              │    │          │     │
│  └──────────┘     └──────────────┘    └──────────┘     │
│        │                 │                  │           │
│        │                 ▼                  │           │
│        │          ┌──────────────┐          │           │
│        │          │   Policy     │          │           │
│        │          │   Gate       │          │           │
│        │          │ (Deny-by-def)│          │           │
│        │          └──────────────┘          │           │
│        │                 │                  │           │
│        ▼                 ▼                  ▼           │
│  ┌──────────────────────────────────────────────┐       │
│  │         NO NETWORK EXFILTRATION              │       │
│  │    (Zero telemetry, zero cloud fallback)     │       │
│  └──────────────────────────────────────────────┘       │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

---

## 6. Failure Domains

| Domain | Component | Failure Mode | Recovery |
|--------|-----------|--------------|----------|
| Runtime | aura-daemon | Crash, OOM | Android restarts service |
| Inference | aura-neocortex | LMK kill, OOM | Daemon spawns new instance |
| Persistence | SQLite | Corruption | Restore from checkpoint |
| Network | Telegram | Disconnect | Automatic reconnect with backoff |
| Policy | Allowlist miss | False positive | Update allowlist, rebuild |

---

## 7. Related Documents

| Document | Purpose |
|----------|---------|
| `docs/architecture/AURA-V4-SYSTEM-ARCHITECTURE.md` | Detailed architecture reference |
| `docs/build/CONTRACT.md` | Platform contract specification |
| `docs/build/FAILURE_TAXONOMY.md` | Failure classification F001-F015 |
| `docs/runtime/BOOT-STAGES.md` | Startup sequence documentation |
| `docs/architecture/AURA-V4-MEMORY-AND-DATA-ARCHITECTURE.md` | Memory hierarchy details |

---

**END OF DOCUMENT**
