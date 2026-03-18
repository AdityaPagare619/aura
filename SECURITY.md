# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 4.0.x (alpha) | ✅ Active development |
| 3.x (Python prototype) | ❌ No security support |

Security fixes are applied to the `main` branch and released as new patch tags.
Alpha versions may receive security fixes without a new tag — always use the latest commit.

---

## Reporting a Vulnerability

**Please do NOT open a public GitHub issue for security vulnerabilities.**

Security issues in AURA can affect personal data stored on-device. We take all
reports seriously and commit to a responsible disclosure process.

### How to Report

1. **GitHub Security Advisories (preferred)**
   Go to: https://github.com/AdityaPagare619/aura/security/advisories/new
   This is a private channel — only you and the maintainer can see the report.

2. **Direct contact**
   If GitHub advisories are unavailable, describe the issue in a DM.

### What to Include

A good vulnerability report contains:
- **Component**: Which crate / subsystem is affected (`aura-daemon`, `aura-neocortex`, `install.sh`, etc.)
- **Description**: What the vulnerability is (1–3 sentences)
- **Impact**: What an attacker could achieve
- **Reproduction**: Minimal steps to reproduce (proof of concept if available)
- **Severity estimate**: Your assessment (Critical / High / Medium / Low)
- **Suggested fix** (optional but appreciated)

### What Happens Next

| Timeline | Action |
|----------|--------|
| 48 hours | Acknowledgement — confirm receipt and initial triage |
| 7 days | Assessment — determine severity, affected versions, fix timeline |
| 30 days | Fix — patch developed, tested, and merged |
| 90 days | Disclosure — public advisory published after fix is released |

For critical vulnerabilities (RCE, data exfiltration), we aim to fix within 14 days.

We will credit you in the security advisory unless you prefer to remain anonymous.

---

## Security Architecture

AURA is designed with security as a first-class concern. Key security properties:

### On-Device Privacy
- **Zero telemetry**: No data ever leaves the device. No analytics, no crash reports, no usage stats.
- **No cloud**: No cloud API calls for inference. Telegram Bot API is the only outbound network call, and it is entirely optional.
- **Anti-cloud absolute**: All LLM inference runs on-device via llama.cpp.

### Vault (Secrets Storage)
- **Encryption**: AES-256-GCM for all stored secrets
- **KDF**: Argon2id with 64MB memory cost, 3 iterations, 4 parallel lanes
- **No plaintext secrets on disk**: API tokens (Telegram bot token) are stored encrypted

### IPC Security
- **Abstract Unix socket**: `@aura_ipc_v4` — accessible only to processes with the same UID (Android isolation)
- **Max message size**: 256KB enforced at both encode and decode — prevents memory exhaustion
- **No authentication bypass**: IPC requires same-UID, no network exposure on Android

### Policy Gate (Deny-by-Default)
- Every action capability is explicitly allowlisted in compiled code
- Blocked patterns for destructive operations: `delete all`, `factory reset`, `format storage`, etc.
- These rules are hardcoded — no config file, no prompt injection can override them
- All security-sensitive operations are audit-logged

### Supply Chain
- All GitHub Actions are pinned to commit SHA (not mutable tags)
- Android NDK r26b download is verified against a hardcoded SHA256
- Model GGUF files are SHA256-verified before use (stable releases only)
- `cargo audit` runs in CI on every push

### Binary Integrity
- Release binaries include `.sha256` sidecar files
- `install.sh` verifies binary checksums before execution
- `install.sh` never pipes curl directly to a shell interpreter

---

## Known Non-Issues

The following are intentional design choices, not security vulnerabilities:

- **Telegram Bot API**: AURA uses Telegram for its interface. This is a designed choice. The Telegram bot token is stored in the encrypted vault. Conversations go through Telegram's servers — this is the tradeoff for having a mobile-friendly interface. Users who want zero network exposure should use direct voice mode (not yet released).
- **No network isolation on Termux**: Termux apps share the app's UID. AURA's Unix socket is accessible to other Termux processes. This is a Termux limitation, not an AURA bug.
- **llama.cpp bundled**: We bundle llama.cpp as a git submodule rather than using a system library. This ensures reproducible builds and known-good versions.

---

## Security Audit History

| Date | Scope | Auditor | Findings |
|------|-------|---------|----------|
| 2026-03-15 | v4.0.0-alpha.1 (self-audit) | Maintainer | See STAGE8-COURTROOM-VERDICTS.md |
| 2026-03-18 | v4.0.0-alpha.6 (android runtime-linking validation) | Maintainer | Verified static C++ runtime linkage path and Android build/release dependency gates |

External security audits are planned before stable (non-alpha) release.
