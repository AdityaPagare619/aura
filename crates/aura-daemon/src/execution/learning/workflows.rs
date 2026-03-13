//! Workflow Learning (System 2 Observation -> System 1 Automation)
//!
//! # Scientific Rigor Methodology
//! This module acts as the "Cerebral Cortex -> Basal Ganglia" consolidator.
//! It observes traces of successful actions and looks for statistical repetition
//! (causal behavior chains) to automatically offer Macros/Recipes to the user.
//! We do not guess automation; we demand rigorous repetition thresholds before
//! abstracting a manual flow into an autonomous Recipe.
//!
//! # Precise System Modeling
//! - **Points**: `ExecutionTrace` (History of completed plans)
//! - **Events**: `observe_trace()`
//! - **Lines**: Extracted `WorkflowPattern`s containing `DslStep`s.
//! - **Invariants**: Must have observed a strict sequence `>= 3` times before synthesizing.

use std::collections::{HashMap, VecDeque};
use aura_types::actions::ActionType;
use aura_types::etg::ActionPlan;
use serde::{Deserialize, Serialize};

/// Maximum number of execution traces retained in the history buffer.
/// Enforced in `observe_success` to prevent unbounded growth.
const MAX_TRACE_HISTORY: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTrace {
    pub goal_description: String,
    pub steps: Vec<ActionType>,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowPattern {
    pub sequence: Vec<ActionType>,
    pub frequency: u32,
    pub avg_time_ms: u64,
}

/// Observes successful plans and extracts recurring sequences.
#[derive(Debug)]
pub struct WorkflowObserver {
    /// History of the last N successful execution traces.
    trace_history: VecDeque<ExecutionTrace>,
    /// Minimum times a sequence must exactly repeat before offering automation.
    min_frequency_threshold: u32,
}

impl Default for WorkflowObserver {
    fn default() -> Self {
        Self {
            trace_history: VecDeque::with_capacity(100),
            min_frequency_threshold: 3,
        }
    }
}

impl WorkflowObserver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a successfully executed plan.
    pub fn observe_success(&mut self, plan: &ActionPlan, current_time_ms: u64) {
        let trace = ExecutionTrace {
            goal_description: plan.goal_description.clone(),
            steps: plan.steps.iter().map(|s| s.action.clone()).collect(),
            timestamp_ms: current_time_ms,
        };

        if self.trace_history.len() >= MAX_TRACE_HISTORY {
            self.trace_history.pop_front();
        }
        self.trace_history.push_back(trace);
    }

    /// Abstract the history buffer to find repeating sequences of actions (minimum length 3).
    /// Returns the most frequent pattern if it crosses the `min_frequency_threshold`.
    pub fn extract_automation_candidate(&self) -> Option<WorkflowPattern> {
        // Use JSON serialization of the sequence as a hashable key.
        let mut sequence_counts: HashMap<String, (Vec<ActionType>, u32)> = HashMap::new();

        // O(N^2) extraction over small fixed buffer.
        // In deep learning systems this would be LSTM/Transformer based sequence modeling.
        for trace in &self.trace_history {
            // We only care about workflows that are at least 3 steps long
            if trace.steps.len() >= 3 {
                let key = serde_json::to_string(&trace.steps).unwrap_or_default();
                let entry = sequence_counts.entry(key).or_insert_with(|| (trace.steps.clone(), 0));
                entry.1 += 1;
            }
        }

        let best_pattern = sequence_counts
            .into_values()
            .filter(|(_, count)| *count >= self.min_frequency_threshold)
            .max_by_key(|(_, count)| *count);

        if let Some((sequence, freq)) = best_pattern {
            Some(WorkflowPattern {
                sequence,
                frequency: freq,
                avg_time_ms: 15_000, // Placeholder calculation
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_types::actions::ActionType;

    #[test]
    fn test_workflow_extraction() {
        let mut observer = WorkflowObserver::new();
        let plan = ActionPlan {
            goal_description: "Check messages".to_string(),
            steps: vec![
                aura_types::dsl::DslStep {
                    action: ActionType::OpenApp { package: "com.whatsapp".to_string() },
                    target: None,
                    timeout_ms: 1000,
                    on_failure: Default::default(),
                    precondition: None,
                    postcondition: None,
                    label: None,
                },
                aura_types::dsl::DslStep {
                    action: ActionType::Tap { x: 100, y: 200 },
                    target: None,
                    timeout_ms: 1000,
                    on_failure: Default::default(),
                    precondition: None,
                    postcondition: None,
                    label: None,
                },
                aura_types::dsl::DslStep {
                    action: ActionType::Back,
                    target: None,
                    timeout_ms: 1000,
                    on_failure: Default::default(),
                    precondition: None,
                    postcondition: None,
                    label: None,
                },
            ],
            estimated_duration_ms: 3000,
            confidence: 0.9,
            source: aura_types::etg::PlanSource::LlmGenerated,
        };

        // Observe it 3 times
        observer.observe_success(&plan, 100);
        observer.observe_success(&plan, 200);
        observer.observe_success(&plan, 300);

        let candidate = observer.extract_automation_candidate().expect("Should extract candidate");
        assert_eq!(candidate.frequency, 3);
        assert_eq!(candidate.sequence.len(), 3);
    }
}
