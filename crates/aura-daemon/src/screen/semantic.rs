//! Screen Semantic Graph — structural understanding of UI screens.
//!
//! Builds a semantic layer on top of the raw accessibility tree, giving AURA
//! the ability to perceive *patterns* (login form, list view, dialog) rather
//! than isolated elements. This is the perception backbone: AURA's "eyes".
//!
//! ## Architecture
//! - [`SemanticGraph`] is a directed graph of [`SemanticNode`]s connected by
//!   typed [`SemanticEdge`]s (contains, follows, labels, controls, groups_with).
//! - Pattern recognition identifies common UI idioms (login, dialog, list, nav).
//! - Landmark detection finds key navigational anchors (primary CTA, back button).
//! - State inference answers "what is the app doing?" at a structural level.

use std::collections::HashMap;

use aura_types::screen::{ScreenNode, ScreenTree};
use tracing::{debug, trace};

// ── Semantic element types ──────────────────────────────────────────────────

/// High-level classification of a UI element's semantic role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ElementKind {
    Button,
    TextField,
    Label,
    Image,
    Container,
    ListItem,
    Toggle,
    Checkbox,
    Scrollable,
    ProgressBar,
    Divider,
    NavigationBar,
    TabBar,
    Toolbar,
    Dialog,
    Unknown,
}

/// Relationship between two semantic nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    /// Parent contains child.
    Contains,
    /// This element visually/sequentially follows another.
    Follows,
    /// A label that describes another element (e.g. "Email" label → text field).
    Labels,
    /// An element that controls another (e.g. toggle → setting row).
    Controls,
    /// Two elements that are grouped together (same logical form/section).
    GroupsWith,
}

// ── Recognized UI patterns ──────────────────────────────────────────────────

/// A recognized high-level UI pattern on the screen.
#[derive(Debug, Clone, PartialEq)]
pub enum UiPattern {
    /// Login/auth form: text fields for credentials + submit button.
    LoginForm {
        email_field: Option<String>,
        password_field: Option<String>,
        submit_button: Option<String>,
    },
    /// List view: scrollable container with repeated item structure.
    ListView {
        container_id: String,
        item_count: u32,
    },
    /// Dialog / popup overlay with title, message, and action buttons.
    Dialog {
        title: Option<String>,
        message: Option<String>,
        buttons: Vec<String>,
    },
    /// Navigation element: tab bar, drawer, or back button.
    Navigation {
        nav_type: NavigationType,
        items: Vec<String>,
    },
    /// Settings page: label + toggle/dropdown pairs.
    SettingsPage {
        setting_pairs: Vec<(String, String)>,
    },
    /// Search bar with input field and optional filter.
    SearchBar { input_field: String },
}

/// Sub-type of navigation patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationType {
    TabBar,
    Drawer,
    BackButton,
    BottomNavigation,
    Toolbar,
}

// ── Landmarks ───────────────────────────────────────────────────────────────

/// A key navigational anchor on the screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Landmark {
    /// The node ID this landmark refers to.
    pub node_id: String,
    /// What kind of landmark this is.
    pub kind: LandmarkKind,
    /// Confidence in this classification (0.0–1.0).
    pub confidence: f32,
}

/// Classification of navigational landmarks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandmarkKind {
    /// Primary call-to-action button (e.g. "Send", "Submit", "OK").
    PrimaryAction,
    /// Text input field expecting user input.
    TextInput,
    /// Back / close / cancel button.
    DismissAction,
    /// Scrollable region.
    ScrollRegion,
    /// Search input.
    SearchInput,
}

// ── State inference ─────────────────────────────────────────────────────────

/// Inferred semantic state of the current screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenSemanticState {
    /// Normal interactive state — user can act.
    Interactive,
    /// Loading: spinner, progress bar, or "loading" text visible.
    Loading,
    /// Error condition: red text, error icons, error messages.
    Error,
    /// Success confirmation: green checkmark, success messages.
    Success,
    /// Input required: focused text field, keyboard likely visible.
    InputRequired,
    /// Blocked by dialog/overlay.
    Blocked,
    /// Empty or transitional screen.
    Transitional,
}

// ── Semantic node and edge ──────────────────────────────────────────────────

/// A node in the semantic graph, wrapping a raw screen node with classification.
#[derive(Debug, Clone)]
pub struct SemanticNode {
    /// Original screen node ID.
    pub node_id: String,
    /// Classified element kind.
    pub kind: ElementKind,
    /// Display text (text or content description).
    pub display_text: Option<String>,
    /// Resource ID if available.
    pub resource_id: Option<String>,
    /// Whether this element is interactive.
    pub is_interactive: bool,
    /// Bounding box center (x, y).
    pub center: (i32, i32),
    /// Tree depth in original a11y tree.
    pub depth: u8,
}

/// A directed edge in the semantic graph.
#[derive(Debug, Clone)]
pub struct SemanticEdge {
    /// Source node ID.
    pub from: String,
    /// Target node ID.
    pub to: String,
    /// Relationship type.
    pub kind: EdgeKind,
    /// Confidence in this relationship (0.0–1.0).
    pub confidence: f32,
}

// ── The semantic graph ──────────────────────────────────────────────────────

/// A semantic graph built from a screen tree, providing structural
/// understanding of the UI.
#[derive(Debug, Clone)]
pub struct SemanticGraph {
    /// All semantic nodes, keyed by original node ID.
    pub nodes: HashMap<String, SemanticNode>,
    /// All edges in the graph.
    pub edges: Vec<SemanticEdge>,
    /// Recognized UI patterns.
    pub patterns: Vec<UiPattern>,
    /// Detected landmarks.
    pub landmarks: Vec<Landmark>,
    /// Inferred screen state.
    pub state: ScreenSemanticState,
    /// Source package name.
    pub package_name: String,
    /// Source activity name.
    pub activity_name: String,
    /// Timestamp when this graph was built (ms since epoch).
    pub built_at_ms: u64,
}

impl SemanticGraph {
    /// Build a semantic graph from a raw screen tree.
    ///
    /// This is the main entry point: parses the tree, classifies elements,
    /// infers relationships, detects patterns and landmarks, and infers state.
    pub fn from_tree(tree: &ScreenTree) -> Self {
        let start = std::time::Instant::now();

        let mut nodes = HashMap::new();
        let mut edges = Vec::new();

        // Phase 1: classify every visible node
        classify_subtree(&tree.root, &mut nodes);

        // Phase 2: infer relationships between nodes
        infer_edges(&tree.root, &nodes, &mut edges);

        // Phase 3: detect patterns
        let patterns = detect_patterns(&nodes, &edges, &tree.root);

        // Phase 4: find landmarks
        let landmarks = detect_landmarks(&nodes);

        // Phase 5: infer screen state
        let state = infer_state(&nodes, &patterns, tree);

        let elapsed_us = start.elapsed().as_micros();
        debug!(
            nodes = nodes.len(),
            edges = edges.len(),
            patterns = patterns.len(),
            landmarks = landmarks.len(),
            ?state,
            elapsed_us,
            "semantic graph built"
        );

        Self {
            nodes,
            edges,
            patterns,
            landmarks,
            state,
            package_name: tree.package_name.clone(),
            activity_name: tree.activity_name.clone(),
            built_at_ms: tree.timestamp_ms,
        }
    }

    /// Find semantic nodes by element kind.
    pub fn nodes_of_kind(&self, kind: ElementKind) -> Vec<&SemanticNode> {
        self.nodes.values().filter(|n| n.kind == kind).collect()
    }

    /// Find all interactive nodes.
    pub fn interactive_nodes(&self) -> Vec<&SemanticNode> {
        self.nodes.values().filter(|n| n.is_interactive).collect()
    }

    /// Find edges of a specific kind originating from a node.
    pub fn edges_from(&self, node_id: &str, kind: EdgeKind) -> Vec<&SemanticEdge> {
        self.edges
            .iter()
            .filter(|e| e.from == node_id && e.kind == kind)
            .collect()
    }

    /// Find edges of a specific kind pointing to a node.
    pub fn edges_to(&self, node_id: &str, kind: EdgeKind) -> Vec<&SemanticEdge> {
        self.edges
            .iter()
            .filter(|e| e.to == node_id && e.kind == kind)
            .collect()
    }

    /// Get the label text for a node (via Labels edges).
    pub fn label_for(&self, node_id: &str) -> Option<&str> {
        let label_edges = self.edges_to(node_id, EdgeKind::Labels);
        for edge in label_edges {
            if let Some(label_node) = self.nodes.get(&edge.from) {
                if let Some(ref text) = label_node.display_text {
                    return Some(text.as_str());
                }
            }
        }
        None
    }

    /// Produce a compact text summary for LLM consumption.
    pub fn summary_for_llm(&self) -> String {
        let mut parts = Vec::new();

        parts.push(format!(
            "App: {} Activity: {}",
            self.package_name, self.activity_name
        ));
        parts.push(format!("State: {:?}", self.state));

        if !self.patterns.is_empty() {
            let pattern_names: Vec<String> = self
                .patterns
                .iter()
                .map(|p| match p {
                    UiPattern::LoginForm { .. } => "LoginForm".to_string(),
                    UiPattern::ListView { item_count, .. } => {
                        format!("ListView({item_count} items)")
                    }
                    UiPattern::Dialog { title, .. } => {
                        format!("Dialog({})", title.as_deref().unwrap_or("untitled"))
                    }
                    UiPattern::Navigation { nav_type, .. } => format!("Nav({nav_type:?})"),
                    UiPattern::SettingsPage { setting_pairs } => {
                        format!("Settings({} pairs)", setting_pairs.len())
                    }
                    UiPattern::SearchBar { .. } => "SearchBar".to_string(),
                })
                .collect();
            parts.push(format!("Patterns: {}", pattern_names.join(", ")));
        }

        if !self.landmarks.is_empty() {
            let landmark_descs: Vec<String> = self
                .landmarks
                .iter()
                .filter(|l| l.confidence >= 0.6)
                .map(|l| {
                    let text = self
                        .nodes
                        .get(&l.node_id)
                        .and_then(|n| n.display_text.as_deref())
                        .unwrap_or("?");
                    format!("{:?}('{}')", l.kind, text)
                })
                .collect();
            parts.push(format!("Landmarks: {}", landmark_descs.join(", ")));
        }

        // List interactive elements
        let interactive: Vec<String> = self
            .interactive_nodes()
            .into_iter()
            .take(20) // cap for token budget
            .map(|n| {
                let text = n.display_text.as_deref().unwrap_or("?");
                format!("{:?}('{}' @{},{})", n.kind, text, n.center.0, n.center.1)
            })
            .collect();
        if !interactive.is_empty() {
            parts.push(format!("Interactive: {}", interactive.join("; ")));
        }

        parts.join("\n")
    }

    /// Estimated memory usage in bytes for cache budgeting.
    pub fn estimated_size_bytes(&self) -> usize {
        let node_size: usize = self
            .nodes
            .values()
            .map(|n| {
                64 + n.node_id.len()
                    + n.display_text.as_ref().map_or(0, |t| t.len())
                    + n.resource_id.as_ref().map_or(0, |r| r.len())
            })
            .sum();
        let edge_size = self.edges.len() * 80;
        let pattern_size = self.patterns.len() * 128;
        let landmark_size = self.landmarks.len() * 64;
        node_size + edge_size + pattern_size + landmark_size + 256
    }
}

// ── Phase 1: Element classification ─────────────────────────────────────────

/// Recursively classify every visible node in the tree.
fn classify_subtree(node: &ScreenNode, out: &mut HashMap<String, SemanticNode>) {
    if !node.is_visible {
        return;
    }

    let kind = classify_element(node);
    let display_text = node
        .text
        .clone()
        .or_else(|| node.content_description.clone());

    let semantic = SemanticNode {
        node_id: node.id.clone(),
        kind,
        display_text,
        resource_id: node.resource_id.clone(),
        is_interactive: node.is_clickable || node.is_editable || node.is_checkable,
        center: (node.bounds.center_x(), node.bounds.center_y()),
        depth: node.depth,
    };

    trace!(id = %node.id, ?kind, "classified element");
    out.insert(node.id.clone(), semantic);

    for child in &node.children {
        classify_subtree(child, out);
    }
}

/// Classify a single screen node into a semantic element kind.
fn classify_element(node: &ScreenNode) -> ElementKind {
    let class_short = node
        .class_name
        .rsplit('.')
        .next()
        .unwrap_or(&node.class_name);

    match class_short {
        "Button"
        | "ImageButton"
        | "MaterialButton"
        | "AppCompatButton"
        | "FloatingActionButton"
        | "ExtendedFloatingActionButton" => ElementKind::Button,

        "EditText"
        | "TextInputEditText"
        | "AutoCompleteTextView"
        | "AppCompatEditText"
        | "SearchEditText" => ElementKind::TextField,

        "TextView" | "AppCompatTextView" | "MaterialTextView" => {
            // A clickable TextView is effectively a button
            if node.is_clickable && !node.is_editable {
                ElementKind::Button
            } else {
                ElementKind::Label
            }
        }

        "ImageView" | "AppCompatImageView" | "CircleImageView" | "ShapeableImageView" => {
            ElementKind::Image
        }

        "Switch" | "SwitchCompat" | "SwitchMaterial" | "ToggleButton" | "CompoundButton" => {
            ElementKind::Toggle
        }

        "CheckBox"
        | "AppCompatCheckBox"
        | "MaterialCheckBox"
        | "RadioButton"
        | "AppCompatRadioButton" => ElementKind::Checkbox,

        "ScrollView"
        | "NestedScrollView"
        | "HorizontalScrollView"
        | "RecyclerView"
        | "ListView"
        | "GridView" => {
            if node.is_scrollable {
                ElementKind::Scrollable
            } else {
                ElementKind::Container
            }
        }

        "ProgressBar"
        | "CircularProgressIndicator"
        | "LinearProgressIndicator"
        | "ContentLoadingProgressBar" => ElementKind::ProgressBar,

        "BottomNavigationView" | "NavigationBarView" | "BottomNavigationItemView" => {
            ElementKind::NavigationBar
        }

        "TabLayout" | "TabItem" | "TabView" => ElementKind::TabBar,

        "Toolbar" | "ActionBar" | "MaterialToolbar" => ElementKind::Toolbar,

        "FrameLayout" | "LinearLayout" | "RelativeLayout" | "ConstraintLayout"
        | "CoordinatorLayout" | "CardView" | "ViewGroup" | "View" => ElementKind::Container,

        _ => {
            // Fallback heuristics for unrecognized classes
            if node.is_editable {
                ElementKind::TextField
            } else if node.is_checkable {
                ElementKind::Checkbox
            } else if node.is_scrollable {
                ElementKind::Scrollable
            } else if node.is_clickable && node.text.is_some() {
                ElementKind::Button
            } else if node.children.is_empty() && node.text.is_some() {
                ElementKind::Label
            } else {
                ElementKind::Unknown
            }
        }
    }
}

// ── Phase 2: Relationship inference ─────────────────────────────────────────

/// Infer edges between semantic nodes based on tree structure and spatial layout.
fn infer_edges(
    node: &ScreenNode,
    nodes: &HashMap<String, SemanticNode>,
    edges: &mut Vec<SemanticEdge>,
) {
    if !node.is_visible {
        return;
    }

    // Contains edges: parent → child
    for child in &node.children {
        if child.is_visible && nodes.contains_key(&child.id) {
            edges.push(SemanticEdge {
                from: node.id.clone(),
                to: child.id.clone(),
                kind: EdgeKind::Contains,
                confidence: 1.0,
            });
        }
    }

    // Follows edges: sequential siblings
    let visible_children: Vec<&ScreenNode> =
        node.children.iter().filter(|c| c.is_visible).collect();
    for window in visible_children.windows(2) {
        if nodes.contains_key(&window[0].id) && nodes.contains_key(&window[1].id) {
            edges.push(SemanticEdge {
                from: window[0].id.clone(),
                to: window[1].id.clone(),
                kind: EdgeKind::Follows,
                confidence: 0.9,
            });
        }
    }

    // Labels edges: a Label node immediately before an interactive node
    for window in visible_children.windows(2) {
        let first = nodes.get(&window[0].id);
        let second = nodes.get(&window[1].id);
        if let (Some(f), Some(s)) = (first, second) {
            // Label → TextField/Toggle/Checkbox
            if f.kind == ElementKind::Label
                && matches!(
                    s.kind,
                    ElementKind::TextField | ElementKind::Toggle | ElementKind::Checkbox
                )
            {
                edges.push(SemanticEdge {
                    from: window[0].id.clone(),
                    to: window[1].id.clone(),
                    kind: EdgeKind::Labels,
                    confidence: 0.85,
                });
            }
        }
    }

    // GroupsWith edges: siblings of the same kind in the same container
    let mut kind_groups: HashMap<ElementKind, Vec<String>> = HashMap::new();
    for child in &visible_children {
        if let Some(sn) = nodes.get(&child.id) {
            if sn.is_interactive {
                kind_groups
                    .entry(sn.kind)
                    .or_default()
                    .push(child.id.clone());
            }
        }
    }
    for (_kind, group) in &kind_groups {
        if group.len() >= 2 && group.len() <= 10 {
            for i in 0..group.len() {
                for j in (i + 1)..group.len() {
                    edges.push(SemanticEdge {
                        from: group[i].clone(),
                        to: group[j].clone(),
                        kind: EdgeKind::GroupsWith,
                        confidence: 0.7,
                    });
                }
            }
        }
    }

    // Recurse
    for child in &node.children {
        infer_edges(child, nodes, edges);
    }
}

// ── Phase 3: Pattern detection ──────────────────────────────────────────────

/// Detect high-level UI patterns from the semantic graph.
fn detect_patterns(
    nodes: &HashMap<String, SemanticNode>,
    _edges: &[SemanticEdge],
    root: &ScreenNode,
) -> Vec<UiPattern> {
    let mut patterns = Vec::new();

    // Collect all text for pattern matching
    let all_text_lower: Vec<String> = nodes
        .values()
        .filter_map(|n| n.display_text.as_ref())
        .map(|t| t.to_lowercase())
        .collect();
    let joined_text = all_text_lower.join(" ");

    // Login form detection
    if let Some(login) = detect_login_form(nodes, &joined_text) {
        patterns.push(login);
    }

    // Dialog detection
    if let Some(dialog) = detect_dialog(nodes, root) {
        patterns.push(dialog);
    }

    // ListView detection
    if let Some(list) = detect_list_view(nodes, root) {
        patterns.push(list);
    }

    // Navigation detection
    if let Some(nav) = detect_navigation(nodes) {
        patterns.push(nav);
    }

    // Settings page detection
    if let Some(settings) = detect_settings_page(nodes, &joined_text) {
        patterns.push(settings);
    }

    // Search bar detection
    if let Some(search) = detect_search_bar(nodes, &joined_text) {
        patterns.push(search);
    }

    patterns
}

fn detect_login_form(
    nodes: &HashMap<String, SemanticNode>,
    joined_text: &str,
) -> Option<UiPattern> {
    let login_keywords = [
        "sign in", "log in", "login", "email", "password", "username",
    ];
    let keyword_hits: u32 = login_keywords
        .iter()
        .filter(|k| joined_text.contains(**k))
        .count() as u32;

    if keyword_hits < 2 {
        return None;
    }

    let text_fields: Vec<&SemanticNode> = nodes
        .values()
        .filter(|n| n.kind == ElementKind::TextField)
        .collect();

    if text_fields.is_empty() {
        return None;
    }

    let email_field = text_fields.iter().find(|n| {
        n.display_text
            .as_ref()
            .map(|t| {
                let lower = t.to_lowercase();
                lower.contains("email") || lower.contains("username") || lower.contains("phone")
            })
            .unwrap_or(false)
            || n.resource_id
                .as_ref()
                .map(|r| {
                    let lower = r.to_lowercase();
                    lower.contains("email") || lower.contains("username")
                })
                .unwrap_or(false)
    });

    let password_field = text_fields.iter().find(|n| {
        n.display_text
            .as_ref()
            .map(|t| t.to_lowercase().contains("password"))
            .unwrap_or(false)
            || n.resource_id
                .as_ref()
                .map(|r| r.to_lowercase().contains("password"))
                .unwrap_or(false)
    });

    let submit_button = nodes.values().find(|n| {
        n.kind == ElementKind::Button
            && n.display_text
                .as_ref()
                .map(|t| {
                    let lower = t.to_lowercase();
                    lower.contains("sign in")
                        || lower.contains("log in")
                        || lower.contains("login")
                        || lower.contains("submit")
                        || lower.contains("continue")
                })
                .unwrap_or(false)
    });

    Some(UiPattern::LoginForm {
        email_field: email_field.map(|n| n.node_id.clone()),
        password_field: password_field.map(|n| n.node_id.clone()),
        submit_button: submit_button.map(|n| n.node_id.clone()),
    })
}

fn detect_dialog(nodes: &HashMap<String, SemanticNode>, root: &ScreenNode) -> Option<UiPattern> {
    // Heuristic: a container covering >60% of the screen with <=8 children,
    // containing buttons and text, is likely a dialog.
    let screen_area = root.bounds.width() as i64 * root.bounds.height() as i64;
    if screen_area <= 0 {
        return None;
    }

    // We look for direct children of root that might be dialog overlays
    for child in &root.children {
        let child_area = child.bounds.width() as i64 * child.bounds.height() as i64;
        let coverage = (child_area * 100) / screen_area;

        if coverage > 40 && coverage < 95 && child.children.len() <= 8 {
            // Collect text and buttons from this subtree
            let mut texts = Vec::new();
            let mut buttons = Vec::new();
            collect_dialog_elements(child, nodes, &mut texts, &mut buttons);

            if !buttons.is_empty() && !texts.is_empty() {
                let title = texts.first().cloned();
                let message = if texts.len() > 1 {
                    Some(texts[1..].join(" "))
                } else {
                    None
                };
                return Some(UiPattern::Dialog {
                    title,
                    message,
                    buttons,
                });
            }
        }
    }

    None
}

fn collect_dialog_elements(
    node: &ScreenNode,
    semantic_nodes: &HashMap<String, SemanticNode>,
    texts: &mut Vec<String>,
    buttons: &mut Vec<String>,
) {
    if let Some(sn) = semantic_nodes.get(&node.id) {
        match sn.kind {
            ElementKind::Label => {
                if let Some(ref text) = sn.display_text {
                    texts.push(text.clone());
                }
            }
            ElementKind::Button => {
                if let Some(ref text) = sn.display_text {
                    buttons.push(text.clone());
                }
            }
            _ => {}
        }
    }
    for child in &node.children {
        collect_dialog_elements(child, semantic_nodes, texts, buttons);
    }
}

fn detect_list_view(nodes: &HashMap<String, SemanticNode>, root: &ScreenNode) -> Option<UiPattern> {
    // Look for scrollable containers with many similar children
    let scrollables: Vec<&SemanticNode> = nodes
        .values()
        .filter(|n| n.kind == ElementKind::Scrollable)
        .collect();

    for scrollable in scrollables {
        // Find the raw node to count children
        if let Some(raw_node) = find_raw_node(root, &scrollable.node_id) {
            let visible_children = raw_node.children.iter().filter(|c| c.is_visible).count();
            if visible_children >= 3 {
                return Some(UiPattern::ListView {
                    container_id: scrollable.node_id.clone(),
                    item_count: visible_children as u32,
                });
            }
        }
    }

    None
}

fn detect_navigation(nodes: &HashMap<String, SemanticNode>) -> Option<UiPattern> {
    let nav_bars: Vec<&SemanticNode> = nodes
        .values()
        .filter(|n| {
            matches!(
                n.kind,
                ElementKind::NavigationBar | ElementKind::TabBar | ElementKind::Toolbar
            )
        })
        .collect();

    if nav_bars.is_empty() {
        return None;
    }

    let nav = &nav_bars[0];
    let nav_type = match nav.kind {
        ElementKind::NavigationBar => NavigationType::BottomNavigation,
        ElementKind::TabBar => NavigationType::TabBar,
        ElementKind::Toolbar => NavigationType::Toolbar,
        _ => NavigationType::BottomNavigation,
    };

    // Collect items text from the surrounding nodes (approximate)
    let items: Vec<String> = nav_bars
        .iter()
        .filter_map(|n| n.display_text.clone())
        .collect();

    Some(UiPattern::Navigation { nav_type, items })
}

fn detect_settings_page(
    nodes: &HashMap<String, SemanticNode>,
    joined_text: &str,
) -> Option<UiPattern> {
    let settings_keywords = ["settings", "preferences", "configuration"];
    let has_settings_keyword = settings_keywords.iter().any(|k| joined_text.contains(k));

    let toggles: Vec<&SemanticNode> = nodes
        .values()
        .filter(|n| matches!(n.kind, ElementKind::Toggle | ElementKind::Checkbox))
        .collect();

    if !has_settings_keyword || toggles.len() < 2 {
        return None;
    }

    // Pair toggles with the closest preceding label
    let mut pairs = Vec::new();
    for toggle in &toggles {
        // Find labels that are spatially near this toggle (within 50px vertically)
        let label = nodes.values().find(|n| {
            n.kind == ElementKind::Label
                && (n.center.1 - toggle.center.1).unsigned_abs() < 50
                && n.center.0 < toggle.center.0
        });
        if let Some(label) = label {
            pairs.push((
                label.display_text.clone().unwrap_or_default(),
                toggle.node_id.clone(),
            ));
        }
    }

    if pairs.is_empty() {
        // Fallback: just list the toggles
        let fallback: Vec<(String, String)> = toggles
            .iter()
            .map(|t| {
                (
                    t.display_text.clone().unwrap_or_default(),
                    t.node_id.clone(),
                )
            })
            .collect();
        return Some(UiPattern::SettingsPage {
            setting_pairs: fallback,
        });
    }

    Some(UiPattern::SettingsPage {
        setting_pairs: pairs,
    })
}

fn detect_search_bar(
    nodes: &HashMap<String, SemanticNode>,
    joined_text: &str,
) -> Option<UiPattern> {
    let search_field = nodes.values().find(|n| {
        n.kind == ElementKind::TextField
            && (n
                .display_text
                .as_ref()
                .map(|t| t.to_lowercase().contains("search"))
                .unwrap_or(false)
                || n.resource_id
                    .as_ref()
                    .map(|r| r.to_lowercase().contains("search"))
                    .unwrap_or(false)
                || joined_text.contains("search"))
    });

    search_field.map(|f| UiPattern::SearchBar {
        input_field: f.node_id.clone(),
    })
}

/// Find a raw ScreenNode by ID in the tree.
fn find_raw_node<'a>(node: &'a ScreenNode, id: &str) -> Option<&'a ScreenNode> {
    if node.id == id {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_raw_node(child, id) {
            return Some(found);
        }
    }
    None
}

// ── Phase 4: Landmark detection ─────────────────────────────────────────────

/// Detect navigational landmarks (key anchors) in the semantic graph.
fn detect_landmarks(nodes: &HashMap<String, SemanticNode>) -> Vec<Landmark> {
    let mut landmarks = Vec::new();

    for node in nodes.values() {
        // Primary action buttons
        if node.kind == ElementKind::Button && node.is_interactive {
            if let Some(ref text) = node.display_text {
                let lower = text.to_lowercase();
                let primary_keywords = [
                    "send", "submit", "ok", "confirm", "save", "done", "next", "continue",
                    "accept", "allow", "sign in", "log in", "buy", "pay", "order",
                ];
                if primary_keywords.iter().any(|k| lower.contains(k)) {
                    landmarks.push(Landmark {
                        node_id: node.node_id.clone(),
                        kind: LandmarkKind::PrimaryAction,
                        confidence: 0.85,
                    });
                }

                let dismiss_keywords = [
                    "back",
                    "close",
                    "cancel",
                    "dismiss",
                    "skip",
                    "no thanks",
                    "later",
                    "not now",
                    "deny",
                ];
                if dismiss_keywords.iter().any(|k| lower.contains(k)) {
                    landmarks.push(Landmark {
                        node_id: node.node_id.clone(),
                        kind: LandmarkKind::DismissAction,
                        confidence: 0.85,
                    });
                }
            }

            // Check content description for icon-only buttons
            if node.display_text.is_none() {
                if let Some(ref rid) = node.resource_id {
                    let lower = rid.to_lowercase();
                    if lower.contains("back") || lower.contains("nav_up") || lower.contains("close")
                    {
                        landmarks.push(Landmark {
                            node_id: node.node_id.clone(),
                            kind: LandmarkKind::DismissAction,
                            confidence: 0.75,
                        });
                    }
                }
            }
        }

        // Text input fields
        if node.kind == ElementKind::TextField {
            let kind = if node
                .display_text
                .as_ref()
                .map(|t| t.to_lowercase().contains("search"))
                .unwrap_or(false)
                || node
                    .resource_id
                    .as_ref()
                    .map(|r| r.to_lowercase().contains("search"))
                    .unwrap_or(false)
            {
                LandmarkKind::SearchInput
            } else {
                LandmarkKind::TextInput
            };
            landmarks.push(Landmark {
                node_id: node.node_id.clone(),
                kind,
                confidence: 0.9,
            });
        }

        // Scrollable regions
        if node.kind == ElementKind::Scrollable {
            landmarks.push(Landmark {
                node_id: node.node_id.clone(),
                kind: LandmarkKind::ScrollRegion,
                confidence: 0.95,
            });
        }
    }

    landmarks
}

// ── Phase 5: State inference ────────────────────────────────────────────────

/// Infer the semantic state of the screen.
fn infer_state(
    nodes: &HashMap<String, SemanticNode>,
    patterns: &[UiPattern],
    _tree: &ScreenTree,
) -> ScreenSemanticState {
    // Check for loading indicators
    let has_progress = nodes.values().any(|n| n.kind == ElementKind::ProgressBar);
    let has_loading_text = nodes.values().any(|n| {
        n.display_text
            .as_ref()
            .map(|t| {
                let lower = t.to_lowercase();
                lower.contains("loading") || lower.contains("please wait")
            })
            .unwrap_or(false)
    });
    if has_progress || has_loading_text {
        return ScreenSemanticState::Loading;
    }

    // Check for error state
    let error_text_count = nodes
        .values()
        .filter(|n| {
            n.display_text
                .as_ref()
                .map(|t| {
                    let lower = t.to_lowercase();
                    lower.contains("error")
                        || lower.contains("failed")
                        || lower.contains("couldn't")
                        || lower.contains("unable to")
                })
                .unwrap_or(false)
        })
        .count();
    if error_text_count >= 2 {
        return ScreenSemanticState::Error;
    }

    // Check for success state
    let success_text = nodes.values().any(|n| {
        n.display_text
            .as_ref()
            .map(|t| {
                let lower = t.to_lowercase();
                lower.contains("success")
                    || lower.contains("completed")
                    || lower.contains("sent")
                    || lower.contains("saved")
                    || lower.contains("confirmed")
            })
            .unwrap_or(false)
    });
    if success_text {
        return ScreenSemanticState::Success;
    }

    // Check for dialog/blocking overlay
    if patterns
        .iter()
        .any(|p| matches!(p, UiPattern::Dialog { .. }))
    {
        return ScreenSemanticState::Blocked;
    }

    // Check for input required (focused text field)
    let has_text_fields = nodes
        .values()
        .any(|n| n.kind == ElementKind::TextField && n.is_interactive);
    let interactive_count = nodes.values().filter(|n| n.is_interactive).count();
    if has_text_fields && interactive_count <= 5 {
        return ScreenSemanticState::InputRequired;
    }

    // Few nodes → transitional
    if nodes.len() <= 2 {
        return ScreenSemanticState::Transitional;
    }

    ScreenSemanticState::Interactive
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
                bottom: 300,
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

    fn make_tree_with(root: ScreenNode, package: &str) -> ScreenTree {
        fn count(n: &ScreenNode) -> u32 {
            1 + n.children.iter().map(|c| count(c)).sum::<u32>()
        }
        ScreenTree {
            node_count: count(&root),
            root,
            package_name: package.into(),
            activity_name: ".Main".into(),
            timestamp_ms: 1_700_000_000_000,
        }
    }

    // ── Classification tests ────────────────────────────────────────────────

    #[test]
    fn test_classify_button() {
        let node = make_node(
            "btn",
            "android.widget.Button",
            Some("OK"),
            true,
            false,
            vec![],
        );
        assert_eq!(classify_element(&node), ElementKind::Button);
    }

    #[test]
    fn test_classify_edit_text() {
        let node = make_node(
            "input",
            "android.widget.EditText",
            Some("Email"),
            true,
            true,
            vec![],
        );
        assert_eq!(classify_element(&node), ElementKind::TextField);
    }

    #[test]
    fn test_classify_clickable_textview_as_button() {
        let node = make_node(
            "link",
            "android.widget.TextView",
            Some("Click me"),
            true,
            false,
            vec![],
        );
        assert_eq!(classify_element(&node), ElementKind::Button);
    }

    #[test]
    fn test_classify_non_clickable_textview_as_label() {
        let node = make_node(
            "lbl",
            "android.widget.TextView",
            Some("Hello"),
            false,
            false,
            vec![],
        );
        assert_eq!(classify_element(&node), ElementKind::Label);
    }

    #[test]
    fn test_classify_switch() {
        let mut node = make_node("sw", "android.widget.Switch", None, true, false, vec![]);
        node.is_checkable = true;
        assert_eq!(classify_element(&node), ElementKind::Toggle);
    }

    #[test]
    fn test_classify_progress_bar() {
        let node = make_node(
            "pb",
            "android.widget.ProgressBar",
            None,
            false,
            false,
            vec![],
        );
        assert_eq!(classify_element(&node), ElementKind::ProgressBar);
    }

    #[test]
    fn test_classify_scrollview() {
        let mut node = make_node(
            "sv",
            "android.widget.ScrollView",
            None,
            false,
            false,
            vec![],
        );
        node.is_scrollable = true;
        assert_eq!(classify_element(&node), ElementKind::Scrollable);
    }

    #[test]
    fn test_classify_image() {
        let node = make_node(
            "img",
            "android.widget.ImageView",
            None,
            false,
            false,
            vec![],
        );
        assert_eq!(classify_element(&node), ElementKind::Image);
    }

    #[test]
    fn test_classify_checkbox() {
        let mut node = make_node(
            "cb",
            "android.widget.CheckBox",
            Some("Remember"),
            true,
            false,
            vec![],
        );
        node.is_checkable = true;
        assert_eq!(classify_element(&node), ElementKind::Checkbox);
    }

    #[test]
    fn test_classify_unknown_editable() {
        let node = make_node(
            "custom",
            "com.custom.SpecialInput",
            Some("Enter"),
            true,
            true,
            vec![],
        );
        assert_eq!(classify_element(&node), ElementKind::TextField);
    }

    // ── Graph building tests ────────────────────────────────────────────────

    #[test]
    fn test_build_semantic_graph_basic() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node(
                    "btn",
                    "android.widget.Button",
                    Some("OK"),
                    true,
                    false,
                    vec![],
                ),
                make_node(
                    "txt",
                    "android.widget.TextView",
                    Some("Hello"),
                    false,
                    false,
                    vec![],
                ),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        assert_eq!(graph.nodes.len(), 3); // root + btn + txt
        assert!(!graph.edges.is_empty()); // at least Contains edges
        assert_eq!(graph.package_name, "com.test");
    }

    #[test]
    fn test_contains_edges_created() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![make_node(
                "child",
                "android.widget.Button",
                Some("X"),
                true,
                false,
                vec![],
            )],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let contains = graph.edges_from("root", EdgeKind::Contains);
        assert_eq!(contains.len(), 1);
        assert_eq!(contains[0].to, "child");
    }

    #[test]
    fn test_follows_edges_created() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node("a", "android.widget.Button", Some("A"), true, false, vec![]),
                make_node("b", "android.widget.Button", Some("B"), true, false, vec![]),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let follows: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Follows)
            .collect();
        assert!(!follows.is_empty());
    }

    #[test]
    fn test_labels_edge_inferred() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node(
                    "lbl",
                    "android.widget.TextView",
                    Some("Email"),
                    false,
                    false,
                    vec![],
                ),
                make_node("inp", "android.widget.EditText", None, true, true, vec![]),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let labels: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Labels)
            .collect();
        assert!(!labels.is_empty());
        assert_eq!(labels[0].from, "lbl");
        assert_eq!(labels[0].to, "inp");
    }

    // ── Pattern detection tests ─────────────────────────────────────────────

    #[test]
    fn test_login_form_detected() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node(
                    "title",
                    "android.widget.TextView",
                    Some("Sign in"),
                    false,
                    false,
                    vec![],
                ),
                {
                    let mut n = make_node(
                        "email",
                        "android.widget.EditText",
                        Some("Email"),
                        true,
                        true,
                        vec![],
                    );
                    n.resource_id = Some("com.test:id/email_input".into());
                    n
                },
                make_node(
                    "pass",
                    "android.widget.EditText",
                    Some("Password"),
                    true,
                    true,
                    vec![],
                ),
                make_node(
                    "btn",
                    "android.widget.Button",
                    Some("Log in"),
                    true,
                    false,
                    vec![],
                ),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let login_patterns: Vec<_> = graph
            .patterns
            .iter()
            .filter(|p| matches!(p, UiPattern::LoginForm { .. }))
            .collect();
        assert!(!login_patterns.is_empty(), "should detect login form");
    }

    #[test]
    fn test_no_login_on_normal_screen() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node(
                    "btn",
                    "android.widget.Button",
                    Some("Send"),
                    true,
                    false,
                    vec![],
                ),
                make_node(
                    "txt",
                    "android.widget.TextView",
                    Some("Hello world"),
                    false,
                    false,
                    vec![],
                ),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let login_patterns: Vec<_> = graph
            .patterns
            .iter()
            .filter(|p| matches!(p, UiPattern::LoginForm { .. }))
            .collect();
        assert!(login_patterns.is_empty(), "should NOT detect login form");
    }

    #[test]
    fn test_navigation_detected() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![make_node(
                "nav",
                "com.google.android.material.bottomnavigation.BottomNavigationView",
                Some("Home"),
                true,
                false,
                vec![],
            )],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let nav_patterns: Vec<_> = graph
            .patterns
            .iter()
            .filter(|p| matches!(p, UiPattern::Navigation { .. }))
            .collect();
        assert!(!nav_patterns.is_empty());
    }

    // ── Landmark detection tests ────────────────────────────────────────────

    #[test]
    fn test_primary_action_landmark() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![make_node(
                "send",
                "android.widget.Button",
                Some("Send"),
                true,
                false,
                vec![],
            )],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let primary: Vec<_> = graph
            .landmarks
            .iter()
            .filter(|l| l.kind == LandmarkKind::PrimaryAction)
            .collect();
        assert!(!primary.is_empty());
        assert_eq!(primary[0].node_id, "send");
    }

    #[test]
    fn test_dismiss_action_landmark() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![make_node(
                "cancel",
                "android.widget.Button",
                Some("Cancel"),
                true,
                false,
                vec![],
            )],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let dismiss: Vec<_> = graph
            .landmarks
            .iter()
            .filter(|l| l.kind == LandmarkKind::DismissAction)
            .collect();
        assert!(!dismiss.is_empty());
    }

    #[test]
    fn test_text_input_landmark() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![make_node(
                "inp",
                "android.widget.EditText",
                Some("Type here"),
                true,
                true,
                vec![],
            )],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let inputs: Vec<_> = graph
            .landmarks
            .iter()
            .filter(|l| l.kind == LandmarkKind::TextInput)
            .collect();
        assert!(!inputs.is_empty());
    }

    // ── State inference tests ───────────────────────────────────────────────

    #[test]
    fn test_state_loading() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node(
                    "pb",
                    "android.widget.ProgressBar",
                    None,
                    false,
                    false,
                    vec![],
                ),
                make_node(
                    "txt",
                    "android.widget.TextView",
                    Some("Loading..."),
                    false,
                    false,
                    vec![],
                ),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);
        assert_eq!(graph.state, ScreenSemanticState::Loading);
    }

    #[test]
    fn test_state_error() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node(
                    "t1",
                    "android.widget.TextView",
                    Some("Error occurred"),
                    false,
                    false,
                    vec![],
                ),
                make_node(
                    "t2",
                    "android.widget.TextView",
                    Some("Failed to connect"),
                    false,
                    false,
                    vec![],
                ),
                make_node(
                    "btn",
                    "android.widget.Button",
                    Some("Retry"),
                    true,
                    false,
                    vec![],
                ),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);
        assert_eq!(graph.state, ScreenSemanticState::Error);
    }

    #[test]
    fn test_state_success() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node(
                    "msg",
                    "android.widget.TextView",
                    Some("Message sent successfully"),
                    false,
                    false,
                    vec![],
                ),
                make_node(
                    "btn",
                    "android.widget.Button",
                    Some("OK"),
                    true,
                    false,
                    vec![],
                ),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);
        assert_eq!(graph.state, ScreenSemanticState::Success);
    }

    #[test]
    fn test_state_interactive() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node("a", "android.widget.Button", Some("A"), true, false, vec![]),
                make_node("b", "android.widget.Button", Some("B"), true, false, vec![]),
                make_node("c", "android.widget.Button", Some("C"), true, false, vec![]),
                make_node("d", "android.widget.Button", Some("D"), true, false, vec![]),
                make_node("e", "android.widget.Button", Some("E"), true, false, vec![]),
                make_node("f", "android.widget.Button", Some("F"), true, false, vec![]),
                make_node(
                    "g",
                    "android.widget.TextView",
                    Some("Title"),
                    false,
                    false,
                    vec![],
                ),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);
        assert_eq!(graph.state, ScreenSemanticState::Interactive);
    }

    // ── Query methods tests ─────────────────────────────────────────────────

    #[test]
    fn test_nodes_of_kind() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node("a", "android.widget.Button", Some("A"), true, false, vec![]),
                make_node("b", "android.widget.Button", Some("B"), true, false, vec![]),
                make_node(
                    "c",
                    "android.widget.TextView",
                    Some("C"),
                    false,
                    false,
                    vec![],
                ),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        assert_eq!(graph.nodes_of_kind(ElementKind::Button).len(), 2);
        assert_eq!(graph.nodes_of_kind(ElementKind::Label).len(), 1);
    }

    #[test]
    fn test_interactive_nodes() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node(
                    "btn",
                    "android.widget.Button",
                    Some("OK"),
                    true,
                    false,
                    vec![],
                ),
                make_node(
                    "txt",
                    "android.widget.TextView",
                    Some("Hi"),
                    false,
                    false,
                    vec![],
                ),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let interactive = graph.interactive_nodes();
        assert_eq!(interactive.len(), 1);
    }

    #[test]
    fn test_label_for() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node(
                    "lbl",
                    "android.widget.TextView",
                    Some("Username"),
                    false,
                    false,
                    vec![],
                ),
                make_node("inp", "android.widget.EditText", None, true, true, vec![]),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let label = graph.label_for("inp");
        assert_eq!(label, Some("Username"));
    }

    #[test]
    fn test_summary_for_llm() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node(
                    "btn",
                    "android.widget.Button",
                    Some("Send"),
                    true,
                    false,
                    vec![],
                ),
                make_node(
                    "inp",
                    "android.widget.EditText",
                    Some("Message"),
                    true,
                    true,
                    vec![],
                ),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let summary = graph.summary_for_llm();
        assert!(summary.contains("com.test"));
        assert!(!summary.is_empty());
    }

    #[test]
    fn test_estimated_size_bytes() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![make_node(
                "btn",
                "android.widget.Button",
                Some("OK"),
                true,
                false,
                vec![],
            )],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let size = graph.estimated_size_bytes();
        assert!(size > 0);
        assert!(size < 100_000); // should be small for a simple tree
    }

    #[test]
    fn test_invisible_nodes_excluded() {
        let mut invisible = make_node(
            "hidden",
            "android.widget.Button",
            Some("Secret"),
            true,
            false,
            vec![],
        );
        invisible.is_visible = false;

        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                invisible,
                make_node(
                    "vis",
                    "android.widget.Button",
                    Some("Visible"),
                    true,
                    false,
                    vec![],
                ),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        assert!(!graph.nodes.contains_key("hidden"));
        assert!(graph.nodes.contains_key("vis"));
    }

    #[test]
    fn test_groups_with_edges() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![
                make_node(
                    "b1",
                    "android.widget.Button",
                    Some("A"),
                    true,
                    false,
                    vec![],
                ),
                make_node(
                    "b2",
                    "android.widget.Button",
                    Some("B"),
                    true,
                    false,
                    vec![],
                ),
                make_node(
                    "b3",
                    "android.widget.Button",
                    Some("C"),
                    true,
                    false,
                    vec![],
                ),
            ],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let groups: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::GroupsWith)
            .collect();
        // 3 buttons: 3 pairs (b1-b2, b1-b3, b2-b3)
        assert_eq!(groups.len(), 3);
    }

    #[test]
    fn test_empty_tree_graph() {
        let root = make_node("root", "FrameLayout", None, false, false, vec![]);
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        assert_eq!(graph.nodes.len(), 1); // just root
        assert!(graph.edges.is_empty());
        assert!(graph.patterns.is_empty());
    }

    #[test]
    fn test_search_bar_detected() {
        let root = make_node(
            "root",
            "FrameLayout",
            None,
            false,
            false,
            vec![{
                let mut n = make_node(
                    "search",
                    "android.widget.EditText",
                    Some("Search..."),
                    true,
                    true,
                    vec![],
                );
                n.resource_id = Some("com.test:id/search_input".into());
                n
            }],
        );
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let search: Vec<_> = graph
            .patterns
            .iter()
            .filter(|p| matches!(p, UiPattern::SearchBar { .. }))
            .collect();
        assert!(!search.is_empty());

        // Also check search input landmark
        let search_landmarks: Vec<_> = graph
            .landmarks
            .iter()
            .filter(|l| l.kind == LandmarkKind::SearchInput)
            .collect();
        assert!(!search_landmarks.is_empty());
    }

    #[test]
    fn test_list_view_detected() {
        let mut scroll = make_node(
            "list",
            "android.widget.ScrollView",
            None,
            false,
            false,
            vec![
                make_node(
                    "item1",
                    "android.widget.TextView",
                    Some("Item 1"),
                    false,
                    false,
                    vec![],
                ),
                make_node(
                    "item2",
                    "android.widget.TextView",
                    Some("Item 2"),
                    false,
                    false,
                    vec![],
                ),
                make_node(
                    "item3",
                    "android.widget.TextView",
                    Some("Item 3"),
                    false,
                    false,
                    vec![],
                ),
            ],
        );
        scroll.is_scrollable = true;

        let root = make_node("root", "FrameLayout", None, false, false, vec![scroll]);
        let tree = make_tree_with(root, "com.test");
        let graph = SemanticGraph::from_tree(&tree);

        let lists: Vec<_> = graph
            .patterns
            .iter()
            .filter(|p| matches!(p, UiPattern::ListView { .. }))
            .collect();
        assert!(!lists.is_empty());
    }
}
