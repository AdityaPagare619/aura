# AURA v4 — Installation and Deployment Architecture

> **Canonical reference for all deployment, installation, and environment setup concerns.**
> Last updated: 2026-03-13

---

## 0. Purpose of This Document

This document specifies how AURA v4 is installed, deployed, and maintained on Android devices via
Termux. It covers the full installer lifecycle: from a fresh Termux environment to a running daemon
with local LLM inference.

This is not pseudocode. The design decisions here are implemented directly in `install.sh` at the
repository root.

---

## 1. OpenClaw Lessons Learned

OpenClaw (github.com/openclaw/openclaw) is a TypeScript-based personal AI assistant with ~309k
GitHub stars as of early 2026. It is not a Rust/Android project — it is a multi-channel AI gateway
running on Node.js. Despite the tech stack difference, its installation and onboarding quality is
exemplary and worth studying.

### 1.1 What OpenClaw Does Well

**Single-command install with guided wizard:**
```bash
npm install -g openclaw@latest
openclaw onboard --install-daemon
```
The `onboard` command walks users through every decision interactively. No raw config editing on
first run. AURA's `install.sh` adopts this principle: the script asks questions, not the user.

**`openclaw doctor` for health checks:**
OpenClaw ships a `doctor` command that audits config, permissions, and channel status. AURA
should have an equivalent: `aura doctor` (post-v4 milestone). The installer itself performs
doctor-like checks at install time.

**Daemon-as-a-service from day one:**
OpenClaw installs a launchd/systemd user service automatically during onboard. AURA does the
same via termux-services, so the daemon survives Termux restarts.

**Plugin/skills architecture via ClawHub:**
OpenClaw exposes a `plugin-sdk` with typed entry points per channel. Each skill is a SKILL.md
file in `~/.openclaw/workspace/skills/<skill>/`. AURA's equivalent is the trust tier +
tool registry system: tools are typed Rust trait objects loaded at daemon startup, not external
plugins. This is intentional — AURA is on-device, security-critical, and cannot load arbitrary
code.

**Versioned releases with update channels:**
OpenClaw has `stable`, `beta`, and `dev` channels with `openclaw update --channel <name>`.
AURA's installer supports this via a `--channel` flag (`stable` / `nightly`).

**What NOT to copy:**
- OpenClaw requires Node ≥22 and npm/pnpm — unsuitable for Termux-first Android
- OpenClaw is cloud-model-first (OpenAI, Anthropic) — AURA is local-model-only by design
- OpenClaw's security model assumes a LAN gateway — AURA's security model is on-device vault
- OpenClaw has no ARM-specific build concerns — AURA cross-compiles for aarch64-linux-android

### 1.2 Key Design Principles Adopted from OpenClaw

| OpenClaw Pattern | AURA Adaptation |
|---|---|
| `onboard --install-daemon` wizard | `install.sh` interactive prompts |
| `doctor` health check command | Pre-flight checks in installer |
| systemd user service autostart | termux-services sv-enable |
| Skills as filesystem files | Config at `~/.config/aura/config.toml` |
| Versioned release channels | `--channel stable\|nightly` flag |
| Clear error messages with fix hints | `die()` function with remediation text |
| Progress indicators during long ops | `curl --progress-bar` + spinner |

---

## 2. Termux Environment Requirements

### 2.1 What Termux Provides

Termux is a terminal emulator + Linux environment for Android. It provides:
- A Debian-like package manager (`pkg` / `apt`)
- A userland Linux environment at `/data/data/com.termux/files/usr/`
- No root required
- Persistent storage at `$HOME` = `/data/data/com.termux/files/home/`
- Storage access via `termux-setup-storage` → `~/storage/`

### 2.2 Minimum Requirements

| Requirement | Minimum | Recommended |
|---|---|---|
| Android version | 7.0 (API 24) | 12+ |
| Architecture | ARM64 (aarch64) | ARM64 |
| Free storage | 8 GB | 16 GB |
| RAM | 4 GB | 8 GB+ |
| Termux version | 0.118 | Latest (F-Droid) |
| Termux:API | Required | Latest |

**ARM32 (armv7) note:** AURA v4 does not support ARM32. The installer detects this and exits with
a clear error. The llama.cpp GGUF inference requires NEON SIMD which is present on ARM64 but
the build complexity for ARM32 is not justified given the memory constraints of 32-bit devices.

### 2.3 Required Termux Packages

These are installed automatically by `install.sh`:

```
build-essential   # gcc, make, pkg-config
git               # source clone
curl              # model download
rust              # via rustup (not pkg)
openssl           # TLS for model download
python3           # build scripts (llama.cpp)
cmake             # llama.cpp build
ninja             # faster builds
patchelf          # fix ELF RPATH for Android
termux-services   # autostart sv management
```

### 2.4 Termux-Specific Path Differences

Standard Linux paths do not apply in Termux:

| Standard | Termux |
|---|---|
| `/usr/bin/bash` | `/data/data/com.termux/files/usr/bin/bash` |
| `/etc/` | `/data/data/com.termux/files/usr/etc/` |
| `/tmp/` | `/data/data/com.termux/files/usr/tmp/` |
| `$HOME` | `/data/data/com.termux/files/home/` |
| Systemd | termux-services (runit-based) |

The shebang in `install.sh` uses `#!/usr/bin/env bash` to be portable across both standard Linux
(for testing) and Termux.

### 2.5 Android Permissions Required

Before AURA can function:
- **Storage access**: `termux-setup-storage` must have been run (installer checks and prompts)
- **Battery optimization**: exempt Termux from battery optimization or AURA daemon will be killed
- **Notifications** (optional): Termux:API for rich notifications

---

## 3. Installation Architecture

### 3.1 High-Level Flow

```
User runs: bash install.sh
      │
      ▼
┌─────────────────────────────────┐
│  Phase 0: Pre-flight Checks     │
│  - Architecture detection       │
│  - Termux version check         │
│  - Storage permission check     │
│  - Free space check (8 GB min)  │
│  - Network connectivity check   │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  Phase 1: Package Install       │
│  - pkg update                   │
│  - Install build-essential, git │
│  - Install cmake, ninja, openssl│
│  - Install termux-services      │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  Phase 2: Rust Toolchain        │
│  - Check if rustup exists       │
│  - Install rustup if missing    │
│  - Add aarch64-linux-android    │
│    cross-compilation target     │
│  - Verify toolchain             │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  Phase 3: Source Acquisition    │
│  - Check if already cloned      │
│  - git clone if not             │
│  - git pull if already present  │
│  - Checkout --channel tag       │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  Phase 4: Model Download        │
│  - Check if GGUF already exists │
│  - Check checksum if exists     │
│  - Calculate storage needed     │
│  - curl with resume support     │
│  - SHA256 verification          │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  Phase 5: Build                 │
│  - cargo build --release        │
│  - Target: aarch64-linux-android│
│    (when cross-compiling)       │
│  - Or native on Termux ARM64    │
│  - Strip binary                 │
│  - Install to $PREFIX/bin/      │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  Phase 6: Configuration         │
│  - mkdir -p ~/.config/aura      │
│  - Write config.toml defaults   │
│  - Set model_path               │
│  - Set socket_path              │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  Phase 7: Service Setup         │
│  - Create sv directory          │
│  - Write run script             │
│  - Enable via sv-enable         │
│  - Start daemon                 │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  Phase 8: First-Time Setup      │
│  - Prompt for vault PIN         │
│  - Initialize trust tiers       │
│  - Run aura --check             │
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│  Phase 9: Success               │
│  - Print usage instructions     │
│  - Print aura status            │
│  - Print alias suggestions      │
└─────────────────────────────────┘
```

### 3.2 Idempotency

The installer is fully idempotent. Running it twice is safe:
- Packages already installed: skipped
- Rustup already installed: skipped
- Repo already cloned: `git pull` only
- Model already downloaded + checksum matches: skipped
- Config already exists: NOT overwritten (user edits preserved)
- Service already enabled: re-enabled (idempotent)

### 3.3 Resume Support for Model Download

The model download uses `curl -C -` (resume from partial download). This means:
1. If download is interrupted, re-running `install.sh` resumes from where it stopped
2. The partial file is kept at `~/.local/share/aura/models/`
3. Checksum is only verified on complete download

---

## 4. Model Download Strategy

### 4.1 Primary Model: Qwen3-8B-Q4_K_M

AURA v4's default model is Qwen3-8B in Q4_K_M quantization.

| Property | Value |
|---|---|
| Model | Qwen/Qwen3-8B |
| Quantization | Q4_K_M (4-bit, medium quality) |
| File size | ~5.2 GB |
| RAM required | ~6 GB |
| HuggingFace repo | `Qwen/Qwen3-8B-GGUF` |
| Filename | `qwen3-8b-q4_k_m.gguf` |

**HuggingFace Direct Download URL pattern:**
```
https://huggingface.co/Qwen/Qwen3-8B-GGUF/resolve/main/qwen3-8b-q4_k_m.gguf
```

Note: As of AURA v4 release, the exact filename and SHA256 must be pinned in the installer.
The installer contains the expected SHA256 checksum as a constant. If the checksum fails,
the user is warned and asked whether to proceed or abort.

### 4.2 Alternative Models

The installer supports a `--model` flag for alternative GGUF models:

| Model | Size | Use Case |
|---|---|---|
| Qwen3-4B-Q4_K_M | ~2.8 GB | Low-RAM devices (4 GB RAM) |
| Qwen3-8B-Q4_K_M | ~5.2 GB | Default (6+ GB RAM) |
| Qwen3-14B-Q4_K_M | ~9.5 GB | High-end devices (12+ GB RAM) |

### 4.3 Storage Layout

```
~/.local/share/aura/
├── models/
│   └── qwen3-8b-q4_k_m.gguf   (primary model)
├── db/
│   ├── episodic.sqlite          (episodic memory)
│   └── semantic.sqlite          (HNSW vector store)
└── logs/
    ├── daemon.log
    └── neocortex.log

~/.config/aura/
└── config.toml                  (user config, never overwritten by installer)

$PREFIX/var/service/aura-daemon/ (termux-services sv directory)
├── run                          (sv run script)
└── log/
    └── run                      (sv log script)
```

### 4.4 Checksum Verification

```bash
# After download completes:
EXPECTED_SHA256="<pinned-at-release-time>"
ACTUAL_SHA256=$(sha256sum "$MODEL_PATH" | cut -d' ' -f1)
if [ "$ACTUAL_SHA256" != "$EXPECTED_SHA256" ]; then
    warn "Checksum mismatch. File may be corrupted or model was updated."
    warn "Expected: $EXPECTED_SHA256"
    warn "Actual:   $ACTUAL_SHA256"
    # Prompt user: abort, retry, or continue anyway
fi
```

---

## 5. Config File Format

### 5.1 `~/.config/aura/config.toml`

```toml
# AURA v4 Configuration
# Generated by install.sh — safe to edit
# See docs/architecture/AURA-V4-INSTALLATION-AND-DEPLOYMENT.md for full reference

[daemon]
# Unix socket path for IPC between daemon and neocortex
socket_path = "/data/data/com.termux/files/home/.local/share/aura/daemon.sock"
# Log level: error | warn | info | debug | trace
log_level = "info"
# Bind daemon HTTP API (optional, for future GUI)
# http_bind = "127.0.0.1:7474"

[neocortex]
# Path to GGUF model file
model_path = "/data/data/com.termux/files/home/.local/share/aura/models/qwen3-8b-q4_k_m.gguf"
# Number of GPU layers to offload (0 = CPU only; -1 = all layers)
# On most Android devices: 0 (no GPU driver support in Termux)
n_gpu_layers = 0
# Context window size (tokens). 4096 is safe for 6 GB RAM devices.
context_size = 4096
# Number of CPU threads for inference
# Defaults to half the CPU count. Tune per device.
n_threads = 4
# Batch size for prompt processing
n_batch = 512

[memory]
# Maximum episodic memories to store
max_episodic_entries = 10000
# HNSW index parameters
hnsw_ef_construction = 200
hnsw_m = 16

[identity]
# User-visible name for AURA
assistant_name = "AURA"
# User's preferred name (set during first-time setup)
user_name = ""
# Personality baseline (0.0–1.0): warmth, curiosity, directness
warmth = 0.7
curiosity = 0.8
directness = 0.6

[vault]
# Vault PIN is stored as Argon2id hash, never plaintext
# Set during first-time setup by install.sh
pin_hash = ""
# Auto-lock after idle seconds (0 = never)
auto_lock_seconds = 0

[trust]
# Default trust tier for new interactions
# tiers: 0=Stranger, 1=Acquaintance, 2=Friend, 3=CloseFriend, 4=Soulmate
default_tier = 1
```

### 5.2 Config Upgrade Path

When AURA is updated, the daemon reads the existing config and adds any new keys with their
default values. It never removes user-set values. Config schema version is tracked via a
`[meta]` section added on first migration.

---

## 6. Build Strategy

### 6.1 Native Build in Termux (Recommended)

When running `install.sh` inside Termux on an ARM64 device, the build is native:

```bash
cargo build --release -p aura-daemon
cargo build --release -p aura-neocortex
```

No cross-compilation needed. Termux's Rust toolchain targets `aarch64-unknown-linux-musl` or
`aarch64-linux-android` depending on setup. The installer configures `.cargo/config.toml` with
the correct linker.

### 6.2 Cross-Compilation from Desktop (Advanced)

For developers building AURA on a Linux x86_64 host for deployment to Android:

```bash
# Install Android NDK target
rustup target add aarch64-linux-android

# Configure linker in .cargo/config.toml:
# [target.aarch64-linux-android]
# linker = "aarch64-linux-android35-clang"

# Build
cargo build --release --target aarch64-linux-android -p aura-daemon
```

The installer handles the Termux-native case. Cross-compilation is documented for CI/CD use.

### 6.3 llama.cpp Build (via aura-llama-sys)

The `aura-llama-sys` crate is a Rust wrapper around llama.cpp. It builds llama.cpp via a
`build.rs` script using CMake. In Termux:

```bash
# Required before cargo build:
pkg install cmake ninja python3
export ANDROID_NDK_HOME=""   # empty = use system compiler, not NDK cross-compiler
export CMAKE_BUILD_TYPE=Release
```

Key CMake flags set by `build.rs`:
- `-DLLAMA_NATIVE=OFF` — no host CPU detection (cross-compat)
- `-DLLAMA_NEON=ON` — ARM NEON SIMD (always on for ARM64)
- `-DLLAMA_METAL=OFF` — no Apple Metal
- `-DLLAMA_CUDA=OFF` — no NVIDIA CUDA
- `-DBUILD_SHARED_LIBS=OFF` — static linking

---

## 7. Service Management (termux-services)

### 7.1 What is termux-services?

termux-services is a runit-based service manager for Termux. It replaces systemd for the
Termux environment. Services are defined as directories in `$PREFIX/var/service/`.

### 7.2 AURA Daemon Service Definition

**`$PREFIX/var/service/aura-daemon/run`:**
```bash
#!/data/data/com.termux/files/usr/bin/sh
exec /data/data/com.termux/files/usr/bin/aura-daemon \
    --config /data/data/com.termux/files/home/.config/aura/config.toml \
    2>&1
```

**`$PREFIX/var/service/aura-daemon/log/run`:**
```bash
#!/data/data/com.termux/files/usr/bin/sh
exec svlogd -tt /data/data/com.termux/files/home/.local/share/aura/logs/
```

**Enable and start:**
```bash
sv-enable aura-daemon
sv up aura-daemon
```

**Check status:**
```bash
sv status aura-daemon
```

### 7.3 Auto-start on Termux Open

termux-services automatically starts enabled services when Termux opens. AURA daemon starts
within ~2 seconds of opening Termux.

### 7.4 Alternative: ~/.bashrc Auto-start

For devices where termux-services is unavailable or unreliable, the installer falls back to
adding a start command to `~/.bashrc`:

```bash
# AURA auto-start (added by install.sh)
if ! pgrep -x aura-daemon > /dev/null; then
    aura-daemon --config ~/.config/aura/config.toml &
    disown
fi
```

---

 ## 8. Troubleshooting Guide

### 8.1 install.sh Fails at Cargo Build Step

**Symptom:** `cargo build` fails with an error about a missing target or cross-compilation failure.

**Check:**
```bash
rustup target list --installed | grep aarch64-linux-android
```

**Fix:** If the target is not listed:
```bash
rustup target add aarch64-linux-android
```

Then re-run `install.sh`.

---

### 8.2 "Linker Not Found" Error During Cross-Compilation

**Symptom:** Error message: `error: linker 'aarch64-linux-android35-clang' not found` or `error: linker 'cc' not found`.

**Check:**
```bash
ls ~/aura/toolchain/bin/ | grep aarch64
```

**Fix:** The NDK toolchain was not downloaded or extracted correctly. Re-run `./install.sh` — it downloads the NDK toolchain automatically and configures `.cargo/config.toml` with the correct linker path.

If the issue persists, check that `ANDROID_NDK_HOME` is either unset (for native Termux builds) or points to a valid NDK r26+ installation (for cross-compilation from desktop).

For native Termux builds, ensure `build-essential` is installed:
```bash
pkg install build-essential cmake ninja
```

**Note:** `LLVM ERROR: Do not know how to split this operator's operand!` is a known llama.cpp + old NDK issue. Ensure NDK r26+ is used, or use native Termux build instead of cross-compilation.

---

### 8.3 llama.cpp Submodule Missing

**Symptom:**
```
error: file not found: llama.cpp/include/llama.h
```
or
```
error[E0463]: can't find crate for 'std'
```
during `aura-llama-sys` build.

**Fix:**
```bash
git submodule update --init --recursive
```

This initializes and populates the `vendor/llama.cpp` submodule. If the submodule was never added to the repo, see the P0-1 blocker in `AURA-V4-PRODUCTION-STATUS.md`.

---

### 8.4 Model File Not Found at Startup

**Symptom:** Daemon log contains:
```
[FATAL] Model not found at ~/aura/models/qwen3-8b-q4_k_m.gguf
```

**Fix:** The model was not downloaded or was downloaded to a different path.

Option 1 — Re-run the installer (it will resume any partial download):
```bash
bash install.sh
```

Option 2 — Download manually with resume support:
```bash
curl -C - -L --progress-bar \
    "https://huggingface.co/Qwen/Qwen3-8B-GGUF/resolve/main/qwen3-8b-q4_k_m.gguf" \
    -o ~/.local/share/aura/models/qwen3-8b-q4_k_m.gguf
```

Note: The model is ~5.2 GB. A stable connection is required. Use `curl -C -` for resume support if the connection drops.

After download, verify the path matches `model_path` in `~/.config/aura/config.toml`.

**Checksum issues:**

`sha256sum: command not found` → `pkg install coreutils`

Checksum mismatch (corrupted download):
```bash
rm ~/.local/share/aura/models/qwen3-8b-q4_k_m.gguf
bash install.sh   # re-downloads from scratch
```

**HuggingFace rate limit / 429:**
HuggingFace throttles unauthenticated downloads. Set a token:
```bash
export HF_TOKEN="hf_your_token_here"
# install.sh checks this env var and adds -H "Authorization: Bearer $HF_TOKEN"
```

---

### 8.5 Daemon Crashes with OOM

**Symptom:** The `aura-daemon` or `aura-neocortex` process is killed by the Android OOM killer. Logs show the process disappearing without a clean shutdown message.

**Fix — Use a smaller model:**
```bash
bash install.sh --model qwen3-4b-q4_k_m
```
The Q2_K quantization variant is also available for very constrained devices (~2 GB RAM for the 4B model).

**Fix — Reduce context window:**
Edit `~/.config/aura/config.toml`:
```toml
[neocortex]
context_size = 2048  # reduce from 4096
```

Then restart: `sv restart aura-daemon`

**Fix — Battery optimization exemption:**
Android may be aggressively killing background processes. Go to:
Settings → Battery → App Battery Management → Termux → Unrestricted

**Fix — Reduce build parallelism (OOM during compile):**
```bash
cargo build --release -j2 -p aura-daemon
```

---

### 8.6 Daemon Not Starting / Permission Errors

**Daemon not starting — check status and logs:**
```bash
sv status aura-daemon
cat ~/.local/share/aura/logs/current
```

**`Permission denied` on socket:**
```bash
chmod 700 ~/.local/share/aura/
rm -f ~/.local/share/aura/daemon.sock
sv restart aura-daemon
```

**`SIGKILL` from Android (background process killed):**
- Go to Android Settings → Battery → App Battery Management → Termux → Unrestricted
- Or: Settings → Developer Options → Running Services → Termux → Don't kill

**`aura` command not found:**
```bash
echo 'export PATH="$PREFIX/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

---

### 8.7 Permission Denied on Accessibility Service

**Symptom:** Daemon logs contain:
```
[ERROR] Accessibility service not enabled
```
or the screen reader returns empty accessibility trees.

**Fix:**
1. Go to: **Settings → Accessibility → Installed Services → AURA**
2. Enable the AURA accessibility service
3. Accept the permission dialog

On some Android versions the path is: Settings → Accessibility → Downloaded Apps → AURA

The accessibility service permission must be re-granted after Android system updates on some devices.

---

### 8.8 Inference Issues (Slow / OOM / Load Error)

**Slow inference (< 2 tokens/sec):**
- Ensure no other apps are running
- Tune thread count in `~/.config/aura/config.toml`: try `n_threads = 4` or `n_threads = 6`
- Check thermal throttling: give device a break if hot

**Out of memory during inference:**
Reduce context size in `~/.config/aura/config.toml`:
```toml
[neocortex]
context_size = 2048  # was 4096
```

**Model file loading error:**
```bash
# Verify file integrity:
sha256sum ~/.local/share/aura/models/qwen3-8b-q4_k_m.gguf
```

---

### 8.9 Battery Drain Is Excessive

**Symptom:** AURA noticeably drains battery even when idle.

**Check — Disable proactive mode:**
Edit `~/.config/aura/config.toml`:
```toml
[arc]
proactive_mode = false
```

**Check — Reduce heartbeat interval:**
```toml
[daemon]
heartbeat_interval_secs = 60  # increase from default (reduces background wake-ups)
```

**Check — Identify the cause:**
```bash
cat ~/.local/share/aura/logs/current | grep -E "ARC|proactive|heartbeat"
```

Proactive mode runs background LLM inference periodically. Disabling it reduces battery usage significantly at the cost of proactive suggestions.

---

### 8.10 Termux Loses Packages After Android Update

**Symptom:** After an Android system update, Termux commands like `cargo`, `git`, or `pkg` stop working, or the package database is corrupted.

**Fix:**
```bash
pkg update && pkg upgrade
```

If `pkg` itself is broken:
```bash
# Re-bootstrap the package database:
apt-get update
apt-get install -y termux-tools
pkg update
```

Then re-run the AURA installer to restore the build environment:
```bash
bash ~/aura/install.sh
```

The installer is idempotent — it will skip steps already completed and only fix what is missing.

---

### 8.11 JNI Bridge Fails to Load libaura_daemon.so

**Symptom:** `adb logcat | grep aura` shows `dlopen` errors such as:
```
java.lang.UnsatisfiedLinkError: dlopen failed: library "libaura_daemon.so" not found
```
or
```
java.lang.UnsatisfiedLinkError: couldn't find "libaura_daemon.so"
```

**Check:**
```bash
adb logcat | grep -E "aura|dlopen|UnsatisfiedLink"
```

**Fix — Rebuild with matching Android API level:**

Check the API level configured in `.cargo/config.toml`:
```toml
[target.aarch64-linux-android]
linker = "aarch64-linux-android35-clang"
```

The API level suffix (`35` in the example) must match the minimum SDK version declared in `AndroidManifest.xml`. If they differ, the `.so` may reference symbols not available on the device.

Update both to match your target Android version (API 24 minimum, API 35 for Android 15), then rebuild:
```bash
cargo build --release --target aarch64-linux-android -p aura-daemon
```

---

### 8.12 PIN Reset (Forgot Vault PIN)

**Warning:** This operation permanently resets all encrypted vault data. Memory, trust tiers, and any data stored in the vault will be lost. This cannot be undone.

**Fix:**
```bash
rm -rf ~/.config/aura/vault/
bash install.sh --reset-pin
```

The `--reset-pin` flag re-runs only the first-time setup phase (Phase 8) of the installer, prompting for a new vault PIN and re-initializing the vault with an empty state.

To export data before resetting (if the daemon is still running):
```bash
aura export --format json --output ~/aura-backup-$(date +%Y%m%d).json
```

---

 ## 9. Multi-Device Support Plan

### 9.1 Current Status (v4.0)

AURA v4.0 is single-device. Each device has its own daemon, memory, and vault.

### 9.2 Planned: Device Pairing (v4.1)

Two AURA instances on the same LAN can share:
- Episodic memory (read-only sync)
- Trust tier configuration
- Vault (encrypted, PIN-protected sync)

Protocol: AURA-to-AURA sync over local Unix/TCP socket with mutual auth via vault-derived keys.

### 9.3 Planned: Desktop Companion (v4.2)

A desktop AURA instance (Linux/macOS) can serve as:
- Build server for Termux (cross-compile aura-daemon for Android)
- Memory sync hub
- Remote inference server (when mobile device is low-power)

### 9.4 Not Planned: Cloud Sync

AURA will never sync to a cloud service. Memory, vault, and config stay on-device or on
user-owned hardware. This is a design invariant, not a milestone item.

---

## 10. Security Considerations

### 10.1 Vault PIN

The vault PIN is the root of trust for AURA's security model. It is:
- Never stored in plaintext
- Stored as Argon2id hash at `config.toml` `vault.pin_hash`
- Required to access trust tier 2+ operations
- Required for any destructive operation (memory wipe, config reset)

### 10.2 Model Integrity

The GGUF model is verified via SHA256 after download. This prevents:
- Corrupted downloads being used for inference
- Tampered model files (supply chain attack mitigation)

### 10.3 Termux Isolation

Termux processes run under the same Android UID as the Termux app. This means:
- AURA cannot access files owned by other Android apps (unless root)
- AURA can access files in `~/storage/` (user-visible storage) after `termux-setup-storage`
- The Android kernel enforces process isolation

### 10.4 No Network Listener by Default

AURA's daemon does NOT open a network port by default. All communication is via Unix socket.
The optional HTTP API (future feature) binds to `127.0.0.1` only and requires vault PIN auth.

---

## 11. Update Procedure

### 11.1 Updating AURA

```bash
# Re-running install.sh handles updates:
bash install.sh --update

# Or manually:
cd ~/aura
git pull
cargo build --release -p aura-daemon -p aura-neocortex
cp target/release/aura-daemon $PREFIX/bin/
sv restart aura-daemon
```

### 11.2 Updating the Model

```bash
# Change model in config.toml, then re-run installer with model flag:
bash install.sh --model qwen3-14b-q4_k_m
```

### 11.3 Version Pinning

The `rust-toolchain.toml` in the repo pins the exact Rust toolchain version. This ensures
reproducible builds across all devices and over time.

---

## 12. install.sh Design Summary

The `install.sh` script at the repository root implements everything described in this document.

**Key design decisions:**

1. **`#!/usr/bin/env bash`** — portable across Termux and standard Linux
2. **Color output with fallback** — detects `NO_COLOR` env var and terminal color support
3. **`set -euo pipefail`** — fail fast, no silent errors
4. **`die()` function** — all errors route through a single function with remediation hints
5. **Phase-based execution** — each phase is a named function, easy to skip with `--skip-phase`
6. **Idempotent** — safe to re-run at any point
7. **Resume-capable** — curl `-C -` for partial downloads
8. **Architecture check first** — fail immediately on unsupported hardware
9. **Storage check before download** — never start a 5 GB download that will fail
10. **Config preserved** — never overwrite existing `config.toml`
11. **Explicit HF_TOKEN support** — for users who need authenticated HuggingFace downloads
12. **`--dry-run` flag** — prints all actions without executing (for debugging)
13. **`--channel` flag** — `stable` (default) or `nightly` for tracking development builds
14. **`--skip-build` flag** — for users who want to provide a pre-built binary

See `install.sh` for the complete implementation.

