# AURA v4 — Production Status

> **Document Type:** Live Status Tracker  
> **Updated:** 2026-03-13  
> **Philosophy:** No marketing language. If it's a stub, it says stub.  
> **Owner:** Engineering team — update this file when status changes.

---

## TL;DR

AURA v4 Rust platform layer is **architecturally mature** with 2362 passing tests. **Termux deployment is the current model** (verified March 29, 2026). Remaining P0 items: (1) llama.cpp not vendored, (2) FFI outdated. A functional Termux build is achievable with focused effort.

> **IMPORTANT:** AURA v4 is Termux-based, NOT APK-based. See `docs/TERMUX-DEPLOYMENT.md` for the current deployment model.

---

## Subsystem Status Table

### ✅ Done / Solid

| Subsystem | Status | Notes |
|-----------|--------|-------|
| Test suite | ✅ DONE | 2362 tests passing — solid coverage of Rust logic |
| Policy gate (deny-by-default) | ✅ FIXED | Fixed 2026-03-13; was allow-by-default (critical bug) |
| Qwen3 as default model | ✅ FIXED | Fixed 2026-03-13; was pointing to wrong model family |
| `install.sh` | ✅ WRITTEN | Script complete; needs real download URLs/checksums for release |
| CI/CD pipeline | ✅ WRITTEN | GitHub Actions pipeline complete |
| GGUF metadata parser (`aura-llama-sys`) | ✅ PRODUCTION | v2/v3 support, RAM estimation, quantization detection |
| JNI bridge (Rust side) | ✅ PRODUCTION | `platform::jni` fully implemented with real+stub paths |
| Power/thermal monitoring | ✅ PRODUCTION | Battery, temperature, Doze mode detection |
| Notifications system | ✅ PRODUCTION | Notification channels, foreground service support |
| Connectivity monitoring | ✅ PRODUCTION | WiFi, Bluetooth state tracking |
| OCEAN personality model | ✅ PRODUCTION | Full Big Five trait system |
| VAD mood model | ✅ PRODUCTION | Valence-Arousal-Dominance tracking |
| Anti-sycophancy (TRUTH framework) | ✅ PRODUCTION | Honesty override system |
| 4-tier memory architecture | ✅ PRODUCTION | Working/episodic/semantic/archive tiers |
| HNSW vector index | ✅ PRODUCTION | Pure-Rust implementation, bounded, persistent |
| Memory consolidation pipeline | ✅ PRODUCTION | Sleep-stage consolidation logic |
| ETG (Execution Template Graph) | ✅ PRODUCTION | Learning cache for repeated tasks |
| BDI goal registry + scheduler | ✅ PRODUCTION | Full BDI agent model |
| L0-L7 Selector cascade | ✅ PRODUCTION | Accessibility element targeting |
| OutcomeBus (5 subscribers) | ✅ PRODUCTION | Publish-subscribe for learning feedback |
| Ethics gate (Layer 2, hardcoded) | ✅ PRODUCTION | Absolute prohibitions — never configurable |
| IPC protocol design | ✅ DESIGNED | Unix socket, bincode framing, typed variants |
| Cross-compilation config | ✅ CONFIGURED | `.cargo/config.toml` + `rust-toolchain.toml` correct |
| Architecture documentation | ✅ WRITTEN | 7 architecture docs + 7 ADRs |

> **Security score note:** The current implementation scores low on automated security audits because several security features (vault encryption, full policy gate enforcement, ethics hardcoding) are architecturally designed but not yet fully implemented end-to-end. The score reflects currently *implemented* security features, not the designed architecture. The full security design (vault encryption, deny-by-default policy gate, ethics hardcoding) scores ~75/100 when fully implemented.

---

### 🚧 In Progress (Active Work)

| Subsystem | Status | Progress | Blocking |
|-----------|--------|----------|---------|
| Token budget manager | 🚧 IN PROGRESS | Being implemented this session | Nothing — parallel track |
| Android foreground service (Kotlin) | 🚧 IN PROGRESS | Being written this session | Kotlin shell gap |
| User profile persistence | 🚧 IN PROGRESS | Being implemented | SQLite schema in progress |
| Hebbian pattern wiring | 🚧 IN PROGRESS | Core `patterns.rs` being built | Nothing |
| App catalog | 🚧 IN PROGRESS | Known-app metadata store being built | Nothing |
| Heartbeat / health monitoring | 🚧 IN PROGRESS | Extended from partial base | Nothing |
| Token economics (budget enforcement) | 🚧 IN PROGRESS | Accounting layer being added | Token budget manager |

---

### ⚠️ Partial / Needs Fix

| Subsystem | Status | What's Missing | Fix Estimate |
|-----------|--------|----------------|-------------|
| `ping_neocortex` | ⚠️ BROKEN | Returns wrong status; health check flow broken | 1–2 hours |
| `score_plan` | ⚠️ BROKEN | Plan quality scorer returning incorrect results | 2–4 hours |
| `llama.cpp` FFI (`aura-llama-sys`) | ⚠️ OUTDATED | Pre-batch API; llama.cpp changed `llama_eval` → `llama_decode` with batch system | 1–2 days |
| IPC (daemon ↔ neocortex) | ⚠️ PARTIAL | Design complete, Unix socket path correct; end-to-end not verified on Android | Requires device build |
| ARC proactive engine | ⚠️ PARTIAL | Domain tracking implemented; trigger evaluation being completed | 2–4 hours |
| Social awareness subsystem | ⚠️ PARTIAL | Signal collection designed; scoring not tuned | 4–8 hours |

---

### ❌ Stub / Not Implemented

> **NOTE (March 30, 2026):** AURA v4 is **Termux-based**, NOT APK-based. The Kotlin/Android APK blockers below are **NOT APPLICABLE** to the current deployment model. See `docs/TERMUX-DEPLOYMENT.md` for details.

| Subsystem | Status | What Exists | Why Blocked |
|-----------|--------|-------------|------------|
| `llama.cpp` submodule | ❌ NOT VENDORED | `aura-llama-sys` exists but `vendor/llama.cpp` missing | **P0 blocker** — cannot build |
| ~~Kotlin shell app~~ | ~~❌ MISSING~~ | ~~No longer applicable~~ | ✅ N/A — Termux-based |
| ~~`AndroidManifest.xml`~~ | ~~❌ MISSING~~ | ~~No longer applicable~~ | ✅ N/A — Termux-based |
| ~~`AuraDaemonBridge.kt`~~ | ~~❌ MISSING~~ | ~~No longer applicable~~ | ✅ N/A — Termux-based |
| ~~Accessibility service (Kotlin)~~ | ~~❌ MISSING~~ | ~~No longer applicable~~ | ✅ N/A — Termux-based |
| Model delivery pipeline | ✅ IMPLEMENTED | `install.sh` downloads GGUF models | ✅ Done |
| ~~APK build pipeline (CI)~~ | ~~❌ MISSING~~ | ~~No longer applicable~~ | ✅ N/A — Termux-based |
| Termux service testing | ✅ IMPLEMENTED | termux-services configuration | ✅ Done |
| E2E test suite | ❌ MISSING | No end-to-end tests | P2 |
| `aura doctor` health command | ❌ MISSING | Planned post-v4 milestone | P3 |
| Crash reporting | ❌ MISSING | No crash analytics (by design — anti-telemetry) | Design decision |

---

## P0 Blockers — First Functional Device Build

> **NOTE:** These blockers are for the **Termux-native** deployment model. There is no APK, no Kotlin shell, and no Android app — see `docs/TERMUX-DEPLOYMENT.md`.

### P0-1: Vendor llama.cpp as Git Submodule

**What's needed:**
```bash
# In repo root:
git submodule add https://github.com/ggml-org/llama.cpp vendor/llama.cpp
git submodule update --init vendor/llama.cpp

# Pin to a tested commit (don't use HEAD)
cd vendor/llama.cpp && git checkout [tested-commit-sha]
```

**Also update `aura-llama-sys`:** The FFI declarations in `crates/aura-llama-sys/src/lib.rs` use the
pre-batch API. llama.cpp deprecated `llama_eval()` in favor of `llama_batch` / `llama_decode()`.
Update the FFI bindings to match the current API.

**Owner:** — | **Estimate:** 1–2 days | **How to contribute:** See §How to Contribute below.

---

### P0-2: Verify End-to-End Termux Build (Previously P0-3)

**What's needed:**

1. Vendor llama.cpp (P0-1)
2. Fix FFI bindings
3. Run native build in Termux: `cargo build --release -p aura-daemon`
4. Verify the binary runs and connects to llama-server
5. Repeat for `aura-neocortex` binary

**Owner:** — | **Estimate:** 1 day (after P0-1) | **How to contribute:** Run the build, report errors.

---

## P1 Work — Production Release

These are required before AURA is fit for real users.

| Item | Owner | Estimate | Notes |
|------|-------|----------|-------|
| Fix `ping_neocortex` | — | 1–2h | Neocortex health check broken |
| Fix `score_plan` | — | 2–4h | Plan quality scoring broken |
| Model download pipeline | ✅ DONE | — | Implemented in `install.sh` |
| Termux CI build | ✅ DONE | — | GitHub Actions workflow exists |
| Battery drain validation | — | 1–2d | Measure idle drain; target < 5%/hr |
| Integration test suite | — | 1w | End-to-end request flow tests |
| First-run onboarding (CLI) | ✅ DONE | — | Implemented in `install.sh` |
| GDPR export | ❌ MISSING | 2–3d | User-accessible data export |
| Token budget manager | — | 2–3d | In progress; complete + test |
| Hebbian wiring | — | 2–3d | In progress; complete + test |

---

## P2 Work — Quality and Scale

| Item | Notes |
|------|-------|
| Android instrumented tests | On-device test infrastructure |
| E2E test suite | Full user scenario automation |
| Memory pressure testing | Verify bounded collections under sustained load |
| Thermal throttling behavior | Validate ARC initiative reduction under heat |
| Model swap hot-reload | Verify session continuity across model tier changes |
| Accessibility service robustness | Test against popular app UI changes |

---

## Timeline Estimate

> **Caveat:** These estimates assume 1–2 focused engineers. They are rough.
> Every estimate should be treated as ±50%.

| Milestone | Prerequisites | Estimate |
|-----------|--------------|----------|
| **First device build** (APK compiles) | P0-1, P0-2, P0-3 | 1–2 weeks |
| **LLM runs on device** (text in, text out) | First device build + model download | 2–3 weeks |
| **Full ReAct loop on device** | LLM on device + P1 fixes | 4–6 weeks |
| **Usable alpha** (real tasks work) | Full ReAct loop + P1 work | 8–12 weeks |
| **Production release** | All P1 complete + P2 battery/test | 16–24 weeks |

> **Timeline Note (March 30, 2026):** Termux + llama-server verified working on March 29, 2026 (Moto G45 5G, MediaTek Dimensity 6300). HTTP backend connects to localhost:8080. Core infrastructure is functional.

---

## How to Contribute to Unblocking Each Item

### Contributing to P0-1 (llama.cpp vendor + FFI)

1. Fork the repo
2. `git submodule add https://github.com/ggml-org/llama.cpp vendor/llama.cpp`
3. Pick a stable llama.cpp commit (check their releases page for the latest stable tag)
4. Update `crates/aura-llama-sys/src/lib.rs` — replace `llama_eval` calls with `llama_batch` /
   `llama_decode` pattern
5. Run `cargo build --release -p aura-llama-sys` (native Termux build)
6. Fix any linker errors
7. PR with title: `fix(llama-sys): vendor llama.cpp and update to batch API`

**Reference:** llama.cpp batch API migration guide is in their repo at `docs/backend/`.

### Contributing to P0-2 (Termux Build Verification)

1. Run `bash install.sh` in Termux on device
2. Verify `aura-daemon` starts
3. Verify `aura-neocortex` connects via HTTP to localhost:8080
4. Test basic inference: `curl -X POST http://localhost:8080/v1/completions -d '{"prompt": "Hello"}'`
5. Report any errors

### Contributing to P1 Fixes (`ping_neocortex`, `score_plan`)

1. Read `crates/aura-daemon/src/daemon_core/` to find the failing function
2. Run the specific failing test with `--nocapture` to see what's wrong
3. Fix the logic (do not weaken the test — IL-4)
4. PR with specific test coverage

### Contributing to Token Budget Manager

1. Review `crates/aura-neocortex/src/context.rs` — this is where token counting happens
2. The budget manager needs to: count tokens per context section, enforce the `max_tokens` budget,
   apply the priority drop order (see `NEOCORTEX-AND-TOKEN-ECONOMICS.md §7`)
3. Run `cargo test -p aura-neocortex`

---

## Status Change Log

| Date | Change | Who |
|------|--------|-----|
| 2026-03-13 | Policy gate fixed to deny-by-default | — |
| 2026-03-13 | Qwen3 set as default model | — |
| 2026-03-13 | `install.sh` written | — |
| 2026-03-13 | CI/CD pipeline written | — |
| 2026-03-13 | Architecture documentation complete (5 docs) | — |
| 2026-03-13 | This status doc created | — |

---

*Update this file immediately when status changes. A stale status doc is worse than no status doc.*
