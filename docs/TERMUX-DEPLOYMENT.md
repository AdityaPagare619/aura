# AURA v4 — Termux Deployment Guide

> **Status:** Current Deployment Model | **Updated:** 2026-03-30
>
> AURA v4 is **Termux-based**, NOT an APK or Play Store application.

---

## 1. Overview

AURA v4 runs exclusively inside **Termux** on Android devices. There is no APK, no Play Store distribution, and no Android app installation.

### What This Means

| Traditional Android App | AURA v4 (Termux) |
|------------------------|-------------------|
| Download from Play Store | Install Termux, run scripts |
| APK file installation | Bash script installation |
| Android Service lifecycle | termux-services (runit-based) |
| JNI native library loading | Native ELF binaries |
| Google Play policies | F-Droid / GitHub distribution |

---

## 2. Installation

### Prerequisites

- Android 10+ (Termux requirement)
- ARM64 (aarch64) architecture
- 4-8 GB RAM (for LLM inference)
- 8-16 GB free storage (for model files)

### Installation Steps

```bash
# 1. Install Termux (from F-Droid or GitHub)
# https://github.com/termux/termux-app/releases

# 2. Open Termux and clone/download AURA
git clone https://github.com/your-repo/aura-v4.git ~/aura

# 3. Run the installer
cd ~/aura
bash install.sh

# 4. Follow prompts:
#    - Enter Telegram bot token (optional)
#    - Configure model selection
#    - Set vault PIN

# 5. Service starts automatically
```

### What install.sh Does

1. **Pre-flight checks** — architecture, storage, permissions
2. **Package installation** — build-essential, git, cmake, rust, termux-services
3. **Source acquisition** — clone or pull repo
4. **Model download** — GGUF model to `~/.local/share/aura/models/`
5. **Build** — compile aura-daemon and aura-neocortex for Termux
6. **Configuration** — create `~/.config/aura/config.toml`
7. **Service setup** — configure termux-services for auto-start

---

## 3. Service Management

### termux-services

AURA uses **termux-services** (runit-based) for service management:

```bash
# Check service status
sv status aura-daemon

# Start service
sv start aura-daemon

# Restart service
sv restart aura-daemon

# Stop service
sv stop aura-daemon

# View logs
cat ~/.local/share/aura/logs/current
```

### Auto-start

When termux-services is enabled, AURA daemon starts automatically when Termux opens.

### Alternative: ~/.bashrc

If termux-services is unavailable, the installer adds auto-start to `~/.bashrc`:

```bash
# Added by install.sh
if ! pgrep -x aura-daemon > /dev/null; then
    aura-daemon --config ~/.config/aura/config.toml &
    disown
fi
```

---

## 4. Architecture

### Two-Process Model

```
┌─────────────────────────────────────────┐
│           Android OS                     │
│                                          │
│  ┌─────────────────────────────────┐    │
│  │       Termux App                │    │
│  │                                 │    │
│  │  ┌─────────────────────────┐   │    │
│  │  │ aura-daemon (PID 1)     │   │    │
│  │  │ - Event loop            │   │    │
│  │  │ - BDI agent logic       │   │    │
│  │  │ - Memory system         │   │    │
│  │  │ - Tool execution        │   │    │
│  │  └───────────┬─────────────┘   │    │
│  │              │                 │    │
│  │              │ Unix socket     │    │
│  │              ▼                 │    │
│  │  ┌─────────────────────────┐   │    │
│  │  │ aura-neocortex (PID 2)  │   │    │
│  │  │ - llama.cpp inference   │   │    │
│  │  │ - GGUF model loading    │   │    │
│  │  └─────────────────────────┘   │    │
│  └─────────────────────────────────┘    │
└─────────────────────────────────────────┘
```

### Process Separation

| Process | Purpose | Lifecycle | Memory |
|---------|---------|-----------|--------|
| `aura-daemon` | Agent logic, memory, execution | Persistent | ~20-50 MB |
| `aura-neocortex` | LLM inference | On-demand | ~500 MB-2 GB |

The separation ensures that the LLM process (killed by Android LMK) doesn't take down the persistent daemon state.

---

## 5. File Locations

```
~/.local/share/aura/
├── models/
│   └── qwen3-8b-q4_k_m.gguf    # LLM model
├── db/
│   ├── episodic.sqlite          # Episodic memory
│   └── semantic.sqlite          # Semantic (vector) memory
└── logs/
    ├── daemon.log
    └── neocortex.log

~/.config/aura/
└── config.toml                  # User configuration

$PREFIX/var/service/aura-daemon/
├── run                          # Service run script
└── log/run                      # Log run script
```

---

## 6. Configuration

### config.toml

```toml
[daemon]
socket_path = "/data/data/com.termux/files/home/.local/share/aura/daemon.sock"
log_level = "info"

[neocortex]
model_path = "/data/data/com.termux/files/home/.local/share/aura/models/qwen3-8b-q4_k_m.gguf"
n_gpu_layers = 0          # 0 = CPU only (Termux)
context_size = 4096       # Tokens
n_threads = 4             # CPU threads

[memory]
max_episodic_entries = 10000

[identity]
assistant_name = "AURA"
warmth = 0.7
curiosity = 0.8
directness = 0.6
```

---

## 7. Troubleshooting

### Service Not Starting

```bash
# Check status
sv status aura-daemon

# View logs
cat ~/.local/share/aura/logs/current

# Check if binary exists
ls -la $PREFIX/bin/aura-daemon
```

### Model Not Found

```bash
# Verify model exists
ls -la ~/.local/share/aura/models/

# Check config path
grep model_path ~/.config/aura/config.toml
```

### Out of Memory

- Use smaller model (Qwen3-4B instead of 8B)
- Reduce context size in config.toml
- Close other apps

### Termux Package Issues

```bash
# After Android system update
pkg update && pkg upgrade

# Re-run installer if needed
bash ~/aura/install.sh
```

---

## 8. Updating

```bash
# Update via installer
bash install.sh --update

# Or manually
cd ~/aura
git pull
cargo build --release -p aura-daemon -p aura-neocortex
cp target/release/aura-daemon $PREFIX/bin/
cp target/release/aura-neocortex $PREFIX/bin/
sv restart aura-daemon
```

---

## 9. Uninstall

```bash
# Stop and disable service
sv disable aura-daemon

# Remove files
rm -rf ~/.local/share/aura/
rm -rf ~/.config/aura/
rm -f $PREFIX/bin/aura-*

# Optionally remove Termux
```

---

## 10. Frequently Asked Questions

### Q: Is there an APK?

**No.** AURA is Termux-only. There are no plans for an APK build.

### Q: Can I install from Play Store?

**No.** Termux is available from F-Droid or GitHub, not the Play Store.

### Q: Does it work offline?

**Yes.** Once installed with the model, AURA works completely offline.

### Q: What models are supported?

- Qwen3-4B-Q4_K_M (~2.8 GB) — Low RAM devices
- Qwen3-8B-Q4_K_M (~5.2 GB) — Default
- Qwen3-14B-Q4_K_M (~9.5 GB) — High-end devices

### Q: How do I get help?

- Check logs: `cat ~/.local/share/aura/logs/current`
- Run diagnostics: `aura doctor` (if available)
- Review this document and `docs/architecture/AURA-V4-INSTALLATION-AND-DEPLOYMENT.md`
