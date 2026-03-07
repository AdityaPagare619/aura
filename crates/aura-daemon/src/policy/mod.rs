//! PolicyGate — safety rule engine for action gating.
//!
//! Evaluates actions against an ordered list of glob-pattern rules loaded
//! from `safety.toml`.  Rule evaluation is first-match-wins with priority
//! ordering.  Effects are: Allow, Deny, Audit, Confirm.
//!
//! # Architecture
//!
//! ```text
//! action string ─► PolicyGate::evaluate() ─► PolicyDecision
//!                        │
//!                        ├─ sorted rules (by priority)
//!                        ├─ glob match on action pattern
//!                        └─ first match → return effect
//! ```
//!
//! # Security Infrastructure
//!
//! ```text
//! ┌─────────┐    ┌──────────┐    ┌─────────────┐
//! │ AuditLog│◄───│ Sandbox  │◄───│EmergencyStop│
//! │ (trace) │    │ (contain)│    │ (kill switch)│
//! └─────────┘    └──────────┘    └─────────────┘
//!       │              │               │
//!       └──────────────┴───────────────┘
//!                      │
//!              Defense in Depth
//! ```

pub mod audit;
pub mod emergency;
pub mod gate;
pub mod rules;
pub mod sandbox;

#[cfg(test)]
pub mod wiring;

pub use audit::{AuditEntry, AuditLevel, AuditLog};
pub use emergency::{EmergencyReason, EmergencyState, EmergencyStop};
pub use gate::{PolicyDecision, PolicyGate, RateLimiter};
pub use rules::{PolicyRule, RuleEffect};
pub use sandbox::{ContainmentLevel, Sandbox, SandboxSession};
