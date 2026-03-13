use crate::manifest::CapabilityManifest;
use async_trait::async_trait;
use serde_json::Value;

/// The foundational trait for all AURA extensions.
/// Extensions are not generic "plugins" — they are categorized by their cognitive function.
#[async_trait]
pub trait Extension: Send + Sync {
    /// The ethical contract explaining what this extension does.
    fn manifest(&self) -> &CapabilityManifest;
    
    /// Health check
    async fn is_healthy(&self) -> bool;
}

/// A **Skill** is an active capability. It lets AURA *do* something new.
/// (e.g., Code Execution, Email Drafting, Calendar Management)
#[async_trait]
pub trait Skill: Extension {
    async fn execute(&self, input: Value) -> Result<Value, String>;
    fn usage_schema(&self) -> Value;
}

/// An **Ability** is a passive edge capability. It runs constantly in the background.
/// (e.g., Audio processing, Bluetooth proximity, Step counting)
#[async_trait]
pub trait Ability: Extension {
    async fn start(&self) -> Result<(), String>;
    async fn stop(&self) -> Result<(), String>;
    fn get_current_state(&self) -> Value;
}

/// A **Lens** is a perceptual filter. It changes *how* AURA views existing data.
/// (e.g., "Developer Lens" extracts code blocks from screen context)
pub trait Lens: Extension {
    fn process_context(&self, raw_context: &str) -> String;
}

/// A **Recipe** is a user-specific chained workflow of Skills/Tools.
/// (e.g., "Morning Briefing": weather tool -> calendar tool -> generate voice)
#[derive(Debug, Clone)]
pub struct Recipe {
    pub manifest: CapabilityManifest,
    pub trigger_pattern: String,
    /// Bounded at runtime to MAX_RECIPE_STEPS entries — enforced by the recipe loader.
    pub steps: Vec<RecipeStep>,
}

#[derive(Debug, Clone)]
pub struct RecipeStep {
    pub tool_or_skill_id: String,
    pub parameters_template: Value,
}
