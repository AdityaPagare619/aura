# AURA v4 — Deployment Guide

Complete guide for building, installing, configuring, and verifying AURA on Android devices.

---

## Table of Contents

- [Prerequisites](#prerequisites)
- [Quick Install (Recommended)](#quick-install-recommended)
- [Manual Build from Source](#manual-build-from-source)
- [Installation Process](#installation-process)
- [Configuration](#configuration)
- [Health Check Verification](#health-check-verification)
- [Updating](#updating)
- [Troubleshooting](#troubleshooting)

---

## Prerequisites

### Hardware

| Requirement | Minimum | Recommended |
|---|---|---|
| **Architecture** | ARM64 (aarch64) | Any ARM64 device (2017+) |
| **RAM** | 4 GB | 8 GB+ |
| **Storage** | 8 GB free | 16 GB free |
| **Android** | 8.0 (API 26) | 12+ |

### Software

| Tool | Version | Purpose |
|---|---|---|
| [Termux](https://f-droid.org/en/packages/com.termux/) | Latest (from F-Droid) | Terminal emulator on Android |
| Rust toolchain | Stable (2026-03-18+) | Building from source |
| Android NDK | r26b | Cross-compilation (desktop builds only) |
| Git | Any | Source acquisition |
| cmake, build-essential, pkg-config | Any | Build dependencies |

> **Important:** Install Termux from [F-Droid](https://f-droid.org/en/packages/com.termux/), **not** the Google Play Store. The Play Store version is outdated and incompatible.

---

## Quick Install (Recommended)

The easiest way to deploy AURA is via the automated installer:

```bash
# 1. Grant storage access
termux-setup-storage

# 2. Download and run the installer
curl -fsSL https://raw.githubusercontent.com/AdityaPagare619/aura/main/install.sh -o install.sh
bash install.sh
```

The installer handles everything: packages, Rust toolchain, source clone, model download, compilation, configuration, and service setup.

### Installation Options

```bash
# Pre-built binaries (faster, no compilation)
bash install.sh --skip-build

# Specific model size
bash install.sh --model qwen3-4b

# Update existing installation
bash install.sh --update

# Dry run (preview actions)
bash install.sh --dry-run
```

See the [README](README.md#installation-options) for the full options table.

---

## Manual Build from Source

For developers or when the automated installer isn't suitable.

### Step 1: Set Up the Build Environment (Termux)

```bash
# Update packages
pkg update && pkg upgrade -y

# Install build dependencies
pkg install -y git rust cmake build-essential pkg-config libopus curl

# (Optional) Install Rust via rustup for pinned toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
```

### Step 2: Set Up the Build Environment (Desktop Cross-Compile)

```bash
# Install Rust (if not present)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y

# Add Android target
rustup target add aarch64-linux-android

# Download Android NDK r26b from:
# https://developer.android.com/ndk/downloads
# Then set the path:
export ANDROID_NDK_HOME="$HOME/Android/Sdk/ndk/26.1.10909125"
```

Configure `.cargo/config.toml`:

```toml
[target.aarch64-linux-android]
linker = "aarch64-linux-android26-clang"
ar = "llvm-ar"
```

### Step 3: Clone the Source

```bash
git clone https://github.com/AdityaPagare619/aura.git ~/aura
cd ~/aura
git submodule update --init --recursive
```

### Step 4: Build

**On-device (Termux):**

```bash
# Build with curl backend (Termux-compatible)
cargo build --release \
  -p aura-daemon \
  -p aura-neocortex \
  --features "aura-daemon/curl-backend,aura-llama-sys/stub"
```

**Cross-compile (desktop → Android):**

```bash
export AURA_COMPILE_LLAMA=true
cargo build --release \
  --target aarch64-linux-android \
  -p aura-daemon \
  -p aura-neocortex
```

**Desktop development (with stubs):**

```bash
cargo check --workspace --features "aura-llama-sys/stub,aura-daemon/voice"
cargo test --workspace --features "aura-llama-sys/stub,aura-daemon/voice"
```

### Step 5: Download the AI Model

```bash
# Auto-detect model based on device RAM
mkdir -p ~/.local/share/aura/models

# Qwen3-8B (recommended, ~5 GB)
curl -L -o ~/.local/share/aura/models/Qwen3-8B-Q4_K_M.gguf \
  "https://huggingface.co/Qwen/Qwen3-8B-GGUF/resolve/main/Qwen3-8B-Q4_K_M.gguf"

# Or smaller models:
# Qwen3-4B (~3 GB) — for 4–6 GB RAM devices
# Qwen3-1.7B (~2 GB) — for <4 GB RAM devices
```

### Step 6: Install Binaries

```bash
# On-device (Termux)
cp target/release/aura-daemon $PREFIX/bin/
cp target/release/aura-neocortex $PREFIX/bin/
chmod +x $PREFIX/bin/aura-daemon $PREFIX/bin/aura-neocortex

# Cross-compiled
cp target/aarch64-linux-android/release/aura-daemon $PREFIX/bin/
cp target/aarch64-linux-android/release/aura-neocortex $PREFIX/bin/
```

---

## Installation Process

The automated installer runs 13 phases. Understanding these helps with troubleshooting.

| Phase | Name | Interactive? | Description |
|---|---|---|---|
| 0 | Pre-flight | No | Checks architecture, Android version, Termux |
| 0.5 | Space budget | No | Displays RAM/storage and model size table |
| 1 | Hardware + Model | **Yes** | Auto-selects AI model, asks confirmation |
| 2 | Telegram wizard | **Yes** | Guides bot creation via @BotFather |
| 3 | Vault & Identity | **Yes** | Sets username and vault PIN |
| 4 | Packages | No | Installs build tools via `pkg` |
| 5 | Rust toolchain | No | Installs Rust stable via rustup |
| 6 | Source | No | Clones AURA repo + llama.cpp submodule |
| 7 | Model download | No | Downloads GGUF model (resumable) |
| 8 | Build | No | Compiles aura-daemon + aura-neocortex |
| 9 | Purge | No | Removes build tools (~4 GB freed) |
| 10 | Config | No | Writes `config.toml` with all settings |
| 11 | Service | No | Sets up auto-start via termux-services |
| 12 | Verify | No | Health checks + success banner |

### File Layout After Installation

```
/data/data/com.termux/files/
├── home/
│   ├── aura/                          # Source code
│   └── .config/aura/
│       └── config.toml                # Configuration
├── usr/
│   ├── bin/
│   │   ├── aura-daemon                # Main process
│   │   └── aura-neocortex             # LLM process
│   └── var/service/
│       └── aura-daemon/               # Service definition
└── home/.local/share/aura/
    ├── models/                        # GGUF model files
    ├── db/                            # SQLite database
    ├── logs/                          # Runtime logs
    └── checkpoints/                   # State checkpoints
```

---

## Configuration

### Config File Location

```
~/.config/aura/config.toml
```

### Config Load Order

1. Compiled-in Rust defaults (`Default::default()`)
2. `~/.config/aura/config.toml` — this file
3. `AURA_*` environment variables (highest priority)

### Essential Configuration

Edit `config.toml` after installation:

```toml
[daemon]
data_dir = "/data/local/tmp/aura"

[neocortex]
model_dir = "/data/local/tmp/aura/models"
n_threads = 4                     # Half your CPU core count
default_n_ctx = 4096              # Context window (increase on 8GB+ devices)

[sqlite]
db_path = "/data/local/tmp/aura/db/aura.db"

[telegram]
bot_token = "YOUR_BOT_TOKEN_HERE"
allowed_chat_ids = [YOUR_CHAT_ID_HERE]
poll_interval_ms = 2000
```

See [`aura-config.example.toml`](aura-config.example.toml) for all available options with detailed comments.

### Environment Variable Overrides

For sensitive values, use environment variables instead of config file entries:

```bash
# In Termux .bashrc or profile
export AURA_TELEGRAM_TOKEN="123456789:ABCdef..."
export AURA_DATA_DIR="/data/local/tmp/aura"
export AURA_DB_PATH="$AURA_DATA_DIR/db/aura.db"
```

See [ENVIRONMENT-VARIABLES.md](ENVIRONMENT-VARIABLES.md) for the complete reference.

---

## Health Check Verification

After installation, verify everything works:

### 1. Binary Check

```bash
# Verify binaries are installed
which aura-daemon && echo "✅ aura-daemon found" || echo "❌ aura-daemon missing"
which aura-neocortex && echo "✅ aura-neocortex found" || echo "❌ aura-neocortex missing"
```

### 2. Config Validation

```bash
# Verify config file exists and is valid TOML
test -f ~/.config/aura/config.toml && echo "✅ config exists" || echo "❌ config missing"

# Check Telegram configuration
grep -q 'bot_token = ".\+' ~/.config/aura/config.toml && echo "✅ bot token set" || echo "❌ bot token empty"
```

### 3. Model Check

```bash
# Verify model file exists
ls -lh ~/.local/share/aura/models/*.gguf 2>/dev/null && echo "✅ model found" || echo "❌ no model file"
```

### 4. Database Check

```bash
# Verify database directory
test -d ~/.local/share/aura/db && echo "✅ db dir exists" || echo "❌ db dir missing"
```

### 5. Daemon Startup

```bash
# Start the daemon
aura-daemon --config ~/.config/aura/config.toml &

# Wait a moment, then check
sleep 3
pgrep -x aura-daemon && echo "✅ daemon running" || echo "❌ daemon not running"
```

### 6. Log Verification

```bash
# Check for errors in the log
tail -20 ~/.local/share/aura/logs/current

# Look for "ready" or "started" messages
grep -i "ready\|started\|listening" ~/.local/share/aura/logs/current
```

### 7. Telegram Bot Test

1. Open Telegram
2. Find your bot
3. Send `/start`
4. AURA should respond within a few seconds

### 8. Full Diagnostic Script

```bash
# Run the built-in verification
bash test-aura.sh
```

---

## Updating

### Automated Update

```bash
bash install.sh --update
```

This pulls the latest code, rebuilds, and preserves your config, memories, and data.

### Manual Update

```bash
cd ~/aura
git pull origin main
git submodule update --init --recursive
cargo build --release -p aura-daemon -p aura-neocortex

# Reinstall binaries
cp target/release/aura-daemon $PREFIX/bin/
cp target/release/aura-neocortex $PREFIX/bin/

# Restart
sv restart aura-daemon
# or
pkill -x aura-daemon && aura-daemon --config ~/.config/aura/config.toml &
```

### Repair Specific Phases

```bash
# Re-download model (if corrupted or interrupted)
bash install.sh --repair model

# Rebuild only
bash install.sh --repair build
```

---

## Linux Systemd Deployment

For running AURA on Linux servers or desktops (not Android):

### Prerequisites

- Linux x86_64 or ARM64
- 4 GB+ RAM
- Rust toolchain

### Installation

```bash
# Clone and build
git clone https://github.com/AdityaPagare619/aura.git ~/aura
cd ~/aura
git submodule update --init --recursive

# Build (uses reqwest backend for Linux)
cargo build --release -p aura-daemon -p aura-neocortex

# Install binaries
sudo cp target/release/aura-daemon /usr/local/bin/
sudo cp target/release/aura-neocortex /usr/local/bin/

# Create data directories
sudo mkdir -p /var/lib/aura/{models,db,logs,checkpoints}
sudo mkdir -p /etc/aura

# Copy config
sudo cp config.toml /etc/aura/config.toml
sudo chmod 600 /etc/aura/config.toml
```

### Systemd Service

Create `/etc/systemd/system/aura-daemon.service`:

```ini
[Unit]
Description=AURA Autonomous Agent
After=network.target
Wants=network.target

[Service]
Type=simple
User=aura
Group=aura
ExecStart=/usr/local/bin/aura-daemon --config /etc/aura/config.toml
Restart=on-failure
RestartSec=5
WorkingDirectory=/var/lib/aura

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/aura

# Resource limits
MemoryMax=2G
CPUQuota=50%

[Install]
WantedBy=multi-user.target
```

### Enable and Start

```bash
# Create service user
sudo useradd -r -s /bin/false aura
sudo chown -R aura:aura /var/lib/aura

# Enable and start
sudo systemctl daemon-reload
sudo systemctl enable aura-daemon
sudo systemctl start aura-daemon

# Check status
sudo systemctl status aura-daemon
sudo journalctl -u aura-daemon -f
```

---

## Docker Deployment

For containerized deployment or testing:

### Dockerfile

```dockerfile
FROM rust:latest AS builder

WORKDIR /app
COPY . .
RUN git submodule update --init --recursive
RUN cargo build --release -p aura-daemon -p aura-neocortex \
    --features "aura-llama-sys/stub"

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y \
    libssl3 ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/aura-daemon /usr/local/bin/
COPY --from=builder /app/target/release/aura-neocortex /usr/local/bin/

VOLUME /var/lib/aura
EXPOSE 19400

CMD ["aura-daemon", "--config", "/etc/aura/config.toml"]
```

### Build and Run

```bash
# Build image
docker build -t aura:latest .

# Run container
docker run -d \
  --name aura \
  -v aura-data:/var/lib/aura \
  -v ./config.toml:/etc/aura/config.toml:ro \
  -p 19400:19400 \
  aura:latest

# Check logs
docker logs -f aura
```

### Docker Compose

```yaml
version: '3.8'
services:
  aura:
    build: .
    container_name: aura
    volumes:
      - aura-data:/var/lib/aura
      - ./config.toml:/etc/aura/config.toml:ro
    ports:
      - "19400:19400"
    restart: unless-stopped
    environment:
      - AURA_DATA_DIR=/var/lib/aura
      - AURA_MODELS_PATH=/var/lib/aura/models

volumes:
  aura-data:
```

---

## Android App Integration

AURA can be integrated with a native Android app via JNI:

### App Setup

1. Add AURA as a dependency in your Android project
2. Load the native library:

```kotlin
class AuraService {
    companion object {
        init {
            System.loadLibrary("aura_daemon")
        }
        
        external fun nativeInit(): Long
        external fun nativeRun(statePtr: Long)
        external fun nativeShutdown(statePtr: Long)
    }
    
    private var statePtr: Long = 0
    
    fun start() {
        statePtr = nativeInit()
        Thread {
            nativeRun(statePtr)
        }.start()
    }
    
    fun stop() {
        nativeShutdown(statePtr)
        statePtr = 0
    }
}
```

### Service Configuration

In `AndroidManifest.xml`:

```xml
<service
    android:name=".AuraService"
    android:exported="false"
    android:process=":aura" />
```

### Permissions

```xml
<uses-permission android:name="android.permission.INTERNET" />
<uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
<uses-permission android:name="android.permission.WAKE_LOCK" />
```

---

## Rollback Procedures

### Pre-Deployment Backup

Always create a backup before major updates:

```bash
# Backup current installation
BACKUP_DIR="$HOME/.aura-backup-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$BACKUP_DIR"

# Backup config and data
cp -r ~/.config/aura "$BACKUP_DIR/config"
cp -r ~/.local/share/aura/db "$BACKUP_DIR/db"
cp -r ~/.local/share/aura/models "$BACKUP_DIR/models"

# Backup binaries
cp $PREFIX/bin/aura-daemon "$BACKUP_DIR/"
cp $PREFIX/bin/aura-neocortex "$BACKUP_DIR/"

echo "Backup created at: $BACKUP_DIR"
```

### Rollback Steps

If deployment fails or causes issues:

```bash
# 1. Stop AURA
sv down aura-daemon 2>/dev/null
pkill -x aura-daemon 2>/dev/null

# 2. Restore from backup
BACKUP_DIR="$HOME/.aura-backup-YYYYMMDD-HHMMSS"  # Use your backup path

# Restore binaries
cp "$BACKUP_DIR/aura-daemon" $PREFIX/bin/
cp "$BACKUP_DIR/aura-neocortex" $PREFIX/bin/
chmod +x $PREFIX/bin/aura-daemon $PREFIX/bin/aura-neocortex

# Restore config (if needed)
cp -r "$BACKUP_DIR/config/"* ~/.config/aura/

# Restore data (if needed)
cp -r "$BACKUP_DIR/db/"* ~/.local/share/aura/db/
cp -r "$BACKUP_DIR/models/"* ~/.local/share/aura/models/

# 3. Restart
sv up aura-daemon 2>/dev/null
aura-daemon --config ~/.config/aura/config.toml &
```

### Emergency Recovery

If AURA won't start after update:

```bash
# 1. Check logs for errors
tail -50 ~/.local/share/aura/logs/current

# 2. Try repair mode
bash install.sh --repair build
bash install.sh --repair model

# 3. Nuclear option: clean reinstall
bash install.sh --update
```

---

## Troubleshooting

### Installation Issues

| Problem | Solution |
|---|---|
| `curl \| bash` crashes | Use two-step method: download first, then `bash install.sh` |
| Build too hot / slow | Use `--skip-build` for pre-built binaries |
| Not enough storage | Use `--model qwen3-1.7b` (~2 GB) |
| Model download fails | `bash install.sh --repair model` |
| `rustls-platform-verifier` panic | Re-download `install.sh` or set `export RUSTUP_USE_CURL=1` |
| `$HOME` mismatch (Termux) | Installer exports `CARGO_HOME` and `RUSTUP_HOME` automatically |
| `pkg` conflicts with Rust | `pkg uninstall -y rust` then `bash install.sh --repair build` |

### Runtime Issues

| Problem | Solution |
|---|---|
| AURA stops on screen lock | Enable Termux wakelock: `termux-wake-lock` |
| Telegram bot unresponsive | Check `pgrep -x aura-daemon`; review logs with `tail -20 ~/.local/share/aura/logs/current` |
| High memory usage | Reduce `n_ctx` in config; use smaller model |
| IPC connection refused | Verify `AURA_NEOCORTEX_BIN` path; check `aura-neocortex` is executable |
| Database locked | Stop daemon; check for stale WAL files in db directory |
| Thermal throttling | Let device cool; AURA will automatically resume |
| Ethics violation logged | Check logs; action was blocked by Iron Laws |

### Build Issues

| Problem | Solution |
|---|---|
| `compile_error!("Features curl-backend and reqwest are mutually exclusive")` | Add `--features aura-daemon/curl-backend` for Termux |
| NDK linker not found | Set `ANDROID_NDK_HOME` to your NDK installation |
| `aura-llama-sys` build fails | Use `--features aura-llama-sys/stub` for development |
| Tests fail with SIGSEGV | Ensure `panic = "unwind"` and `lto = "thin"` in release profile |
| JNI build errors | Check NDK version (must be r26b) |

### Security Issues (From Audit)

| Problem | Solution |
|---|---|
| Bot token exposed in config | Revoke token immediately; use `AURA_TELEGRAM_TOKEN` env var instead |
| Path traversal warning | Ensure model paths are canonicalized |
| IPC authentication failure | Regenerate session tokens; check CSPRNG |
| Rate limit exceeded | Increase `rate_limit_actions_per_min` in config |

### Log Locations

| Log | Path |
|---|---|
| Daemon runtime | `~/.local/share/aura/logs/current` |
| Install log | `~/.aura-install.log` (or shown on failure) |
| Termux service | `~/.local/share/aura/logs/aura-daemon/current` |
| Systemd (Linux) | `sudo journalctl -u aura-daemon` |

---

## Uninstallation

### Termux/Android

```bash
# Stop services
sv down aura-daemon 2>/dev/null
pkill -x aura-daemon 2>/dev/null

# Remove everything
rm -rf ~/aura ~/.config/aura ~/.local/share/aura
rm -f $PREFIX/bin/aura-daemon $PREFIX/bin/aura-neocortex
rm -rf $PREFIX/var/service/aura-daemon
```

### Linux Systemd

```bash
# Stop and disable service
sudo systemctl stop aura-daemon
sudo systemctl disable aura-daemon

# Remove service file
sudo rm /etc/systemd/system/aura-daemon.service
sudo systemctl daemon-reload

# Remove binaries and data
sudo rm /usr/local/bin/aura-daemon /usr/local/bin/aura-neocortex
sudo rm -rf /var/lib/aura /etc/aura

# Remove service user
sudo userdel aura
```

### Docker

```bash
# Stop and remove container
docker stop aura
docker rm aura

# Remove image
docker rmi aura:latest

# Remove volume (WARNING: deletes all data)
docker volume rm aura-data
```
