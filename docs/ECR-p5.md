# PART VI — ACTION PLAN

---

## §26 Prioritized Action Plan

### 26.1 TIER 0 — Ship Blockers (Fix Before Any Release, Target: Week 1–2)

These 15 findings make the system either exploitable, non-functional, or unsafe on real hardware. Nothing ships until all are resolved.

| Priority | ID | Fix | Owner | Effort |
|----------|----|-----|-------|--------|
| P0-1 | AND-CRIT-002 | Add `ACCESS_NETWORK_STATE` + `ACCESS_WIFI_STATE` to AndroidManifest | Android | 5 min |
| P0-2 | AND-CRIT-001 | Add `foregroundServiceType` to manifest + service declaration | Android | 15 min |
| P0-3 | CI-CRIT-001 | Fix `ci.yml` toolchain: `stable` → `nightly-2026-03-01` | DevOps | 15 min |
| P0-4 | CI-CRIT-002 | Add `--features stub` + `submodules: recursive` to `release.yml` | DevOps | 15 min |
| P0-5 | SEC-CRIT-001 | Replace `==` with `subtle::ConstantTimeEq` in `vault.rs:811` | Security | 30 min |
| P0-6 | SEC-CRIT-002 | Add `ZeroizeOnDrop` to `VaultKey` struct in `vault.rs:~690` | Security | 30 min |
| P0-7 | SEC-CRIT-004 | Replace unsalted `sha256sum` PIN with salted hash in `install.sh:884` | Security | 1 hr |
| P0-8 | AND-CRIT-006 | Wrap all `AccessibilityNodeInfo` in try/finally + `recycle()` | Android | 1 hr |
| P0-9 | AND-CRIT-003 | Add `sensorManager.unregisterListener(this)` in `onDestroy()` | Android | 30 min |
| P0-10 | AND-CRIT-004 | Replace timed WakeLock with indefinite + explicit release in `onDestroy()` | Android | 1 hr |
| P0-11 | AND-CRIT-005 | Replace `@Volatile` check-then-act with `AtomicBoolean.compareAndSet()` | Android | 30 min |
| P0-12 | LLM-CRIT-001 | Fix FFI binding: `*const LlamaToken` not `*mut` in `lib.rs:1397` | LLM | 1 hr |
| P0-13 | AND-CRIT-007 | Add JNI exception check after every Kotlin callback in `jni_bridge.rs` | Rust/Android | 3 hr |
| P0-14 | SEC-CRIT-003 | Generate real SHA256 checksums; add CI gate to block `PLACEHOLDER` in `install.sh` | DevOps/Security | 2 hr |
| P0-15 | PERF-DEAD-001 | Fix `handle.block_on()` in async context in `monitor.rs` | Rust | 1 hr |

**Total TIER 0 effort: ~13 hours of actual coding. These are not hard changes — they are small, precise, high-leverage fixes.**

### 26.2 TIER 1 — High Severity (Fix Within 2–4 Weeks)

| ID | Fix | Owner | Effort |
|----|-----|-------|--------|
| PERF-CRIT-C1 | Persistent `NeocortexClient` — reuse TCP connection across ReAct iterations | Rust | 4 hr |
| PERF-CRIT-C3 | Replace `Vec<Message>` with `VecDeque` in `context.rs` | Rust | 2 hr |
| PERF-CRIT-C4 | Running token estimate instead of full rebuild per truncation | Rust | 3 hr |
| LLM-HIGH-002 | Add `llama_free` + `llama_free_model` calls in `LoadedModel::drop()` | LLM | 1 hr |
| LLM-HIGH-003 | Replace nanos seed with `rand::thread_rng().gen()` | LLM | 30 min |
| SEC-HIGH-001 | Remove or quarantine Telegram HTTP backend; audit for other external calls | Security | 4 hr |
| SEC-HIGH-002 | Add `[UNTRUSTED SCREEN CONTENT]` label in `prompts.rs:549` | LLM | 30 min |
| AND-HIGH-001 | Align battery thresholds: one canonical value across `heartbeat.rs` and `monitor.rs` | Rust/Android | 1 hr |
| AND-HIGH-002 | Add `armeabi-v7a` and `x86_64` Cargo cross-compile configs | DevOps | 2 hr |
| PLUG-CRIT-001 | Remove `simulate_action_result()` or implement real action execution | Rust | 1 week |
| TEST-CRIT-001 | Audit and replace all 45 tautological assertions with real tests | Testing | 1 week |
| TEST-CRIT-002 | Write minimum 20 test functions for `react.rs` | Testing | 1 week |
| LLM-MED-001 | Change `DEFAULT_CONTEXT_BUDGET` from 2048 to 24576 | Config | 5 min |
| PERF-HIGH-006 | Deduplicate lock ordering; document canonical order (SQLite → HNSW) | Rust | 2 hr |

### 26.3 TIER 2 — Medium Priority (Fix Within 4–8 Weeks)

| ID | Fix | Effort |
|----|-----|--------|
| PERF-HIGH-001 | Replace `vec![false; n]` visited array in HNSW with `HashSet` | 2 hr |
| PERF-HIGH-002 | Replace global `Mutex` with `RwLock` or `DashMap` for embedding cache | 4 hr |
| PERF-HIGH-003 | Return `Arc<Vec<f32>>` from cache; O(1) LRU via `lru` crate | 3 hr |
| PERF-HIGH-004 | Add NEON SIMD for k-means dot products in `consolidation.rs` | 1 week |
| PERF-HIGH-005 | Parallelize 100 embed calls with `buffer_unordered` | 3 hr |
| LLM-CRIT-002 | Implement decode-time GBNF using `llama_grammar_init` | 2 weeks |
| LLM-HIGH-001 | Either fix `RouteClassifier` or delete it — no zombie code | 1 day |
| LLM-HIGH-004 | Sync token budgets: neocortex returns actual token count per response | 4 hr |
| AND-HIGH-003 | Implement real `system_api.rs` methods (replace 12 stubs) | 2 weeks |
| DOC-CRIT-001 | Canonicalize trust tier model — one document, one enum, remove others | 1 day |
| DOC-CRIT-002 | Canonicalize ethics rules — code is source of truth (11), update both docs | 1 hr |
| RUST-HIGH-003 | Add `// SAFETY:` comments to all 18 unjustified unsafe blocks | 4 hr |
| RUST-MED-001 | Replace all `unwrap()` in non-test code with proper `?` or handled errors | 1 day |
| CI-HIGH-001 | Fix Android CI: pin NDK, set `ANDROID_NDK_HOME`, add Rust Android targets | DevOps | 1 day |
| CI-HIGH-002 | Add `cargo audit`, secret scanning to all CI workflows | DevOps | 2 hr |
| OPS-HIGH-001 | Add timeout on LLM calls (default: 60s, configurable) | 2 hr |
| OPS-HIGH-002 | Add async watchdog task to detect and restart deadlocked subsystems | 4 hr |

---

## §27 Work Allocation by Team

### 27.1 Security Team (Week 1 Priority)
**All TIER 0 security fixes first:**
- SEC-CRIT-001: `vault.rs:811` — `subtle::ConstantTimeEq` (30 min)
- SEC-CRIT-002: `vault.rs:~690` — `ZeroizeOnDrop` (30 min)
- SEC-CRIT-003: `install.sh` — real checksums + CI gate (2 hr)
- SEC-CRIT-004: `install.sh:884` — salted PIN hash (1 hr)
- SEC-HIGH-001: Telegram backend audit + removal (4 hr)
- SEC-HIGH-002: `prompts.rs:549` — untrusted label (30 min)

**Then TIER 2:**
- Add vault access audit log
- Design key rotation mechanism

### 27.2 Android Team (Week 1 Priority)
**All TIER 0 Android fixes:**
- AND-CRIT-002: Manifest permissions (5 min)
- AND-CRIT-001: foregroundServiceType (15 min)
- AND-CRIT-006: AccessibilityNodeInfo recycle (1 hr)
- AND-CRIT-003: Sensor listener unregister (30 min)
- AND-CRIT-004: WakeLock indefinite (1 hr)
- AND-CRIT-005: AtomicBoolean race fix (30 min)

**Then TIER 1:**
- Battery threshold alignment
- ABI configuration fix

**Then TIER 2:**
- system_api.rs real implementations

### 27.3 Rust Core Team (Week 1–2)
**TIER 0:**
- LLM-CRIT-001: FFI const→mut fix (1 hr)
- AND-CRIT-007: JNI exception checks (3 hr)
- PERF-DEAD-001: block_on in async (1 hr)

**TIER 1:**
- PERF-CRIT-C1: Persistent NeocortexClient
- PERF-CRIT-C3/C4: VecDeque context truncation
- LLM-HIGH-002: LoadedModel Drop fix
- LLM-HIGH-003: RNG seed fix
- Lock ordering documentation + enforcement

### 27.4 DevOps Team (Week 1)
**TIER 0 — CI must work before anything else ships:**
- CI-CRIT-001: Toolchain fix in ci.yml (15 min)
- CI-CRIT-002: release.yml flags fix (15 min)
- SEC-CRIT-003: CI gate for placeholder checksums

**TIER 1:**
- Android CI pipeline complete fix
- cargo audit integration
- Secret scanning

### 27.5 Testing Team (Week 2–4)
- Audit and replace all 45 tautological tests
- Write 20+ meaningful tests for `react.rs`
- Write security regression tests for SEC-CRIT-001/002
- Write Android integration tests
- Establish coverage requirements: minimum 60% meaningful coverage per module

---

## §28 Cross-Team Protocol

### 28.1 Change Gate Rules

Before any finding fix is marked complete, the following gates must pass:

| Fix Type | Required Verification |
|----------|-----------------------|
| Security primitive change | Second developer reviews + test with timing oracle |
| Android Manifest change | Verified on physical Android 14 device |
| CI change | Watch pipeline run to green — no "should work" |
| Stub replacement | Integration test proves real behavior, not simulated |
| Documentation update | Code and doc reviewed in same PR |
| Unsafe block | `// SAFETY:` comment present + reviewer confirms invariant |

### 28.2 No Self-Certification

No developer may mark their own TIER 0 fix as complete. Every TIER 0 fix requires:
1. The fix implemented
2. A second reviewer confirms the fix at the code level
3. A runtime verification (device test, pipeline run, timing test)
4. Findings register updated with: fix commit SHA, reviewer name, verification evidence

### 28.3 Stub Registry

All stubs in the codebase must be tracked in a mandatory registry. No file with a registered stub may be included in a release build unless the stub is replaced or the registry entry is explicitly marked `APPROVED-EXCEPTION` with documented rationale.

Current registry entries (TIER 0 — must be resolved before release):
1. `simulate_action_result()` — action execution stub
2. `install.sh:39,44,49` — checksum placeholders
3. `system_api.rs` — 12 stub platform methods

---

## §29 Cascade Dependency Registry

Some fixes are prerequisites for other fixes. This registry prevents doing work out of order.

```
CI-CRIT-001 (toolchain fix)
    └── MUST be done before ANY automated verification
        └── Enables: cargo audit, test runs, Android builds

CI-CRIT-002 (release flags)
    └── MUST be done before any release artifact is trusted
        └── Enables: real release pipeline

SEC-CRIT-001 + SEC-CRIT-002 (vault security)
    └── Both must be done together — fixing timing without zeroing is incomplete
        └── Enables: vault can be considered secure in isolation

AND-CRIT-001 + AND-CRIT-002 (Android manifest)
    └── Must be done before testing any other Android fix
        └── Without these, the service cannot start on Android 14 — no other Android test works

LLM-CRIT-001 (FFI fix)
    └── Must be done before any ASAN/Miri testing
        └── Current UB invalidates memory safety analysis

AND-HIGH-003 (system_api stubs)
    └── Depends on: correct understanding of what each API returns (requires Android platform knowledge)
    └── Blocks: PolicyGate meaningful operation

PLUG-CRIT-001 (simulate_action_result)
    └── Depends on: plugin architecture design decision
    └── Blocks: any meaningful end-to-end task execution test
```

---

## §30 MVS Gate — Minimum Viable Shippable

AURA v4 may be released when ALL of the following conditions are met:

### 30.1 Security Gate (Non-Negotiable)
- [ ] `vault.rs:811` — constant-time HMAC comparison verified by timing test
- [ ] `vault.rs:~690` — key material verified zeroed on drop (memory dump test)
- [ ] `install.sh` — all three checksum lines contain real SHA256 values
- [ ] `install.sh:884` — PIN uses salted hash (argon2 or equivalent)
- [ ] No HTTP calls to external domains in production binary (grep verified)
- [ ] Screen content injected with `[UNTRUSTED]` label in all prompt paths

### 30.2 Android Gate (Non-Negotiable)
- [ ] Service starts without crash on Android 14 physical device
- [ ] Service starts without crash on Android 13 physical device
- [ ] No `SecurityException` on first run (permission verification)
- [ ] Service survives 30 minutes without daemon freeze (WakeLock test)
- [ ] No `AccessibilityNodeInfo` pool exhaustion after 1000 events
- [ ] No sensor memory leak detected over 24-hour run

### 30.3 CI Gate (Non-Negotiable)
- [ ] `ci.yml` runs to green on nightly-2026-03-01 toolchain
- [ ] `release.yml` produces a valid binary artifact
- [ ] Android CI produces a valid APK
- [ ] `cargo audit` shows no critical CVEs

### 30.4 Functionality Gate (Non-Negotiable)
- [ ] No `PLACEHOLDER` strings in any release artifact (grep gate)
- [ ] `simulate_action_result()` either replaced or removed from production binary
- [ ] `LoadedModel::drop()` verified to call `llama_free` and `llama_free_model`

### 30.5 Test Gate
- [ ] Zero tautological tests (no `assert!(true)`, no always-true disjunctions)
- [ ] `react.rs` has minimum 10 meaningful test functions
- [ ] All 18 critical findings have regression test or verified non-testable exception

---

# APPENDICES

---

## Appendix A: Full Findings Register

**Severity legend:** CRIT = Critical | HIGH = High | MED = Medium | LOW = Low

| ID | Sev | Domain | Location | Issue |
|----|-----|--------|----------|-------|
| SEC-CRIT-001 | CRIT | Security | `vault.rs:811-812` | Non-constant-time HMAC comparison |
| SEC-CRIT-002 | CRIT | Security | `vault.rs:~690` | AES-256 key not zeroed on drop |
| SEC-CRIT-003 | CRIT | Security/CI | `install.sh:39,44,49` | Placeholder SHA256 checksums |
| SEC-CRIT-004 | CRIT | Security | `install.sh:884` | Unsalted PIN hash |
| LLM-CRIT-001 | CRIT | LLM/FFI | `lib.rs:1397` | Const→mut pointer cast — UB |
| LLM-CRIT-002 | CRIT | LLM | `inference.rs:368-385` | GBNF post-generation only |
| AND-CRIT-001 | CRIT | Android | `AuraForegroundService.kt` | Missing foregroundServiceType |
| AND-CRIT-002 | CRIT | Android | `AndroidManifest.xml` | Undeclared permissions |
| AND-CRIT-003 | CRIT | Android | `AuraDaemonBridge.kt` | Sensor listeners never unregistered |
| AND-CRIT-004 | CRIT | Android | `AuraForegroundService.kt` | WakeLock expires after 10 min |
| AND-CRIT-005 | CRIT | Android | `AuraDaemonBridge.kt` | WakeLock race condition |
| AND-CRIT-006 | CRIT | Android | `AuraAccessibilityService.kt` | AccessibilityNodeInfo not recycled |
| AND-CRIT-007 | CRIT | Android/Rust | `jni_bridge.rs` | No JNI exception checks |
| CI-CRIT-001 | CRIT | CI/CD | `ci.yml` | Wrong Rust toolchain |
| CI-CRIT-002 | CRIT | CI/CD | `release.yml` | Missing flags |
| TEST-CRIT-001 | CRIT | Tests | `integration_tests.rs` | 45 tautological assertions |
| TEST-CRIT-002 | CRIT | Tests | `react.rs` | Zero test functions (2821 lines) |
| DOC-CRIT-001 | CRIT | Docs | Multiple | 5-way trust tier inconsistency |
| DOC-CRIT-002 | CRIT | Docs | Multiple | Ethics rules 3 different counts |
| PLUG-CRIT-001 | CRIT | Plugin | `action.rs` | `simulate_action_result()` in production |
| PERF-CRIT-C1 | CRIT | Perf | `react.rs` | New TCP socket per LLM call |
| PERF-CRIT-C2 | CRIT | Perf | `react.rs` | DGS fast path permanently disabled |
| PERF-CRIT-C3 | CRIT | Perf | `context.rs:385,398` | O(n²) context truncation |
| PERF-CRIT-C4 | CRIT | Perf | `context.rs:398` | Full prompt rebuild per truncation |
| PERF-HIGH-001 | HIGH | Perf | `hnsw.rs:600` | O(n) visited array per search_layer |
| PERF-HIGH-002 | HIGH | Perf | `embeddings.rs` | Global Mutex serializes embeddings |
| PERF-HIGH-003 | HIGH | Perf | `embeddings.rs` | O(n) LRU + 6KB clone per hit |
| PERF-HIGH-004 | HIGH | Perf | `consolidation.rs:569` | Scalar dot products, no SIMD |
| PERF-HIGH-005 | HIGH | Perf | `consolidation.rs:543` | 100 sequential embed calls |
| PERF-HIGH-006 | HIGH | Perf | `monitor.rs` | block_on in async — deadlock |
| PERF-HIGH-007 | HIGH | Perf | `episodic.rs` | Lock ordering violation |
| LLM-HIGH-001 | HIGH | LLM | `react.rs` | RouteClassifier dead code |
| LLM-HIGH-002 | HIGH | LLM | `model.rs:634-645` | LoadedModel Drop leaks memory |
| LLM-HIGH-003 | HIGH | LLM | `lib.rs:1344-1351` | Weak RNG seeding |
| LLM-HIGH-004 | HIGH | LLM | Multiple | MAX_REACT_ITERATIONS asymmetry |
| LLM-HIGH-005 | HIGH | LLM | Multiple | Token budget drift daemon/neocortex |
| SEC-HIGH-001 | HIGH | Security | `telegram/reqwest_backend.rs` | HTTP to api.telegram.org — Iron Law |
| SEC-HIGH-002 | HIGH | Security | `prompts.rs:549` | Prompt injection vector |
| SEC-HIGH-003 | HIGH | Security | `lib.rs:1344` | Weak RNG seeding |
| AND-HIGH-001 | HIGH | Android | `heartbeat.rs` vs `monitor.rs` | Battery threshold mismatch |
| AND-HIGH-002 | HIGH | Android | `build.gradle.kts` | ABI mismatch |
| AND-HIGH-003 | HIGH | Android | `system_api.rs` | 12 stub platform methods |
| AND-HIGH-004 | HIGH | Android | `AuraDaemonBridge.kt` | No daemon reconnection logic |
| AND-HIGH-005 | HIGH | Android | Multiple | No graceful degradation |
| CI-HIGH-001 | HIGH | CI/CD | Android workflow | Never produced working APK |
| CI-HIGH-002 | HIGH | CI/CD | All workflows | No cargo audit / secret scan |
| CI-HIGH-003 | HIGH | CI/CD | All workflows | Integration tests not run in CI |
| TEST-HIGH-001 | HIGH | Tests | `consolidation.rs` | 0% meaningful coverage |
| TEST-HIGH-002 | HIGH | Tests | `vault.rs` | SEC-CRIT-001 missed by test |
| RUST-HIGH-001 | HIGH | Rust | `jni_bridge.rs` | No JNI exception checks |
| RUST-HIGH-002 | HIGH | Rust | `context.rs:385,398` | O(n²) Vec truncation |
| RUST-HIGH-003 | HIGH | Rust | 18 unsafe blocks | No SAFETY comments |
| RUST-HIGH-004 | HIGH | Rust | `react.rs` | Mutex guard held across await |
| ARCH-HIGH-001 | HIGH | Arch | `telegram/reqwest_backend.rs` | Iron Law: anti-cloud violated |
| ARCH-HIGH-002 | HIGH | Arch | `action.rs` | Iron Law: stub in production |
| ARCH-HIGH-003 | HIGH | Arch | `react.rs` | RouteClassifier dead — DGS disabled |
| ARCH-HIGH-004 | HIGH | Arch | `config.rs` | Context budget 6.25% of window |
| ARCH-HIGH-005 | HIGH | Arch | Multiple | Token tracking drift |
| OPS-HIGH-001 | HIGH | Ops | LLM client | No timeout on LLM calls |
| OPS-HIGH-002 | HIGH | Ops | Runtime | No watchdog for deadlock |
| OPS-HIGH-003 | HIGH | Ops | `hnsw.rs` | HNSW not persisted on shutdown |
| PLUG-HIGH-001 | HIGH | Plugin | Multiple | Trust tier inconsistency |
| PLUG-HIGH-002 | HIGH | Plugin | Runtime | No capability enforcement |
| SCALE-HIGH-001 | HIGH | Scale | `config.rs` | Context budget 16× below capability |
| SCALE-HIGH-002 | HIGH | Scale | `embeddings.rs` | Embedding serialized by Mutex |
| LLM-MED-001 | MED | LLM | `config.rs` | DEFAULT_CONTEXT_BUDGET=2048 |
| LLM-MED-002 | MED | LLM | `react.rs` | No backoff/retry on LLM failure |
| RUST-MED-001 | MED | Rust | Multiple | `unwrap()` in non-test code |
| RUST-MED-002 | MED | Rust | `embeddings.rs` | Global Mutex serializes work |
| RUST-MED-003 | MED | Rust | `consolidation.rs` | Sequential embed calls |
| RUST-MED-004 | MED | Rust | Multiple | Missing `#[must_use]` on Results |
| ARCH-MED-001 | MED | Arch | Multiple | Lock ordering not documented |
| ARCH-MED-002 | MED | Arch | `config.rs` | REACT_ITERATIONS asymmetry |
| ARCH-MED-003 | MED | Arch | Multiple | Battery threshold mismatch |
| CI-MED-001 | MED | CI/CD | `release.yml` | No code signing |
| CI-MED-002 | MED | CI/CD | All workflows | No Cargo cache |
| CI-MED-003 | MED | CI/CD | All workflows | No API level matrix |
| TEST-MED-001 | MED | Tests | Multiple | No property-based crypto tests |
| TEST-MED-002 | MED | Tests | Multiple | No concurrency tests |
| DOC-MED-001 | MED | Docs | Multiple | DGS documented but not wired |
| DOC-MED-002 | MED | Docs | Multiple | Iron Laws doc missing Telegram |
| DOC-MED-003 | MED | Docs | Multiple | Unsafe count wrong (23 vs 70) |
| OPS-MED-001 | MED | Ops | LLM client | No circuit breaker |
| OPS-MED-002 | MED | Ops | `consolidation.rs` | Consolidation can starve foreground |
| OPS-MED-003 | MED | Ops | Startup | No structured health check |
| AND-MED-001 | MED | Android | `AuraForegroundService.kt` | Notification channel for API 26+ |
| AND-MED-002 | MED | Android | `build.gradle.kts` | minSdk not specified |
| PLUG-MED-001 | MED | Plugin | Loader | No signature verification |
| PLUG-MED-002 | MED | Plugin | Runtime | No resource quota per plugin |
| SCALE-MED-001 | MED | Scale | Multiple | No request queuing |
| SCALE-MED-002 | MED | Scale | `hnsw.rs` | Index size unbounded |
| RUST-LOW-001 | LOW | Rust | Multiple | Clippy suppressed with `#[allow]` |
| SEC-LOW-001 | LOW | Security | Multiple | Debug logging may leak values |
| SEC-MED-001 | MED | Security | `vault.rs` | No vault access audit log |
| SEC-MED-002 | MED | Security | `vault.rs` | No key rotation mechanism |
| CI-LOW-001 | LOW | CI/CD | README | No build status badge |
| TEST-LOW-001 | LOW | Tests | Multiple | Test names don't describe failure |
| DOC-LOW-001 | LOW | Docs | Multiple | Crate names wrong in docs |
| ARCH-LOW-001 | LOW | Arch | Multiple | aura-daemon/aura-types boundary blurry |
| AND-MED-003 | MED | Android | Multiple | No Doze mode in Kotlin layer |
| PERF-MED-001 | MED | Perf | Multiple | No memory pressure backpressure |
| PERF-MED-002 | MED | Perf | `hnsw.rs` | HNSW fully in RAM, no paging |

**Total: 18 Critical | 45 High | 46 Medium | 17 Low = 126 Findings**

---

## Appendix B: Crate Map & Unsafe Inventory

### B.1 Crate Structure

```
aura-v4/
├── aura-daemon/          # Core orchestration, tools, memory, ReAct
├── aura-neocortex/       # LLM inference, llama.cpp FFI, teacher pipeline
├── aura-types/           # Shared types, traits, protocol definitions
└── android/              # Kotlin layer + JNI bridge (Rust side in aura-daemon)
```

### B.2 Correct Crate Names

| Wrong (in old docs) | Correct |
|---------------------|---------|
| `aura-core` | `aura-neocortex` |
| `aura-android` | `aura-types` |

### B.3 Unsafe Inventory

| Crate | Unsafe blocks | Justified (with SAFETY) | Unjustified |
|-------|--------------|------------------------|-------------|
| `aura-neocortex` | 41 | 28 | **13** |
| `aura-daemon` | 24 | 19 | **5** |
| `aura-types` | 5 | 5 | 0 |
| **Total** | **70** | **52** | **18** |

---

## Appendix C: Security Threat Model

| Threat | Vector | Current Mitigation | Gap |
|--------|--------|--------------------|-----|
| Timing attack on vault | Local process timing | None | SEC-CRIT-001 |
| Key extraction from memory | Memory dump/cold boot | None | SEC-CRIT-002 |
| Supply chain attack | Tampered binary install | None (placeholder checksums) | SEC-CRIT-003 |
| PIN brute force | Rainbow table | None (unsalted SHA256) | SEC-CRIT-004 |
| Prompt injection | Screen content | None (no trust labeling) | SEC-HIGH-002 |
| Data exfiltration | Telegram backend | None | SEC-HIGH-001 |

---

## Appendix D: Android Compatibility Matrix

| Android Version | API Level | Status | Blocker |
|----------------|-----------|--------|---------|
| Android 14 | 34 | 🔴 CRASHES | AND-CRIT-001 |
| Android 13 | 33 | 🔴 CRASHES | AND-CRIT-002 |
| Android 12 | 32 | 🔴 CRASHES | AND-CRIT-002 |
| Android 11 | 30 | 🔴 CRASHES | AND-CRIT-002 |
| Any version | Any | 🔴 DEGRADES | AND-CRIT-003/004/005/006/007 |

No Android version works correctly in the current state.

---

## Appendix E: LLM Call Budget Analysis

### Worst-Case Request Scenario

| Parameter | Value |
|-----------|-------|
| Daemon MAX_REACT_ITERATIONS | 10 |
| Neocortex MAX_REACT_ITERATIONS | 5 |
| Best-of-N candidates | 3 |
| **Worst-case LLM calls per request** | **10 × 5 × 3 = 150 calls** |

### Context Window Utilization

| Parameter | Current | Optimal |
|-----------|---------|---------|
| DEFAULT_CONTEXT_BUDGET | 2,048 tokens | 24,576 tokens |
| Model window | 32,768 tokens | 32,768 tokens |
| Utilization | **6.25%** | 75% |

---

## Appendix F: Test Coverage Heatmap

| Module | Lines | Real Tests | Tautological | Coverage |
|--------|-------|-----------|--------------|----------|
| `react.rs` | 2,821 | 0 | 0 | **0%** |
| `consolidation.rs` | 680 | 0 | 2 | **0%** |
| `inference.rs` | 420 | 1 | 2 | **~8%** |
| `vault.rs` | 1,240 | 3 | 5 | **~12%** |
| `semantic.rs` | 760 | 2 | 1 | **~14%** |
| `episodic.rs` | 890 | 4 | 0 | **~18%** |
| `context.rs` | 310 | 3 | 0 | **~35%** |
| `hnsw.rs` | 540 | 5 | 0 | **~45%** |
| **Overall** | **~44,000** | **~18 real** | **45** | **~15%** |

---

## Appendix G: Glossary

| Term | Definition |
|------|------------|
| **ReAct** | Reasoning + Acting loop — iterative LLM reasoning with tool execution |
| **DGS** | Direct Goal Satisfaction — fast path for simple tasks, bypassing ReAct |
| **GBNF** | Grammar-based format enforcement for LLM outputs |
| **HNSW** | Hierarchical Navigable Small World — approximate nearest-neighbor graph for semantic search |
| **BoN** | Best-of-N — generate N candidates, select best by scoring |
| **Teacher Pipeline** | 5-layer self-improvement system (CoT, Logprob, Cascade, Reflection, BoN) |
| **Iron Laws** | Inviolable architectural principles (anti-cloud, LLM=brain, no stubs in prod) |
| **Theater AGI** | Fake AI behavior implemented via keyword matching instead of real reasoning — banned |
| **WakeLock** | Android mechanism preventing CPU sleep during background work |
| **JNI** | Java Native Interface — bridge between Kotlin/Java and Rust native code |
| **FFI** | Foreign Function Interface — Rust ↔ C (llama.cpp) interop layer |
| **MVS** | Minimum Viable Shippable — the gate conditions for any release |
| **PolicyGate** | Rust component that decides whether to execute tasks based on system state |

---

*End of AURA v4 Enterprise Code Review*  
*Document ID: AURA-V4-ECR-FINAL | Version 1.0 | 2026-03-14*  
*Supersedes: ENTERPRISE-CODE-REVIEW-PART-A.md, PART-B.md, PART-C.md, AUDIT-REPORT.md*
