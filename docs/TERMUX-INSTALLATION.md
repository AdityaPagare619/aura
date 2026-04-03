# AURA — Termux Installation Guide

**Your Personal Local AGI — Private, On-Device, No Cloud**

**Version:** 4.0.0-merged
**Platform:** Termux (Linux on Android)
**Primary Interface:** Telegram

---

## Overview

This guide walks you through installing AURA on your Android device using Termux. AURA runs entirely on-device — no cloud, no API keys, no data leaving your phone. Your Telegram bot is the main interface.

**Estimated time:** 30-60 minutes (mostly build time)
**Storage needed:** 4-8 GB (temporary, ~4 GB freed after build)
**Minimum RAM:** 3 GB (auto-selects model based on your device)

---

## Step 1: Install Termux

### Download from F-Droid (NOT Play Store)

The Play Store version of Termux is outdated and broken. Always use F-Droid:

1. Install F-Droid: https://f-droid.org/
2. Open F-Droid → search "Termux"
3. Install **Termux** and **Termux:API**

> **Termux:API** is needed for hardware access (microphone, wakelock, battery info).

### Initial Setup

Open Termux and run:

```bash
# Update packages
pkg update && pkg upgrade

# Grant storage access (allows access to /sdcard)
termux-setup-storage

# Verify you're on aarch64 (ARM64)
uname -m
# Should output: aarch64
```

### Enable Wakelock (Critical!)

Android will kill background processes to save battery. You MUST enable wakelock:

1. Swipe down from the top of your screen
2. Find the Termux notification
3. Long-press it
4. Tap "Wakelock"

This prevents Android from sleeping Termux. Without this, AURA will be killed within minutes.

---

## Step 2: Install AURA

### Option A: One-Command Install (Recommended)

```bash
# Clone the repository
git clone https://github.com/AdityaPagare619/aura.git
cd aura

# Run the installer
bash install.sh
```

The installer will:
1. **Profile your hardware** — detect RAM, CPU, select optimal model
2. **Set up Telegram** — interactive bot token wizard with live verification
3. **Set up Vault PIN** — gates sensitive operations
4. **Install packages** — build tools, Rust, OpenSSL, etc.
5. **Download model** — resumable, SHA256 verified
6. **Build from source** — cargo build --release (10-30 min)
7. **Configure** — generate full config.toml
8. **Set up service** — termux-services for auto-start

### Option B: Manual Install

If you prefer control over each step:

#### 2a. Install Dependencies

```bash
pkg install build-essential git curl openssl cmake ninja libopus termux-services coreutils
```

#### 2b. Install Rust Toolchain

```bash
# Termux-specific workaround for TLS issues
export RUSTUP_USE_CURL=1
export RUSTUP_INIT_SKIP_PATH_CHECK=yes

# Install rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly --profile minimal

# Source cargo environment
source ~/.cargo/env

# Verify
rustc --version
cargo --version
```

#### 2c. Clone and Build

```bash
git clone https://github.com/AdityaPagare619/aura.git
cd aura
git submodule update --init --recursive

# Build (this takes 10-30 minutes on-device)
cargo build --release --features "aura-daemon/voice"

# Install binaries
cp target/release/aura-daemon $PREFIX/bin/
cp target/release/aura-neocortex $PREFIX/bin/
chmod +x $PREFIX/bin/aura-daemon $PREFIX/bin/aura-neocortex
```

#### 2d. Download Model

```bash
mkdir -p ~/.local/share/aura/models

# Example: Qwen3-8B Q4_K_M (for 6+ GB RAM devices)
cd ~/.local/share/aura/models
curl -L -o Qwen3-8B-Q4_K_M.gguf \
  "https://huggingface.co/Qwen/Qwen3-8B-GGUF/resolve/main/Qwen3-8B-Q4_K_M.gguf"

# Verify GGUF magic bytes
head -c4 Qwen3-8B-Q4_K_M.gguf | od -A n -t x1
# Should start with: 47 47 55 46 (GGUF)
```

#### 2e. Create Config

```bash
mkdir -p ~/.config/aura
cat > ~/.config/aura/config.toml << 'EOF'
[daemon]
data_dir = "~/.local/share/aura"
log_level = "info"

[telegram]
enabled = true
bot_token = "YOUR_BOT_TOKEN_HERE"
allowed_chat_ids = [YOUR_TELEGRAM_USER_ID]

[neocortex]
model_dir = "~/.local/share/aura/models"
default_model_name = "Qwen3-8B-Q4_K_M.gguf"
n_threads = 4

[identity]
user_name = "YourName"
assistant_name = "AURA"

[sqlite]
db_path = "~/.local/share/aura/db/aura.db"
EOF

chmod 600 ~/.config/aura/config.toml
```

---

## Step 3: Configure Telegram Bot

AURA communicates through Telegram. You need a bot token:

### Get a Bot Token

1. Open Telegram → search **@BotFather**
2. Send `/newbot`
3. Follow prompts — choose a name and username
4. BotFather gives you a token: `1234567890:ABCDefGhIJKlmNoPQRsTUVwxyZ`

### Get Your User ID

1. Open Telegram → search **@userinfobot**
2. Send `/start`
3. It replies with your numeric ID (e.g., `987654321`)

### Update Config

```bash
nano ~/.config/aura/config.toml
```

Set:
- `bot_token = "your-token-here"`
- `allowed_chat_ids = [your-user-id]`

---

## Step 4: Start AURA

### Method 1: Manual Start

```bash
aura-daemon
```

### Method 2: Service (Auto-Start on Termux Boot)

```bash
# Enable the service
sv-enable aura-daemon

# Start it
sv up aura-daemon

# Check status
sv status aura-daemon
```

### Method 3: Using Helper Script

```bash
bash start-aura.sh
```

### Verify It's Working

```bash
# Check status
bash status-aura.sh

# Full verification
bash verify.sh

# Quick health check
bash deploy/health-check.sh
```

### Test Telegram

1. Open Telegram
2. Find your bot (search the username you created)
3. Send `/start`
4. AURA should respond

---

## Step 5: Daily Operation

### Start AURA

```bash
# If using termux-services
sv up aura-daemon

# Or manually
aura-daemon
```

### Check Status

```bash
bash status-aura.sh
bash deploy/health-check.sh
```

### View Logs

```bash
tail -f ~/.local/share/aura/logs/daemon.log
```

### Stop AURA

```bash
bash stop-aura.sh
```

### Update AURA

```bash
cd ~/aura
git pull
bash install.sh --update
```

### Rollback (If Update Breaks Something)

```bash
bash rollback-aura.sh
```

---

## Troubleshooting

### Build Fails

**Problem:** `cargo build` fails with linker errors

```bash
# Fix: ensure lld is installed
pkg install lld

# Clean and rebuild
cargo clean
cargo build --release --features "aura-daemon/voice"
```

### "CANNOT LINK EXECUTABLE"

**Problem:** Binary won't start, says "library not found"

```bash
# Install missing shared libraries
pkg install libc++

# Verify binary architecture
file $PREFIX/bin/aura-daemon
# Should say: ELF 64-bit LSB shared object, ARM aarch64
```

### Daemon Crashes Immediately

**Problem:** AURA starts then dies within seconds

```bash
# Check logs
tail -50 ~/.local/share/aura/logs/daemon.log

# Common causes:
# 1. Not enough RAM — try smaller model (--model qwen3-4b)
# 2. Config file missing — run install.sh
# 3. Model file corrupted — re-download with --repair model
```

### Telegram Not Responding

**Problem:** Bot doesn't respond to messages

```bash
# Verify bot token
grep bot_token ~/.config/aura/config.toml

# Test token manually
curl -s "https://api.telegram.org/botYOUR_TOKEN/getMe"
# Should return: {"ok":true,"result":{"id":...,"username":"..."}}

# Check if daemon is running
pgrep -f aura-daemon

# Restart daemon
bash restart-aura.sh
```

### Android Keeps Killing AURA

**Problem:** AURA stops after a few minutes

1. **Enable wakelock:** Swipe down → hold Termux notification → Wakelock
2. **Disable battery optimization:** Settings → Apps → Termux → Battery → Unrestricted
3. **Keep Termux in recent apps:** Don't swipe it away

### Low Storage

**Problem:** Not enough space for build + model

```bash
# Check available space
df -h ~

# Clean up after build (installer does this automatically)
cargo clean
rm -rf target/

# If you already have Rust installed and don't need it for rebuilding:
rustup self uninstall -y
```

### Model Download Fails

**Problem:** curl fails or downloads HTML instead of model

```bash
# Retry just the model download
bash install.sh --repair model

# If rate-limited by HuggingFace, use a token:
HF_TOKEN=your_token_here bash install.sh --repair model

# Or download manually:
cd ~/.local/share/aura/models
curl -L -o model.gguf "https://huggingface.co/..."
```

---

## File Structure

```
~/.config/aura/
└── config.toml              # Main configuration

~/.local/share/aura/
├── models/
│   └── Qwen3-8B-Q4_K_M.gguf # AI model (auto-selected by RAM)
├── db/
│   └── aura.db              # SQLite database (memories, episodes)
├── logs/
│   └── daemon.log           # Runtime logs
└── aura-daemon.pid          # PID file

$PREFIX/bin/
├── aura-daemon              # Main daemon binary
└── aura-neocortex           # LLM inference binary

~/aura/                      # Source code (can be deleted after build)
├── Cargo.toml
├── install.sh
├── start-aura.sh
├── stop-aura.sh
├── verify.sh
└── ...
```

---

## Command Reference

### Installer Options

```bash
bash install.sh                          # Standard install
bash install.sh --model qwen3-4b         # Force specific model
bash install.sh --skip-build             # Use pre-built binary
bash install.sh --skip-model             # Skip model download
bash install.sh --update                 # Update existing install
bash install.sh --repair model           # Re-run just model download
bash install.sh --dry-run                # Preview what would happen
bash install.sh --keep-build-tools       # Don't purge Rust after build
```

### Service Management

```bash
sv up aura-daemon                        # Start service
sv down aura-daemon                      # Stop service
sv status aura-daemon                    # Check status
sv-enable aura-daemon                    # Enable auto-start
sv-disable aura-daemon                   # Disable auto-start
```

### Diagnostics

```bash
bash verify.sh                           # Full verification
bash verify.sh --quick                   # Quick checks only
bash deploy/health-check.sh              # Runtime health
bash deploy/health-check.sh --json       # JSON output
bash status-aura.sh                      # Process status
bash monitor-aura.sh --watch             # Live monitoring
bash rollback-aura.sh                    # Rollback to previous version
```

---

*For the full installation guide including APK setup, see [INSTALLATION.md](./INSTALLATION.md).*
