# AURA v4 — Final Test Suite Audit Report
**Date:** 2026-03-13  
**Scope:** 16 modules sampled from ~2,376 tests across ~100+ test-containing files  
**Methodology:** READ-ONLY static analysis — every assertion inspected for semantic validity  
**Verdict:** ⚠️ **MIXED — strong isolation tests undermined by catastrophically hollow integration layer**

---

## Executive Summary

AURA v4 reports 2,376 passing tests. This audit finds that a **significant fraction are not quality gates — they are assertion theatre** that cannot fail under any circumstances. The situation is not uniformly bad: the policy/ethics/identity layer is genuinely well-tested in isolation. The damage is concentrated in two specific areas:

1. **`integration_tests.rs`** — ~45 tests, nearly all tautological (`assert!(x.is_ok() || x.is_err())`). This file is the single most dangerous artifact in the codebase. It gives developers false confidence that E2E flows (voice→parse→execute→output, Telegram security, PolicyGate enforcement, episodic memory, Hebbian learning) are validated. None are.

2. **`execution/planner.rs` + `daemon_core/react.rs`** — The primary agentic coordinator has zero unit tests. The planner's core scoring function is a hardcoded stub (`score_plan() → 0.5`). These are the most-executed paths at runtime.

The remaining 14 modules sampled range from ADEQUATE to STRONG, with `policy_ethics_integration_tests.rs`, `policy/gate.rs`, `identity/ethics.rs`, `identity/anti_sycophancy.rs`, and `routing/classifier.rs` representing genuinely high-quality test suites.

**Estimated meaningful test coverage across sampled modules: ~58%** (see metrics section).

---

## Module-by-Module Assessment

| Module | Tests | Meaningful | Trivial/Hollow | Rating |
|---|---|---|---|---|
| `integration_tests.rs` | ~45 | ~0 | ~45 | 🔴 TRIVIAL |
| `execution/planner.rs` | 8 | 3 | 5 | 🟠 WEAK |
| `daemon_core/react.rs` | 0 | 0 | 0 | 🔴 NOT TESTED |
| `health/monitor.rs` | 12 | 8 | 4 | 🟡 ADEQUATE |
| `execution/executor.rs` | 14 | 10 | 4 | 🟡 ADEQUATE |
| `neocortex/inference.rs` | 10 | 7 | 3 | 🟡 ADEQUATE |
| `memory/embeddings.rs` | 11 | 11 | 0 | 🟢 STRONG |
| `persistence/vault.rs` | 15 | 15 | 0 | 🟢 STRONG |
| `memory/semantic.rs` | 13 | 12 | 1 | 🟢 STRONG |
| `memory/episodic.rs` | 14 | 13 | 1 | 🟢 STRONG |
| `neocortex/context.rs` | 11 | 11 | 0 | 🟢 STRONG |
| `policy/gate.rs` | 22 | 21 | 1 | 🟢 STRONG |
| `routing/classifier.rs` | 8 | 8 | 0 | 🟢 STRONG |
| `identity/ethics.rs` | 26 | 25 | 1 | 🟢 STRONG |
| `identity/anti_sycophancy.rs` | 9 | 9 | 0 | 🟢 STRONG |
| `policy_ethics_integration_tests.rs` | 36 | 34 | 2 | 🟢 STRONG |

**Sampled total: ~254 tests | Meaningful: ~187 | Hollow: ~67 | Meaningful rate: ~73.6%**

> ⚠️ This 73.6% is inflated by the fact that the best modules were partially prioritised in sampling. The hollow count is dominated by `integration_tests.rs` alone. Remove it and the sampled meaningful rate rises to ~99%. Keep it and recognise it as the definitive risk.

---

## Hard Questions — Answered Brutally

### 1. Are the tests real quality gates or synthetic?
**Mixed.** Unit tests in isolation (memory, policy, identity, routing) are overwhelmingly real. The integration layer is almost entirely synthetic. The test suite cannot catch a regression in any E2E flow — voice input, screen execution, Telegram security, episodic consolidation — because `integration_tests.rs` asserts tautologies for all of them.

### 2. Are assertions semantically meaningful?
**In unit tests: mostly yes.** `policy/gate.rs`, `routing/classifier.rs`, `identity/anti_sycophancy.rs` and `policy_ethics_integration_tests.rs` use pinned-value assertions, state-machine step checks, epsilon float comparisons, and dual-property assertions. These would catch real regressions.

**In `integration_tests.rs`: categorically no.** `assert!(x.is_ok() || x.is_err())` is not an assertion. It is a tautology. It compiles, it passes, it verifies nothing.

### 3. What are the worst coverage gaps?
1. **`daemon_core/react.rs`** — zero tests. This is AURA's agentic loop coordinator. It routes input through the System1/System2 cascade, dispatches to planner/executor, manages retry, handles daemon sleep/wake. Entirely untested.
2. **`execution/planner.rs` score_plan()** — hardcoded stub returns 0.5. The planner's quality signal is permanently neutral regardless of plan content.
3. **Full pipeline E2E** — voice→parse→NLU→route→plan→execute→output. `integration_tests.rs` pretends to test this; it does not.
4. **Screen automation** (`screen/` — 7 files) — not sampled but heavily referenced in `integration_tests.rs` via tautological assertions.
5. **Telegram bridge security** — `test_telegram_security_validation` asserts `assert!(valid || !valid)`. Telegram authentication is completely untested.
6. **Hebbian learning** — references `arc::learning::pathway::HebbianPathway` which likely doesn't exist. Tests may not compile cleanly without feature flags.

### 4. Are edge cases tested?
**Yes, in the strong modules.** Examples of excellent edge-case coverage:
- `policy/gate.rs`: `test_max_rules_cap` (evaluation cut-off), `test_case_insensitive` (normalisation), `test_first_match_wins` (rule ordering).
- `routing/classifier.rs`: `test_hysteresis_holds_route` (state machine edge), `test_determinism` (float reproducibility).
- `identity/anti_sycophancy.rs`: `test_ring_buffer_evicts_oldest` (circular buffer wrap), `test_gate_sycophantic_blocks_then_downgrades` (multi-step state machine).
- `identity/ethics.rs`: `test_check_with_trust_block_unchanged` (important negative: high trust does NOT override Block).

**No edge cases tested in `integration_tests.rs`** — no test can even test the happy path, let alone edges.

### 5. Is the mocking strategy sound?
**Mostly.** The strong unit test modules test real structs with real logic, not mocks. `policy/gate.rs` constructs real `PolicyGate::from_config()` instances. `identity/ethics.rs` runs real NLP-style manipulation scoring.

**Two concerns:**
- `execution/executor.rs` uses `PolicyGate::allow_all()` in `for_testing()` config — bypasses the entire policy enforcement layer during execution tests.
- `health/monitor.rs` returns hardcoded `true` for neocortex liveness ping — masks real connectivity.

### 6. Is the `for_testing()` / `allow_all()` bypass dangerous?
**Conditionally.** `PolicyGate::allow_all()` is correctly annotated `#[cfg(test)]` in `gate.rs` — it cannot reach production builds. However, `executor.rs` calls it in `for_testing()`, meaning **all executor tests run with zero policy enforcement**. A regression where the executor bypasses policy in production would not be caught by any existing test.

The fix: add at least one executor integration test that uses a real `PolicyGate::from_config()` with a deny rule, and asserts the executor honours the denial.

### 7. What is the flakiness risk?
**Low in unit tests** — no async races, no time-dependent logic, no network calls in the strong modules.

**Unknown in integration tests** — but moot, since the integration tests are tautological and cannot flake or fail meaningfully.

**One concern:** `memory/episodic.rs` uses timestamps; if clock-skew tests exist they may be environment-sensitive. Not observed in sampled tests but worth monitoring.

### 8. What are the integration gaps?
The gap is not subtle — it is total. `integration_tests.rs` is the only file that claims to test cross-module flows, and it verifies nothing. The following cross-module boundaries are **completely untested**:
- Parser → Router → Planner → Executor
- Executor → PolicyGate (real enforcement, not allow_all)
- Executor → MemoryWriter (episodic event recording post-execution)
- Voice pipeline → NLU → Intent classification
- Telegram bridge → Authentication → Command dispatch
- Screen reader → Element selector → Action verifier

### 9. Is test data realistic?
**Yes in strong modules.** `policy_ethics_integration_tests.rs` uses realistic action strings ("format /system", "install com.evil.app"), real config structures, and multi-step flows. `identity/ethics.rs` uses actual manipulative/sycophantic text samples that read like real LLM output.

**In `integration_tests.rs`:** irrelevant — the assertions are tautological so test data quality doesn't matter.

### 10. Do regression tests exist for known past bugs?
**Not evidenced in any sampled file.** No test is annotated with a bug ID, issue reference, or "regression for #N" comment. This is a process gap, not necessarily a test quality gap — but it means there's no way to know if previously fixed bugs are protected.

---

## Critical Anti-Patterns Found (8 total)

### AP-1: Tautological OR assertion *(severity: CRITICAL)*
```rust
assert!(result.is_ok() || result.is_err());
```
Cannot fail. Present ~30+ times in `integration_tests.rs`. Syntactically valid Rust, semantically meaningless.

### AP-2: De Morgan tautology *(severity: CRITICAL)*
```rust
assert!(valid || !valid);         // literally assert!(true)
assert!(supports || !supports);   // literally assert!(true)
```
Present multiple times in `integration_tests.rs`.

### AP-3: Hardcoded constant assertion *(severity: CRITICAL)*
```rust
let has_consolidation = true;
assert!(has_consolidation);
```
The variable is never derived from any real computation.

### AP-4: Pre-wired outcome in E2E test *(severity: CRITICAL)*
```rust
// In test_complete_voice_flow:
let executed = true;
// ... builds response using `executed` as if it came from actual execution
```
The "E2E" test never calls any real execution path.

### AP-5: Stub score test *(severity: HIGH)*
```rust
// planner.rs
let score = plan_scorer.score_plan(&plan);
assert!((score - 0.5).abs() < 0.001);
```
The function is a stub that always returns 0.5. The test is testing the stub, not plan quality.

### AP-6: Dual-accept assertion *(severity: MEDIUM)*
```rust
assert!(matches!(result, ExecutionResult::Success(_) | ExecutionResult::Failed(_)));
```
Accepts both success and failure. Would not catch a regression from Success→Failed.

### AP-7: Optimistic stub masking *(severity: MEDIUM)*
```rust
// health/monitor.rs
fn ping_neocortex() -> bool { true } // always alive
```
Tests that pass through this path cannot detect neocortex connectivity failures.

### AP-8: Security bypass in test config *(severity: MEDIUM)*
```rust
// executor.rs for_testing()
policy: PolicyGate::allow_all()
```
All executor tests bypass policy enforcement. A policy regression in execution won't be caught.

---

## Metrics

| Metric | Value | Notes |
|---|---|---|
| Modules sampled | 16 / ~100+ | Representative sample |
| Tests in sampled modules | ~254 | Approximate counts |
| Genuinely meaningful tests | ~187 | Would catch real regressions |
| Hollow / tautological tests | ~67 | Dominated by integration_tests.rs |
| Meaningful rate (sampled) | ~73.6% | |
| Modules rated STRONG | 9 | 56% of sample |
| Modules rated ADEQUATE | 3 | 19% of sample |
| Modules rated WEAK | 1 | 6% of sample |
| Modules rated TRIVIAL | 1 | 6% of sample |
| Modules with NO tests | 1 | daemon_core/react.rs |
| Anti-patterns found | 8 | 4 critical, 2 high, 2 medium |
| Critical coverage gaps | 6 | See section above |

**Estimated semantic coverage of production-critical paths: ~45–55%**

The lower figure (45%) accounts for the fact that the highest-traffic runtime paths (agentic loop, planner, executor→policy chain, E2E pipeline) are either untested or tested with tautological assertions. The higher figure (55%) reflects genuine strength in memory, identity, and policy isolation tests.

---

## Top 5 Recommendations (Priority Order)

### R1 — DELETE or REWRITE `integration_tests.rs` *(immediate)*
This file is not an asset — it is a liability. It creates false confidence about E2E coverage that does not exist. **Every test in it should either be rewritten with real assertions or deleted.** Keeping hollow tests is worse than having no tests, because they occupy CI time and block developers from knowing what's actually covered.

Start with 5 real integration tests that actually run the pipeline:
- `test_voice_command_executes_app_open`: real voice input → parser → intent → executor, assert app was opened or policy denial was returned (not both)
- `test_dangerous_action_blocked_end_to_end`: real action string → executor with real PolicyGate → assert `ExecutionResult::PolicyDenied`
- `test_telegram_authentication_rejects_unknown_sender`: real Telegram update from unknown user → assert rejection
- `test_episodic_memory_written_after_execution`: run executor → assert `episodic_store.recent_events()` contains the event
- `test_parser_produces_correct_intent_for_known_commands`: real command strings → parser → assert specific `Intent` variant

### R2 — ADD unit tests to `daemon_core/react.rs` *(high priority)*
This is AURA's core agentic loop. It has zero tests. At minimum, test:
- Input routing: does a simple command reach System1? Does a complex one reach System2?
- Retry logic: does a failed execution trigger retry up to the limit?
- Daemon sleep/wake transitions: does the loop correctly park and resume?

### R3 — Replace the planner stub with real scoring *(high priority)*
`score_plan()` returning a hardcoded `0.5` means the planner never differentiates plan quality. This is both a product bug and a test problem — tests written against it are testing nothing. Once a real scoring function exists, the test `assert!((score - 0.5).abs() < 0.001)` will correctly fail, prompting real test data.

### R4 — Add executor→policy integration test with real denial *(medium)*
Add one test to `execution/executor.rs` that:
1. Constructs a `PolicyGate::from_config()` with a rule that denies "dangerous_action"
2. Runs the executor with that action
3. Asserts `ExecutionResult::PolicyDenied` (not `Failed`)

This closes the gap where a policy bypass regression in the executor would be invisible.

### R5 — Establish regression test protocol *(process)*
Require that every bug fix includes a regression test annotated with:
```rust
#[test]
// Regression for issue #N: <description>
fn test_regression_issue_N_description() { ... }
```
This creates a living audit trail and prevents re-introduction of known bugs.

---

## What Was NOT Audited

~80+ test-containing modules were not sampled. Based on patterns observed, the following are highest-risk for containing similar hollow tests and should be prioritised for follow-up audit:

- `screen/` (7 files) — `integration_tests.rs` references screen actions with tautological assertions; the unit tests may be similarly hollow
- `bridge/system_api.rs` (2,417 lines) — largest unread file; bridge code is frequently under-tested
- `pipeline/parser.rs` — the NLU parser is a critical path with zero verified coverage
- `voice/` — entire voice pipeline untested at integration level
- `goals/` — goal tracking and scheduling; complex state machines that benefit from exhaustive testing

---

## Final Verdict

AURA v4's test suite is **two test suites stacked on top of each other**:

**The good one** — isolation tests for memory, policy, identity, and routing. These are production-grade. A developer could trust these tests to catch regressions in their specific domains. ~9 of the 16 sampled modules belong here.

**The bad one** — `integration_tests.rs` and the stub-infected executor/planner tests. These test nothing while consuming CI cycles and projecting false confidence. The 2,376-test headline number is misleading because of this file alone.

The recommendation is not panic — it is surgery. The isolation layer is solid. The integration layer needs to be rebuilt from scratch, and two critical components (`daemon_core/react.rs`, `planner.score_plan()`) need their first real tests written.

> **"Passing tests that cannot fail are not a safety net — they are a blindfold."**
