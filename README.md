<div align="center">

# AURA

### Your phone's AI. Entirely on your phone.

*Privacy-first · On-device · No cloud · No tracking · Built in Rust*

[![CI](https://github.com/AdityaPagare619/aura/actions/workflows/ci.yml/badge.svg)](https://github.com/AdityaPagare619/aura/actions/workflows/ci.yml)
[![Android Build](https://github.com/AdityaPagare619/aura/actions/workflows/build-android.yml/badge.svg)](https://github.com/AdityaPagare619/aura/actions/workflows/build-android.yml)
[![License: Proprietary](https://img.shields.io/badge/license-Proprietary-red.svg)](#license)
[![Version](https://img.shields.io/badge/version-4.0.0--alpha.1-blue.svg)](#production-status)

</div>

---

AURA is a production-grade AI assistant that runs **entirely on your Android device**. No subscriptions. No API keys. No data leaving your phone. Ever.

Powered by [Qwen-3](https://huggingface.co/Qwen/Qwen3-8B) via [llama.cpp](https://github.com/ggerganov/llama.cpp), built with a full cognitive architecture in Rust, installed via [Termux](https://termux.dev).

> **AURA's promise:** Your conversations, memories, and personal data never leave your device. There are zero cloud calls, zero telemetry endpoints, and zero analytics. The only optional external connection is the Telegram Bot API — which you control with your own bot token.

---

### 📱 Quick Install (Android)

> **Prerequisites:** Android 8.0+, ARM64 phone, 4 GB+ RAM, 8 GB free storage.
>
> 1. Install **[Termux from F-Droid](https://f-droid.org/en/packages/com.termux/)** (NOT Google Play)
> 2. Open Termux and run:
> ```bash
> termux-setup-storage && curl -fsSL https://raw.githubusercontent.com/AdityaPagare619/aura/main/install.sh -o install.sh && bash install.sh
> ```
> 3. Follow the prompts (model selection, Telegram bot token, vault PIN)
> 4. Done! Open Telegram → message your bot → start chatting with AURA
>
> **[Detailed step-by-step guide ↓](#step-by-step-installation)** · **[Troubleshooting ↓](#troubleshooting)**

---

## Table of Contents

- [Install on Android](#install-on-android)
  - [What You Need](#what-you-need)
  - [Step-by-Step Installation](#step-by-step-installation)
  - [Installation Options](#installation-options)
  - [What the Installer Does](#what-the-installer-does)
- [After Installation](#after-installation)
  - [First Launch](#first-launch)
  - [Managing AURA](#managing-aura)
  - [Updating AURA](#updating-aura)
- [What Makes AURA Different](#what-makes-aura-different)
- [Architecture Overview](#architecture-overview)
- [Crates](#crates)
- [Key Systems](#key-systems)
- [Troubleshooting](#troubleshooting)
- [Development](#development)
- [Documentation](#documentation)
- [Production Status](#production-status)
- [FAQ](#faq)
- [License](#license)

---

## Install on Android

### What You Need

| Requirement | Minimum | Recommended |
|---|---|---|
| **Android version** | 8.0 (API 26) | 12+ |
| **Processor** | ARM64 (aarch64) | Any ARM64 phone made after 2017 |
| **RAM** | 4 GB | 8 GB+ |
| **Free storage** | 8 GB | 16 GB |
| **App** | [Termux](https://f-droid.org/en/packages/com.termux/) from **F-Droid** | Latest version |
| **Network** | Needed for install only | Wi-Fi recommended for model download |

> **Important:** Install Termux from [F-Droid](https://f-droid.org/en/packages/com.termux/), **not** the Google Play Store. The Play Store version is outdated and will not work.

### Step-by-Step Installation

**Step 1 — Install Termux**

Download and install [Termux from F-Droid](https://f-droid.org/en/packages/com.termux/). Open it — you'll see a terminal.

**Step 2 — Grant storage access**

```bash
termux-setup-storage
```

Tap "Allow" when Android asks for permission. This lets AURA access files if needed.

**Step 3 — Download and run the installer**

The recommended method (lets you inspect the script before running):

```bash
curl -fsSL https://raw.githubusercontent.com/AdityaPagare619/aura/main/install.sh -o install.sh
bash install.sh
```

Or as a one-liner (safe — all interactive prompts read from your terminal, not the pipe):

```bash
curl -fsSL https://raw.githubusercontent.com/AdityaPagare619/aura/main/install.sh | bash
```

**Step 4 — Follow the interactive setup**

The installer will ask you three things upfront, then run unattended:

1. **Model selection** — Auto-detected from your RAM. Just press Enter to confirm, or pick a different size.
2. **Telegram bot token** — Create a bot via [@BotFather](https://t.me/BotFather) on Telegram and paste the token.
3. **Vault PIN** — A 4+ character PIN that protects sensitive operations.

After these prompts, the installer runs fully unattended. You can lock your screen.

> **Tip:** Before locking your screen, enable Termux's **wakelock**: pull down the notification shade → long-press the Termux notification → tap "Acquire Wakelock". This prevents Android from killing the build process.

**Step 5 — Wait for completion (10–30 minutes)**

The installer will:
- Install system packages
- Set up the Rust toolchain
- Clone AURA and its dependencies
- Download your selected AI model (2–10 GB)
- Build the binaries
- Generate your config
- Set up auto-start

When it's done, you'll see a green success banner.

### Installation Options

| Option | What it does |
|---|---|
| `--model qwen3-4b` | Force a specific model instead of auto-detection |
| `--skip-build` | Download pre-built binaries from GitHub Releases instead of compiling |
| `--skip-model` | Skip model download (if you already have the GGUF file) |
| `--skip-service` | Don't set up auto-start |
| `--keep-build-tools` | Keep the Rust toolchain after build (~4 GB) |
| `--update` | Update an existing installation |
| `--repair model` | Re-run only the model download phase |
| `--repair build` | Re-run only the build phase |
| `--dry-run` | Show what would happen without doing anything |
| `--channel nightly` | Use the latest development branch instead of the stable tag |

**Examples:**

```bash
# Standard install (auto-detects everything):
bash install.sh

# Install with 4B model for a phone with limited RAM:
bash install.sh --model qwen3-4b

# Skip compilation — download pre-built binaries (faster, no thermal throttling):
bash install.sh --skip-build

# Update an existing installation:
bash install.sh --update

# Re-download the model (if the download was interrupted):
bash install.sh --repair model
```

### What the Installer Does

The installer runs in 12 phases. All user interaction happens in the first 3 phases, so you only need to be present for ~2 minutes.

| Phase | Name | Interactive? | What happens |
|---|---|---|---|
| 0 | Pre-flight | No | Checks architecture, Android version, Termux, network |
| 0.5 | Space budget | No | Shows RAM/storage and model size table |
| 1 | Hardware + Model | **Yes** | Auto-selects AI model, asks you to confirm |
| 2 | Telegram wizard | **Yes** | Guides you through creating a Telegram bot |
| 3 | Vault & Identity | **Yes** | Sets your name and vault PIN |
| 4 | Packages | No | Installs build tools via `pkg` |
| 5 | Rust toolchain | No | Installs Rust nightly via rustup |
| 6 | Source | No | Clones AURA repo + llama.cpp submodule |
| 7 | Model download | No | Downloads your selected GGUF model (resumable) |
| 8 | Build | No | Compiles aura-daemon + aura-neocortex |
| 9 | Purge | No | Removes build tools to free ~4 GB |
| 10 | Config | No | Writes `config.toml` with all your settings |
| 11 | Service | No | Sets up auto-start via termux-services |
| 12 | Verify | No | Checks everything and shows success banner |

A full install log is saved automatically. If anything fails, share the log file when reporting issues.

---

## After Installation

### First Launch

1. Open Telegram
2. Find the bot you created with @BotFather
3. Send `/start`
4. Start chatting — AURA is running locally on your phone

AURA starts automatically whenever you open Termux. If you enabled wakelock, it keeps running in the background.

### Managing AURA

```bash
# Check if AURA is running:
pgrep -x aura-daemon && echo "✅ running" || echo "❌ stopped"

# Start manually:
aura-daemon --config ~/.config/aura/config.toml &

# View live logs:
tail -f ~/.local/share/aura/logs/current

# Stop AURA:
sv down aura-daemon          # if using termux-services
# or
pkill -x aura-daemon         # direct kill
```

### Updating AURA

```bash
bash install.sh --update
```

This pulls the latest code, rebuilds, and preserves your config, memories, and data.

### Useful Paths

| What | Path |
|---|---|
| Config | `~/.config/aura/config.toml` |
| Models | `~/.local/share/aura/models/` |
| Database | `~/.local/share/aura/db/` |
| Logs | `~/.local/share/aura/logs/` |
| Source | `~/aura/` |

---

## What Makes AURA Different

| | AURA | ChatGPT / Gemini / Siri |
|---|---|---|
| **Runs offline** | ✅ Always | ❌ Never |
| **Your data leaves device** | ❌ Never | ✅ Always |
| **Remembers across sessions** | ✅ 4-tier memory | ❌ Resets each time |
| **Learns your patterns** | ✅ Hebbian learning + decay | ❌ No |
| **Proactive (not just reactive)** | ✅ ARC system | ❌ No |
| **Monthly subscription** | ❌ Free forever | ✅ $20+/month |
| **Works without internet** | ✅ Full functionality | ❌ Useless |
| **You control the model** | ✅ Pick your model size | ❌ No choice |

---

## Architecture Overview

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

### The 7 Iron Laws

1. **LLM = Brain, Rust = Body** — All reasoning lives in the LLM. Rust handles perception, memory, execution, and safety.
2. **Theater AGI BANNED** — No hardcoded heuristics or if-else reasoning chains. The LLM decides.
3. **Anti-Cloud Absolute** — Zero telemetry, zero cloud fallback. Everything runs on-device.
4. **Privacy-First** — All user data stored locally in SQLite + bincode. Nothing leaves the phone.
5. **Deny-by-Default Policy Gate** — Every capability is denied unless explicitly in the compile-time allow-list.
6. **Never Change Correct Logic to Make Tests Pass** — Tests reflect reality. Fix bugs, not tests.
7. **15 Absolute Ethics Rules Hardcoded** — Compiled into the binary. No config file can override them.

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

Hebbian re-ranking: items co-accessed with recent context get a 20% score boost.

### Inference (6-layer teacher stack)
GBNF grammar → Chain-of-Thought → Logprob calibration → Cascade retry (threshold 0.5) → Cross-model reflection → Best-of-N (N=3)

### IPC (daemon ↔ neocortex)
- **Android:** Abstract Unix domain socket (`@aura_ipc_v4`) — fast, zero-copy, no filesystem
- **Desktop (dev):** TCP loopback `127.0.0.1:19400` — same protocol, different transport
- **Frame format:** `[4-byte LE length][bincode payload]` with 256 KB size limit

### Security
- **Vault**: AES-256-GCM + Argon2id (64 MB memory, 3 iterations, 4 parallel)
- **4-tier data classification**: public / internal / confidential / secret
- **Ethics**: 15 absolute rules compiled into binary. No config file can override them.

### ARC (Adaptive Reasoning & Context)
Proactive behavioral intelligence across 10 life domains (health, finance, relationships, growth, ...). Initiative budget system prevents spam — AURA only reaches out when it has earned the right to.

---

## Troubleshooting

### Installation Issues

**"AURA crashes during `curl | bash` install"**

This is usually caused by the download being interrupted. Use the two-step method instead:
```bash
curl -fsSL https://raw.githubusercontent.com/AdityaPagare619/aura/main/install.sh -o install.sh
bash install.sh
```

**"Build takes too long / phone gets very hot"**

Mobile SoCs aren't designed for sustained compilation. You have two options:
1. **Use pre-built binaries:** `bash install.sh --skip-build`
2. **If building from source:** Enable wakelock, plug in charger, and give it 15–30 minutes. The installer limits build parallelism to prevent thermal throttling.

**"Not enough storage"**

AURA needs ~8 GB minimum. The biggest items are:
- AI model: 2–10 GB (depending on which model you chose)
- Rust toolchain + build: ~4 GB (purged automatically after build)
- AURA binaries + data: ~0.5 GB

Use `--model qwen3-1.5b` for the smallest model (~2 GB).

**"Model download keeps failing"**

The download is resumable. Just re-run:
```bash
bash install.sh --repair model
```
If you're rate-limited by HuggingFace, get a free token at [huggingface.co/settings/tokens](https://huggingface.co/settings/tokens) and run:
```bash
HF_TOKEN=your_token_here bash install.sh --repair model
```

### Runtime Issues

**"AURA stops when I lock my screen"**

Enable Termux wakelock:
1. Pull down notification shade
2. Long-press the Termux notification
3. Tap "Acquire Wakelock"

Or from the terminal: `termux-wake-lock`

**"Telegram bot doesn't respond"**

1. Check if the daemon is running: `pgrep -x aura-daemon`
2. Check logs: `tail -20 ~/.local/share/aura/logs/current`
3. Verify your bot token: `grep bot_token ~/.config/aura/config.toml`
4. Restart: `sv restart aura-daemon` or `pkill -x aura-daemon && aura-daemon --config ~/.config/aura/config.toml &`

**"How do I change the AI model?"**

1. Edit `~/.config/aura/config.toml` — change the `model_file` field
2. Download the new model: `bash install.sh --repair model --model qwen3-8b`
3. Restart the daemon

---

## Development

### Prerequisites

- **Rust nightly** (pinned to `nightly-2026-03-01` via `rust-toolchain.toml`)
- **cmake** + **build-essential** + **pkg-config** + **libopus-dev** (for voice features)
- For Android cross-compilation: Android NDK r26+ (CI downloads it automatically)

The project uses Rust nightly for the `negative_impls` feature. The exact toolchain version is pinned in `rust-toolchain.toml` so `rustup` will install it automatically.

### Build (desktop, for development)

```bash
git clone https://github.com/AdityaPagare619/aura.git
cd aura
git submodule update --init --recursive

# Type-check the full workspace (uses stub feature for llama.cpp on non-ARM):
cargo check --workspace --features "aura-llama-sys/stub,aura-daemon/voice"

# Run tests:
cargo test --workspace --features "aura-llama-sys/stub,aura-daemon/voice"

# Lint:
cargo clippy --workspace --features "aura-llama-sys/stub,aura-daemon/voice" -- -D warnings

# Format check:
cargo fmt --check
```

### Build (Android ARM64)

```bash
rustup target add aarch64-linux-android
# Set NDK path in .cargo/config.toml (see docs/architecture/AURA-V4-CONTRIBUTING-AND-DEV-SETUP.md)
cargo build --target aarch64-linux-android --release -p aura-daemon -p aura-neocortex --features "aura-daemon/voice"
```

### Test

```bash
# Full test suite (2830 tests):
cargo test --workspace --features "aura-llama-sys/stub,aura-daemon/voice"

# Just the daemon (2416 tests):
cargo test -p aura-daemon --features "aura-llama-sys/stub,aura-daemon/voice"

# Just the neocortex (264 tests):
cargo test -p aura-neocortex --features "aura-llama-sys/stub"

# Just shared types (48 tests):
cargo test -p aura-types
```

### CI Pipeline

The CI runs 6 parallel jobs on every push and PR:

| Job | What it checks |
|---|---|
| `check` | `cargo check` — full workspace type verification |
| `test` | `cargo test` — 2830 tests across all crates |
| `clippy` | `cargo clippy` — zero warnings enforced |
| `fmt` | `cargo fmt --check` — consistent formatting |
| `audit` | `cargo audit` — no known CVEs in dependencies |
| `version-check` | Cargo.toml version matches git tag |

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

| Check | Status |
|---|---|
| `cargo check --workspace` | ✅ **PASS** |
| `cargo clippy` (zero warnings) | ✅ **PASS** |
| `cargo fmt --check` | ✅ **PASS** |
| Test suite | ✅ **2830 / 2830 passing** |
| Android cross-compilation (CI) | ✅ **PASS** |
| IPC protocol (daemon ↔ neocortex) | ✅ **Verified** |
| install.sh `curl \| bash` safety | ✅ **All reads use `/dev/tty`** |

See [PRODUCTION-STATUS.md](docs/architecture/AURA-V4-PRODUCTION-STATUS.md) for the full breakdown.

---

## FAQ

**Q: Does AURA need internet to work?**
No. After installation, AURA runs fully offline. The only optional online feature is Telegram (so you can chat from anywhere on your phone). The AI model runs locally.

**Q: Will AURA slow down my phone?**
AURA uses ~200–400 MB RAM when idle. During inference (when it's thinking), it uses more, but the thermal management system automatically throttles when your phone gets warm.

**Q: Can I use AURA without Telegram?**
Telegram is currently the primary interface. Voice (STT/TTS) and a native Android app are planned for future releases.

**Q: What models does AURA support?**

| Model | Download Size | RAM Usage | Best For |
|---|---|---|---|
| Qwen3-1.7B Q4_K_M | ~2 GB | ~2 GB | Budget phones with <4 GB RAM |
| Qwen3-4B Q4_K_M | ~3 GB | ~4 GB | Mid-range phones |
| Qwen3-8B Q4_K_M | ~5 GB | ~6 GB | Flagship phones (recommended) |
| Qwen3-14B Q4_K_M | ~10 GB | ~10 GB | High-RAM tablets |

**Q: Is my data safe?**
Yes. AURA follows the "Anti-Cloud Absolute" principle: zero telemetry, zero analytics, zero cloud calls. Your data is encrypted on-device with AES-256-GCM. The vault uses Argon2id for key derivation.

**Q: How do I uninstall AURA?**
```bash
sv down aura-daemon 2>/dev/null
pkill -x aura-daemon 2>/dev/null
rm -rf ~/aura ~/.config/aura ~/.local/share/aura
rm -f /data/data/com.termux/files/usr/bin/aura-daemon
rm -f /data/data/com.termux/files/usr/bin/aura-neocortex
rm -rf /data/data/com.termux/files/usr/var/service/aura-daemon
```

---

## License

Proprietary — All Rights Reserved © 2026 Aditya Pagare

Personal use only. No redistribution, modification, or commercial use without explicit written permission.
