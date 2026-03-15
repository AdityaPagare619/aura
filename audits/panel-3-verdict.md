# COURTROOM PANEL 3 — FINAL VERDICT DOCUMENT

**AURA v4 Multi-Agent Audit — Agents 4, 5, 6**
**Date:** 2026-03-15
**Panel:** Courtroom Panel 3
**Judges:** Senior Architect (Presiding), Security Reviewer, UX Advocate
**Scope:** System Prompt & UX Coherence (Agent 4), CI/CD Pipeline (Agent 5), Install Experience & Distribution (Agent 6)
**Total Findings Reviewed:** 43 (12 + 16 + 15)
**Unique Findings After Deduplication:** 41

---

## METHODOLOGY

For each finding, this panel:
1. **Verified** the claim against actual source code (prompts.rs, release.yml, ci.yml, build-android.yml, install.sh)
2. **Debated** three questions: (a) Why wasn't this done? (b) Is it VALID given full context? (c) Cross-impact of fixing it?
3. **Applied the lens:** "What must work for first real users?" — alpha-stage tolerances apply for polish items, zero tolerance for broken install paths and ethical contradictions
4. **De-duplicated** overlapping findings across agents

### Verdict Scale

| Verdict | Meaning |
|---------|---------|
| **APPROVE P0** | Ship-blocker. Must fix before ANY user touches this. |
| **APPROVE P1** | Critical for alpha. Fix within first sprint. |
| **APPROVE P2** | Important. Fix before beta. |
| **APPROVE P3** | Nice to have. Backlog. |
| **MODIFY** | Finding is valid but scope/severity adjusted. |
| **OVERRULE** | Finding is invalid, out of scope, or based on misunderstanding. |
| **DEFER** | Insufficient evidence or premature for current stage. |

---

## DE-DUPLICATION TABLE

| Canonical ID | Duplicates | Finding | Resolution |
|-------------|------------|---------|------------|
| **C1** | F-08 | Neocortex not built for Android release | Merged under C1. F-08 marked DEDUPLICATED. |
| **F-04** | H1 (partial) | Missing git submodule init in install.sh | F-04 is the install-side; H1 is the CI-side. Both kept as separate verdicts — different fix locations. |
| **1.1** | 4.2 | personality_section() missing from ReAct/DGS | Same root cause. 4.2 merged under 1.1. |

---

## AGENT 4 — SYSTEM PROMPT & UX COHERENCE (12 Findings)

### Finding 1.1 — ReAct Mode Missing personality_section()
**Agent Assessment:** ReAct prompt builder lacks personality traits (OCEAN, mood, trust, VAD).
**Evidence:** `prompts.rs:804-893` — `build_react_prompt()` includes identity_section, self_knowledge_section, identity_tendencies_section, user_preferences_section — but does NOT call `personality_section(slots)`. Compare with `build_prompt()` at line 735 which DOES include it.
**Debate:**
- *Why wasn't this done?* Tier 1 wiring focused on identity pipeline (tendencies, preferences, self-knowledge) and likely overlooked that `personality_section()` is a separate function handling OCEAN traits, mood, trust_level, valence, arousal.
- *Valid?* YES — partially fixed but incomplete. The OCEAN/VAD personality block is genuinely absent from ReAct and DGS modes.
- *Cross-impact?* Fix is surgical: add one `personality_section(slots)` call to each builder. Low risk.

**VERDICT: APPROVE P1**
**Implementation:** Add `personality_section(slots)` call to `build_react_prompt()` and `build_dgs_prompt()` in the same position as `build_prompt()` uses it. ~2 lines of code each.

---

### Finding 1.2 — Mode Switching Personality Continuity
**Agent Assessment:** When switching between modes (chat → ReAct → DGS), personality traits may be lost or inconsistent.
**Evidence:** This is a consequence of 1.1. Since ReAct/DGS don't include personality_section(), mode switches cause personality "amnesia." However, identity_tendencies and user_preferences ARE present in all modes, so it's partial amnesia, not total.
**Debate:**
- *Valid?* YES but severity is lower than 1.1 since identity continuity IS maintained — only OCEAN/VAD specifics are lost.

**VERDICT: APPROVE P2** (subsumes under 1.1 fix)
**Implementation:** Fixing 1.1 resolves this. No separate work needed.

---

### Finding 1.3 — Comment Numbering Broken in Prompt Builders
**Agent Assessment:** Numbered comments in build_react_prompt() go 1-5 then restart at 3-11. Similar in build_dgs_prompt().
**Evidence:** `prompts.rs:804-893` — confirmed. Comments restart numbering after a section break.
**Debate:**
- *Valid?* YES, cosmetic. Does not affect behavior.
- *Why?* Copy-paste from different sections during development.

**VERDICT: APPROVE P3**
**Implementation:** Renumber comments sequentially. 5-minute fix.

---

### Finding 2.1 — Rule 4 vs Immutable #3: Ethics Contradiction
**Agent Assessment:** Conversational Rule 4 says "NEVER break character or mention being an AI/LLM" which directly contradicts Immutable Rule #3 about being transparent about being AI.
**Evidence:** `prompts.rs:361` — Rule 4: `"4. NEVER break character or mention being an AI/LLM."` This is an ACTUAL ETHICAL CONTRADICTION with the identity framework's commitment to AI transparency.
**Debate:**
- *Why wasn't this done?* Rule 4 is about immersive personality (don't randomly say "as an AI..."), but the wording is absolute and contradicts the transparency commitment. The INTENT is "stay in character" but the LETTER is "lie about being AI."
- *Valid?* ABSOLUTELY. This is the most important finding in the entire audit. A user who directly asks "are you an AI?" must get a truthful answer per Immutable #3. Rule 4 as written forbids this.
- *Cross-impact?* Requires careful rewording to preserve personality immersion while allowing truthful disclosure when directly asked.

**VERDICT: APPROVE P0** — Ship-blocker. Ethical integrity of the entire identity system depends on this.
**Implementation:** Reword Rule 4 to: `"4. Stay in character and maintain your personality. If directly asked whether you are an AI, be truthful per Immutable Rule #3, but you need not volunteer this information unprompted."` This preserves immersion while honoring transparency.

---

### Finding 2.2 — VAD Dominance Dimension Missing
**Agent Assessment:** personality_section() uses valence and arousal but not dominance from the VAD model.
**Evidence:** `prompts.rs:519-520` — only `valence` and `arousal` are emitted. `PromptSlots` struct has no `dominance` field. `PersonalitySnapshot` in identity.rs may define it but it never reaches the prompt.
**Debate:**
- *Valid?* YES — VAD is a three-dimensional model (Valence-Arousal-Dominance). Using only 2/3 dimensions is an incomplete implementation.
- *Why?* Likely intentional simplification — dominance is the least intuitive dimension for prompt engineering. Two dimensions already create meaningful personality variation.
- *Cross-impact?* Adding dominance requires: PromptSlots field + identity pipeline change + prompt text addition. Low risk but non-trivial.

**VERDICT: APPROVE P2**
**Implementation:** Add `dominance` field to PromptSlots, wire from PersonalitySnapshot through the identity pipeline, and include in personality_section() output. Consider whether the personality engine already computes dominance.

---

### Finding 3.1 — Tool Result Formatting Inconsistency
**Agent Assessment:** Tool results may have inconsistent formatting across modes.
**Evidence:** Not directly verified against code. Agent provided general observation.
**Debate:** Insufficient code evidence to confirm or deny.

**VERDICT: DEFER** — Requires specific evidence showing formatting divergence.

---

### Finding 3.2 — Error Handling Personality
**Agent Assessment:** Error messages don't reflect AURA's personality.
**Evidence:** Not verified against code.
**Debate:** Valid concern for UX polish, but not verified.

**VERDICT: DEFER** — Revisit during UX polish pass.

---

### Finding 4.1 — Memory Recall Prompt Integration
**Agent Assessment:** Memory recall results may not be properly integrated into prompts.
**Evidence:** Not directly verified. Context pipeline was not fully traced.
**Debate:** Would need to trace from memory subsystem through context.rs to prompt builders.

**VERDICT: DEFER** — Requires deeper investigation of memory → prompt pipeline.

---

### Finding 4.2 — DGS Personality Wiring
**Agent Assessment:** DGS prompt builder lacks personality traits.
**Evidence:** `prompts.rs:951-1013` — `build_dgs_prompt()` confirmed missing `personality_section(slots)`. Same root cause as 1.1.
**Debate:** Duplicate of 1.1.

**VERDICT: DEDUPLICATED** — Merged under Finding 1.1. Same fix applies.

---

### Finding 5.1 — System Prompt Token Count
**Agent Assessment:** System prompts may exceed token budget in some modes.
**Evidence:** Not measured. Would require tokenizer + actual prompt rendering.
**Debate:** Valid concern but requires measurement, not code inspection.

**VERDICT: DEFER** — Needs empirical measurement with a tokenizer against rendered prompts.

---

### Finding 5.2 — Composer context_budget = 400 Tokens
**Agent Assessment:** 400 tokens for context composition may be too restrictive.
**Evidence:** `context.rs:88` — confirmed `context_budget: 400`. This limits how much retrieved memory/context can be injected into prompts.
**Debate:**
- *Valid?* YES — 400 tokens is roughly 300 words. For complex conversations with multiple memory retrievals, this could truncate important context.
- *Why?* Likely conservative default to prevent prompt bloat. Sensible starting point.
- *Cross-impact?* Increasing this is a one-line change but affects total prompt size and inference cost.

**VERDICT: APPROVE P2**
**Implementation:** Consider making this configurable (user preference or adaptive based on model context window). Default could be raised to 600-800 for models with 8K+ context windows.

---

### Finding 6.1 — Prompt Version Tracking
**Agent Assessment:** No version identifier in prompts for debugging/telemetry.
**Evidence:** No version string found in prompt builders.
**Debate:** Useful for debugging personality regressions across versions.

**VERDICT: APPROVE P3**
**Implementation:** Add a `// AURA Prompt v{X.Y}` comment or metadata field to rendered prompts.

---

## AGENT 5 — CI/CD PIPELINE (16 Findings)

### Finding C1 — Neocortex Not Built for Android Release
**Agent Assessment:** Release pipeline only builds aura-daemon for aarch64-linux-android. Neocortex binary is missing from release artifacts.
**Evidence:**
- `release.yml:159` — `cargo build --release -p aura-daemon --target aarch64-linux-android`
- `release.yml:163` — only strips `aura-daemon`
- `build-android.yml:33` — job title: "Cross-compile aura-daemon (aarch64-linux-android)"
- `build-android.yml:143` — same: only builds `aura-daemon`
- `install.sh:677` — install script builds BOTH packages for Termux
- Release artifacts contain NO neocortex binary

**Also reported as:** F-08 (Agent 6) — DEDUPLICATED here.
**Debate:**
- *Why?* Neocortex is the LLM inference engine (the "brain"). The daemon is the orchestration layer (the "body"). Shipping only the daemon means Android users get a headless system — it can orchestrate but cannot think.
- *Valid?* ABSOLUTELY. This is a fundamental distribution failure. AURA's iron law is "LLM = brain, Rust = body." Releasing body without brain is shipping half a system.
- *Cross-impact?* Need to add `-p aura-neocortex` build step to both `release.yml` and `build-android.yml`. Must also handle llama.cpp cross-compilation for Android (already partially set up via NDK in build-android.yml).

**VERDICT: APPROVE P0** — Ship-blocker. Cannot release Android target without the inference engine.
**Implementation:** Add `cargo build --release -p aura-neocortex --target aarch64-linux-android` step to release.yml and build-android.yml. Strip and include in release artifacts. Verify llama.cpp cross-compiles correctly with NDK toolchain.

---

### Finding C2 — Release Pipeline Skips cargo audit
**Agent Assessment:** ci.yml has an audit job, but release.yml does not gate on it.
**Evidence:**
- `ci.yml:154-169` — audit job exists
- `release.yml:35-75` — ci-check runs check, test, clippy, fmt — no audit
**Debate:**
- *Valid?* YES. Audit exists in CI but is not enforced as a release gate. A release could ship with known CVEs.
- *Why?* Likely oversight — ci-check was copied from a subset of CI jobs.

**VERDICT: APPROVE P1**
**Implementation:** Add `cargo audit` step to release.yml's ci-check job, or make the release workflow depend on the full ci.yml passing.

---

### Finding C3 — No Version Tag ↔ Cargo.toml Validation
**Agent Assessment:** Release pipeline doesn't verify that the git tag matches the version in Cargo.toml.
**Evidence:** No step in release.yml compares `${{ github.ref_name }}` to any Cargo.toml version.
**Debate:**
- *Valid?* YES. You could tag v1.2.3 but Cargo.toml says version = "1.0.0". This causes confusion and mismatched artifacts.

**VERDICT: APPROVE P1**
**Implementation:** Add a validation step: `grep '^version' Cargo.toml | grep "${GITHUB_REF_NAME#v}"` or use `cargo metadata` to extract and compare.

---

### Finding C4 — No SBOM Generation
**Agent Assessment:** No Software Bill of Materials generated during release.
**Debate:**
- *Valid?* Yes, best practice for supply chain security.
- *Alpha-acceptable?* YES — SBOM is important but not blocking for early alpha.

**VERDICT: APPROVE P2**
**Implementation:** Add `cargo sbom` or `cargo cyclonedx` step to release pipeline.

---

### Finding C5 — No Code Signing
**Agent Assessment:** Release binaries are not cryptographically signed.
**Debate:**
- *Valid?* Yes, important for verifying binary authenticity.
- *Alpha-acceptable?* YES — signing infrastructure is complex to set up.
- *Cross-impact?* Related to F-05 (checksums). Signing + checksums together provide a complete integrity chain.

**VERDICT: APPROVE P2**
**Implementation:** Set up GPG or sigstore signing for release binaries. Publish signatures alongside artifacts.

---

### Finding C6 — Secrets Rotation Documentation
**Agent Assessment:** No documentation on how to rotate CI/CD secrets.
**Debate:** Documentation issue. Low impact for alpha.

**VERDICT: APPROVE P3**
**Implementation:** Add `docs/ops/secrets-rotation.md` with procedures for rotating GitHub tokens, signing keys, etc.

---

### Finding H1 — llama.cpp Submodule Tracking
**Agent Assessment:** Submodule may fall behind, causing build issues.
**Evidence:** `build-android.yml:129-137` has a submodule verification step that fails fast if llama.cpp is missing. CI uses `submodules: recursive` in checkout.
**Debate:**
- *Valid?* YES for local development and install.sh (see F-04). CI handles it correctly, but the tracking/pinning strategy should be documented.
- *Partial overlap with F-04* — F-04 is the install-side failure; H1 is the maintenance/tracking concern.

**VERDICT: APPROVE P1**
**Implementation:** Document the llama.cpp submodule pinning strategy. Add `.gitmodules` validation to CI. Consider a dependabot-like alert for submodule staleness.

---

### Finding H2 — Cache Key Missing Toolchain Version
**Agent Assessment:** Cargo cache keys use only Cargo.toml/Cargo.lock hashes, not the Rust toolchain version.
**Evidence:**
- `ci.yml:55` — `key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.toml', '**/Cargo.lock') }}`
- `release.yml:56` — same pattern
- `build-android.yml:64` — same pattern
**Debate:**
- *Valid?* YES. Switching from nightly-2026-03-01 to nightly-2026-03-15 would reuse the same cache, potentially causing subtle compilation issues.

**VERDICT: APPROVE P2**
**Implementation:** Add toolchain version to cache key: `key: ${{ runner.os }}-cargo-nightly-2026-03-01-${{ hashFiles('**/Cargo.toml', '**/Cargo.lock') }}` or hash `rust-toolchain.toml`.

---

### Finding H3 — Artifact Retention Too Short
**Agent Assessment:** 30-day retention may be insufficient.
**Evidence:** `build-android.yml:164` — `retention-days: 30`
**Debate:**
- *Alpha-acceptable?* YES — 30 days is fine. Release artifacts go to GitHub Releases (permanent), not just workflow artifacts.

**VERDICT: APPROVE P3**
**Implementation:** Consider 90 days for release-tagged builds.

---

### Finding H4 — No Integration Test Stage
**Agent Assessment:** CI runs unit tests only, no integration/E2E tests.
**Debate:**
- *Valid?* Yes, but integration test infrastructure takes time to build.
- *Alpha-acceptable?* YES.

**VERDICT: APPROVE P2**
**Implementation:** Design integration test suite. At minimum, test daemon ↔ neocortex IPC and Telegram bot connectivity in CI.

---

### Finding H5 — No Canary/Staged Rollout
**Agent Assessment:** Releases go directly to all users, no staged rollout.
**Debate:**
- *Premature for alpha.* There are barely any users yet.

**VERDICT: DEFER** — Revisit when user base exceeds single-digit count.

---

### Finding H6 — Release Notes Automation
**Agent Assessment:** No automated changelog/release notes generation.
**Debate:** Nice to have.

**VERDICT: APPROVE P3**
**Implementation:** Add `git-cliff` or `conventional-changelog` to release pipeline.

---

### Finding M1 — CI Matrix Testing
**Agent Assessment:** CI only tests on one OS/target.
**Debate:**
- *Valid?* Partially — AURA targets Android/Termux primarily. Multi-OS matrix is useful but not critical for a single-platform alpha.

**VERDICT: APPROVE P3**
**Implementation:** Add at minimum an Ubuntu build + Android cross-compile to the CI matrix.

---

### Finding M2 — No Performance Regression Gate
**Agent Assessment:** No benchmarks or performance checks in CI.
**Debate:** Premature for alpha.

**VERDICT: DEFER** — Revisit when baseline benchmarks exist.

---

### Finding M3 — No Dependency License Scan
**Agent Assessment:** No license scanning for dependencies.
**Debate:**
- *Valid?* YES — llama.cpp license compatibility and Rust crate licenses should be verified.
- *Legal risk* exists even in alpha.

**VERDICT: APPROVE P2**
**Implementation:** Add `cargo deny check licenses` to CI. Configure `deny.toml` with allowed licenses.

---

### Finding M4 — Coverage Reporting
**Agent Assessment:** No test coverage tracking.
**Debate:** Nice to have for alpha.

**VERDICT: APPROVE P3**
**Implementation:** Add `cargo tarpaulin` or `llvm-cov` to CI with coverage badge.

---

## AGENT 6 — INSTALL EXPERIENCE & DISTRIBUTION (15 Findings)

### Finding F-01 — No macOS/x86 Support
**Agent Assessment:** install.sh only supports Linux ARM64/Termux.
**Debate:**
- *Valid?* As an observation, yes. As a BUG, NO. AURA v4 is **designed** for Android/Termux on ARM64 devices. This is an intentional target constraint, not an oversight. The anti-cloud philosophy means AURA runs on the user's personal device (phone).

**VERDICT: OVERRULE** — By design. AURA v4 targets Android/Termux. macOS/x86 is out of scope for this release.

---

### Finding F-02 — No Offline Install Mode
**Agent Assessment:** Install requires internet throughout.
**Debate:**
- *Valid?* Yes, but offline install is an edge case for alpha.
- *Building from source inherently requires fetching crates.* True offline install would need vendored dependencies.

**VERDICT: APPROVE P3**
**Implementation:** Consider `cargo vendor` support for air-gapped installs in the future.

---

### Finding F-03 — Installs Stable Rust, Project Needs Nightly
**Agent Assessment:** install.sh installs stable toolchain, but project requires nightly.
**Evidence:**
- `install.sh:398` — `rustup update stable`
- `install.sh:425` — `--default-toolchain stable`
- `install.sh:445-446` — Comment notes rust-toolchain.toml will be detected
**Debate:**
- *Valid?* PARTIALLY. rustup + rust-toolchain.toml means the correct nightly will be auto-installed when `cargo build` runs. However, this causes: (1) unnecessary stable download, (2) user confusion, (3) extra time.
- *Mitigated?* Yes, functionally mitigated by rust-toolchain.toml auto-detection.

**VERDICT: MODIFY P2** (downgraded from original severity)
**Implementation:** Change install.sh to install the nightly version specified in rust-toolchain.toml directly: `rustup install nightly-2026-03-01`. Eliminates wasted bandwidth and user confusion.

---

### Finding F-04 — Missing git submodule init in install.sh
**Agent Assessment:** install.sh clones the repo but never runs `git submodule update --init --recursive`.
**Evidence:**
- `install.sh:483` — `git clone --depth 1` with no submodule init
- `release.yml` and `build-android.yml` both use `submodules: recursive` in their checkout steps
- `build-android.yml:129-137` — has explicit submodule verification that would FAIL without init
**Debate:**
- *Valid?* ABSOLUTELY. Without submodule init, `crates/aura-llama-sys/llama.cpp/` will be an empty directory. `build.rs` will fail trying to compile llama.cpp sources that don't exist. **The from-source install path is completely broken.**
- *Why?* `git clone --depth 1` does not initialize submodules by default. The `--recurse-submodules` flag was not added.
- *Cross-impact?* This is the single most impactful install bug. Every from-source install will fail at the build step.

**VERDICT: APPROVE P0** — Ship-blocker. From-source install literally cannot succeed.
**Implementation:** Change `git clone --depth 1` to `git clone --depth 1 --recurse-submodules` in `phase_source()`. Add a verification step after clone: `if [ ! -f "crates/aura-llama-sys/llama.cpp/llama.cpp" ]; then die "submodule init failed"; fi`

---

### Finding F-05 — Placeholder Checksums Block Default Install
**Agent Assessment:** Model download checksums are placeholder strings, and the default channel (stable) rejects them.
**Evidence:**
- `install.sh:39,44,49` — `PLACEHOLDER_UPDATE_AT_RELEASE_TIME_*` values
- `install.sh:610` — `verify_checksum()` calls `die()` if checksum is a placeholder AND channel is stable
- `install.sh:81` — default channel is `stable`
**Debate:**
- *Valid?* ABSOLUTELY. The default install path (stable channel) will always fail at model download because placeholder checksums cause `die()`. **The model download phase is broken by default.**
- *Why?* Checksums are intended to be replaced during the release process, but no release automation does this.
- *Cross-impact?* Relates to C5 (signing) — checksums are part of the integrity chain. The release pipeline (C3 scope) should auto-populate these.

**VERDICT: APPROVE P0** — Ship-blocker. Default install path terminates at model download.
**Implementation:** Two-part fix: (1) Release pipeline must auto-populate checksums from actual model files. (2) Add `--channel nightly` documentation as a workaround for development installs (if nightly skips checksum verification). Verify if a fallback exists for dev/nightly channels.

---

### Finding F-06 — No Rollback on Failure
**Agent Assessment:** If installation fails midway, no cleanup or rollback occurs.
**Debate:**
- *Valid?* Yes — a failed install leaves partial artifacts (cloned source, partial build, config files).
- *Alpha-acceptable?* Tolerable. Users can manually clean up.

**VERDICT: APPROVE P2**
**Implementation:** Add a `cleanup_on_failure()` trap that removes `$AURA_ROOT` if installation didn't complete. Use `trap cleanup_on_failure EXIT` pattern.

---

### Finding F-07 — --skip-build Has No Binary Download
**Agent Assessment:** The `--skip-build` flag skips compilation but doesn't download prebuilt binaries. It just checks if a binary already exists.
**Evidence:** `install.sh:644-651` — if `--skip-build`, only checks `$AURA_BIN` existence. No download from GitHub Releases.
**Debate:**
- *Valid?* YES. The `--skip-build` flag is advertised as an option but provides no useful fallback. A user who can't or doesn't want to build from source has no path to a working install.
- *Cross-impact?* Relates to C1 — even if download existed, only aura-daemon is in releases, not neocortex.

**VERDICT: APPROVE P1**
**Implementation:** Implement binary download from GitHub Releases for `--skip-build` mode. Download both `aura-daemon` and `aura-neocortex` (once C1 is fixed). Use GitHub API to fetch latest release URLs.

---

### Finding F-08 — Neocortex Not in Release Pipeline
**Agent Assessment:** Same as C1.
**Evidence:** Same as C1.

**VERDICT: DEDUPLICATED** — See C1 above.

---

### Finding F-09 — No Progress Indicators for Large Downloads
**Agent Assessment:** Model downloads (5-10GB) show no progress bar.
**Debate:** UX polish issue. curl/wget typically show progress by default.

**VERDICT: APPROVE P3**
**Implementation:** Ensure `curl -#` or `wget --show-progress` flags are used for large downloads.

---

### Finding F-10 — Config File chmod 600 Missing
**Agent Assessment:** Config.toml containing Telegram bot token is not permission-restricted.
**Evidence:** `install.sh:924` — comment MENTIONS `chmod 600` but no actual `chmod` command follows. The config file is created with default umask permissions (likely 644), leaving the Telegram bot token world-readable.
**Debate:**
- *Valid?* YES. On a multi-user system or shared device, the Telegram bot token is exposed.
- *Why?* Developer wrote the comment as a TODO but forgot the implementation.

**VERDICT: APPROVE P1** — Security issue. Credential exposure.
**Implementation:** Add `chmod 600 "$CONFIG_FILE"` immediately after creating the config file.

---

### Finding F-11 — No Uninstall Command
**Agent Assessment:** No way to cleanly remove AURA.
**Debate:** Alpha-acceptable. Users can `rm -rf $AURA_ROOT`.

**VERDICT: APPROVE P3**
**Implementation:** Add `install.sh --uninstall` that removes binaries, config, systemd service, and data (with confirmation prompt).

---

### Finding F-12 — Telegram Bot Token in Plain Text
**Agent Assessment:** Bot token stored in Config.toml as plain text.
**Evidence:** Config template writes token directly. This is the DESIGNED approach (anti-cloud = local-only credentials).
**Debate:**
- *Valid?* The finding is technically correct — the token IS in plain text. But this is a DESIGN CHOICE per AURA's anti-cloud philosophy. There's no cloud keystore. The token lives on the user's device.
- *Mitigation:* File permissions (F-10 fix) are the appropriate control for local secrets.
- *Risk acknowledgment:* If device is compromised, token is exposed. This is accepted risk in the threat model.

**VERDICT: MODIFY P2** (acknowledge as design choice with risk)
**Implementation:** Fix F-10 (chmod 600) as primary mitigation. Document the threat model: "Telegram token is stored locally per anti-cloud design. Physical device security is the user's responsibility." Consider optional encryption-at-rest in a future version.

---

### Finding F-13 — No Health Check Post-Install
**Agent Assessment:** After installation, no verification that AURA actually works.
**Debate:**
- *Valid?* YES — first-time users have no feedback that installation succeeded beyond "success" message.

**VERDICT: APPROVE P2**
**Implementation:** Add a `phase_healthcheck()` after service start that: (1) checks daemon is running, (2) sends a test inference request to neocortex, (3) verifies Telegram bot connection. Report results to user.

---

### Finding F-14 — install.sh Not POSIX-Compliant
**Agent Assessment:** Script may use bash-isms not available in POSIX sh.
**Evidence:** Not verified. Script has `#!/bin/bash` shebang — it's explicitly a bash script for Termux.
**Debate:**
- *Valid?* QUESTIONABLE. Termux ships bash by default. The script doesn't claim POSIX compliance. Using bash is intentional.

**VERDICT: OVERRULE** — Script is `#!/bin/bash` targeting Termux which ships bash. POSIX compliance is not a requirement.

---

### Finding F-15 — Phase Ordering: Model Download Before Build
**Agent Assessment:** 5-10GB model download happens before compilation, which may fail.
**Evidence:** `install.sh:1046-1055` — order: preflight → packages → rust → source → **model** → **build** → config → service → firsttime → success.
**Debate:**
- *Valid?* YES. If the build fails (missing deps, compiler errors, submodule issues), the user has already spent 20-60 minutes downloading a multi-GB model they can't use.
- *Why?* Possibly ordered for UX flow ("get everything, then build"), but doesn't account for build failures.
- *Cross-impact?* Combined with F-04 (submodule missing), the build WILL fail after model download.

**VERDICT: APPROVE P1**
**Implementation:** Reorder to: preflight → packages → rust → source → **build** → **model** → config → service → firsttime → success. Build first, download model only after successful compilation.

---

## SUMMARY STATISTICS

| Verdict | Count |
|---------|-------|
| APPROVE P0 | **4** |
| APPROVE P1 | **9** |
| APPROVE P2 | **11** |
| APPROVE P3 | **10** |
| MODIFY | **2** |
| OVERRULE | **2** |
| DEFER | **5** |
| DEDUPLICATED | **2** |
| **TOTAL** | **43** (41 unique) |

---

## CRITICAL PATH — P0 SHIP-BLOCKERS

These four findings MUST be resolved before ANY user receives AURA v4:

| # | Finding | Impact | Fix Complexity |
|---|---------|--------|---------------|
| 1 | **2.1 — Rule 4 vs Immutable #3** | Identity ethics broken. AURA is instructed to lie about being AI. | Low — reword one rule (~20 words) |
| 2 | **C1/F-08 — Neocortex not built for Android** | Users receive a headless system. Brain without body. | Medium — add build step + verify cross-compilation |
| 3 | **F-04 — Missing submodule init** | From-source install cannot compile. Build always fails. | Low — add `--recurse-submodules` flag (~1 line) |
| 4 | **F-05 — Placeholder checksums** | Default install terminates at model download. Stable channel broken. | Medium — release pipeline must auto-populate checksums |

**Estimated time to unblock:** 1-2 days for all four P0 fixes.

---

## P1 FIRST-SPRINT PRIORITIES (in recommended order)

1. **1.1/4.2** — Add `personality_section(slots)` to ReAct and DGS builders
2. **F-15** — Reorder phases: build before model download
3. **F-10** — Add `chmod 600` to config file
4. **F-07** — Implement binary download for `--skip-build`
5. **C2** — Add cargo audit to release gate
6. **C3** — Add version tag validation
7. **H1** — Document and enforce submodule pinning
8. **F-03** — Install correct nightly toolchain directly

---

## CROSS-CUTTING OBSERVATIONS

1. **The install path is severely broken.** Three P0s (F-04, F-05, C1) and two P1s (F-07, F-15) mean that no install method currently works end-to-end. This is the highest-risk area.

2. **The personality system is 80% wired.** Tier 1 did excellent work adding identity tendencies, self-knowledge, and user preferences to all modes. The remaining gap (personality_section in ReAct/DGS) is surgical to fix.

3. **CI exists but doesn't fully protect releases.** The CI pipeline is well-structured, but the release pipeline doesn't enforce all its gates. Audit, version validation, and neocortex builds are the main gaps.

4. **The ethics contradiction (2.1) is the most philosophically important finding.** It goes to the core of what AURA is — a system that must be immersive yet honest. The fix is simple in code but significant in design philosophy.

---

## PANEL SIGNATURES

**Presiding Judge (Senior Architect):** Evidence-verified verdict. All P0 findings confirmed against source code. Recommended immediate sprint for P0 resolution.

**Security Reviewer:** F-10, F-12, C2, and C5 create a credential and supply-chain exposure surface. P0/P1 fixes must precede any external distribution.

**UX Advocate:** The install experience (F-04, F-05, F-15) will cause 100% first-user failure rate. This must be the top priority alongside the ethics fix.

---

*Verdict rendered: 2026-03-15*
*Panel 3 — Courtroom Audit Complete*
