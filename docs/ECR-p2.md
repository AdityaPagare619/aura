## §9 Performance & Concurrency

### 9.1 Overview

Performance analysis covered the daemon's async runtime, memory subsystem, LLM call patterns, and concurrency primitives. Four critical-severity performance issues were identified — each independently capable of causing production service degradation or system freeze. The Android memory budget is near its ceiling under peak load.

### 9.2 Critical Performance Issues

#### PERF-CRIT-C1 — New TCP Socket Per LLM Call
**File:** `react.rs` (NeocortexClient usage)

```rust
// Called on every ReAct iteration:
let client = NeocortexClient::connect(&self.config.neocortex_addr).await?;
let response = client.infer(prompt).await?;
// client dropped here — TCP socket closed
```

`NeocortexClient::connect()` opens a fresh TCP connection on every ReAct iteration. For a 10-step ReAct loop, this is 10 TCP handshakes to a local socket. Even on loopback, this adds ~5–20ms per call and creates unnecessary OS resource churn.

**Fix:** Maintain a persistent `NeocortexClient` in the daemon state, or use a connection pool. The client should be constructed once at daemon startup and reused.

#### PERF-CRIT-C2 — DGS Fast Path Permanently Disabled
**File:** `react.rs` — `classify_task()`

```rust
fn classify_task(&self, task: &Task) -> TaskRoute {
    // RouteClassifier logic exists but is bypassed:
    TaskRoute::SemanticReact  // hardcoded — always full LLM reasoning
}
```

The Direct Goal Satisfaction (DGS) path was designed for simple tasks that don't need full ReAct reasoning (e.g., "what time is it?", "set a timer"). It is permanently bypassed. Every task — regardless of complexity — goes through multi-step LLM reasoning. The 441-line `RouteClassifier` is dead code consuming no CPU but representing wasted design work and a missing optimization.

**Fix:** Either wire up `RouteClassifier` correctly, or delete it and document the conscious decision that all tasks use SemanticReact.

#### PERF-CRIT-C3 — O(n²) Context Truncation
**File:** `context.rs:385,398`

```rust
// In truncation loop — worst case ~47 iterations for 50-turn history:
while self.estimate_tokens() > self.max_tokens {
    self.history.remove(0);  // Vec::remove(0) = O(n) — shifts all elements left
    // estimate_tokens() also re-scans entire history: O(n)
}
// Total: O(n²) for full truncation
```

For a 50-turn conversation history being truncated, this loop runs ~47 times, each calling `remove(0)` (O(n) shift) and `estimate_tokens()` (O(n) scan). Total work: O(n²) ≈ 2,350 operations for what could be O(n).

**Fix:**
```rust
// Replace Vec<Message> with VecDeque<Message>:
use std::collections::VecDeque;
// Then:
while self.estimate_tokens() > self.max_tokens {
    self.history.pop_front();  // O(1)
}
```

#### PERF-CRIT-C4 — Full Prompt Rebuild on Every Truncation Iteration
**File:** `context.rs:398` (inside truncation loop)

`estimate_tokens()` re-assembles the full prompt string and re-runs the token estimator on every iteration of the truncation loop. For a context with 50 turns being truncated to fit budget, this means ~47 full prompt re-assemblies and token counts — approximately 47 redundant string concatenations.

**Fix:** Track a running token estimate and decrement it as messages are removed, instead of recomputing from scratch.

### 9.3 High Performance Issues

#### PERF-HIGH-001 — O(n) Visited Array Allocation in HNSW Search
**File:** `hnsw.rs:600`

```rust
fn search_layer(&self, query: &[f32], entry: NodeId, ef: usize, layer: usize) -> Vec<NodeId> {
    let mut visited = vec![false; self.nodes.len()];  // Allocated fresh every call
    // ...
}
```

`search_layer()` is called multiple times per HNSW query (once per layer). Each call allocates a `Vec<bool>` of size `self.nodes.len()`. With 100K nodes, this is 100KB per call, multiple calls per query. At high query rates, this is significant allocator pressure.

**Fix:** Use a `HashSet<NodeId>` (only stores visited nodes) or maintain a reusable visited bitset with a generation counter to avoid clearing.

#### PERF-HIGH-002 — Global Mutex Serializes All Embedding Threads
**File:** `embeddings.rs`

```rust
static EMBEDDING_CACHE: Mutex<Option<EmbeddingCache>> = Mutex::new(None);
```

A single global `std::sync::Mutex` guards the embedding cache. Every embedding operation — including cache reads — must acquire this lock. With `consolidation.rs` issuing 100 sequential `embed()` calls per Deep consolidation pass, this becomes a single-threaded bottleneck even on multi-core hardware.

**Fix:** Replace with `RwLock` for read-heavy workload, or use a concurrent cache (`DashMap`). Separate the hot path (cache lookup) from the cold path (embedding computation).

#### PERF-HIGH-003 — LRU Eviction is O(n) with Clone per Hit
**File:** `embeddings.rs`

The LRU eviction scans the entire cache linearly to find the least-recently-used entry. Additionally, every cache hit clones the 1536-dimensional embedding vector (1536 × 4 bytes = 6KB per clone).

**Fix:** Use `lru` crate for O(1) eviction. Return `Arc<Vec<f32>>` instead of cloning.

#### PERF-HIGH-004 — k-means Without SIMD
**File:** `consolidation.rs:569`

```rust
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum::<f32>()
    // scalar — no NEON SIMD
}
```

k-means clustering on 1536-dimensional vectors uses scalar dot products. ARM NEON can process 4 floats per instruction — SIMD would give 4–8× speedup on this inner loop. During Deep consolidation with 100 embeddings, this is measurable.

**Fix:** Use `std::simd` (nightly) or `nalgebra`/`faer` for vectorized operations.

#### PERF-HIGH-005 — 100 Sequential Embed Calls Hit Global Mutex
**File:** `consolidation.rs:543`

```rust
for memory in &memories {
    let embedding = self.embed(&memory.content).await?;  // Each hits global Mutex
    embeddings.push(embedding);
}
```

100 sequential `embed()` calls during Deep consolidation. Even if each is fast, 100 sequential Mutex acquisitions/releases with potential contention serializes what could be parallelized.

**Fix:** Batch embeddings or use `futures::stream::iter(...).buffer_unordered(N)` for concurrent embedding with bounded parallelism.

### 9.4 Concurrency Safety Issues

#### PERF-DEAD-001 — `block_on` Inside Async Context
**File:** `monitor.rs` — `ping_neocortex()`

```rust
async fn monitor_loop(&self) {
    // ...
    let result = self.handle.block_on(self.ping_neocortex());  // DEADLOCK RISK
}
```

Calling `handle.block_on()` from within an async context can deadlock if the current thread is a Tokio worker thread and the inner future needs to schedule work on that same thread. This is a known Tokio anti-pattern.

**Fix:** Use `.await` directly, or restructure to spawn the ping as a separate task.

#### PERF-DEAD-002 — Lock Ordering Violation
**File:** `episodic.rs`

```rust
async fn consolidate_episode(&self) {
    let _sqlite_guard = self.sqlite_mutex.lock().await;  // tokio Mutex
    tokio::task::spawn_blocking(move || {
        let _hnsw_guard = self.hnsw_mutex.lock().unwrap();  // std Mutex inside spawn_blocking
        // Holds both locks
    }).await;
}
```

Holds a tokio async Mutex while entering `spawn_blocking` and then acquiring a std sync Mutex. This creates an inconsistent lock ordering that differs from other code paths. If any other code path acquires HNSW first then SQLite, deadlock is guaranteed.

**Fix:** Document global lock ordering (always: SQLite → HNSW or always HNSW → SQLite) and enforce it with a `LockOrder` newtype that makes wrong-order acquisition a compile error.

### 9.5 Memory Budget Analysis

| Scenario | Estimated RSS | Ceiling | Risk |
|----------|--------------|---------|------|
| Idle daemon | ~120MB | 400MB | ✅ Safe |
| Normal operation (ReAct, 5 iterations) | 180–330MB | 400MB | ⚠️ Watch |
| Peak: Best-of-N (3 candidates) | 300–450MB | 400MB | 🔴 Can breach |
| Peak + consolidation running | 380–520MB | 400MB | 🔴 OOM risk |

Android kills processes exceeding their memory budget. At peak load (BoN sampling + concurrent consolidation), AURA can breach the 400MB ceiling and trigger an OOM kill, presenting as a crash with no error message.

**Mitigation:** Serialize BoN sampling and consolidation — never allow both to run concurrently.

### 9.6 Performance Findings Summary

| ID | Severity | Location | Issue |
|----|----------|----------|-------|
| PERF-CRIT-C1 | Critical | `react.rs` | New TCP socket per LLM call |
| PERF-CRIT-C2 | Critical | `react.rs` | DGS fast path permanently disabled |
| PERF-CRIT-C3 | Critical | `context.rs:385,398` | O(n²) context truncation |
| PERF-CRIT-C4 | Critical | `context.rs:398` | Full prompt rebuild per truncation iteration |
| PERF-HIGH-001 | High | `hnsw.rs:600` | O(n) visited array per search_layer call |
| PERF-HIGH-002 | High | `embeddings.rs` | Global Mutex serializes all embedding work |
| PERF-HIGH-003 | High | `embeddings.rs` | O(n) LRU eviction + 6KB clone per hit |
| PERF-HIGH-004 | High | `consolidation.rs:569` | Scalar dot products, no NEON SIMD |
| PERF-HIGH-005 | High | `consolidation.rs:543` | 100 sequential embed calls |
| PERF-HIGH-006 | High | `monitor.rs` | `block_on()` in async context — deadlock |
| PERF-HIGH-007 | High | `episodic.rs` | Lock ordering violation |
| PERF-MED-001 | Medium | Multiple | No memory pressure backpressure mechanism |
| PERF-MED-002 | Medium | `hnsw.rs` | HNSW index fully loaded in RAM — no paging |

---

## §10 LLM/AI Integration

### 10.1 Overview

The LLM integration covers the teacher pipeline, ReAct engine, context management, inference server FFI, and routing logic. The design is genuinely sophisticated — the 5-layer teacher stack with Best-of-N, Reflection, and Cascade self-improvement is real ML engineering, not cosmetic. However, several implementation gaps mean the system operates at a fraction of its designed capability, and one FFI issue constitutes undefined behavior.

### 10.2 Teacher Pipeline Status

| Layer | Name | Status | Notes |
|-------|------|--------|-------|
| 0 | GBNF Grammar Enforcement | ⚠️ PARTIAL | Applied post-generation only — not decode-time |
| 1 | Chain-of-Thought Prompting | ✅ REAL | Working |
| 2 | Logprob Steering | ✅ REAL | Working |
| 3 | Cascade Self-Improvement | ✅ REAL | Working |
| 4 | Reflection & Self-Critique | ✅ REAL | Working |
| 5 | Best-of-N Sampling | ✅ REAL | Working — memory risk at N=3 |

**Layer 0 (LLM-CRIT-002):** GBNF grammar enforcement is applied as a post-processing filter after generation completes (`inference.rs:368-385`). This means the model can generate non-conforming output and the grammar check only rejects it after full generation. True constrained decoding requires grammar application at the decode step, not post-hoc. This wastes full inference compute on outputs that will be rejected.

### 10.3 Critical LLM Issues

#### LLM-CRIT-001 — Const-to-Mutable Pointer Cast (= RUST-CRIT-001)
**File:** `lib.rs:1397`

```rust
let token_ptr = tokens.as_ptr() as *mut LlamaToken;
llama_eval(ctx, token_ptr, tokens.len() as i32, n_past, n_threads);
```

`tokens.as_ptr()` returns `*const LlamaToken`. Casting to `*mut` and passing to llama.cpp is undefined behavior — Rust's aliasing rules forbid this even if llama.cpp never actually writes through the pointer. The llama.cpp API takes `llama_token *` but does not mutate the input tokens. The correct fix is to update the FFI binding to `const llama_token *`, which matches the actual C API contract.

#### LLM-CRIT-002 — GBNF Grammar Applied Post-Generation
**File:** `inference.rs:368-385`

```rust
let output = self.generate_raw(prompt, params).await?;
// Grammar applied HERE — after full generation:
if let Some(grammar) = &self.grammar {
    if !grammar.validate(&output) {
        return Err(InferenceError::GrammarViolation);
    }
}
```

**Impact:** Full inference compute wasted on invalid outputs. For structured JSON output requirements, the model may need multiple retries, each consuming full context + generation time.

**Fix:** Use llama.cpp's built-in grammar sampling (`llama_grammar_init`, `llama_sample_grammar`) which enforces constraints at each decode step, making grammar violations impossible rather than just detectable.

### 10.4 RouteClassifier — 441 Lines of Dead Code

**File:** `react.rs` — `classify_task()`

```rust
pub fn classify_task(&self, task: &Task) -> TaskRoute {
    // RouteClassifier has 441 lines of logic including:
    // - Complexity scoring
    // - Task type detection
    // - DGS eligibility evaluation
    // All of it bypassed by:
    TaskRoute::SemanticReact
}
```

**Impact:** 
- Simple tasks (time queries, calculations, lookups) that could complete in 1 LLM call go through 5–10 ReAct iterations
- Response latency for simple tasks: ~10–30 seconds instead of ~2–3 seconds
- Token consumption: 5–10× higher than necessary for simple tasks

**Decision required:** Either fix the classifier (wire up `RouteClassifier` properly) or document that AURA intentionally routes all tasks through full reasoning and delete the dead code.

### 10.5 Context Window Underutilization

**File:** `config.rs` or equivalent — `DEFAULT_CONTEXT_BUDGET`

```rust
const DEFAULT_CONTEXT_BUDGET: usize = 2048;  // tokens
// Model supports: 32,768 tokens
// Utilization: 2048/32768 = 6.25%
```

The system is using 1/16th of the available context window. This means:
- Conversation histories are truncated far earlier than necessary
- Multi-step reasoning chains are artificially constrained
- The O(n²) truncation bug (PERF-CRIT-C3) fires far more often than it would with a proper budget

**Fix:** Set `DEFAULT_CONTEXT_BUDGET` to at least `24576` (75% of window, leaving headroom for system prompt and response). This is a one-line config change with significant capability impact.

### 10.6 Model Memory Leak

**File:** `model.rs:634-645`

```rust
impl Drop for LoadedModel {
    fn drop(&mut self) {
        // llama_free(self.ctx) is NOT called
        // llama_free_model(self.model) is NOT called
        // Memory is leaked on every model unload
    }
}
```

When `LoadedModel` is dropped (e.g., during model switching or daemon restart), the llama.cpp context and model allocations are not freed. For a model requiring 4–8GB of RAM, this is a critical memory leak that can exhaust available memory across daemon restarts without full process exit.

**Fix:**
```rust
impl Drop for LoadedModel {
    fn drop(&mut self) {
        unsafe {
            llama_free(self.ctx);
            llama_free_model(self.model);
        }
    }
}
```

### 10.7 RNG Seeding Risk

**File:** `lib.rs:1344-1351`

```rust
let seed = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap()
    .subsec_nanos();
llama_set_rng_seed(ctx, seed);
```

Using nanosecond timestamps as RNG seed has two issues:
1. **Determinism:** If two inference calls start within the same nanosecond (common under load), they get identical seeds → identical "random" outputs
2. **Predictability:** A nanosecond timestamp is not cryptographically unpredictable

**Fix:** Use `rand::thread_rng().gen::<u32>()` for a properly seeded RNG value.

### 10.8 Token Budget Drift

**Architecture issue — no file:line**

Two independent token tracking systems:
- `aura-daemon`: `TokenBudgetManager` — tracks tokens consumed by the ReAct loop
- `aura-neocortex`: `TokenTracker` — tracks tokens at inference time

These systems do not synchronize. Over a long ReAct loop, the daemon's budget accounting can drift from the neocortex's actual token consumption (due to prompt template overhead, system message injection, etc.). The daemon may believe it has budget remaining when the neocortex is already near context limit — causing inference failures mid-task.

**Fix:** Add a token sync RPC: neocortex returns actual token count consumed with each response; daemon uses this to update its `TokenBudgetManager`.

### 10.9 LLM/AI Integration Findings Summary

| ID | Severity | Location | Issue |
|----|----------|----------|-------|
| LLM-CRIT-001 | Critical | `lib.rs:1397` | Const→mut pointer cast — UB |
| LLM-CRIT-002 | Critical | `inference.rs:368-385` | GBNF post-generation only |
| LLM-HIGH-001 | High | `react.rs` | RouteClassifier dead — DGS never used |
| LLM-HIGH-002 | High | `model.rs:634-645` | LoadedModel Drop doesn't free llama.cpp memory |
| LLM-HIGH-003 | High | `lib.rs:1344-1351` | Weak RNG seeding |
| LLM-HIGH-004 | High | Multiple | MAX_REACT_ITERATIONS asymmetry (10 vs 5) |
| LLM-HIGH-005 | High | Multiple | Token budget drift between daemon and neocortex |
| LLM-MED-001 | Medium | `config.rs` | DEFAULT_CONTEXT_BUDGET=2048 (6.25% of window) |
| LLM-MED-002 | Medium | `react.rs` | No backoff/retry on LLM failure |
| LLM-MED-003 | Medium | `inference.rs` | No latency/token telemetry (local only) |

---

## §11 Android/Mobile Platform

### 11.1 Overview

The Android platform analysis revealed a stark quality split:

- **Rust platform layer** (`power.rs`, `thermal.rs`, `doze.rs`, `battery.rs`): **Production-quality.** Correct platform API usage, proper error handling, well-structured abstraction.
- **Kotlin integration layer** (`AuraForegroundService.kt`, `AuraDaemonBridge.kt`, `AuraAccessibilityService.kt`, JNI bridge): **7 crash-class critical defects.** The gap between these two layers suggests the Rust layer was built carefully and the Kotlin layer was written quickly without Android platform expertise.

**Bottom line:** The Android integration cannot run correctly on any current Android device.

### 11.2 Critical Android Defects

#### AND-CRIT-001 — Missing foregroundServiceType (Android 14 crash)
**File:** `AuraForegroundService.kt` + `AndroidManifest.xml`

Android 14 (API 34) requires all foreground services to declare a `foregroundServiceType`. Services that omit it throw `MissingForegroundServiceTypeException` on start. This crashes the service on all Android 14 devices — approximately 40% of the active Android device fleet as of 2026.

**Fix — AndroidManifest.xml:**
```xml
<service
    android:name=".AuraForegroundService"
    android:foregroundServiceType="dataSync|specialUse"
    android:exported="false" />
```
Also add to Manifest root:
```xml
<uses-permission android:name="android.permission.FOREGROUND_SERVICE_DATA_SYNC" />
<uses-permission android:name="android.permission.FOREGROUND_SERVICE_SPECIAL_USE" />
```

#### AND-CRIT-002 — Undeclared Permissions (SecurityException on ALL devices)
**File:** `AndroidManifest.xml`

```kotlin
// Used in AuraDaemonBridge.kt and monitor.rs Kotlin callbacks:
val connectivityManager = getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
val networkInfo = connectivityManager.activeNetworkInfo  // Requires ACCESS_NETWORK_STATE
```

`ACCESS_NETWORK_STATE` and `ACCESS_WIFI_STATE` are used but not declared in `AndroidManifest.xml`. Android throws `SecurityException` at runtime. This is not a crash on some devices — it crashes on **all** devices, **every time** network state is queried.

**Fix — add to AndroidManifest.xml:**
```xml
<uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
<uses-permission android:name="android.permission.ACCESS_WIFI_STATE" />
```

#### AND-CRIT-003 — Sensor Listeners Never Unregistered (Memory Leak)
**File:** `AuraDaemonBridge.kt`

```kotlin
class AuraDaemonBridge : SensorEventListener {
    fun startMonitoring() {
        sensorManager.registerListener(this, accelerometer, SensorManager.SENSOR_DELAY_NORMAL)
        // No matching unregisterListener() in onDestroy() or cleanup()
    }
    // onDestroy() exists but does NOT unregister sensor listeners
}
```

Sensor listeners hold a reference to the registering object. Without unregistration, the `AuraDaemonBridge` instance cannot be garbage collected even after the service is destroyed. Additionally, sensors keep firing callbacks into a "dead" service, consuming battery.

**Fix:**
```kotlin
override fun onDestroy() {
    super.onDestroy()
    sensorManager.unregisterListener(this)
    // ... other cleanup
}
```

#### AND-CRIT-004 — WakeLock Expires After 10 Minutes (Daemon Freezes)
**File:** `AuraForegroundService.kt`

```kotlin
wakeLock = powerManager.newWakeLock(
    PowerManager.PARTIAL_WAKE_LOCK, 
    "Aura::DaemonWakeLock"
).apply {
    acquire(10 * 60 * 1000L)  // 10 minutes — never renewed
}
```

WakeLock is acquired with a 10-minute timeout and never renewed. After 10 minutes, the CPU can sleep while the daemon is mid-task. This causes:
- Active ReAct loops to freeze
- Memory consolidation to halt mid-write (potential corruption)
- Sensor monitoring to stop silently

**Fix:** Use an indefinite WakeLock with explicit release, and release it in `onDestroy()`:
```kotlin
wakeLock.acquire()  // No timeout — indefinite
// In onDestroy():
if (wakeLock.isHeld) wakeLock.release()
```

#### AND-CRIT-005 — WakeLock Race Condition
**File:** `AuraDaemonBridge.kt`

```kotlin
@Volatile var wakeLockHeld: Boolean = false

fun renewWakeLock() {
    if (!wakeLockHeld) {          // Thread A reads false
                                   // Thread B reads false — RACE
        wakeLock.acquire()         // Both threads acquire
        wakeLockHeld = true        // Both set true
    }
}
```

`@Volatile` guarantees visibility but not atomicity for compound check-then-act operations. Two threads can simultaneously observe `wakeLockHeld == false` and both call `acquire()`, resulting in a double-acquired WakeLock that requires double release.

**Fix:** Use `@Synchronized` or `AtomicBoolean.compareAndSet()`:
```kotlin
private val wakeLockHeld = AtomicBoolean(false)

fun renewWakeLock() {
    if (wakeLockHeld.compareAndSet(false, true)) {
        wakeLock.acquire()
    }
}
```

#### AND-CRIT-006 — AccessibilityNodeInfo Not Recycled (Pool Exhaustion)
**File:** `AuraAccessibilityService.kt`

```kotlin
override fun onAccessibilityEvent(event: AccessibilityEvent) {
    val rootNode = getRootInActiveWindow()  // Acquires from pool
    processNode(rootNode)
    // rootNode.recycle() is NEVER called
}
```

`AccessibilityNodeInfo` objects come from a finite system pool. Each acquired node that isn't recycled permanently removes a slot from the pool. After enough events (typically hundreds to low thousands), the pool is exhausted and `getRootInActiveWindow()` returns null, breaking the accessibility service entirely.

**Fix:**
```kotlin
override fun onAccessibilityEvent(event: AccessibilityEvent) {
    val rootNode = getRootInActiveWindow() ?: return
    try {
        processNode(rootNode)
    } finally {
        rootNode.recycle()
    }
}
```

#### AND-CRIT-007 — No JNI Exception Checks After Kotlin Callbacks
**File:** `jni_bridge.rs`

```rust
// Current — no exception check:
env.call_method(obj, "onSensorUpdate", "(FFF)V", &[x.into(), y.into(), z.into()]);
// If onSensorUpdate() throws in Kotlin, JVM exception is now pending
// Next JNI call with pending exception = undefined behavior
```

JNI specification requires that after every JNI call that can throw a Java exception, the native code must check for a pending exception before making further JNI calls. Failure to do so constitutes undefined behavior and can cause JVM crashes.

**Fix pattern — apply to all JNI callbacks:**
```rust
env.call_method(obj, "onSensorUpdate", "(FFF)V", &[x.into(), y.into(), z.into()])?;
if env.exception_check().unwrap_or(true) {
    let _ = env.exception_clear();
    return Err(JniError::KotlinException("onSensorUpdate".into()));
}
```

### 11.3 High Android Issues

#### AND-HIGH-001 — Battery Threshold Mismatch
| Module | LOW threshold | CRITICAL threshold |
|--------|-------------|-------------------|
| `heartbeat.rs` | 20% | 10% |
| `monitor.rs` | 10% | 5% |

Two different modules disagree on what "low battery" means by 2×. Policy decisions based on battery state (defer tasks, reduce polling frequency) will behave inconsistently depending on which module is consulted.

#### AND-HIGH-002 — ABI Configuration Mismatch
**`build.gradle.kts`:**
```kotlin
abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")  // 3 ABIs
```
**`.cargo/config.toml`:**
```toml
[target.aarch64-linux-android]  # Only arm64 configured
linker = "..."
```

`armeabi-v7a` (32-bit ARM) and `x86_64` (emulator) builds are declared in Gradle but have no Cargo cross-compilation configuration. APK builds for these targets will fail silently or include ARM64 library for non-ARM64 targets.

#### AND-HIGH-003 — PolicyGate Evaluates Fabricated State
**File:** `system_api.rs`

Approximately 12 methods return hardcoded placeholder values:
```rust
pub fn get_available_storage() -> u64 { 1_000_000_000 }  // Always 1GB
pub fn get_cpu_temperature() -> f32 { 35.0 }  // Always 35°C
pub fn is_device_charging() -> bool { false }  // Always not charging
```

`PolicyGate` uses these values to make task scheduling decisions. Tasks that should be deferred (low storage, high temperature, charging state) are never deferred because the gate always sees the same fabricated "normal" state.

### 11.4 Android Quality Summary

| Layer | Quality | Issues |
|-------|---------|--------|
| Rust platform layer | Production-quality ✅ | Minor: battery threshold mismatch |
| Kotlin service layer | Crash-prone 🔴 | 7 critical, 5 high |
| JNI bridge | Unsafe 🔴 | UB on any Kotlin exception |
| CI Android pipeline | Never worked 🔴 | Never produced a valid APK |

### 11.5 Android Findings Summary

| ID | Severity | Location | Issue |
|----|----------|----------|-------|
| AND-CRIT-001 | Critical | `AuraForegroundService.kt` | Missing foregroundServiceType — Android 14 crash |
| AND-CRIT-002 | Critical | `AndroidManifest.xml` | Undeclared permissions — SecurityException all devices |
| AND-CRIT-003 | Critical | `AuraDaemonBridge.kt` | Sensor listeners never unregistered |
| AND-CRIT-004 | Critical | `AuraForegroundService.kt` | WakeLock expires after 10 min — daemon freezes |
| AND-CRIT-005 | Critical | `AuraDaemonBridge.kt` | WakeLock race condition |
| AND-CRIT-006 | Critical | `AuraAccessibilityService.kt` | AccessibilityNodeInfo never recycled |
| AND-CRIT-007 | Critical | `jni_bridge.rs` | No JNI exception checks — UB |
| AND-HIGH-001 | High | `heartbeat.rs` vs `monitor.rs` | Battery threshold mismatch (20% vs 10%) |
| AND-HIGH-002 | High | `build.gradle.kts` vs `.cargo/config.toml` | ABI mismatch — 2 of 3 targets fail |
| AND-HIGH-003 | High | `system_api.rs` | 12 stub methods — PolicyGate evaluates fake state |
| AND-HIGH-004 | High | `AuraDaemonBridge.kt` | No reconnection logic if Rust daemon crashes |
| AND-HIGH-005 | High | Multiple | No graceful degradation when daemon unavailable |
| AND-MED-001 | Medium | `AuraForegroundService.kt` | Notification channel not created for API 26+ |
| AND-MED-002 | Medium | `build.gradle.kts` | minSdk not specified — implicit compatibility risks |
| AND-MED-003 | Medium | Multiple | No Doze mode handling in Kotlin layer |
