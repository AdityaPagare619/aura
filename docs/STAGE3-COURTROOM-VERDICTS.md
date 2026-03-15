# AURA v4 — Stage 3: COURTROOM VERDICTS (FINAL)

**Date:** 2026-03-15
**Status:** OFFICIAL VERDICT — Binding for all Stage 4 implementation work
**Judged by:** Orchestrator (Polymath + Multi-domain sequential thinking, 7 rounds)
**Inputs:** 7 Agent findings (Stage 2) + 4 Agent Panel assessments (Stage 3 inputs) + Iron Laws + Prior Binding Verdicts + User Sovereignty Principle + Codebase context
**Method:** 7 rounds of sequential thinking through: Architecture, Security/Privacy, Mobile/Real-Time, UX/CI/CD/Install, Voice/Philosophy, Cascading What-If Analysis, Final Consolidation

---

## COURTROOM METHODOLOGY

This verdict was NOT produced by agents. It was produced by the orchestrator after deep multi-domain analysis:

1. **Round 1:** Triage & Categorization — grouped 72+ raw findings into 9 natural categories, de-duplicated to ~55-60 unique issues
2. **Round 2:** Security & Privacy Expert Lens — recalibrated from cloud-service to on-device threat model
3. **Round 3:** Mobile Engineering & Real-Time Systems Lens — assessed ship blockers, crash bugs, operational robustness
4. **Round 4:** UX/Identity + CI/CD + Install Expert Lens — cross-referenced with Tier 1 work, identity philosophy
5. **Round 5:** Voice/Real-Time + Philosophy Cross-Check — verified against Iron Laws and User Sovereignty
6. **Round 6:** What-If Cascading Analysis — traced every P0/P1 change through the codebase for side effects
7. **Round 7:** Final Consolidated Verdict — priority matrix with effort estimates and implementation order

Key principles applied:
- **On-device threat model, NOT cloud-service** — several agent CRITICAL findings were downgraded because agents applied multi-tenant cloud threat modeling to a single-user on-device app
- **Courtroom Judgment Principle** — for each finding, asked "Was this intentional or a genuine gap?" None of the P0/P1 items were intentional design choices
- **Don't sacrifice core for voice** — streaming LLM→TTS deferred to beta per user mandate
- **Tier 1 cross-reference** — verified that some agent findings may already be fixed by Tier 1 identity wiring
- **User Sovereignty override** — Rule 4 contradiction resolved in favor of Immutable #3 (transparency)

---

## P0: MUST FIX BEFORE ANY ALPHA RELEASE

These items block ALL users. No alpha ships without these fixed.

| ID | Issue | Source | Fix | Effort | Why P0 |
|----|-------|--------|-----|--------|--------|
| P0-1 | UTF-8 byte-level truncation → panic on emoji/non-ASCII | SEC-HIGH-004, Panel 2 | Replace byte slicing with `char_indices()` in prompts.rs + any other truncation sites | 1-2 hrs | **Crash bug.** Day 1 crash for every non-English user. Panel upgraded to P0, I strongly agree. |
| P0-2 | Rule 4 "NEVER mention being AI" contradicts Immutable #3 "transparent about being AI" | Agent 4, 2.1 | Rewrite Rule 4: "I don't lead with disclaimers about being AI, but I'm always honest about my nature when asked or when it's relevant to the conversation." | 30 min | **Philosophical integrity.** AURA's identity must be internally consistent. Immutable #3 wins per User Sovereignty Principle. |
| P0-3 | llama.cpp submodule empty + missing `git submodule update --init --recursive` in install.sh | B1, F-04 | Pin submodule to specific commit (not master), add submodule init to install.sh Phase 2 | 45 min | **Nothing compiles without this.** Build prerequisite. |
| P0-4 | aura-neocortex NEVER built for Android | C1, F-08, Panel 3 | Add neocortex to build-android.yml cross-compilation using same NDK/toolchain as daemon | 2-4 hrs | **No inference engine = AURA can't think.** Release ships a body without a brain. |
| P0-5 | Placeholder SHA256 checksums → `die()` on stable channel | F-05, Panel 3 | (a) Make checksum verification warn-only for dev/alpha channels; (b) Generate real checksums in release.yml pipeline | 2-3 hrs | **100% first-user failure rate.** Default install path broken. |
| P0-6 | install.sh installs `rustup stable` but project requires `nightly-2026-03-01` | F-03, Panel 3 | Change install.sh to install correct nightly toolchain + aarch64-linux-android target | 15 min | **Build from source fails immediately.** Every Termux user blocked. |

**Total P0 effort: ~7-10 hours**

---

## P1: FIX FOR ALPHA QUALITY

Ship-quality items. AURA works without these but has notable quality/security gaps.

| ID | Issue | Source | Fix | Effort | Why P1 |
|----|-------|--------|-----|--------|--------|
| P1-1 | `nativeShutdown()` no-op + cancel_flag not wired to JNI | B2, ORA-001/002 | Wire `cancel_flag` AtomicBool to JNI entry point in lib.rs. Shutdown sequence: JNI → cancel_flag → daemon stops → closes neocortex stdin → neocortex exits | 1-2 hrs | Unclean shutdown risks checkpoint data loss, resource leaks. |
| P1-2 | User message / external content not boundary-marked in prompts (prompt injection vector) | SEC-CRIT-005, SEC-HIGH-002, Panel 2 | Add XML-style delimiters around external content (file contents, tool outputs, forwarded messages) in prompts.rs. User's own messages are trusted (User Sovereignty). ReAct observations also tagged. | 3-4 hrs | #1 real attack vector for on-device LLMs processing external content. |
| P1-3 | `get_sandbox()` reconstructs fresh sandbox each call (security state loss) | SEC-CRIT-007, Panel 2 | Cache sandbox instances by extension ID. LRU eviction when too many. Checkpoint state on operation completion. | 6-8 hrs | Genuine bug. Rate limits, permission denials, audit trail all lost on each call. |
| P1-4 | Runtime `.expect()` → panic on legitimate None/Err in production | ORA-HIGH, Panel 1 | Audit all `.expect()` calls in daemon + neocortex. Replace with proper error handling (match/unwrap_or/?) for any that can legitimately fail. | 3-5 hrs | Each one is a potential production crash. |
| P1-5 | Release pipeline skips `cargo audit` | C2, Panel 3 | Add `cargo audit` step to release.yml before artifact build | 30 min | CVEs can ship silently. Trivial fix, high value. |
| P1-6 | llama.cpp submodule tracks `master` (reproducibility risk) | H1, Panel 3 | Pin to specific commit hash (combined with P0-3) | _(included in P0-3)_ | Builds may break randomly when llama.cpp master changes. |
| P1-7 | `--skip-build` has no binary download implementation | F-07, Panel 3 | Implement download from GitHub Releases + checksum verify + install. Agent 6 provided complete function code. | 2-3 hrs | Critical UX path for users who can't compile locally. |
| P1-8 | Android Audio FFI — all stubs are TODO | Agent 7, Panel 4 | Implement Android audio capture (AudioRecord) + playback (AudioTrack) via JNI bridge. Kotlin AudioManager → JNI → Rust voice pipeline. | 3-5 days | **Voice feature gate.** No voice without this. TIME-BOXED: 5 days. If exceeds, ship text-only alpha. |
| P1-9 | `smart_transcribe()` always re-runs Whisper batch (negates streaming STT) | Agent 7, Panel 4 | In voice mode, skip batch re-transcription and use streaming result directly. | 1 hr (after P1-8) | Saves 0.8-2 seconds of voice latency. Only meaningful after Audio FFI works. |
| P1-10 | ReAct/DGS mode drops personality sections | Agent 4, 1.1/1.2 | **VERIFY FIRST** — Tier 1 already wired identity across all 3 context paths. If still missing, 1-line fix per mode. | 1-2 hrs verify, 30 min fix if needed | May already be resolved. Must verify before fixing. |

**Total P1 effort: ~20-28 hours (excluding Audio FFI) + 3-5 days for Audio FFI**

---

## P2: POST-ALPHA QUALITY IMPROVEMENTS

Not blocking alpha, but should be done before beta or as fast-follows.

| ID | Issue | Source | Fix | Effort |
|----|-------|--------|-----|--------|
| P2-1 | Config JSON loaded but values ignored at runtime | ORA-HIGH | Wire config values to runtime behavior | 3-4 hrs |
| P2-2 | `set_permission()` has no auth or audit trail | SEC-HIGH-003 | Add permission change logging for user transparency | 4-5 hrs |
| P2-3 | VAD Dominance dimension missing (code has 2D, docs specify 3D) | Agent 4, 2.2 | Add dominance field to PersonalitySnapshot/mood struct | 1-2 hrs |
| P2-4 | No version tag ↔ Cargo.toml validation in CI | C3 | Add validation step to release.yml | 1 hr |
| P2-5 | CI cache key missing toolchain version | H2 | Add Rust toolchain version to cache key hash | 15 min |
| P2-6 | aura-llama-sys duplicates workspace dependencies | H3 | Align with workspace Cargo.toml | 1 hr |
| P2-7 | No scheduled/nightly CI | H4 | Add cron schedule to ci.yml | 1 hr |
| P2-8 | test-termux-install.yml is dead v3 workflow | Panel 3 | Delete or update to v4 structure | 30 min |
| P2-9 | Config.toml with pin_hash not chmod 600 | F-10 | Add chmod 600 after config write in install.sh | 5 min |
| P2-10 | Phase ordering: model download before build | F-15 | Swap in install.sh: build first, then download model | 30 min |
| P2-11 | Fragile neocortex stdin shutdown | ORA-HIGH | Add graceful shutdown signal (SIGTERM handler or explicit command) | 2-3 hrs |
| P2-12 | Blocking neocortex poll loop | ORA-MED | Investigate and fix potential latency spikes | 1-2 hrs |
| P2-13 | Composer context_budget 400 tokens tight with identity overhead | Agent 4, 5.2 | Monitor during testing. Increase if truncation issues emerge. | 0 (monitoring) |
| P2-14 | VAD silence timeout 500ms→300ms | Agent 7 | Config change after voice works | 5 min |

**Total P2 effort: ~15-20 hours**

---

## P3: DEFERRED TO POST-LAUNCH

| ID | Issue | Source | Reason for Deferral |
|----|-------|--------|---------------------|
| P3-1 | Vault auth bare bool → crypto session (SEC-CRIT-006) | Panel 2 | Single-user on-device. Device lock provides real auth. Over-engineering for alpha. |
| P3-2 | Single encryption key for all vault tiers (SEC-CRIT-008) | Panel 2 | Per-tier keys add complexity without meaningful security gain on single device. |
| P3-3 | No key rotation mechanism (SEC-HIGH-005) | Panel 2 | Key rotation matters for long-lived services. Post-launch. |
| P3-4 | ThermalCritical handler incomplete | ORA-MED | Android OS handles thermal management. Not critical for alpha. |
| P3-5 | Streaming LLM→TTS pipeline | Agent 7, Panel 4 | **DEFERRED TO BETA.** Too risky for alpha — touches core inference loop. User mandate: "Don't sacrifice core for voice." Alpha voice = works. Beta voice = fast. |
| P3-6 | TTS "streaming" is fake (synthesizes full then chunks) | Agent 7, Panel 4 | Same reasoning as P3-5. Beta optimization. |
| P3-7 | Conflict resolution precedence (tendencies vs preferences) | Agent 4, 3.3 | Implied by architecture (User Sovereignty: user prefs win for style, immutables win for safety). Document when convenient. |
| P3-8 | Comment numbering broken in ReAct/DGS | Agent 4, 1.3 | Cosmetic only. |

---

## OVERRULED FINDINGS

These agent findings were REJECTED by the courtroom after analysis:

| Finding | Source | Reason for Overruling |
|---------|--------|-----------------------|
| Extension author crypto attestation (SEC-HIGH-001) | Agent 3, Panel 2 | **No extension marketplace exists.** Extensions are sideloaded by the user, who IS the trust authority. Agent applied marketplace threat model to a sideload-only architecture. |
| Extension update verification | Agent 3 | Same — no marketplace, no update channel. |
| Tamper-resistant audit logs | Agent 3 | Over-engineering for single-user on-device. Who is the adversary? The user? That contradicts User Sovereignty. |
| bincode RC pinned (B5) | Agent 1 | **BINDING PRECEDENT from prior courtroom.** Already accepted. Case closed. |
| No UI Activity for permissions (B3) | Agent 1 | **Telegram is a DESIGNED CHOICE, not a violation.** The user explicitly stated this. Partially valid: Android permissions are handled at app install via manifest, not UI Activity. |
| FFI signatures unverified (B4) | Agent 1 | **Cannot verify without compiler.** `cargo` is not in PATH. Not actionable in current environment. Will be verified when CI/CD builds run. |

---

## CRITICAL CORRECTIONS TO AGENT PANEL ASSESSMENTS

| Agent Panel Claim | Orchestrator Correction |
|-------------------|------------------------|
| Panel 2: SEC-CRIT-005 (prompt injection) at P2 | **UPGRADED to P1.** Prompt injection is the #1 real attack vector for on-device LLMs processing external content. Cannot wait. |
| Panel 2: Several findings called "CRITICAL" | **Multiple downgraded.** Agents applied cloud-service/multi-tenant threat model. On a single-user on-device app, vault auth bool (SEC-CRIT-006) and per-tier keys (SEC-CRIT-008) are P3 deferrals, not CRITICALs. |
| Panel 4: Android Audio FFI as P0 | **Downgraded to P1.** Voice is a FEATURE, not a ship requirement. Text via Telegram is the primary alpha interface. Voice is P1 with a 5-day time-box. |
| Panel 4: Streaming LLM→TTS as DEFER | **AGREED.** This is the correct call. Core architecture protection. |
| Panel 3: ReAct/DGS personality as P0 | **Changed to VERIFY FIRST.** Tier 1 implementation already wired identity across all 3 context paths. This may already be fixed. Must verify before "fixing." |
| Panel 1: Total P1 effort ~2.5 hours | **CORRECTED to 7-10 hours for P0 alone.** Panel significantly underestimated CI/CD and cross-compilation work. |

---

## IMPLEMENTATION ORDER (Dependency-Aware Sprints)

### Sprint 1: Quick P0 Wins (Day 1, ~2.5 hours)
- P0-1: UTF-8 truncation fix
- P0-2: Rule 4 rewrite
- P0-3 + P1-6: Submodule pin + install.sh init
- P0-6: Nightly toolchain in install.sh

### Sprint 2: CI/CD P0 Fixes (Day 2-3, ~5-7 hours)
- P0-4: Neocortex Android cross-compilation
- P0-5: Checksum handling (install.sh + release pipeline)
- P1-5: cargo audit in release pipeline

### Sprint 3: Core P1 Fixes (Day 3-5, ~12-17 hours)
- P1-1: nativeShutdown wiring
- P1-2: Prompt boundary marking
- P1-4: .expect() triage
- P1-10: Verify ReAct/DGS personality (may be already fixed)
- P1-7: --skip-build download implementation

### Sprint 4: Security P1 (Day 5-8, ~6-8 hours)
- P1-3: Sandbox persistence
- Any P1 overflow from Sprint 3

### ⭐ TEXT-ONLY ALPHA SHIP DECISION POSSIBLE HERE (Day 8) ⭐

### Sprint 5: Voice Foundation (Day 8-13, time-boxed 5 days)
- P1-8: Android Audio FFI implementation

### Sprint 6: Voice Polish (Day 13, ~1 hour)
- P1-9: Remove Whisper re-transcription

### Sprint 7: P2 Sweep (Day 14-16)
- All P2 items in priority order

### ⭐ FULL ALPHA SHIP DECISION (Day 16) ⭐

---

## EFFORT SUMMARY

| Priority | Count | Estimated Effort |
|----------|-------|-----------------|
| P0 | 6 items | 7-10 hours |
| P1 | 10 items | 20-28 hours + 3-5 days (Audio FFI) |
| P2 | 14 items | 15-20 hours |
| P3/Defer | 8 items | — |
| Overruled | 6 items | — |
| **TOTAL** | **44 items adjudicated** | **~42-58 hours + 3-5 days** |

---

## BINDING DECISIONS FOR STAGE 4

1. **All P0 items are non-negotiable.** No alpha release without all 6 fixed.
2. **P1 items define alpha quality.** Ship decision at Day 8 for text-only alpha.
3. **Audio FFI is time-boxed to 5 days.** If not complete, ship text-only alpha, add voice in alpha.2.
4. **Streaming LLM→TTS is DEFERRED to beta.** This protects the core inference architecture per user mandate.
5. **On-device threat model is canonical.** Stop applying cloud-service security patterns.
6. **Verify before fixing** — always check if Tier 1 already addressed an issue before writing new code.
7. **Agent panels are INPUTS, not verdicts.** This document IS the verdict.

---

## PRIOR VERDICTS REINFORCED

All verdicts from `COURTROOM-VERDICT-SHAPING-DECISIONS.md` remain binding:
- User Sovereignty Principle: REINFORCED
- Three Categories (IMMUTABLE/USER-SOVEREIGN/EMERGENT): REINFORCED
- 5 Constitutional Tendencies: REINFORCED
- Smart Adaptation Without Restriction: REINFORCED
- Ship Gate After Tier 1: REINFORCED (now refined to "after P0+P1")
- Iron Laws: ALL REINFORCED

---

*This verdict is FINAL and BINDING for Stage 4 implementation.*
*Any deviation requires courtroom reconvening with full sequential thinking.*
*Document version: 1.0 | Judge: Orchestrator (7-round deep analysis)*
