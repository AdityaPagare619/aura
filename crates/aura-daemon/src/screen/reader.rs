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

/// Detect the current app state from the screen tree using heuristics.
pub fn detect_app_state(tree: &ScreenTree) -> AppState {
    // Empty tree → crashed or transition
    if tree.node_count == 0 {
        return AppState::Crashed;
    }

    // Check for system dialogs (permission prompts, crash dialogs, etc.)
    // These checks MUST come before the node_count <= 2 guard because
    // permission prompts and system dialogs can legitimately have very few nodes.
    let package = &tree.package_name;
    if is_permission_package(package) {
        return AppState::PermissionPrompt;
    }
    if is_system_dialog_package(package) {
        return AppState::SystemDialog;
    }

    if tree.node_count <= 2 {
        return AppState::EmptyOrTransition;
    }

    // Collect all text for pattern matching
    let all_text = tree.all_text();
    let all_text_lower: Vec<String> = all_text.iter().map(|t| t.to_lowercase()).collect();
    let joined_text = all_text_lower.join(" ");

    // Check for crash dialog
    if is_crash_dialog(&joined_text, &tree.root) {
        return AppState::Crashed;
    }

    // Check for error dialog
    if is_error_dialog(&joined_text, &tree.root) {
        return AppState::ErrorDialog;
    }

    // Check for login screen
    if is_login_screen(&joined_text, &tree.root) {
        return AppState::LoginScreen;
    }

    // Check for loading state
    if is_loading_state(&joined_text, &tree.root) {
        return AppState::Loading;
    }

    // Check for overlay blocking
    if is_overlay_blocking(&tree.root) {
        return AppState::OverlayBlocking;
    }

    // Check for keyboard / input mode
    if is_keyboard_visible(&tree.root) {
        return AppState::InputMode;
    }

    AppState::Normal
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

/// Known permission dialog packages.
fn is_permission_package(package: &str) -> bool {
    matches!(
        package,
        "com.android.packageinstaller"
            | "com.google.android.permissioncontroller"
            | "com.android.permissioncontroller"
            | "com.samsung.android.permissioncontroller"
    )
}

/// Known system dialog packages that aren't the target app.
fn is_system_dialog_package(package: &str) -> bool {
    package == "android"
        || package == "com.android.systemui"
        || package == "com.android.settings"
        || package.starts_with("com.android.internal")
}

/// Check if the current screen looks like a crash dialog.
fn is_crash_dialog(joined_text: &str, root: &ScreenNode) -> bool {
    let crash_patterns = [
        "has stopped",
        "keeps stopping",
        "isn't responding",
        "has crashed",
        "unfortunately",
        "force close",
        "app isn't responding",
        "close app",
        "wait",
    ];

    let has_crash_text = crash_patterns
        .iter()
        .any(|pattern| joined_text.contains(pattern));

    // Must also have a dismiss button (OK, Close app, etc.)
    if has_crash_text {
        return has_button_with_text(root, &["ok", "close app", "close", "wait"]);
    }

    false
}

/// Check if the current screen looks like an error dialog.
fn is_error_dialog(joined_text: &str, root: &ScreenNode) -> bool {
    let error_patterns = [
        "error",
        "failed",
        "couldn't",
        "unable to",
        "no internet",
        "no connection",
        "network error",
        "something went wrong",
        "try again",
        "retry",
    ];

    let error_score: u32 = error_patterns
        .iter()
        .filter(|p| joined_text.contains(**p))
        .count() as u32;

    // Need at least 2 error-related strings to confirm
    error_score >= 2
        && has_button_with_text(root, &["ok", "retry", "try again", "dismiss", "close"])
}

/// Check if the current screen looks like a login/auth screen.
fn is_login_screen(joined_text: &str, root: &ScreenNode) -> bool {
    let login_patterns = [
        "sign in",
        "log in",
        "login",
        "email",
        "password",
        "username",
        "forgot password",
        "create account",
        "register",
    ];

    let login_score: u32 = login_patterns
        .iter()
        .filter(|p| joined_text.contains(**p))
        .count() as u32;

    // Need at least 2 login-related strings and at least one editable field
    login_score >= 2 && has_editable_field(root)
}

/// Check if the screen appears to be in a loading state.
fn is_loading_state(joined_text: &str, root: &ScreenNode) -> bool {
    let loading_patterns = ["loading", "please wait", "spinner", "progress"];

    let has_loading_text = loading_patterns.iter().any(|p| joined_text.contains(p));

    // Also check for ProgressBar class in the tree
    let has_progress_bar = has_class_name(root, "ProgressBar");

    has_loading_text || has_progress_bar
}

/// Check if an overlay or popup is blocking the main content.
fn is_overlay_blocking(root: &ScreenNode) -> bool {
    // Heuristic: if there's a node covering >80% of the screen that is
    // not the root and has a small number of children, it's likely an overlay.
    let screen_area = root.bounds.width() as i64 * root.bounds.height() as i64;
    if screen_area <= 0 {
        return false;
    }

    for child in &root.children {
        let child_area = child.bounds.width() as i64 * child.bounds.height() as i64;
        let coverage = (child_area * 100) / screen_area;

        // A child covering >80% of screen with few children = likely overlay
        if coverage > 80 && child.children.len() <= 5 && child.is_clickable {
            return true;
        }
    }

    false
}

/// Check if a keyboard is likely visible.
fn is_keyboard_visible(root: &ScreenNode) -> bool {
    fn check_node(node: &ScreenNode) -> bool {
        if node.package_name.contains("inputmethod")
            || node.package_name.contains("keyboard")
            || node.class_name.contains("KeyboardView")
            || node.class_name.contains("InputView")
        {
            return true;
        }
        node.children.iter().any(|c| check_node(c))
    }
    check_node(root)
}

/// Check if any node in the tree has a button with text matching one of the patterns.
fn has_button_with_text(node: &ScreenNode, patterns: &[&str]) -> bool {
    if node.is_clickable {
        if let Some(ref text) = node.text {
            let lower = text.to_lowercase();
            if patterns.iter().any(|p| lower.contains(p)) {
                return true;
            }
        }
        if let Some(ref desc) = node.content_description {
            let lower = desc.to_lowercase();
            if patterns.iter().any(|p| lower.contains(p)) {
                return true;
            }
        }
    }
    node.children
        .iter()
        .any(|c| has_button_with_text(c, patterns))
}

/// Check if there's at least one editable field in the tree.
fn has_editable_field(node: &ScreenNode) -> bool {
    if node.is_editable {
        return true;
    }
    node.children.iter().any(|c| has_editable_field(c))
}

/// Check if any node has the given class name (short match).
fn has_class_name(node: &ScreenNode, class: &str) -> bool {
    let short = node
        .class_name
        .rsplit('.')
        .next()
        .unwrap_or(&node.class_name);
    if short == class {
        return true;
    }
    node.children.iter().any(|c| has_class_name(c, class))
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
        assert_eq!(summary.app_state, AppState::Normal);
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
        assert_eq!(detect_app_state(&tree), AppState::Crashed);
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
        assert_eq!(detect_app_state(&tree), AppState::PermissionPrompt);
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
        // Package is "android" → SystemDialog, which is checked before crash
        assert_eq!(detect_app_state(&tree), AppState::SystemDialog);
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
        assert_eq!(detect_app_state(&tree), AppState::Crashed);
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
        assert_eq!(detect_app_state(&tree), AppState::LoginScreen);
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
        assert_eq!(detect_app_state(&tree), AppState::Loading);
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
        assert_eq!(detect_app_state(&tree), AppState::ErrorDialog);
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
