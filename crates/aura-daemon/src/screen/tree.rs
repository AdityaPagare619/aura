use aura_types::screen::{Bounds, ScreenNode, ScreenTree};
use serde::{Deserialize, Serialize};

/// Maximum depth of the accessibility tree we will parse.
const MAX_TREE_DEPTH: u8 = 30;
/// Maximum total nodes we will parse (prevents OOM on complex screens).
const MAX_TOTAL_NODES: usize = 5000;

/// Raw accessibility node received from JNI.
/// This maps directly to the fields Android's AccessibilityNodeInfo exposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawA11yNode {
    pub class_name: String,
    pub text: Option<String>,
    pub content_desc: Option<String>,
    pub resource_id: Option<String>,
    pub package_name: String,
    pub bounds_left: i32,
    pub bounds_top: i32,
    pub bounds_right: i32,
    pub bounds_bottom: i32,
    pub is_clickable: bool,
    pub is_scrollable: bool,
    pub is_editable: bool,
    pub is_checkable: bool,
    pub is_checked: bool,
    pub is_enabled: bool,
    pub is_focused: bool,
    pub is_visible: bool,
    pub children_indices: Vec<usize>,
}

/// Parse a flat array of raw accessibility nodes into a hierarchical ScreenTree.
///
/// The first element (index 0) is treated as the root. We do BFS from root,
/// capping depth at MAX_TREE_DEPTH and total nodes at MAX_TOTAL_NODES.
pub fn parse_tree(raw_nodes: &[RawA11yNode]) -> ScreenTree {
    if raw_nodes.is_empty() {
        return ScreenTree {
            root: make_empty_root(),
            package_name: String::new(),
            activity_name: String::new(),
            timestamp_ms: current_timestamp_ms(),
            node_count: 0,
        };
    }

    let mut node_count: usize = 0;
    let root = build_node(raw_nodes, 0, 0, &mut node_count);

    let package_name = raw_nodes[0].package_name.clone();

    ScreenTree {
        root,
        package_name,
        activity_name: String::new(),
        timestamp_ms: current_timestamp_ms(),
        node_count: node_count as u32,
    }
}

/// Recursively build a ScreenNode from a RawA11yNode, honoring depth and count limits.
fn build_node(
    raw_nodes: &[RawA11yNode],
    index: usize,
    depth: u8,
    node_count: &mut usize,
) -> ScreenNode {
    if *node_count >= MAX_TOTAL_NODES || depth > MAX_TREE_DEPTH || index >= raw_nodes.len() {
        return make_empty_root();
    }

    *node_count += 1;
    let raw = &raw_nodes[index];

    let children: Vec<ScreenNode> = if depth < MAX_TREE_DEPTH {
        let valid_indices: Vec<usize> = raw
            .children_indices
            .iter()
            .filter(|&&ci| ci < raw_nodes.len())
            .copied()
            .collect();
        let mut result = Vec::new();
        for ci in valid_indices {
            if *node_count >= MAX_TOTAL_NODES {
                break;
            }
            result.push(build_node(raw_nodes, ci, depth + 1, node_count));
        }
        result
    } else {
        Vec::new()
    };

    ScreenNode {
        id: format!("node_{index}"),
        class_name: raw.class_name.clone(),
        package_name: raw.package_name.clone(),
        text: raw.text.clone(),
        content_description: raw.content_desc.clone(),
        resource_id: raw.resource_id.clone(),
        bounds: Bounds {
            left: raw.bounds_left,
            top: raw.bounds_top,
            right: raw.bounds_right,
            bottom: raw.bounds_bottom,
        },
        is_clickable: raw.is_clickable,
        is_scrollable: raw.is_scrollable,
        is_editable: raw.is_editable,
        is_checkable: raw.is_checkable,
        is_checked: raw.is_checked,
        is_enabled: raw.is_enabled,
        is_focused: raw.is_focused,
        is_visible: raw.is_visible,
        children,
        depth,
    }
}

fn make_empty_root() -> ScreenNode {
    ScreenNode {
        id: "empty".to_string(),
        class_name: String::new(),
        package_name: String::new(),
        text: None,
        content_description: None,
        resource_id: None,
        bounds: Bounds {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        is_clickable: false,
        is_scrollable: false,
        is_editable: false,
        is_checkable: false,
        is_checked: false,
        is_enabled: false,
        is_focused: false,
        is_visible: false,
        children: Vec::new(),
        depth: 0,
    }
}

fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Extended ScreenTree helpers ──────────────────────────────────────────────

/// Extension trait for ScreenTree with additional search/query methods
/// beyond what aura-types provides.
pub trait ScreenTreeExt {
    /// Case-insensitive contains search for text nodes.
    fn find_by_text_contains(&self, text: &str) -> Vec<&ScreenNode>;
    /// Find first node with an exact resource_id match.
    fn find_first_by_resource_id(&self, id: &str) -> Option<&ScreenNode>;
    /// Find first node whose content_description contains the given string.
    fn find_by_content_desc_contains(&self, desc: &str) -> Option<&ScreenNode>;
    /// Generate an XPath-like path from root to the given node.
    fn to_xpath(&self, node: &ScreenNode) -> String;
    /// Find node at given coordinates.
    fn find_at_coordinates(&self, x: i32, y: i32) -> Option<&ScreenNode>;
    /// Find all editable nodes.
    fn find_editable(&self) -> Vec<&ScreenNode>;
}

impl ScreenTreeExt for ScreenTree {
    fn find_by_text_contains(&self, text: &str) -> Vec<&ScreenNode> {
        let lower = text.to_lowercase();
        let mut results = Vec::new();
        collect_text_contains(&self.root, &lower, &mut results);
        results
    }

    fn find_first_by_resource_id(&self, id: &str) -> Option<&ScreenNode> {
        find_first_resource_id(&self.root, id)
    }

    fn find_by_content_desc_contains(&self, desc: &str) -> Option<&ScreenNode> {
        let lower = desc.to_lowercase();
        find_content_desc_contains(&self.root, &lower)
    }

    fn to_xpath(&self, node: &ScreenNode) -> String {
        let mut path_segments = Vec::new();
        build_xpath_path(&self.root, &node.id, &mut path_segments);
        if path_segments.is_empty() {
            format!("//{}", node.class_name)
        } else {
            path_segments.reverse();
            let mut xpath = String::new();
            for seg in &path_segments {
                xpath.push('/');
                xpath.push_str(seg);
            }
            xpath
        }
    }

    fn find_at_coordinates(&self, x: i32, y: i32) -> Option<&ScreenNode> {
        find_deepest_at_coords(&self.root, x, y)
    }

    fn find_editable(&self) -> Vec<&ScreenNode> {
        let mut results = Vec::new();
        collect_editable(&self.root, &mut results);
        results
    }
}

fn collect_text_contains<'a>(node: &'a ScreenNode, lower: &str, results: &mut Vec<&'a ScreenNode>) {
    if let Some(ref text) = node.text {
        if text.to_lowercase().contains(lower) {
            results.push(node);
        }
    }
    if let Some(ref desc) = node.content_description {
        if desc.to_lowercase().contains(lower) && !results.iter().any(|n| n.id == node.id) {
            results.push(node);
        }
    }
    for child in &node.children {
        collect_text_contains(child, lower, results);
    }
}

fn find_first_resource_id<'a>(node: &'a ScreenNode, id: &str) -> Option<&'a ScreenNode> {
    if node.resource_id.as_deref() == Some(id) {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_first_resource_id(child, id) {
            return Some(found);
        }
    }
    None
}

fn find_content_desc_contains<'a>(node: &'a ScreenNode, lower: &str) -> Option<&'a ScreenNode> {
    if let Some(ref desc) = node.content_description {
        if desc.to_lowercase().contains(lower) {
            return Some(node);
        }
    }
    for child in &node.children {
        if let Some(found) = find_content_desc_contains(child, lower) {
            return Some(found);
        }
    }
    None
}

/// Build an xpath by searching for a node with `target_id` in the subtree.
/// Returns true if found, populating `path` in reverse order (leaf first).
fn build_xpath_path(node: &ScreenNode, target_id: &str, path: &mut Vec<String>) -> bool {
    if node.id == target_id {
        let segment = xpath_segment(node);
        path.push(segment);
        return true;
    }
    for (i, child) in node.children.iter().enumerate() {
        if build_xpath_path(child, target_id, path) {
            // Count siblings with same class_name for positional index
            let same_class_count = node
                .children
                .iter()
                .filter(|c| c.class_name == child.class_name)
                .count();
            let segment = if same_class_count > 1 {
                // Find position among same-class siblings
                let pos = node
                    .children
                    .iter()
                    .filter(|c| c.class_name == child.class_name)
                    .position(|c| c.id == child.id)
                    .unwrap_or(i)
                    + 1;
                format!("{}[{}]", short_class(&node.class_name), pos)
            } else {
                short_class(&node.class_name)
            };
            path.push(segment);
            return true;
        }
    }
    false
}

fn xpath_segment(node: &ScreenNode) -> String {
    let base = short_class(&node.class_name);
    if let Some(ref rid) = node.resource_id {
        format!("{base}[@resource-id='{rid}']")
    } else if let Some(ref text) = node.text {
        format!("{base}[@text='{text}']")
    } else {
        base
    }
}

fn short_class(class_name: &str) -> String {
    // Extract simple class name from fully qualified Java class
    class_name
        .rsplit('.')
        .next()
        .unwrap_or(class_name)
        .to_string()
}

fn find_deepest_at_coords(node: &ScreenNode, x: i32, y: i32) -> Option<&ScreenNode> {
    if !node.bounds.contains(x, y) {
        return None;
    }
    // Check children depth-first; return deepest match
    for child in &node.children {
        if let Some(found) = find_deepest_at_coords(child, x, y) {
            return Some(found);
        }
    }
    // This node contains the coords and no child does
    Some(node)
}

fn collect_editable<'a>(node: &'a ScreenNode, results: &mut Vec<&'a ScreenNode>) {
    if node.is_editable {
        results.push(node);
    }
    for child in &node.children {
        collect_editable(child, results);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_raw_nodes() -> Vec<RawA11yNode> {
        vec![
            RawA11yNode {
                class_name: "android.widget.FrameLayout".into(),
                text: None,
                content_desc: None,
                resource_id: None,
                package_name: "com.test".into(),
                bounds_left: 0,
                bounds_top: 0,
                bounds_right: 1080,
                bounds_bottom: 1920,
                is_clickable: false,
                is_scrollable: false,
                is_editable: false,
                is_checkable: false,
                is_checked: false,
                is_enabled: true,
                is_focused: false,
                is_visible: true,
                children_indices: vec![1, 2, 3],
            },
            RawA11yNode {
                class_name: "android.widget.Button".into(),
                text: Some("Send".into()),
                content_desc: Some("Send message".into()),
                resource_id: Some("com.test:id/send_btn".into()),
                package_name: "com.test".into(),
                bounds_left: 800,
                bounds_top: 1700,
                bounds_right: 1000,
                bounds_bottom: 1800,
                is_clickable: true,
                is_scrollable: false,
                is_editable: false,
                is_checkable: false,
                is_checked: false,
                is_enabled: true,
                is_focused: false,
                is_visible: true,
                children_indices: vec![],
            },
            RawA11yNode {
                class_name: "android.widget.EditText".into(),
                text: Some("Hello".into()),
                content_desc: None,
                resource_id: Some("com.test:id/input".into()),
                package_name: "com.test".into(),
                bounds_left: 0,
                bounds_top: 1700,
                bounds_right: 780,
                bounds_bottom: 1800,
                is_clickable: true,
                is_scrollable: false,
                is_editable: true,
                is_checkable: false,
                is_checked: false,
                is_enabled: true,
                is_focused: true,
                is_visible: true,
                children_indices: vec![],
            },
            RawA11yNode {
                class_name: "android.widget.ScrollView".into(),
                text: None,
                content_desc: None,
                resource_id: Some("com.test:id/scroll".into()),
                package_name: "com.test".into(),
                bounds_left: 0,
                bounds_top: 0,
                bounds_right: 1080,
                bounds_bottom: 1700,
                is_clickable: false,
                is_scrollable: true,
                is_editable: false,
                is_checkable: false,
                is_checked: false,
                is_enabled: true,
                is_focused: false,
                is_visible: true,
                children_indices: vec![],
            },
        ]
    }

    #[test]
    fn test_parse_tree_basic() {
        let raw = make_raw_nodes();
        let tree = parse_tree(&raw);
        assert_eq!(tree.node_count, 4);
        assert_eq!(tree.package_name, "com.test");
        assert_eq!(tree.root.children.len(), 3);
    }

    #[test]
    fn test_parse_tree_empty() {
        let tree = parse_tree(&[]);
        assert_eq!(tree.node_count, 0);
    }

    #[test]
    fn test_find_by_text_contains() {
        let raw = make_raw_nodes();
        let tree = parse_tree(&raw);
        let results = tree.find_by_text_contains("send");
        assert!(!results.is_empty());
        assert!(results[0].text.as_deref() == Some("Send"));
    }

    #[test]
    fn test_find_first_by_resource_id() {
        let raw = make_raw_nodes();
        let tree = parse_tree(&raw);
        let node = tree.find_first_by_resource_id("com.test:id/send_btn");
        assert!(node.is_some());
        assert_eq!(node.unwrap().text.as_deref(), Some("Send"));
    }

    #[test]
    fn test_find_content_desc_contains() {
        let raw = make_raw_nodes();
        let tree = parse_tree(&raw);
        let node = tree.find_by_content_desc_contains("send message");
        assert!(node.is_some());
    }

    #[test]
    fn test_find_at_coordinates() {
        let raw = make_raw_nodes();
        let tree = parse_tree(&raw);
        // Should find the Send button at its center
        let node = tree.find_at_coordinates(900, 1750);
        assert!(node.is_some());
        assert_eq!(node.unwrap().text.as_deref(), Some("Send"));
    }

    #[test]
    fn test_find_editable() {
        let raw = make_raw_nodes();
        let tree = parse_tree(&raw);
        let editable = tree.find_editable();
        assert_eq!(editable.len(), 1);
        assert!(editable[0].is_editable);
    }

    #[test]
    fn test_to_xpath() {
        let raw = make_raw_nodes();
        let tree = parse_tree(&raw);
        let send = tree
            .find_first_by_resource_id("com.test:id/send_btn")
            .unwrap();
        let xpath = tree.to_xpath(send);
        assert!(xpath.contains("Button"));
        assert!(xpath.contains("send_btn"));
    }

    #[test]
    fn test_max_nodes_cap() {
        // Create a tree exceeding the cap
        let mut raw = Vec::new();
        // Root
        raw.push(RawA11yNode {
            class_name: "root".into(),
            text: None,
            content_desc: None,
            resource_id: None,
            package_name: "com.test".into(),
            bounds_left: 0,
            bounds_top: 0,
            bounds_right: 100,
            bounds_bottom: 100,
            is_clickable: false,
            is_scrollable: false,
            is_editable: false,
            is_checkable: false,
            is_checked: false,
            is_enabled: true,
            is_focused: false,
            is_visible: true,
            children_indices: (1..5001).collect(),
        });
        // 5000 child nodes
        for i in 1..=5000 {
            raw.push(RawA11yNode {
                class_name: format!("child_{i}"),
                text: None,
                content_desc: None,
                resource_id: None,
                package_name: "com.test".into(),
                bounds_left: 0,
                bounds_top: 0,
                bounds_right: 10,
                bounds_bottom: 10,
                is_clickable: false,
                is_scrollable: false,
                is_editable: false,
                is_checkable: false,
                is_checked: false,
                is_enabled: true,
                is_focused: false,
                is_visible: true,
                children_indices: vec![],
            });
        }
        let tree = parse_tree(&raw);
        // Should be capped at MAX_TOTAL_NODES (5000)
        assert!(tree.node_count <= 5000);
    }
}
