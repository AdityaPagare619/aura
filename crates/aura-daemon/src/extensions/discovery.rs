use serde_json::Value;

/// Smart Extension Discovery (E5)
/// Rather than a traditional "App Store", AURA observes user needs
/// and suggests Abilities or Skills proactively.
pub struct ExtensionDiscovery;

impl ExtensionDiscovery {
    pub fn new() -> Self {
        Self
    }

    /// Called when the ETG or prediction engine identifies a repeated failure
    /// or a missing capability in a specific domain.
    pub async fn analyze_gap(&self, domain: &str, missing_action: &str) -> Option<String> {
        // In a real implementation, this would query a registry or HNSW semantic index
        // of available skills/recipes matching the `missing_action` embedding.
        // For now, it returns mock suggestions.
        if domain == "finance" && missing_action == "process_receipt" {
            Some("suggest: 'Expensify Lens' to automatically parse receipts".into())
        } else if domain == "productivity" && missing_action == "summarize_meeting" {
            Some("suggest: 'Meeting Minutes Skill' to auto-summarize transcripts".into())
        } else {
            None
        }
    }

    /// Evaluates if a newly encountered user routine would benefit from a Recipe.
    pub fn suggest_recipe_for_routine(&self, routine_name: &str, steps: &[&str]) -> Option<String> {
        if steps.len() > 3 {
             Some(format!("You perform '{}' daily. Would you like me to create a Recipe to run these {} steps automatically?", routine_name, steps.len()))
        } else {
             None
        }
    }
}
