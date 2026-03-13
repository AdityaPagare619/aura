use serde::{Deserialize, Serialize};

/// A single node in the accessibility tree representing a UI element.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenNode {
    pub id: String,
    pub class_name: String,
    pub package_name: String,
    pub text: Option<String>,
    pub content_description: Option<String>,
    pub resource_id: Option<String>,
    pub bounds: Bounds,
    pub is_clickable: bool,
    pub is_scrollable: bool,
    pub is_editable: bool,
    pub is_checkable: bool,
    pub is_checked: bool,
    pub is_enabled: bool,
    pub is_focused: bool,
    pub is_visible: bool,
    /// Bounded at runtime to MAX_SCREEN_NODE_DEPTH levels — enforced by the accessibility bridge.
    pub children: Vec<ScreenNode>,
    pub depth: u8,
}

/// Bounding rectangle for a UI element in screen coordinates.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Bounds {
    #[must_use]
    pub fn center_x(&self) -> i32 {
        (self.left + self.right) / 2
    }

    #[must_use]
    pub fn center_y(&self) -> i32 {
        (self.top + self.bottom) / 2
    }

    #[must_use]
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    #[must_use]
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    #[must_use]
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }

    #[must_use]
    pub fn overlaps(&self, other: &Bounds) -> bool {
        self.left < other.right
            && self.right > other.left
            && self.top < other.bottom
            && self.bottom > other.top
    }
}

/// Complete accessibility tree snapshot for a screen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenTree {
    pub root: ScreenNode,
    pub package_name: String,
    pub activity_name: String,
    pub timestamp_ms: u64,
    pub node_count: u32,
}

impl ScreenTree {
    /// Find all nodes whose text matches the given string exactly.
    #[must_use]
    pub fn find_by_text(&self, text: &str) -> Vec<&ScreenNode> {
        let mut results = Vec::new();
        Self::collect_by_text(&self.root, text, &mut results);
        results
    }

    fn collect_by_text<'a>(node: &'a ScreenNode, text: &str, results: &mut Vec<&'a ScreenNode>) {
        if node.text.as_deref() == Some(text) {
            results.push(node);
        }
        for child in &node.children {
            Self::collect_by_text(child, text, results);
        }
    }

    /// Find all nodes with the given resource_id.
    #[must_use]
    pub fn find_by_resource_id(&self, resource_id: &str) -> Vec<&ScreenNode> {
        let mut results = Vec::new();
        Self::collect_by_resource_id(&self.root, resource_id, &mut results);
        results
    }

    fn collect_by_resource_id<'a>(
        node: &'a ScreenNode,
        resource_id: &str,
        results: &mut Vec<&'a ScreenNode>,
    ) {
        if node.resource_id.as_deref() == Some(resource_id) {
            results.push(node);
        }
        for child in &node.children {
            Self::collect_by_resource_id(child, resource_id, results);
        }
    }

    /// Find all nodes with the given content description.
    #[must_use]
    pub fn find_by_content_desc(&self, desc: &str) -> Vec<&ScreenNode> {
        let mut results = Vec::new();
        Self::collect_by_content_desc(&self.root, desc, &mut results);
        results
    }

    fn collect_by_content_desc<'a>(
        node: &'a ScreenNode,
        desc: &str,
        results: &mut Vec<&'a ScreenNode>,
    ) {
        if node.content_description.as_deref() == Some(desc) {
            results.push(node);
        }
        for child in &node.children {
            Self::collect_by_content_desc(child, desc, results);
        }
    }

    /// Find all clickable nodes in the tree.
    #[must_use]
    pub fn find_clickable(&self) -> Vec<&ScreenNode> {
        let mut results = Vec::new();
        Self::collect_clickable(&self.root, &mut results);
        results
    }

    fn collect_clickable<'a>(node: &'a ScreenNode, results: &mut Vec<&'a ScreenNode>) {
        if node.is_clickable {
            results.push(node);
        }
        for child in &node.children {
            Self::collect_clickable(child, results);
        }
    }

    /// Find all scrollable nodes in the tree.
    #[must_use]
    pub fn find_scrollable(&self) -> Vec<&ScreenNode> {
        let mut results = Vec::new();
        Self::collect_scrollable(&self.root, &mut results);
        results
    }

    fn collect_scrollable<'a>(node: &'a ScreenNode, results: &mut Vec<&'a ScreenNode>) {
        if node.is_scrollable {
            results.push(node);
        }
        for child in &node.children {
            Self::collect_scrollable(child, results);
        }
    }

    /// Collect all visible text from the entire tree.
    #[must_use]
    pub fn all_text(&self) -> Vec<String> {
        let mut results = Vec::new();
        Self::collect_all_text(&self.root, &mut results);
        results
    }

    fn collect_all_text(node: &ScreenNode, results: &mut Vec<String>) {
        if let Some(ref text) = node.text {
            if !text.is_empty() {
                results.push(text.clone());
            }
        }
        for child in &node.children {
            Self::collect_all_text(child, results);
        }
    }
}

/// Diff between two screen states — used for post-action verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenDiff {
    /// Bounded at runtime to MAX_SCREEN_DIFF_NODES entries — enforced by the diff engine.
    pub added_nodes: Vec<String>,
    /// Bounded at runtime to MAX_SCREEN_DIFF_NODES entries — enforced by the diff engine.
    pub removed_nodes: Vec<String>,
    /// Pairs of (node_id, description_of_change).
    /// Bounded at runtime to MAX_SCREEN_DIFF_NODES entries — enforced by the diff engine.
    pub changed_nodes: Vec<(String, String)>,
    pub screen_changed: bool,
}

/// Raw accessibility node as received from the Android JNI bridge before
/// full semantic processing.  Structurally identical to [`ScreenNode`]; the
/// alias exists so call-sites in the event pipeline can express intent
/// (raw, unprocessed tree) without a separate type definition.
pub type RawA11yNode = ScreenNode;

#[cfg(test)]
mod tests {
    use super::*;

    fn make_leaf(id: &str, text: Option<&str>, clickable: bool, scrollable: bool) -> ScreenNode {
        ScreenNode {
            id: id.to_string(),
            class_name: "android.widget.TextView".to_string(),
            package_name: "com.test".to_string(),
            text: text.map(|s| s.to_string()),
            content_description: None,
            resource_id: Some(format!("com.test:id/{}", id)),
            bounds: Bounds {
                left: 0,
                top: 0,
                right: 100,
                bottom: 50,
            },
            is_clickable: clickable,
            is_scrollable: scrollable,
            is_editable: false,
            is_checkable: false,
            is_checked: false,
            is_enabled: true,
            is_focused: false,
            is_visible: true,
            children: vec![],
            depth: 1,
        }
    }

    fn make_tree() -> ScreenTree {
        let child1 = make_leaf("btn_ok", Some("OK"), true, false);
        let child2 = make_leaf("txt_hello", Some("Hello"), false, false);
        let child3 = make_leaf("scroll_list", Some("item"), false, true);

        let root = ScreenNode {
            id: "root".to_string(),
            class_name: "android.widget.FrameLayout".to_string(),
            package_name: "com.test".to_string(),
            text: None,
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
            children: vec![child1, child2, child3],
            depth: 0,
        };

        ScreenTree {
            root,
            package_name: "com.test".to_string(),
            activity_name: ".MainActivity".to_string(),
            timestamp_ms: 1_700_000_000_000,
            node_count: 4,
        }
    }

    #[test]
    fn test_bounds_geometry() {
        let b = Bounds {
            left: 10,
            top: 20,
            right: 110,
            bottom: 70,
        };
        assert_eq!(b.center_x(), 60);
        assert_eq!(b.center_y(), 45);
        assert_eq!(b.width(), 100);
        assert_eq!(b.height(), 50);
        assert!(b.contains(60, 45));
        assert!(!b.contains(0, 0));
    }

    #[test]
    fn test_bounds_overlaps() {
        let a = Bounds {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        };
        let b = Bounds {
            left: 50,
            top: 50,
            right: 150,
            bottom: 150,
        };
        let c = Bounds {
            left: 200,
            top: 200,
            right: 300,
            bottom: 300,
        };
        assert!(a.overlaps(&b));
        assert!(!a.overlaps(&c));
    }

    #[test]
    fn test_screen_tree_find_by_text() {
        let tree = make_tree();
        let ok_nodes = tree.find_by_text("OK");
        assert_eq!(ok_nodes.len(), 1);
        assert_eq!(ok_nodes[0].id, "btn_ok");

        let missing = tree.find_by_text("nonexistent");
        assert!(missing.is_empty());
    }

    #[test]
    fn test_screen_tree_find_clickable_and_scrollable() {
        let tree = make_tree();
        let clickable = tree.find_clickable();
        assert_eq!(clickable.len(), 1);
        assert_eq!(clickable[0].id, "btn_ok");

        let scrollable = tree.find_scrollable();
        assert_eq!(scrollable.len(), 1);
        assert_eq!(scrollable[0].id, "scroll_list");
    }

    #[test]
    fn test_screen_tree_all_text() {
        let tree = make_tree();
        let texts = tree.all_text();
        assert_eq!(texts.len(), 3);
        assert!(texts.contains(&"OK".to_string()));
        assert!(texts.contains(&"Hello".to_string()));
    }

    #[test]
    fn test_screen_tree_find_by_resource_id() {
        let tree = make_tree();
        let nodes = tree.find_by_resource_id("com.test:id/btn_ok");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "btn_ok");
    }
}
