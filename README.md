<div align="center">

# AURA

### Your phone's AI. Entirely on your phone.

*Privacy-first · On-device · No cloud · No tracking · Built in Rust*

[![CI](https://github.com/AdityaPagare619/aura/actions/workflows/ci.yml/badge.svg)](https://github.com/AdityaPagare619/aura/actions/workflows/ci.yml)
[![Android Build](https://github.com/AdityaPagare619/aura/actions/workflows/build-android.yml/badge.svg)](https://github.com/AdityaPagare619/aura/actions/workflows/build-android.yml)
[![License: Proprietary](https://img.shields.io/badge/license-Proprietary-red.svg)](#license)

</div>

---

AURA is a production-grade AI assistant that runs **entirely on your Android device**. No subscriptions. No API keys. No data leaving your phone. Ever.

Powered by [Qwen-3](https://huggingface.co/Qwen/Qwen3-8B) via [llama.cpp](https://github.com/ggerganov/llama.cpp), built with a full cognitive architecture in Rust, installed via [Termux](https://termux.dev) in a single command.

---

## Install (Android via Termux)

**One command — paste in Termux:**

```bash
curl -fsSL https://raw.githubusercontent.com/AdityaPagare619/aura/main/install.sh | bash
```

Or if you prefer to inspect the script first:

```bash
curl -fsSL https://raw.githubusercontent.com/AdityaPagare619/aura/main/install.sh -o install.sh
less install.sh          # read it
bash install.sh          # run it
```

The installer handles everything: Rust toolchain, Android NDK, model download (~5.2 GB), daemon binary, config generation, PIN setup, and autostart via termux-services. An install log is saved automatically for sharing if anything goes wrong.

**Requirements:** Android 10+, ARM64, ~8 GB free storage, 4 GB+ RAM

---

## What makes AURA different

| | AURA | ChatGPT / Gemini |
|---|---|---|
| Runs offline | ✅ Always | ❌ Never |
| Your data leaves device | ❌ Never | ✅ Always |
| Remembers across sessions | ✅ 4-tier memory | ❌ Resets |
| Learns your patterns | ✅ Hebbian + decay | ❌ No |
| Proactive (not just reactive) | ✅ ARC system | ❌ No |
| Monthly subscription | ❌ Free forever | ✅ $20+/month |

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  Interface Layer                                             │
│  Telegram Bot  │  Voice (STT/TTS)  │  JNI Android Bridge   │
├─────────────────────────────────────────────────────────────┤
│  Neocortex (LLM Layer)              aura-neocortex crate    │
│  ┌────────────┐  ┌──────────────┐  ┌────────────────────┐  │
│  │  Qwen-3    │  │  6-layer     │  │  Context Budget    │  │
│  │  8B Q4_K_M │  │  Teacher     │  │  Manager (2048t)   │  │
│  │  llama.cpp │  │  Stack       │  │                    │  │
│  └────────────┘  └──────────────┘  └────────────────────┘  │
├─────────────────────────────────────────────────────────────┤
│  Cognitive Core (System 1 + System 2)   aura-daemon crate   │
│  ┌────────────┐  ┌──────────────┐  ┌────────────────────┐  │
│  │  Pipeline  │  │  ReAct Loop  │  │  11-stage Executor  │  │
│  │  (parse →  │  │  (max 10     │  │  + PolicyGate       │  │
│  │   route)   │  │   iterations)│  │  (deny-by-default)  │  │
│  └────────────┘  └──────────────┘  └────────────────────┘  │
├─────────────────────────────────────────────────────────────┤
│  Memory (4-Tier)                                            │
│  Working (RAM) → Episodic (SQLite+HNSW) →                   │
│  Semantic (FTS5+HNSW+RRF) → Archive (LZ4/ZSTD)             │
├─────────────────────────────────────────────────────────────┤
│  Identity & Ethics                                          │
│  OCEAN+VAD personality  │  15 hardcoded ethics rules        │
│  Anti-sycophancy (0.4)  │  Trust tiers (Stranger→Soulmate)  │
├─────────────────────────────────────────────────────────────┤
│  ARC (Adaptive Reasoning & Context)                         │
│  10 life domains  │  8 context modes  │  Initiative budget  │
├─────────────────────────────────────────────────────────────┤
│  Android Platform                                           │
│  Heartbeat (30s)  │  Battery/Thermal events  │  Safe mode   │
└─────────────────────────────────────────────────────────────┘
```

### Core design principles

**LLM = brain, Rust = body.** Rust does zero semantic reasoning. It routes, schedules, stores, and enforces. Every decision that requires understanding goes to the LLM.

**No theater AGI.** No keyword matching for intent. No regex NLU. The LLM handles all natural language understanding through the ReAct loop.

**Deny-by-default policy gate.** Every action capability is explicitly allowlisted. Prompt injection cannot grant new permissions — they're hardcoded, not configurable.

---

## Crates

| Crate | Lines | Purpose |
|---|---|---|
| `aura-daemon` | ~45,000 | Cognitive core: memory, identity, execution, ARC, health |
| `aura-neocortex` | ~8,000 | LLM inference: 6-layer teacher stack, context management |
| `aura-llama-sys` | ~500 | FFI bindings to llama.cpp (ARM64 batch API) |
| `aura-types` | ~2,000 | Shared types, IPC protocol, config, events |

---

## Key Systems

### Memory (4-tier)
- **Working** — in-RAM, <1ms, current conversation context
- **Episodic** — SQLite + pure-Rust HNSW, 2-8ms, recent events
- **Semantic** — SQLite + FTS5 + HNSW + RRF re-ranking, 5-15ms, long-term knowledge
- **Archive** — ZSTD compressed, 50-200ms, cold storage

Hebbian re-ranking: items co-accessed with recent context get 20% score boost.

### Inference (6-layer teacher stack)
GBNF grammar → Chain-of-Thought → Logprob calibration → Cascade retry (threshold 0.5) → Cross-model reflection → Best-of-N (N=3)

### Security
- **Vault**: AES-256-GCM + Argon2id (64MB memory, 3 iterations, 4 parallel)
- **4-tier data classification**: public / internal / confidential / secret
- **Ethics**: 15 absolute rules compiled into binary. No config file can override them.

### ARC (Adaptive Reasoning & Context)
Proactive behavioral intelligence across 10 life domains (health, finance, relationships, growth, ...). Initiative budget system prevents spam — AURA only reaches out when it has earned the right to.

---

## Development

### Prerequisites
- Rust stable (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- For Android cross-compilation: Android NDK r26+ (install.sh downloads it automatically)

### Build (desktop, for development)
```bash
git clone https://github.com/AdityaPagare619/aura.git
cd aura
git submodule update --init --recursive
cargo check --workspace
cargo test -p aura-daemon
```

### Build (Android ARM64)
```bash
rustup target add aarch64-linux-android
# Set NDK path in .cargo/config.toml (see docs/architecture/AURA-V4-CONTRIBUTING-AND-DEV-SETUP.md)
cargo build --target aarch64-linux-android --release
```

### Test
```bash
cargo test -p aura-daemon   # 2376 tests
cargo check --workspace      # Full type check
```

---

## Documentation

| Doc | Description |
|---|---|
| [System Architecture](docs/architecture/AURA-V4-SYSTEM-ARCHITECTURE.md) | Full architectural overview, 7 Iron Laws |
| [Operational Flow](docs/architecture/AURA-V4-OPERATIONAL-FLOW.md) | Request lifecycle end-to-end |
| [Neocortex & Token Economics](docs/architecture/AURA-V4-NEOCORTEX-AND-TOKEN-ECONOMICS.md) | LLM inference design |
| [Memory & Data](docs/architecture/AURA-V4-MEMORY-AND-DATA-ARCHITECTURE.md) | 4-tier memory, HNSW, Hebbian |
| [Identity, Ethics & Philosophy](docs/architecture/AURA-V4-IDENTITY-ETHICS-AND-PHILOSOPHY.md) | OCEAN, trust, 15 ethics rules |
| [Security Model](docs/architecture/AURA-V4-SECURITY-MODEL.md) | Vault, policy gate, threat model |
| [ARC Behavioral Intelligence](docs/architecture/AURA-V4-ARC-BEHAVIORAL-INTELLIGENCE.md) | Proactive system, life domains |
| [Installation & Deployment](docs/architecture/AURA-V4-INSTALLATION-AND-DEPLOYMENT.md) | Full install guide + troubleshooting |
| [Contributing](docs/architecture/AURA-V4-CONTRIBUTING-AND-DEV-SETUP.md) | Dev setup, PR guide |
| [Production Status](docs/architecture/AURA-V4-PRODUCTION-STATUS.md) | Live implementation status |
| [ADR Index](docs/adr/) | 7 Architecture Decision Records |

---

## Production Status

- `cargo check --workspace` — **PASS**
- Test suite — **2376 / 2376 passing**
- Android cross-compilation — **PASS** (CI verified)
- llama.cpp submodule — requires `git submodule update --init --recursive`

See [PRODUCTION-STATUS.md](docs/architecture/AURA-V4-PRODUCTION-STATUS.md) for full breakdown.

---

## License

Proprietary — All Rights Reserved © 2026 Aditya Pagare

Personal use only. No redistribution, modification, or commercial use without explicit written permission.
