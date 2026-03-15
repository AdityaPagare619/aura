use std::fmt;

use serde::{Deserialize, Serialize};

/// CapabilityManifest declares exactly what an extension can and cannot do.
/// AURA's ethics engine uses this to approve or deny installation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    /// Bounded at runtime to MAX_MANIFEST_PERMISSIONS entries — enforced at load site.
    pub permissions: Vec<Permission>,
    pub execution_tier: ExecutionTier,
    pub max_memory_mb: u32,
    pub max_cpu_percent: u8,
    /// Maximum execution time per invocation in milliseconds.
    /// 0 = no limit (only valid for AURA Core extensions).
    pub max_execution_time_ms: u64,
}

/// Maximum number of permissions a single [`CapabilityManifest`] may declare.
pub const MAX_MANIFEST_PERMISSIONS: usize = 32;

/// Hard ceiling on memory any single extension can request (256 MB).
/// Prevents a manifest from claiming gigabytes on a 4–8 GB mobile device.
pub const MAX_EXTENSION_MEMORY_MB: u32 = 256;

/// Hard ceiling on CPU any single extension can request (50%).
pub const MAX_EXTENSION_CPU_PERCENT: u8 = 50;

/// Hard ceiling on execution time any single extension can request (30 seconds).
pub const MAX_EXTENSION_EXECUTION_MS: u64 = 30_000;

/// Ethical gating parameter: determines the sandbox level.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecutionTier {
    /// Fully sandboxed WASM, purely functional, no side effects.
    Functional,
    /// Can read specific context but cannot act (Lenses).
    Observer,
    /// Can propose actions but requires explicit AURA + User approval (Skills).
    Advisor,
    /// Trusted to execute actions autonomously (Recipes).
    Autonomous,
}

/// Permissions that an extension can request.
///
/// Each variant represents a distinct capability. Extensions declare their
/// required permissions in their [`CapabilityManifest`]. The sandbox
/// enforces these grants at runtime — **deny by default**.
///
/// # Adding Permissions
///
/// When adding a new variant, also update:
/// 1. `ExtensionSandbox::check_permission()` in `aura-daemon`
/// 2. `Permission::as_policy_action()` so PolicyGate can evaluate it
/// 3. Integration tests for the new permission
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Permission {
    // --- Memory permissions ---
    /// Read memory within a specific domain (e.g., "contacts", "calendar").
    ReadMemoryDomain(String),
    /// Read from any memory store (semantic, episodic, working).
    ReadMemory,
    /// Write to Semantic memory (high-privilege — stores permanent knowledge).
    WriteSemanticMemory,
    /// Write to any memory store.
    WriteMemory,

    // --- Communication permissions ---
    /// Send messages via Telegram or other configured channels.
    SendMessage,

    // --- Tool permissions ---
    /// Use a specific core tool by ID.
    UseTool(String),
    /// Execute any available tool (broad — use sparingly).
    ExecuteTools,

    // --- Screen / UI permissions ---
    /// Observe screen context (accessibility service).
    ObserveScreen,
    /// Read accessibility tree data for automation.
    AccessScreen,

    // --- Network permissions ---
    /// Connect to a specific external host (scoped).
    NetworkEgress(String),
    /// General network access (VERY restricted — prefer `NetworkEgress`).
    NetworkAccess,
}

impl Permission {
    /// Convert this permission into a PolicyGate action string.
    ///
    /// This bridges the extension permission model with the PolicyGate
    /// rule evaluation engine. Each permission maps to a namespaced
    /// action string that PolicyGate rules can match against.
    #[must_use]
    pub fn as_policy_action(&self) -> String {
        match self {
            Self::ReadMemoryDomain(domain) => format!("ext:memory:read:{domain}"),
            Self::ReadMemory => "ext:memory:read:*".to_string(),
            Self::WriteSemanticMemory => "ext:memory:write:semantic".to_string(),
            Self::WriteMemory => "ext:memory:write:*".to_string(),
            Self::SendMessage => "ext:message:send".to_string(),
            Self::UseTool(tool_id) => format!("ext:tool:use:{tool_id}"),
            Self::ExecuteTools => "ext:tool:execute:*".to_string(),
            Self::ObserveScreen => "ext:screen:observe".to_string(),
            Self::AccessScreen => "ext:screen:access".to_string(),
            Self::NetworkEgress(host) => format!("ext:network:egress:{host}"),
            Self::NetworkAccess => "ext:network:access:*".to_string(),
        }
    }

    /// Sensitivity tier for this permission (0 = safe, 3 = critical).
    ///
    /// Used by the sandbox to determine how aggressively to audit/confirm.
    #[must_use]
    pub fn sensitivity(&self) -> u8 {
        match self {
            Self::ObserveScreen | Self::ReadMemoryDomain(_) => 1,
            Self::ReadMemory | Self::AccessScreen | Self::UseTool(_) => 2,
            Self::SendMessage | Self::ExecuteTools | Self::NetworkEgress(_) => 2,
            Self::WriteSemanticMemory | Self::WriteMemory | Self::NetworkAccess => 3,
        }
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadMemoryDomain(d) => write!(f, "ReadMemoryDomain({d})"),
            Self::ReadMemory => write!(f, "ReadMemory"),
            Self::WriteSemanticMemory => write!(f, "WriteSemanticMemory"),
            Self::WriteMemory => write!(f, "WriteMemory"),
            Self::SendMessage => write!(f, "SendMessage"),
            Self::UseTool(t) => write!(f, "UseTool({t})"),
            Self::ExecuteTools => write!(f, "ExecuteTools"),
            Self::ObserveScreen => write!(f, "ObserveScreen"),
            Self::AccessScreen => write!(f, "AccessScreen"),
            Self::NetworkEgress(h) => write!(f, "NetworkEgress({h})"),
            Self::NetworkAccess => write!(f, "NetworkAccess"),
        }
    }
}
