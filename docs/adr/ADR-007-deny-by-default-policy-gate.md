# ADR-007: Deny-by-Default Policy Gate

**Status:** Accepted  
**Date:** 2026-03-13  
**Deciders:** AURA Core Team

## Context

The policy gate (`production_policy_gate()`) was previously implemented as `allow_all_builder()` — it allowed all capability requests by default. This violated the principle of least privilege and Iron Law IL-7.

During the v4 production audit on 2026-03-13, this was identified as a critical security gap:

- Any capability that had not been explicitly denied would be silently permitted
- Prompt injection attacks could exploit new or unreviewed capabilities without any code changes
- The allow-all posture is fundamentally incompatible with a security-first, on-device agent that has access to notifications, messages, calls, and system settings

The gate lives in `crates/aura-daemon/src/policy/wiring.rs` and is the single point of capability authorization for all tool calls passing through the executor pipeline (Stage 2.5 of the 11-stage executor).

## Decision

Replace `allow_all_builder()` with a deny-by-default gate in `crates/aura-daemon/src/policy/wiring.rs`.

```rust
// Before (INSECURE — removed):
// let gate = allow_all_builder().build();

// After (production_policy_gate):
let gate = production_policy_gate();
```

All capability requests are denied unless explicitly listed in the allow-list. The allow-list is defined at compile time, not runtime, so it cannot be bypassed by prompt injection or configuration file changes.

Key properties of the new gate:

- **Compile-time allow-list:** Permitted capabilities are enumerated in source code. Adding a capability requires a code change, PR review, and new build.
- **Deny-by-default:** Any capability not on the allow-list returns `PolicyDecision::Denied` regardless of the requesting context, trust tier, or LLM instruction.
- **Not overridable at runtime:** The gate is not parameterized by config files, user settings, or dynamic state. Runtime context can only further restrict (e.g., deny during focus mode) — never expand the base allow-list.
- **Audit-logged:** All decisions (allow and deny) pass through `policy/audit.rs` for on-device audit logging.

## Consequences

### Positive

- **Prompt injection resistance:** A malicious prompt cannot grant capabilities that are not already on the compile-time allow-list. Even if the LLM is tricked into requesting a capability, the gate denies it.
- **Explicit capability review:** Every new capability requires a deliberate code change reviewed by a human. There is no path to accidentally enabling something.
- **Defense-in-depth:** Aligns with Iron Law IL-7. The policy gate layer provides a hard boundary that does not depend on the LLM's judgment or the correctness of upstream validation.
- **Audit completeness:** With deny-by-default, any denied request in the audit log represents a genuine unexpected event — not routine traffic. This makes anomaly detection meaningful.

### Negative

- **New legitimate capabilities require code changes:** Any new tool or capability that AURA should be allowed to use must be explicitly added to the allow-list. This is intentional friction — it is the feature, not a bug.
- **Initial migration overhead:** Existing capabilities must be enumerated and added to the allow-list. Any omission during migration will surface as a denied action in testing.

## Alternatives Considered

### 1. Capability Allowlist in Config File

Allow the allow-list to be defined in `~/.config/aura/config.toml` so users can extend it without rebuilding.

**Rejected:** Runtime config can be read and potentially modified by malicious content processed by the LLM (prompt injection writing to config, or a compromised config file). The compile-time boundary is the only guarantee that cannot be subverted by runtime content.

### 2. Sandboxed Subprocess Per Action

Run each tool invocation in an isolated subprocess with minimal capabilities, using OS-level sandboxing (seccomp, Android SELinux contexts).

**Rejected:** Too high overhead on mobile hardware. Spawning a subprocess per action adds 50–200ms latency per tool call, which would make multi-step ReAct loops unacceptably slow (a 10-step task would add 500ms–2s of pure overhead). The policy gate in Rust achieves the same capability-restriction goal with microsecond overhead.

## References

- `crates/aura-daemon/src/policy/wiring.rs` — Policy gate construction; location of the `allow_all_builder()` → `production_policy_gate()` change
- `crates/aura-daemon/src/policy/gate.rs` — `PolicyGate::evaluate()` — the runtime enforcement point
- `crates/aura-daemon/src/policy/audit.rs` — Audit logging for all policy decisions
- `docs/architecture/AURA-V4-OPERATIONAL-FLOW.md §4` — 11-stage executor pipeline; Stage 2.5 is the PolicyGate check
- `docs/architecture/AURA-V4-IDENTITY-ETHICS-AND-PHILOSOPHY.md` — Iron Law IL-7 definition
