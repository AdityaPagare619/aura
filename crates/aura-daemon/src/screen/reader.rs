//! Screen summary extraction and AppState detection heuristics.
//!
//! Extracts a compact summary of the current screen for the LLM/Neocortex,
//! and detects what "state" the app is in (loading, error dialog, permission prompt, etc.).

use aura_types::screen::{ScreenNode, ScreenTree};
use serde::{Deserialize, Serialize};

/// Compact summary of the current screen for LLM context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenSummary {
    /// Package name of the foreground app.
    pub package_name: String,
    /// Activity name (if known).
    pub activity_name: String,
    /// All visible text on screen, deduplicated.
    pub visible_text: Vec<String>,
    /// Number of interactive (clickable) elements.
    pub clickable_count: u32,
    /// Number of editable (text input) elements.
    pub editable_count: u32,
    /// Number of scrollable containers.
    pub scrollable_count: u32,
    /// Whether a keyboard is likely visible (heuristic).
    pub keyboard_visible: bool,
    /// Detected app state.
    pub app_state: AppState,
    /// Total node count in the tree.
    pub node_count: u32,
}

/// Detected application state based on accessibility tree heuristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppState {
    /// Normal interactive state.
    Normal,
    /// App appears to be loading (spinner/progress visible).
    Loading,
    /// An error dialog is shown.
    ErrorDialog,
    /// A permission prompt from the system is shown.
    PermissionPrompt,
    /// A system dialog (not from the target app) is in the foreground.
    SystemDialog,
    /// App has crashed (crash dialog visible or tree is empty).
    Crashed,
    /// Keyboard is covering most of the screen (input mode).
    InputMode,
    /// Screen appears empty or minimal (possible transition state).
    EmptyOrTransition,
    /// Login / authentication screen detected.
    LoginScreen,
    /// An overlay or popup is blocking the main content.
    OverlayBlocking,
    /// State is not yet determined; LLM will classify from raw tree data.
    Unknown,
}

/// Extract a compact screen summary from the accessibility tree.
pub fn extract_screen_summary(tree: &ScreenTree) -> ScreenSummary {
    let mut visible_text = Vec::new();
    let mut clickable_count = 0u32;
    let mut editable_count = 0u32;
    let mut scrollable_count = 0u32;
    let mut keyboard_visible = false;

    collect_summary_stats(
        &tree.root,
        &mut visible_text,
        &mut clickable_count,
        &mut editable_count,
        &mut scrollable_count,
        &mut keyboard_visible,
    );

    // Deduplicate text while preserving order
    let mut seen = std::collections::HashSet::new();
    visible_text.retain(|t| seen.insert(t.clone()));

    let app_state = detect_app_state(tree);

    ScreenSummary {
        package_name: tree.package_name.clone(),
        activity_name: tree.activity_name.clone(),
        visible_text,
        clickable_count,
        editable_count,
        scrollable_count,
        keyboard_visible,
        app_state,
        node_count: tree.node_count,
    }
}

/// Detect the current app state from the screen tree.
///
/// Iron Law: Rust does NOT do NLP keyword matching or semantic inference.
/// The LLM classifies app state from raw screen data. Rust returns Unknown.
pub fn detect_app_state(_tree: &ScreenTree) -> AppState {
    AppState::Unknown
}

// ── Heuristic helpers ───────────────────────────────────────────────────────

fn collect_summary_stats(
    node: &ScreenNode,
    visible_text: &mut Vec<String>,
    clickable: &mut u32,
    editable: &mut u32,
    scrollable: &mut u32,
    keyboard_visible: &mut bool,
) {
    if node.is_visible {
        if let Some(ref text) = node.text {
            if !text.is_empty() && text.len() < 200 {
                visible_text.push(text.clone());
            }
        }
        if node.is_clickable {
            *clickable += 1;
        }
        if node.is_editable {
            *editable += 1;
        }
        if node.is_scrollable {
            *scrollable += 1;
        }
    }

    // Keyboard detection: look for input method package or typical keyboard class names
    if node.package_name.contains("inputmethod")
        || node.package_name.contains("keyboard")
        || node.class_name.contains("KeyboardView")
        || node.class_name.contains("InputView")
    {
        *keyboard_visible = true;
    }

    for child in &node.children {
        collect_summary_stats(
            child,
            visible_text,
            clickable,
            editable,
            scrollable,
            keyboard_visible,
        );
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use aura_types::screen::Bounds;

    fn make_node(
        id: &str,
        class: &str,
        text: Option<&str>,
        clickable: bool,
        editable: bool,
        children: Vec<ScreenNode>,
    ) -> ScreenNode {
        ScreenNode {
            id: id.into(),
            class_name: class.into(),
            package_name: "com.test".into(),
            text: text.map(|s| s.into()),
            content_description: None,
            resource_id: None,
            bounds: Bounds {
                left: 100,
                top: 200,
                right: 300,
                bottom: 400,
            },
            is_clickable: clickable,
            is_scrollable: false,
            is_editable: editable,
            is_checkable: false,
            is_checked: false,
            is_enabled: true,
            is_focused: false,
            is_visible: true,
            children,
            depth: 0,
        }
    }

    fn make_tree_with_root(mut root: ScreenNode, package: &str) -> ScreenTree {
        fn count_nodes(node: &ScreenNode) -> u32 {
            1 + node.children.iter().map(|c| count_nodes(c)).sum::<u32>()
        }
        // Root always gets full-screen bounds (children keep their smaller defaults)
        root.bounds = Bounds {
            left: 0,
            top: 0,
            right: 1080,
            bottom: 1920,
        };
        let count = count_nodes(&root);
        ScreenTree {
            root,
            package_name: package.into(),
            activity_name: ".Main".into(),
            timestamp_ms: 1_700_000_000_000,
            node_count: count,
        }
    }

    #[test]
    fn test_normal_app_state() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node("btn", "Button", Some("OK"), true, false, vec![]),
                make_node("txt", "TextView", Some("Hello world"), false, false, vec![]),
            ],
        );
        let tree = make_tree_with_root(root, "com.test");

        let summary = extract_screen_summary(&tree);
        assert_eq!(summary.app_state, AppState::Unknown);
        assert_eq!(summary.clickable_count, 1);
        assert_eq!(summary.visible_text.len(), 2);
    }

    #[test]
    fn test_empty_tree_crashed() {
        let tree = ScreenTree {
            root: make_node("empty", "FrameLayout", None, false, false, vec![]),
            package_name: "com.test".into(),
            activity_name: String::new(),
            timestamp_ms: 0,
            node_count: 0,
        };
        // detect_app_state always returns Unknown — LLM classifies app state.
        assert_eq!(detect_app_state(&tree), AppState::Unknown);
    }

    #[test]
    fn test_permission_prompt_detection() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![make_node(
                "btn",
                "Button",
                Some("Allow"),
                true,
                false,
                vec![],
            )],
        );
        let tree = make_tree_with_root(root, "com.google.android.permissioncontroller");
        // detect_app_state always returns Unknown — LLM classifies app state from raw tree.
        assert_eq!(detect_app_state(&tree), AppState::Unknown);
    }

    #[test]
    fn test_crash_dialog_detection() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node(
                    "msg",
                    "TextView",
                    Some("App has stopped"),
                    false,
                    false,
                    vec![],
                ),
                make_node("btn", "Button", Some("Close app"), true, false, vec![]),
            ],
        );
        let tree = make_tree_with_root(root, "android");
        // detect_app_state always returns Unknown — LLM classifies app state from raw tree.
        assert_eq!(detect_app_state(&tree), AppState::Unknown);
    }

    #[test]
    fn test_crash_dialog_in_app() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node(
                    "msg",
                    "TextView",
                    Some("App keeps stopping"),
                    false,
                    false,
                    vec![],
                ),
                make_node("btn", "Button", Some("OK"), true, false, vec![]),
            ],
        );
        let tree = make_tree_with_root(root, "com.test");
        // detect_app_state always returns Unknown — LLM classifies app state from raw tree.
        assert_eq!(detect_app_state(&tree), AppState::Unknown);
    }

    #[test]
    fn test_login_screen_detection() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node("title", "TextView", Some("Sign in"), false, false, vec![]),
                make_node("email", "EditText", Some("Email"), true, true, vec![]),
                make_node("pass", "EditText", Some("Password"), true, true, vec![]),
                make_node("btn", "Button", Some("Log in"), true, false, vec![]),
            ],
        );
        let tree = make_tree_with_root(root, "com.test");
        // detect_app_state always returns Unknown — LLM classifies app state from raw tree.
        assert_eq!(detect_app_state(&tree), AppState::Unknown);
    }

    #[test]
    fn test_loading_state_detection() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node(
                    "spinner",
                    "android.widget.ProgressBar",
                    None,
                    false,
                    false,
                    vec![],
                ),
                make_node("text", "TextView", Some("Loading..."), false, false, vec![]),
            ],
        );
        let tree = make_tree_with_root(root, "com.test");
        // detect_app_state always returns Unknown — LLM classifies app state from raw tree.
        assert_eq!(detect_app_state(&tree), AppState::Unknown);
    }

    #[test]
    fn test_error_dialog_detection() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node("title", "TextView", Some("Error"), false, false, vec![]),
                make_node(
                    "msg",
                    "TextView",
                    Some("Something went wrong. Unable to connect."),
                    false,
                    false,
                    vec![],
                ),
                make_node("btn", "Button", Some("Retry"), true, false, vec![]),
            ],
        );
        let tree = make_tree_with_root(root, "com.test");
        // detect_app_state always returns Unknown — LLM classifies app state from raw tree.
        assert_eq!(detect_app_state(&tree), AppState::Unknown);
    }

    #[test]
    fn test_screen_summary_deduplicates_text() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node("t1", "TextView", Some("Hello"), false, false, vec![]),
                make_node("t2", "TextView", Some("Hello"), false, false, vec![]),
                make_node("t3", "TextView", Some("World"), false, false, vec![]),
            ],
        );
        let tree = make_tree_with_root(root, "com.test");
        let summary = extract_screen_summary(&tree);
        assert_eq!(summary.visible_text.len(), 2); // deduplicated
    }
}
