# ADR-004: Two-Layer Safety Borders

**Status:** Accepted  
**Date:** 2026-03-01  
**Deciders:** AURA Core Team

## Context

AURA executes real actions on a user's phone — tapping buttons, typing text, navigating apps, managing files. A misbehaving agent could:

1. Factory reset the device (data loss)
2. Send messages without consent (social harm)
3. Make purchases or financial transactions (financial harm)
4. Disable security features (device compromise)
5. Access sensitive apps in unintended ways (privacy violation)

Safety must be:
- **Layered:** No single point of failure
- **Non-bypassable:** Core protections can't be disabled by configuration
- **Auditable:** Sensitive actions logged even if allowed
- **Configurable:** Users can adjust restrictions for non-critical actions
- **Fast:** Safety checks are in the hot path — every action is evaluated

## Decision

Implement a **two-layer safety system**: a configurable PolicyGate (Layer 1) backed by a hardcoded identity-level ethics gate (Layer 2).

### Safety Evaluation Flow

```
  Incoming Action
       │
       ▼
┌──────────────────────────────────────────┐
│  LAYER 1: PolicyGate  (policy/gate.rs)   │
│                                          │
│  Configurable rules, user-adjustable     │
│  Glob pattern matching on action names   │
│  Priority-sorted, first-match-wins       │
│  Effects: Allow | Deny | Audit | Confirm │
│                                          │
│  Max rules per event cap (DoS protect)   │
└──────────────┬───────────────────────────┘
               │
               │ if Allow or Audit
               ▼
┌──────────────────────────────────────────┐
│  LAYER 2: Identity Ethics                │
│  (identity/ethics.rs)                    │
│                                          │
│  HARDCODED — not configurable            │
│  blocked_patterns[]: absolute deny list  │
│  audit_keywords[]: always-log list       │
│  Cannot be overridden by Layer 1         │
└──────────────┬───────────────────────────┘
               │
               │ if passed both layers
               ▼
         Action Executed
```

### Layer 1: PolicyGate

**Location:** `crates/aura-daemon/src/policy/gate.rs`, `policy/rules.rs`

Configurable rule engine evaluated at runtime for every action:

```rust
// policy/rules.rs
enum RuleEffect {
    Allow,    // Action permitted
    Deny,     // Action blocked, user notified
    Audit,    // Action permitted but logged for review
    Confirm,  // Action paused, user must confirm
}
```

**Rule evaluation:**
1. Rules sorted by priority (lower number = higher priority)
2. Each rule has a glob pattern matched against the action identifier
3. First matching rule wins — evaluation stops
4. No match → falls through to Layer 2 (default behavior is layer-dependent)

**Max rules per event:** Capped to prevent pathological configurations from causing O(n) evaluation delays.

### Layer 2: Identity Ethics

**Location:** `crates/aura-daemon/src/identity/ethics.rs`

Hardcoded safety backstop that cannot be overridden:

**Blocked patterns (always denied):**
- `factory reset` — irreversible data loss
- `format storage` — irreversible data loss
- `root device` — security compromise
- `bypass lock` — security compromise
- Additional patterns for financial fraud, identity theft, etc.

**Audit keywords (always logged):**
- Sensitive terms that trigger logging even for allowed actions
- Enables post-hoc review of potentially risky behavior

### Four Safety Categories

Actions are conceptually classified into four zones:

| Category | Layer 1 | Layer 2 | Example |
|----------|---------|---------|---------|
| **Forbidden** | Deny | Blocked | Factory reset, format storage |
| **Restricted** | Confirm | Audit | Send payment, delete app |
| **Monitored** | Audit | Pass | Read notifications, access contacts |
| **Free** | Allow | Pass | Open app, scroll, type text |

### Anti-Sycophancy Gate

A specialized safety mechanism for conversation replies from the neocortex (`daemon_core/main_loop.rs`, IPC inbound handler for `ConversationReply`):

| Verdict | Action |
|---------|--------|
| `Pass` | Reply delivered to user as-is |
| `Nudge` | Reply delivered with a correction note appended |
| `Block` | Reply suppressed, user gets a neutral fallback |

This prevents the LLM from generating harmful, misleading, or excessively agreeable responses.

## Consequences

### Positive

- **Defense in depth:** Even if Layer 1 is misconfigured (all rules set to Allow), Layer 2's hardcoded blocks prevent catastrophic actions
- **User agency:** Layer 1 is fully configurable. Power users can relax restrictions for trusted apps while keeping Layer 2's absolute protections
- **Auditability:** Audit effect creates a trail of sensitive actions without blocking them. Users can review what AURA did with their data
- **Low overhead:** Glob pattern matching is O(n) in rule count with early termination. Typical rule sets (<50 rules) evaluate in microseconds
- **Confirmation flow:** Restricted actions pause for user confirmation rather than silently blocking, keeping the user in control

### Negative

- **Glob limitations:** Glob patterns can't express complex conditions (e.g., "allow sending messages only to contacts"). Future work may need a richer rule language
- **Hardcoded rigidity:** Layer 2 blocked patterns require a code release to update. Can't respond to new threat patterns dynamically
- **False positives:** Overly broad glob patterns (e.g., `*delete*`) may block benign actions like "delete draft email"

## Alternatives Considered

### 1. Single-Layer Configurable Rules Only
- **Rejected:** A misconfiguration could allow factory reset. The identity ethics layer exists precisely because some actions must never be permitted regardless of configuration.

### 2. ML-Based Action Classification
- **Rejected:** ML classifiers add latency, require training data for safety-critical decisions, and can be adversarially fooled. Deterministic pattern matching is more predictable for safety.

### 3. Capability-Based Security (Android Permissions)
- **Rejected:** Android permissions are too coarse-grained. AccessibilityService has broad permissions by design. AURA needs finer-grained action-level controls within that permission scope.

### 4. User Confirmation for Everything
- **Rejected:** Confirmation fatigue would make the agent unusable. Users would either disable confirmations or stop using AURA. The tiered approach reserves confirmation for genuinely risky actions.

## References

- `crates/aura-daemon/src/policy/gate.rs` — PolicyGate, rule evaluation, first-match-wins logic
- `crates/aura-daemon/src/policy/rules.rs` — RuleEffect enum, PolicyRule struct, glob matching
- `crates/aura-daemon/src/identity/ethics.rs` — Identity-level blocked patterns, audit keywords
- `crates/aura-daemon/src/daemon_core/main_loop.rs` — Anti-sycophancy gate on ConversationReply
