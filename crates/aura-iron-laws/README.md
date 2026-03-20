# AURA Iron Laws — Immutable Ethics Layer

The 7 Iron Laws are AURA's non-negotiable ethical constraints.

## The 7 Laws

| # | Law | Description | Override? |
|---|-----|-------------|-----------|
| 1 | NeverHarm | Never harm humans | NEVER |
| 2 | ConsentForLearning | Learn only with consent | Can be granted |
| 3 | PrivacySovereignty | Zero telemetry by default | NEVER |
| 4 | TransparentReasoning | Explain all decisions | Always required |
| 5 | AntiSycophancy | Truth over approval | NEVER |
| 6 | DenyByDefault | Consent mandatory | Must be granted |
| 7 | AuditFinality | Audit verdicts final | NEVER |

## Compile-Time Enforcement

The laws are enforced at compile time via:
- `const fn` assertions
- `PhantomData` markers preventing unsafe sharing
- `!Sync` markers preventing thread sharing
- Build script checksum verification

## Usage

```rust
use aura_iron_laws::{EthicsGate, Action, IronLaw};

let mut gate = EthicsGate::new();

// Grant learning consent
gate.grant_learning_consent();

// Evaluate an action
let action = Action::new("Remember user preferences")
    .learning()
    .with_consent();

match gate.evaluate(&action) {
    EthicsResult::Permitted => println!("Action allowed"),
    EthicsResult::Denied(v) => {
        eprintln!("IRON LAW VIOLATION: {}", v.law.description());
    }
    EthicsResult::RequiresConsent { law, .. } => {
        eprintln!("Must obtain consent for {}", law.description());
    }
}
```
