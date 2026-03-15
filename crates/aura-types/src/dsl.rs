use serde::{Deserialize, Serialize};

use crate::{
    actions::{ActionType, TargetSelector},
    tools::{ParamValue, RiskLevel},
};

/// A single step in the DSL execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DslStep {
    pub action: ActionType,
    pub target: Option<TargetSelector>,
    pub timeout_ms: u32,
    pub on_failure: FailureStrategy,
    pub precondition: Option<DslCondition>,
    pub postcondition: Option<DslCondition>,
    pub label: Option<String>,
}

/// Condition that can be evaluated against current screen state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DslCondition {
    ElementExists(TargetSelector),
    ElementNotExists(TargetSelector),
    TextEquals {
        selector: TargetSelector,
        expected: String,
    },
    AppInForeground(String),
    ScreenContainsText(String),
    /// Bounded: max MAX_DSL_CONDITION_OPERANDS items enforced at construction site.
    And(Vec<DslCondition>),
    /// Bounded: max MAX_DSL_CONDITION_OPERANDS items enforced at construction site.
    Or(Vec<DslCondition>),
    Not(Box<DslCondition>),
}

/// Max operands in a compound [`DslCondition::And`] or [`DslCondition::Or`].
pub const MAX_DSL_CONDITION_OPERANDS: usize = 16;

/// Strategy when a step fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FailureStrategy {
    /// Retry up to `max` times.
    Retry { max: u8 },
    /// Skip this step and continue.
    Skip,
    /// Abort the entire plan.
    Abort,
    /// Execute fallback steps instead.
    /// Bounded: max MAX_FAILURE_FALLBACK_STEPS items enforced at construction site.
    Fallback(Vec<DslStep>),
    /// Ask the user what to do.
    AskUser(String),
}

/// Max fallback steps in [`FailureStrategy::Fallback`].
pub const MAX_FAILURE_FALLBACK_STEPS: usize = 8;

impl Default for FailureStrategy {
    fn default() -> Self {
        FailureStrategy::Retry { max: 3 }
    }
}

/// Safety level for a DSL block — controls step limits.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SafetyLevel {
    /// Standard operation: max 200 steps.
    Normal,
    /// Reduced limit: max 50 steps.
    Safety,
    /// Extended limit: max 500 steps (power user / admin).
    Power,
}

impl SafetyLevel {
    /// Maximum number of steps allowed for this safety level.
    #[must_use]
    pub fn max_steps(&self) -> u32 {
        match self {
            SafetyLevel::Normal => 200,
            SafetyLevel::Safety => 50,
            SafetyLevel::Power => 500,
        }
    }
}

/// A named block of DSL steps with safety constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DslBlock {
    pub name: String,
    /// Bounded at runtime to `max_total_steps` entries — enforced by `can_add_steps()` at
    /// construction site.
    pub steps: Vec<DslStep>,
    pub max_total_steps: u32,
    pub safety_level: SafetyLevel,
}

impl Default for DslBlock {
    fn default() -> Self {
        Self {
            name: String::new(),
            steps: Vec::new(),
            max_total_steps: 200,
            safety_level: SafetyLevel::Normal,
        }
    }
}

impl DslBlock {
    /// Check if adding more steps would exceed the block's limit.
    #[must_use]
    pub fn can_add_steps(&self, count: u32) -> bool {
        (self.steps.len() as u32) + count <= self.max_total_steps
    }
}

// ---------------------------------------------------------------------------
// Tool invocation bridge — connects parsed intents to DSL execution
// ---------------------------------------------------------------------------

/// A high-level tool call that the planner resolves into concrete [`DslStep`]s.
///
/// This bridges the gap between the NLP parser output (which produces intents
/// with named parameters) and the low-level DSL engine (which executes screen
/// actions). The planner decomposes a `ToolCall` into one or more `DslStep`s.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Tool name, matching a key in `TOOL_REGISTRY`.
    pub tool_name: String,
    /// Resolved parameter values. Bounded at runtime to MAX_TOOL_PARAMETERS entries — enforced by
    /// consumer.
    pub parameters: Vec<(String, ParamValue)>,
    /// Risk level (copied from tool schema for quick checks).
    pub risk_level: RiskLevel,
    /// Whether user explicitly confirmed this action.
    pub user_confirmed: bool,
    /// Source parse confidence (0.0–1.0).
    pub confidence: f32,
}

/// Result of executing a tool call through the DSL engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// The tool that was called.
    pub tool_name: String,
    /// Whether execution succeeded.
    pub success: bool,
    /// How long execution took in milliseconds.
    pub duration_ms: u32,
    /// Number of DSL steps executed.
    pub steps_executed: u32,
    /// Human-readable summary of what happened.
    pub summary: String,
    /// Error message if failed.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::ScrollDirection;

    #[test]
    fn test_safety_level_max_steps() {
        assert_eq!(SafetyLevel::Normal.max_steps(), 200);
        assert_eq!(SafetyLevel::Safety.max_steps(), 50);
        assert_eq!(SafetyLevel::Power.max_steps(), 500);
    }

    #[test]
    fn test_dsl_block_defaults() {
        let block = DslBlock::default();
        assert_eq!(block.max_total_steps, 200);
        assert_eq!(block.safety_level, SafetyLevel::Normal);
        assert!(block.steps.is_empty());
    }

    #[test]
    fn test_dsl_block_can_add_steps() {
        let mut block = DslBlock {
            name: "test".to_string(),
            steps: Vec::new(),
            max_total_steps: 3,
            safety_level: SafetyLevel::Safety,
        };
        assert!(block.can_add_steps(3));
        assert!(!block.can_add_steps(4));

        block.steps.push(DslStep {
            action: ActionType::Scroll {
                direction: ScrollDirection::Down,
                amount: 300,
            },
            target: None,
            timeout_ms: 2000,
            on_failure: FailureStrategy::default(),
            precondition: None,
            postcondition: None,
            label: None,
        });
        assert!(block.can_add_steps(2));
        assert!(!block.can_add_steps(3));
    }

    #[test]
    fn test_dsl_condition_compound() {
        let cond = DslCondition::And(vec![
            DslCondition::AppInForeground("com.whatsapp".to_string()),
            DslCondition::Not(Box::new(DslCondition::ElementExists(TargetSelector::Text(
                "Loading...".to_string(),
            )))),
        ]);
        let json = serde_json::to_string(&cond).unwrap();
        assert!(json.contains("And"));
        assert!(json.contains("Not"));
    }

    #[test]
    fn test_failure_strategy_default() {
        let strategy = FailureStrategy::default();
        match strategy {
            FailureStrategy::Retry { max } => assert_eq!(max, 3),
            _ => panic!("expected Retry"),
        }
    }

    #[test]
    fn test_tool_call_serialization() {
        let call = ToolCall {
            tool_name: "message_send".to_string(),
            parameters: vec![
                (
                    "contact".to_string(),
                    ParamValue::String("Alice".to_string()),
                ),
                ("text".to_string(), ParamValue::String("Hello!".to_string())),
            ],
            risk_level: RiskLevel::Medium,
            user_confirmed: false,
            confidence: 0.95,
        };
        let json = serde_json::to_string(&call).unwrap();
        assert!(json.contains("message_send"));
        assert!(json.contains("Alice"));
    }

    #[test]
    fn test_tool_call_result() {
        let result = ToolCallResult {
            tool_name: "app_open".to_string(),
            success: true,
            duration_ms: 1500,
            steps_executed: 3,
            summary: "Opened WhatsApp".to_string(),
            error: None,
        };
        assert!(result.success);
        assert!(result.error.is_none());
    }
}
