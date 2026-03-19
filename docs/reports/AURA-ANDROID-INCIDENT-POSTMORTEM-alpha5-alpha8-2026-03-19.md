# AURA Android Incident Postmortem (alpha.5 → alpha.8)

Status: **Open incident (containment active)**  
Date: 2026-03-19  
Scope: Android/Termux prebuilt (`--skip-build`) runtime viability for `aura-daemon` and `aura-neocortex`

---

## 0) Executive Summary

Between `v4.0.0-alpha.5` and `v4.0.0-alpha.8`, the team improved installer honesty and fail-fast behavior, but still shipped Android artifacts that fail runtime startup on real devices.

Two independent devices reproduced the same failure class:

- `Downloaded aura-daemon failed runtime probe (--version)`
- `Failure class: startup segfault (exit 139)`

This is **not a single-device anomaly**. It is a release-system failure where CI/build success was treated as sufficient without mandatory real-device runtime gating.

---

## 1) Incident Definition

### Incident ID
`INC-AURA-ANDROID-F001-2026-03`

### Trigger condition
Release artifacts published to channel are checksum-valid but non-runnable on real Android/Termux devices.

### User-facing symptom
Installer downloads verified artifacts then aborts on runtime probe with startup segfault.

### Severity
**SEV-1 (release integrity breach)**

Rationale:
- Core onboarding path (`curl ... | bash --skip-build`) fails for real users.
- Failure reproduced across more than one physical device.

---

## 2) Evidence Bundle (Ground Truth)

Primary evidence files:

- `docs/reports/19-03-2026/03_install_skipbuild.log`
- `docs/reports/19-03-2026/08_logcat_filtered.txt`
- `docs/reports/19-03-2026/V8/02_repair_build_only.log`
- `docs/reports/19-03-2026/V8/ALL_IN_ONE.txt`
- user-reported fresh-device run output (2026-03-19)

Key observed facts:

1. Installer tag/channel resolution is correct (`alpha.8` metadata, stable tag points to `v4.0.0-alpha.8`).
2. Release artifact download and checksum verification succeed.
3. Runtime probe fails immediately with `exit 139` (`SIGSEGV` / `SEGV_MAPERR` / fault addr `0x0`).
4. Same failure class appears on original and fresh device.
5. Installer removes failed binaries after probe failure by design, so subsequent direct probes may show “missing binary”.

Conclusion from evidence:

> Artifact authenticity is true; artifact runnability is false.

---

## 3) Timeline (alpha.5 → alpha.8)

### alpha.5 line

- `763ac49` — linked `c++_static` for Android neocortex builds.

### alpha.6 line

- `a27d5f4` — explicit Android NDK native search roots in workflows + build script.
- `81fe9ab` — explicit `c++abi` linkage for unresolved C++ ABI symbols.
- `40460f9` tagged as `v4.0.0-alpha.6` metadata release line.

### alpha.7 line

- `f7804f4` — hardened installer runtime validation and stale-binary reuse checks.
- `cb50579` tagged as `v4.0.0-alpha.7` metadata line.

### alpha.8 line

- `172d224` tagged as `v4.0.0-alpha.8`:
  - skip-build fail-fast ordering (binary validation before model download).
- `16fd8a7` post-tag metadata sync to alpha.8 in main.

### Field/runtime outcome

- Despite these changes, alpha.8 prebuilt daemon runtime probe fails on real devices with `exit 139`.

---

## 4) What Worked vs What Failed

## Worked

1. Installer now blocks false-success installs (hard-fail on runtime probe failure).
2. Skip-build path now avoids multi-GB model waste before binary viability check.
3. Stale-local-binary reuse behavior was tightened.

## Failed

1. Producer-side release system still allowed non-runnable artifacts to be published.
2. CI green remained an insufficient proxy for field runnability.
3. Governance drift occurred (tag cut vs post-tag metadata updates), increasing release-state ambiguity.

---

## 5) Root Cause (System-Level)

This incident is primarily a **validation architecture failure**, not merely a single code bug.

### Primary root cause

No mandatory producer-side real-device runtime gate was required for release promotion.

### Contributing factors

1. Host/stub-heavy CI checks did not fully represent deployed Android runtime conditions.
2. Runtime dependency checks were necessary but not sufficient.
3. Release process optimized for build completion, not field startup truth.

---

## 6) Non-Causes (from evidence)

The following are not supported as primary causes for this incident:

1. User forgetting to clean device state (fresh-device reproduces failure).
2. Corrupted download (checksums match release assets).
3. Wrong installer tag being used (tag and version checks are correct).

---

## 7) Process Failure Analysis

## Failure mode A: CI-green/device-red promotion

- Build and release workflows can pass without proving startup viability on representative physical devices.

## Failure mode B: Reactive guardrails, delayed producer controls

- Installer became safer, but it was used as last-line detection for bad artifacts already shipped.

## Failure mode C: Release state ambiguity

- Tag and metadata synchronization not strictly single-cut, single-source.

---

## 8) Immediate Containment (Active)

1. Treat current Android prebuilt line as incident-affected until producer-side runtime gate exists.
2. Freeze stable promotion of Android artifacts that have not passed real-device runtime proof.
3. Use installer hard-fail as user safety control, not as release acceptance substitute.

---

## 9) Corrective Actions (Systemic)

## CA-1 (P0) — Release runtime contract gate

Block release publish/promotion unless runtime probes pass on required device set.

## CA-2 (P0) — Immutable release manifest

For each release: bind `tag -> commit SHA -> workflow run IDs -> asset SHA256 -> runtime proof bundle`.

## CA-3 (P1) — Promotion state machine

Enforce `dev -> freeze -> rc -> stable`, no direct stable promotion from raw tag.

## CA-4 (P1) — Failure taxonomy integration

Use `F001/F002/F099` as machine-gated release signals, not only installer UX signals.

## CA-5 (P1) — Drift prevention

Require version/tag/docs/config synchronization in one release cut or force a new tag.

---

## 10) Lessons Learned

1. Checksum-valid artifact ≠ runnable artifact.
2. Installer fail-fast protects users but does not make release correct.
3. Runtime truth must be a producer obligation, not end-user burden.

---

## 11) Accountability & Ownership

Assigned owner domains:

- Build/Toolchain Owner — deterministic artifact construction
- Platform Reliability Owner — real-device startup viability
- Validation/QA Owner — runtime contract gates and device matrix
- Release Governance Owner — promotion policy and rollback controls
- Installer Owner — user safety messaging and deterministic fail-fast UX

---

## 12) Exit Criteria for Incident Closure

Incident is closed only when all conditions are met:

1. Android artifacts pass producer-side runtime probes on required device matrix.
2. Release promotion policy enforces runtime gate before stable.
3. Immutable release manifest exists and is verified by installer/ops tooling.
4. One full RC-to-stable cycle completes without F001 recurrence.

---

## 13) Final Verdict

This was a **system-design and release-governance miss**, not user misuse.

The team improved tactical safety controls (good), but strategic reliability controls were incomplete (critical).  
Future work must prioritize validation architecture and release governance before further feature/runtime expansion.
