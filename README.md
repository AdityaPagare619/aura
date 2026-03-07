# AURA v4 — On-Device AGI Assistant

> *"I remember, therefore I become."*

AURA is a privacy-first, on-device AI assistant that *thinks*, not just responds. Built in Rust for Android, running entirely on the user's phone.

## Architecture

```
┌─────────────────────────────────────────────────┐
│                  Telegram UI                     │
├─────────────────────────────────────────────────┤
│              Neocortex (LLM Layer)               │
│     ┌──────────┐ ┌──────────┐ ┌──────────┐      │
│     │  Model    │ │ Prompts  │ │Inference │      │
│     │  Select   │ │ Assembly │ │  Stack   │      │
│     └──────────┘ └──────────┘ └──────────┘      │
├─────────────────────────────────────────────────┤
│                 ARC (Cognitive Core)              │
│   ┌──────────┐ ┌──────────┐ ┌──────────┐        │
│   │ Learning  │ │ Routing  │ │ Executor │        │
│   │ Engine    │ │ Engine   │ │ Pipeline │        │
│   ├──────────┤ └──────────┘ └──────────┘        │
│   │ Hebbian   │ ┌──────────┐ ┌──────────┐       │
│   │ Prediction│ │ Patterns │ │ Dreaming │       │
│   │ Dimension │ │ Routines │ │          │       │
│   └──────────┘ └──────────┘ └──────────┘        │
├─────────────────────────────────────────────────┤
│                   Identity                       │
│   ┌──────────┐ ┌──────────┐ ┌──────────┐        │
│   │Personality│ │ Ethics/  │ │Epistemic │        │
│   │ OCEAN     │ │ TRUTH    │ │Awareness │        │
│   └──────────┘ └──────────┘ └──────────┘        │
├─────────────────────────────────────────────────┤
│              Memory (4-Tier SQLite)               │
│   Working → Episodic → Semantic → Archive        │
│   HNSW Index │ Embeddings │ Consolidation        │
├─────────────────────────────────────────────────┤
│              Brainstem (Daemon Core)              │
│   Checkpoint │ Power │ Startup │ Shutdown         │
└─────────────────────────────────────────────────┘
```

## Workspace Crates

| Crate | Description |
|---|---|
| `aura-daemon` | Core daemon — memory, learning, identity, routing, execution |
| `aura-types` | Shared types, configs, error definitions |
| `aura-neocortex` | LLM inference layer — model selection, prompts, teacher stack |
| `aura-llama-sys` | FFI bindings to llama.cpp for on-device inference |

## Key Differentiators

- **Active Inference**: Predicts user behavior, computes surprise, adapts
- **Emergent Dimensions**: Discovers unique user patterns (not preset labels)
- **Epistemic Awareness**: Knows what it doesn't know, hedges honestly
- **4-Tier Memory**: Working → Episodic → Semantic → Archive with HNSW
- **TRUTH Framework**: Anti-hallucination, anti-sycophancy, epistemic gates
- **On-Device Only**: No cloud. Your data never leaves your phone.

## Build

```bash
# Desktop (for testing)
cargo build
cargo test

# Android (via NDK cross-compilation)
cargo build --target aarch64-linux-android --release
```

## CI

GitHub Actions runs:
- `cargo check` (compilation)
- `cargo test` (unit + integration tests)
- `cargo clippy` (lint)
- `cargo fmt --check` (formatting)

See [`.github/workflows/ci.yml`](.github/workflows/ci.yml)

## License

Proprietary — All Rights Reserved
