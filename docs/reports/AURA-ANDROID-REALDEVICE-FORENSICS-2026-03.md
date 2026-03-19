# AURA Android Real-Device Forensics (March 2026)

Status: Active investigation log (cross-domain)
Owner: Core Runtime + Mobile Systems + DevOps/Release + Installer/Ops

---

## 1) Mission

Run AURA end-to-end on a real Termux device (install, daemon, neocortex, IPC, Telegram) with production-grade reliability.

Primary requirement: `curl ... | bash` (especially `--skip-build`) must install runnable binaries, not stale or broken artifacts.

---

## 2) Confirmed Facts (Evidence)

### Earlier field failures (real device)
- `aura-neocortex` failed at runtime with dynamic linker error:
  - `library "libc++_shared.so" not found`
- `aura-daemon` could still start and print version.

### CI / release progression
- Android build linkage was repaired in stages:
  1. static libc++ archive discovery pathing
  2. explicit C++ ABI linkage for unresolved `__cxa*`/`__gxx_personality*` symbols
- Main branch Build Android and Release workflows later passed.
- Tag/release `v4.0.0-alpha.6` published with daemon + neocortex assets and `.sha256` sidecars.

### Real-device state mismatch (critical)
- Installer banner showed `4.0.0-alpha.6`.
- Runtime probe still reported:
  - `aura-daemon 4.0.0-alpha.5`
  - `aura-neocortex` still linked against missing `libc++_shared.so`.
- Binary timestamps/sizes matched older artifacts, not alpha.6 release sizes.

Conclusion: device was running stale local binaries despite newer installer/release metadata.

---

## 3) Root-Cause Model (Cross-domain)

### Runtime/Core
- No current evidence that core Rust architecture is globally corrupted.
- Observed failures explained by wrong binaries present on device.

### Mobile Systems
- Android linker behavior is authoritative truth source on device.
- Device logs are higher priority than CI assumptions.

### DevOps/Release
- CI validated build/link/package invariants for release artifacts.
- CI cannot guarantee device has actually pulled/replaced those artifacts.

### Installer/Ops (primary fault line)
- In `--skip-build`, installer could early-return when local binaries existed (`Using existing binaries`).
- This permits stale binary reuse across reinstalls/upgrades.

### Governance / process
- Past loop pattern: one issue fixed, next appears.
- Corrected approach: forensic baseline -> deterministic cleanup -> manual artifact truth test -> then installer patch.

---

## 4) Device Forensic Steps Run

### A) Baseline checks
- Version/probe checks repeatedly showed alpha.5 daemon + neocortex linker failure.

### B) Cleanup
- AURA binaries, service dirs, config/data/source were removed.
- Verification showed target paths missing and commands absent.

### C) Manual alpha.6 artifact test (first attempt)
- Binary downloads started.
- Failure occurred on checksum sidecar write to `/tmp`:
  - `Permission denied` writing `/tmp/aura-daemon.sha256`.

Interpretation:
- This is an execution-environment path issue (`/tmp` portability), not a proof that alpha.6 binaries are bad.

### D) Manual alpha.6 artifact test (second attempt, `$PREFIX/tmp`)

Observed results:
- Downloads succeeded for both binaries.
- SHA256 matched published release sidecars exactly:
  - daemon: `8edab09dec9b730d2eaa4d0e7eef785efcb7819ca98d0507256a213c0ce5c1d0`
  - neocortex: `7f3c72e397fa2a77890802db5ba5944b56badc82764a7e95dd315e857582ff84`
- Runtime probes for both binaries failed with `Segmentation fault` (exit 139).

Implications:
- Not a stale-download issue.
- Not a checksum/integrity issue.
- Not a missing-file issue.
- Runtime crash now becomes primary root class.

### E) Forensic traces

From collected `strace` + `logcat`:
- Both binaries die with `SIGSEGV / SEGV_MAPERR / si_addr=NULL` very early.
- Crash signatures are structurally identical across daemon and neocortex.
- `LD_PRELOAD` in environment is set to:
  - `/data/data/com.termux/files/usr/lib/libtermux-exec-ld-preload.so`
- Loader opens `libtermux-exec-ld-preload.so` before crash.
- `crash_dump64` helper cannot exec due app sandbox policy (`EPERM`), so symbolized Android tombstone is unavailable (expected under this context).

Interpretation (updated):
- The strongest common factor is not feature logic, but process bootstrap/runtime environment.
- Candidate fault domains now include:
  1) binary construction/toolchain ABI
  2) runtime loader interaction with Termux preload (`LD_PRELOAD`)
  3) post-link manipulation/strip effects

---

## 5) Patterns Noticed

1. **State drift risk**: install metadata can update while local binaries remain old.
2. **Operator transport risk**: command relay through chat can mutate operators and break scripts.
3. **Path portability risk**: `/tmp` is not universally safe in all Termux contexts.
4. **Verification semantics gap**: warning-only post-install probe can hide hard runtime failures.

---

## 6) Immediate Remediation Plan (No guessing)

1. Run differential execution tests with and without Termux preload:
   - baseline: current shell env
   - `LD_PRELOAD=` (empty)
   - `env -i` minimal environment
2. If runtime passes when preload is removed:
   - Patch installer to force release refresh in `--skip-build` unless explicit local-binary override.
   - Add runtime launch wrappers that sanitize `LD_PRELOAD` for Aura binaries.
   - Add version mismatch detection (installed binary vs target tag).
   - Promote neocortex probe failure from warning to hard failure.
3. If runtime still fails even with preload removed:
   - Re-open artifact/runtime packaging root cause and inspect downloaded binary dependencies directly on device.
   - Build unstripped comparison artifact and run side-by-side to isolate strip/toolchain effects.

---

## 7) Governance Rules (Active)

1. No multi-change speculative patches.
2. Every change declares hypothesis, expected outcome, rollback criteria.
3. One in-progress experiment at a time.
4. Every failed run updates this document (or linked decision log).
5. No completion claims without evidence bundle from CI + real device.

---

## 8) Evidence Anchors

- Release tag: `v4.0.0-alpha.6`
- Release workflow run: `23253150920` (success)
- Main CI and Build Android post-merge runs: passed
- Device logs: repeated alpha.5 daemon output + neocortex `libc++_shared.so` failure before cleanup
- Device logs: cleanup success + `/tmp` permission issue during manual checksum fetch

---

## 9) Open Questions

1. Do binaries run when `LD_PRELOAD` is cleared (`LD_PRELOAD=`)?
2. Should installer default behavior always replace existing binaries on `--skip-build`?
3. Should config regeneration mode be explicit (`--regenerate-config`) for recovery scenarios while preserving default non-destructive behavior?
4. Is `llvm-strip` introducing runtime-invalid output on this device class, or is crash independent of strip?

---

## 10) Hypothesis Tree (Post-segfault Evidence)

Current evidence (both binaries, identical early SIGSEGV at NULL, authentic checksums) points to a shared startup/runtime failure domain.

### H1 — Termux preload/environment interaction (Confidence: 0.45)

Why plausible:
- `LD_PRELOAD` is set to `libtermux-exec-ld-preload.so`.
- Both binaries fail identically before app-level behavior diverges.
- Strace shows preload path opening before crash.

Discriminator:
- Run binaries with `LD_PRELOAD=` and minimal env. If crash disappears, H1 confirmed.

### H2 — Strip-induced runtime invalidation (Confidence: 0.20)

Why plausible:
- Release workflow strips binaries before publish.
- If section/program header handling is wrong, startup can break.

Discriminator:
- Compare runtime of stripped release binaries vs unstripped build artifacts from same commit.

### H3 — Toolchain / ABI / linker construction mismatch (Confidence: 0.35)

Why plausible:
- Identical startup crash across binaries can originate from common build/link configuration.
- Prior linkage interventions changed C++ runtime behavior significantly.

Discriminator:
- Build equivalent artifacts via canonical `cargo-ndk` pipeline and compare runtime.

### Decision Gate Order (strict)

1. H1 preload differential test
2. H2 stripped vs unstripped differential
3. H3 cargo-ndk/toolchain differential

No additional release/toolchain patches before this gate sequence resolves.

---

## 11) PR1 Installer Contract Hardening — Local Implementation Delta (2026-03-19)

Scope implemented locally in working tree (`install.sh`, `verify.sh`):

1. **`--skip-build` existing binary reuse hardened**
   - Existing binaries are no longer reused by presence-only check.
   - Reuse now requires:
     - daemon runtime probe success (`--version`), and
     - daemon version matching target release tag version, and
     - neocortex runtime probe success (`--help`).
   - Any mismatch/probe failure forces release artifact re-download.

2. **Runtime failure classification added in installer path**
   - Failure classes emitted during probe failures:
     - `startup segfault (exit 139)`
     - `linker missing dependency`
     - `unknown runtime failure`

3. **Installer phase verify now hard-fails on runtime probe failure**
   - daemon non-response to `--version` marks runtime probe failure.
   - neocortex non-response to `--help` marks runtime probe failure.
   - runtime probe failures now terminate install with non-zero exit.

4. **`verify.sh` failure taxonomy surfaced in summary**
   - Added failure-code classification and summary list:
     - `F001_STARTUP_SEGFAULT`
     - `F002_DYNAMIC_LINKER_DEPENDENCY`
     - `F099_UNKNOWN_RUNTIME`
   - daemon startup test now captures exit classification when process dies early.

Validation status for this delta:
- Shell syntax checks passed (`bash -n install.sh`, `bash -n verify.sh`).
- Real-device runtime validation pending (must be executed before merge/release claims).
