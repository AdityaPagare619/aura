# AURA v4 — Contributing and Development Setup

> **Audience:** Engineers contributing code to AURA v4.  
> **Prerequisites:** Read [README.md](README.md) and all seven ADRs before writing any code.  
> **Status:** Living document.

---

## Table of Contents

1. [Development Environment Setup](#1-development-environment-setup)
2. [Rust Toolchain Setup](#2-rust-toolchain-setup)
3. [Running the Test Suite](#3-running-the-test-suite)
4. [Android Cross-Compilation](#4-android-cross-compilation)
5. [Code Architecture: Where Things Live](#5-code-architecture-where-things-live)
6. [Iron Laws Checklist for Contributors](#6-iron-laws-checklist-for-contributors)
7. [PR Process and Code Review Checklist](#7-pr-process-and-code-review-checklist)
8. [How to Add New Features](#8-how-to-add-new-features)
9. [Debugging: Tracing a Request Through the System](#9-debugging-tracing-a-request-through-the-system)
10. [Common Mistakes and How to Avoid Them](#10-common-mistakes-and-how-to-avoid-them)

---

## 1. Development Environment Setup

### 1.1 Windows + WSL2 (Primary Dev Path)

AURA cross-compiles from any host to `aarch64-linux-android`. Windows with WSL2 is the most common
setup.

```bash
# Install WSL2 (PowerShell as Administrator)
wsl --install -d Ubuntu-24.04

# Inside WSL2 — install system dependencies
sudo apt-get update && sudo apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    cmake \
    clang \
    llvm \
    unzip \
    curl \
    git \
    sqlite3 \
    libsqlite3-dev

# Install Android NDK (required for cross-compilation)
# Download NDK r26d or later from https://developer.android.com/ndk/downloads
# Extract to ~/android-ndk-r26d
export ANDROID_NDK_HOME=~/android-ndk-r26d
echo 'export ANDROID_NDK_HOME=~/android-ndk-r26d' >> ~/.bashrc
echo 'export PATH=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin:$PATH' >> ~/.bashrc
source ~/.bashrc
```

### 1.2 macOS

```bash
# Install Homebrew dependencies
brew install cmake llvm openssl pkg-config sqlite

# Set up LLVM paths (required for llama.cpp compilation)
export LLVM_SYS_160_PREFIX=$(brew --prefix llvm)
echo 'export LLVM_SYS_160_PREFIX=$(brew --prefix llvm)' >> ~/.zshrc

# Install Android NDK via Android Studio SDK Manager or directly:
# https://developer.android.com/ndk/downloads → extract to ~/android-ndk-r26d
export ANDROID_NDK_HOME=~/android-ndk-r26d
```

### 1.3 Linux (Ubuntu/Debian)

```bash
sudo apt-get update && sudo apt-get install -y \
    build-essential pkg-config libssl-dev cmake \
    clang llvm unzip curl git sqlite3 libsqlite3-dev

# Android NDK — same as WSL2 setup above
```

### 1.4 Clone the Repository

```bash
git clone https://github.com/AdityaPagare619/aura.git
cd aura-v4

# Initialize submodules (llama.cpp when it is vendored — currently P0 blocker)
git submodule update --init --recursive
```

---

## 2. Rust Toolchain Setup

AURA v4 requires a specific Rust toolchain. The `rust-toolchain.toml` at the repo root pins this —
`rustup` will auto-install on first `cargo` command.

```toml
# rust-toolchain.toml (do not change without team discussion)
[toolchain]
channel = "stable"
targets = ["aarch64-linux-android", "x86_64-linux-android", "aarch64-apple-ios"]
components = ["rustfmt", "clippy", "rust-src"]
```

Manual setup if needed:

```bash
# Install rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Add Android targets
rustup target add aarch64-linux-android
rustup target add x86_64-linux-android   # emulator target
rustup target add armv7-linux-androideabi # 32-bit legacy

# Verify
rustup show
cargo --version   # should be ≥ 1.77
```

### 2.1 Cargo Configuration

The `.cargo/config.toml` at the workspace root configures Android linkers. Verify your NDK path
matches:

```toml
# .cargo/config.toml (excerpt — do not edit manually)
[target.aarch64-linux-android]
linker = "aarch64-linux-android34-clang"

[target.x86_64-linux-android]
linker = "x86_64-linux-android34-clang"
```

If you get linker errors, check that `$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin`
is in your `PATH`.

---

## 3. Running the Test Suite

### 3.1 Full Test Suite

```bash
# Run all tests in the workspace
cargo test

# Run only the daemon tests (2362 tests — primary test target)
cargo test -p aura-daemon

# Run with output (useful for debugging failing tests)
cargo test -p aura-daemon -- --nocapture

# Run a specific test module
cargo test -p aura-daemon memory::episodic

# Run a specific test by name
cargo test -p aura-daemon test_policy_gate_deny_by_default
```

### 3.2 Linting and Formatting

All PRs must pass these before merge:

```bash
# Format check (CI enforces this)
cargo fmt --all -- --check

# Clippy (treat warnings as errors in CI)
cargo clippy --all-targets --all-features -- -D warnings

# Type check without building (fast)
cargo check --all-targets
```

### 3.3 Test Categories

| Category | Command | What it covers |
|----------|---------|----------------|
| Unit tests | `cargo test -p aura-daemon` | Per-module logic |
| Type system | `cargo check` | Compile-time correctness |
| Integration | `cargo test -p aura-daemon --test '*'` | Multi-module flows (being added) |
| Policy gate | `cargo test -p aura-daemon policy` | Safety enforcement |
| Memory | `cargo test -p aura-daemon memory` | 4-tier memory system |

> **Note:** Android instrumented tests and E2E tests do not exist yet (P1 gap). See
> [AURA-V4-PRODUCTION-STATUS.md](AURA-V4-PRODUCTION-STATUS.md).

### 3.4 Iron Law IL-4: Never Change Production Logic to Pass Tests

If a test is failing, the answer is **never** to weaken production assertions. Options in order of
preference:

1. Fix the bug in production code
2. Fix the test if it is wrong (with explanation in commit message)
3. Mark the test `#[ignore]` with a tracking issue if it requires infrastructure not available

---

## 4. Android Cross-Compilation

### 4.1 Build for Android ARM64

```bash
# Build the daemon shared library for Android
cargo build --release \
    --target aarch64-linux-android \
    -p aura-daemon

# Output: target/aarch64-linux-android/release/libaura_core.so
```

### 4.2 Build the Neocortex Binary

```bash
cargo build --release \
    --target aarch64-linux-android \
    -p aura-neocortex

# Output: target/aarch64-linux-android/release/aura-neocortex
```

### 4.3 Copy to Android Project (When Kotlin shell exists)

```bash
# Copy .so to jniLibs (run from workspace root)
mkdir -p android-app/app/src/main/jniLibs/arm64-v8a/
cp target/aarch64-linux-android/release/libaura_core.so \
   android-app/app/src/main/jniLibs/arm64-v8a/
```

### 4.4 Verify Cross-Compilation Without Android Device

```bash
# Check the binary is actually ARM64
file target/aarch64-linux-android/release/libaura_core.so
# Expected: ELF 64-bit LSB shared object, ARM aarch64

# Check exported JNI symbols
nm -D target/aarch64-linux-android/release/libaura_core.so | grep Java_
```

### 4.5 Current Cross-Compilation Status

> ⚠️ Cross-compilation is configured but **not verified end-to-end** (see `PRODUCTION-READINESS`
> §3). The `.cargo/config.toml` and `rust-toolchain.toml` are correct, but llama.cpp is not yet
> vendored, so a full build will fail at the `aura-llama-sys` link step.

---

## 5. Code Architecture: Where Things Live

```
crates/
├── aura-types/                    # ALL shared types — no logic here
│   └── src/
│       ├── ipc.rs                 # DaemonToNeocortex, NeocortexToDaemon enums
│       ├── plan.rs                # ActionPlan, PlanStep, ToolDefinition
│       ├── etg.rs                 # ETG graph types
│       ├── memory.rs              # MemoryItem, MemoryTier
│       └── context.rs             # ContextPackage
│
├── aura-daemon/                   # The body — everything except LLM inference
│   └── src/
│       ├── daemon_core/
│       │   ├── main_loop.rs       # tokio event loop, select! over 7 channels
│       │   ├── react.rs           # classify_task() (intentional stub → System2)
│       │   └── routing.rs         # System1/System2 dispatch
│       │
│       ├── memory/                # 14 modules
│       │   ├── working.rs         # Tier 1: hot context, BoundedVec
│       │   ├── episodic.rs        # Tier 2: SQLite-backed events
│       │   ├── semantic.rs        # Tier 3: distilled facts
│       │   ├── archive.rs         # Tier 4: cold compressed storage
│       │   ├── hnsw.rs            # Pure-Rust HNSW vector index
│       │   ├── embeddings.rs      # TF-IDF + neural embedding dispatch
│       │   ├── patterns.rs        # Hebbian wiring
│       │   ├── consolidation.rs   # Sleep-stage consolidation pipeline
│       │   ├── vault.rs           # AES-256-GCM CriticalVault
│       │   └── intelligence.rs    # MemoryIntelligence: spreading activation
│       │
│       ├── goals/
│       │   ├── registry.rs        # GoalRegistry: active + queued goals
│       │   ├── scheduler.rs       # Priority scheduling, deadline tracking
│       │   ├── bdi.rs             # BDI agent: Beliefs-Desires-Intentions
│       │   └── decomposer.rs      # HTN goal decomposition
│       │
│       ├── identity/              # 12 modules
│       │   ├── ocean.rs           # Big Five personality (OCEAN)
│       │   ├── vad.rs             # Valence-Arousal-Dominance mood
│       │   ├── ethics.rs          # Hardcoded ethics gate (Layer 2 safety)
│       │   ├── relationship.rs    # Trust + relationship stage tracking
│       │   └── truth.rs           # Anti-sycophancy TRUTH framework
│       │
│       ├── policy/                # 8 modules — Layer 1 safety
│       │   ├── gate.rs            # PolicyGate: deny-by-default
│       │   ├── rules.rs           # Configurable rule engine
│       │   ├── sandbox.rs         # Action sandboxing
│       │   ├── audit.rs           # Audit log for sensitive actions
│       │   └── boundaries.rs      # Hard action limits
│       │
│       ├── execution/
│       │   ├── executor.rs        # 11-stage pipeline
│       │   ├── etg.rs             # ETG: Execution Template Graph
│       │   ├── planner.rs         # Plan lifecycle management
│       │   ├── monitor.rs         # Step completion verification
│       │   └── retry.rs           # Backoff + replanning
│       │
│       ├── arc/                   # ARC: Autonomous Reasoning Core
│       │   ├── life_arc.rs        # 10 life domains, scoring
│       │   ├── health.rs          # Device health monitoring
│       │   ├── proactive.rs       # Proactive trigger engine
│       │   ├── routines.rs        # Routine learning + suggestions
│       │   ├── social.rs          # Social awareness subsystem
│       │   └── forest_guardian.rs # Attention protection, doomscroll detection
│       │
│       ├── screen/
│       │   ├── selector.rs        # L0-L7 selector cascade
│       │   ├── actions.rs         # Tap, type, scroll, swipe
│       │   └── state.rs           # ScreenDescription capture
│       │
│       └── platform/
│           ├── jni.rs             # JNI bridge (entry point from Kotlin)
│           ├── power.rs           # Battery + thermal monitoring
│           ├── notifications.rs   # Android notification channels
│           ├── connectivity.rs    # WiFi, Bluetooth state
│           └── sensors.rs         # Accelerometer, ambient light
│
├── aura-neocortex/                # The brain — LLM inference only
│   └── src/
│       ├── inference.rs           # llama.cpp inference wrapper
│       ├── context.rs             # Context assembly + token counting
│       ├── model.rs               # Model loading, tier management
│       ├── grammar.rs             # GBNF grammar enforcement
│       ├── prompts.rs             # System prompt builder
│       └── model_capabilities.rs  # Tier-specific parameter tables
│
├── aura-llama-sys/                # FFI bindings to llama.cpp + GGUF metadata parser
│   └── src/
│       ├── lib.rs                 # ❌ Outdated — needs batch API update
│       └── gguf_meta.rs           # ✅ GGUF metadata parser (production quality)
```

---

## 6. Iron Laws Checklist for Contributors

Run through this checklist before every PR. Every "yes" to a violation question is a blocker.

```
IRON LAWS COMPLIANCE CHECKLIST
================================

IL-1 — LLM = Brain, Rust = Body
  [ ] Does any new Rust code interpret the meaning of user input?
  [ ] Does any Rust code generate behavioral directives for the LLM?
  [ ] Does any Rust code decide which action to take based on text content?
  → If any box checked: STOP. Move that logic to the LLM prompt.

IL-2 — Theater AGI Banned
  [ ] Does any new Rust code contain string matching on user-facing language?
  [ ] Does any code have if/else chains like if intent == "cancel" { ... }?
  [ ] Does any code "score" user intent using keyword presence?
  → If any box checked: STOP. This is Theater AGI. Delete it.

IL-3 — Fast-Path Parsers Acceptable
  [ ] Does the new structural parser match only fixed syntax (e.g., "open <app>")?
  [ ] Is the parser in screen/selector.rs or a clearly marked fast-path module?
  → Fast-path parsers for open/call/timer/alarm/brightness/wifi are explicitly allowed.

IL-4 — Never Change Production Logic for Tests
  [ ] Did you weaken any assertion, guard, or validation to make a test pass?
  [ ] Did you add special-case behavior gated on #[cfg(test)]?
  → If yes: Revert. Fix the code or fix the test with justification.

IL-5 — Anti-Cloud Absolute
  [ ] Does any new code make a network request?
  [ ] Does any new code fall back to a remote API?
  [ ] Does any new code send user data off-device?
  → If yes: Delete it. No exceptions.

IL-6 — Privacy-First
  [ ] Is all new user data stored exclusively on-device?
  [ ] Is sensitive new data classified and encrypted via CriticalVault?
  [ ] Does GDPR export cover the new data?
  → Verify data classification in MEMORY-AND-DATA-ARCHITECTURE §10.

IL-7 — No Sycophancy
  [ ] Does any prompt change encourage agreement with the user over truth?
  [ ] Does any identity state change lower the honesty weight?
  → Review IDENTITY-ETHICS §5 anti-sycophancy system before touching identity/truth.rs.
```

---

## 7. PR Process and Code Review Checklist

### 7.1 Before Opening a PR

1. Run the full test suite: `cargo test`
2. Run clippy: `cargo clippy --all-targets -- -D warnings`
3. Run format check: `cargo fmt --all -- --check`
4. Complete the Iron Laws Checklist (§6)
5. If your change affects the IPC contract (adds/modifies `DaemonToNeocortex` or
   `NeocortexToDaemon` variants), update `AURA-V4-GROUND-TRUTH-ARCHITECTURE.md`

### 7.2 PR Description Template

```markdown
## What This Does
[1-3 sentence summary]

## Iron Laws Check
- IL-1 (Rust reasons nothing): [pass/N/A]
- IL-2 (No Theater AGI): [pass/N/A]
- IL-4 (No production changes for tests): [pass/N/A]
- IL-5 (No cloud): [pass/N/A]
- IL-6 (Privacy): [pass/N/A]

## Testing
- [ ] `cargo test` passes
- [ ] `cargo clippy` clean
- [ ] New tests added for new logic
- [ ] Relevant docs updated

## Docs Updated
- [ ] Architecture doc updated (if applicable)
- [ ] ADR added (if new architectural decision)
```

### 7.3 Code Review Criteria

Reviewers check in this order:

1. **Iron Law violations** — any violation blocks merge
2. **Test coverage** — new logic without tests is a soft block (requires justification)
3. **Bounds checking** — any new collection must be `BoundedVec` / `BoundedMap` / `CircularBuffer`
4. **Error handling** — no `unwrap()` in production paths (tests: acceptable)
5. **IPC contract** — breaking changes to `aura-types` require type version bump
6. **Doc accuracy** — if the PR changes behavior, docs must reflect it

---

## 8. How to Add New Features

### 8.1 Adding a New ARC Life Domain

ARC domains are defined in `crates/aura-daemon/src/arc/mod.rs`.

1. Add a new variant to the `DomainId` enum
2. Add a scoring weight to the domain weights configuration
3. Add context mode transitions involving the new domain to `ContextMode`
4. Add proactive trigger conditions in `proactive.rs` if needed
5. Update `AURA-V4-ARC-BEHAVIORAL-INTELLIGENCE.md` §2 with the new domain
6. Add tests: domain scoring, transition conditions, LQI formula impact

```rust
// Example: adding a new domain (see arc/mod.rs)
pub enum DomainId {
    Health = 0,
    Social = 1,
    Productivity = 2,
    Finance = 3,
    Lifestyle = 4,
    Entertainment = 5,
    Learning = 6,
    Communication = 7,
    Environment = 8,
    PersonalGrowth = 9,
    YourNewDomain,  // ← add here
}
```

### 8.2 Adding a New Tool / Action Type

Tools are the typed actions the LLM can invoke. They span two locations:

**1. Type definition** (`crates/aura-types/src/plan.rs`):

```rust
pub enum ToolAction {
    LaunchApp { package: String },
    SendNotification { title: String, body: String },
    // ... existing tools ...
    YourNewTool { param1: String, param2: u32 },  // ← add here
}
```

**2. Execution handler** (`crates/aura-daemon/src/execution/executor.rs`):

```rust
async fn execute_action(&self, action: &ToolAction) -> Result<ActionOutcome> {
    match action {
        ToolAction::YourNewTool { param1, param2 } => {
            // Execute the action — Rust body, not brain
            // No reasoning here. Just execution.
            self.platform.do_the_thing(param1, *param2).await
        }
        // ...
    }
}
```

**3. Tool description for LLM** (`crates/aura-neocortex/src/prompts.rs`):

Add a description to the tool definition list that goes into the system prompt. The LLM needs to
know what the tool does, its parameters, and when to use it.

**4. Policy gate** (`crates/aura-daemon/src/policy/rules.rs`):

Add a policy rule for the new tool. Default to the most restrictive tier and relax only if
justified. See `AURA-V4-SECURITY-MODEL.md` §4 for trust tier definitions.

**5. Tests**: Add unit tests for the executor handler and policy rule.

### 8.3 Adding a New Memory Tier

The 4-tier architecture (working → episodic → semantic → archive) is defined in ADR-003. Adding a
fifth tier would be an architectural change requiring a new ADR.

To modify an existing tier (e.g., increase capacity, change retention policy):

1. Find the tier in `crates/aura-daemon/src/memory/<tier>.rs`
2. Check `BoundedVec` capacities — these are set as `const CAP: usize` constants
3. Update the importance scoring formula in `memory/intelligence.rs` if promotion thresholds change
4. Update `AURA-V4-MEMORY-AND-DATA-ARCHITECTURE.md` §2 and §3
5. Run `cargo test -p aura-daemon memory` — all memory tests must pass

### 8.4 Adding a New IPC Message Type

IPC messages are the contract between the daemon and neocortex. Changes here are high-impact.

1. Add the new variant to `DaemonToNeocortex` or `NeocortexToDaemon` in
   `crates/aura-types/src/ipc.rs`
2. Handle the new variant in `crates/aura-daemon/src/daemon_core/routing.rs`
3. Handle the new variant in `crates/aura-neocortex/src/inference.rs`
4. Update `AURA-V4-MASTER-SYSTEM-ARCHITECTURE.md` §3 IPC variant table
5. Update `AURA-V4-GROUND-TRUTH-ARCHITECTURE.md` §2 IPC Contract

---

## 9. Debugging: Tracing a Request Through the System

### 9.1 Enable Trace Logging

```bash
# Set RUST_LOG before running tests or the daemon
export RUST_LOG=aura_daemon=trace,aura_neocortex=trace

# For a specific module only
export RUST_LOG=aura_daemon::daemon_core::react=trace

# Run with logging
cargo test -p aura-daemon -- --nocapture 2>&1 | less
```

### 9.2 Request Lifecycle (Where to Set Breakpoints)

```
1. User input arrives
   → platform/jni.rs: Java_dev_aura_v4_AuraDaemonBridge_processInput()
   → daemon_core/main_loop.rs: event loop receives InputEvent

2. Routing decision
   → daemon_core/react.rs: classify_task()  ← always returns SemanticReact
   → daemon_core/routing.rs: dispatch_to_system2()

3. Context assembly
   → memory/working.rs: get_recent_context()
   → memory/episodic.rs: retrieve_relevant()
   → memory/semantic.rs: lookup_facts()
   → identity/ocean.rs + vad.rs: get_personality_state()
   → platform/screen.rs: capture_screen_state()

4. IPC send to Neocortex
   → daemon_core/ipc_client.rs: send_message(DaemonToNeocortex::Converse)

5. LLM reasoning (Neocortex process)
   → neocortex/inference.rs: run_inference()
   → neocortex/context.rs: assemble_context()  ← token budget applied here
   → neocortex/grammar.rs: enforce_gbnf()

6. IPC receive from Neocortex
   → daemon_core/ipc_client.rs: receive_message()
   → NeocortexToDaemon::ConversationReply | PlanReady | ReActDecision

7. Execution (if PlanReady)
   → execution/executor.rs: execute_step()  ← 11-stage pipeline
   → policy/gate.rs: evaluate_action()  ← safety check
   → screen/actions.rs: perform_action()

8. Outcome feedback
   → daemon_core/outcome_bus.rs: publish(ActionOutcome)
   → memory, goals, identity, arc, hebbian: subscribe and update
```

### 9.3 Common Debugging Scenarios

**"The LLM isn't responding"**
- Check if Neocortex process is running: look for `aura-neocortex` in process list
- Check IPC socket: `/tmp/aura-neocortex.sock` (host) or `/data/data/dev.aura.v4/aura.sock` (Android)
- Check model loaded: look for `[neocortex] Model loaded: tier=Standard` in logs
- Verify llama.cpp is vendored (`git submodule status`)

**"A test is failing with 'policy denied'"**
- Check `policy/gate.rs` — deny-by-default means unknown actions are denied
- Add the action to the appropriate trust tier in `policy/rules.rs`
- Verify the test is not relying on allow-by-default (that was the pre-fix behavior)

**"Memory retrieval returns nothing"**
- Check tier promotion thresholds — episodic memories need `importance > 0.3` for promotion
- Check HNSW index is populated: `memory/hnsw.rs` insert path
- Verify embeddings are being generated: check `memory/embeddings.rs` for TF-IDF fallback

---

## 10. Common Mistakes and How to Avoid Them

### Mistake 1: Adding NLU logic in Rust

```rust
// ❌ WRONG — Theater AGI, violates IL-1 and IL-2
fn route_request(input: &str) -> Route {
    if input.contains("cancel") || input.contains("stop") {
        Route::CancelCurrentTask
    } else if input.contains("remind") {
        Route::SetReminder
    } else {
        Route::Unknown
    }
}

// ✅ CORRECT — Rust routes to LLM, LLM decides
fn route_request(_input: &str) -> Route {
    Route::SemanticReact  // Always. LLM classifies.
}
```

### Mistake 2: Unbounded collections

```rust
// ❌ WRONG — can OOM on mobile
let mut memories: Vec<MemoryItem> = Vec::new();

// ✅ CORRECT — bounded at compile time
let mut memories: BoundedVec<MemoryItem, 256> = BoundedVec::new();
```

### Mistake 3: Blocking on the main event loop

```rust
// ❌ WRONG — blocks tokio single-thread runtime
fn handle_request(&self, req: Request) {
    let result = std::thread::sleep(Duration::from_secs(1));  // blocks!
}

// ✅ CORRECT — async all the way
async fn handle_request(&self, req: Request) {
    tokio::time::sleep(Duration::from_secs(1)).await;
}
```

### Mistake 4: Storing reasoning in Rust state

```rust
// ❌ WRONG — Rust is deciding what the LLM "should" think
struct DaemonState {
    inferred_user_mood: String,    // No. LLM infers mood.
    guessed_user_intent: Intent,  // No. LLM classifies intent.
}

// ✅ CORRECT — Rust stores raw observable facts
struct DaemonState {
    vad_valence: f32,         // Raw number from identity system
    last_action_success: bool, // Observable fact
    interaction_count: u32,   // Counter
}
```

### Mistake 5: Breaking the IPC contract

Every change to `aura-types/src/ipc.rs` affects both the daemon and neocortex. Adding a new variant
without handling it in both processes causes a panic in the deserializer. Always:

1. Add the variant to the enum in `aura-types`
2. Add `match` arm in daemon handler
3. Add `match` arm in neocortex handler
4. Run `cargo check --all-targets` to verify no missing match arms

### Mistake 6: Writing to SQLite on the main loop

SQLite writes are synchronous I/O. Run them in `tokio::task::spawn_blocking`:

```rust
// ✅ CORRECT
let result = tokio::task::spawn_blocking(move || {
    db.execute("INSERT INTO memories ...", params)
}).await?;
```

### Mistake 7: Not testing the policy gate

Any new tool action must be tested against the policy gate. The gate is deny-by-default.
If your test doesn't explicitly grant the required trust tier, the action will be denied.

```rust
#[tokio::test]
async fn test_my_new_tool() {
    let mut policy = PolicyGate::new_for_test();
    policy.grant_tier(TrustTier::Trusted);  // ← explicit grant for test
    
    let result = executor.execute_action(&ToolAction::YourNewTool { ... }, &policy).await;
    assert!(result.is_ok());
}
```

---

*Questions? Check the ADRs for the "why" before filing an issue. The answer is often there.*
