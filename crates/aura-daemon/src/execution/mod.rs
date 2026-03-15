//! Execution engine: orchestrates action plans on the device screen.
//!
//! Submodules:
//! - `cycle` — 4-tier cycle detection with zero-heap transition buffer
//! - `retry` — Exponential backoff retry logic
//! - `monitor` — Execution monitoring with 10 invariants
//! - `etg` — Element-Transition Graph (in-memory + SQLite)
//! - `executor` — Main `execute_plan()` engine

pub mod cycle;
pub mod etg;
pub mod executor;
pub mod learning;
pub mod monitor;
pub mod planner;
pub mod react;
pub mod retry;
pub mod tools;

pub use cycle::{CycleDetector, CycleTier, TransitionEntry};
pub use etg::EtgStore;
pub use executor::{ExecutionOutcome, Executor};
pub use learning::{ExecutionTrace, WorkflowObserver, WorkflowPattern};
pub use monitor::{ExecutionMonitor, InvariantViolation};
pub use planner::{ActionPlanner, EnhancedPlanner, PlanError};
pub use react::{CognitiveState, EscalationContext, SemanticReact};
pub use retry::{retry_with_backoff, RetryPolicy};
