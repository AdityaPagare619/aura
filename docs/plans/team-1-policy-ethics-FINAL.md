# TEAM 1: PolicyGate + Ethics Wiring - Research & Plan

## Executive Summary

This document outlines the implementation of PolicyGate and Ethics (TRUTH framework) wiring into the AURA v4 daemon execution and response paths.

## Current State Analysis

### PolicyGate (Action Execution)
**Location:** `policy/gate.rs`

The PolicyGate in `policy/gate.rs` is a rule-based policy evaluation system with:
- Priority-sorted rule evaluation (first match wins)
- Rate limiting for suspicious action bursts
- Effects: Allow, Deny, Confirm, Audit

**Already Wired in:**
- `react.rs` lines 1173-1211 (DGS execution path)
- `react.rs` lines 1454-1482 (SemanticReact execution path)
- `react.rs` lines 1656-1666 (standalone execution)
- `main_loop.rs` lines 1141-1146 (TaskRequest handling)

### Ethics/TRUTH Framework (Response Validation)
**Location:** `identity/ethics.rs`

The TRUTH framework validates responses against five principles:
- Truthful: Detects deceptive patterns
- Relevant: Checks response length
- Unbiased: Detects bias indicators
- Transparent: Detects evasion patterns
- Helpful: Checks for bare refusals

**Already Wired in:**
- `main_loop.rs` lines 1863-1903 (TRUTH validation)
- `main_loop.rs` lines 1905-1919 (Anti-sycophancy check)

### Anti-Sycophancy Guard
**Location:** `identity/anti_sycophancy.rs`

Tracks response patterns over sliding window to detect:
- Agreement ratio
- Hedging frequency
- Opinion reversal
- Praise density
- Challenge avoidance

## First Principles Analysis

### Why These Checks Matter

1. **PolicyGate (Action Path):**
   - First Principle: "AURA must know its limits"
   - Prevents destructive actions without confirmation
   - Protects against banking-related actions
   - Rate limiting prevents abuse

2. **TRUTH Framework (Response Path):**
   - First Principle: "Be truthful, relevant, unbiased, transparent, helpful"
   - Prevents deceptive responses
   - Ensures transparency in limitations
   - Detects manipulation attempts

3. **Anti-Sycophancy (Response Path):**
   - First Principle: "Don't just agree with the user"
   - Prevents harmful agreement patterns
   - Encourages honest pushback

## Scientific Rigor - Hypotheses

### Hypothesis 1: PolicyGate blocks dangerous actions
- Test: Verify dangerous actions are denied
- Test: Verify rate limiting triggers on burst
- Test: Verify audit logging on denials

### Hypothesis 2: TRUTH Framework detects violations
- Test: Verify deceptive patterns detected
- Test: Verify biased language flagged
- Test: Verify evasion patterns caught

### Hypothesis 3: Anti-sycophancy detects patterns
- Test: Verify high agreement ratio flagged
- Test: Verify lack of challenge detected

## Implementation Details

### Wiring Points (Already Present)

1. **Action Path (react.rs):**
   ```
   PolicyContext::check_action() → gate.evaluate() → audit.log_policy_decision()
   ```

2. **Response Path (main_loop.rs):**
   ```
   ConversationReply → validate_response() → check_response() → send_response()
   ```

### Test Strategy

Write 20+ integration tests covering:
1. PolicyGate action blocking (5 tests)
2. Rate limiting behavior (3 tests)
3. TRUTH framework validation (5 tests)
4. Anti-sycophancy detection (4 tests)
5. Audit logging (3 tests)

## Test Requirements

Per task requirements:
- NO `.unwrap()` — use `Result<T, AuraError>`
- Use `tracing::{info, warn, error, debug}` for logging
- Write 20+ tests, ALL must pass

## Verification Plan

1. Run `cargo check -p aura-daemon` → 0 errors
2. Run `cargo test -p aura-daemon --lib` → 0 new failures

## Summary

The PolicyGate and Ethics wiring is **already implemented** in the codebase:
- PolicyGate is called BEFORE executor.execute() in react.rs
- TRUTH framework and anti-sycophancy are called BEFORE send_response() in main_loop.rs

The task now requires:
1. Verifying existing tests pass
2. Writing 20+ additional integration tests
3. Ensuring no regressions
