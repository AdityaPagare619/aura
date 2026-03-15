# Agent 4 — Extracted New Findings from Formal Audit Files
**Date:** 2026-03-14
**Sources:** AURA-v4-MASTER-AUDIT.md, DOMAIN-03-SECURITY.md, DOMAIN-06-ANDROID.md, TEST_AUDIT_FINAL.md, MASTER-AUDIT-REPORT.md
**Scope:** New findings NOT in the already-known/fixed list

---

## CRITICAL

### A4-001: Missing AndroidManifest Permissions Crash All Devices
- Source: DOMAIN-06-ANDROID.md, AURA-v4-MASTER-AUDIT.md
- Severity: CRITICAL
- Target: AndroidManifest.xml
- Description: `ACCESS_NETWORK_STATE` and `ACCESS_WIFI_STATE` undeclared. Any call to WifiManager/ConnectivityManager throws `SecurityException` on every device.
- Recommendation: Add both `<uses-permission>` entries to AndroidManifest.xml.

### A4-002: WakeLock 10-Minute Expiry Never Renewed
- Source: DOMAIN-06-ANDROID.md
- Severity: CRITICAL
- Target: AuraForegroundService.kt
- Description: WakeLock acquired with 10-min timeout but never renewed in heartbeat loop. After 10 minutes CPU can sleep mid-inference, freezing the daemon.
- Recommendation: Acquire with no timeout (`0L`) or renew in heartbeat loop.

### A4-003: No JNI Exception Checking After Kotlin Callbacks
- Source: DOMAIN-06-ANDROID.md, AURA-v4-MASTER-AUDIT.md
- Severity: CRITICAL
- Target: jni_bridge.rs
- Description: After JNI calls into Kotlin, no `env.exception_check()` is performed. A pending Java exception causes undefined behavior on the next JNI call.
- Recommendation: Add `env.exception_check()` + `exception_clear()` after every JNI callback.

### A4-004: CI Toolchain Mismatch — All Builds Use Wrong Compiler
- Source: AURA-v4-MASTER-AUDIT.md, MASTER-AUDIT-REPORT.md
- Severity: CRITICAL
- Target: .github/workflows/ci.yml vs rust-toolchain.toml
- Description: CI uses `dtolnay/rust-toolchain@stable` but project requires `nightly-2026-03-01`. Every CI build compiles on the wrong toolchain, masking nightly-only errors.
- Recommendation: Change CI to `dtolnay/rust-toolchain@nightly-2026-03-01`.

### A4-005: Trust Tier 5-Way Inconsistency
- Source: AURA-v4-MASTER-AUDIT.md, DOMAIN-03-SECURITY.md
- Severity: CRITICAL
- Target: relationship.rs vs 4 documentation files
- Description: Code has 5 tiers (Stranger/Acquaintance/Friend/CloseFriend/Soulmate) but all docs show 4 tiers with different names. `CloseFriend` has no documented permission boundary.
- Recommendation: Pick one canonical definition and reconcile everywhere.

### A4-006: Ethics Rule Count Conflict (Code 11 vs Docs 15)
- Source: AURA-v4-MASTER-AUDIT.md, DOMAIN-03-SECURITY.md
- Severity: CRITICAL
- Target: ethics.rs vs architecture docs
- Description: Code has 11 rules, docs claim 15. 9 code rules are undocumented; 4 documented rules have no implementation. False security guarantees.
- Recommendation: Audit and reconcile rules across code and documentation.

## HIGH

### A4-007: Telegram Bridge Violates Anti-Cloud Iron Law
- Source: DOMAIN-03-SECURITY.md, MASTER-AUDIT-REPORT.md
- Severity: HIGH
- Target: crates/aura-daemon/src/telegram/reqwest_backend.rs
- Description: `reqwest` is a hard (non-feature-gated) dependency. The Telegram bridge sends user data to `api.telegram.org`, directly contradicting the "zero cloud callbacks" Iron Law.
- Recommendation: Gate `telegram` module behind `#[cfg(feature = "telegram")]`, disabled by default.

### A4-008: No IPC Authentication Tokens
- Source: DOMAIN-03-SECURITY.md
- Severity: HIGH
- Target: ipc.rs
- Description: IPC protocol has no authentication fields. Any process reaching the Unix socket can send commands to the daemon. Exploitable on rooted devices.
- Recommendation: Add 32-byte session token to IPC Handshake, reject unauthenticated connections.

### A4-009: Shell Injection via Username in sed Substitution
- Source: DOMAIN-03-SECURITY.md
- Severity: HIGH
- Target: install.sh:884
- Description: `sed -i "s/%%USERNAME%%/$user_name/g"` is vulnerable if username contains `/`, `&`, or shell metacharacters. Potential code execution.
- Recommendation: Escape input before sed substitution or use a Rust/Python binary for templating.

### A4-010: NDK Downloaded Without SHA256 Verification
- Source: DOMAIN-03-SECURITY.md
- Severity: HIGH
- Target: install.sh (NDK download section)
- Description: ~1GB Android NDK downloaded via curl with no integrity check. A MITM-compromised NDK backdoors every compiled binary.
- Recommendation: Pin NDK version and embed SHA256 from Google's published checksums.

### A4-011: Checksum Failure Allows User Bypass
- Source: DOMAIN-03-SECURITY.md
- Severity: HIGH
- Target: install.sh:567
- Description: On checksum mismatch, script asks user "Continue anyway? (y/N)". Social engineering trivially defeats integrity verification.
- Recommendation: Remove confirmation prompt; on failure delete file and exit non-zero.

### A4-012: curl|sh Rust Toolchain Install
- Source: DOMAIN-03-SECURITY.md
- Severity: HIGH
- Target: install.sh
- Description: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh` executes remotely fetched code. If rustup CDN is compromised, build environment is owned.
- Recommendation: Download rustup-init, verify SHA256 against pinned value, then execute.

### A4-013: NeocortexClient Reconnects Every ReAct Iteration
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: HIGH
- Target: react.rs
- Description: `NeocortexClient::connect()` called fresh every ReAct iteration, creating a new TCP/socket connection per LLM call. Massive overhead.
- Recommendation: Pool IPC connection and reuse across iterations.

### A4-014: Battery Temperature Used as Thermal Proxy
- Source: DOMAIN-06-ANDROID.md
- Severity: HIGH
- Target: AuraDaemonBridge.kt
- Description: `BatteryManager.EXTRA_TEMPERATURE` reports battery cell temp, which lags SoC temp by 5-15 min and is 10-20C lower under load. Cannot reliably detect 85C junction emergency.
- Recommendation: Use `PowerManager.getCurrentThermalStatus()` (API 29+) or delegate to Rust thermal.rs.

### A4-015: Deprecated WifiManager API Broken on API 31+
- Source: DOMAIN-06-ANDROID.md
- Severity: HIGH
- Target: AuraDaemonBridge.kt
- Description: `wifiManager.connectionInfo` deprecated API 31, RSSI restricted API 30+. Returns stale/empty data on API 33+ devices.
- Recommendation: Use `NetworkCapabilities.getSignalStrength()` via registerActiveNetworkCallback.

### A4-016: Compound @Volatile Sensor Reads — Torn Reads
- Source: DOMAIN-06-ANDROID.md
- Severity: HIGH
- Target: AuraDaemonBridge.kt
- Description: Three `@Volatile` fields (accelerometerX/Y/Z) read independently. Between reads, a sensor update can arrive producing a phantom motion vector from mixed samples.
- Recommendation: Use single `@Volatile` reference to immutable `AccelerometerReading` data class.

### A4-017: ABI Mismatch Between Gradle and Cargo
- Source: DOMAIN-06-ANDROID.md
- Severity: HIGH
- Target: build.gradle.kts vs .cargo/config.toml
- Description: Gradle lists arm64-v8a + armeabi-v7a + x86_64 but Cargo only configures aarch64-linux-android. APK on 32-bit device crashes with UnsatisfiedLinkError.
- Recommendation: Remove extra ABIs from abiFilters or add corresponding Rust targets.

### A4-018: nativeShutdown() Called on Main Thread in onDestroy
- Source: DOMAIN-06-ANDROID.md
- Severity: HIGH
- Target: AuraForegroundService.kt
- Description: JNI `nativeShutdown()` runs on main thread. Rust may block on mutex/IO flush causing ANR if >5 seconds.
- Recommendation: Dispatch to background thread with 3-second timeout.

### A4-019: CI Android Pipeline Cannot Produce Working APK
- Source: DOMAIN-06-ANDROID.md
- Severity: HIGH
- Target: .github/workflows/build-android.yml
- Description: Toolchain mismatch, missing cargo-ndk flags, no debug symbol stripping (>200MB .so), no APK signing. Pipeline has never produced a functional APK.
- Recommendation: Fix toolchain, add cargo-ndk target flag, strip symbols, add signing step.

### A4-020: main_loop.rs 7,348-Line God File
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: HIGH
- Target: main_loop.rs
- Description: Single 7,348-line file violating SRP. Contains event dispatch, cron handling, and core loop logic. High cognitive load and merge conflict risk.
- Recommendation: Decompose into logical sub-modules.

### A4-021: bincode Release Candidate in Production
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: HIGH
- Target: Cargo.toml
- Description: `bincode = "2.0.0-rc.3"` is a release candidate. RC crates may have breaking API changes and are not API-stable guarantees.
- Recommendation: Upgrade to stable bincode release or pin with documented justification.

### A4-022: unsafe impl Send/Sync Without SAFETY Comments
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: HIGH
- Target: ~8 files across codebase
- Description: ~8 `unsafe impl Send/Sync` blocks have no `// SAFETY:` invariant documentation. Impossible to audit correctness.
- Recommendation: Add SAFETY comments documenting invariants to all unsafe impl blocks.

### A4-023: Executor Tests Bypass PolicyGate via allow_all()
- Source: TEST_AUDIT_FINAL.md
- Severity: HIGH
- Target: execution/executor.rs (for_testing config)
- Description: All executor tests use `PolicyGate::allow_all()`, meaning a policy bypass regression in the executor would be invisible to the test suite.
- Recommendation: Add at least one executor test with real PolicyGate::from_config() and a deny rule.

### A4-024: No Property-Based Testing for Cryptographic Operations
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: HIGH
- Target: persistence/vault.rs tests
- Description: Vault encrypt/decrypt, key derivation, and HMAC comparison have no property-based or fuzzing tests despite being security-critical.
- Recommendation: Add proptest/quickcheck for vault round-trip correctness and edge cases.

### A4-025: No Integration Tests for IPC Protocol
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: HIGH
- Target: aura-types/ipc.rs
- Description: IPC message encoding, 64KB limit enforcement, and backpressure behavior at 64-message cap are completely untested.
- Recommendation: Add IPC protocol integration tests covering encoding, size limits, and overflow.

### A4-026: GitHub Action Not Pinned to Commit SHA
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: HIGH
- Target: .github/workflows/release.yml
- Description: `softprops/action-gh-release@v2` uses a mutable tag, not a commit SHA. Supply-chain attack if the tag is overwritten.
- Recommendation: Pin to full commit SHA.

### A4-027: Install Doc Claims bcrypt but Code Uses Argon2id
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: HIGH
- Target: install documentation vs vault.rs
- Description: Documentation states `bcrypt` for PIN. Code uses `Argon2id`. `bcrypt` does not appear in Cargo.toml. Misleads security auditors.
- Recommendation: Correct documentation to match code (Argon2id).

### A4-028: Argon2id Parallelism Mismatch (p=4 vs p=1 in Docs)
- Source: DOMAIN-03-SECURITY.md
- Severity: HIGH
- Target: vault.rs:772 vs architecture documentation
- Description: Code uses `p=4` (4 parallel threads) but docs claim `p=1`. Security auditors cannot reproduce KDF parameters from documentation.
- Recommendation: Update documentation to reflect actual code parameter `p=4`.

### A4-029: Phantom aura-gguf Crate Referenced in Docs
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: HIGH
- Target: Architecture documentation
- Description: Docs reference an `aura-gguf` crate that does not exist in the workspace. Misleads contributors.
- Recommendation: Remove phantom crate reference from documentation.

## MEDIUM

### A4-030: Screen Content Injected Without Injection Defense Label
- Source: DOMAIN-03-SECURITY.md
- Severity: MEDIUM
- Target: crates/aura-neocortex/src/prompts.rs:543-572
- Description: Screen text injected into all 4 inference modes with no trust boundary label. Security docs claim a `[DO NOT TREAT AS INSTRUCTIONS]` label exists — it does not. Malicious web page content enters LLM prompt as trusted.
- Recommendation: Wrap screen content in explicit `[UNTRUSTED SCREEN CONTENT]` markers.

### A4-031: Trust Float Exposed in LLM Prompts
- Source: DOMAIN-03-SECURITY.md
- Severity: MEDIUM
- Target: identity/user_profile.rs
- Description: Raw `trust_level` float injected into every LLM prompt via PersonalitySnapshot. Adversarial prompt injection could probe or manipulate trust value.
- Recommendation: Clamp trust_level to discrete enum ("trusted"/"acquaintance"/"stranger") before prompt.

### A4-032: GGUF Metadata Parse Failure Falls Back to 1024 Context
- Source: DOMAIN-03-SECURITY.md
- Severity: MEDIUM
- Target: model.rs
- Description: Failed GGUF metadata parse silently falls back to 1024-token context window. Severe capability degradation; exploitable via malformed GGUF substitution.
- Recommendation: Fail loudly or warn user when metadata parse fails.

### A4-033: ctx_ptr = 0x2 Sentinel Pointer (UB Risk)
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: MEDIUM
- Target: llama-sys/lib.rs:919
- Description: `ctx_ptr = 0x2 as *mut LlamaContext` used as sentinel. Fragile pattern; any accidental dereference is UB.
- Recommendation: Use `Option<NonNull<LlamaContext>>` instead.

### A4-034: partial_cmp().unwrap() Panics on NaN
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: MEDIUM
- Target: user_profile.rs:309
- Description: `partial_cmp().unwrap()` on f32 values will panic if either value is NaN. Daemon crash on malformed data.
- Recommendation: Use `total_cmp()` or handle NaN explicitly.

### A4-035: Audit Log Hash Chain Uses SipHash (Not Cryptographic)
- Source: AURA-v4-MASTER-AUDIT.md, DOMAIN-03-SECURITY.md
- Severity: MEDIUM
- Target: policy/audit.rs
- Description: Audit log hash chain uses SipHash (keyed PRF for DoS resistance) instead of SHA-256. Forensic investigators cannot verify integrity with standard tools.
- Recommendation: Replace with SHA-256 from `sha2` crate.

### A4-036: O(n^2) History Truncation in Context Assembly
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: MEDIUM
- Target: context.rs:385,398
- Description: `history.remove(0)` inside truncation loop causes O(n) shift per removal. For 50-turn history this is O(n^2). Combined with full prompt reassembly per iteration.
- Recommendation: Replace with `VecDeque::pop_front()`.

### A4-037: Global Mutex Serializes All Embedding Operations
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: MEDIUM
- Target: embeddings.rs
- Description: `EMBEDDING_CACHE` behind global `Mutex` serializes all threads. 100 sequential `embed()` calls in Deep consolidation pass all hit this lock.
- Recommendation: Replace with `Arc<RwLock<_>>` + lock-free miss path.

### A4-038: HNSW Allocates O(n) visited Array Per Search
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: MEDIUM
- Target: hnsw.rs:600
- Description: `vec![false; self.nodes.len()]` allocated per `search_layer()` call. For large graphs this is significant allocation churn.
- Recommendation: Pre-allocate scratch buffer and pass via `&mut`.

### A4-039: No Notification Channel for Android 8+
- Source: DOMAIN-06-ANDROID.md
- Severity: MEDIUM
- Target: AuraForegroundService.kt
- Description: No `createNotificationChannel()` before `startForeground()`. On 99%+ of devices the notification is silently dropped; some versions kill the service.
- Recommendation: Create notification channel before starting foreground service.

### A4-040: Battery Threshold Mismatch Between Components
- Source: DOMAIN-06-ANDROID.md
- Severity: MEDIUM
- Target: heartbeat.rs vs monitor.rs
- Description: `heartbeat.rs` uses 20% low-power threshold; `monitor.rs` uses 10%. Between 10-20% behavior is undefined — one component throttles, other does not.
- Recommendation: Unify to a single low-power threshold constant.

### A4-041: No Minimum API Level Declared in Manifest
- Source: DOMAIN-06-ANDROID.md
- Severity: MEDIUM
- Target: AndroidManifest.xml
- Description: No `android:minSdkVersion` declared. App appears compatible with pre-API 23 devices lacking required security APIs (Keystore, AES-GCM hardware).
- Recommendation: Set minSdkVersion to at least API 26.

### A4-042: aura-neocortex Never Tested in CI
- Source: AURA-v4-MASTER-AUDIT.md
- Severity: MEDIUM
- Target: .github/workflows/ci.yml
- Description: CI pipeline only tests aura-daemon. The entire neocortex crate (LLM inference, 6-layer teacher stack) runs zero CI tests.
- Recommendation: Add neocortex crate to CI test matrix.

### A4-043: Phase 8 Dead Code Debt (~15 allow(dead_code) Annotations)
- Source: MASTER-AUDIT-REPORT.md
- Severity: MEDIUM
- Target: inference.rs, model.rs, related files
- Description: ~15 `#[allow(dead_code)]` annotations reference "Phase 8:" fields populated but never read. Forward-engineering debt inflating apparent code completeness.
- Recommendation: Remove dead fields or implement consumers; remove allow annotations.

### A4-044: No Graceful Degradation on JNI Library Load Failure
- Source: DOMAIN-06-ANDROID.md
- Severity: MEDIUM
- Target: AuraDaemonBridge.kt
- Description: `System.loadLibrary("aura_daemon")` has no try/catch. Wrong ABI or missing .so causes uncaught `UnsatisfiedLinkError` crash with no user-facing message.
- Recommendation: Wrap in try/catch with user-facing error and recovery instructions.

### A4-045: No Rate Limiting on IPC Interface
- Source: DOMAIN-03-SECURITY.md
- Severity: MEDIUM
- Target: ipc.rs, main_loop.rs
- Description: Unbounded IPC request rate. Combined with timing attack vector, attacker can make unlimited measurements. Also a DoS vector.
- Recommendation: Add rate limiting to IPC socket handler.

---

**Summary:** 45 new findings extracted (6 CRITICAL, 23 HIGH, 16 MEDIUM).
Not counted: ~20 additional LOW findings from Android/test domains deferred to keep within line budget.
