# Contributing to AURA

> **Current status:** AURA v4 is in active solo development. External contributions are not yet accepted. This file documents the process for when contributions open.

For detailed development setup, architecture documentation, and contribution guidelines, see:

**[docs/architecture/AURA-V4-CONTRIBUTING-AND-DEV-SETUP.md](docs/architecture/AURA-V4-CONTRIBUTING-AND-DEV-SETUP.md)**

---

## Quick Reference

### Bug Reports

Use the GitHub Issue template: [Bug Report](.github/ISSUE_TEMPLATE/bug_report.md)

Include your install log: `~/.local/share/aura/logs/` or the `INSTALL_LOG` path printed during install.

### Security Vulnerabilities

**Do not open a public issue.** See [SECURITY.md](SECURITY.md) for the responsible disclosure process.

### Development Setup

```bash
git clone https://github.com/AdityaPagare619/aura.git
cd aura
git submodule update --init --recursive
cargo check --workspace --features stub
cargo test --workspace --features stub
```

See the full dev guide for Android cross-compilation, NDK setup, and the test harness.

### Iron Laws (Non-Negotiable)

1. **LLM = brain, Rust = body** — Rust reasons nothing; all NLU goes to the LLM
2. **No cloud, ever** — zero telemetry, no fallback APIs, no external services
3. **No theater AGI** — no keyword matching for intent or NLU in Rust
4. **Never change production logic to make tests pass**
5. **Deny-by-default policy gate** — every capability is explicitly allowlisted

Any PR that violates these laws will be closed without review.
