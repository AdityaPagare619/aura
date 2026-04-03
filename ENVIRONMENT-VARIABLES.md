# AURA v4 — Environment Variables Reference

Complete reference of all environment variables recognized by AURA's runtime, build system, and installer.

## Resolution Order

Every path in AURA follows this resolution order:

1. **Environment variable** — `AURA_*` override (highest priority)
2. **Platform default** — `#[cfg(target_os = "android")]` compiled-in default
3. **Fallback** — `dirs` crate / `$HOME` / `$PREFIX` (Termux)

---

## Runtime Variables

These variables affect `aura-daemon` and `aura-neocortex` at runtime. Set them before launching the processes.

### `AURA_HOME`

| | |
|---|---|
| **Purpose** | User home directory for AURA |
| **Used by** | `aura-daemon` (path resolution) |
| **Android default** | `$HOME` (Termux: `/data/data/com.termux/files/home`) |
| **Desktop default** | `dirs::home_dir()` (~ on Linux/macOS, `%USERPROFILE%` on Windows) |

```bash
# Example
export AURA_HOME="/data/data/com.termux/files/home"
```

### `AURA_DATA_DIR`

| | |
|---|---|
| **Purpose** | Root data directory for all AURA state (logs, checkpoints, models) |
| **Used by** | `aura-daemon` (config `daemon.data_dir`) |
| **Android default** | `/data/local/tmp/aura` |
| **Desktop default** | `~/.local/share/aura` (Linux), `~/Library/Application Support/aura` (macOS), `%APPDATA%/aura` (Windows) |

```bash
# Example
export AURA_DATA_DIR="/sdcard/aura-data"
```

> Subdirectories created under `AURA_DATA_DIR`:
> - `models/` — GGUF model files
> - `db/` — SQLite database
> - `logs/` — Runtime logs
> - `checkpoints/` — State checkpoints

### `AURA_MODELS_PATH`

| | |
|---|---|
| **Purpose** | Directory containing `.gguf` model files |
| **Used by** | `aura-daemon` (model discovery), `aura-neocortex` (model loading) |
| **Android default** | `/data/local/tmp/aura/models` |
| **Desktop default** | `<AURA_DATA_DIR>/models` |

```bash
# Example
export AURA_MODELS_PATH="/data/local/tmp/aura/models"
```

### `AURA_DB_PATH`

| | |
|---|---|
| **Purpose** | Path to the SQLite database file |
| **Used by** | `aura-daemon` (memory system, config `sqlite.db_path`) |
| **Android default** | `/data/data/com.aura/databases/aura.db` |
| **Desktop default** | `<AURA_DATA_DIR>/aura.db` |

```bash
# Example
export AURA_DB_PATH="/data/data/com.termux/files/home/.local/share/aura/db/aura.db"
```

### `AURA_NEOCORTEX_BIN`

| | |
|---|---|
| **Purpose** | Absolute path to the `aura-neocortex` binary |
| **Used by** | `aura-daemon` (IPC process spawning) |
| **Android default** | `/data/local/tmp/aura-neocortex` |
| **Desktop default** | `aura-neocortex` (expects binary on `$PATH`) |

```bash
# Example
export AURA_NEOCORTEX_BIN="/data/data/com.termux/files/usr/bin/aura-neocortex"
```

### `AURA_TELEGRAM_TOKEN`

| | |
|---|---|
| **Purpose** | Telegram bot token (overrides `config.toml` value) |
| **Used by** | `aura-daemon` (Telegram integration) |
| **Default** | Read from `[telegram].bot_token` in config |

```bash
# Example — prefer env var over config file for security
export AURA_TELEGRAM_TOKEN="123456789:ABCdefGHIjklMNOpqrSTUvwxYZ"
```

> **Security note:** Setting the token via environment variable is more secure than writing it to `config.toml`, as it doesn't persist to disk.

### `AURA_LOG_PATH`

| | |
|---|---|
| **Purpose** | Directory for log files |
| **Used by** | `aura-daemon` (tracing subscriber) |
| **Default** | `<AURA_DATA_DIR>/logs/` |

```bash
# Example
export AURA_LOG_PATH="/data/data/com.termux/files/home/.local/share/aura/logs"
```

### `AURA_CONFIG_PATH`

| | |
|---|---|
| **Purpose** | Path to the configuration file |
| **Used by** | `aura-daemon` (config loading) |
| **Android default** | `~/.config/aura/config.toml` (Termux) |
| **Desktop default** | `~/.config/aura/config.toml` |

```bash
# Example
export AURA_CONFIG_PATH="/data/data/com.termux/files/home/.config/aura/config.toml"
```

### `AURA_MODEL_DIR`

| | |
|---|---|
| **Purpose** | Alternative model directory (used in daemon core loop for neocortex IPC) |
| **Used by** | `aura-daemon::daemon_core::main_loop` |
| **Default** | Falls back to `AURA_MODELS_PATH` or config value |

### `AURA_STARTED`

| | |
|---|---|
| **Purpose** | Guard flag — prevents duplicate daemon launches |
| **Used by** | `install.sh`, shell profiles |
| **Default** | Unset (first launch) |
| **Values** | `1` = daemon already started in this shell session |

---

## Build-Time Variables

These variables affect `cargo build` and the `aura-llama-sys` build script.

### `AURA_COMPILE_LLAMA`

| | |
|---|---|
| **Purpose** | Control whether `aura-llama-sys` compiles real `llama.cpp` or uses stubs |
| **Used by** | `crates/aura-llama-sys/build.rs` |
| **Default** | Unset → stub mode (no native compilation) |
| **Values** | `"true"` = compile real llama.cpp; unset/other = stub mode |

```bash
# Desktop development (stub — no llama.cpp needed):
cargo check --workspace --features aura-llama-sys/stub

# Android build (real llama.cpp):
export AURA_COMPILE_LLAMA=true
cargo build --release --target aarch64-linux-android -p aura-neocortex
```

### `ANDROID_NDK_HOME` / `NDK_HOME` / `ANDROID_NDK_ROOT`

| | |
|---|---|
| **Purpose** | Path to the Android NDK installation |
| **Used by** | `aura-llama-sys/build.rs`, `.cargo/config.toml` |
| **Required for** | Android cross-compilation |
| **Recommended** | NDK r26b |

```bash
# Example
export ANDROID_NDK_HOME="$HOME/Android/Sdk/ndk/26.1.10909125"
```

> The build script checks all three variables in order: `NDK_HOME` → `ANDROID_NDK_HOME` → `ANDROID_NDK_ROOT`.

### `ANDROID_NDK_HOST_TAG`

| | |
|---|---|
| **Purpose** | NDK host platform tag for finding prebuilt tools |
| **Used by** | `aura-llama-sys/build.rs` |
| **Default** | `linux-x86_64` |

```bash
# macOS example
export ANDROID_NDK_HOST_TAG="darwin-x86_64"
```

---

## Installer Variables

These variables are read by `install.sh`.

### `AURA_REPO`

| | |
|---|---|
| **Purpose** | Override the git repository URL for installation |
| **Used by** | `install.sh` |
| **Default** | `https://github.com/AdityaPagare619/aura.git` |

```bash
# Fork installation
export AURA_REPO="https://github.com/myorg/aura-fork.git"
bash install.sh
```

### `AURA_ALLOW_UNVERIFIED_ARTIFACTS`

| | |
|---|---|
| **Purpose** | Emergency bypass — skip SHA256 verification for pre-built binaries |
| **Used by** | `install.sh` |
| **Default** | `0` (verification required) |
| **Values** | `1` = skip verification (NOT recommended for production) |

```bash
# Emergency only — bypasses checksum verification
export AURA_ALLOW_UNVERIFIED_ARTIFACTS=1
bash install.sh --skip-build
```

> **Warning:** This disables a critical security check. Only use for emergency testing when SHA256 metadata is temporarily missing.

### `HF_TOKEN`

| | |
|---|---|
| **Purpose** | HuggingFace authentication token for model downloads |
| **Used by** | `install.sh` (model download phase) |
| **Default** | Unset (anonymous downloads, may be rate-limited) |

```bash
# Get a free token at: https://huggingface.co/settings/tokens
export HF_TOKEN="hf_xxxxxxxxxxxxxxxxxxxxxxxx"
bash install.sh --repair model
```

---

## Platform-Specific Behavior Summary

| Variable | Android (Termux/APK) | Linux | macOS | Windows |
|---|---|---|---|---|
| `AURA_HOME` | `/data/data/com.termux/files/home` | `~` | `~` | `%USERPROFILE%` |
| `AURA_DATA_DIR` | `/data/local/tmp/aura` | `~/.local/share/aura` | `~/Library/Application Support/aura` | `%APPDATA%/aura` |
| `AURA_MODELS_PATH` | `/data/local/tmp/aura/models` | `<data_dir>/models` | `<data_dir>/models` | `<data_dir>/models` |
| `AURA_DB_PATH` | `/data/data/com.aura/databases/aura.db` | `<data_dir>/aura.db` | `<data_dir>/aura.db` | `<data_dir>/aura.db` |
| `AURA_NEOCORTEX_BIN` | `/data/local/tmp/aura-neocortex` | `aura-neocortex` (on PATH) | `aura-neocortex` (on PATH) | `aura-neocortex.exe` (on PATH) |

---

## Quick Reference

```bash
# Minimal runtime setup (Termux)
export AURA_DATA_DIR="/data/local/tmp/aura"
export AURA_MODELS_PATH="$AURA_DATA_DIR/models"
export AURA_DB_PATH="$AURA_DATA_DIR/db/aura.db"
export AURA_NEOCORTEX_BIN="/data/data/com.termux/files/usr/bin/aura-neocortex"
export AURA_CONFIG_PATH="$HOME/.config/aura/config.toml"

# Build setup (Android cross-compile)
export ANDROID_NDK_HOME="$HOME/ndk/26.1.10909125"
export AURA_COMPILE_LLAMA=true
```
