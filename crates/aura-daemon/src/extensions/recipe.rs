use std::collections::HashMap;
use aura_types::extensions::{Recipe, RecipeStep};
use aura_types::manifest::CapabilityManifest;
use serde::{Deserialize, Serialize};

/// Recipe template for privacy-safe export and import.
/// Replaces hardcoded PII with role-based bindings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeTemplate {
    pub name: String,
    pub description: String,
    pub trigger_pattern: String,
    pub roles: Vec<RoleBinding>,
    pub template_steps: Vec<TemplateStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleBinding {
    pub role_id: String,
    pub description: String, // e.g., "The user's primary bank app", "A close family member"
    pub required_permissions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateStep {
    pub skill_id: String,
    /// Arguments where values might be template strings like "${{ role_id.property }}"
    pub template_args: HashMap<String, String>,
}

impl RecipeTemplate {
    /// Extracts a template from a concrete Recipe, replacing specific arguments with role placeholders.
    /// This is a simplified version; a real implementation would use LLM or advanced heuristics
    /// to detect PII and replace it with semantic roles.
    pub fn from_recipe(recipe: &Recipe, roles: Vec<RoleBinding>) -> Self {
        let mut template_steps = Vec::new();

        for step in &recipe.steps {
            let mut template_args = HashMap::new();
            if let Some(obj) = step.parameters_template.as_object() {
                for (k, v) in obj {
                    // In a privacy-safe export, we'd replace actual data with Role bindings.
                    // For this MVP, we just take the string representation.
                    let val_str = v.to_string().replace("\"", "");
                    template_args.insert(k.clone(), val_str);
                }
            }

            template_steps.push(TemplateStep {
                skill_id: step.tool_or_skill_id.clone(),
                template_args,
            });
        }

        RecipeTemplate {
            name: recipe.manifest.name.clone(),
            description: recipe.manifest.description.clone(),
            trigger_pattern: recipe.trigger_pattern.clone(),
            roles,
            template_steps,
        }
    }

    /// Binds a Template back into an executable Recipe by providing concrete values for the roles.
    pub fn instantiate(&self, bindings: &HashMap<String, String>, new_manifest: CapabilityManifest) -> Recipe {
        let mut steps = Vec::new();

        for t_step in &self.template_steps {
            let mut args = serde_json::Map::new();
            for (k, v) in &t_step.template_args {
                // Perform substitution if it looks like a template var
                let mut final_val = v.clone();
                for (role_id, concrete_val) in bindings {
                    let placeholder = format!("${{{{ {} }}}}", role_id);
                    final_val = final_val.replace(&placeholder, concrete_val);
                }
                args.insert(k.clone(), serde_json::Value::String(final_val));
            }

            steps.push(RecipeStep {
                tool_or_skill_id: t_step.skill_id.clone(),
                parameters_template: serde_json::Value::Object(args),
            });
        }

        Recipe {
            manifest: new_manifest,
            trigger_pattern: self.trigger_pattern.clone(),
            steps,
        }
    }
}
