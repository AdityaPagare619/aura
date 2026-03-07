//! Execution engine: orchestrates action plans on the device screen.
//!
//! Submodules:
//! - `cycle` — 4-tier cycle detection with zero-heap transition buffer
//! - `retry` — Exponential backoff retry logic
//! - `monitor` — Execution monitoring with 10 invariants
//! - `etg` — Element-Transition Graph (in-memory + SQLite)
//! - `executor` — Main `execute_plan()` engine

pub mod cycle;
pub mod retry;
pub mod monitor;
pub mod etg;
pub mod executor;
pub mod planner;

pub use cycle::{CycleDetector, CycleTier, TransitionEntry};
pub use retry::{RetryPolicy, retry_with_backoff};
pub use monitor::{ExecutionMonitor, InvariantViolation};
pub use etg::EtgStore;
pub use executor::{Executor, ExecutionOutcome};
pub use planner::{ActionPlanner, PlanError};
