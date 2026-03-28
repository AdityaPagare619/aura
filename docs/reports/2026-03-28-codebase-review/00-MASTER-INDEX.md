# AURA Full Codebase Review — Master Index (Code-First)

Date: 2026-03-28
Scope baseline commit (green main observed in CI history): `e662aed4a98b395c8b642431f6f1cbe52eaf3979`
Branch reviewed for this documentation update: `copilot/fix-lambda-cpp-compilation-issues`

This review package is intentionally split into separate enterprise documents:

1. [01-ARCHITECTURE-DESIGN-AND-SYSTEM-WIRING.md](./01-ARCHITECTURE-DESIGN-AND-SYSTEM-WIRING.md)
2. [02-OPERATIONAL-FLOW.md](./02-OPERATIONAL-FLOW.md)
3. [03-MEMORY-DATA-STRUCTURES-AND-ANALYTICS.md](./03-MEMORY-DATA-STRUCTURES-AND-ANALYTICS.md)
4. [04-PRODUCTION-CODE-QUALITY-RUST-ENGINEERING.md](./04-PRODUCTION-CODE-QUALITY-RUST-ENGINEERING.md)
5. [05-DEVOPS-INFRASTRUCTURE-AND-CI-ARCHITECTURE.md](./05-DEVOPS-INFRASTRUCTURE-AND-CI-ARCHITECTURE.md)
6. [06-ANDROID-AND-BINARY-CASE-STUDY.md](./06-ANDROID-AND-BINARY-CASE-STUDY.md)
7. [07-CI-DIAGNOSTICS-AND-LOG-STUDY.md](./07-CI-DIAGNOSTICS-AND-LOG-STUDY.md)

## Review method

- Code-first and project-structure-first review (Rust crates, scripts, workflows, infra paths).
- CI diagnostics included using GitHub Actions run/job/log inspection.
- No product/UI redesign in this package; this is architecture and engineering intelligence documentation.

## Repository-wide structure snapshot

Primary roots examined:

- `crates/aura-daemon`
- `crates/aura-neocortex`
- `crates/aura-llama-sys`
- `crates/aura-types`
- `crates/aura-iron-laws`
- `android/`
- `.github/workflows/`
- `infrastructure/`
- `install.sh`
- `Makefile`

## Quick facts verified from code

- Install script size: `install.sh` = 1866 lines.
- Android-specific workflow exists: `.github/workflows/build-android.yml` = 225 lines.
- Main CI workflow exists: `.github/workflows/ci.yml` = 284 lines.
- Daemon startup is explicitly 8-phase: `crates/aura-daemon/src/daemon_core/startup.rs`.
- Daemon main runtime loop is centralized around `tokio::select!`: `crates/aura-daemon/src/daemon_core/main_loop.rs`.
