# AURA v4 — Architecture Documentation

Comprehensive architectural overview of the AURA on-device AI assistant system.

---

## Table of Contents

- [System Overview](#system-overview)
- [5-Crate Structure](#5-crate-structure)
- [Platform Abstraction Layer](#platform-abstraction-layer)
- [IPC Protocol](#ipc-protocol)
- [Memory System](#memory-system)
- [Ethics Layer](#ethics-layer)
- [Cognitive Architecture](#cognitive-architecture)
- [Security Model](#security-model)

---

## System Overview

AURA v4 is a production-grade AI assistant that runs entirely on Android devices. It is built as a Rust workspace with 5 crates implementing a two-process architecture: a **daemon** (cognitive core) and a **neocortex** (LLM inference engine), communicating via authenticated IPC.

```
┌─────────────────────────────────────────────────────────────────┐
│  Interface Layer                                                 │
│  Telegram Bot  │  Voice (STT/TTS)  │  JNI Android Bridge       │
├─────────────────────────────────────────────────────────────────┤
│  Neocortex (LLM Layer)                  aura-neocortex crate    │
│  ┌──────────────┐  ┌────────────────┐  ┌────────────────────┐  │
│  │  Qwen-3      │  │  6-layer       │  │  Context Budget    │  │
│  │  8B Q4_K_M   │  │  Teacher       │  │  Manager (2048t)   │  │
│  │  llama.cpp   │  │  Stack         │  │                    │  │
│  └──────────────┘  └────────────────┘  └────────────────────┘  │
├─────────────────────────────────────────────────────────────────┤
│  Cognitive Core (System 1 + System 2)   aura-daemon crate      │
│  ┌──────────────┐  ┌────────────────┐  ┌────────────────────┐  │
│  │  Pipeline    │  │  ReAct Loop    │  │  11-stage Executor  │  │
│  │  (parse →    │  │  (max 10       │  │  + PolicyGate       │  │
│  │   route)     │  │   iterations)  │  │  (deny-by-default)  │  │
│  └──────────────┘  └────────────────┘  └────────────────────┘  │
├─────────────────────────────────────────────────────────────────┤
│  Memory (4-Tier)                                                │
│  Working (RAM) → Episodic (SQLite+HNSW) →                     │
│  Semantic (FTS5+HNSW+RRF) → Archive (LZ4/ZSTD)               │
├─────────────────────────────────────────────────────────────────┤
│  Identity & Ethics                    aura-iron-laws crate     │
│  OCEAN+VAD personality  │  15 hardcoded ethics rules           │
│  Anti-sycophancy (0.4)  │  Trust tiers (Stranger→Soulmate)    │
├─────────────────────────────────────────────────────────────────┤
│  ARC (Adaptive Reasoning & Context)                            │
│  10 life domains  │  8 context modes  │  Initiative budget    │
├─────────────────────────────────────────────────────────────────┤
│  Android Platform                                              │
│  Heartbeat (30s)  │  Battery/Thermal events  │  Safe mode     │
└─────────────────────────────────────────────────────────────────┘
```

### The 7 Iron Laws

| # | Law | Description | Override? |
|---|-----|-------------|-----------|
| 1 | **LLM = Brain, Rust = Body** | All reasoning in LLM. Rust handles perception, memory, execution, safety. | Never |
| 2 | **Theater AGI BANNED** | No hardcoded heuristics or if-else reasoning chains. The LLM decides. | Never |
| 3 | **Anti-Cloud Absolute** | Zero telemetry, zero cloud fallback. Everything on-device. | Never |
| 4 | **Privacy-First** | All data stored locally in SQLite + bincode. Nothing leaves the phone. | Never |
| 5 | **Deny-by-Default Policy Gate** | Every capability denied unless in compile-time allow-list. | Must grant |
| 6 | **Never Change Correct Logic** | Tests reflect reality. Fix bugs, not tests. | Never |
| 7 | **15 Absolute Ethics Rules** | Compiled into binary. No config file can override. | Never |

---

## 5-Crate Structure

### Dependency Graph

```
aura-types (shared types, IPC protocol, config)
    │
    ├── aura-daemon (main process: memory, policy, platform, IPC client)
    │
    ├── aura-neocortex (LLM process: inference, IPC handler)
    │
    ├── aura-llama-sys (FFI bindings to llama.cpp)
    │
    └── aura-iron-laws (ethics layer, independent)
```

### Crate Details

| Crate | Purpose | Lines | Key Modules |
|---|---|---|---|
| **`aura-types`** | Shared types, IPC protocol, config structs | ~2,000 | `config.rs`, `ipc.rs`, `memory.rs`, `dsl.rs`, `events.rs`, `errors.rs` |
| **`aura-daemon`** | Cognitive core: memory, identity, execution, ARC, health | ~45,000 | `daemon_core`, `memory`, `identity`, `execution`, `telegram`, `voice`, `screen`, `ipc`, `arc`, `goals`, `health`, `platform` |
| **`aura-neocortex`** | LLM inference: 6-layer teacher stack, context management | ~8,000 | `inference`, `model`, `context`, `grammar`, `prompts`, `ipc_handler`, `tool_format` |
| **`aura-llama-sys`** | FFI bindings to llama.cpp (ARM64 batch API) | ~500 | `lib.rs`, `build.rs` (conditional compilation with stub mode) |
| **`aura-iron-laws`** | Immutable ethics layer — 7 Iron Laws enforced at compile time | ~300 | `EthicsGate`, `Action`, `IronLaw` evaluation |

### Build Targets

| Binary | Crate | Description |
|---|---|---|
| `aura-daemon` | `aura-daemon` | Main process — cognitive core, Telegram bot, memory, execution |
| `aura-neocortex` | `aura-neocortex` | LLM inference process — separate PID, killable by Android LMKD |

### Feature Flags

| Feature | Crate | Purpose |
|---|---|---|
| `curl-backend` | `aura-daemon` | HTTP backend using system curl + OpenSSL (Termux) |
| `reqwest` | `aura-daemon` | HTTP backend using rustls (CI/Linux) — **mutually exclusive** with `curl-backend` |
| `voice` | `aura-daemon` | Enable STT/TTS voice pipeline |
| `stub` | `aura-llama-sys` | Use stub FFI bindings (no real llama.cpp needed for dev/test) |

---

## Platform Abstraction Layer

AURA supports Android (Termux/APK), Linux, macOS, and Windows through a platform abstraction layer in `crates/aura-daemon/src/platform/`.

### Path Resolution

The `path_resolver` module (`platform/path_resolver.rs`) provides device-agnostic path resolution:

```
Environment Variable (AURA_*)  →  Platform Default  →  Fallback
```

| Function | Env Var | Android Default | Desktop Default |
|---|---|---|---|
| `data_dir()` | `AURA_DATA_DIR` | `/data/local/tmp/aura` | `~/.local/share/aura` |
| `model_dir()` | `AURA_MODELS_PATH` | `/data/local/tmp/aura/models` | `<data_dir>/models` |
| `db_path()` | `AURA_DB_PATH` | `/data/data/com.aura/databases/aura.db` | `<data_dir>/aura.db` |
| `home_dir()` | `AURA_HOME` | Termux `$HOME` | `dirs::home_dir()` |
| `neocortex_bin()` | `AURA_NEOCORTEX_BIN` | `/data/local/tmp/aura-neocortex` | `aura-neocortex` (on PATH) |

### Conditional Compilation

Platform-specific code uses `#[cfg(target_os = "android")]`:

```rust
#[cfg(target_os = "android")]
{
    // Android-specific: abstract Unix socket, Termux paths, JNI
    PathBuf::from("/data/local/tmp/aura")
}

#[cfg(not(target_os = "android"))]
{
    // Desktop: TCP fallback, dirs crate paths
    dirs::data_local_dir().join("aura")
}
```

### HTTP Backend Selection

The Telegram HTTP backend is selected at compile time via mutually exclusive features:

| Feature | Backend | TLS | Platform |
|---|---|---|---|
| `curl-backend` | System `libcurl` via FFI | OpenSSL (system) | Termux/Android |
| `reqwest` | `reqwest` crate | rustls (bundled) | CI/Linux |

This is enforced by `compile_error!` in `aura-daemon/src/lib.rs` — the build fails if both or neither feature is selected.

See [ENVIRONMENT-VARIABLES.md](ENVIRONMENT-VARIABLES.md) for the complete variable reference.

---

## IPC Protocol

### Transport

| Platform | Transport | Address |
|---|---|---|
| Android | Abstract Unix domain socket | `@aura_ipc_v4` |
| Desktop (dev) | TCP loopback | `127.0.0.1:19400` |

### Wire Format

All messages use a **length-prefixed bincode** frame:

```
[4-byte LE u32 length][bincode payload]
```

- **Max message size:** 256 KB (`MAX_MESSAGE_SIZE`)
- **Header size:** 4 bytes (`LENGTH_PREFIX_SIZE` / `FRAME_HEADER_SIZE`)
- **Request timeout:** 30 seconds (`REQUEST_TIMEOUT`)

### Authentication

Every message is wrapped in an `AuthenticatedEnvelope<T>`:

```rust
pub struct AuthenticatedEnvelope<T> {
    pub protocol_version: u32,   // Current: 3
    pub session_token: String,   // 32-byte hex (64 chars), CSPRNG
    pub seq: u64,                // Monotonic sequence (replay prevention)
    pub payload: T,              // The actual IPC message
}
```

- **Session token:** Generated at process spawn time via CSPRNG, shared via inherited file descriptor (not CLI args or env vars)
- **Replay protection:** Monotonic sequence numbers
- **Invalid messages:** Silently dropped (no error response — prevents oracle attacks)
- **Protocol version:** Checked on every message; mismatched versions are rejected

### Message Types

The IPC protocol supports two directions:

**Daemon → Neocortex:**
- `Load` — Load a model (with path, context size, thread count)
- `Infer` — Run inference (with context package, inference mode)
- `Unload` — Unload current model
- `HealthCheck` — Ping neocortex process

**Neocortex → Daemon:**
- `InferResult` — Inference output (with text, token counts, confidence)
- `ModelError` — Model loading failure
- `HealthResponse` — Health check response

### IPC Constants (Single Source of Truth)

All wire protocol constants are defined in `aura-types/src/ipc.rs`:

```rust
pub const MAX_MESSAGE_SIZE: usize = 256 * 1024;     // 256 KB
pub const LENGTH_PREFIX_SIZE: usize = 4;             // 4 bytes
pub const FRAME_HEADER_SIZE: usize = 4;              // Alias
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const PROTOCOL_VERSION: u32 = 3;
```

---

## Memory System

AURA implements a 4-tier memory architecture modeled on human memory systems.

### Tier Architecture

```
┌──────────────────────────────────────────────────────────────┐
│  Working Memory (RAM)                                        │
│  • Current conversation context                              │
│  • Latency: <1ms                                             │
│  • Capacity: ~2048 tokens                                    │
├──────────────────────────────────────────────────────────────┤
│  Episodic Memory (SQLite + HNSW)                             │
│  • Recent events, conversations, observations                │
│  • Latency: 2-8ms                                            │
│  • Capacity: 10,000 entries (configurable)                   │
├──────────────────────────────────────────────────────────────┤
│  Semantic Memory (SQLite + FTS5 + HNSW + RRF)               │
│  • Long-term knowledge, facts, patterns                      │
│  • Latency: 5-15ms                                           │
│  • Capacity: 5,000 entries (configurable)                    │
├──────────────────────────────────────────────────────────────┤
│  Archive (LZ4/ZSTD Compressed)                               │
│  • Cold storage for old memories                             │
│  • Latency: 50-200ms                                         │
│  • Capacity: unlimited (disk-bound)                          │
└──────────────────────────────────────────────────────────────┘
```

### Retrieval Scoring

Memory retrieval uses a weighted composite score:

| Component | Weight | Description |
|---|---|---|
| Similarity | 0.40 | Embedding cosine similarity |
| Recency | 0.20 | How recent the memory is |
| Importance | 0.20 | Stored importance score |
| Activation | 0.20 | Hebbian activation score |

**Hebbian re-ranking:** Items co-accessed with recent context receive a 20% score boost, mimicking associative memory.

### Consolidation

When episodic memory reaches capacity (`max_episodes`, default 10,000), consolidation promotes the most-accessed episodes to semantic memory. The consolidation process:

1. Scores episodes by access frequency × recency × importance
2. Promotes top entries to semantic memory
3. Archives oldest low-relevance entries with LZ4/ZSTD compression

### Storage

- **Database:** SQLite with WAL mode (4 MB journal limit)
- **Episodic:** Pure-Rust HNSW index for approximate nearest-neighbor search
- **Semantic:** FTS5 full-text search + HNSW + Reciprocal Rank Fusion (RRF) re-ranking
- **Archive:** Bincode serialization with LZ4 (fast) or ZSTD (high-ratio) compression

---

## Ethics Layer

### Architecture

The ethics system is implemented in `aura-iron-laws` as an **independent crate** with no dependencies on the daemon. This ensures the ethics gate cannot be bypassed by any other code path.

### The 7 Iron Laws

| # | Law | Enforcement | Override |
|---|-----|-------------|----------|
| 1 | **NeverHarm** | Compile-time `const fn` assertions | NEVER |
| 2 | **ConsentForLearning** | Runtime gate check | Can be granted |
| 3 | **PrivacySovereignty** | Compile-time + runtime | NEVER |
| 4 | **TransparentReasoning** | Runtime audit logging | Always required |
| 5 | **AntiSycophancy** | Compile-time markers (`!Sync`) | NEVER |
| 6 | **DenyByDefault** | Compile-time + PolicyGate | Must be granted |
| 7 | **AuditFinality** | Runtime audit verdicts | NEVER |

### Compile-Time Enforcement

```rust
// PhantomData markers prevent unsafe sharing
// !Sync markers prevent thread sharing
// Build script checksum verification
// const fn assertions for invariant checks
```

### Runtime Ethics Gate

```rust
use aura_iron_laws::{EthicsGate, Action, IronLaw};

let mut gate = EthicsGate::new();

let action = Action::new("Remember user preferences")
    .learning()
    .with_consent();

match gate.evaluate(&action) {
    EthicsResult::Permitted => { /* proceed */ }
    EthicsResult::Denied(v) => { /* blocked by iron law */ }
    EthicsResult::RequiresConsent { law, .. } => { /* obtain consent */ }
}
```

### Anti-Sycophancy System

- **Ring buffer:** 20 recent response patterns analyzed
- **Block threshold:** 0.40 (composite score — blocks and regenerates)
- **Warn threshold:** 0.25 (emits warning, allows response)
- **Max regenerations:** 3 (prevents infinite loops)

### Policy Gate (Deny-by-Default)

The PolicyGate runs on every action:

- Default effect: configurable (`allow`, `deny`, `audit`, `confirm`)
- Compile-time allow-list for capabilities
- Configurable rules in `[policy]` section of config
- Max 256 rules, first-match-wins evaluation

---

## Cognitive Architecture

### Dual-Process Model (System 1 + System 2)

AURA implements a dual-process cognitive architecture:

- **System 1 (Fast Path):** Amygdala scoring → routing decision. Handles simple, low-complexity events without LLM inference.
- **System 2 (Slow Path):** Full LLM inference via neocortex. Handles complex reasoning, planning, and creative tasks.

### Routing Decision

Events are scored on 4 dimensions:

| Factor | Weight | Description |
|---|---|---|
| Complexity | 0.40 | Estimated reasoning difficulty |
| Importance | 0.25 | User-facing priority |
| Urgency | 0.20 | Time-sensitivity |
| Memory load | 0.15 | Context required |

Events scoring above `complexity_threshold` (default 0.50) route to neocortex. Hysteresis gap (0.15) prevents route flapping.

### 6-Layer Teacher Stack

The neocortex inference pipeline applies 6 processing layers:

1. **GBNF Grammar** — Constrained generation for structured outputs
2. **Chain-of-Thought** — Explicit reasoning trace
3. **Logprob Calibration** — Token probability analysis
4. **Cascade Retry** — Retry with adjusted parameters (threshold 0.5)
5. **Cross-Model Reflection** — Self-evaluation of outputs
6. **Best-of-N** — Generate N=3 candidates, select best

### Inference Modes

| Mode | Temperature | Use Case |
|---|---|---|
| `planner` | 0.1 | Multi-step plan generation |
| `strategist` | 0.4 | Strategic reasoning about goals |
| `composer` | 0.2 | DSL action sequence generation |
| `conversational` | 0.7 | Natural language replies |

### Execution Engine

The execution engine runs action plans through an 11-stage pipeline with a deny-by-default PolicyGate. Rate limiting (60 actions/min) and human-like pacing (150–500ms delays) prevent detection by apps.

### ARC (Adaptive Reasoning & Context)

Proactive behavioral intelligence across 10 life domains:
- Health, Finance, Relationships, Growth, Career, Social, Home, Creative, Learning, Meta

An **initiative budget** system prevents spam — AURA only reaches out proactively when it has earned the right through accumulated trust and context.

---

## Security Model

### Vault System

- **Encryption:** AES-256-GCM
- **Key Derivation:** Argon2id (64 MB memory, 3 iterations, 4 parallel)
- **Data Classification:** 4-tier (public / internal / confidential / secret)

### IPC Security

- **Session tokens:** 32-byte CSPRNG, shared via inherited FD
- **Replay protection:** Monotonic sequence numbers
- **Oracle attack prevention:** Invalid messages silently dropped
- **Protocol versioning:** Version 3, checked on every message

### Policy Gate

- **Default:** Deny-by-default
- **Max rules:** 256
- **Evaluation:** First-match-wins, ordered by priority
- **Effects:** `allow`, `deny`, `audit`, `confirm`

### Power Management

5 power tiers with asymmetric hysteresis prevent oscillation:

| Tier | Battery | Model | Inferences/Hour | Proactive |
|---|---|---|---|---|
| Charging | >80% | Full8B | 120 | ✅ |
| Normal | 40–80% | Standard4B | 60 | ✅ |
| Conserve | 20–40% | Brainstem1_5B | 20 | ❌ |
| Critical | 10–20% | Brainstem1_5B | 5 | ❌ |
| Emergency | <10% | Brainstem1_5B | 0 | ❌ |

### Thermal Management

| State | Temperature | Action |
|---|---|---|
| Normal | <40°C | Full operation |
| Warm | 40–45°C | Reduced background scanning |
| Hot | 45–50°C | Inference paused |
| Critical | >50°C | Emergency checkpoint, all suspended |

---

## Extension Architecture

AURA supports a sandboxed extension system with 4 containment levels:

```
┌─────────────────────────────────────────────────────────────┐
│  Extension Sandbox                                           │
├─────────────────────────────────────────────────────────────┤
│  Level 1: Pure Function                                      │
│  • No side effects                                           │
│  • No I/O, no network                                        │
│  • Compile-time verified                                     │
├─────────────────────────────────────────────────────────────┤
│  Level 2: Controlled I/O                                     │
│  • File read/write (sandboxed)                               │
│  • No network access                                         │
│  • Audit logged                                              │
├─────────────────────────────────────────────────────────────┤
│  Level 3: Network Access                                     │
│  • Whitelisted domains only                                  │
│  • Rate limited                                              │
│  • User consent required                                     │
├─────────────────────────────────────────────────────────────┤
│  Level 4: Full System                                        │
│  • JNI bridge access                                         │
│  • Requires vault PIN                                        │
│  • Full audit trail                                          │
└─────────────────────────────────────────────────────────────┘
```

### Extension Loading

Extensions are compiled into the binary (no dynamic loading). Each extension:
- Implements the `Extension` trait
- Declares required containment level
- Registers with the PolicyGate
- Subject to ethics evaluation

---

## JNI Bridge

The JNI bridge enables the daemon to interact with Android system services:

### Core Functions

| Function | Purpose | Parameters |
|---|---|---|
| `nativeInit()` | Initialize daemon state | Returns state pointer |
| `nativeRun()` | Start main loop | State pointer, blocks until shutdown |
| `nativeShutdown()` | Clean shutdown | State pointer (frees state) |

### Android Intents

The JNI bridge can invoke Android intents for:
- Opening URLs (whitelisted: `https://`, `http://` only)
- Sending notifications
- Accessing device sensors (battery, thermal, screen state)
- Triggering accessibility services

### Security Considerations

> **Critical:** The JNI bridge is a high-risk surface. The enterprise audit identified:
> - Use-after-free risk in `nativeInit()` → `nativeRun()` → `nativeShutdown()` sequence
> - URL injection vulnerability (now fixed with scheme whitelist)
> - Thread safety concerns with raw pointer handling
>
> See [Security Model](#security-model) for mitigation details.

---

## Voice System

AURA supports optional STT/TTS voice interaction via the `voice` feature flag:

### Components

| Component | Library | Purpose |
|---|---|---|
| **Wake Word** | Custom | Detect activation phrase |
| **STT** | Whisper.cpp | Speech-to-text transcription |
| **VAD** | Silero | Voice Activity Detection |
| **TTS** | Piper | Text-to-speech synthesis |
| **Signal Processing** | Custom | Audio preprocessing |

### Thread Safety Note

The voice subsystem wraps C libraries that may not be thread-safe. The enterprise audit identified 7 `unsafe impl Send` types in the voice module. Each type requires explicit thread-safety documentation from the underlying C library.

---

## Audit Findings Summary

The enterprise audit (April 2026) identified 127+ issues across 6 categories:

### Critical Issues (34)

| Category | Count | Status |
|---|---|---|
| Security | 5 | Being addressed |
| Build | 2 | Being addressed |
| Architecture | 4 | Being addressed |
| Deployment | 7 | Being addressed |
| Code Quality | 8 | Being addressed |
| Testing | 8 | Being addressed |

### Architectural Strengths (Preserved)

These design decisions were validated as excellent:

| Feature | Why It's Strong |
|---|---|
| **4-tier memory** | WAL-mode SQLite with cross-tier queries, HNSW vector search |
| **Authenticated IPC** | CSPRNG tokens + protocol versioning + rate limiting |
| **Teacher stack** | CoT forcing + grammar constraints + cascade retry |
| **Physics-based power** | Real battery metrics (mWh, mA, °C) not just percentages |
| **Ethics layer** | Independent crate, compile-time enforcement, can't be bypassed |

### Known Issues Being Tracked

| Issue | Location | Impact | Fix Status |
|---|---|---|---|
| JNI Use-After-Free | `lib.rs:179` | High | Pending sentinel tracking |
| `unsafe impl Send` (7 types) | `voice/*.rs` | Medium | Pending thread-safety audit |
| Path traversal in model loading | `lib.rs:1331` | High | Pending canonicalization |
| Duplicated IPC constants | `protocol.rs` + `ipc_handler.rs` | Low | Pending consolidation |
| Hardcoded Android paths | Multiple files | Medium | Being refactored |

---

## Further Reading

| Document | Description |
|---|---|
| [API-REFERENCE.md](docs/API-REFERENCE.md) | Public interfaces, IPC protocol, JNI bridge |
| [DEPLOYMENT-GUIDE.md](DEPLOYMENT-GUIDE.md) | Step-by-step deployment instructions |
| [USER-JOURNEY.md](docs/USER-JOURNEY.md) | First-time setup and daily usage guide |
| [ENVIRONMENT-VARIABLES.md](ENVIRONMENT-VARIABLES.md) | Complete environment variable reference |
| [README.md](README.md) | Project overview and quick install |
| [aura-config.example.toml](aura-config.example.toml) | All config options with comments |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Development setup and contribution guide |
| [SECURITY.md](SECURITY.md) | Security policy and vulnerability reporting |
| [CHANGELOG.md](CHANGELOG.md) | Release history |
