use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{info, warn, error};
use aura_types::extensions::{Ability, Lens, Recipe, Skill};
use aura_types::manifest::{
    CapabilityManifest, ExecutionTier, Permission, MAX_EXTENSION_CPU_PERCENT,
    MAX_EXTENSION_EXECUTION_MS, MAX_EXTENSION_MEMORY_MB, MAX_MANIFEST_PERMISSIONS,
};

use super::sandbox::{ExtensionSandbox, SandboxState, SandboxStats, MAX_SANDBOXED_EXTENSIONS};

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

// ─── Manifest Validation Errors ─────────────────────────────────────────────

/// Errors during extension registration and validation.
#[derive(Debug, Clone)]
pub enum LoaderError {
    /// Manifest failed security or resource validation.
    ManifestRejected(String),
    /// Extension capacity limit reached.
    RegistryFull { kind: &'static str, max: usize },
    /// Sandbox creation failed.
    SandboxError(String),
    /// Extension lifecycle error (on_load failed, etc.).
    LifecycleError(String),
    /// Permission validation failed.
    PermissionError(String),
}

impl std::fmt::Display for LoaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ManifestRejected(msg) => write!(f, "manifest rejected: {msg}"),
            Self::RegistryFull { kind, max } => {
                write!(f, "{kind} registry full (max {max})")
            }
            Self::SandboxError(msg) => write!(f, "sandbox error: {msg}"),
            Self::LifecycleError(msg) => write!(f, "lifecycle error: {msg}"),
            Self::PermissionError(msg) => write!(f, "permission error: {msg}"),
        }
    }
}

impl std::error::Error for LoaderError {}

// ─── ExtensionEntry ─────────────────────────────────────────────────────────

/// A registered extension paired with its sandbox.
#[derive(Debug)]
struct ExtensionEntry {
    sandbox: ExtensionSandbox,
    registered_at: Instant,
}

// ─── CapabilityLoader ───────────────────────────────────────────────────────

/// CapabilityLoader manages the loading, validation, sandboxing, and
/// lifecycle of AURA extensions.
///
/// Every extension goes through a validation pipeline before registration:
///
/// ```text
/// register_*()
///     │
///     ├── verify_manifest() — resource limits, permission caps, security policies
///     ├── validate_permissions() — tier restrictions, author restrictions
///     ├── ExtensionSandbox::new() — create sandbox with capability grants
///     ├── sandbox.activate() — transition to Active state
///     ├── (for Abilities) ability.start() — start background processing
///     │
///     └── ✅ Registered & sandboxed
/// ```
///
/// Extensions are compiled into the binary (Rust traits), not dynamically loaded.
/// This is safer for on-device execution — no untrusted binary loading.
pub struct CapabilityLoader {
    abilities: Arc<RwLock<HashMap<String, Arc<dyn Ability>>>>,
    skills: Arc<RwLock<HashMap<String, Arc<dyn Skill>>>>,
    lenses: Arc<RwLock<HashMap<String, Arc<dyn Lens>>>>,
    recipes: Arc<RwLock<HashMap<String, Recipe>>>,
    /// Sandbox entries keyed by extension ID.
    sandboxes: Arc<RwLock<HashMap<String, ExtensionEntry>>>,
}

impl CapabilityLoader {
    pub fn new() -> Self {
        Self {
            abilities: Arc::new(RwLock::new(HashMap::with_capacity(MAX_ABILITIES))),
            skills: Arc::new(RwLock::new(HashMap::with_capacity(MAX_SKILLS))),
            lenses: Arc::new(RwLock::new(HashMap::with_capacity(MAX_LENSES))),
            recipes: Arc::new(RwLock::new(HashMap::with_capacity(MAX_RECIPES))),
            sandboxes: Arc::new(RwLock::new(HashMap::with_capacity(MAX_SANDBOXED_EXTENSIONS))),
        }
    }

    /// Registers a new active Ability (background capability).
    ///
    /// Validates the manifest, creates a sandbox, activates it, and starts
    /// the ability's background processing. Returns `Err` if any step fails.
    pub async fn register_ability(&self, ability: Arc<dyn Ability>) -> Result<(), LoaderError> {
        let manifest = ability.manifest();
        self.full_manifest_validation(manifest)?;

        // Capacity check.
        let mut guard = self.abilities.write().await;
        if guard.len() >= MAX_ABILITIES && !guard.contains_key(&manifest.id) {
            warn!(
                name = manifest.name.as_str(),
                max = MAX_ABILITIES,
                "ability registry full — rejecting registration"
            );
            return Err(LoaderError::RegistryFull {
                kind: "ability",
                max: MAX_ABILITIES,
            });
        }

        // Create and activate sandbox.
        let mut sandbox = ExtensionSandbox::new(manifest)
            .map_err(LoaderError::SandboxError)?;
        sandbox.activate();

        info!(
            "Registered Ability: {} (v{}) [tier={:?}, perms={}]",
            manifest.name,
            manifest.version,
            manifest.execution_tier,
            manifest.permissions.len()
        );
        guard.insert(manifest.id.clone(), ability.clone());
        drop(guard);

        // Store sandbox.
        self.store_sandbox(manifest.id.clone(), sandbox).await;

        // Auto-start Abilities when registered.
        if let Err(e) = ability.start().await {
            error!("Failed to start ability {}: {}", manifest.name, e);
            // Suspend the sandbox on start failure.
            self.suspend_sandbox(&manifest.id, &format!("start failed: {e}"))
                .await;
        }

        Ok(())
    }

    /// Registers a new Skill (action capability).
    ///
    /// Validates the manifest and creates a sandbox. Skills are activated
    /// on-demand when `get_skill()` is called for execution.
    pub async fn register_skill(&self, skill: Arc<dyn Skill>) -> Result<(), LoaderError> {
        let manifest = skill.manifest();
        self.full_manifest_validation(manifest)?;

        // Capacity check.
        let mut guard = self.skills.write().await;
        if guard.len() >= MAX_SKILLS && !guard.contains_key(&manifest.id) {
            warn!(
                name = manifest.name.as_str(),
                max = MAX_SKILLS,
                "skill registry full — rejecting registration"
            );
            return Err(LoaderError::RegistryFull {
                kind: "skill",
                max: MAX_SKILLS,
            });
        }

        // Create and activate sandbox.
        let mut sandbox = ExtensionSandbox::new(manifest)
            .map_err(LoaderError::SandboxError)?;
        sandbox.activate();

        info!(
            "Registered Skill: {} (v{}) [tier={:?}, perms={}]",
            manifest.name,
            manifest.version,
            manifest.execution_tier,
            manifest.permissions.len()
        );
        guard.insert(manifest.id.clone(), skill);
        drop(guard);

        // Store sandbox.
        self.store_sandbox(manifest.id.clone(), sandbox).await;

        Ok(())
    }

    /// Registers a new Lens (perceptual filter).
    ///
    /// Lenses must be in `Functional` or `Observer` tier — they only
    /// transform context, never mutate state.
    pub async fn register_lens(
        &self,
        id: String,
        lens: Arc<dyn Lens>,
    ) -> Result<(), LoaderError> {
        let manifest = lens.manifest();
        self.full_manifest_validation(manifest)?;

        // Lenses must be Functional or Observer tier.
        if !matches!(
            manifest.execution_tier,
            ExecutionTier::Functional | ExecutionTier::Observer
        ) {
            return Err(LoaderError::PermissionError(format!(
                "Lens '{}' must be Functional or Observer tier (got {:?})",
                manifest.name, manifest.execution_tier
            )));
        }

        let mut guard = self.lenses.write().await;
        if guard.len() >= MAX_LENSES && !guard.contains_key(&id) {
            return Err(LoaderError::RegistryFull {
                kind: "lens",
                max: MAX_LENSES,
            });
        }

        let mut sandbox = ExtensionSandbox::new(manifest)
            .map_err(LoaderError::SandboxError)?;
        sandbox.activate();

        info!(
            "Registered Lens: {} (v{})",
            manifest.name, manifest.version
        );
        guard.insert(id.clone(), lens);
        drop(guard);

        self.store_sandbox(manifest.id.clone(), sandbox).await;
        Ok(())
    }

    /// Fetches a specific skill for execution.
    ///
    /// Returns `None` if the skill doesn't exist or its sandbox is not active.
    pub async fn get_skill(&self, id: &str) -> Option<Arc<dyn Skill>> {
        // Check sandbox is active before returning the skill.
        let sandbox_guard = self.sandboxes.read().await;
        if let Some(entry) = sandbox_guard.get(id) {
            if entry.sandbox.state() != SandboxState::Active {
                warn!(
                    skill_id = id,
                    state = %entry.sandbox.state(),
                    "skill sandbox not active — refusing access"
                );
                return None;
            }
        }
        drop(sandbox_guard);

        let guard = self.skills.read().await;
        guard.get(id).cloned()
    }

    /// Get a read-only snapshot of an extension's sandbox statistics.
    ///
    /// Returns `None` if the extension is not registered. The returned stats
    /// are a point-in-time clone — they do NOT update as the sandbox mutates.
    /// For permission enforcement, use [`check_extension_permission`] instead.
    pub async fn get_sandbox_stats(&self, extension_id: &str) -> Option<SandboxStats> {
        let guard = self.sandboxes.read().await;
        guard.get(extension_id).map(|e| e.sandbox.stats().clone())
    }

    /// Execute a permission check against an extension's sandbox.
    ///
    /// **This is the primary API for enforcing permissions at runtime.**
    /// It mutates the sandbox in-place — recording checks, denials, and
    /// violations — so that accumulated stats (including the auto-disable
    /// threshold at `MAX_VIOLATIONS_BEFORE_DISABLE`) work correctly.
    ///
    /// Returns `true` if the permission is granted, `false` otherwise.
    pub async fn check_extension_permission(
        &self,
        extension_id: &str,
        permission: &Permission,
    ) -> bool {
        let mut guard = self.sandboxes.write().await;
        if let Some(entry) = guard.get_mut(extension_id) {
            let result = entry.sandbox.check_permission(permission);
            result.allowed
        } else {
            warn!(
                extension_id = extension_id,
                "permission check for unknown extension — denied"
            );
            false
        }
    }

    /// Applies all active lenses to a raw input string (e.g., text from screen).
    pub async fn apply_lenses(&self, raw_input: &str) -> String {
        let guard = self.lenses.read().await;
        let mut processed = raw_input.to_string();
        for lens in guard.values() {
            processed = lens.process_context(&processed);
        }
        processed
    }

    /// Unregister an extension by ID, running its cleanup lifecycle.
    pub async fn unregister(&self, extension_id: &str) {
        // Remove sandbox.
        let mut sandbox_guard = self.sandboxes.write().await;
        if let Some(mut entry) = sandbox_guard.remove(extension_id) {
            entry.sandbox.deactivate();
            info!(extension_id = extension_id, "extension unregistered");
        }
        drop(sandbox_guard);

        // Remove from all registries.
        self.abilities.write().await.remove(extension_id);
        self.skills.write().await.remove(extension_id);
        self.lenses.write().await.remove(extension_id);
    }

    /// Get summary of all registered extensions and their sandbox states.
    pub async fn extension_summary(&self) -> Vec<ExtensionSummary> {
        let guard = self.sandboxes.read().await;
        guard
            .iter()
            .map(|(id, entry)| ExtensionSummary {
                id: id.clone(),
                name: entry.sandbox.context().extension_name.clone(),
                state: entry.sandbox.state(),
                permission_count: entry.sandbox.context().permission_count(),
                tier: entry.sandbox.context().execution_tier,
                total_checks: entry.sandbox.stats().total_checks,
                total_denials: entry.sandbox.stats().total_denials,
            })
            .collect()
    }

    /// Total number of registered extensions across all categories.
    pub async fn total_extensions(&self) -> usize {
        let a = self.abilities.read().await.len();
        let s = self.skills.read().await.len();
        let l = self.lenses.read().await.len();
        a + s + l
    }

    // -----------------------------------------------------------------------
    // Manifest validation pipeline
    // -----------------------------------------------------------------------

    /// Full manifest validation pipeline.
    ///
    /// Checks:
    /// 1. Resource limits against hard ceilings.
    /// 2. Permission count against maximum.
    /// 3. Permission compatibility with execution tier.
    /// 4. Author-based restrictions.
    /// 5. Sandbox capacity.
    fn full_manifest_validation(&self, manifest: &CapabilityManifest) -> Result<(), LoaderError> {
        // 1. Resource limits.
        if manifest.max_memory_mb > MAX_EXTENSION_MEMORY_MB {
            return Err(LoaderError::ManifestRejected(format!(
                "'{}' requests {}MB memory (max {}MB)",
                manifest.name, manifest.max_memory_mb, MAX_EXTENSION_MEMORY_MB
            )));
        }
        if manifest.max_cpu_percent > MAX_EXTENSION_CPU_PERCENT {
            return Err(LoaderError::ManifestRejected(format!(
                "'{}' requests {}% CPU (max {}%)",
                manifest.name, manifest.max_cpu_percent, MAX_EXTENSION_CPU_PERCENT
            )));
        }
        if manifest.max_execution_time_ms > MAX_EXTENSION_EXECUTION_MS {
            return Err(LoaderError::ManifestRejected(format!(
                "'{}' requests {}ms execution time (max {}ms)",
                manifest.name, manifest.max_execution_time_ms, MAX_EXTENSION_EXECUTION_MS
            )));
        }

        // 2. Permission count.
        if manifest.permissions.len() > MAX_MANIFEST_PERMISSIONS {
            return Err(LoaderError::ManifestRejected(format!(
                "'{}' declares {} permissions (max {})",
                manifest.name,
                manifest.permissions.len(),
                MAX_MANIFEST_PERMISSIONS
            )));
        }

        // 3. Tier-permission compatibility.
        self.validate_tier_permissions(manifest)?;

        // 4. Author restrictions.
        self.validate_author_permissions(manifest)?;

        // 5. ID and name validation.
        if manifest.id.is_empty() || manifest.name.is_empty() {
            return Err(LoaderError::ManifestRejected(
                "extension ID and name must be non-empty".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate that permissions are compatible with the execution tier.
    fn validate_tier_permissions(&self, manifest: &CapabilityManifest) -> Result<(), LoaderError> {
        for perm in &manifest.permissions {
            match manifest.execution_tier {
                ExecutionTier::Functional => {
                    if !matches!(
                        perm,
                        Permission::ReadMemoryDomain(_) | Permission::ObserveScreen
                    ) {
                        return Err(LoaderError::PermissionError(format!(
                            "Functional tier '{}' cannot request permission '{}'",
                            manifest.name, perm
                        )));
                    }
                }
                ExecutionTier::Observer => {
                    if matches!(
                        perm,
                        Permission::WriteMemory
                            | Permission::WriteSemanticMemory
                            | Permission::SendMessage
                            | Permission::ExecuteTools
                            | Permission::NetworkAccess
                    ) {
                        return Err(LoaderError::PermissionError(format!(
                            "Observer tier '{}' cannot request permission '{}'",
                            manifest.name, perm
                        )));
                    }
                }
                ExecutionTier::Advisor | ExecutionTier::Autonomous => {
                    // No tier restrictions for Advisor/Autonomous.
                }
            }
        }
        Ok(())
    }

    /// Validate author-based permission restrictions.
    fn validate_author_permissions(
        &self,
        manifest: &CapabilityManifest,
    ) -> Result<(), LoaderError> {
        if manifest.author == "AURA Core" {
            return Ok(()); // Core extensions are fully trusted.
        }

        for perm in &manifest.permissions {
            match perm {
                Permission::WriteSemanticMemory => {
                    return Err(LoaderError::PermissionError(format!(
                        "third-party '{}' cannot request WriteSemanticMemory",
                        manifest.name
                    )));
                }
                Permission::NetworkAccess => {
                    return Err(LoaderError::PermissionError(format!(
                        "third-party '{}' cannot request unrestricted NetworkAccess \
                         (use NetworkEgress with specific hosts)",
                        manifest.name
                    )));
                }
                _ => {}
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Sandbox management
    // -----------------------------------------------------------------------

    /// Store a sandbox entry.
    async fn store_sandbox(&self, id: String, sandbox: ExtensionSandbox) {
        let mut guard = self.sandboxes.write().await;
        guard.insert(
            id,
            ExtensionEntry {
                sandbox,
                registered_at: Instant::now(),
            },
        );
    }

    /// Suspend an extension's sandbox.
    async fn suspend_sandbox(&self, id: &str, reason: &str) {
        let mut guard = self.sandboxes.write().await;
        if let Some(entry) = guard.get_mut(id) {
            entry.sandbox.suspend(reason);
        }
    }
}

// ─── ExtensionSummary ───────────────────────────────────────────────────────

/// Summary of a registered extension for reporting.
#[derive(Debug, Clone)]
pub struct ExtensionSummary {
    pub id: String,
    pub name: String,
    pub state: SandboxState,
    pub permission_count: usize,
    pub tier: ExecutionTier,
    pub total_checks: u64,
    pub total_denials: u64,
}
