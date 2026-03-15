# AURA v4 — Stage 2: All Agent Findings (Complete)
## Date: 2026-03-15
## Status: ALL 7 AGENTS COMPLETE — AWAITING COURTROOM JUDGMENT

---

## Agent 1: Ship Readiness Audit (PREVIOUS SESSION)

### Ship-Blockers
| # | Severity | Issue | Pre-Judgment Notes |
|---|----------|-------|-------------------|
| B1 | CRITICAL | llama.cpp submodule is empty (.gitkeep only) | Likely git clone issue |
| B2 | CRITICAL | nativeShutdown() in lib.rs is a no-op | CONFIRMED |
| B3 | HIGH | No UI Activity for permission grants | Telegram-by-design, partially valid |
| B4 | HIGH | FFI signatures unverified | Can't verify without compiling |
| B5 | HIGH | bincode pinned to RC 2.0.0-rc.3 | ALREADY ACCEPTED — OVERRULED |

### Ship-Warnings: W1-W6

---

## Agent 2: Operational Readiness (12 findings)

### CRITICAL (2)
- ORA-001: JNI nativeShutdown is no-op — cancel_flag never exposed to JNI
- ORA-002: cancel_flag exists in DaemonState but not wired to JNI

### HIGH (3)
- Config JSON ignored at runtime
- Runtime .expect() panic risk
- Fragile neocortex stdin shutdown

### MEDIUM (3)
- Memory placeholder .expect()
- ThermalCritical handler incomplete
- Blocking neocortex poll loop

### LOW (2) + INFO (2)

### Key Positive: Architecture fundamentally sound — atomic checkpoints, bounded channels, graceful degradation

---

## Agent 3: Security Deep-Dive (22 findings)

### CRITICAL (4)
- SEC-CRIT-005: User message not wrapped as untrusted in prompts.rs (prompt injection)
- SEC-CRIT-006: Vault auth is bare bool (no cryptographic proof)
- SEC-CRIT-007: get_sandbox() reconstructs fresh sandbox (security state loss)
- SEC-CRIT-008: Single encryption key for all vault tiers

### HIGH (5)
- SEC-HIGH-001: Extension author string comparison (no crypto attestation)
- SEC-HIGH-002: ReAct observations not marked untrusted
- SEC-HIGH-003: set_permission() has no auth or audit
- SEC-HIGH-004: UTF-8 panic on byte-level string truncation
- SEC-HIGH-005: No key rotation mechanism for vault

### MEDIUM (6), LOW (5), INFO (2)

---

## Agent 4: System Prompt & UX Coherence (12 findings)

### CRITICAL (2)
- 1.1 + 4.2: ReAct mode drops ENTIRE personality dimension — no personality_section() call
- 1.2: DGS mode also drops personality dimension

### HIGH (1)
- 2.1: Conversational Rule 4 "NEVER mention being AI" contradicts Immutable #3 "transparent about being AI"

### MEDIUM (2)
- 2.2: VAD Dominance dimension missing from PersonalitySnapshot (doc specifies 3D, code has 2D)
- 5.2: Composer context_budget (400 tokens) dangerously tight with identity overhead

### LOW (2)
- 3.3: No conflict resolution precedence statement between identity_tendencies and user_preferences
- 1.3: Comment numbering broken in ReAct/DGS builders

### Confirmed Correct: Reflection prompt minimal (intentional), BoN inherits full build_prompt(), truncation order correct, SelfKnowledge accurate, constitutional tendencies match verdict

---

## Agent 5: CI/CD Pipeline Audit (16 findings)

### CRITICAL (3)
- C1: aura-neocortex NEVER built for Android — release ships daemon without inference engine
- C2: Release pipeline skips cargo audit — CVEs can ship
- C3: No version tag validation — tag can mismatch Cargo.toml version

### HIGH (4)
- H1: llama.cpp submodule tracks master branch (reproducibility risk)
- H2: Cache key missing toolchain version (stale artifacts)
- H3: aura-llama-sys duplicates workspace dependencies
- H4: No scheduled CI (nightly regression detection)

### MEDIUM (6), LOW (4)

### Key Positive: SHA-pinned actions, NDK integrity verification, Cargo.lock committed, proper concurrency controls

### Architectural Question: cdylib+JNI vs standalone binary (Termux vs APK distribution model)

---

## Agent 6: Install Experience & Distribution (15 findings)

### BLOCKER (2)
- F-05: Placeholder checksums cause die() on stable channel — DEFAULT INSTALL PATH BROKEN
- F-07: --skip-build has NO binary download — only checks local file

### CRITICAL (1)
- F-04: Missing git submodule update --init --recursive — build will fail

### HIGH (4)
- F-08: aura-neocortex not in release pipeline
- F-03: Installs rustup stable but project needs nightly-2026-03-01
- F-10: Config.toml with pin_hash never gets chmod 600
- F-15: Phase ordering — model download before build wastes time if build fails

### MEDIUM (3), LOW (4)

### UX Score: 5/10 — Architecture good but default path broken

### Exact --skip-build implementation provided (complete function with download, checksum verify, install)

---

## Agent 7: Voice Latency & Real-Time Pipeline

### Current End-to-End: 3.9s - 12.3s (4-12x worse than 1s target)

### Three Biggest Killers:
1. LLM generates ALL tokens before TTS starts (2-8s) — no streaming
2. smart_transcribe() negates streaming STT by always re-running Whisper batch (0.8-2s)
3. TTS "streaming" is fake — synthesizes full text then chunks (0.5-1.5s)

### Achievable Targets (after all optimizations):
- SD 8 Gen 3 + 1.5B: 650-950ms (SUB-SECOND POSSIBLE)
- SD 7xx + 1.5B: 850-1350ms
- SD 7xx + 3B: 1200-2100ms
- SD 6xx + 1.5B: 1350-2100ms

### P0 Required Changes:
1. Streaming LLM→TTS (sentence-level) — 3-5 days, saves 2-7 seconds
2. Remove Whisper re-transcription in voice mode — 1 hour, saves 0.8-2 seconds
3. Reduce VAD silence timeout to 300ms — 5 min, saves 200ms
4. Implement Android audio FFI — 3-5 days (prerequisite for any voice)

### Architecture Change: 5 files, ~300-400 lines for streaming LLM→TTS

---

## TOTALS ACROSS ALL AGENTS

| Severity | Count |
|----------|-------|
| BLOCKER | 2 |
| CRITICAL | 12 |
| HIGH | 17 |
| MEDIUM | 20 |
| LOW | 17 |
| INFO | 4+ |
| **TOTAL** | **72+** |

### De-duplicated Cross-Agent Overlap:
- "neocortex not built for Android" appears in Agent 1, 5, 6 (same issue, 3 agents found it)
- "nativeShutdown no-op" appears in Agent 1, 2 (same issue)
- "submodule empty/not initialized" appears in Agent 1, 6 (same issue)

### Unique Issues After De-duplication: ~55-60
