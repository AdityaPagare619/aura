# AURA Installation Guide

**Personal Local AGI — Private, On-Device, No Cloud**

**Version:** 4.0.0-merged
**Primary Platform:** Termux (Linux on Android)
**Secondary Platform:** Android APK (prebuilt binaries)
**Last Updated:** 2026-04-03

---

## What is AURA?

AURA is the world's first **private, local AGI** — not a chatbot, not enterprise SaaS. It runs entirely **on your device** with no cloud dependencies. Your data never leaves your phone.

**Main Interface:** Telegram bot
**Runtime:** Termux (Android) or standalone APK
**AI Models:** Qwen3 GGUF (1.7B / 4B / 8B / 14B — auto-selected by RAM)

---

## Prerequisites

### Minimum Device Requirements

| Requirement | Minimum | Recommended |
|-------------|---------|-------------|
| **Android Version** | 8.0 (API 26) | 12+ (API 31+) |
| **RAM** | 3 GB | 6 GB+ |
| **Storage** | 4 GB free | 8 GB free |
| **CPU** | ARM64 (aarch64) | Snapdragon 8-series / Tensor |
| **Termux** | Latest F-Droid build | Latest F-Droid build |

### Model Selection (Auto-Detected by RAM)

| Model | Size | Min RAM | Best For |
|-------|------|---------|----------|
| Qwen3-1.7B Q8_0 | ~2 GB | 3 GB | Low-end devices |
| Qwen3-4B Q4_K_M | ~3 GB | 4 GB | Budget / mid-range |
| Qwen3-8B Q4_K_M | ~5 GB | 6 GB | Flagship phones |
| Qwen3-14B Q4_K_M | ~9 GB | 10 GB | Tablets / high-RAM |

---

## Platform 1: Termux (Linux on Android) — PRIMARY

> **This is the primary and recommended installation method.** AURA is designed to run natively in Termux — a full Linux environment on your Android device.

### Quick Install (One Command)

```bash
bash install.sh
```

That's it. The installer handles everything: hardware profiling, model selection, Telegram setup, building, and service configuration.

### Detailed Installation

See [TERMUX-INSTALLATION.md](./TERMUX-INSTALLATION.md) for step-by-step instructions including:
- Termux installation from F-Droid
- Rust toolchain setup
- Manual build process
- Telegram bot configuration
- Wakelock setup for always-on operation
- Troubleshooting

### Installation Flow

```
┌─────────────────────────────────────────────────────────────┐
│                   TERMUX INSTALLATION                        │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  STEP 1: Install Termux from F-Droid                        │
│  ┌─────────────────────────────────────────────────────┐    │
│  │ https://f-droid.org/packages/com.termux/             │    │
│  │ Also install Termux:API for hardware access          │    │
│  │ https://f-droid.org/packages/com.termux.api/         │    │
│  └─────────────────────────────────────────────────────┘    │
│                         ↓                                   │
│  STEP 2: Run the Installer                                  │
│  ┌─────────────────────────────────────────────────────┐    │
│  │ $ bash install.sh                                    │    │
│  │                                                      │    │
│  │ Interactive setup (done once):                       │    │
│  │  ├─ Hardware profiling + model auto-selection        │    │
│  │  ├─ Telegram bot wizard (token verification)         │    │
│  │  └─ Vault PIN + identity setup                       │    │
│  │                                                      │    │
│  │ Unattended install:                                  │    │
│  │  ├─ Package installation (pkg install)               │    │
│  │  ├─ Rust toolchain (nightly)                         │    │
│  │  ├─ Source clone + submodules                        │    │
│  │  ├─ Model download (resumable, SHA256 verified)      │    │
│  │  ├─ Build (cargo build --release, 10-30 min)         │    │
│  │  ├─ Purge build tools (~4 GB freed)                  │    │
│  │  ├─ Config generation (config.toml)                  │    │
│  │  └─ Service setup (termux-services or .bashrc)       │    │
│  └─────────────────────────────────────────────────────┘    │
│                         ↓                                   │
│  STEP 3: Start AURA                                         │
│  ┌─────────────────────────────────────────────────────┐    │
│  │ $ sv up aura-daemon          # if termux-services    │    │
│  │ $ aura-daemon                # manual start          │    │
│  │                                                      │    │
│  │ Enable wakelock (critical!):                         │    │
│  │  Swipe down → hold Termux notification → Wakelock    │    │
│  └─────────────────────────────────────────────────────┘    │
│                         ↓                                   │
│  STEP 4: Chat with AURA on Telegram                         │
│  ┌─────────────────────────────────────────────────────┐    │
│  │ Open Telegram → message your bot → AURA responds     │    │
│  └─────────────────────────────────────────────────────┘    │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Helper Scripts

| Script | Purpose |
|--------|---------|
| `install.sh` | Full installation (build + models + config) |
| `start-aura.sh` | Start daemon |
| `stop-aura.sh` | Stop daemon (add `--stop-llama` to also stop llama-server) |
| `restart-aura.sh` | Restart daemon |
| `status-aura.sh` | Show running status |
| `monitor-aura.sh` | Continuous monitoring (add `--watch` for live) |
| `verify.sh` | Full installation verification |
| `deploy/health-check.sh` | Runtime health check (add `--json` for machine-readable) |
| `rollback-aura.sh` | Rollback to previous version |

### Termux-Specific Considerations

| Aspect | Detail |
|--------|--------|
| **Build time** | 10-30 minutes on-device (ARM compilation) |
| **Storage** | Models + build can reach 8+ GB temporarily. `termux-setup-storage` recommended. |
| **Battery** | Use `termux-wake-lock` to prevent Android from killing the daemon |
| **Audio** | Via Termux:API — limited echo cancellation |
| **Updates** | `bash install.sh --update` or `git pull && cargo build --release` |
| **Config** | `~/.config/aura/config.toml` — never overwritten by installer after first run |

---

## Platform 2: Android APK — SECONDARY

> The APK is a prebuilt binary wrapper. It still requires Termux for the full AURA experience. The APK provides a native Android UI layer on top of the Termux daemon.

### Installation

1. Go to the AURA GitHub Releases page
2. Download the latest `aura-v4.x.x.apk`
3. Enable "Install unknown apps" for your file manager/browser
4. Install the APK
5. On first launch, AURA will guide you through model download

### APK Architecture

The APK is a thin Android shell around the Termux daemon:
- **Foreground Service:** Keeps the daemon alive
- **Model Management:** Downloads and manages GGUF models
- **UI Layer:** Native Android interface (optional — Telegram is the primary interface)
- **Permissions:** Minimal — only what's needed for the daemon to run

### Required Android Permissions

| Permission | Why | Can Deny? |
|------------|-----|-----------|
| `FOREGROUND_SERVICE` | Keep daemon running | No — required |
| `INTERNET` | Telegram bot API | No — required |
| `RECORD_AUDIO` | Voice interaction | Yes — text mode still works |
| `POST_NOTIFICATIONS` | Alerts, reminders | Yes |

---

## Configuration Reference

### `config.toml` Key Settings

```toml
[daemon]
data_dir                = "~/.local/share/aura"
log_level               = "info"

[telegram]
enabled         = true
bot_token       = "your-bot-token-from-botfather"
allowed_chat_ids = [your-telegram-user-id]

[neocortex]
model_dir              = "~/.local/share/aura/models"
default_model_name     = "Qwen3-8B-Q4_K_M.gguf"
n_threads              = 4

[identity]
user_name       = "YourName"
assistant_name  = "AURA"

[sqlite]
db_path           = "~/.local/share/aura/db/aura.db"
```

### File Locations

| Path | Purpose |
|------|---------|
| `~/.config/aura/config.toml` | Configuration |
| `~/.local/share/aura/models/` | GGUF model files |
| `~/.local/share/aura/db/aura.db` | SQLite database |
| `~/.local/share/aura/logs/` | Daemon logs |
| `$PREFIX/bin/aura-daemon` | Main binary |
| `$PREFIX/bin/aura-neocortex` | LLM inference binary |

---

## Troubleshooting

### Common Issues

| Problem | Solution |
|---------|----------|
| Termux build fails | Run `pkg upgrade` first. Ensure 4+ GB free storage. |
| Daemon crashes on start | Check `daemon.log` in data directory. Often: OOM (not enough RAM). |
| Telegram not responding | Check `config.toml` — bot token and chat IDs must be correct |
| Model download fails | Check network. Use `HF_TOKEN=your_token` if rate-limited. Re-run with `--repair model` |
| Android kills daemon | Enable wakelock: swipe down → hold Termux notification → Wakelock |
| "CANNOT LINK EXECUTABLE" | Missing shared library. Run `pkg install libc++` |
| Rust install fails on Termux | The installer handles this automatically with `RUSTUP_USE_CURL=1` |

### Diagnostic Commands

```bash
# Full verification
bash verify.sh

# Quick health check
bash deploy/health-check.sh

# Check daemon logs
tail -f ~/.local/share/aura/logs/daemon.log

# Verify installation
bash verify.sh --quick

# Rollback to previous version
bash rollback-aura.sh
```

### Getting Help

| Channel | Use For |
|---------|---------|
| GitHub Issues | Bug reports, feature requests |
| `/help` in Telegram | Command reference |
| `/status` in Telegram | System health check |
| `/debug dump all` | Full state dump for diagnostics |

---

## Uninstallation

### Termux
```bash
# Stop AURA
bash stop-aura.sh

# Remove AURA files
rm -rf ~/aura
rm -rf ~/.config/aura
rm -rf ~/.local/share/aura
rm -f $PREFIX/bin/aura-daemon $PREFIX/bin/aura-neocortex
```

### Android APK
1. Settings → Apps → AURA → Uninstall
2. Models are in app storage and will be deleted automatically
3. To preserve data: export via `/export` command first

---

*This document covers installation on all supported platforms. For detailed Termux setup, see [TERMUX-INSTALLATION.md](./TERMUX-INSTALLATION.md).*
