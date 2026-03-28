# 01 — Operational Flows (Code-Derived)

## 1) Install flow (Termux-first)

Primary installer is `install.sh` (large phased installer).

### 1.1 Environment and architecture gating

- Detects Termux paths (`/data/data/com.termux`), configures installation roots and binaries.
- Enforces ARM64/aarch64 requirement.
- Derives model/storage/binary paths and env roots.

Reference: `install.sh:93-118`, `:335-343`.

### 1.2 Rust toolchain phase

Installer implements Termux-specific rustup hardening:

- `RUSTUP_USE_CURL=1`
- `RUSTUP_INIT_SKIP_PATH_CHECK=yes`
- explicit `CARGO_HOME` / `RUSTUP_HOME`
- optional removal of conflicting pkg-installed rust

Reference: `install.sh:755-764`, `:766-773`, `:775-812`.

### 1.3 Model phase

Model selection and download are controlled by RAM-based auto-selection with explicit override flags, GGUF sanity checks, size checks, and checksum validation.

Reference: `install.sh:458-506`, `:867-1002`.

## 2) Daemon startup flow

Daemon binary flow:

1. parse CLI config path
2. init tracing/panic hook
3. load config
4. run `startup(config)` 8-phase sequence
5. wire external shutdown flag to internal cancel flag
6. run async main loop until cancellation

Reference: `crates/aura-daemon/src/bin/main.rs:99-211`, `startup.rs:266-370`.

## 3) Inference operational flow

### 3.1 IPC transport

- Default socket in neocortex is platform dependent:
  - Android: `@aura_ipc_v4`
  - host: `127.0.0.1:19400`

Reference: `crates/aura-neocortex/src/main.rs:42-54`.

### 3.2 Backend/model load boundary

`load_model_ffi` path:

- Ensures backend init state before load.
- On non-Android, can initialize stub backend if missing.
- On Android, missing FFI init causes explicit error and returns null pointers (no forced panic at that call site).
- Actual backend dispatch then occurs for load/free.

Reference: `crates/aura-neocortex/src/model.rs:1075-1124`.

## 4) Voice and external interface flow

`main_loop` composes input sources and bridges:

- Telegram bridge and queue paths
- optional Voice bridge via `feature = "voice"`
- parser/amygdala/policy/context/routing chain

Reference: `crates/aura-daemon/src/daemon_core/main_loop.rs:56-65`, `:99-158`.

## 5) Safety/degradation flow

Policy flow uses layered gating with rate-limited action filtering.

- burst-suspicious action denial occurs before full rule pass.
- decision types drive whether execution proceeds, audits, asks confirmation, or denies.

Reference: `crates/aura-daemon/src/policy/gate.rs:69-117`, `:166-189`.

## 6) Shutdown and resilience flow

Daemon main binary creates signal-driven external shutdown flag; this flag is periodically bridged into daemon cancel flag and main loop exits cleanly.

Reference: `crates/aura-daemon/src/bin/main.rs:167-211`.

## 7) Operational risks discovered from CI + code alignment

### 7.1 Artifact handoff mismatch risk

`device-validate` expects artifact `aura-daemon-android-v2` from workflow run context.

Reference: `.github/workflows/device-validate.yml:31-35`.

Observed failure run `23677813385` failed at download step due to missing artifact name in triggering context.

### 7.2 CI script path typo risk

In CI build stage, verification step lists `targets/...` (plural) while actual build output is `target/...`.

Reference: `.github/workflows/ci.yml:140-143`.

(Commit history shows this was addressed in later changes; still relevant as a class of operational mismatch risk).

## 8) First-principles operations view

From code, AURA operations are governed by these practical first principles:

1. deterministic startup phases
2. strict IPC envelope framing and versioning
3. bounded memory/channel/caching defaults (mobile constraints)
4. deny/confirm/audit safety pipeline before side-effects
5. installer-led environment normalization in Termux
