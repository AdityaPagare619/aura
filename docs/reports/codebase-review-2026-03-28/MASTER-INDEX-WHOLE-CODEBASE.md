# MASTER INDEX — Whole-Codebase Operational and Architecture Map (AURA)

> Goal: provide one single, extremely detailed entrypoint that explains what the whole codebase does, how systems are wired, and where each responsibility lives.

## 0) Quick navigation

- Executive architecture: `./00-EXECUTIVE-SYSTEM-ARCHITECTURE.md`
- Operational flows: `./01-OPERATIONAL-FLOWS.md`
- Memory/data structures: `./02-MEMORY-DATA-STRUCTURES-ANALYTICS.md`
- Rust production quality: `./03-RUST-PRODUCTION-CODE-QUALITY-INSIGHTS.md`
- DevOps/infra: `./04-DEVOPS-INFRA-ARCHITECTURE.md`
- Android study: `./05-ANDROID-CASE-STUDY.md`
- Binary/build architecture: `./06-BINARY-BUILD-ARCHITECTURE.md`

---

## 1) What the full codebase does (top-level mission)

AURA is a local-first assistant runtime built around:

1. **Daemon orchestrator (`aura-daemon`)** — central runtime nervous system.
2. **Inference process (`aura-neocortex`)** — model loading + inferencing endpoint over IPC.
3. **Backend adapter (`aura-llama-sys`)** — backend abstraction that supports stub and Android native paths.
4. **Shared contracts (`aura-types`)** — typed schemas for config/events/ipc/tools/power/memory.
5. **Policy + identity + memory + execution stack** — implemented mostly in daemon modules.
6. **Android + Termux delivery surfaces** — app shell + installer + workflows.

This means the codebase is not “only llama integration”; it is a complete system runtime, with llama as one component of a larger architecture.

---

## 2) Repository anatomy (what each major folder is for)

### 2.1 Core runtime crates

- `crates/aura-daemon`
  - Main orchestrator and operational runtime.
  - Contains startup, main loop, policy, memory, identity, bridges, IPC client and process management.
- `crates/aura-neocortex`
  - Separate inference process/binary.
  - Handles model loading, context assembly, grammar/inference formatting, request handling.
- `crates/aura-llama-sys`
  - Build script for native path + runtime backend abstraction.
  - Stub backend for host/test and FFI backend for Android-target paths.
- `crates/aura-types`
  - Contract package for all core data types.
- `crates/aura-iron-laws`
  - Dedicated policy/constraints crate segment.

### 2.2 Platform and operations folders

- `android`
  - Android Gradle project and app config shell.
- `.github/workflows`
  - CI/CD pipelines (check/test/clippy/build/release/device validation).
- `infrastructure`
  - infra scripts, taxonomy, reproducibility and testing scaffolding.
- `install.sh`
  - installer-driven operational entrypoint for Termux flows.

---

## 3) Crate-by-crate deep map

## 3.1 `aura-daemon` — the runtime control tower

The daemon crate is the dominant runtime body.

### 3.1.1 Startup architecture

`startup(config)` runs explicit boot phases with budgets and sequencing:

1. JNI load
2. runtime init
3. DB open
4. checkpoint restore
5. subsystems init
6. IPC bind
7. cron schedule
8. ready

Reference: `crates/aura-daemon/src/daemon_core/startup.rs:266-370`.

### 3.1.2 Main loop architecture

The main loop is intentionally centralized (`main_loop.rs`) and coordinates all channels/systems in one orchestrated event loop.

Key design intent is documented in-file: single-writer mutable state to avoid hidden lock complexity.

References:
- architecture commentary: `crates/aura-daemon/src/daemon_core/main_loop.rs:5-39`
- subsystem imports/wiring surface: `crates/aura-daemon/src/daemon_core/main_loop.rs:93-156`

### 3.1.3 Major daemon subsystem families (directory map)

- `daemon_core/` — startup, checkpoint, shutdown, channel wiring, central runtime logic.
- `policy/` — rules, gate, emergency, sandbox, boundaries, audit.
- `identity/` — affective, anti-sycophancy, relationship, profile, personality governance.
- `memory/` — episodic/semantic/working memory, consolidation, compaction, embeddings interfaces.
- `execution/` — planner, executor, retry, cycle, monitoring.
- `ipc/` — protocol framing + process spawn/client semantics.
- `bridge/` — telegram/system/voice bridge layers.
- `routing/` + `pipeline/` — classifier, parser, amygdala/contextor, system paths.
- `arc/`, `goals/`, `health/`, `platform/`, `voice/`, `telemetry/` — specialized operational domains.

(Representative module file list can be generated from `find crates/aura-daemon/src -name '*.rs'`.)

## 3.2 `aura-neocortex` — inference service process

Core modules:

- `main.rs` — process entrypoint and socket binding defaults.
- `ipc_handler.rs` — IPC request handling loop.
- `model.rs` — model load/free, backend dispatch boundaries.
- `inference.rs` — inferencing operations.
- `context.rs`, `prompts.rs`, `grammar.rs`, `tool_format.rs` — shaping and constraints around prompting.

Default IPC endpoint split:

- Android: `@aura_ipc_v4`
- host: `127.0.0.1:19400`

Reference: `crates/aura-neocortex/src/main.rs:42-54`.

## 3.3 `aura-llama-sys` — backend adaptation layer

### 3.3.1 Build behavior

`build.rs` selects behavior by target:

- Android+aarch64: compile native llama C/C++ units (when stub feature is not enabled).
- Stub feature path: mark stub mode.
- Host path: stub marker path.

Reference: `crates/aura-llama-sys/build.rs:16-87`.

### 3.3.2 Runtime backend API

- Global `OnceLock` backend instance.
- `init_stub_backend(seed)`
- `init_ffi_backend(lib_path)` (android cfg)
- `backend()` accessor enforces pre-init contract.

Reference: `crates/aura-llama-sys/src/lib.rs:1716-1759`.

## 3.4 `aura-types` — shared canonical contracts

Core files and domains:

- `ipc.rs` — protocol version, authenticated envelope, context/failure payloads.
- `config.rs` — global runtime configuration shapes.
- `events.rs`, `actions.rs`, `goals.rs` — event/action/goal contracts.
- `memory.rs` — memory data contracts.
- `power.rs` — thermal/memory pressure + model memory estimates.
- `tools.rs`, `screen.rs`, `identity.rs`, `outcome.rs` etc.

Protocol contract evidence:

- `PROTOCOL_VERSION = 3`
- `AuthenticatedEnvelope<T>` fields and version checks

Reference: `crates/aura-types/src/ipc.rs:7-65`.

---

## 4) End-to-end runtime wiring (request lifecycle)

## 4.1 Boot lifecycle

1. Daemon entrypoint parses args and config.
2. `startup(config)` executes phased init.
3. Main loop begins and drives all subsystem interactions.

Reference:
- `crates/aura-daemon/src/bin/main.rs:99-211`
- `crates/aura-daemon/src/daemon_core/startup.rs:266-370`

## 4.2 Inference lifecycle (high-level)

1. Daemon receives input/events.
2. Parser + scoring/contextualization chain executes.
3. Routed requests are packaged into typed IPC payloads.
4. Neocortex receives, loads/uses model backend and returns results.
5. Daemon consumes responses and routes to output/bridge/reaction/memory paths.

Wiring evidence:

- runtime loop and subsystem map in `main_loop.rs`.
- IPC transport constants and framing in `ipc/protocol.rs`.
- model backend dispatch in `neocortex/model.rs`.

---

## 5) Platform architecture and Android specificity

## 5.1 Android app shell

Android project contains Gradle app config and manifest surfaces.

Representative files:

- `android/app/build.gradle.kts`
- `android/app/src/main/AndroidManifest.xml`

## 5.2 Termux operational model

`install.sh` drives practical deployment and hardens rustup/toolchain setup for Termux constraints.

Critical exports and assumptions are coded directly (RUSTUP_USE_CURL/CARGO_HOME/RUSTUP_HOME/etc.).

Reference: `install.sh:755-764`, `:758-763`.

---

## 6) DevOps and release architecture map

Workflows:

- `ci.yml` — main multi-stage validation/build pipeline.
- `build-android.yml` — dedicated Android cross-compile path with NDK validation.
- `device-validate.yml` — artifact consumption + device-style checks.
- `release.yml`, `binary-audit.yml`, `f001-diagnostic.yml`, etc.

Known operational coupling:

- `device-validate.yml` expects artifact `aura-daemon-android-v2`.
- Failure occurs if artifact missing in trigger context.

Reference: `.github/workflows/device-validate.yml:31-35`.

---

## 7) Security/safety and boundedness design index

## 7.1 IPC and payload bounds

- Framed messages with max payload caps.
- decode and EOF conditions handled as explicit errors.

Reference: `crates/aura-daemon/src/ipc/protocol.rs:47-57`, `:94-119`.

## 7.2 Policy gate

- rule evaluation + sliding-window rate limiting.
- bounded key space to limit memory growth.

Reference: `crates/aura-daemon/src/policy/gate.rs:28-31`, `:73-117`, `:195-220`.

## 7.3 Memory pressure models

- model memory estimator with explicit KV cache formula and context-sensitivity.

Reference: `crates/aura-types/src/power.rs:578-600`.

---

## 8) “What to read first” role-based reading paths

## 8.1 For architecture leadership

1. `00-EXECUTIVE-SYSTEM-ARCHITECTURE.md`
2. `04-DEVOPS-INFRA-ARCHITECTURE.md`
3. this master index

## 8.2 For runtime engineers

1. `crates/aura-daemon/src/daemon_core/startup.rs`
2. `crates/aura-daemon/src/daemon_core/main_loop.rs`
3. `crates/aura-neocortex/src/model.rs`
4. `crates/aura-daemon/src/ipc/protocol.rs`

## 8.3 For Android and build engineers

1. `06-BINARY-BUILD-ARCHITECTURE.md`
2. `05-ANDROID-CASE-STUDY.md`
3. `crates/aura-llama-sys/build.rs`
4. `.github/workflows/build-android.yml`

## 8.4 For policy/safety engineers

1. `crates/aura-daemon/src/policy/gate.rs`
2. `crates/aura-daemon/src/policy/`
3. `crates/aura-types/src/ipc.rs`

---

## 9) Current package status and intent

This index is designed to be the single master doorway for all detailed review docs and code-level anchors. It intentionally avoids abstract-only narratives and maps concrete file-level responsibility so teams can traverse from strategic understanding directly to implementation.

