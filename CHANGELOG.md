# Changelog

All notable changes to AURA v4 are documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html)

---

## [Unreleased]

### Planned
- Whisper.cpp on-device STT integration
- Direct voice interface (non-Telegram) via Android AudioRecord
- Proactive push via Android notifications (no Telegram required)
- ARC behavioral intelligence: goal tracking, life domain scoring
- HNSW memory consolidation background thread
- On-device model fine-tuning (LoRA adapter injection)

---

## [4.0.0-alpha.6] — 2026-03-18

### Fixed
- Android `aura-neocortex` linking for release artifacts:
  - added deterministic Android NDK static libc++ search paths
  - linked required C++ ABI runtime symbols for static linkage
- Restored `Build Android` workflow green state on `main` for neocortex + daemon
- Preserved runtime dependency gate to block `libc++_shared.so` in release artifacts

### Installer / Release Hardening
- `install.sh --skip-build` now performs runtime probes for downloaded binaries
  and fails fast if artifacts are not runnable on target device
- Release pipeline publishes checksum sidecars and includes explicit verification instructions

### Updated
- Workspace version bumped to `4.0.0-alpha.6`
- Installer stable channel now targets `v4.0.0-alpha.6`
- Config defaults updated to `4.0.0-alpha.6`
- README and architecture metadata updated for alpha.6 pre-release validation

---

## [4.0.0-alpha.1] — 2026-03-15

### Overview
First tagged release of AURA v4 — the complete Rust rewrite from aura-v3.
This is an alpha release intended for on-device testing via Termux.
All core systems are implemented and the Termux installer is production-grade.

### Added

#### Core Architecture
- `aura-daemon` crate: full cognitive core (~45,000 lines Rust)
- `aura-neocortex` crate: separate LLM inference process with TCP IPC
- `aura-llama-sys` crate: FFI bindings to llama.cpp (ARM64 batch API, GBNF grammar)
- `aura-types` crate: shared types, IPC protocol (bincode wire framing), full config schema

#### Memory System (4-tier)
- Working memory: in-RAM, <1ms, current conversation context
- Episodic memory: SQLite + pure-Rust HNSW, 2–8ms, recent events
- Semantic memory: SQLite + FTS5 + HNSW + RRF re-ranking, 5–15ms, long-term knowledge
- Archive memory: ZSTD compressed, 50–200ms, cold storage
- Hebbian re-ranking: co-accessed items get 20% score boost

#### Inference (Neocortex)
- 6-layer teacher stack: GBNF grammar → CoT → Logprob calibration → Cascade retry → Cross-model reflection → Best-of-N (N=3)
- Grammar-constrained sampling via `llama_sampler_init_grammar()` — invalid tokens masked before softmax
- `StubBackend`: bigram-based English generator for host CI (no llama.cpp required)
- `FfiBackend`: statically-linked llama.cpp for Android ARM64
- Multi-model tier system: Brainstem1.5B / Standard4B / Full8B, auto-selected by battery/thermal state
- GGUF metadata scanning with `ModelCapabilities` extraction

#### Identity & Ethics
- OCEAN personality model (openness=0.85, conscientiousness=0.75, extraversion=0.50)
- Valence-Arousal-Dominance (VAD) mood system with hysteresis
- 5-stage trust/relationship model: Stranger → Acquaintance → Friend → Close Friend → Soulmate
- Anti-sycophancy guard: composite score gate (block_threshold=0.40, warn_threshold=0.25)
- 15 hardcoded ethics rules compiled into binary — no config file can override
- Deny-by-default policy gate: every action capability is explicitly allowlisted

#### ARC (Adaptive Reasoning & Context)
- 10 life domains: health, finance, relationships, career, growth, creativity, environment, social, spiritual, wellbeing
- Initiative budget system: proactive outreach only when earned
- 8 context modes with dynamic mode-switching
- Cron-based background routines with thermal/battery awareness

#### Security
- Vault: AES-256-GCM + Argon2id (64MB memory, 3 iterations, 4 parallel)
- 4-tier data classification: public / internal / confidential / secret
- IPC: Unix domain socket (Android abstract `@aura_ipc_v4`), TCP fallback on host
- Max message size enforcement: 256KB normal, 16KB low-memory mode
- 30s read timeout, 5s write timeout on all IPC streams

#### Platform
- Android foreground service with bound lifecycle
- Battery/thermal state machine with hysteresis (prevents oscillation near tier boundaries)
- Termux service supervisor via `termux-services`
- Heartbeat watchdog (30s interval)
- Checkpoint/restore with bincode serialization (version-gated)

#### Telegram Interface
- Full Bot API integration via long-polling (no webhooks — mobile-friendly)
- Voice message pipeline: OGG Opus → PCM → Whisper.cpp STT (stub on alpha)
- Command handlers: `/start`, `/help`, `/status`, `/mood`, `/memory`, `/settings`
- Rate limiting, error recovery, graceful reconnect

#### DevOps / CI/CD
- GitHub Actions CI: 6 parallel jobs (check, test, clippy, fmt, audit, version-check)
- GitHub Actions release pipeline: CI gate → Android cross-compile → GitHub Release
- Android NDK r26b with SHA256 verification in CI
- `--features stub` skips llama.cpp native compilation on CI hosts
- Release artifacts: `aura-daemon-{TAG}-aarch64-linux-android` + `.sha256` sidecar
- Termux installer (`install.sh`): 9-phase, never pipes curl to sh, SHA256 model verify, Argon2id vault setup

#### Configuration
- Full `aura.toml` master config: 15+ sections, all fields documented
- `config/default.toml`, `config/safety.toml`, `config/power.toml`
- Environment variable overrides: `AURA_<SECTION>_<FIELD>`
- `aura-config.example.toml`: annotated template for new installs

### Architecture Decisions

| ADR | Decision |
|-----|----------|
| ADR-001 | Rust-only daemon (no Python, no Node.js) |
| ADR-002 | Telegram API via long-polling (designed choice, not cloud violation) |
| ADR-003 | bincode v2 (=2.0.0-rc.3 pinned) for IPC wire format |
| ADR-004 | Separate neocortex process — LLM crash cannot kill daemon |
| ADR-005 | `--features stub` for CI; native FFI only on Android target |
| ADR-006 | Abstract Unix socket on Android; TCP on host |
| ADR-007 | Release profile: opt-level "z", LTO, 1 codegen unit, strip, panic=abort |

### Known Limitations (alpha)
- Model SHA256 checksums are PLACEHOLDER values — alpha build skips checksum verify for model files
- Whisper.cpp STT is stubbed — voice input requires manual transcription workaround
- ARC proactive module is implemented but not battle-tested
- No OTA update mechanism yet — reinstall for upgrades

### Breaking Changes
- This is a full rewrite from v3. No migration path from aura-v3 data.

---

## [3.x] — (Historical — Python prototype)

The v3 series was a Python/FastAPI prototype used to validate the architecture.
All learnings were incorporated into the v4 Rust rewrite.
v3 source lives in `../aura-v3/` (parent directory).

---

[Unreleased]: https://github.com/AdityaPagare619/aura/compare/v4.0.0-alpha.6...HEAD
[4.0.0-alpha.6]: https://github.com/AdityaPagare619/aura/releases/tag/v4.0.0-alpha.6
[4.0.0-alpha.1]: https://github.com/AdityaPagare619/aura/releases/tag/v4.0.0-alpha.1
