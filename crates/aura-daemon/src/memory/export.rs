//! Privacy-safe pattern export format.
//!
//! This module provides the ability to export learned workflows and patterns
//! while stripping out personal data, specific targets, and other PII.
//! The exported "Recipes" can be shared safely across AURA instances.

use crate::execution::learning::workflows::WorkflowPattern;
use aura_types::actions::{ActionType, ClickTarget};
use serde::{Deserialize, Serialize};

/// An anonymized, generic representation of a workflow or pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedRecipe {
    /// Generic name inferred or provided for the recipe.
    pub name: String,
    /// The sequence of actions, with PII and specific data removed.
    pub abstract_sequence: Vec<ActionType>,
    /// Minimum success frequency observed before export.
    pub success_frequency: u32,
    /// Version of the exporter used.
    pub version: u32,
}

impl ExportedRecipe {
    /// Anonymize a given `WorkflowPattern` by stripping out personal target data.
    pub fn from_workflow(name: &str, pattern: &WorkflowPattern) -> Self {
        let abstract_sequence = pattern
            .sequence
            .iter()
            .map(|action| anonymize_action(action))
            .collect();

        Self {
            name: name.to_string(),
            abstract_sequence,
            success_frequency: pattern.frequency,
            version: 1,
        }
    }
}

/// Strip sensitive data from an `ActionType`, leaving only the structural intent.
fn anonymize_action(action: &ActionType) -> ActionType {
    match action {
        ActionType::LeftClick { .. } => ActionType::LeftClick { x: 0.0, y: 0.0 }, // Coordinates removed
        ActionType::RightClick { .. } => ActionType::RightClick { x: 0.0, y: 0.0 },
        ActionType::DoubleClick { .. } => ActionType::DoubleClick { x: 0.0, y: 0.0 },
        ActionType::Click { .. } => {
            // Leave as a generic Click with no target, or a safe placeholder
            ActionType::Click { target: ClickTarget::Coordinate { x: 0.0, y: 0.0 } }
        }
        ActionType::TypeText { .. } => ActionType::TypeText {
            text: "<redacted>".to_string(),
        },
        ActionType::PressKey { key } => ActionType::PressKey { key: key.clone() },
        ActionType::Wait { duration_ms } => ActionType::Wait { duration_ms: *duration_ms },
        ActionType::ExtractText { field_name } => ActionType::ExtractText {
            field_name: field_name.clone(), // Field names (e.g., 'price') usually aren't PII
        },
        ActionType::Scroll { direction, amount } => ActionType::Scroll {
            direction: direction.clone(),
            amount: *amount,
        },
        ActionType::ShellCommand { .. } => ActionType::ShellCommand {
            command: "<redacted>".to_string(), // Highly sensitive, redact the whole command
        },
        ActionType::OpenApp { package } => ActionType::OpenApp {
            package: package.clone(), // Package names are usually public info (e.g., "com.whatsapp")
        },
        ActionType::ApiCall { endpoint, .. } => ActionType::ApiCall {
            endpoint: endpoint.clone(),
            body: Some("<redacted>".to_string()),
        },
        ActionType::Finished => ActionType::Finished,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anonymize_sensitive_actions() {
        let sensitive_type = ActionType::TypeText { text: "my_password_123".to_string() };
        let anonymized = anonymize_action(&sensitive_type);
        
        if let ActionType::TypeText { text } = anonymized {
            assert_eq!(text, "<redacted>");
        } else {
            panic!("Expected TypeText");
        }

        let sensitive_cmd = ActionType::ShellCommand { command: "rm -rf /my/private/files".to_string() };
        let anonymized_cmd = anonymize_action(&sensitive_cmd);
        if let ActionType::ShellCommand { command } = anonymized_cmd {
            assert_eq!(command, "<redacted>");
        } else {
            panic!("Expected ShellCommand");
        }
    }
}
