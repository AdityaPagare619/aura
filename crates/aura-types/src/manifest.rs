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
    pub permissions: Vec<Permission>,
    pub execution_tier: ExecutionTier,
    pub max_memory_mb: u32,
    pub max_cpu_percent: u8,
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Permission {
    /// Read memory within a specific domain
    ReadMemoryDomain(String),
    /// Observe screen context (accessibility service)
    ObserveScreen,
    /// Connect to a specific external host
    NetworkEgress(String),
    /// Use a specific core tool
    UseTool(String),
    /// Write to Semantic memory
    WriteSemanticMemory,
}
