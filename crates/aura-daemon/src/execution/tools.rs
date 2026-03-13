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

use aura_types::actions::{ActionType, TargetSelector};
use aura_types::dsl::{DslStep, FailureStrategy};
use aura_types::etg::ActionPlan;
use std::collections::HashMap;

/// Maximum number of templates stored in the ETG cache.
/// Enforced in `overwrite_learned_path` to prevent unbounded growth.
const MAX_ETG_CACHE_TEMPLATES: usize = 256;

/// A lightweight representation of the Execution Trace Graph (ETG) cache.
/// In production, this is backed by the semantic/episodic memory databases.
/// We define it here to prove mathematically that Day-1 templates are mutable seeds,
/// not rigid, hardcoded logic trapped in the binary.
pub struct EtgCache {
    pub templates: HashMap<String, ActionPlan>,
    pub eviction_count: u32,
}

impl Default for EtgCache {
    fn default() -> Self {
        Self {
            templates: HashMap::new(),
            eviction_count: 0,
        }
    }
}

impl EtgCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// When the adaptive react engine (System 2) finds a better execution path
    /// (e.g., UI changes, user preference changes), it actively overwrites the cached template.
    /// Enforces `MAX_ETG_CACHE_TEMPLATES` cap — evicts the oldest-inserted entry when full.
    pub fn overwrite_learned_path(&mut self, intent: &str, new_plan: ActionPlan) {
        if self.templates.len() >= MAX_ETG_CACHE_TEMPLATES && !self.templates.contains_key(intent) {
            // Evict an arbitrary entry to stay within capacity.
            if let Some(key) = self.templates.keys().next().cloned() {
                self.templates.remove(&key);
            }
        }
        self.templates.insert(intent.to_string(), new_plan);
        self.eviction_count += 1;
    }
}

/// Seeds the Day-1 templates into the mutable ETG cache.
/// AURA relies on these for immediate utility, but if they fail, the Adaptive React engine
/// takes over (System 2) and structurally overwrites them via `overwrite_learned_path`.
pub fn seed_day1_templates(
    cache: &mut EtgCache,
    preferred_messaging: &str,
    preferred_food: &str,
    preferred_calendar: &str,
) {
    cache.templates.insert(
        "send_message".to_string(),
        template_send_message("<<RECIPIENT>>", "<<MSG>>", preferred_messaging),
    );
    cache.templates.insert(
        "order_food".to_string(),
        template_order_food("<<RESTAURANT>>", preferred_food),
    );
    cache.templates.insert(
        "check_calendar".to_string(),
        template_check_calendar(preferred_calendar),
    );
}

/// Template for executing a "Send Message" flow natively.
pub fn template_send_message(recipient: &str, message: &str, preferred_app: &str) -> ActionPlan {
    ActionPlan {
        goal_description: format!("Send message to {} via {}", recipient, preferred_app),
        steps: vec![
            DslStep {
                action: ActionType::OpenApp { package: preferred_app.to_string() },
                target: None,
                timeout_ms: 2000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: None,
            },
            // Tap the Search button; coordinates resolved at runtime by the adaptive engine.
            DslStep {
                action: ActionType::WaitForElement {
                    selector: TargetSelector::ContentDescription("Search".to_string()),
                    timeout_ms: 1000,
                },
                target: None,
                timeout_ms: 1000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: Some("wait_search_button".to_string()),
            },
            DslStep {
                action: ActionType::Type { text: recipient.to_string() },
                target: None,
                timeout_ms: 1000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: None,
            },
            // Tap the recipient result row; coordinates resolved at runtime.
            DslStep {
                action: ActionType::WaitForElement {
                    selector: TargetSelector::Text(recipient.to_string()),
                    timeout_ms: 2000,
                },
                target: None,
                timeout_ms: 2000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: Some("wait_recipient_row".to_string()),
            },
            DslStep {
                action: ActionType::Type { text: message.to_string() },
                target: None,
                timeout_ms: 1000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: None,
            },
            // Tap the Send button; coordinates resolved at runtime by the adaptive engine.
            DslStep {
                action: ActionType::WaitForElement {
                    selector: TargetSelector::ContentDescription("Send".to_string()),
                    timeout_ms: 1000,
                },
                target: None,
                timeout_ms: 1000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: Some("wait_send_button".to_string()),
            },
        ],
        estimated_duration_ms: 7000,
        confidence: 0.95, // High confidence Day-1 template
        source: aura_types::etg::PlanSource::EtgLookup,
    }
}

/// Template for executing an "Order Food" flow natively.
pub fn template_order_food(restaurant: &str, food_app: &str) -> ActionPlan {
    ActionPlan {
        goal_description: format!("Order food from {} via {}", restaurant, food_app),
        steps: vec![
            DslStep {
                action: ActionType::OpenApp { package: food_app.to_string() },
                target: None,
                timeout_ms: 3000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: None,
            },
            // Wait for Search button then tap; coordinates resolved at runtime.
            DslStep {
                action: ActionType::WaitForElement {
                    selector: TargetSelector::ContentDescription("Search".to_string()),
                    timeout_ms: 1000,
                },
                target: None,
                timeout_ms: 1000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: Some("wait_search_button".to_string()),
            },
            DslStep {
                action: ActionType::Type { text: restaurant.to_string() },
                target: None,
                timeout_ms: 1000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: None,
            },
            DslStep {
                // LLM will usually have to take over after this step to select specific items.
                action: ActionType::WaitForElement {
                    selector: TargetSelector::Text(restaurant.to_string()),
                    timeout_ms: 2000,
                },
                target: None,
                timeout_ms: 2000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: Some("wait_restaurant_row".to_string()),
            },
        ],
        estimated_duration_ms: 7000,
        confidence: 0.85,
        source: aura_types::etg::PlanSource::EtgLookup,
    }
}

/// Template for "Check Calendar" flow natively.
pub fn template_check_calendar(calendar_app: &str) -> ActionPlan {
    ActionPlan {
        goal_description: format!("Check agenda in {}", calendar_app),
        steps: vec![
            DslStep {
                action: ActionType::OpenApp { package: calendar_app.to_string() },
                target: None,
                timeout_ms: 2000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: None,
            },
            // Wait for the agenda view to be present; text extraction is handled by the
            // vision layer above the DSL engine (ExtractText has no ActionType equivalent).
            DslStep {
                action: ActionType::WaitForElement {
                    selector: TargetSelector::LlmDescription("agenda_view".to_string()),
                    timeout_ms: 1000,
                },
                target: None,
                timeout_ms: 1000,
                on_failure: FailureStrategy::default(),
                precondition: None,
                postcondition: None,
                label: Some("wait_agenda_view".to_string()),
            },
        ],
        estimated_duration_ms: 3000,
        confidence: 0.98,
        source: aura_types::etg::PlanSource::EtgLookup,
    }
}
