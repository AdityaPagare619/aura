pub mod actions;
pub mod anti_bot;
pub mod cache;
pub mod reader;
pub mod selector;
pub mod tree;
pub mod verifier;

pub use actions::{MockScreenProvider, ScreenProvider};
pub use anti_bot::AntiBot;
pub use cache::ScreenCache;
pub use reader::{detect_app_state, extract_screen_summary, AppState, ScreenSummary};
pub use selector::{resolve_target, ResolvedTarget};
pub use tree::{parse_tree, RawA11yNode};
pub use verifier::{verify_action, VerificationResult};
