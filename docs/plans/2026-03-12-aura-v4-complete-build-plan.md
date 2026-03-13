# AURA v4 — Complete Build Plan (March 12, 2026)

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Take AURA v4 from "architecturally cleaned codebase" to "real-world deployable on-device Android AI assistant" — zero Theater AGI, fully wired subsystems, passing tests, Android APK that boots.

**Architecture:** Two-process (daemon PID1 + neocortex PID2), Unix socket IPC, LLM=brain/Rust=body, privacy-first on-device. See `docs/AURA-V4-GROUND-TRUTH-ARCHITECTURE.md` for canonical spec.

**Tech Stack:** Rust (stable), tokio, llama.cpp FFI, SQLite, Android NDK, Telegram bot API, Unix domain sockets, bincode IPC framing.

**Compass:** `docs/redesign/HONEST-REFLECTION-AND-PHILOSOPHY-SHIFT.md`

**Iron Laws (never violate):**
- LLM reasons, Rust executes — never swap roles
- Theater AGI = Rust making behavioral decisions from user content analysis — BANNED
- Zero cloud, zero telemetry, zero tolerance
- `cargo check --workspace` must be zero errors at end of every phase
- Backups to `checkpoints/` before modifying any file

---

## Current Baseline (March 12, 2026)

| Metric | State |
|---|---|
| `cargo check` | ✅ 0 errors, 47 warnings |
| `cargo test` | ✅ 2051 passing, 0 failing |
| Total files | 166 Rust files, ~116,706 lines |
| Theater AGI removed | ~80% done |
| Subsystems wired | ~40% done (PolicyGate, ethics, affective, BDI NOT in path) |

## What Was Done In Prior Sessions

### Theater AGI Removed (confirmed):
- `memory/`: importance, compaction, working, mod, feedback — clean
- `policy/`: gate, emergency, sandbox, wiring — fixed  
- `goals/`: registry, conflicts, scheduler, decomposer, tracker — fixed
- `routing/`: classifier, system1 — verified clean
- `screen/`: selector, reader, actions — fixed; semantic.rs deleted
- `daemon_core/`: main_loop, react, calibration, onboarding, tutorial — fixed
- `execution/executor.rs` — PolicyGate wired via production_policy_gate()
- `identity/`: all files fixed this session
- `pipeline/contextor.rs` — fixed
- `telegram/`: approval, security, dialogue — fixed
- `persistence/`: vault, safe_mode, integrity, journal — fixed
- `aura-neocortex/`: model, inference, prompts — fixed
- `aura-types/events.rs` — compile error fixed
- `extensions/`: loader, discovery — fixed
- `arc/`: attention, skills, mod, morning, health/sleep, health/medication, proactive/routines, proactive/welcome — fixed

### NOT Yet Verified (agents returned empty — treat as unconfirmed):
- `execution/`: cycle, etg, planner, react, retry, tools, monitor, learning/
- `pipeline/`: amygdala, entity, parser, slots, mod
- `memory/`: consolidation, embeddings, episodic, export, patterns, semantic, workflows, archive, hnsw
- `aura-types/`: actions, config, dsl, errors, etg, goals, identity, ipc, manifest, memory, outcome, power, screen, tools
- `aura-llama-sys/`: lib, gguf_meta
- `arc/`: life_arc/, social/, learning/interests, cron, routines

---

## Phase 1: Verify & Baseline Lock

**Goal:** Confirm exact current state. Know precisely what is clean and what is not.

### Step 1.1: Run cargo check + test
```
cd C:\Users\Lenovo\aura-v3\aura-v4
cargo check --workspace 2>&1
cargo test --workspace 2>&1 | tail -5
```
Expected: 0 errors, 2051+ tests passing.

### Step 1.2: Audit unconfirmed files — execution/
Read each file in `execution/` and report Theater AGI violations + unbounded collections.
Files: `cycle.rs`, `etg.rs`, `planner.rs`, `react.rs`, `retry.rs`, `tools.rs`, `monitor.rs`, `learning/mod.rs`, `learning/workflows.rs`

### Step 1.3: Audit unconfirmed files — pipeline/
Files: `amygdala.rs`, `entity.rs`, `parser.rs`, `slots.rs`, `mod.rs`

### Step 1.4: Audit unconfirmed files — aura-types/
Files: all 14 type definition files + check ipc.rs for ReActStep/ReActDecision

### Step 1.5: Audit unconfirmed files — aura-llama-sys/
Files: `lib.rs`, `gguf_meta.rs`

### Step 1.6: Audit remaining arc/ files
Files: `life_arc/*.rs`, `social/*.rs`, `learning/interests.rs`, `cron.rs`, `routines.rs`

### Step 1.7: Audit root-level daemon files
Files: `reaction.rs`, `outcome_bus.rs`, `integration_tests.rs`, `policy_ethics_integration_tests.rs`

### Step 1.8: Audit ipc/ subsystem
Files: `client.rs`, `protocol.rs`, `spawn.rs`, `mod.rs`

### Step 1.9: Audit voice/ remaining
Files: `biomarkers.rs`, `modality_state_machine.rs`, `stt.rs`, `tts.rs`, `vad.rs`, `wake_word.rs`, `audio_io.rs`, `call_handler.rs`, `signal_processing.rs`

**Commit after Phase 1:** `audit: complete Theater AGI sweep, all files confirmed clean`

---

## Phase 2: Fix Known Architecture Bugs

From `AURA-V4-GROUND-TRUTH-ARCHITECTURE.md` Surgery Map (§7, FIX section):

### Step 2.1: Fix inference.rs:789 — type mismatch
**File:** `crates/aura-neocortex/src/inference.rs:789`
**Bug:** Uses `aura_types::ipc::GenerationConfig` where `ModeConfig` is required — compile-time or logic error
**Fix:** Replace with correct `ModeConfig` type

### Step 2.2: Fix inference.rs:503 — wrong ReAct goal
**File:** `crates/aura-neocortex/src/inference.rs:503`  
**Bug:** ReAct goal set to `base_prompt.system_prompt.clone()` instead of original user request
**Fix:** Add `original_goal: String` field to `AssembledPrompt`; use it at line 503

### Step 2.3: Fix affective.rs:211-218 — wrong conditional ordering
**File:** `crates/aura-daemon/src/identity/affective.rs:211-218`
**Bug:** Incorrect conditional ordering in mood context string generation
**Fix:** Reorder conditionals to match intended semantic (high arousal + positive valence = excited, not calm)

### Step 2.4: Fix user_profile.rs:325-341 — effective_ocean() no-op
**File:** `crates/aura-daemon/src/identity/user_profile.rs:325-341`
**Bug:** `effective_ocean()` returns unadjusted values at default calibration — function is no-op
**Fix:** Either fix the calibration logic (apply the adjuster properly) or remove the function and call sites

### Step 2.5: Fix main_loop.rs handle_cron_tick — God Function
**File:** `crates/aura-daemon/src/daemon_core/main_loop.rs`
**Bug:** 1,200-line string-matching God Function in handle_cron_tick
**Fix:** Refactor to event-dispatch table (map event_type → handler function)

**Cargo check after every fix. Commit:** `fix: resolve architecture bugs per ground-truth spec`

---

## Phase 3: Wire PolicyGate + Ethics into Live Paths

This is the most critical safety wiring. Currently dangerous actions CAN execute without safety checks.

### Step 3.1: Locate exact insertion point in react.rs
Read `daemon_core/react.rs` — find where `executor.execute(action)` is called (not `execution/executor.rs` — the call site is in `daemon_core/react.rs`)

### Step 3.2: Wire PolicyGate before executor.execute()
```rust
// In daemon_core/react.rs — before executor.execute(action)
let policy_result = self.policy_gate.evaluate(&action).await?;
match policy_result.effect {
    RuleEffect::Deny => {
        return Ok(ReActOutcome::Denied { reason: policy_result.reason });
    }
    RuleEffect::Confirm => {
        // Send approval request via Telegram, await response
        let approved = self.request_user_approval(&action).await?;
        if !approved { return Ok(ReActOutcome::Denied { reason: "User declined".into() }); }
    }
    RuleEffect::Allow | RuleEffect::Audit => { /* proceed */ }
}
```

### Step 3.3: Wire ethics.check_response() before ResponseRouter
Read `daemon_core/main_loop.rs` — find where responses are sent to ResponseRouter.
Wire `ethics.check_response(response)` call before any response reaches the user.

### Step 3.4: Wire anti-sycophancy check in response path
Wire `anti_sycophancy.check(response)` immediately after ethics check.
If flagged: send back to neocortex with "reconsider" instruction (or log + pass through if no neocortex available).

### Step 3.5: Write 20+ integration tests
File: `src/policy_ethics_integration_tests.rs`
Test scenarios:
- Allowed action passes PolicyGate → executor called
- Banking action denied by PolicyGate → executor NOT called, denial returned
- CONFIRM action → approval requested → if approved executor called
- Ethics check passes clean response → response delivered
- Ethics check flags sycophantic response → modified or rejected
- Anti-sycophancy catches "you're absolutely right about everything"

**Cargo check + cargo test. Commit:** `feat: wire PolicyGate and ethics into live execution and response paths`

---

## Phase 4: Wire AffectiveEngine + BDI Scheduler

### Step 4.1: Wire AffectiveEngine into main_loop
Read `identity/affective.rs` to understand `process_event()` API.
In `main_loop.rs`:
- Replace simple EWMA mood tracking with `AffectiveEngine::process_event()` calls
- Wire on: user message received, action completed, action failed, proactive suggestion accepted/rejected
- Pass resulting VAD state into ContextPackage (not as directive — as raw values per architecture spec)

### Step 4.2: Wire BDI scheduler
Read `goals/scheduler.rs` to understand `schedule()` and `next_goal()` API.
In `main_loop.rs`:
- Replace `checkpoint.goals` Vec access with `scheduler.schedule()` and `scheduler.next_goal()`
- Wire goal completion/failure back to scheduler

### Step 4.3: Wire ProactiveEngine::tick()
Read `arc/proactive/suggestions.rs` to understand `tick()` API.
In `main_loop.rs` periodic timer section:
- Add `proactive_engine.tick().await` every 5 minutes
- Route resulting suggestions through ResponseRouter to user

### Step 4.4: Write 15+ tests
- VAD updates on user events (positive tone → valence increases)
- BDI goal scheduling (complex goal → HTN decomposition)
- Proactive engine generates morning briefing at correct time

**Cargo check + cargo test. Commit:** `feat: wire AffectiveEngine, BDI scheduler, ProactiveEngine`

---

## Phase 5: Close the ReAct Loop (Critical)

Currently `daemon_core/react.rs` uses `simulate_action_result()` — the loop is OPEN. The neocortex never sees real screen state.

### Step 5.1: Add ReActObservation + ReActStep to aura-types/ipc.rs
```rust
// In aura-types/src/ipc.rs
pub enum DaemonToNeocortex {
    ContextPackage(ContextPackage),
    ReActObservation(ReActObservation),  // ADD THIS
    Embed(EmbedRequest),
}

pub enum NeocortexToDaemon {
    ReActStep(ReActStep),               // ADD THIS
    FinalResponse(FinalResponse),
    EmbeddingResult(EmbeddingResult),
}

pub struct ReActObservation {
    pub request_id: u64,
    pub step_index: u32,
    pub screen_state: ScreenState,      // REAL screen captured after action
    pub action_result: ActionResult,    // success/failure + any output
}

pub struct ReActStep {
    pub request_id: u64,
    pub thought: String,
    pub action: String,
    pub action_input: serde_json::Value,
    pub is_final: bool,                 // true = this is FinalResponse
}
```

### Step 5.2: Remove simulate_action_result() calls in daemon_core/react.rs
Lines ~1662 and ~1779: replace simulation with real IPC round-trip:
1. Receive `ReActStep` from neocortex
2. Execute action on device  
3. Capture real screen state (via ScreenReader)
4. Send `ReActObservation` back to neocortex
5. Receive next `ReActStep` or `FinalResponse`

### Step 5.3: Implement handler in neocortex ipc_handler.rs
Read `aura-neocortex/src/ipc_handler.rs` — add handler for `ReActObservation`:
- Receives observation
- Feeds it back into inference loop as next input
- Continues until `FinalResponse`

### Step 5.4: Implement Embed handler
The `DaemonToNeocortex::Embed` variant has no handler — implement it or mark dead code clearly.

### Step 5.5: Write 10+ ReAct loop tests
- Single-step action: think → act → observe → respond
- Multi-step: think → act → observe → think → act → observe → respond  
- Action failure: step fails → retry or graceful escalation
- Loop timeout: after N steps without FinalResponse, escalate

**Cargo check + cargo test. Commit:** `feat: close ReAct loop with real IPC observation rounds`

---

## Phase 6: Dreaming Consolidation + Memory Fixes

### Step 6.1: Implement dreaming consolidation in arc/learning/dreaming.rs
Per architecture spec §6:
1. Trigger: device charging + idle + 2am-5am window
2. Pull recent episodic memories (since last consolidation timestamp)
3. Build a consolidation ContextPackage: episodes as text, prompt = "Distill key facts about this user"
4. Send to neocortex via IPC
5. Write LLM-returned facts to semantic memory (HNSW)
6. Mark episodic entries as `consolidated = true`
7. Prune ETG entries with < 10% success rate

### Step 6.2: Fix episodic O(n) scan bug
Read `memory/episodic.rs` — find the linear scan code.
Add HNSW index or at minimum a sorted index by timestamp for range queries.
Target: < 50ms query with 10,000+ memories.

### Step 6.3: Fix archive compression
Read `memory/archive.rs` — find the fake RLE implementation.
Replace with real LZ4 compression (use `lz4_flex` crate or `flate2`).

### Step 6.4: Wire dreaming into main_loop periodic timer
Add check: `if platform.is_charging() && platform.is_idle() && is_overnight_window() { dreaming.run().await; }`

**Cargo check + cargo test. Commit:** `feat: implement dreaming consolidation, fix episodic scan, real archive compression`

---

## Phase 7: Quality Gates

### Step 7.1: Fix all 47 compiler warnings
Run `cargo check --workspace 2>&1 | grep warning`. Fix each:
- Unused imports: remove
- Dead code: add `#[allow(dead_code)]` with `// TODO(wire): Phase N` comment or actually wire/remove
- Unreachable patterns: fix

### Step 7.2: Eliminate critical unwrap() calls in hot paths
Run: `grep -rn "\.unwrap()" crates/ --include="*.rs"` 
Priority hot paths: `daemon_core/react.rs`, `ipc/`, `execution/executor.rs`, `memory/`
Replace with `?`, `.unwrap_or_default()`, or explicit error logging.

### Step 7.3: Write 50+ integration tests
File: `src/integration_tests.rs`
Scenarios to cover:
- Day-zero boot: no memories, OCEAN all 0.5, first request routes to S2, response delivered
- S1 cache hit: ETG has matching flow, executes directly without LLM
- S2 full loop: LLM receives ContextPackage, returns ReActStep, action executed, observation returned
- PolicyGate blocks: banking action denied, user notified
- Ethics check: response modified by ethics checker
- Memory write: episodic memory grows after interaction
- Dreaming: consolidation runs, semantic facts written
- Telegram delivery: response routed through telegram bridge

### Step 7.4: Verify relationship.rs negativity bias
Per architecture spec: current bias is 1.5× but literature supports 2-3×.
Read `identity/relationship.rs` — find the negativity bias constant.
Update to 2.0× (conservative midpoint) with doc comment citing literature.

**Cargo check + cargo test (must be 0 errors, 0 failures). Commit:** `quality: fix 47 warnings, eliminate hot-path unwraps, add 50+ integration tests`

---

## Phase 8: Android Build

### Step 8.1: Verify Cargo.toml Android targets
Check `Cargo.toml` and `.cargo/config.toml` for `aarch64-linux-android` target configuration.
Verify NDK path is configured.

### Step 8.2: Attempt cross-compilation
```
cargo build --target aarch64-linux-android --release 2>&1
```
Fix any compilation errors that only surface on Android target.

### Step 8.3: Verify JNI stubs
Read `platform/jni_bridge.rs` — all JNI functions should have `// TODO(jni): Phase N wire-point` comments.
None should panic() in production.

### Step 8.4: Verify android/ directory
Read `android/` directory — check for correct Gradle setup, permissions in AndroidManifest.xml (Accessibility, Notification, Battery, etc.)

### Step 8.5: Day-zero boot path verification
Trace the code path:
1. `daemon_core/startup.rs` — 8-phase init — confirm all phases instantiate correctly with empty state
2. `ipc/spawn.rs` — neocortex process spawned correctly
3. `ipc/client.rs` — connection to neocortex established
4. First user message → parser → contextor → router → S2 (empty ETG) → ContextPackage built → IPC to neocortex → response → delivery

Write a manual trace test that can be run without an Android device.

**Commit:** `feat: Android build verified, day-zero boot path traced`

---

## Phase 9: Final Validation

### Step 9.1: Full test suite
```
cargo test --workspace --release 2>&1
```
Must be 0 failures. Target: 2100+ tests.

### Step 9.2: Cargo check zero warnings
```
cargo check --workspace 2>&1 | grep -c warning
```
Target: 0 warnings.

### Step 9.3: Run clippy
```
cargo clippy --workspace 2>&1
```
Fix all warnings.

### Step 9.4: Verify quality gates from master plan
All 13 gates from AURA-V4-MASTER-PLAN.md §5.1:
- [ ] Compiles clean ✅ (was already passing)
- [ ] Tests pass ✅ (was already passing)
- [ ] Clippy clean
- [ ] PolicyGate wired (Phase 3)
- [ ] Ethics wired (Phase 3)
- [ ] Affective wired (Phase 4)
- [ ] Goals wired (Phase 4)
- [ ] Proactive wired (Phase 4)
- [ ] Dreaming works (Phase 6)
- [ ] Memory fixed (Phase 6)
- [ ] Integration tests 50+ (Phase 7)
- [ ] OOM never kills (platform/ already partial)
- [ ] Thermal never kills (platform/ already partial)

**Final commit:** `chore: AURA v4 all quality gates passing — ready for real-world deployment`

---

## Execution Rules (Non-Negotiable)

1. **Verify before claiming done** — run cargo check after every change
2. **Read before editing** — never edit a file you haven't read this session
3. **Backup before risky changes** — `checkpoints/` for anything > 50 lines changed
4. **One phase at a time** — do not start Phase N+1 until Phase N is verified clean
5. **Report evidence** — show actual cargo check output, not "it passed"
6. **No Theater AGI** — if you introduce a Rust function that makes behavioral decisions from user content, you've broken the architecture
7. **No stubs that mask bugs** — `todo!()` is allowed only with architectural comment explaining why it's a future phase wire-point

---

## File Reference (All 166 Rust files)

```
crates/aura-daemon/src/
  arc/cron.rs, routines.rs, mod.rs
  arc/health/{fitness,medication,mod,sleep,vitals}.rs
  arc/learning/{interests,mod,skills}.rs
  arc/life_arc/{financial,growth,health_arc,mod,primitives,relationships}.rs
  arc/proactive/{attention,mod,morning,routines,suggestions,welcome}.rs
  arc/social/{birthday,contacts,gap,graph,health,importance,mod}.rs
  bridge/{mod,router,system_api,telegram_bridge,voice_bridge}.rs
  daemon_core/{calibration,channels,checkpoint,main_loop,mod,onboarding,proactive_dispatcher,react,shutdown,startup,tutorial}.rs
  execution/{cycle,etg,executor,mod,monitor,planner,react,retry,tools}.rs
  execution/learning/{mod,workflows}.rs
  extensions/{discovery,loader,mod,recipe}.rs
  goals/{conflicts,decomposer,mod,registry,scheduler,tracker}.rs
  health/{mod,monitor}.rs
  identity/{affective,anti_sycophancy,behavior_modifiers,epistemic,ethics,mod,personality,proactive_consent,prompt_personality,relationship,thinking_partner,user_profile}.rs
  ipc/{client,mod,protocol,spawn}.rs
  memory/{archive,compaction,consolidation,embeddings,episodic,export,feedback,hnsw,importance,mod,patterns,semantic,workflows,working}.rs
  persistence/{integrity,journal,mod,safe_mode,vault}.rs
  pipeline/{amygdala,contextor,entity,mod,parser,slots}.rs
  platform/{connectivity,doze,jni_bridge,mod,notifications,power,sensors,thermal}.rs
  policy/{audit,boundaries,emergency,gate,mod,rules,sandbox,wiring}.rs
  routing/{classifier,mod,system1,system2}.rs
  screen/{actions,anti_bot,cache,mod,reader,selector,tree,verifier}.rs
  telegram/{approval,audit,commands,dashboard,dialogue,mod,polling,queue,reqwest_backend,security,voice_handler}.rs
  telegram/handlers/{agency,ai,config,debug,memory,mod,security,system}.rs
  telemetry/{counters,mod,ring}.rs
  voice/{audio_io,biomarkers,call_handler,mod,modality_state_machine,personality_voice,signal_processing,stt,tts,vad,wake_word}.rs
  integration_tests.rs, lib.rs, outcome_bus.rs, policy_ethics_integration_tests.rs, reaction.rs

crates/aura-neocortex/src/
  {aura_config,context,grammar,inference,ipc_handler,main,model,model_capabilities,prompts,tool_format}.rs

crates/aura-types/src/
  {actions,config,dsl,errors,etg,events,extensions,goals,identity,ipc,lib,manifest,memory,outcome,power,screen,tools}.rs

crates/aura-llama-sys/src/
  {gguf_meta,lib}.rs
```

---

*Plan created March 12, 2026. Executor: direct implementation — no agent delegation for execution steps.*
