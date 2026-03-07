# Agent E: NLP Parser Hardening + Contextor Wiring

**Date:** 2026-03-05
**Agent:** E (NLP & Contextor)
**Status:** In Progress

---

## Objective

Harden the NLP parser (verify negation detection, multi-command decomposition) and
wire the Contextor's enriched output into the dispatch pipeline so the LLM actually
receives memory context, personality, and token budget.

---

## Research Findings

### 1. Negation Detection — REAL (NOT a stub)

`NegationDetector` in `parser.rs:90-268` is fully implemented:

- **Explicit negators** (11): "do not", "don't", "dont", "never", "stop", "cancel",
  "abort", "halt", "quit", "no", "not" — each with confidence scores (0.70–0.95)
- **Contracted forms** (22): "won't", "wouldn't", "shouldn't", "can't", "cannot",
  "couldn't", "mustn't", "isn't", "aren't", "doesn't", "didn't", "hasn't", "haven't"
  — with/without apostrophes
- **Implicit negators** (10): "avoid", "skip", "forget about", "forget",
  "refrain from", "prevent", "ignore", "leave out", "hold off on"
- **Double negation**: Even count = NOT negated, odd = negated; confidence lowered
  for double negation (×0.7, capped at 0.75)
- **Scoped negation**: "but", "however", "although", "yet" as scope boundaries
- **Integration**: Stage 9 in `parse()` (lines 1132-1161) wraps in
  `NluIntent::Negated` if confidence ≥ 0.6; asks clarification if 0.4–0.6
- **Safety**: `NluIntent::Negated` returns `None` from `tool_name()`, blocking execution

**Verdict:** No code changes needed. Needs 15+ tests (currently zero).

### 2. Multi-Command Decomposition — REAL (NOT a stub)

`CommandDecomposer` in `parser.rs:271-503` is fully implemented:

- **Sequential markers**: " and then ", " after that ", " then ", " next ",
  " followed by "
- **Parallel markers**: " and also ", " and ", " also ", " plus ", ", and "
- **Conditional patterns**: "if X, Y" and "if X, Y, otherwise Z"
- **"First X then Y" pattern**: Dedicated handler
- **Comma-separated commands**: With heuristic verb check (`looks_like_command()`)
- **Recursive decomposition**: The "after" part is recursively decomposed
- **Verb validation**: ~35 common action verbs to avoid false splits

**Verdict:** No code changes needed. Needs tests (currently zero).

### 3. Dialogue State — REAL (NOT a stub)

`DialogueState` in `parser.rs:507-658`:

- Ring buffer of 10 recent turns with intent/entity tracking
- Coreference resolution: "her"/"him"/"them" → last_contact, "it" → last_entity
- Repeat detection: "do that again", "repeat that", "again", "same thing"
- Whole-word pronoun matching and replacement

**Verdict:** No code changes needed. Needs tests (currently zero).

### 4. Contextor — WIRED BUT OUTPUT DISCARDED (critical bug)

`Contextor` in `contextor.rs` (1393 lines) is fully implemented AND wired into
`main_loop.rs` at Stage 5 (lines 655-680). **BUT:**

```rust
let _enriched = match enriched {  // ← _enriched prefix = UNUSED!
    Ok(e) => { ... e }
    Err(e) => { ... dispatch_system1(...); return Ok(()); }
};
```

- Stage 6 Route Classification (line 683) uses original `scored`, NOT enriched
- `dispatch_system2()` takes `&ScoredEvent`, not enriched data
- Memory context, personality prompt, and token budget are **completely discarded**

---

## Work Plan

### Task 1: Add 15+ Tests to parser.rs (Negation, Multi-Command, Dialogue)

Add comprehensive tests covering all three subsystems:

**Negation Detection (7 tests):**
1. Simple negation — "don't call mom" → `Negated` wrapping `CallMake`
2. Double negation — "don't not call mom" → NOT negated (cancels out)
3. Scoped negation — "don't call but do text" → negation only on first clause
4. Contracted negation — "won't call mom" → negated
5. Implicit negation — "avoid calling mom" → negated
6. No negation — "call mom" → not negated
7. Negated intent returns `None` from `tool_name()`

**Multi-Command Decomposition (5 tests):**
8. Sequential split — "call mom and then text dad" → 2 commands, Sequential
9. Parallel split — "call mom and text dad" → 2 commands, Parallel
10. Conditional — "if raining, bring umbrella" → Conditional
11. First/then — "first call mom then text dad" → Sequential
12. Single command — "call mom" → not compound

**Dialogue State (4 tests):**
13. Coreference — record "call Alice", then "call her" → resolves to Alice
14. Repeat command — "do that again" detected as repeat
15. Entity tracking — record turn with contact, verify last_contact updated
16. Non-repeat — "call bob" is NOT a repeat command

### Task 2: Wire Contextor Output into Dispatch Pipeline

In `main_loop.rs`:

1. **Rename `_enriched` → `enriched`** at line 664
2. **Add `prepare_request_enriched` method** to `System2Gateway` that accepts
   `EnrichedEvent` and populates `ContextPackage` with:
   - `memory_snippets` from `enriched.memory_context`
   - `conversation_history` from `enriched.conversation_history`
   - `personality` from enriched user context
   - `token_budget` from `enriched.context_token_budget`
   - `current_screen` from `enriched.screen_summary`
   - `active_goal` from `enriched.active_goals`
3. **Update `dispatch_system2()`** to accept `Option<&EnrichedEvent>` and use
   `prepare_request_enriched` when available, falling back to `prepare_request`

### Task 3: Verify

- `cargo check` passes
- `cargo test -p aura-daemon --lib` passes with all 15+ new tests green

---

## Files Modified

| File | Change |
|------|--------|
| `crates/aura-daemon/src/pipeline/parser.rs` | +15 tests |
| `crates/aura-daemon/src/daemon_core/main_loop.rs` | Wire enriched output |
| `crates/aura-daemon/src/routing/system2.rs` | Add `prepare_request_enriched` |

---

## Risks & Mitigations

- **ContextPackage size overflow**: `EnrichedEvent` may produce >64KB context.
  Mitigation: The Contextor already applies token budgeting; we enforce
  `ContextPackage::MAX_SIZE` check before IPC send.
- **Latency**: Extra data copy from enriched → ContextPackage. Mitigation: This is
  a simple field mapping, sub-microsecond.
- **Type mismatch**: `MemorySnippet` types in contextor vs ipc may differ.
  Mitigation: Both use `aura_types::ipc::MemorySnippet`.
