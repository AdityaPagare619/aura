//! Post-action verification: compare before/after ScreenTree snapshots to
//! determine if an action had the expected effect.
//!
//! Used after every action to decide:
//! - Did the screen change at all?
//! - Did the right thing change?
//! - Is the new state what we expected?

use aura_types::screen::{ScreenDiff, ScreenNode, ScreenTree};
use tracing::trace;

/// Result of post-action verification.
#[derive(Debug, Clone)]
pub struct VerificationResult {
    /// Whether the screen changed at all.
    pub screen_changed: bool,
    /// Structural diff between before and after.
    pub diff: ScreenDiff,
    /// Confidence that the action had its intended effect (0.0–1.0).
    pub confidence: f32,
    /// Whether the foreground app changed.
    pub app_changed: bool,
    /// Whether the activity changed.
    pub activity_changed: bool,
    /// Number of new nodes that appeared.
    pub nodes_added: u32,
    /// Number of nodes that disappeared.
    pub nodes_removed: u32,
    /// Number of nodes whose text changed.
    pub text_changes: u32,
}

/// Verify the effect of an action by comparing before/after screen trees.
///
/// Returns a `VerificationResult` describing what changed and how confident
/// we are that the action succeeded.
pub fn verify_action(
    before: &ScreenTree,
    after: &ScreenTree,
    expected_change: Option<&ExpectedChange>,
) -> VerificationResult {
    let app_changed = before.package_name != after.package_name;
    let activity_changed = before.activity_name != after.activity_name;

    // Build node ID maps for both trees
    let before_ids = collect_node_ids(&before.root);
    let after_ids = collect_node_ids(&after.root);

    // Compute structural diff
    let added: Vec<String> = after_ids
        .iter()
        .filter(|id| !before_ids.contains(id))
        .cloned()
        .collect();

    let removed: Vec<String> = before_ids
        .iter()
        .filter(|id| !after_ids.contains(id))
        .cloned()
        .collect();

    // Find nodes that exist in both but have changed text
    let mut changed = Vec::new();
    let before_texts = collect_node_texts(&before.root);
    let after_texts = collect_node_texts(&after.root);

    for (id, before_text) in &before_texts {
        if let Some((_, after_text)) = after_texts.iter().find(|(aid, _)| aid == id) {
            if before_text != after_text {
                changed.push((
                    id.clone(),
                    format!("text: '{}' -> '{}'", before_text, after_text),
                ));
            }
        }
    }

    let screen_changed = !added.is_empty()
        || !removed.is_empty()
        || !changed.is_empty()
        || app_changed
        || activity_changed;

    let nodes_added = added.len() as u32;
    let nodes_removed = removed.len() as u32;
    let text_changes = changed.len() as u32;

    // Compute confidence based on what changed and what was expected
    let confidence = compute_confidence(
        screen_changed,
        app_changed,
        activity_changed,
        nodes_added,
        nodes_removed,
        text_changes,
        expected_change,
        before,
        after,
    );

    let diff = ScreenDiff {
        added_nodes: added,
        removed_nodes: removed,
        changed_nodes: changed,
        screen_changed,
    };

    VerificationResult {
        screen_changed,
        diff,
        confidence,
        app_changed,
        activity_changed,
        nodes_added,
        nodes_removed,
        text_changes,
    }
}

/// What kind of change we expect from an action.
#[derive(Debug, Clone)]
pub enum ExpectedChange {
    /// Screen should change somehow (tap, navigate).
    ScreenChange,
    /// A specific text should appear.
    TextAppears(String),
    /// A specific text should disappear.
    TextDisappears(String),
    /// A specific element should become visible.
    ElementAppears(String),
    /// Navigation to a different activity.
    ActivityChange,
    /// Navigation to a different app.
    AppChange,
    /// No change expected (assertion, check).
    NoChange,
    /// Text in a specific element should change.
    TextInElementChanges(String),
}

/// Compute a FNV-1a 64-bit hash of the screen tree structure.
/// Used for cycle detection state hashing.
///
/// Excludes volatile fields: status bar text, notification shade, animation
/// counters, scroll positions, exact pixel coordinates.
pub fn hash_screen_state(tree: &ScreenTree) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    let prime: u64 = 0x100000001b3; // FNV-1a prime

    // Hash the package and activity
    for byte in tree.package_name.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(prime);
    }
    for byte in tree.activity_name.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(prime);
    }

    // Hash the tree structure (class names, text, resource-ids)
    // but NOT bounds, scroll position, focus state, or other volatile fields
    hash_node_structure(&tree.root, &mut hash, prime);

    hash
}

/// Compute a FNV-1a 32-bit hash of an action for cycle detection.
pub fn hash_action(action_type: u8, target_id: &str) -> u32 {
    let mut hash: u32 = 0x811c9dc5; // FNV-1a 32-bit offset basis
    let prime: u32 = 0x01000193; // FNV-1a 32-bit prime

    hash ^= action_type as u32;
    hash = hash.wrapping_mul(prime);

    for byte in target_id.bytes().take(28) {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(prime);
    }

    hash
}

// ── Internals ───────────────────────────────────────────────────────────────

/// Normalize text for fuzzy comparison: lowercase, strip non-alphanumeric
/// characters (except spaces), and collapse multiple spaces.
fn normalize_for_match(text: &str) -> String {
    let lower = text.to_lowercase();
    let cleaned: String = lower
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();
    // Collapse multiple spaces
    let mut result = String::with_capacity(cleaned.len());
    let mut prev_space = true; // trim leading
    for c in cleaned.chars() {
        if c.is_whitespace() {
            if !prev_space {
                result.push(' ');
                prev_space = true;
            }
        } else {
            result.push(c);
            prev_space = false;
        }
    }
    // Trim trailing space
    if result.ends_with(' ') {
        result.pop();
    }
    result
}

/// Collect all content descriptions from a node tree.
fn collect_all_content_descriptions(node: &ScreenNode) -> Vec<String> {
    let mut results = Vec::new();
    if let Some(ref desc) = node.content_description {
        if !desc.is_empty() {
            results.push(desc.clone());
        }
    }
    for child in &node.children {
        results.extend(collect_all_content_descriptions(child));
    }
    results
}

/// Check how well expected text matches any text on screen.
///
/// Returns a quality score:
/// - `1.0` — exact match (after normalization)
/// - `0.7` — one contains the other as a substring
/// - `0.4` — at least one significant word matches
/// - `0.0` — no match
fn text_match_quality(tree: &ScreenTree, expected: &str) -> f32 {
    let norm_expected = normalize_for_match(expected);
    if norm_expected.is_empty() {
        return 0.0;
    }

    // Gather all visible text and content descriptions
    let mut all_strings: Vec<String> = tree.all_text();
    all_strings.extend(collect_all_content_descriptions(&tree.root));

    let mut best = 0.0_f32;

    for raw in &all_strings {
        let norm = normalize_for_match(raw);
        if norm.is_empty() {
            continue;
        }

        // Exact match
        if norm == norm_expected {
            trace!(expected, found = %raw, "exact text match");
            return 1.0;
        }

        // Substring match (either direction)
        if norm.contains(&norm_expected) || norm_expected.contains(&norm) {
            if best < 0.7 {
                best = 0.7;
            }
        }
    }

    if best > 0.0 {
        return best;
    }

    // Word-level matching: check if any significant word from expected appears
    let expected_words: Vec<&str> = norm_expected
        .split_whitespace()
        .filter(|w| w.len() > 2) // skip very short words
        .collect();

    if expected_words.is_empty() {
        return 0.0;
    }

    for raw in &all_strings {
        let norm = normalize_for_match(raw);
        for word in &expected_words {
            if norm.contains(word) {
                return 0.4;
            }
        }
    }

    0.0
}

/// Verify that specific text appears on the current screen.
///
/// Uses fuzzy matching: case-insensitive, punctuation-tolerant.
/// Returns `true` if the text (or a close match) is found.
pub fn verify_text_appears(tree: &ScreenTree, expected_text: &str) -> bool {
    text_match_quality(tree, expected_text) > 0.3
}

/// Verify that specific text does NOT appear on the current screen.
///
/// Returns `true` if the text is absent (match quality ≤ 0.3).
pub fn verify_text_disappears(tree: &ScreenTree, expected_text: &str) -> bool {
    text_match_quality(tree, expected_text) <= 0.3
}

fn hash_node_structure(node: &ScreenNode, hash: &mut u64, prime: u64) {
    // Hash class name
    for byte in node.class_name.bytes() {
        *hash ^= byte as u64;
        *hash = hash.wrapping_mul(prime);
    }

    // Hash text (major content signal)
    if let Some(ref text) = node.text {
        for byte in text.bytes() {
            *hash ^= byte as u64;
            *hash = hash.wrapping_mul(prime);
        }
    }

    // Hash resource-id (stable identifier)
    if let Some(ref rid) = node.resource_id {
        for byte in rid.bytes() {
            *hash ^= byte as u64;
            *hash = hash.wrapping_mul(prime);
        }
    }

    // Hash content description
    if let Some(ref desc) = node.content_description {
        for byte in desc.bytes() {
            *hash ^= byte as u64;
            *hash = hash.wrapping_mul(prime);
        }
    }

    // Hash interactive state (clickable, enabled) since these affect behavior
    *hash ^= node.is_clickable as u64;
    *hash = hash.wrapping_mul(prime);
    *hash ^= node.is_enabled as u64;
    *hash = hash.wrapping_mul(prime);

    // DO NOT hash: bounds, is_focused, is_checked, scroll position
    // These are volatile and change between captures without meaningful state change

    // Recurse into children
    for child in &node.children {
        hash_node_structure(child, hash, prime);
    }
}

fn collect_node_ids(node: &ScreenNode) -> Vec<String> {
    let mut ids = vec![node.id.clone()];
    for child in &node.children {
        ids.extend(collect_node_ids(child));
    }
    ids
}

fn collect_node_texts(node: &ScreenNode) -> Vec<(String, String)> {
    let mut texts = Vec::new();
    if let Some(ref text) = node.text {
        texts.push((node.id.clone(), text.clone()));
    }
    for child in &node.children {
        texts.extend(collect_node_texts(child));
    }
    texts
}

fn compute_confidence(
    screen_changed: bool,
    app_changed: bool,
    activity_changed: bool,
    nodes_added: u32,
    _nodes_removed: u32,
    text_changes: u32,
    expected: Option<&ExpectedChange>,
    before: &ScreenTree,
    after: &ScreenTree,
) -> f32 {
    match expected {
        Some(ExpectedChange::ScreenChange) => {
            if screen_changed {
                0.9
            } else {
                0.1
            }
        },
        Some(ExpectedChange::NoChange) => {
            if !screen_changed {
                0.9
            } else {
                0.5
            }
        },
        Some(ExpectedChange::ActivityChange) => {
            if activity_changed {
                0.95
            } else if screen_changed {
                0.4
            } else {
                0.1
            }
        },
        Some(ExpectedChange::AppChange) => {
            if app_changed {
                0.95
            } else if activity_changed {
                0.5
            } else {
                0.1
            }
        },
        Some(ExpectedChange::TextAppears(expected_text)) => {
            let quality = text_match_quality(after, expected_text);
            if quality >= 0.9 {
                0.95 // Exact match found
            } else if quality >= 0.5 {
                0.80 // Partial / substring match
            } else if quality >= 0.3 {
                0.60 // Word-level match
            } else if text_changes > 0 {
                0.25 // Text changed but not the expected text — low confidence
            } else if screen_changed {
                0.2 // Screen changed but no text match
            } else {
                0.1 // Nothing changed
            }
        },
        Some(ExpectedChange::TextDisappears(expected_text)) => {
            let was_present = text_match_quality(before, expected_text) > 0.3;
            let still_present = text_match_quality(after, expected_text) > 0.3;
            if was_present && !still_present {
                0.95 // Text was there and is now gone
            } else if !was_present {
                0.5 // Text wasn't there to begin with
            } else {
                0.1 // Text is still present
            }
        },
        Some(ExpectedChange::ElementAppears(_)) => {
            if nodes_added > 0 {
                0.7
            } else if screen_changed {
                0.4
            } else {
                0.1
            }
        },
        Some(ExpectedChange::TextInElementChanges(_)) => {
            if text_changes > 0 {
                0.8
            } else if screen_changed {
                0.4
            } else {
                0.1
            }
        },
        None => {
            // No expectation — just report whether something changed
            if app_changed || activity_changed {
                0.85
            } else if screen_changed {
                0.7
            } else {
                0.3
            }
        },
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use aura_types::screen::Bounds;

    use super::*;

    fn make_node(id: &str, text: Option<&str>, children: Vec<ScreenNode>) -> ScreenNode {
        ScreenNode {
            id: id.into(),
            class_name: "TextView".into(),
            package_name: "com.test".into(),
            text: text.map(|s| s.into()),
            content_description: None,
            resource_id: None,
            bounds: Bounds {
                left: 0,
                top: 0,
                right: 100,
                bottom: 50,
            },
            is_clickable: false,
            is_scrollable: false,
            is_editable: false,
            is_checkable: false,
            is_checked: false,
            is_enabled: true,
            is_focused: false,
            is_visible: true,
            children,
            depth: 0,
        }
    }

    fn make_tree(root: ScreenNode, package: &str, activity: &str) -> ScreenTree {
        fn count(n: &ScreenNode) -> u32 {
            1 + n.children.iter().map(|c| count(c)).sum::<u32>()
        }
        ScreenTree {
            node_count: count(&root),
            root,
            package_name: package.into(),
            activity_name: activity.into(),
            timestamp_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn test_no_change() {
        let tree = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        let result = verify_action(&tree, &tree, Some(&ExpectedChange::NoChange));
        assert!(!result.screen_changed);
        assert!(result.confidence > 0.8);
    }

    #[test]
    fn test_text_change() {
        let before = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        let after = make_tree(
            make_node("root", Some("World"), vec![]),
            "com.test",
            ".Main",
        );
        let result = verify_action(&before, &after, Some(&ExpectedChange::ScreenChange));
        assert!(result.screen_changed);
        assert_eq!(result.text_changes, 1);
        assert!(result.confidence > 0.8);
    }

    #[test]
    fn test_node_added() {
        let before = make_tree(make_node("root", None, vec![]), "com.test", ".Main");
        let after = make_tree(
            make_node(
                "root",
                None,
                vec![make_node("new_child", Some("New"), vec![])],
            ),
            "com.test",
            ".Main",
        );
        let result = verify_action(&before, &after, None);
        assert!(result.screen_changed);
        assert_eq!(result.nodes_added, 1);
    }

    #[test]
    fn test_app_change() {
        let before = make_tree(make_node("root", None, vec![]), "com.app1", ".Main");
        let after = make_tree(make_node("root", None, vec![]), "com.app2", ".Main");
        let result = verify_action(&before, &after, Some(&ExpectedChange::AppChange));
        assert!(result.app_changed);
        assert!(result.confidence > 0.9);
    }

    #[test]
    fn test_activity_change() {
        let before = make_tree(make_node("root", None, vec![]), "com.test", ".Main");
        let after = make_tree(make_node("root", None, vec![]), "com.test", ".Settings");
        let result = verify_action(&before, &after, Some(&ExpectedChange::ActivityChange));
        assert!(result.activity_changed);
        assert!(result.confidence > 0.9);
    }

    #[test]
    fn test_hash_screen_state_deterministic() {
        let tree = make_tree(
            make_node(
                "root",
                Some("Test"),
                vec![make_node("child", Some("Data"), vec![])],
            ),
            "com.test",
            ".Main",
        );
        let h1 = hash_screen_state(&tree);
        let h2 = hash_screen_state(&tree);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_screen_state_different_content() {
        let t1 = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        let t2 = make_tree(
            make_node("root", Some("World"), vec![]),
            "com.test",
            ".Main",
        );
        assert_ne!(hash_screen_state(&t1), hash_screen_state(&t2));
    }

    #[test]
    fn test_hash_action_deterministic() {
        let h1 = hash_action(1, "com.test:id/btn");
        let h2 = hash_action(1, "com.test:id/btn");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_action_different_inputs() {
        let h1 = hash_action(1, "com.test:id/btn");
        let h2 = hash_action(2, "com.test:id/btn");
        let h3 = hash_action(1, "com.test:id/other");
        assert_ne!(h1, h2);
        assert_ne!(h1, h3);
    }

    // ── normalize_for_match tests ───────────────────────────────────────────

    #[test]
    fn test_normalize_lowercase() {
        assert_eq!(normalize_for_match("Hello World"), "hello world");
    }

    #[test]
    fn test_normalize_strip_punctuation() {
        assert_eq!(normalize_for_match("Hello, World!"), "hello world");
    }

    #[test]
    fn test_normalize_collapse_spaces() {
        assert_eq!(normalize_for_match("  hello   world  "), "hello world");
    }

    // ── text_match_quality tests ────────────────────────────────────────────

    #[test]
    fn test_match_quality_exact() {
        let tree = make_tree(
            make_node("root", Some("Login"), vec![]),
            "com.test",
            ".Main",
        );
        let quality = text_match_quality(&tree, "Login");
        assert!((quality - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_match_quality_case_insensitive() {
        let tree = make_tree(
            make_node("root", Some("Login"), vec![]),
            "com.test",
            ".Main",
        );
        let quality = text_match_quality(&tree, "login");
        assert!((quality - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_match_quality_partial() {
        let tree = make_tree(
            make_node("root", Some("Sign in to continue"), vec![]),
            "com.test",
            ".Main",
        );
        let quality = text_match_quality(&tree, "Sign in");
        assert!(quality >= 0.7);
    }

    #[test]
    fn test_match_quality_no_match() {
        let tree = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        let quality = text_match_quality(&tree, "Goodbye");
        assert!((quality - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_match_quality_word_match() {
        let tree = make_tree(
            make_node("root", Some("Please enter your email address"), vec![]),
            "com.test",
            ".Main",
        );
        // "email" is a significant word that appears in the tree text
        let quality = text_match_quality(&tree, "email");
        assert!(quality >= 0.4);
    }

    #[test]
    fn test_match_quality_content_description() {
        let mut node = make_node("root", None, vec![]);
        node.content_description = Some("Navigate back".into());
        let tree = make_tree(node, "com.test", ".Main");
        let quality = text_match_quality(&tree, "Navigate back");
        assert!((quality - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_match_quality_empty_expected() {
        let tree = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        let quality = text_match_quality(&tree, "");
        assert!((quality - 0.0).abs() < f32::EPSILON);
    }

    // ── verify_text_appears / disappears tests ──────────────────────────────

    #[test]
    fn test_verify_text_appears_present() {
        let tree = make_tree(
            make_node("root", Some("Welcome back"), vec![]),
            "com.test",
            ".Main",
        );
        assert!(verify_text_appears(&tree, "Welcome back"));
    }

    #[test]
    fn test_verify_text_appears_not_present() {
        let tree = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        assert!(!verify_text_appears(&tree, "Goodbye forever"));
    }

    #[test]
    fn test_verify_text_appears_case_insensitive() {
        let tree = make_tree(
            make_node("root", Some("SUCCESS"), vec![]),
            "com.test",
            ".Main",
        );
        assert!(verify_text_appears(&tree, "success"));
    }

    #[test]
    fn test_verify_text_appears_partial() {
        let tree = make_tree(
            make_node("root", Some("Error: connection failed"), vec![]),
            "com.test",
            ".Main",
        );
        assert!(verify_text_appears(&tree, "connection failed"));
    }

    #[test]
    fn test_verify_text_disappears_gone() {
        let tree = make_tree(
            make_node("root", Some("Other text"), vec![]),
            "com.test",
            ".Main",
        );
        assert!(verify_text_disappears(&tree, "Loading..."));
    }

    #[test]
    fn test_verify_text_disappears_still_present() {
        let tree = make_tree(
            make_node("root", Some("Loading..."), vec![]),
            "com.test",
            ".Main",
        );
        assert!(!verify_text_disappears(&tree, "Loading"));
    }

    #[test]
    fn test_verify_text_disappears_empty_tree() {
        let tree = make_tree(make_node("root", None, vec![]), "com.test", ".Main");
        assert!(verify_text_disappears(&tree, "anything"));
    }

    // ── compute_confidence with TextAppears/TextDisappears ──────────────────

    #[test]
    fn test_confidence_text_appears_found() {
        let before = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        let after = make_tree(
            make_node(
                "root",
                Some("Hello"),
                vec![make_node("msg", Some("Success!"), vec![])],
            ),
            "com.test",
            ".Main",
        );
        let result = verify_action(
            &before,
            &after,
            Some(&ExpectedChange::TextAppears("Success".into())),
        );
        assert!(
            result.confidence >= 0.8,
            "confidence was {}",
            result.confidence
        );
    }

    #[test]
    fn test_confidence_text_appears_not_found() {
        let before = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        let after = make_tree(
            make_node("root", Some("World"), vec![]),
            "com.test",
            ".Main",
        );
        let result = verify_action(
            &before,
            &after,
            Some(&ExpectedChange::TextAppears("Goodbye".into())),
        );
        // Text changed but expected text not found
        assert!(
            result.confidence <= 0.5,
            "confidence was {}",
            result.confidence
        );
    }

    #[test]
    fn test_confidence_text_appears_nothing_changed() {
        let tree = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        let result = verify_action(
            &tree,
            &tree,
            Some(&ExpectedChange::TextAppears("World".into())),
        );
        assert!(
            result.confidence <= 0.2,
            "confidence was {}",
            result.confidence
        );
    }

    #[test]
    fn test_confidence_text_disappears_gone() {
        let before = make_tree(
            make_node("root", Some("Loading..."), vec![]),
            "com.test",
            ".Main",
        );
        let after = make_tree(make_node("root", Some("Done"), vec![]), "com.test", ".Main");
        let result = verify_action(
            &before,
            &after,
            Some(&ExpectedChange::TextDisappears("Loading".into())),
        );
        assert!(
            result.confidence >= 0.9,
            "confidence was {}",
            result.confidence
        );
    }

    #[test]
    fn test_confidence_text_disappears_still_there() {
        let tree = make_tree(
            make_node("root", Some("Loading..."), vec![]),
            "com.test",
            ".Main",
        );
        let result = verify_action(
            &tree,
            &tree,
            Some(&ExpectedChange::TextDisappears("Loading".into())),
        );
        assert!(
            result.confidence <= 0.2,
            "confidence was {}",
            result.confidence
        );
    }

    // ── TextAppears confidence fix tests ───────────────────────────────────
    // Verify the corrected confidence hierarchy:
    //   exact(0.95) > partial(0.80) > word(0.60) > wrong-text(0.25) > screen-only(0.2) >
    // nothing(0.1)

    #[test]
    fn test_text_appears_wrong_text_changed_low_confidence() {
        // Text changed but expected text never appeared → must be 0.25
        let before = make_tree(
            make_node("root", Some("Old text"), vec![]),
            "com.test",
            ".Main",
        );
        let after = make_tree(
            make_node("root", Some("New text"), vec![]),
            "com.test",
            ".Main",
        );
        let result = verify_action(
            &before,
            &after,
            Some(&ExpectedChange::TextAppears("Expected phrase".into())),
        );
        assert!(
            (result.confidence - 0.25).abs() < f32::EPSILON,
            "wrong-text-changed confidence should be 0.25, got {}",
            result.confidence
        );
    }

    #[test]
    fn test_text_appears_screen_changed_no_text_match_low_confidence() {
        // Screen structure changed (node added) but no text matches → must be 0.2
        let before = make_tree(make_node("root", None, vec![]), "com.test", ".Main");
        let after = make_tree(
            make_node(
                "root",
                None,
                vec![make_node("child", None, vec![])], // node added, no text
            ),
            "com.test",
            ".Main",
        );
        let result = verify_action(
            &before,
            &after,
            Some(&ExpectedChange::TextAppears("Missing text".into())),
        );
        // text_changes == 0, screen_changed == true → 0.2
        assert!(
            (result.confidence - 0.2).abs() < f32::EPSILON,
            "screen-changed-only confidence should be 0.2, got {}",
            result.confidence
        );
    }

    #[test]
    fn test_text_appears_nothing_changed_is_lowest() {
        // No change at all → 0.1 (even lower than screen-changed)
        let tree = make_tree(
            make_node("root", Some("Static"), vec![]),
            "com.test",
            ".Main",
        );
        let result = verify_action(
            &tree,
            &tree,
            Some(&ExpectedChange::TextAppears("Dynamic".into())),
        );
        assert!(
            (result.confidence - 0.1).abs() < f32::EPSILON,
            "nothing-changed confidence should be 0.1, got {}",
            result.confidence
        );
    }

    #[test]
    fn test_text_appears_wrong_text_below_screen_change_for_other_types() {
        // For TextAppears: wrong-text (0.25) must be LESS than partial (0.80)
        // and LESS than word-level (0.60). This test verifies ordering.
        let before = make_tree(
            make_node("root", Some("Price: $10"), vec![]),
            "com.test",
            ".Main",
        );
        let after_wrong_text = make_tree(
            make_node("root", Some("Price: $20"), vec![]),
            "com.test",
            ".Main",
        );
        let after_partial = make_tree(
            make_node("root", Some("Order confirmed successfully"), vec![]),
            "com.test",
            ".Main",
        );
        let expected = ExpectedChange::TextAppears("Order confirmed".into());

        let wrong = verify_action(&before, &after_wrong_text, Some(&expected));
        let partial = verify_action(&before, &after_partial, Some(&expected));

        assert!(
            partial.confidence > wrong.confidence,
            "partial match ({}) should be greater than wrong-text ({})",
            partial.confidence,
            wrong.confidence
        );
    }

    #[test]
    fn test_text_appears_confidence_hierarchy_full() {
        // Verify the complete ordering: exact > partial > word > wrong-text > screen-only > nothing
        let before = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        let expected = ExpectedChange::TextAppears("Welcome message".into());

        // 1. Exact match
        let after_exact = make_tree(
            make_node("root", Some("Welcome message"), vec![]),
            "com.test",
            ".Main",
        );
        let exact = verify_action(&before, &after_exact, Some(&expected));

        // 2. Partial match (substring)
        let after_partial = make_tree(
            make_node("root", Some("Welcome message and more content"), vec![]),
            "com.test",
            ".Main",
        );
        let partial = verify_action(&before, &after_partial, Some(&expected));

        // 3. Wrong text changed
        let after_wrong = make_tree(
            make_node("root", Some("Goodbye"), vec![]),
            "com.test",
            ".Main",
        );
        let wrong = verify_action(&before, &after_wrong, Some(&expected));

        // 4. Screen changed only (no text changes)
        let after_screen = make_tree(
            make_node(
                "root",
                Some("Hello"),
                vec![make_node("extra", None, vec![])],
            ),
            "com.test",
            ".Main",
        );
        let screen = verify_action(&before, &after_screen, Some(&expected));

        // 5. Nothing changed
        let nothing = verify_action(&before, &before, Some(&expected));

        // Verify monotonic decrease
        assert!(
            exact.confidence > partial.confidence || exact.confidence == partial.confidence,
            "exact({}) >= partial({})",
            exact.confidence,
            partial.confidence
        );
        assert!(
            partial.confidence > wrong.confidence,
            "partial({}) > wrong({})",
            partial.confidence,
            wrong.confidence
        );
        assert!(
            wrong.confidence > screen.confidence || wrong.confidence == screen.confidence,
            "wrong({}) >= screen({})",
            wrong.confidence,
            screen.confidence
        );
        assert!(
            screen.confidence > nothing.confidence,
            "screen({}) > nothing({})",
            screen.confidence,
            nothing.confidence
        );
    }

    #[test]
    fn test_text_appears_multiple_text_changes_wrong_text() {
        // Multiple text nodes changed, but none match expected → still 0.25
        let before = make_tree(
            make_node(
                "root",
                Some("Title"),
                vec![
                    make_node("a", Some("Subtitle"), vec![]),
                    make_node("b", Some("Footer"), vec![]),
                ],
            ),
            "com.test",
            ".Main",
        );
        let after = make_tree(
            make_node(
                "root",
                Some("New Title"),
                vec![
                    make_node("a", Some("New Subtitle"), vec![]),
                    make_node("b", Some("New Footer"), vec![]),
                ],
            ),
            "com.test",
            ".Main",
        );
        let result = verify_action(
            &before,
            &after,
            Some(&ExpectedChange::TextAppears("Completely different".into())),
        );
        assert!(
            (result.confidence - 0.25).abs() < f32::EPSILON,
            "multiple wrong text changes confidence should be 0.25, got {}",
            result.confidence
        );
    }

    // ── Edge cases ──────────────────────────────────────────────────────────

    #[test]
    fn test_match_quality_special_characters() {
        let tree = make_tree(
            make_node("root", Some("Price: $9.99!"), vec![]),
            "com.test",
            ".Main",
        );
        // After normalization: "price 9 99" vs "price 9 99" — exact
        let quality = text_match_quality(&tree, "Price: $9.99!");
        assert!(quality >= 0.9);
    }

    #[test]
    fn test_match_quality_empty_tree() {
        let tree = make_tree(make_node("root", None, vec![]), "com.test", ".Main");
        let quality = text_match_quality(&tree, "anything");
        assert!((quality - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_match_quality_deep_tree() {
        let tree = make_tree(
            make_node(
                "root",
                None,
                vec![make_node(
                    "level1",
                    None,
                    vec![make_node("level2", Some("Deep text"), vec![])],
                )],
            ),
            "com.test",
            ".Main",
        );
        let quality = text_match_quality(&tree, "Deep text");
        assert!((quality - 1.0).abs() < f32::EPSILON);
    }
}
