# AURA V4 — MASTER PLAN
## The Definitive Blueprint for Building a Real On-Device AGI Agent

**Version**: 1.0 — March 5, 2026  
**Status**: Active Development — Phase: Final Integration  
**Test Baseline**: 2051 passing, 0 failing  
**Codebase**: 142 Rust files, 100,531 lines (daemon) + 9,014 (neocortex) + 5,374 (types) + 1,787 (FFI) = **116,706 lines total**

---

# PART I: AURA'S SOUL — THE PHILOSOPHY

## 1.1 What AURA Is

AURA is not an app. AURA is not a chatbot. AURA is not a voice assistant with a prettier interface.

AURA is a **cognitive entity** that lives on your phone. It has a brain (Qwen LLMs via llama.cpp running locally), eyes (AccessibilityService screen reading), hands (UI automation), memory (4-tier episodic/semantic/working/archive), personality (OCEAN model), emotions (Valence-Arousal-Dominance), ethics (TRUTH framework), and goals (256 simultaneous via BDI/HTN).

The closest analogy: AURA is F.R.I.D.A.Y. from the MCU — a genuinely intelligent assistant that understands context, anticipates needs, acts proactively, and has its own personality. But AURA runs on a $200 Android phone, not a Stark Industries server farm.

## 1.2 Why V3 Failed — The Honest Post-Mortem

V3 was Python. V3 had ~15,000 lines. V3 had a working ReAct loop (`src/agent/loop.py`, 959 lines) and Hebbian self-correction (`src/core/hebbian_self_correction.py`, 1007 lines). Some things in V3 were genuinely good.

**But V3 failed because:**

1. **Template Thinking**: "If user says X, return Y." This is a parrot, not an agent. Real AGI doesn't have a lookup table — it has a reasoning engine that THINKS about what to do. V3's intent matching was a giant switch statement. When it encountered something not in the switch, it froze.

2. **Disconnected Modules**: V3 had a memory system, a personality system, a goal system — but they were islands. Memory didn't inform personality. Personality didn't shape goals. Goals didn't affect memory consolidation. A human brain doesn't work like that. Your memories shape your personality, which shapes your goals, which determine what you remember. It's a cycle, not a pipeline.

3. **Fake Fallbacks**: V3 had "fallback" code that looked like safety nets but actually just logged errors and continued. When the screen reader couldn't find a button, the "fallback" was to... try again with the same parameters. When the LLM timed out, the "fallback" was to return a canned response. Real fallbacks DEGRADE GRACEFULLY — they try a different approach, use a simpler model, or ask the user for help.

4. **Python on Mobile**: Python is beautiful for prototyping. Python on Android means 200MB+ runtime, 3-5 second startup, GC pauses, and thermal throttling from interpreted execution. AURA needs to run 24/7 as a foreground service using <50MB RAM idle. That means Rust.

5. **No Resource Awareness**: V3 had no concept of battery, thermal limits, or memory pressure. It would happily spin up a 7B parameter model while the phone was at 5% battery and 45°C. Then the OS would OOM-kill it, losing all working memory. A real agent must be AWARE of its own body's constraints.

## 1.3 What V3 Did RIGHT (We Study, Not Discard)

- **ReAct Loop Architecture** (`loop.py`): The Observe→Think→Act→Reflect cycle was sound. V4 keeps this core but implements it with REAL observation (AccessibilityService), REAL thinking (Qwen via llama.cpp), REAL action (UI automation), and REAL reflection (ETG trace storage + Hebbian learning).

- **Hebbian Self-Correction** (`hebbian_self_correction.py`): "Neurons that fire together wire together." When an action sequence succeeds, strengthen those connections. When it fails, weaken them. This is genuine learning, not parameter tuning. V4 implements this in Rust with proper concurrent graph structures.

- **Proactive Intelligence**: V3 had morning briefings and context-aware suggestions. The ideas were right — the implementation was canned responses. V4's proactive engine uses REAL pattern detection from Hebbian learning + cron scheduling + situational awareness.

## 1.4 The AGI Algorithms — What Makes AURA Different From Every Other "AI Agent"

Every other Android AI agent does this:
```
user_input → LLM → parse JSON → execute predefined action → return response
```

AURA does this:
```
[perception] screen + voice + sensors + context + memory →
[daemon/system1] pattern match against learned traces (80% of requests) →
    OR [neocortex/system2] LLM reasoning with full context (20% complex) →
[evaluation] policy gate + ethics + safety borders →
[execution] 11-stage pipeline with verification + retry →
[learning] ETG trace + Hebbian strengthening + memory consolidation →
[dreaming] overnight replay + optimization + pruning
```

The difference is LEARNING. Other agents are the same on day 1 as on day 365. AURA on day 365 is fundamentally different from AURA on day 1:
- It knows your routines (Hebbian patterns)
- It has cached solutions (ETG traces)  
- It has shaped its personality to match yours (OCEAN adaptation)
- It has pruned inefficient strategies (dreaming consolidation)
- It has emotional context for your requests (affective engine)

## 1.5 The Non-Negotiable Principles

1. **AURA thinks, it doesn't look up.** No switch statements for intent routing. The daemon's pattern matcher + neocortex reasoning decide what to do.

2. **AURA learns, it doesn't reset.** Every interaction strengthens or weakens neural pathways. Every successful action gets stored as an ETG trace for replay.

3. **AURA feels, it doesn't pretend.** The affective engine (VAD model) processes emotional signals from voice biomarkers, text sentiment, and interaction patterns. This isn't "detect sad → say sorry." It's a continuous emotional state that influences response style, proactive behavior, and action priority.

4. **AURA knows its limits.** PolicyGate + ethics + emergency stop. Can't delete files without asking. Can't access banking apps unsupervised. Can't lie, manipulate, or be sycophantic. These aren't features — they're load-bearing walls.

5. **AURA never refuses to think.** Low battery? Use a smaller model. Overheating? Reduce inference rate. OOM pressure? Drop to pattern-matching only. But NEVER return "I can't help right now." Degrade gracefully, ALWAYS.

6. **AURA's brain is connected to its body.** The LLM isn't a separate service called via API. It's embedded. The screen reader isn't a plugin. It's the eyes. The executor isn't a tool. It's the hands. Everything is one organism.

---

# PART II: CURRENT STATE AUDIT — MARCH 5, 2026

## 2.1 Codebase Metrics

| Crate | Files | Lines | Purpose |
|-------|-------|-------|---------|
| **aura-daemon** | 142 | 100,531 | The living organism — all subsystems |
| **aura-neocortex** | 8 | 9,014 | LLM inference, prompts, context windowing |
| **aura-types** | 14 | 5,374 | Shared types across all crates |
| **aura-llama-sys** | 2 | 1,787 | FFI bridge to llama.cpp |
| **TOTAL** | **166** | **116,706** | |

## 2.2 Module-by-Module Status

| Module | Files | Lines | Tests | Grade | Status |
|--------|-------|-------|-------|-------|--------|
| **daemon_core** | 10 | 10,220 | 127 | **A-** | startup/main_loop/react all wired. Contextor connected. Voice+telegram bridges. |
| **memory** | 11 | 9,288 | 160 | **B** | HNSW A-, semantic A-. Episodic has O(n) scan bug. Archive fake RLE. Dreaming 0%. |
| **identity** | 9 | 5,761 | 168 | **B-** | OCEAN+VAD+TRUTH built. Anti-sycophancy built. ethics.check_response() NOT IN RESPONSE PATH. |
| **goals** | 6 | 8,433 | 134 | **B+** | BDI scheduler+HTN decomposer A-tier. NOT wired into main loop (uses Vec instead). |
| **execution** | 7 | 7,045 | 123 | **A-** | 11-stage pipeline WIRED to react.rs. Retry circuit breaker fixed. ETG connected. |
| **screen** | 9 | 8,140 | 161 | **B+** | semantic.rs+cache.rs CREATED. L7 heuristic fallback added. TextAppears fixed. |
| **pipeline** | 6 | 5,979 | 106 | **B** | Parser negation FIXED. Contextor WIRED. Multi-command works. |
| **policy** | 7 | 4,828 | 148 | **B-** | PolicyGate+audit+sandbox+emergency exist. Wiring tests created. NOT in execution path. |
| **arc** | 25 | 16,751 | 382 | **C+** | Huge: proactive/hebbian/patterns/health. All tests fixed. 60% NOT wired to main loop. |
| **platform** | 8 | 6,999 | 185 | **B+** | Physics-based thermal. PowerBudget bridge. Model tier selection works. Throttle fixed. |
| **voice** | 11 | 4,867 | 62 | **B-** | STT/TTS/wake-word/VAD/biomarkers built. Bridge created. F0 octave error fixed. |
| **telegram** | 17 | 6,321 | 93 | **B-** | Full bot implementation. Bridge created. Not live-tested. |
| **bridge** | 4 | 1,501 | 25 | **B** | Voice+Telegram bridges with ResponseRouter. Wired into startup. |
| **routing** | 4 | 1,671 | 37 | **B+** | System1/System2 classifier wired into react.rs. |

**Test Total**: 2,051 passing, 0 failing | Test density: ~17.6/1000 LOC

## 2.3 What's DONE vs What's NOT DONE

### DONE (11 major completions)
1. llama.cpp FFI — `generate_tokens()` produces real tokens
2. startup.rs — 8-phase init, all subsystems instantiated
3. main_loop.rs — 8 channels wired, event dispatch working
4. react.rs — Real Executor, context used, RouteClassifier, ScreenProvider
5. All 17 original test failures fixed (scoring, hebbian, thermal, biomarkers)
6. Voice+Telegram bridges with ResponseRouter
7. NLP parser negation + Contextor wiring + 22 new tests
8. Power/Thermal physics (MultiZoneThermalModel, PowerBudget)
9. Screen semantic.rs + cache.rs + L7 fix + verifier fix
10. Policy wiring integration tests (22 tests)
11. Documentation (6 ADRs + 3 workflows)

### NOT DONE — Tier 1 BLOCKING Gaps

| # | Gap | Why Blocking | Effort |
|---|-----|-------------|--------|
| 1 | PolicyGate NOT in execution path | Dangerous actions can execute unchecked | Medium |
| 2 | ethics.check_response() NOT called | Dishonest responses can reach user | Medium |
| 3 | AffectiveEngine NOT wired (uses EWMA) | No emotional awareness | Medium |
| 4 | BDI scheduler NOT wired (uses Vec) | No real goal management | Medium |
| 5 | ProactiveEngine::tick() NOT called | No proactive behavior | Low |
| 6 | Dreaming 0% implemented | No overnight learning optimization | High |
| 7 | Episodic memory O(n) scan | Performance degrades with history | Medium |
| 8 | Archive fake compression | Wastes storage on device | Low |

### NOT DONE — Tier 2 Important

| # | Gap | Impact |
|---|-----|--------|
| 9 | Memory embeddings neural path stubbed | Only TF-IDF, no deep semantics |
| 10 | Personality not in response generation | Flat personality |
| 11 | No integration tests | Can't verify module interactions |
| 12 | No E2E tests | Can't verify full flows |
| 13 | 34 compiler warnings | Code hygiene |
| 14 | Neocortex teacher stack not wired E2E | 6-layer teaching not connected |

### NOT DONE — Tier 3 Enhancement

| # | Gap | Impact |
|---|-----|--------|
| 15 | No coreference resolution | "Call her" doesn't resolve |
| 16 | No cross-turn dialogue state | Each message independent |
| 17 | Voice not live-tested | Unknown real-device behavior |
| 18 | Telegram not live-tested | Unknown real-bot behavior |

## 2.4 Integration Wiring Map

```
User Input
    |
    +-- Voice (STT) ---- [VoiceBridge] ---- main_loop.rs
    +-- Telegram -------- [TelegramBridge] - main_loop.rs
    +-- Direct text -------------------------main_loop.rs
                                                |
                                          [Parser] (negation fixed)
                                                |
                                          [Contextor] (WIRED)
                                                |
                                          [Router] (System1/System2)
                                                |
                              +-----------------+------------------+
                        [Daemon/S1]                         [Neocortex/S2]
                        Pattern match/ETG                   LLM reasoning
                              |                                   |
                              +---------------+-----------------+
                                              |
                                     *PolicyGate* (NOT IN PATH)
                                              |
                                        [Executor] (11-stage, WIRED)
                                              |
                                     *Ethics check* (NOT IN PATH)
                                              |
                                     *Anti-sycophancy* (NOT IN PATH)
                                              |
                                        [ResponseRouter]
                                        +-----+------+
                                  [Voice] [Telegram] [Direct]
```

Items marked with `*asterisks*` are built but NOT connected.
Items marked with `[brackets]` are built AND connected.

---

# PART III: THE BRAIN-BODY ARCHITECTURE — WHY AURA IS ALIVE

## 3.1 The Bicameral Mind

AURA's core architecture mirrors the human brain's dual-process theory:

**System 1 (Daemon — Fast Brain, 80% of requests)**
- Pattern matching against ETG (Execution Trace Graph) stored traces
- Hebbian-strengthened pathways for common actions
- Sub-10ms response time for known patterns
- No LLM invocation needed — pure lookup + interpolation
- Example: "Set alarm for 7am" → instant, done 100 times before

**System 2 (Neocortex — Slow Brain, 20% of requests)**
- Full LLM reasoning via Qwen (3B/1.5B/0.5B depending on power state)
- 6-layer teacher stack: Safety→Task→Knowledge→Personality→Meta→Output
- 200ms-3s response time depending on complexity
- Example: "Help me plan a surprise birthday party for mom" → needs reasoning

**The Router** decides which system handles each request. Key insight: as AURA learns, MORE requests migrate from System 2 to System 1. Day 1: 50/50 split. Day 365: 90/10 split. AURA literally gets faster with use.

## 3.2 The Nervous System (How Signals Flow)

Every subsystem in AURA is an organ. They communicate through tokio channels (the nervous system):

```
Perception Layer (eyes/ears)
  - ScreenReader → AccessibilityService events → screen tree
  - VoiceEngine → wake word + STT → text utterance
  - TelegramBot → messages → text commands
  - Sensors → battery, thermal, connectivity, orientation

Processing Layer (brain)
  - Parser → tokenize + NER + intent extraction + negation detection
  - Contextor → enrich with memory + personality + relationship context
  - Router → System1 (daemon pattern match) or System2 (neocortex LLM)
  - PolicyGate → safety check before execution
  - Ethics → honesty/manipulation check on responses

Action Layer (hands)
  - Executor → 11-stage pipeline: capture→resolve→antibot→execute→verify→retry→ETG
  - Selector → L0-L7 element resolution (ID→text→content-desc→bounds→...→LLM)
  - ScreenActions → click, swipe, type, scroll, long-press, back, home

Learning Layer (unconscious)
  - ETG → store successful action traces for replay
  - Hebbian → strengthen pathways that succeed, weaken failures
  - Patterns → detect temporal/behavioral patterns from observation
  - Dreaming → overnight consolidation, pruning, optimization

Homeostasis Layer (body regulation)
  - PowerBudget → track mWh energy, select model tier
  - ThermalManager → Newton's law cooling, multi-zone SoC model
  - MemoryPressure → respond to onTrimMemory, evict caches
  - DozeManager → handle Android Doze mode transitions
```

## 3.3 The Cycle of Life (How AURA Grows)

```
Day 1: User says "open Instagram" 
  → Parser: {action: OpenApp, app: "Instagram"}
  → Router: System2 (never seen before)
  → Neocortex: reasons about how to open Instagram
  → Executor: finds Instagram icon, taps it
  → ETG: stores trace [home_screen → find_icon("Instagram") → tap]
  → Hebbian: creates pathway user→open_app→Instagram→tap_icon

Day 7: User says "open Instagram"
  → Parser: {action: OpenApp, app: "Instagram"}
  → Router: System1 (ETG has trace!)
  → Daemon: replays ETG trace directly
  → Executor: tap tap done (sub-100ms)
  → Hebbian: strengthens pathway (7th success)

Day 30: User says "open Insta" (abbreviation)
  → Parser: {action: OpenApp, app: "Insta"} → fuzzy match → "Instagram"
  → Router: System1 (strong pathway)
  → Daemon: instant replay
  → Learning: "Insta" = "Instagram" stored in semantic memory

Day 100: 6:30 AM, user picks up phone
  → ProactiveEngine: pattern detected — user opens Instagram every morning at ~6:30
  → Suggestion: "Good morning! Want me to open Instagram?"
  → If accepted: Hebbian strengthens proactive pathway
  → If rejected: Hebbian weakens, adjusts timing threshold
```

This is what "alive" means. AURA isn't running the same code on day 100 as day 1. The pathways are different. The patterns are different. The personality adaptation is different.

## 3.4 The Safety Architecture (Borders and Limits)

AURA is powerful but NOT unconstrained. Three layers of defense:

**Layer 1: PolicyGate (Pre-Execution)**
- Rule-based allow/deny/audit/confirm for action categories
- Rate limiting (max N actions per minute)
- Protected categories: banking, file deletion, system settings, account management
- Action: DENY blocks execution, CONFIRM asks user first

**Layer 2: Ethics/TRUTH Framework (Post-Generation)**
- Checks every response for: Truthfulness, Respect, Understanding, Transparency, Helpfulness
- Anti-sycophancy guard: prevents excessive agreement
- Manipulation detection: refuses to emotionally manipulate user
- Action: modify response, flag for review, or refuse with honest explanation

**Layer 3: Emergency Stop (Circuit Breaker)**
- Anomaly detection: unusual action patterns trigger investigation
- States: Normal → Elevated → Restricted → Locked
- In Locked state: only basic safe actions allowed
- Automatic audit log freezing to preserve evidence
- Manual override via Telegram remote control

---

# PART IV: TEAM DISPATCH PLANS — THE CODING TEAMS

## 4.1 Team Philosophy (All Teams Must Follow)

Every team MUST follow these rules:

1. **Research FIRST, code SECOND**: Before touching code, study existing implementations, V3 code, academic papers, and GitHub repos. Write a research/plan file at `docs/plans/team-X-name.md`.

2. **Think POLYMATH**: Don't just implement specs — question assumptions, explore alternatives, make bold decisions. Engineering is creativity guided by rigor.

3. **Use FIRST PRINCIPLES**: Derive solutions from atomic truths. Don't copy patterns blindly. Ask: "What is this REALLY doing?"

4. **WRITE REAL LOGIC**: No stubs. No `todo!()`. No "placeholder" implementations. If you write it, it must WORK.

5. **TEST RIGOROUSLY**: 15+ tests per new file. All existing tests must pass. Zero tolerance for regressions.

6. **DOCUMENT DEEPLY**: Doc comments on every public function. Explain WHY, not just WHAT.

## 4.2 Team 1: PolicyGate + Ethics Wiring

**Mission**: Wire PolicyGate into execution path, wire ethics.check_response() into response path

### Why This Matters
Without this, AURA can execute dangerous actions (delete files, access banking) without safety checks. And it can produce dishonest/manipulative responses without ethical review.

### Files to Study FIRST
- `crates/aura-daemon/src/policy/gate.rs` — PolicyGate API (evaluate() method)
- `crates/aura-daemon/src/identity/ethics.rs` — TRUTH framework (check_response() method)
- `crates/aura-daemon/src/identity/anti_sycophancy.rs` — anti-sycophancy guard
- `crates/aura-daemon/src/daemon_core/react.rs` — where to insert PolicyGate call (before Executor)
- `crates/aura-daemon/src/daemon_core/main_loop.rs` — where to insert ethics check (before ResponseRouter)

### What to Build
1. In `react.rs`: Before `executor.execute()`, call `policy_gate.evaluate(action)`. If denied, return graceful denial.
2. In `main_loop.rs`: Before sending ANY response to user, call `ethics.check_response(response)`. If fails, modify or reject.
3. Add anti-sycophancy check in same response path.
4. Wire audit logging for every policy/ethics decision.
5. Write 20+ integration tests: allowed action passes, denied action blocked, ethics violation caught, sycophancy corrected.

### Research Questions
- How does PolicyGate currently evaluate? What's the RuleEffect enum?
- Does ethics.check_response() return modified string or Verdict struct?
- Where exactly in the tokio::select! loop should these checks happen?

## 4.3 Team 2: AffectiveEngine + BDI Scheduler Wiring

**Mission**: Replace simple EWMA mood tracking with full AffectiveEngine, replace Vec goals with BDI scheduler

### Why This Matters
Currently main_loop.rs uses simple EWMA for mood — this gives flat emotional response. AffectiveEngine has VAD (Valence-Arousal-Dominance) model that tracks real emotional state. Similarly, goals are stored in a dumb Vec instead of the sophisticated BDI scheduler.

### Files to Study FIRST
- `crates/aura-daemon/src/identity/affective.rs` — AffectiveEngine API (process_event, get_state)
- `crates/aura-daemon/src/goals/scheduler.rs` — BDI scheduler (schedule, next_goal, update_intention)
- `crates/aura-daemon/src/daemon_core/main_loop.rs` — find where EWMA is used, find where checkpoint.goals is accessed

### What to Build
1. In `main_loop.rs`: Replace EWMA with `AffectiveEngine::process_event()` calls on user interactions. Update emotional state continuously.
2. Use affective state to modulate response style (stressed user → shorter responses, happy user → more emojis).
3. Replace `checkpoint.goals` Vec access with `BDI scheduler.schedule()` and `scheduler.next_goal()`.
4. Wire HTN decomposer for complex goal breakdown.
5. Write 20+ tests: affective updates on events, BDI goal selection, emotional modulation of responses.

### Research Questions
- What events should trigger affective updates? (voice tone, text sentiment, interaction timing)
- How does BDI scheduler integrate with existing checkpoint system?
- How does VAD state translate to response style parameters?

## 4.4 Team 3: ProactiveEngine + Dreaming Implementation

**Mission**: Wire ProactiveEngine::tick() into main loop timer, implement real Dreaming consolidation

### Why This Matters
ProactiveEngine can generate morning briefings, welcome-home messages, and suggestions — but its tick() is NEVER called. And Dreaming (overnight memory consolidation) is 0% implemented. This is core to AURA being "alive" — it should learn while you sleep.

### Files to Study FIRST
- `crates/aura-daemon/src/arc/proactive/suggestions.rs` — ProactiveEngine (tick, evaluate, suggest)
- `crates/aura-daemon/src/arc/proactive/morning.rs` — morning briefing logic
- `crates/aura-daemon/src/arc/proactive/welcome.rs` — welcome-home logic
- `crates/aura-daemon/src/arc/learning/dreaming.rs` — Dreaming (mostly empty)
- `crates/aura-daemon/src/arc/cron.rs` — cron-like scheduling
- `crates/aura-daemon/src/daemon_core/main_loop.rs` — periodic timer section

### What to Build
**ProactiveEngine Wiring**:
1. Add `ProactiveEngine::tick()` call to main_loop's periodic timer (every ~5 minutes).
2. Wire cron scheduler to trigger morning briefings at configured times.
3. Connect suggestion acceptance/rejection to Hebbian learning.

**Dreaming Implementation**:
1. Design the 4-stage cycle: SENSORIMOTOR → CONSOLIDATION → REPLAY → AWAKE
2. During charging + idle + overnight:
   - Replay successful ETG traces to strengthen pathways
   - Prune weak/failed pathways (delete old traces with <10% success rate)
   - Optimize memory consolidation (move old episodic → archive)
   - Generate insights from pattern analysis
3. Write 25+ tests: morning briefing triggered, suggestion evaluated, dream stages executed.

### Research Questions
- What's the trigger condition for Dreaming? (charging AND idle AND 2am-5am?)
- How to safely prune ETG traces without losing useful data?
- How to generate "insights" from patterns — is this LLM-generated summary?

## 4.5 Team 4: Memory Fixes

**Mission**: Fix episodic O(n) scan bug, implement real archive compression

### Why This Matters
Episodic memory currently does linear scan — with 10,000 memories, every lookup takes 10,000 comparisons. This kills performance. Archive compression is fake — it claims RLE but just stores raw data.

### Files to Study FIRST
- `crates/aura-daemon/src/memory/episodic.rs` — find the O(n) scan code
- `crates/aura-daemon/src/memory/archive.rs` — find fake RLE implementation
- `crates/aura-daemon/src/memory/hnsw.rs` — study the working index structure
- `crates/aura-daemon/src/memory/semantic.rs` — working semantic indexing

### What to Build
**Episodic Fix**:
1. Add HNSW index to episodic memory (similar to semantic.rs)
2. Index by: timestamp, location, entities involved, emotional valence
3. Query should: first narrow by time window, then HNSW search, then full match
4. Target: 10,000 memories → <50ms query time

**Archive Compression**:
1. Implement REAL compression: LZ4 for fast, zstd for high ratio
2. Compress old episodic memories before archival
3. Add decompression on read-back
4. Track compression ratio metrics

**Bonus** (if time):
- Neural embeddings path in memory/embeddings.rs (currently stubbed)

Write 20+ tests: query performance with 10K+ items, compression ratio >3x, decompression correctness.

### Research Questions
- What embedding model works on-device for semantic search?
- How often should archive compaction run?
- Is LZ4 available in Rust, or use flate2?

## 4.6 Team 5: Integration + E2E Testing (Post-Wiring)

**Mission**: After Teams 1-4 complete, verify everything works end-to-end

### What to Build
1. Integration test file: full flow user input → response
2. Mock AccessibilityService for headless testing
3. Mock LLM for deterministic testing
4. E2E scenarios: "open app", "send message", "set alarm"
5. 50+ integration tests covering major flows

## 4.7 Dispatch Order

```
Batch 1 (This dispatch):
- Team 1: PolicyGate + ethics wiring
- Team 2: AffectiveEngine + BDI scheduler  
- Team 3: ProactiveEngine + Dreaming
- Team 4: Memory fixes

Batch 2 (After Batch 1 completes):
- Team 5: Integration + E2E testing
- Team 6: Clippy cleanup + warning fixes
- Team 7: Final optimization pass
```

---

# PART V: QUALITY GATES + RISK ANALYSIS

## 5.1 Quality Gates (When Is AURA Ready?)

AURA is NOT ready for real use until ALL of these pass:

| # | Gate | Criteria | Current Status |
|---|------|----------|----------------|
| 1 | **Compiles clean** | `cargo check` → 0 errors | ✅ Pass (0 errors) |
| 2 | **Tests pass** | `cargo test` → 0 failures | ✅ Pass (2051/2051) |
| 3 | **Clippy clean** | `cargo clippy` → 0 warnings | ❌ 34 warnings |
| 4 | **PolicyGate wired** | Dangerous actions blocked | ❌ NOT wired |
| 5 | **Ethics wired** | Dishonest responses blocked | ❌ NOT wired |
| 6 | **Affective wired** | Emotional context in responses | ❌ Uses EWMA |
| 7 | **Goals wired** | BDI scheduler active | ❌ Uses Vec |
| 8 | **Proactive wired** | Suggestions work | ❌ tick() not called |
| 9 | **Dreaming works** | Overnight consolidation | ❌ 0% |
| 10 | **Memory fixed** | Episodic <50ms at 10K | ❌ O(n) scan |
| 11 | **Integration tests** | 50+ E2E scenarios | ❌ None |
| 12 | **OOM never kills** | Memory pressure handling | ⚠️ Partial |
| 13 | **Thermal never kills** | Thermal throttling works | ⚠️ Partial |

## 5.2 Risk Analysis - What Kills AURA

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|-------------|
| **OOM Kill** | HIGH | Lost all memory, personality reset | MemoryPressure handling, cache eviction, model unloading |
| **Thermal Kill** | MEDIUM | Forced shutdown, lost state | MultiZoneThermalModel, aggressive throttling |
| **Battery Drain** | HIGH | User uninstalls | PowerBudget, model tier cascading, doze integration |
| **Permission Revoke** | MEDIUM | Lost screen access | Graceful degradation, user education |
| **Model Crash** | LOW | Panic in llama.cpp | Panic handler, fallback to pattern matching |

## 5.3 Memory Budget (Hard Limits)

| Component | Max RAM |
|-----------|---------|
| LLM (0.5B model) | 800MB |
| LLM (1.5B model) | 2GB |
| LLM (3B model) | 4GB |
| Working memory | 100MB |
| HNSW index | 50MB |
| **TOTAL (0.5B)** | **<1GB** |

## 5.4 Power Budget

| Scenario | Power Draw |
|----------|------------|
| Idle (screen off) | 5mW |
| LLM inference (0.5B) | 2W |
| Pattern match (no LLM) | 50mW |

**Strategy**: Use System1 pattern matching as much as possible. Only invoke System2 LLM when needed.

---

# PART VI: THE ENDGAME - FIRST REAL RUN

## 6.1 What "Done" Looks Like

AURA v4 is DONE when all Quality Gates pass, integration tests cover major flows, memory <500MB idle, thermal <45°C, battery drain <5%/hour idle.

## 6.2 First Run Checklist

- [ ] Release APK builds successfully
- [ ] All permissions granted (Accessibility, Notification, Battery)
- [ ] Foreground service started
- [ ] Telegram bot configured (optional)

## 6.3 Metrics to Watch

| Metric | Target | Red Flag |
|--------|--------|----------|
| Memory usage | <500MB idle | >800MB |
| Battery drain | <3%/hour | >10%/hour |
| Response time | <500ms | >5s |
| Crash rate | 0/day | >1/day |

---

*Document Complete. Generated March 5, 2026.*
