pub mod discovery;
pub mod loader;
pub mod recipe;
pub mod sandbox;

pub use discovery::{
    CapabilityReport, ExtensionCapability, ExtensionDiscovery, GapSignal, RoutineSignal,
};
pub use loader::{CapabilityLoader, ExtensionSummary, LoaderError};
pub use recipe::RecipeTemplate;
pub use sandbox::{
    ExtensionContext, ExtensionSandbox, PermissionCheckResult, SandboxState, SandboxStats,
};
