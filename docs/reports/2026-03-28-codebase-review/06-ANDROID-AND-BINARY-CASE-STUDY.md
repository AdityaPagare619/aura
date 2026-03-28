# Android and Binary Case Study (Code-First)

## 1) Build and linker strategy

Android cross-build surfaces:

- `/home/runner/work/aura/aura/.cargo/config.toml`
- `/home/runner/work/aura/aura/.github/workflows/build-android.yml`
- `/home/runner/work/aura/aura/Cargo.toml` release profile

Key points verified:

- target `aarch64-linux-android`
- NDK-based linker wiring
- release profile tuned for Android stability (`lto=thin`, `panic=unwind`)

## 2) llama backend compile path

`aura-llama-sys` build script behavior is target-aware and feature-aware:

- Android + aarch64 + `stub` OFF => native llama.cpp compile path
- Android + aarch64 + `stub` ON => stub path, native compile skipped by design
- non-Android => stub cfg marker emitted

Reference:

- `/home/runner/work/aura/aura/crates/aura-llama-sys/build.rs`

## 3) IPC and process architecture relevance

Android IPC endpoint:

- `@aura_ipc_v4`

Host fallback:

- `127.0.0.1:19400`

Reference:

- `/home/runner/work/aura/aura/crates/aura-daemon/src/ipc/protocol.rs:38-45`

## 4) Operational context for binary behavior

AURA is operationally Termux-oriented via installer and scripts, not a Play Store-only packaging model.

Reference:

- `/home/runner/work/aura/aura/install.sh`

## 5) Crash-analysis implications from current code

Given current wiring, Android stability is multi-factor:

- Rust binary runtime/startup and memory behavior,
- native dependency/linking details,
- device/OEM runtime differences,
- context/runtime configuration.

This supports broad diagnosis beyond only llama.cpp-native concerns.
