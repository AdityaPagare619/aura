use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Unified error hierarchy for all AURA subsystems.
#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum AuraError {
    #[error("IPC error: {0}")]
    Ipc(#[from] IpcError),

    #[error("Execution error: {0}")]
    Exec(#[from] ExecError),

    #[error("Memory error: {0}")]
    Memory(#[from] MemError),

    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),

    #[error("Platform error: {0}")]
    Platform(#[from] PlatformError),

    #[error("Config error: {0}")]
    Config(#[from] ConfigError),

    #[error("Screen error: {0}")]
    Screen(#[from] ScreenError),

    #[error("Identity error: {0}")]
    Identity(#[from] IdentityError),

    #[error("Goal error: {0}")]
    Goal(#[from] GoalError),

    #[error("Onboarding error: {0}")]
    Onboarding(#[from] OnboardingError),

    #[error("Security error: {0}")]
    Security(#[from] SecurityError),
}

/// Errors from the onboarding / first-run subsystem.
#[derive(Error, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OnboardingError {
    #[error("onboarding phase failed: {phase} — {reason}")]
    PhaseFailed { phase: String, reason: String },

    #[error("onboarding state corrupted: {0}")]
    StateCorrupted(String),

    #[error("onboarding already completed")]
    AlreadyCompleted,

    #[error("calibration failed: {0}")]
    CalibrationFailed(String),

    #[error("tutorial step failed: {step} — {reason}")]
    TutorialStepFailed { step: String, reason: String },

    #[error("user profile error: {0}")]
    ProfileError(String),

    #[error("onboarding persistence failed: {0}")]
    PersistenceFailed(String),

    #[error("onboarding interrupted at phase: {0}")]
    Interrupted(String),
}

/// Errors from the goal management subsystem.
#[derive(Error, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GoalError {
    #[error("goal not found: {0}")]
    NotFound(u64),

    #[error("goal capacity exceeded: max {max}")]
    CapacityExceeded { max: usize },

    #[error("invalid state transition: {from} → {to}")]
    InvalidTransition { from: String, to: String },

    #[error("goal already exists: {0}")]
    AlreadyExists(u64),

    #[error("decomposition failed: {0}")]
    DecompositionFailed(String),

    #[error("no matching capability for: {0}")]
    NoCapability(String),

    #[error("goal deadline exceeded: goal {goal_id}")]
    DeadlineExceeded { goal_id: u64 },

    #[error("max retries exhausted: goal {goal_id}, attempts {attempts}")]
    RetriesExhausted { goal_id: u64, attempts: u8 },

    #[error("dependency cycle detected: {0}")]
    DependencyCycle(String),

    #[error("scheduler full: {active} active goals")]
    SchedulerFull { active: usize },
}

/// Errors from the IPC layer between daemon and neocortex.
#[derive(Error, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IpcError {
    #[error("connection failed")]
    ConnectionFailed,

    #[error("operation timed out")]
    Timeout,

    #[error("message too large: {0} bytes")]
    MessageTooLarge(usize),

    #[error("failed to deserialize message")]
    DeserializeFailed,

    #[error("child process died")]
    ProcessDied,
}

/// Errors from the execution engine (ETG + DSL runner).
#[derive(Error, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecError {
    #[error("step {step} timed out on action '{action}'")]
    StepTimeout { step: u32, action: String },

    #[error("cycle detected in tier {tier}")]
    CycleDetected { tier: u8 },

    #[error("max steps exceeded: {0}")]
    MaxStepsExceeded(u32),

    #[error("target not found: selector '{selector}'")]
    TargetNotFound { selector: String },

    #[error("action '{action}' failed: {reason}")]
    ActionFailed { action: String, reason: String },

    #[error("app not installed: {0}")]
    AppNotInstalled(String),
}

/// Errors from the memory subsystem.
#[derive(Error, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemError {
    #[error("database corrupted")]
    DatabaseCorrupted,

    #[error("storage full")]
    StorageFull,

    #[error("consolidation failed: {0}")]
    ConsolidationFailed(String),

    #[error("embedding generation failed")]
    EmbeddingFailed,

    #[error("query failed: {0}")]
    QueryFailed(String),

    #[error("serialization failed: {0}")]
    SerializationFailed(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("database error: {0}")]
    DatabaseError(String),

    #[error("schema migration failed: {0}")]
    MigrationFailed(String),
}

/// Errors from LLM inference.
#[derive(Error, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LlmError {
    #[error("model not loaded")]
    ModelNotLoaded,

    #[error("inference failed: {0}")]
    InferenceFailed(String),

    #[error("token budget exhausted")]
    TokenBudgetExhausted,

    #[error("context too large: {size} tokens (max {max})")]
    ContextTooLarge { size: u32, max: u32 },

    #[error("model load failed: {0}")]
    ModelLoadFailed(String),
}

/// Errors from platform-specific operations.
#[derive(Error, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PlatformError {
    #[error("accessibility service not running")]
    AccessibilityNotRunning,

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("unsupported API level: {0}")]
    UnsupportedApiLevel(u32),

    #[error("root not available")]
    RootNotAvailable,

    #[error("battery read failed: {0}")]
    BatteryReadFailed(String),

    #[error("thermal read failed: {0}")]
    ThermalReadFailed(String),

    #[error("notification post failed: {0}")]
    NotificationFailed(String),

    #[error("wakelock acquire failed: {0}")]
    WakelockFailed(String),

    #[error("doze state unknown: {0}")]
    DozeStateUnknown(String),

    #[error("JNI call failed: {0}")]
    JniFailed(String),

    #[error("callback capacity exceeded: max {max}")]
    CallbackCapacityExceeded { max: usize },

    #[error("sensor read failed: {0}")]
    SensorReadFailed(String),

    #[error("connectivity read failed: {0}")]
    ConnectivityReadFailed(String),
}

/// Errors from configuration parsing/validation.
#[derive(Error, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConfigError {
    #[error("invalid config: {field} — {reason}")]
    InvalidField { field: String, reason: String },

    #[error("config file not found: {0}")]
    FileNotFound(String),

    #[error("parse error: {0}")]
    ParseError(String),
}

/// Errors from the screen capture/tree subsystem.
#[derive(Error, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ScreenError {
    #[error("accessibility tree unavailable")]
    TreeUnavailable,

    #[error("element not found: selector '{selector}'")]
    ElementNotFound { selector: String },

    #[error("action not supported: {0}")]
    ActionNotSupported(String),

    #[error("accessibility service disconnected")]
    ServiceDisconnected,
}

/// Errors from identity/personality subsystem.
#[derive(Error, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IdentityError {
    #[error("trait value out of range: {trait_name}={value}")]
    TraitOutOfRange { trait_name: String, value: String },

    #[error("mood update too frequent (cooldown active)")]
    CooldownActive,

    #[error("personality snapshot corrupted")]
    SnapshotCorrupted,
}

/// Errors from the security subsystem (audit, sandbox, emergency).
#[derive(Error, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SecurityError {
    #[error("audit log corrupted: {0}")]
    AuditCorrupted(String),

    #[error("audit log full: capacity {capacity}")]
    AuditFull { capacity: usize },

    #[error("hash chain tampered at entry {index}")]
    HashChainTampered { index: u64 },

    #[error("sandbox refused action: {reason}")]
    SandboxRefused { reason: String },

    #[error("sandbox resource limit exceeded: {resource}")]
    ResourceLimitExceeded { resource: String },

    #[error("rollback failed: {reason}")]
    RollbackFailed { reason: String },

    #[error("emergency stop active: {reason}")]
    EmergencyActive { reason: String },

    #[error("watchdog timeout: no heartbeat for {elapsed_ms}ms")]
    WatchdogTimeout { elapsed_ms: u64 },

    #[error("anomaly detected: {description}")]
    AnomalyDetected { description: String },

    #[error("recovery failed: {reason}")]
    RecoveryFailed { reason: String },

    #[error("containment level violation: action requires L{required}, got L{actual}")]
    ContainmentViolation { required: u8, actual: u8 },

    #[error("audit query failed: {0}")]
    AuditQueryFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_messages() {
        let ipc = IpcError::MessageTooLarge(65536);
        assert_eq!(ipc.to_string(), "message too large: 65536 bytes");

        let exec = ExecError::StepTimeout {
            step: 3,
            action: "Tap".to_string(),
        };
        assert_eq!(exec.to_string(), "step 3 timed out on action 'Tap'");

        let screen = ScreenError::ElementNotFound {
            selector: "resource_id:btn_ok".to_string(),
        };
        assert_eq!(
            screen.to_string(),
            "element not found: selector 'resource_id:btn_ok'"
        );
    }

    #[test]
    fn test_mem_error_variants() {
        let err = MemError::QueryFailed("timeout".to_string());
        assert_eq!(err.to_string(), "query failed: timeout");

        let err2 = MemError::DatabaseError("locked".to_string());
        assert_eq!(err2.to_string(), "database error: locked");

        let err3 = MemError::NotFound("episode 42".to_string());
        assert_eq!(err3.to_string(), "not found: episode 42");
    }

    #[test]
    fn test_aura_error_from_sub_errors() {
        let ipc_err = IpcError::Timeout;
        let aura_err: AuraError = ipc_err.into();
        assert!(matches!(aura_err, AuraError::Ipc(IpcError::Timeout)));

        let mem_err = MemError::StorageFull;
        let aura_err: AuraError = mem_err.into();
        assert!(matches!(aura_err, AuraError::Memory(MemError::StorageFull)));

        let exec_err = ExecError::MaxStepsExceeded(200);
        let aura_err: AuraError = exec_err.into();
        assert!(matches!(
            aura_err,
            AuraError::Exec(ExecError::MaxStepsExceeded(200))
        ));
    }

    #[test]
    fn test_error_clone_and_debug() {
        let err = AuraError::Screen(ScreenError::TreeUnavailable);
        let cloned = err.clone();
        let debug_str = format!("{:?}", cloned);
        assert!(debug_str.contains("TreeUnavailable"));
    }

    #[test]
    fn test_security_error_variants() {
        let err = SecurityError::SandboxRefused {
            reason: "forbidden action".to_string(),
        };
        assert_eq!(err.to_string(), "sandbox refused action: forbidden action");

        let err2 = SecurityError::EmergencyActive {
            reason: "loop detected".to_string(),
        };
        assert_eq!(err2.to_string(), "emergency stop active: loop detected");

        let err3 = SecurityError::HashChainTampered { index: 42 };
        assert_eq!(err3.to_string(), "hash chain tampered at entry 42");

        let aura_err: AuraError = err3.into();
        assert!(matches!(
            aura_err,
            AuraError::Security(SecurityError::HashChainTampered { index: 42 })
        ));
    }
}
