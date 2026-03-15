use crate::manifest::{CapabilityManifest, Permission};
use async_trait::async_trait;
use serde_json::Value;
use std::fmt;

// ---------------------------------------------------------------------------
// Extension Error
// ---------------------------------------------------------------------------

/// Errors that extensions can produce during lifecycle or execution.
#[derive(Debug, Clone)]
pub enum ExtensionError {
    /// Extension failed to initialize.
    InitializationFailed(String),
    /// Extension was denied a required permission.
    PermissionDenied {
        permission: Permission,
        reason: String,
    },
    /// Extension exceeded its resource limits.
    ResourceLimitExceeded(String),
    /// Extension execution timed out.
    ExecutionTimeout { limit_ms: u64 },
    /// General runtime error within the extension.
    RuntimeError(String),
    /// Extension is in an invalid state for the requested operation.
    InvalidState(String),
}

impl fmt::Display for ExtensionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InitializationFailed(msg) => write!(f, "init failed: {msg}"),
            Self::PermissionDenied { permission, reason } => {
                write!(f, "permission denied ({permission}): {reason}")
            }
            Self::ResourceLimitExceeded(msg) => write!(f, "resource limit: {msg}"),
            Self::ExecutionTimeout { limit_ms } => {
                write!(f, "execution timeout ({limit_ms}ms)")
            }
            Self::RuntimeError(msg) => write!(f, "runtime error: {msg}"),
            Self::InvalidState(msg) => write!(f, "invalid state: {msg}"),
        }
    }
}

impl std::error::Error for ExtensionError {}

// ---------------------------------------------------------------------------
// Extension Trait (base)
// ---------------------------------------------------------------------------

/// The foundational trait for all AURA extensions.
///
/// Extensions are not generic "plugins" — they are categorized by their
/// cognitive function (Skill, Ability, Lens). Every extension MUST declare
/// a [`CapabilityManifest`] listing its permissions, resource limits, and
/// identity.
///
/// # Lifecycle
///
/// ```text
/// new() → on_load() → [on_message() | execute()] → on_unload()
/// ```
///
/// - `on_load()` is called once when the extension is registered.
/// - `on_message()` handles incoming events/messages.
/// - `on_unload()` is called when the extension is deregistered.
///
/// # Permissions
///
/// Extensions declare required permissions via `required_permissions()`.
/// The sandbox validates these against the manifest and PolicyGate
/// before granting access. **Deny by default** — if a permission isn't
/// explicitly granted, the action is blocked.
///
/// # Developer Guide
///
/// To create a new extension:
///
/// 1. Implement this trait (and optionally `Skill`, `Ability`, or `Lens`).
/// 2. Define a `CapabilityManifest` listing your ID, permissions, and limits.
/// 3. Return your required permissions from `required_permissions()`.
/// 4. Handle lifecycle events in `on_load()` / `on_unload()`.
/// 5. Process messages in `on_message()`.
/// 6. Register with `CapabilityLoader::register_*()`.
///
/// ```ignore
/// struct MySkill { /* state */ }
///
/// #[async_trait]
/// impl Extension for MySkill {
///     fn manifest(&self) -> &CapabilityManifest { &self.manifest }
///     fn name(&self) -> &str { "my-skill" }
///     fn version(&self) -> &str { "0.1.0" }
///     fn required_permissions(&self) -> Vec<Permission> {
///         vec![Permission::ReadMemory, Permission::UseTool("web_search".into())]
///     }
///     async fn on_load(&mut self) -> Result<(), ExtensionError> { Ok(()) }
///     async fn on_message(&mut self, msg: &str) -> Result<Option<String>, ExtensionError> {
///         Ok(Some(format!("processed: {msg}")))
///     }
///     async fn on_unload(&mut self) -> Result<(), ExtensionError> { Ok(()) }
///     async fn is_healthy(&self) -> bool { true }
/// }
/// ```
#[async_trait]
pub trait Extension: Send + Sync {
    /// The ethical contract explaining what this extension does.
    fn manifest(&self) -> &CapabilityManifest;

    /// Human-readable name of the extension.
    fn name(&self) -> &str {
        &self.manifest().name
    }

    /// Semantic version string.
    fn version(&self) -> &str {
        &self.manifest().version
    }

    /// Permissions this extension requires to function.
    ///
    /// These are validated against the manifest's `permissions` field
    /// during registration. The sandbox will deny any permission not
    /// listed here AND in the manifest.
    fn required_permissions(&self) -> Vec<Permission> {
        self.manifest().permissions.clone()
    }

    /// Called once when the extension is loaded into the daemon.
    ///
    /// Use this for initialization: opening connections, loading state,
    /// validating configuration. Return `Err` to abort registration.
    async fn on_load(&mut self) -> Result<(), ExtensionError> {
        Ok(())
    }

    /// Handle an incoming message or event.
    ///
    /// Returns `Ok(Some(response))` if the extension produced output,
    /// `Ok(None)` if the message was consumed silently, or `Err` on failure.
    ///
    /// Note: The sandbox enforces permission checks BEFORE this is called.
    /// Extensions should not need to check permissions themselves.
    async fn on_message(&mut self, _msg: &str) -> Result<Option<String>, ExtensionError> {
        Ok(None)
    }

    /// Called when the extension is being unloaded.
    ///
    /// Use this for cleanup: closing connections, flushing state,
    /// releasing resources. Errors here are logged but do not prevent unload.
    async fn on_unload(&mut self) -> Result<(), ExtensionError> {
        Ok(())
    }

    /// Health check — returns true if the extension is functioning correctly.
    async fn is_healthy(&self) -> bool;
}

// ---------------------------------------------------------------------------
// Skill
// ---------------------------------------------------------------------------

/// A **Skill** is an active capability. It lets AURA *do* something new.
/// (e.g., Code Execution, Email Drafting, Calendar Management)
///
/// Skills are the primary way to extend AURA's action space.
/// Each execution goes through the permission sandbox and PolicyGate.
#[async_trait]
pub trait Skill: Extension {
    /// Execute the skill with the given input parameters.
    ///
    /// The sandbox enforces:
    /// - Permission checks (does the extension have `ExecuteTools`?)
    /// - Resource limits (memory, CPU, execution time)
    /// - PolicyGate evaluation (is this action allowed?)
    async fn execute(&self, input: Value) -> Result<Value, ExtensionError>;

    /// JSON Schema describing the expected input format.
    fn usage_schema(&self) -> Value;
}

// ---------------------------------------------------------------------------
// Ability
// ---------------------------------------------------------------------------

/// An **Ability** is a passive edge capability. It runs constantly in the background.
/// (e.g., Audio processing, Bluetooth proximity, Step counting)
///
/// Abilities are long-running and consume resources continuously.
/// The sandbox enforces stricter resource limits on Abilities.
#[async_trait]
pub trait Ability: Extension {
    /// Start the background processing loop.
    async fn start(&self) -> Result<(), ExtensionError>;

    /// Stop the background processing loop.
    async fn stop(&self) -> Result<(), ExtensionError>;

    /// Get the current state snapshot (for the LLM to reason about).
    fn get_current_state(&self) -> Value;
}

// ---------------------------------------------------------------------------
// Lens
// ---------------------------------------------------------------------------

/// A **Lens** is a perceptual filter. It changes *how* AURA views existing data.
/// (e.g., "Developer Lens" extracts code blocks from screen context)
///
/// Lenses are pure transformations — they read context but never mutate state.
/// They should only require `ObserveScreen` or `ReadMemory` permissions.
pub trait Lens: Extension {
    /// Transform raw context through this lens.
    fn process_context(&self, raw_context: &str) -> String;
}

// ---------------------------------------------------------------------------
// Recipe
// ---------------------------------------------------------------------------

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
