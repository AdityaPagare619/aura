//! Extension Sandbox — capability-based permission enforcement.
//!
//! Every extension action routes through the sandbox before execution.
//! The sandbox enforces:
//!
//! 1. **Permission grants** — only explicitly granted permissions are allowed.
//! 2. **Resource limits** — memory, CPU, execution time per extension.
//! 3. **PolicyGate integration** — extension actions are evaluated as policy actions.
//! 4. **Audit trail** — every permission check is loggable.
//!
//! # Architecture
//!
//! ```text
//! Extension Action
//!       │
//!       ▼
//! ┌─────────────────┐
//! │ ExtensionSandbox │
//! │                   │
//! │ 1. check_permission() ── Is this permission granted?
//! │ 2. check_resource_limits() ── Within budget?
//! │ 3. PolicyGate::evaluate() ── Does policy allow this?
//! │ 4. audit_log() ── Record the decision
//! │                   │
//! └───────┬───────────┘
//!         │
//!     ✅ ALLOW  or  ❌ DENY
//! ```
//!
//! # Design Principles
//!
//! - **Deny by default**: Extensions get NO permissions unless explicitly granted.
//! - **Least privilege**: Grant minimum permissions needed.
//! - **Fail closed**: If permission check fails, deny (never fall through).
//! - **Auditable**: Every check can be traced.
//! - **No runtime code loading**: Extensions are compiled in (Rust traits).

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use aura_types::{
    extensions::ExtensionError,
    manifest::{
        CapabilityManifest, ExecutionTier, Permission, MAX_EXTENSION_CPU_PERCENT,
        MAX_EXTENSION_EXECUTION_MS, MAX_EXTENSION_MEMORY_MB, MAX_MANIFEST_PERMISSIONS,
    },
};
use tracing::{debug, info, warn};

use crate::policy::{PolicyGate, RuleEffect};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of extensions that can be sandboxed simultaneously.
pub const MAX_SANDBOXED_EXTENSIONS: usize = 64;

/// Maximum number of permission check violations before an extension
/// is forcibly disabled (suspicious behavior detection).
const MAX_VIOLATIONS_BEFORE_DISABLE: u32 = 10;

// ---------------------------------------------------------------------------
// ExtensionContext
// ---------------------------------------------------------------------------

/// Runtime context provided to a sandboxed extension.
///
/// Contains the granted permissions, resource limits, and metadata needed
/// for the extension to operate within its sandbox boundaries.
///
/// # Usage
///
/// ```ignore
/// let ctx = ExtensionContext::from_manifest(&manifest)?;
/// assert!(ctx.has_permission(&Permission::ReadMemory));
/// assert!(!ctx.has_permission(&Permission::NetworkAccess)); // not granted
/// ```
#[derive(Debug, Clone)]
pub struct ExtensionContext {
    /// The extension's unique identifier.
    pub extension_id: String,
    /// Human-readable name.
    pub extension_name: String,
    /// The set of permissions explicitly granted to this extension.
    granted_permissions: HashSet<Permission>,
    /// Maximum memory this extension may use (bytes).
    pub max_memory_bytes: u64,
    /// Maximum CPU percentage this extension may consume.
    pub max_cpu_percent: u8,
    /// Maximum execution time per invocation (milliseconds).
    pub max_execution_time_ms: u64,
    /// The execution tier determining sandbox strictness.
    pub execution_tier: ExecutionTier,
    /// Author identifier (used for trust decisions).
    pub author: String,
}

impl ExtensionContext {
    /// Build an `ExtensionContext` from a validated manifest.
    ///
    /// Returns `Err` if the manifest violates hard limits (e.g., requests
    /// more memory than `MAX_EXTENSION_MEMORY_MB`).
    pub fn from_manifest(manifest: &CapabilityManifest) -> Result<Self, String> {
        // Validate permission count.
        if manifest.permissions.len() > MAX_MANIFEST_PERMISSIONS {
            return Err(format!(
                "manifest '{}' declares {} permissions (max {})",
                manifest.name,
                manifest.permissions.len(),
                MAX_MANIFEST_PERMISSIONS
            ));
        }

        // Validate resource limits against hard ceilings.
        if manifest.max_memory_mb > MAX_EXTENSION_MEMORY_MB {
            return Err(format!(
                "manifest '{}' requests {}MB memory (max {}MB)",
                manifest.name, manifest.max_memory_mb, MAX_EXTENSION_MEMORY_MB
            ));
        }
        if manifest.max_cpu_percent > MAX_EXTENSION_CPU_PERCENT {
            return Err(format!(
                "manifest '{}' requests {}% CPU (max {}%)",
                manifest.name, manifest.max_cpu_percent, MAX_EXTENSION_CPU_PERCENT
            ));
        }
        if manifest.max_execution_time_ms > MAX_EXTENSION_EXECUTION_MS {
            return Err(format!(
                "manifest '{}' requests {}ms execution time (max {}ms)",
                manifest.name, manifest.max_execution_time_ms, MAX_EXTENSION_EXECUTION_MS
            ));
        }

        Ok(Self {
            extension_id: manifest.id.clone(),
            extension_name: manifest.name.clone(),
            granted_permissions: manifest.permissions.iter().cloned().collect(),
            max_memory_bytes: (manifest.max_memory_mb as u64) * 1024 * 1024,
            max_cpu_percent: manifest.max_cpu_percent,
            max_execution_time_ms: manifest.max_execution_time_ms,
            execution_tier: manifest.execution_tier,
            author: manifest.author.clone(),
        })
    }

    /// Check whether a specific permission has been granted.
    #[must_use]
    pub fn has_permission(&self, perm: &Permission) -> bool {
        self.granted_permissions.contains(perm)
    }

    /// Get all granted permissions.
    #[must_use]
    pub fn granted_permissions(&self) -> &HashSet<Permission> {
        &self.granted_permissions
    }

    /// Number of granted permissions.
    #[must_use]
    pub fn permission_count(&self) -> usize {
        self.granted_permissions.len()
    }
}

// ---------------------------------------------------------------------------
// PermissionCheckResult
// ---------------------------------------------------------------------------

/// Outcome of a permission check within the sandbox.
#[derive(Debug, Clone)]
pub struct PermissionCheckResult {
    /// Whether the permission was granted.
    pub allowed: bool,
    /// The permission that was checked.
    pub permission: Permission,
    /// Which extension requested it.
    pub extension_id: String,
    /// Why the decision was made.
    pub reason: String,
}

impl PermissionCheckResult {
    /// Create an allowed result.
    fn allow(permission: Permission, extension_id: String) -> Self {
        Self {
            allowed: true,
            permission,
            extension_id,
            reason: "permission granted".to_string(),
        }
    }

    /// Create a denied result.
    fn deny(permission: Permission, extension_id: String, reason: String) -> Self {
        Self {
            allowed: false,
            permission,
            extension_id,
            reason,
        }
    }
}

// ---------------------------------------------------------------------------
// SandboxState
// ---------------------------------------------------------------------------

/// Runtime state of a sandboxed extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxState {
    /// Extension is registered but not yet loaded.
    Registered,
    /// Extension is loaded and active.
    Active,
    /// Extension is temporarily suspended (e.g., resource violation).
    Suspended,
    /// Extension has been unloaded.
    Unloaded,
    /// Extension was disabled due to repeated violations.
    Disabled,
}

impl std::fmt::Display for SandboxState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Registered => write!(f, "registered"),
            Self::Active => write!(f, "active"),
            Self::Suspended => write!(f, "suspended"),
            Self::Unloaded => write!(f, "unloaded"),
            Self::Disabled => write!(f, "disabled"),
        }
    }
}

// ---------------------------------------------------------------------------
// SandboxStats
// ---------------------------------------------------------------------------

/// Aggregate statistics for a sandboxed extension.
#[derive(Debug, Clone, Default)]
pub struct SandboxStats {
    /// Total permission checks performed.
    pub total_checks: u64,
    /// Total permission denials.
    pub total_denials: u64,
    /// Total successful executions.
    pub total_executions: u64,
    /// Total execution time across all invocations (ms).
    pub total_execution_time_ms: u64,
    /// Number of policy violations (denied by PolicyGate, not just permissions).
    pub policy_violations: u32,
}

// ---------------------------------------------------------------------------
// ExtensionSandbox
// ---------------------------------------------------------------------------

/// Wraps an extension with capability-based permission enforcement.
///
/// Every action the extension attempts is validated against:
/// 1. The extension's granted permissions (from manifest).
/// 2. Resource limits (memory, CPU, execution time).
/// 3. PolicyGate rules (if integrated).
///
/// # Security Model
///
/// ```text
/// Extension.execute(input)
///     │
///     ├── check_permission(ExecuteTools) → DENY if not granted
///     ├── check_resource_limits() → DENY if over budget
///     ├── PolicyGate.evaluate("ext:tool:execute:*") → DENY if policy blocks
///     │
///     └── ✅ Proceed with execution
/// ```
///
/// The sandbox never grants permissions not in the manifest.
/// The sandbox never allows actions that PolicyGate denies.
/// The sandbox never exceeds resource limits.
#[derive(Debug)]
pub struct ExtensionSandbox {
    /// The runtime context (permissions + limits).
    context: ExtensionContext,
    /// Current state of this sandbox.
    state: SandboxState,
    /// Running statistics.
    stats: SandboxStats,
    /// Number of consecutive violations (triggers disable at threshold).
    violation_count: u32,
    /// When the extension was loaded.
    loaded_at: Option<Instant>,
    /// Optional PolicyGate for action-level policy evaluation.
    policy_gate: Option<PolicyGate>,
}

impl ExtensionSandbox {
    /// Create a new sandbox from a validated manifest.
    ///
    /// The extension starts in `Registered` state. Call `activate()` after
    /// successful `on_load()` to transition to `Active`.
    pub fn new(manifest: &CapabilityManifest) -> Result<Self, String> {
        let context = ExtensionContext::from_manifest(manifest)?;

        info!(
            extension = context.extension_name.as_str(),
            permissions = context.permission_count(),
            tier = ?context.execution_tier,
            "sandbox created"
        );

        Ok(Self {
            context,
            state: SandboxState::Registered,
            stats: SandboxStats::default(),
            violation_count: 0,
            loaded_at: None,
            policy_gate: None,
        })
    }

    /// Transition to Active state after successful on_load().
    pub fn activate(&mut self) {
        self.state = SandboxState::Active;
        self.loaded_at = Some(Instant::now());
        info!(
            extension = self.context.extension_name.as_str(),
            "sandbox activated"
        );
    }

    /// Transition to Unloaded state after on_unload().
    pub fn deactivate(&mut self) {
        self.state = SandboxState::Unloaded;
        info!(
            extension = self.context.extension_name.as_str(),
            "sandbox deactivated"
        );
    }

    /// Attach a PolicyGate for action-level policy evaluation.
    ///
    /// When set, `check_permission()` will also evaluate the action
    /// against the PolicyGate rules after grant/tier/author checks pass.
    pub fn set_policy_gate(&mut self, gate: PolicyGate) {
        self.policy_gate = Some(gate);
    }

    /// Suspend the extension (e.g., due to resource violation).
    pub fn suspend(&mut self, reason: &str) {
        self.state = SandboxState::Suspended;
        warn!(
            extension = self.context.extension_name.as_str(),
            reason = reason,
            "sandbox suspended"
        );
    }

    // -----------------------------------------------------------------------
    // Permission checking
    // -----------------------------------------------------------------------

    /// Check whether the extension has a specific permission.
    ///
    /// This is the primary security gate. Every extension action must
    /// call this before proceeding. Returns a `PermissionCheckResult`
    /// that can be inspected and logged.
    ///
    /// # Deny Conditions
    ///
    /// - Extension is not in `Active` state.
    /// - Permission is not in the granted set.
    /// - Extension has been disabled due to repeated violations.
    ///
    /// # Audit
    ///
    /// Every check is counted in `SandboxStats`. Denials increment
    /// the violation counter. At `MAX_VIOLATIONS_BEFORE_DISABLE`,
    /// the extension is forcibly disabled.
    pub fn check_permission(&mut self, permission: &Permission) -> PermissionCheckResult {
        self.stats.total_checks += 1;
        let ext_id = self.context.extension_id.clone();

        // Gate: must be active.
        if self.state != SandboxState::Active {
            self.record_violation();
            return PermissionCheckResult::deny(
                permission.clone(),
                ext_id,
                format!("extension is {} (must be active)", self.state),
            );
        }

        // Gate: permission must be explicitly granted.
        if !self.context.has_permission(permission) {
            self.record_violation();
            let result = PermissionCheckResult::deny(
                permission.clone(),
                ext_id,
                format!("permission '{}' not granted", permission),
            );
            warn!(
                extension = self.context.extension_name.as_str(),
                permission = %permission,
                violation_count = self.violation_count,
                "permission denied"
            );
            return result;
        }

        // Tier-based restrictions.
        if let Some(reason) = self.check_tier_restriction(permission) {
            self.record_violation();
            return PermissionCheckResult::deny(permission.clone(), ext_id, reason);
        }

        // Third-party restrictions.
        if let Some(reason) = self.check_author_restriction(permission) {
            self.record_violation();
            return PermissionCheckResult::deny(permission.clone(), ext_id, reason);
        }

        // PolicyGate integration: evaluate action against policy rules.
        let action = self.policy_action_for(permission);
        if let Some(ref mut gate) = self.policy_gate {
            let decision = gate.evaluate(&action);
            if matches!(decision.effect, RuleEffect::Deny) {
                self.stats.policy_violations += 1;
                self.record_violation();
                return PermissionCheckResult::deny(
                    permission.clone(),
                    ext_id,
                    format!("PolicyGate denied: {}", decision.reason),
                );
            }
        }

        debug!(
            extension = self.context.extension_name.as_str(),
            permission = %permission,
            "permission granted"
        );

        PermissionCheckResult::allow(permission.clone(), ext_id)
    }

    /// Build the PolicyGate action string for a given permission.
    ///
    /// This bridges extension permissions with PolicyGate evaluation.
    /// Call `PolicyGate::evaluate()` with this string to get a policy decision.
    ///
    /// Returns the action string formatted as: `ext:{extension_id}:{permission_action}`
    #[must_use]
    pub fn policy_action_for(&self, permission: &Permission) -> String {
        format!(
            "ext:{}:{}",
            self.context.extension_id,
            permission.as_policy_action()
        )
    }

    // -----------------------------------------------------------------------
    // Resource limit checking
    // -----------------------------------------------------------------------

    /// Validate that the extension hasn't exceeded its execution time limit.
    ///
    /// Call this periodically during long-running operations to enforce
    /// the `max_execution_time_ms` limit from the manifest.
    #[must_use]
    pub fn check_execution_time(&self, started_at: Instant) -> Result<(), ExtensionError> {
        if self.context.max_execution_time_ms == 0 {
            return Ok(()); // No limit (AURA Core only).
        }

        let elapsed = started_at.elapsed();
        let limit = Duration::from_millis(self.context.max_execution_time_ms);

        if elapsed > limit {
            return Err(ExtensionError::ExecutionTimeout {
                limit_ms: self.context.max_execution_time_ms,
            });
        }

        Ok(())
    }

    /// Record that an execution completed, tracking time for stats.
    pub fn record_execution(&mut self, duration_ms: u64) {
        self.stats.total_executions += 1;
        self.stats.total_execution_time_ms = self
            .stats
            .total_execution_time_ms
            .saturating_add(duration_ms);
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Get the sandbox context.
    #[must_use]
    pub fn context(&self) -> &ExtensionContext {
        &self.context
    }

    /// Get the current sandbox state.
    #[must_use]
    pub fn state(&self) -> SandboxState {
        self.state
    }

    /// Get aggregate statistics.
    #[must_use]
    pub fn stats(&self) -> &SandboxStats {
        &self.stats
    }

    /// How long this extension has been loaded.
    #[must_use]
    pub fn uptime(&self) -> Option<Duration> {
        self.loaded_at.map(|t| t.elapsed())
    }

    /// Whether this extension is currently active and usable.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state == SandboxState::Active
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Record a permission violation and potentially disable the extension.
    fn record_violation(&mut self) {
        self.stats.total_denials += 1;
        self.violation_count += 1;

        if self.violation_count >= MAX_VIOLATIONS_BEFORE_DISABLE {
            warn!(
                extension = self.context.extension_name.as_str(),
                violations = self.violation_count,
                "extension disabled due to repeated violations"
            );
            self.state = SandboxState::Disabled;
        }
    }

    /// Tier-based permission restrictions.
    ///
    /// - `Functional` tier: only ReadMemoryDomain, ObserveScreen allowed.
    /// - `Observer` tier: no write, send, execute, or network permissions.
    fn check_tier_restriction(&self, permission: &Permission) -> Option<String> {
        match self.context.execution_tier {
            ExecutionTier::Functional => {
                // Functional extensions can only read specific domains.
                match permission {
                    Permission::ReadMemoryDomain(_) | Permission::ObserveScreen => None,
                    other => Some(format!("Functional tier cannot use permission '{other}'")),
                }
            },
            ExecutionTier::Observer => {
                // Observer extensions cannot write, send, execute, or network.
                match permission {
                    Permission::WriteMemory
                    | Permission::WriteSemanticMemory
                    | Permission::SendMessage
                    | Permission::ExecuteTools
                    | Permission::NetworkAccess
                    | Permission::NetworkEgress(_) => Some(format!(
                        "Observer tier cannot use permission '{permission}'"
                    )),
                    _ => None,
                }
            },
            ExecutionTier::Advisor | ExecutionTier::Autonomous => None,
        }
    }

    /// Author-based permission restrictions.
    ///
    /// Third-party extensions (author != "AURA Core") face additional
    /// restrictions on dangerous permissions.
    fn check_author_restriction(&self, permission: &Permission) -> Option<String> {
        if self.context.author == "AURA Core" {
            return None; // Core extensions are fully trusted.
        }

        match permission {
            Permission::WriteSemanticMemory => {
                Some("third-party extensions cannot write to semantic memory".to_string())
            },
            Permission::NetworkAccess => Some(
                "third-party extensions cannot have unrestricted network access \
                 (use NetworkEgress with specific hosts instead)"
                    .to_string(),
            ),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use aura_types::manifest::{CapabilityManifest, ExecutionTier, Permission};

    use super::*;

    fn core_manifest() -> CapabilityManifest {
        CapabilityManifest {
            id: "core.test".to_string(),
            name: "Test Core Extension".to_string(),
            version: "1.0.0".to_string(),
            author: "AURA Core".to_string(),
            description: "Test extension".to_string(),
            permissions: vec![
                Permission::ReadMemory,
                Permission::WriteMemory,
                Permission::SendMessage,
                Permission::ExecuteTools,
            ],
            execution_tier: ExecutionTier::Autonomous,
            max_memory_mb: 64,
            max_cpu_percent: 20,
            max_execution_time_ms: 5000,
        }
    }

    fn third_party_manifest() -> CapabilityManifest {
        CapabilityManifest {
            id: "3p.weather".to_string(),
            name: "Weather Skill".to_string(),
            version: "0.1.0".to_string(),
            author: "ThirdPartyDev".to_string(),
            description: "Weather lookup".to_string(),
            permissions: vec![
                Permission::ReadMemory,
                Permission::NetworkEgress("api.weather.com".to_string()),
            ],
            execution_tier: ExecutionTier::Advisor,
            max_memory_mb: 16,
            max_cpu_percent: 10,
            max_execution_time_ms: 5000,
        }
    }

    fn functional_manifest() -> CapabilityManifest {
        CapabilityManifest {
            id: "func.lens".to_string(),
            name: "Code Lens".to_string(),
            version: "0.1.0".to_string(),
            author: "AURA Core".to_string(),
            description: "Extracts code blocks".to_string(),
            permissions: vec![
                Permission::ObserveScreen,
                Permission::ReadMemoryDomain("code".to_string()),
            ],
            execution_tier: ExecutionTier::Functional,
            max_memory_mb: 8,
            max_cpu_percent: 5,
            max_execution_time_ms: 1000,
        }
    }

    #[test]
    fn test_sandbox_creation() {
        let sandbox = ExtensionSandbox::new(&core_manifest()).unwrap();
        assert_eq!(sandbox.state(), SandboxState::Registered);
        assert!(!sandbox.is_active());
    }

    #[test]
    fn test_sandbox_activation() {
        let mut sandbox = ExtensionSandbox::new(&core_manifest()).unwrap();
        sandbox.activate();
        assert_eq!(sandbox.state(), SandboxState::Active);
        assert!(sandbox.is_active());
    }

    #[test]
    fn test_permission_denied_when_not_active() {
        let mut sandbox = ExtensionSandbox::new(&core_manifest()).unwrap();
        // Still in Registered state — should deny.
        let result = sandbox.check_permission(&Permission::ReadMemory);
        assert!(!result.allowed);
        assert!(result.reason.contains("must be active"));
    }

    #[test]
    fn test_permission_granted_when_active() {
        let mut sandbox = ExtensionSandbox::new(&core_manifest()).unwrap();
        sandbox.activate();
        let result = sandbox.check_permission(&Permission::ReadMemory);
        assert!(result.allowed);
    }

    #[test]
    fn test_permission_denied_when_not_granted() {
        let mut sandbox = ExtensionSandbox::new(&core_manifest()).unwrap();
        sandbox.activate();
        // NetworkAccess is not in the core manifest.
        let result = sandbox.check_permission(&Permission::NetworkAccess);
        assert!(!result.allowed);
        assert!(result.reason.contains("not granted"));
    }

    #[test]
    fn test_third_party_cannot_write_semantic_memory() {
        let mut manifest = third_party_manifest();
        manifest.permissions.push(Permission::WriteSemanticMemory);
        let mut sandbox = ExtensionSandbox::new(&manifest).unwrap();
        sandbox.activate();
        let result = sandbox.check_permission(&Permission::WriteSemanticMemory);
        assert!(!result.allowed);
        assert!(result.reason.contains("third-party"));
    }

    #[test]
    fn test_third_party_cannot_unrestricted_network() {
        let mut manifest = third_party_manifest();
        manifest.permissions.push(Permission::NetworkAccess);
        let mut sandbox = ExtensionSandbox::new(&manifest).unwrap();
        sandbox.activate();
        let result = sandbox.check_permission(&Permission::NetworkAccess);
        assert!(!result.allowed);
        assert!(result.reason.contains("unrestricted network"));
    }

    #[test]
    fn test_third_party_can_scoped_network() {
        let mut sandbox = ExtensionSandbox::new(&third_party_manifest()).unwrap();
        sandbox.activate();
        let result =
            sandbox.check_permission(&Permission::NetworkEgress("api.weather.com".to_string()));
        assert!(result.allowed);
    }

    #[test]
    fn test_functional_tier_restrictions() {
        let mut sandbox = ExtensionSandbox::new(&functional_manifest()).unwrap();
        sandbox.activate();

        // Allowed: ObserveScreen, ReadMemoryDomain.
        let r1 = sandbox.check_permission(&Permission::ObserveScreen);
        assert!(r1.allowed);

        let r2 = sandbox.check_permission(&Permission::ReadMemoryDomain("code".to_string()));
        assert!(r2.allowed);
    }

    #[test]
    fn test_functional_tier_denies_write() {
        let mut manifest = functional_manifest();
        manifest.permissions.push(Permission::WriteMemory);
        let mut sandbox = ExtensionSandbox::new(&manifest).unwrap();
        sandbox.activate();
        let result = sandbox.check_permission(&Permission::WriteMemory);
        assert!(!result.allowed);
        assert!(result.reason.contains("Functional tier"));
    }

    #[test]
    fn test_violation_tracking_disables_extension() {
        let mut sandbox = ExtensionSandbox::new(&core_manifest()).unwrap();
        sandbox.activate();

        // Trigger MAX_VIOLATIONS_BEFORE_DISABLE denials.
        for _ in 0..MAX_VIOLATIONS_BEFORE_DISABLE {
            sandbox.check_permission(&Permission::NetworkAccess);
        }

        assert_eq!(sandbox.state(), SandboxState::Disabled);
        assert!(!sandbox.is_active());
    }

    #[test]
    fn test_manifest_exceeds_memory_limit() {
        let mut manifest = core_manifest();
        manifest.max_memory_mb = MAX_EXTENSION_MEMORY_MB + 1;
        let result = ExtensionSandbox::new(&manifest);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("memory"));
    }

    #[test]
    fn test_manifest_exceeds_cpu_limit() {
        let mut manifest = core_manifest();
        manifest.max_cpu_percent = MAX_EXTENSION_CPU_PERCENT + 1;
        let result = ExtensionSandbox::new(&manifest);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("CPU"));
    }

    #[test]
    fn test_manifest_exceeds_execution_time() {
        let mut manifest = core_manifest();
        manifest.max_execution_time_ms = MAX_EXTENSION_EXECUTION_MS + 1;
        let result = ExtensionSandbox::new(&manifest);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("execution time"));
    }

    #[test]
    fn test_manifest_exceeds_permission_count() {
        let mut manifest = core_manifest();
        manifest.permissions = (0..MAX_MANIFEST_PERMISSIONS + 1)
            .map(|i| Permission::ReadMemoryDomain(format!("domain_{i}")))
            .collect();
        let result = ExtensionSandbox::new(&manifest);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("permissions"));
    }

    #[test]
    fn test_execution_time_tracking() {
        let mut sandbox = ExtensionSandbox::new(&core_manifest()).unwrap();
        sandbox.activate();

        let started = Instant::now();
        // Immediately check — should be within limit.
        assert!(sandbox.check_execution_time(started).is_ok());

        sandbox.record_execution(100);
        assert_eq!(sandbox.stats().total_executions, 1);
        assert_eq!(sandbox.stats().total_execution_time_ms, 100);
    }

    #[test]
    fn test_policy_action_string() {
        let sandbox = ExtensionSandbox::new(&core_manifest()).unwrap();
        let action = sandbox.policy_action_for(&Permission::ReadMemory);
        assert_eq!(action, "ext:core.test:ext:memory:read:*");
    }

    #[test]
    fn test_context_from_manifest() {
        let manifest = core_manifest();
        let ctx = ExtensionContext::from_manifest(&manifest).unwrap();
        assert_eq!(ctx.extension_id, "core.test");
        assert!(ctx.has_permission(&Permission::ReadMemory));
        assert!(!ctx.has_permission(&Permission::NetworkAccess));
        assert_eq!(ctx.permission_count(), 4);
        assert_eq!(ctx.max_memory_bytes, 64 * 1024 * 1024);
    }

    #[test]
    fn test_sandbox_suspend_and_deny() {
        let mut sandbox = ExtensionSandbox::new(&core_manifest()).unwrap();
        sandbox.activate();

        let r1 = sandbox.check_permission(&Permission::ReadMemory);
        assert!(r1.allowed);

        sandbox.suspend("test suspension");
        assert_eq!(sandbox.state(), SandboxState::Suspended);

        let r2 = sandbox.check_permission(&Permission::ReadMemory);
        assert!(!r2.allowed);
    }

    // -----------------------------------------------------------------------
    // PolicyGate integration tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_policy_gate_deny_blocks_permitted_action() {
        use crate::policy::{PolicyGate, PolicyRule, RuleEffect};

        let mut sandbox = ExtensionSandbox::new(&core_manifest()).unwrap();
        sandbox.activate();

        // Confirm ReadMemory is normally allowed.
        let r = sandbox.check_permission(&Permission::ReadMemory);
        assert!(r.allowed);

        // Attach a PolicyGate that denies all memory actions for extensions.
        let mut gate = PolicyGate::allow_all();
        gate.add_rule(PolicyRule {
            name: "block-ext-memory".to_string(),
            action_pattern: "*memory*".to_string(),
            effect: RuleEffect::Deny,
            reason: "memory access blocked by policy".to_string(),
            priority: 0,
        });
        sandbox.set_policy_gate(gate);

        // Now ReadMemory should be denied by PolicyGate even though
        // the permission is granted in the manifest.
        let r = sandbox.check_permission(&Permission::ReadMemory);
        assert!(!r.allowed);
        assert!(r.reason.contains("PolicyGate denied"));
        assert!(r.reason.contains("memory access blocked by policy"));
        assert_eq!(sandbox.stats().policy_violations, 1);
    }

    #[test]
    fn test_policy_gate_absent_allows_normal_flow() {
        // No PolicyGate set — check_permission works exactly as before.
        let mut sandbox = ExtensionSandbox::new(&core_manifest()).unwrap();
        sandbox.activate();

        let r = sandbox.check_permission(&Permission::ReadMemory);
        assert!(r.allowed);

        let r = sandbox.check_permission(&Permission::WriteMemory);
        assert!(r.allowed);

        // Ungrated permission still denied.
        let r = sandbox.check_permission(&Permission::NetworkAccess);
        assert!(!r.allowed);
        assert!(r.reason.contains("not granted"));

        // No policy violations recorded.
        assert_eq!(sandbox.stats().policy_violations, 0);
    }

    #[test]
    fn test_policy_gate_violation_counts_toward_disable() {
        use crate::policy::{PolicyGate, PolicyRule, RuleEffect};

        let mut sandbox = ExtensionSandbox::new(&core_manifest()).unwrap();
        sandbox.activate();

        // Attach a PolicyGate that denies ReadMemory actions.
        let mut gate = PolicyGate::allow_all();
        gate.add_rule(PolicyRule {
            name: "block-ext-memory".to_string(),
            action_pattern: "*memory*".to_string(),
            effect: RuleEffect::Deny,
            reason: "blocked".to_string(),
            priority: 0,
        });
        sandbox.set_policy_gate(gate);

        // Trigger policy denials up to the disable threshold.
        for _ in 0..MAX_VIOLATIONS_BEFORE_DISABLE {
            let r = sandbox.check_permission(&Permission::ReadMemory);
            assert!(!r.allowed);
        }

        // Extension should now be disabled.
        assert_eq!(sandbox.state(), SandboxState::Disabled);
        assert!(!sandbox.is_active());
        assert_eq!(
            sandbox.stats().policy_violations,
            MAX_VIOLATIONS_BEFORE_DISABLE,
        );
    }
}
