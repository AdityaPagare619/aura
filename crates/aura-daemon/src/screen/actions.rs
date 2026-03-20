//! Screen interaction providers — the abstraction boundary for all screen control.
//!
//! All screen interaction goes through the `ScreenProvider` trait. This enables:
//! - `AndroidScreenProvider`: real device via AccessibilityService JNI
//! - `MockScreenProvider`: fully functional test double that loads fixture screen trees and
//!   simulates transitions by advancing through a `Vec<ScreenTree>`.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use aura_types::{
    actions::{ActionResult, ActionType},
    errors::ScreenError,
    screen::ScreenTree,
};

/// Maximum number of actions retained in the `MockScreenProvider` action log.
/// Prevents unbounded memory growth during long-running test scenarios.
const MAX_ACTION_LOG: usize = 1024;

/// The core abstraction for all screen interaction.
/// Every action AURA performs on the device goes through this trait.
pub trait ScreenProvider: Send + Sync {
    /// Capture the current accessibility tree.
    fn capture_tree(&self) -> Result<ScreenTree, ScreenError>;

    /// Execute an action on the device screen.
    fn execute_action(&self, action: &ActionType) -> Result<ActionResult, ScreenError>;

    /// Get the current foreground package name.
    fn foreground_package(&self) -> Result<String, ScreenError>;

    /// Health-check: is the accessibility service alive?
    fn is_alive(&self) -> bool;
}

// ── Mock Screen Provider ────────────────────────────────────────────────────

/// A fully functional mock screen provider for testing.
///
/// Loads a sequence of `ScreenTree` snapshots and simulates state transitions:
/// - Each `execute_action` call advances to the next tree in the sequence
/// - `capture_tree` returns the current tree
/// - Action results are configurable per action type
pub struct MockScreenProvider {
    /// Sequence of screen trees representing different screen states
    trees: Vec<ScreenTree>,
    /// Current index into the trees vector
    current_index: AtomicU64,
    /// Whether actions should succeed by default
    actions_succeed: bool,
    /// Simulated action duration in ms
    action_duration_ms: u32,
    /// Whether the provider is "alive"
    alive: bool,
    /// Actions that were executed (for assertions in tests)
    action_log: std::sync::Mutex<Vec<ActionType>>,
}

impl MockScreenProvider {
    /// Create a new mock with a sequence of screen trees.
    ///
    /// After each action, the provider advances to the next tree.
    /// When the end is reached, it stays on the last tree.
    pub fn new(trees: Vec<ScreenTree>) -> Self {
        assert!(
            !trees.is_empty(),
            "MockScreenProvider requires at least one tree"
        );
        Self {
            trees,
            current_index: AtomicU64::new(0),
            actions_succeed: true,
            action_duration_ms: 100,
            alive: true,
            action_log: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Create a mock with a single static tree (screen never changes).
    pub fn single(tree: ScreenTree) -> Self {
        Self::new(vec![tree])
    }

    /// Set whether actions should succeed.
    pub fn set_actions_succeed(&mut self, succeed: bool) {
        self.actions_succeed = succeed;
    }

    /// Set simulated action duration.
    pub fn set_action_duration_ms(&mut self, ms: u32) {
        self.action_duration_ms = ms;
    }

    /// Set whether the provider is alive.
    pub fn set_alive(&mut self, alive: bool) {
        self.alive = alive;
    }

    /// Get the current tree index.
    pub fn current_index(&self) -> usize {
        self.current_index.load(Ordering::SeqCst) as usize
    }

    /// Get all actions that were executed.
    pub fn action_log(&self) -> Result<Vec<ActionType>, ScreenError> {
        Ok(self
            .action_log
            .lock()
            .map_err(|_| ScreenError::ServiceDisconnected)?
            .clone())
    }

    /// Reset the provider to the first tree.
    pub fn reset(&self) -> Result<(), ScreenError> {
        self.current_index.store(0, Ordering::SeqCst);
        self.action_log
            .lock()
            .map_err(|_| ScreenError::ServiceDisconnected)?
            .clear();
        Ok(())
    }

    /// Advance to the next tree in the sequence.
    fn advance(&self) -> bool {
        let current = self.current_index.load(Ordering::SeqCst) as usize;
        if current + 1 < self.trees.len() {
            self.current_index
                .store((current + 1) as u64, Ordering::SeqCst);
            true // screen changed
        } else {
            false // already at last tree, no change
        }
    }
}

impl ScreenProvider for MockScreenProvider {
    fn capture_tree(&self) -> Result<ScreenTree, ScreenError> {
        if !self.alive {
            return Err(ScreenError::ServiceDisconnected);
        }
        let idx = self.current_index.load(Ordering::SeqCst) as usize;
        let idx = idx.min(self.trees.len() - 1);
        Ok(self.trees[idx].clone())
    }

    fn execute_action(&self, action: &ActionType) -> Result<ActionResult, ScreenError> {
        if !self.alive {
            return Err(ScreenError::ServiceDisconnected);
        }

        // Log the action (bounded to MAX_ACTION_LOG; oldest entry evicted on overflow)
        {
            let mut log = self
                .action_log
                .lock()
                .map_err(|_| ScreenError::ServiceDisconnected)?;
            if log.len() >= MAX_ACTION_LOG {
                log.remove(0); // evict oldest
            }
            log.push(action.clone());
        }

        if !self.actions_succeed {
            return Ok(ActionResult {
                success: false,
                duration_ms: self.action_duration_ms,
                error: Some("mock action failed".to_string()),
                screen_changed: false,
                matched_element: None,
            });
        }

        // Advance to next tree (simulates screen changing after action)
        let screen_changed = self.advance();

        Ok(ActionResult {
            success: true,
            duration_ms: self.action_duration_ms,
            error: None,
            screen_changed,
            matched_element: None,
        })
    }

    fn foreground_package(&self) -> Result<String, ScreenError> {
        if !self.alive {
            return Err(ScreenError::ServiceDisconnected);
        }
        let idx = self.current_index.load(Ordering::SeqCst) as usize;
        let idx = idx.min(self.trees.len() - 1);
        Ok(self.trees[idx].package_name.clone())
    }

    fn is_alive(&self) -> bool {
        self.alive
    }
}

// ── Android Screen Provider ─────────────────────────────────────────────────

/// Real screen provider that communicates with Android AccessibilityService via JNI.
///
/// On non-Android targets, all methods return `ServiceDisconnected` errors.
/// The actual JNI bindings are compiled only for `target_os = "android"`.
pub struct AndroidScreenProvider {
    /// Last known foreground package
    last_package: std::sync::Mutex<String>,
    /// Monotonic action counter for generating unique action IDs
    action_counter: AtomicU64,
}

impl AndroidScreenProvider {
    pub fn new() -> Self {
        Self {
            last_package: std::sync::Mutex::new(String::new()),
            action_counter: AtomicU64::new(0),
        }
    }
}

impl Default for AndroidScreenProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenProvider for AndroidScreenProvider {
    fn capture_tree(&self) -> Result<ScreenTree, ScreenError> {
        // On Android: call JNI to get the accessibility tree
        // On other platforms: return error
        #[cfg(target_os = "android")]
        {
            capture_tree_jni()
        }

        #[cfg(not(target_os = "android"))]
        {
            Err(ScreenError::ServiceDisconnected)
        }
    }

    fn execute_action(&self, action: &ActionType) -> Result<ActionResult, ScreenError> {
        let start = Instant::now();
        self.action_counter.fetch_add(1, Ordering::Relaxed);

        #[cfg(target_os = "android")]
        {
            let result = execute_action_jni(action)?;
            return Ok(result);
        }

        #[cfg(not(target_os = "android"))]
        {
            let _ = start;
            // Simulate action on non-Android (for development/testing)
            let duration_ms = match action {
                ActionType::Tap { .. } => 50,
                ActionType::LongPress { .. } => 200,
                ActionType::Swipe { duration_ms, .. } => *duration_ms,
                ActionType::Type { text } => (text.len() as u32) * 30 + 50,
                ActionType::Scroll { .. } => 150,
                ActionType::Back | ActionType::Home | ActionType::Recents => 50,
                ActionType::OpenApp { .. } => 1000,
                ActionType::NotificationAction { .. } => 100,
                ActionType::WaitForElement { timeout_ms, .. } => *timeout_ms / 10,
                ActionType::AssertElement { .. } => 10,
            };

            Ok(ActionResult {
                success: true,
                duration_ms,
                error: None,
                screen_changed: true,
                matched_element: None,
            })
        }
    }

    fn foreground_package(&self) -> Result<String, ScreenError> {
        #[cfg(target_os = "android")]
        {
            foreground_package_jni()
        }

        #[cfg(not(target_os = "android"))]
        {
            let pkg = self
                .last_package
                .lock()
                .map_err(|_| ScreenError::ServiceDisconnected)?
                .clone();
            if pkg.is_empty() {
                Err(ScreenError::ServiceDisconnected)
            } else {
                Ok(pkg)
            }
        }
    }

    fn is_alive(&self) -> bool {
        #[cfg(target_os = "android")]
        {
            is_alive_jni()
        }

        #[cfg(not(target_os = "android"))]
        {
            false
        }
    }
}

// ── JNI stubs for Android ───────────────────────────────────────────────────

#[cfg(target_os = "android")]
fn capture_tree_jni() -> Result<ScreenTree, ScreenError> {
    // Delegate to the centralised JNI bridge.
    let buf = crate::platform::jni_bridge::jni_get_screen_tree()
        .map_err(|_| ScreenError::TreeUnavailable)?;

    // Deserialize with bincode RC3 API.
    let (raw_nodes, _): (Vec<crate::screen::tree::RawA11yNode>, _) =
        bincode::serde::decode_from_slice(&buf, bincode::config::standard())
            .map_err(|_| ScreenError::TreeUnavailable)?;

    Ok(crate::screen::tree::parse_tree(&raw_nodes))
}

#[cfg(target_os = "android")]
fn execute_action_jni(action: &ActionType) -> Result<ActionResult, ScreenError> {
    use tracing::error;

    let start = Instant::now();

    // Dispatch to the centralised JNI bridge based on action type.
    let success = match action {
        ActionType::Tap { x, y } => {
            crate::platform::jni_bridge::jni_perform_tap(*x, *y).map_err(|e| {
                error!("JNI tap failed: {e}");
                ScreenError::ActionNotSupported(format!("{action:?}"))
            })?
        }
        ActionType::Swipe {
            from_x,
            from_y,
            to_x,
            to_y,
            duration_ms,
        } => crate::platform::jni_bridge::jni_perform_swipe(
            *from_x,
            *from_y,
            *to_x,
            *to_y,
            *duration_ms as i32,
        )
        .map_err(|e| {
            error!("JNI swipe failed: {e}");
            ScreenError::ActionNotSupported(format!("{action:?}"))
        })?,
        ActionType::Type { text } => {
            crate::platform::jni_bridge::jni_type_text(text).map_err(|e| {
                error!("JNI type failed: {e}");
                ScreenError::ActionNotSupported(format!("{action:?}"))
            })?
        }
        ActionType::Back => crate::platform::jni_bridge::jni_press_back().map_err(|e| {
            error!("JNI back failed: {e}");
            ScreenError::ActionNotSupported(format!("{action:?}"))
        })?,
        ActionType::Home => crate::platform::jni_bridge::jni_press_home().map_err(|e| {
            error!("JNI home failed: {e}");
            ScreenError::ActionNotSupported(format!("{action:?}"))
        })?,
        ActionType::Recents => crate::platform::jni_bridge::jni_press_recents().map_err(|e| {
            error!("JNI recents failed: {e}");
            ScreenError::ActionNotSupported(format!("{action:?}"))
        })?,
        _ => {
            // For actions not yet mapped to dedicated JNI calls, fall back to
            // the JSON-serialised generic path.
            execute_action_generic_jni(action)?
        }
    };

    let duration_ms = start.elapsed().as_millis() as u32;

    Ok(ActionResult {
        success,
        duration_ms,
        error: if success {
            None
        } else {
            Some("JNI action returned false".into())
        },
        screen_changed: success,
        matched_element: None,
    })
}

/// Generic JNI action execution via JSON serialization — fallback for action
/// types without dedicated bridge methods.
#[cfg(target_os = "android")]
fn execute_action_generic_jni(action: &ActionType) -> Result<bool, ScreenError> {
    use tracing::error;

    let mut env =
        crate::platform::jni_bridge::jni_env().map_err(|_| ScreenError::ServiceDisconnected)?;

    let cls = env
        .find_class("dev/aura/v4/AuraDaemonBridge")
        .map_err(|_| ScreenError::ServiceDisconnected)?;

    let action_json = serde_json::to_string(action)
        .map_err(|_| ScreenError::ActionNotSupported("serialization failed".into()))?;
    let j_action = env
        .new_string(&action_json)
        .map_err(|_| ScreenError::ServiceDisconnected)?;

    let result = env
        .call_static_method(
            cls,
            "executeAction",
            "(Ljava/lang/String;)Z",
            &[(&j_action).into()],
        )
        .map_err(|e| {
            error!("JNI executeAction failed: {e}");
            ScreenError::ActionNotSupported(format!("{action:?}"))
        })?;

    Ok(result.z().unwrap_or(false))
}

#[cfg(target_os = "android")]
fn foreground_package_jni() -> Result<String, ScreenError> {
    crate::platform::jni_bridge::jni_get_foreground_package()
        .map_err(|_| ScreenError::ServiceDisconnected)
}

#[cfg(target_os = "android")]
fn is_alive_jni() -> bool {
    crate::platform::jni_bridge::jni_is_service_alive()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use aura_types::screen::{Bounds, ScreenNode};

    use super::*;

    fn make_test_tree(package: &str, text: &str) -> ScreenTree {
        ScreenTree {
            root: ScreenNode {
                id: "root".into(),
                class_name: "android.widget.FrameLayout".into(),
                package_name: package.into(),
                text: Some(text.into()),
                content_description: None,
                resource_id: None,
                bounds: Bounds {
                    left: 0,
                    top: 0,
                    right: 1080,
                    bottom: 1920,
                },
                is_clickable: false,
                is_scrollable: false,
                is_editable: false,
                is_checkable: false,
                is_checked: false,
                is_enabled: true,
                is_focused: false,
                is_visible: true,
                children: vec![],
                depth: 0,
            },
            package_name: package.into(),
            activity_name: ".MainActivity".into(),
            timestamp_ms: 1_700_000_000_000,
            node_count: 1,
        }
    }

    #[test]
    fn test_mock_provider_single_tree() {
        let tree = make_test_tree("com.test", "Screen 1");
        let provider = MockScreenProvider::single(tree);

        assert!(provider.is_alive());

        let captured = provider.capture_tree().unwrap();
        assert_eq!(captured.package_name, "com.test");

        // Action should succeed but screen shouldn't change (only one tree)
        let result = provider
            .execute_action(&ActionType::Tap { x: 100, y: 200 })
            .unwrap();
        assert!(result.success);
        assert!(!result.screen_changed); // only one tree, can't advance
    }

    #[test]
    fn test_mock_provider_multi_tree_transitions() {
        let trees = vec![
            make_test_tree("com.test", "Screen 1"),
            make_test_tree("com.test", "Screen 2"),
            make_test_tree("com.test", "Screen 3"),
        ];
        let provider = MockScreenProvider::new(trees);

        // Start at tree 0
        let t0 = provider.capture_tree().unwrap();
        assert_eq!(t0.root.text.as_deref(), Some("Screen 1"));

        // Tap → advance to tree 1
        let r1 = provider
            .execute_action(&ActionType::Tap { x: 0, y: 0 })
            .unwrap();
        assert!(r1.success);
        assert!(r1.screen_changed);

        let t1 = provider.capture_tree().unwrap();
        assert_eq!(t1.root.text.as_deref(), Some("Screen 2"));

        // Tap again → advance to tree 2
        let r2 = provider.execute_action(&ActionType::Back).unwrap();
        assert!(r2.success);
        assert!(r2.screen_changed);

        let t2 = provider.capture_tree().unwrap();
        assert_eq!(t2.root.text.as_deref(), Some("Screen 3"));

        // Tap again → already at last tree, no change
        let r3 = provider.execute_action(&ActionType::Home).unwrap();
        assert!(r3.success);
        assert!(!r3.screen_changed);
    }

    #[test]
    fn test_mock_provider_action_log() {
        let tree = make_test_tree("com.test", "Screen 1");
        let provider = MockScreenProvider::single(tree);

        provider
            .execute_action(&ActionType::Tap { x: 100, y: 200 })
            .unwrap();
        provider.execute_action(&ActionType::Back).unwrap();
        provider
            .execute_action(&ActionType::Type {
                text: "hello".into(),
            })
            .unwrap();

        let log = provider.action_log().unwrap();
        assert_eq!(log.len(), 3);
        assert!(matches!(log[0], ActionType::Tap { x: 100, y: 200 }));
        assert!(matches!(log[1], ActionType::Back));
    }

    #[test]
    fn test_mock_provider_fails_when_not_alive() {
        let tree = make_test_tree("com.test", "Screen 1");
        let mut provider = MockScreenProvider::single(tree);
        provider.set_alive(false);

        assert!(!provider.is_alive());
        assert!(provider.capture_tree().is_err());
        assert!(provider.execute_action(&ActionType::Back).is_err());
    }

    #[test]
    fn test_mock_provider_action_failure() {
        let tree = make_test_tree("com.test", "Screen 1");
        let mut provider = MockScreenProvider::single(tree);
        provider.set_actions_succeed(false);

        let result = provider
            .execute_action(&ActionType::Tap { x: 0, y: 0 })
            .unwrap();
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_mock_provider_reset() {
        let trees = vec![
            make_test_tree("com.test", "Screen 1"),
            make_test_tree("com.test", "Screen 2"),
        ];
        let provider = MockScreenProvider::new(trees);

        // Advance
        provider.execute_action(&ActionType::Back).unwrap();
        assert_eq!(provider.current_index(), 1);

        // Reset
        provider.reset().unwrap();
        assert_eq!(provider.current_index(), 0);
        assert!(provider.action_log().unwrap().is_empty());
    }

    #[test]
    fn test_mock_provider_foreground_package() {
        let trees = vec![
            make_test_tree("com.app1", "Screen 1"),
            make_test_tree("com.app2", "Screen 2"),
        ];
        let provider = MockScreenProvider::new(trees);

        assert_eq!(provider.foreground_package().unwrap(), "com.app1");

        provider.execute_action(&ActionType::Back).unwrap();
        assert_eq!(provider.foreground_package().unwrap(), "com.app2");
    }

    #[test]
    fn test_android_provider_not_alive_on_non_android() {
        let provider = AndroidScreenProvider::new();
        assert!(!provider.is_alive());
    }
}
