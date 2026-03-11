//! Crash-safe persistence — WAL journal, integrity verification, and safe mode.
//!
//! # Architecture
//!
//! ```text
//!  ┌─────────────┐    ┌──────────────────┐    ┌────────────────┐
//!  │ journal.rs   │───▶│ integrity.rs      │───▶│ safe_mode.rs   │
//!  │ WAL append   │    │ verify on boot    │    │ degraded ops   │
//!  │ commit/recover│    │ personality/trust  │    │ no learn/proact│
//!  └─────────────┘    └──────────────────┘    └────────────────┘
//! ```
//!
//! ## Flow
//! 1. On startup: open/create journal → `recover()` → replay committed entries
//! 2. Run `IntegrityVerifier::full_verification()` on recovered state
//! 3. If critical issues → activate `SafeModeState`
//! 4. During operation: `journal.append()` before state mutation, `journal.commit()`
//!    after mutation succeeds

pub mod integrity;
pub mod journal;
pub mod safe_mode;
pub mod vault;

pub use integrity::{IntegrityVerifier, VerificationReport, VerificationSeverity};
pub use journal::{JournalCategory, JournalEntry, JournalError, RecoveryReport, WriteAheadJournal};
pub use safe_mode::SafeModeState;
pub use vault::{CriticalVault, DataCategory, DataTier, VaultEntry, VaultError};
