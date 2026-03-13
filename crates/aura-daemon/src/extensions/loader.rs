use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};
use aura_types::extensions::{Ability, Lens, Recipe, Skill};
use aura_types::manifest::CapabilityManifest;

// ─── Capacity limits ────────────────────────────────────────────────────────
//
// Iron Law: bounded collections everywhere. Android devices have 4–8 GB shared
// with the OS. Extensions are third-party code — an unbounded registry is a
// trivial OOM vector. These caps are intentionally conservative.

/// Maximum number of concurrently registered Ability extensions.
const MAX_ABILITIES: usize = 32;
/// Maximum number of concurrently registered Skill extensions.
const MAX_SKILLS: usize = 64;
/// Maximum number of concurrently registered Lens extensions.
const MAX_LENSES: usize = 32;
/// Maximum number of cached Recipes.
const MAX_RECIPES: usize = 128;

/// CapabilityLoader manages the dynamic loading, unloading, and routing of AURA extensions.
/// Note: Real implementation would use libloading for dylo or wasmtime for WASM sandboxing (E5).
/// For now, it manages statically linked but dynamically toggleable capabilities.
#[allow(dead_code)] // Phase 8: recipes field used by capability hot-loading system
pub struct CapabilityLoader {
    abilities: Arc<RwLock<HashMap<String, Arc<dyn Ability>>>>,
    skills: Arc<RwLock<HashMap<String, Arc<dyn Skill>>>>,
    lenses: Arc<RwLock<HashMap<String, Arc<dyn Lens>>>>,
    recipes: Arc<RwLock<HashMap<String, Recipe>>>,
}

impl CapabilityLoader {
    pub fn new() -> Self {
        Self {
            abilities: Arc::new(RwLock::new(HashMap::with_capacity(MAX_ABILITIES))),
            skills: Arc::new(RwLock::new(HashMap::with_capacity(MAX_SKILLS))),
            lenses: Arc::new(RwLock::new(HashMap::with_capacity(MAX_LENSES))),
            recipes: Arc::new(RwLock::new(HashMap::with_capacity(MAX_RECIPES))),
        }
    }

    /// Registers a new active Ability (background capability).
    pub async fn register_ability(&self, ability: Arc<dyn Ability>) -> Result<(), String> {
        let manifest = ability.manifest();
        if !self.verify_manifest(manifest) {
            return Err("Manifest failed ethical/security review".into());
        }

        let mut guard = self.abilities.write().await;
        // Bounded collection: reject if at capacity (prevents unbounded heap growth).
        if guard.len() >= MAX_ABILITIES && !guard.contains_key(&manifest.id) {
            warn!(
                name = manifest.name.as_str(),
                max = MAX_ABILITIES,
                "ability registry full — rejecting registration"
            );
            return Err(format!("ability registry full (max {MAX_ABILITIES})"));
        }

        info!("Registering Ability: {} (v{})", manifest.name, manifest.version);
        guard.insert(manifest.id.clone(), ability.clone());
        drop(guard);

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

        let mut guard = self.skills.write().await;
        // Bounded collection: reject if at capacity.
        if guard.len() >= MAX_SKILLS && !guard.contains_key(&manifest.id) {
            warn!(
                name = manifest.name.as_str(),
                max = MAX_SKILLS,
                "skill registry full — rejecting registration"
            );
            return Err(format!("skill registry full (max {MAX_SKILLS})"));
        }

        info!("Registering Skill: {} (v{})", manifest.name, manifest.version);
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
