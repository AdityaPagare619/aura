//! Tool Registry for Day-1 Action Templates
//!
//! # Creative Risk Leadership & Strategic Vision
//! Instead of making AURA figure out everything from scratch, we inject a strategic
//! set of "Day-1" capability templates. These are guaranteed, high-value workflows
//! modeled perfectly through `precise-system-modeling`.
//!
//! Includes templates for:
//! 1. Send Message
//! 2. Order Food
//! 3. Check Calendar

use aura_types::actions::{ActionType, ClickTarget, InputType};
use aura_types::dsl::DslStep;
use aura_types::etg::ActionPlan;

/// Template for executing a "Send Message" flow natively.
pub fn template_send_message(recipient: &str, message: &str, preferred_app: &str) -> ActionPlan {
    ActionPlan {
        goal_description: format!("Send message to {} via {}", recipient, preferred_app),
        steps: vec![
            DslStep {
                action: Some(ActionType::OpenApp { package: preferred_app.to_string() }),
                target: None,
                timeout_ms: 2000,
                on_failure: Default::default(),
                precondition: None,
                postcondition: None,
                label: None,
            },
            DslStep {
                action: Some(ActionType::Click { target: ClickTarget::ContentDesc("Search".to_string()) }),
                target: None,
                timeout_ms: 1000,
                ..Default::default()
            },
            DslStep {
                action: Some(ActionType::Input { text: recipient.to_string(), input_type: InputType::Keyboard }),
                target: None,
                timeout_ms: 1000,
                ..Default::default()
            },
            DslStep {
                action: Some(ActionType::Click { target: ClickTarget::Text(recipient.to_string()) }),
                target: None,
                timeout_ms: 1000,
                ..Default::default()
            },
            DslStep {
                action: Some(ActionType::Input { text: message.to_string(), input_type: InputType::Keyboard }),
                target: None,
                timeout_ms: 1000,
                ..Default::default()
            },
            DslStep {
                action: Some(ActionType::Click { target: ClickTarget::ContentDesc("Send".to_string()) }),
                target: None,
                timeout_ms: 1000,
                ..Default::default()
            },
        ],
        estimated_duration_ms: 7000,
        confidence: 0.95, // High confidence Day-1 template
        source: aura_types::etg::PlanSource::Cached,
    }
}

/// Template for executing an "Order Food" flow natively.
pub fn template_order_food(restaurant: &str, food_app: &str) -> ActionPlan {
    ActionPlan {
        goal_description: format!("Order food from {} via {}", restaurant, food_app),
        steps: vec![
            DslStep {
                action: Some(ActionType::OpenApp { package: food_app.to_string() }),
                target: None,
                timeout_ms: 3000,
                ..Default::default()
            },
            DslStep {
                action: Some(ActionType::Click { target: ClickTarget::ContentDesc("Search".to_string()) }),
                target: None,
                timeout_ms: 1000,
                ..Default::default()
            },
            DslStep {
                action: Some(ActionType::Input { text: restaurant.to_string(), input_type: InputType::Keyboard }),
                target: None,
                timeout_ms: 1000,
                ..Default::default()
            },
            DslStep {
                // LLM will usually have to takeover after this step to select specific items.
                action: Some(ActionType::Click { target: ClickTarget::Text(restaurant.to_string()) }),
                target: None,
                timeout_ms: 2000,
                ..Default::default()
            },
        ],
        estimated_duration_ms: 7000,
        confidence: 0.85,
        source: aura_types::etg::PlanSource::Cached,
    }
}

/// Template for "Check Calendar" flow natively.
pub fn template_check_calendar(calendar_app: &str) -> ActionPlan {
    ActionPlan {
        goal_description: format!("Check agenda in {}", calendar_app),
        steps: vec![
            DslStep {
                action: Some(ActionType::OpenApp { package: calendar_app.to_string() }),
                target: None,
                timeout_ms: 2000,
                ..Default::default()
            },
            DslStep {
                action: Some(ActionType::ExtractText { field_name: "agenda_view".to_string() }),
                target: None,
                timeout_ms: 1000,
                ..Default::default()
            },
        ],
        estimated_duration_ms: 3000,
        confidence: 0.98,
        source: aura_types::etg::PlanSource::Cached,
    }
}
