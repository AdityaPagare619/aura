use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};
use aura_types::extensions::{Ability, Extension, Lens, Recipe, Skill};
use aura_types::manifest::CapabilityManifest;

/// CapabilityLoader manages the dynamic loading, unloading, and routing of AURA extensions.
/// Note: Real implementation would use libloading for dylo or wasmtime for WASM sandboxing (E5).
/// For now, it manages statically linked but dynamically toggleable capabilities.
pub struct CapabilityLoader {
    abilities: Arc<RwLock<HashMap<String, Arc<dyn Ability>>>>,
    skills: Arc<RwLock<HashMap<String, Arc<dyn Skill>>>>,
    lenses: Arc<RwLock<HashMap<String, Arc<dyn Lens>>>>,
    recipes: Arc<RwLock<HashMap<String, Recipe>>>,
}

impl CapabilityLoader {
    pub fn new() -> Self {
        Self {
            abilities: Arc::new(RwLock::new(HashMap::new())),
            skills: Arc::new(RwLock::new(HashMap::new())),
            lenses: Arc::new(RwLock::new(HashMap::new())),
            recipes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Registers a new active Ability (background capability).
    pub async fn register_ability(&self, ability: Arc<dyn Ability>) -> Result<(), String> {
        let manifest = ability.manifest();
        if !self.verify_manifest(manifest) {
            return Err("Manifest failed ethical/security review".into());
        }

        info!("Registering Ability: {} (v{})", manifest.name, manifest.version);
        let mut guard = self.abilities.write().await;
        guard.insert(manifest.id.clone(), ability.clone());
        
        // Auto-start Abilities when registered
        if let Err(e) = ability.start().await {
            error!("Failed to start ability {}: {}", manifest.name, e);
        }
        
        Ok(())
    }

    /// Registers a new Skill (action capability).
    pub async fn register_skill(&self, skill: Arc<dyn Skill>) -> Result<(), String> {
        let manifest = skill.manifest();
        if !self.verify_manifest(manifest) {
            return Err("Manifest failed ethical/security review".into());
        }

        info!("Registering Skill: {} (v{})", manifest.name, manifest.version);
        let mut guard = self.skills.write().await;
        guard.insert(manifest.id.clone(), skill);
        Ok(())
    }

    /// Checks if a manifest breaks core security/privacy rules.
    /// This is the primary sandbox checkpoint (E4).
    fn verify_manifest(&self, manifest: &CapabilityManifest) -> bool {
        // Example policy: No 3rd party extension can write to Semantic memory
        use aura_types::manifest::Permission;
        for perm in &manifest.permissions {
            if matches!(perm, Permission::WriteSemanticMemory) && manifest.author != "AURA Core" {
                warn!("Rejected {}: 3rd party extensions cannot write to Semantic Memory", manifest.name);
                return false;
            }
        }

        // Example policy: CPU limits
        if manifest.max_cpu_percent > 30 {
            warn!("Rejected {}: Requests too much CPU ({}%)", manifest.name, manifest.max_cpu_percent);
            return false;
        }

        true
    }

    /// Fetches a specific skill for execution.
    pub async fn get_skill(&self, id: &str) -> Option<Arc<dyn Skill>> {
        let guard = self.skills.read().await;
        guard.get(id).cloned()
    }

    /// Applies all active lenses to a raw input string (e.g., text from screen)
    pub async fn apply_lenses(&self, raw_input: &str) -> String {
        let guard = self.lenses.read().await;
        let mut processed = raw_input.to_string();
        for lens in guard.values() {
            processed = lens.process_context(&processed);
        }
        processed
    }
}
