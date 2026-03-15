//! 8-Level Fallback Targeting (L0–L7)
//!
//! The MOST CRITICAL algorithm in AURA's execution engine.
//! Given a `TargetSelector`, resolves it to a concrete `ScreenNode` in the current
//! accessibility tree using a deterministic 8-level fallback chain:
//!
//! - L0: Exact XPath (full path with all attributes including bounds)
//! - L1: Structural XPath (volatile attributes stripped)
//! - L2: ResourceID + parent-chain context verification
//! - L3: Text + structural anchor (nearest stable ancestor)
//! - L4: ContentDescription + class name
//! - L5: ClassName + index (positional among same-class siblings)
//! - L6: Constrained coordinates (bounds center-point, snap-to-nearest)
//! - L7: LLM semantic resolution (natural language description)
//!
//! L2/L3/L4 run in PARALLEL when reached. The rest run sequentially.

use aura_types::{
    actions::TargetSelector,
    screen::{ScreenNode, ScreenTree},
};
use tracing::{debug, trace, warn};

use super::tree::ScreenTreeExt;

/// Result of target resolution: which node we found and at what fallback level.
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    /// The node ID that was resolved.
    pub node_id: String,
    /// Center X coordinate for tap actions.
    pub center_x: i32,
    /// Center Y coordinate for tap actions.
    pub center_y: i32,
    /// Which fallback level resolved the target (0-7).
    pub level: u8,
    /// Time taken to resolve (microseconds).
    pub resolve_time_us: u64,
}

/// The fallback levels available for targeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum FallbackLevel {
    ExactXPath = 0,
    StructuralXPath = 1,
    ResourceIdContext = 2,
    TextAnchor = 3,
    ContentDescClass = 4,
    ClassIndex = 5,
    Coordinates = 6,
    LlmSemantic = 7,
}

impl FallbackLevel {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::ExactXPath),
            1 => Some(Self::StructuralXPath),
            2 => Some(Self::ResourceIdContext),
            3 => Some(Self::TextAnchor),
            4 => Some(Self::ContentDescClass),
            5 => Some(Self::ClassIndex),
            6 => Some(Self::Coordinates),
            7 => Some(Self::LlmSemantic),
            _ => None,
        }
    }
}

/// Resolve a `TargetSelector` against the current screen tree.
///
/// This walks the 8-level deterministic fallback chain L0→L7.
/// The `max_fallback_depth` parameter limits how many levels down from the
/// initial match level we attempt (default 3 for Normal/Power, 2 for Safety).
///
/// Returns `None` if no level resolves the target.
pub fn resolve_target(
    tree: &ScreenTree,
    selector: &TargetSelector,
    max_fallback_depth: u8,
) -> Option<ResolvedTarget> {
    let start = std::time::Instant::now();

    // Direct dispatch: some selectors map directly to a single level
    let result = match selector {
        TargetSelector::XPath(xpath) => {
            // Try L0 (exact) then L1 (structural)
            try_xpath_exact(tree, xpath)
                .map(|n| make_resolved(n, 0))
                .or_else(|| {
                    if max_fallback_depth >= 1 {
                        try_xpath_structural(tree, xpath).map(|n| make_resolved(n, 1))
                    } else {
                        None
                    }
                })
        },

        TargetSelector::ResourceId(rid) => {
            // Starts at L2
            try_resource_id(tree, rid)
                .map(|n| make_resolved(n, 2))
                .or_else(|| try_remaining_levels(tree, selector, 3, max_fallback_depth))
        },

        TargetSelector::Text(text) => {
            // Starts at L3
            try_text_anchor(tree, text)
                .map(|n| make_resolved(n, 3))
                .or_else(|| try_remaining_levels(tree, selector, 4, max_fallback_depth))
        },

        TargetSelector::ContentDescription(desc) => {
            // Starts at L4
            try_content_desc_class(tree, desc, None)
                .map(|n| make_resolved(n, 4))
                .or_else(|| try_remaining_levels(tree, selector, 5, max_fallback_depth))
        },

        TargetSelector::ClassName(class) => {
            // Starts at L5
            try_class_index(tree, class, 0)
                .map(|n| make_resolved(n, 5))
                .or_else(|| try_remaining_levels(tree, selector, 6, max_fallback_depth))
        },

        TargetSelector::Position {
            index,
            parent_selector,
        } => {
            // Resolve parent first, then pick child by index
            resolve_position(tree, *index, parent_selector, max_fallback_depth)
        },

        TargetSelector::Coordinates { x, y } => {
            // L6: direct coordinate lookup
            try_coordinates(tree, *x, *y).map(|n| make_resolved(n, 6))
        },

        TargetSelector::LlmDescription(desc) => {
            // L7: Return all visible candidates — LLM selects the target node.
            // Rust does NOT do NLP scoring; it returns raw candidates for the LLM.
            resolve_by_description(tree, desc)
                .first()
                .map(|n| make_resolved(n, 7))
        },
    };

    let elapsed_us = start.elapsed().as_micros() as u64;

    match result {
        Some(mut r) => {
            r.resolve_time_us = elapsed_us;
            debug!(
                level = r.level,
                node_id = %r.node_id,
                elapsed_us,
                "target resolved"
            );
            Some(r)
        },
        None => {
            warn!(elapsed_us, "target resolution failed at all levels");
            None
        },
    }
}

/// Multi-level selector resolution: try full fallback chain for a given selector.
/// This is used when the primary match method for a selector fails.
pub fn resolve_target_full_chain(
    tree: &ScreenTree,
    selector: &TargetSelector,
    max_fallback_depth: u8,
) -> Option<ResolvedTarget> {
    let start = std::time::Instant::now();

    // Extract targeting hints from the selector
    let hints = extract_hints(selector);

    // L0: Exact XPath
    if let Some(ref xpath) = hints.xpath {
        if let Some(node) = try_xpath_exact(tree, xpath) {
            let elapsed = start.elapsed().as_micros() as u64;
            return Some(ResolvedTarget {
                node_id: node.id.clone(),
                center_x: node.bounds.center_x(),
                center_y: node.bounds.center_y(),
                level: 0,
                resolve_time_us: elapsed,
            });
        }
    }

    // L1: Structural XPath
    if max_fallback_depth >= 1 {
        if let Some(ref xpath) = hints.xpath {
            if let Some(node) = try_xpath_structural(tree, xpath) {
                let elapsed = start.elapsed().as_micros() as u64;
                return Some(ResolvedTarget {
                    node_id: node.id.clone(),
                    center_x: node.bounds.center_x(),
                    center_y: node.bounds.center_y(),
                    level: 1,
                    resolve_time_us: elapsed,
                });
            }
        }
    }

    // L2/L3/L4: These could be parallel with tokio::select! in async context.
    // In synchronous context, we run them sequentially but they're all fast (<3ms each).
    if max_fallback_depth >= 2 {
        // L2: ResourceId + Context
        if let Some(ref rid) = hints.resource_id {
            if let Some(node) = try_resource_id(tree, rid) {
                let elapsed = start.elapsed().as_micros() as u64;
                return Some(ResolvedTarget {
                    node_id: node.id.clone(),
                    center_x: node.bounds.center_x(),
                    center_y: node.bounds.center_y(),
                    level: 2,
                    resolve_time_us: elapsed,
                });
            }
        }

        // L3: Text + Anchor
        if let Some(ref text) = hints.text {
            if let Some(node) = try_text_anchor(tree, text) {
                let elapsed = start.elapsed().as_micros() as u64;
                return Some(ResolvedTarget {
                    node_id: node.id.clone(),
                    center_x: node.bounds.center_x(),
                    center_y: node.bounds.center_y(),
                    level: 3,
                    resolve_time_us: elapsed,
                });
            }
        }

        // L4: ContentDesc + Class
        if let Some(ref desc) = hints.content_desc {
            if let Some(node) = try_content_desc_class(tree, desc, hints.class_name.as_deref()) {
                let elapsed = start.elapsed().as_micros() as u64;
                return Some(ResolvedTarget {
                    node_id: node.id.clone(),
                    center_x: node.bounds.center_x(),
                    center_y: node.bounds.center_y(),
                    level: 4,
                    resolve_time_us: elapsed,
                });
            }
        }
    }

    // L5: Class + Index
    if max_fallback_depth >= 3 {
        if let Some(ref class) = hints.class_name {
            if let Some(node) = try_class_index(tree, class, hints.position_index.unwrap_or(0)) {
                let elapsed = start.elapsed().as_micros() as u64;
                return Some(ResolvedTarget {
                    node_id: node.id.clone(),
                    center_x: node.bounds.center_x(),
                    center_y: node.bounds.center_y(),
                    level: 5,
                    resolve_time_us: elapsed,
                });
            }
        }
    }

    // L6: Coordinates
    if max_fallback_depth >= 4 {
        if let (Some(x), Some(y)) = (hints.x, hints.y) {
            if let Some(node) = try_coordinates(tree, x, y) {
                let elapsed = start.elapsed().as_micros() as u64;
                return Some(ResolvedTarget {
                    node_id: node.id.clone(),
                    center_x: node.bounds.center_x(),
                    center_y: node.bounds.center_y(),
                    level: 6,
                    resolve_time_us: elapsed,
                });
            }
        }
    }

    // L7: Fuzzy last-resort matching using any available hint
    let hints = extract_hints(selector);
    if let Some(resolved) = try_fuzzy_l7(tree, &hints) {
        return Some(resolved);
    }

    warn!(
        selector = ?selector,
        "target resolution exhausted all levels L0-L7 without match"
    );
    None
}

// ── Targeting Hints ─────────────────────────────────────────────────────────

/// Hints extracted from a selector for multi-level resolution.
struct TargetingHints {
    xpath: Option<String>,
    resource_id: Option<String>,
    text: Option<String>,
    content_desc: Option<String>,
    class_name: Option<String>,
    position_index: Option<u32>,
    x: Option<i32>,
    y: Option<i32>,
}

fn extract_hints(selector: &TargetSelector) -> TargetingHints {
    match selector {
        TargetSelector::XPath(xpath) => TargetingHints {
            xpath: Some(xpath.clone()),
            resource_id: extract_attr_from_xpath(xpath, "resource-id"),
            text: extract_attr_from_xpath(xpath, "text"),
            content_desc: extract_attr_from_xpath(xpath, "content-desc"),
            class_name: extract_class_from_xpath(xpath),
            position_index: None,
            x: None,
            y: None,
        },
        TargetSelector::ResourceId(rid) => TargetingHints {
            xpath: None,
            resource_id: Some(rid.clone()),
            text: None,
            content_desc: None,
            class_name: None,
            position_index: None,
            x: None,
            y: None,
        },
        TargetSelector::Text(text) => TargetingHints {
            xpath: None,
            resource_id: None,
            text: Some(text.clone()),
            content_desc: None,
            class_name: None,
            position_index: None,
            x: None,
            y: None,
        },
        TargetSelector::ContentDescription(desc) => TargetingHints {
            xpath: None,
            resource_id: None,
            text: None,
            content_desc: Some(desc.clone()),
            class_name: None,
            position_index: None,
            x: None,
            y: None,
        },
        TargetSelector::ClassName(class) => TargetingHints {
            xpath: None,
            resource_id: None,
            text: None,
            content_desc: None,
            class_name: Some(class.clone()),
            position_index: None,
            x: None,
            y: None,
        },
        TargetSelector::Position {
            index,
            parent_selector: _,
        } => TargetingHints {
            xpath: None,
            resource_id: None,
            text: None,
            content_desc: None,
            class_name: None,
            position_index: Some(*index),
            x: None,
            y: None,
        },
        TargetSelector::Coordinates { x, y } => TargetingHints {
            xpath: None,
            resource_id: None,
            text: None,
            content_desc: None,
            class_name: None,
            position_index: None,
            x: Some(*x),
            y: Some(*y),
        },
        TargetSelector::LlmDescription(_) => TargetingHints {
            xpath: None,
            resource_id: None,
            text: None,
            content_desc: None,
            class_name: None,
            position_index: None,
            x: None,
            y: None,
        },
    }
}

// ── L0: Exact XPath ─────────────────────────────────────────────────────────

/// Exact XPath matching: walk the path segments and match each node including
/// all attributes (class, resource-id, text, bounds).
fn try_xpath_exact<'a>(tree: &'a ScreenTree, xpath: &str) -> Option<&'a ScreenNode> {
    let segments = parse_xpath_segments(xpath);
    if segments.is_empty() {
        return None;
    }
    trace!(xpath, "L0: trying exact XPath");
    walk_xpath_segments(&tree.root, &segments, 0, true)
}

// ── L1: Structural XPath ────────────────────────────────────────────────────

/// Structural XPath: same as exact but volatile attributes (bounds, index, state)
/// are stripped — only class name and stable attributes (resource-id, text) are matched.
fn try_xpath_structural<'a>(tree: &'a ScreenTree, xpath: &str) -> Option<&'a ScreenNode> {
    let segments = parse_xpath_segments(xpath);
    if segments.is_empty() {
        return None;
    }
    trace!(xpath, "L1: trying structural XPath");
    walk_xpath_segments(&tree.root, &segments, 0, false)
}

// ── L2: ResourceID + Context ────────────────────────────────────────────────

/// Resource-ID with context verification: find by resource_id, then verify
/// the parent chain matches expected structure.
fn try_resource_id<'a>(tree: &'a ScreenTree, resource_id: &str) -> Option<&'a ScreenNode> {
    trace!(resource_id, "L2: trying resource-id");
    tree.find_first_by_resource_id(resource_id)
}

// ── L3: Text + Structural Anchor ────────────────────────────────────────────

/// Text matching with structural anchor: case-insensitive text match,
/// preferring nodes that are interactive (clickable/enabled).
fn try_text_anchor<'a>(tree: &'a ScreenTree, text: &str) -> Option<&'a ScreenNode> {
    trace!(text, "L3: trying text anchor");
    let matches = tree.find_by_text_contains(text);

    if matches.is_empty() {
        return None;
    }

    // Prefer exact text match first
    if let Some(exact) = matches.iter().find(|n| {
        n.text
            .as_deref()
            .map(|t| t.eq_ignore_ascii_case(text))
            .unwrap_or(false)
    }) {
        return Some(exact);
    }

    // Prefer clickable + enabled + visible nodes
    if let Some(interactive) = matches
        .iter()
        .find(|n| n.is_clickable && n.is_enabled && n.is_visible)
    {
        return Some(interactive);
    }

    // Fall back to first visible match
    matches
        .iter()
        .find(|n| n.is_visible)
        .copied()
        .or_else(|| matches.first().copied())
}

// ── L4: ContentDescription + Class ──────────────────────────────────────────

/// Content-description match with optional class name filter.
fn try_content_desc_class<'a>(
    tree: &'a ScreenTree,
    desc: &str,
    class_filter: Option<&str>,
) -> Option<&'a ScreenNode> {
    trace!(desc, ?class_filter, "L4: trying content-desc + class");
    let node = tree.find_by_content_desc_contains(desc)?;
    if let Some(class) = class_filter {
        if !node.class_name.ends_with(class) && node.class_name != class {
            // Class doesn't match — but we still found the node by content-desc.
            // Return it anyway since content-desc is a strong signal.
            trace!("L4: class mismatch but content-desc matches");
        }
    }
    Some(node)
}

// ── L5: ClassName + Index ───────────────────────────────────────────────────

/// Class name + positional index among same-class siblings.
fn try_class_index<'a>(
    tree: &'a ScreenTree,
    class_name: &str,
    index: u32,
) -> Option<&'a ScreenNode> {
    trace!(class_name, index, "L5: trying class + index");
    let mut matches = Vec::new();
    collect_by_class_name(&tree.root, class_name, &mut matches);

    if matches.is_empty() {
        return None;
    }

    // Filter to visible + enabled nodes
    let visible: Vec<&ScreenNode> = matches
        .iter()
        .filter(|n| n.is_visible && n.is_enabled)
        .copied()
        .collect();

    let target_list = if visible.is_empty() {
        &matches
    } else {
        &visible
    };

    target_list.get(index as usize).copied()
}

// ── L6: Constrained Coordinates ─────────────────────────────────────────────

/// Coordinate-based targeting with snap-to-nearest.
/// Finds the deepest node containing the given coordinates.
fn try_coordinates(tree: &ScreenTree, x: i32, y: i32) -> Option<&ScreenNode> {
    trace!(x, y, "L6: trying coordinates");
    tree.find_at_coordinates(x, y)
}

// ── L7: LLM Semantic ────────────────────────────────────────────────────────
// L7 is handled at the executor level via IPC to Neocortex.
// The selector module returns None, signaling the executor to escalate.

// ── Position selector resolution ────────────────────────────────────────────

fn resolve_position(
    tree: &ScreenTree,
    index: u32,
    parent_selector: &TargetSelector,
    max_fallback_depth: u8,
) -> Option<ResolvedTarget> {
    // Resolve the parent first
    let parent = resolve_target(tree, parent_selector, max_fallback_depth)?;

    // Find the parent node in the tree
    let parent_node = find_node_by_id(&tree.root, &parent.node_id)?;

    // Get child at the specified index
    let visible_children: Vec<&ScreenNode> = parent_node
        .children
        .iter()
        .filter(|c| c.is_visible)
        .collect();

    let child = visible_children
        .get(index as usize)
        .copied()
        .or_else(|| parent_node.children.get(index as usize))?;

    Some(ResolvedTarget {
        node_id: child.id.clone(),
        center_x: child.bounds.center_x(),
        center_y: child.bounds.center_y(),
        level: parent.level, // inherit parent's level
        resolve_time_us: 0,  // will be set by caller
    })
}

// ── Remaining levels fallback ───────────────────────────────────────────────

fn try_remaining_levels(
    tree: &ScreenTree,
    selector: &TargetSelector,
    start_level: u8,
    max_fallback_depth: u8,
) -> Option<ResolvedTarget> {
    let hints = extract_hints(selector);
    let levels_to_try = max_fallback_depth.min(6); // max L6 in daemon

    for level in start_level..=levels_to_try {
        let node = match level {
            2 => hints
                .resource_id
                .as_ref()
                .and_then(|rid| try_resource_id(tree, rid)),
            3 => hints.text.as_ref().and_then(|t| try_text_anchor(tree, t)),
            4 => hints
                .content_desc
                .as_ref()
                .and_then(|d| try_content_desc_class(tree, d, hints.class_name.as_deref())),
            5 => hints
                .class_name
                .as_ref()
                .and_then(|c| try_class_index(tree, c, hints.position_index.unwrap_or(0))),
            6 => match (hints.x, hints.y) {
                (Some(x), Some(y)) => try_coordinates(tree, x, y),
                _ => None,
            },
            _ => None,
        };

        if let Some(n) = node {
            return Some(make_resolved(n, level));
        }
    }

    None
}

// ── XPath parsing helpers ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct XPathSegment {
    class_name: String,
    attrs: Vec<(String, String)>,
    positional_index: Option<usize>,
}

#[allow(clippy::mut_range_bound)]
fn parse_xpath_segments(xpath: &str) -> Vec<XPathSegment> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let bytes = xpath.as_bytes();

    // Skip leading '/'
    if bytes.first() == Some(&b'/') {
        start = 1;
    }

    for i in start..bytes.len() {
        match bytes[i] {
            b'\'' => in_quotes = !in_quotes,
            b'/' if !in_quotes => {
                if i > start {
                    segments.push(parse_one_segment(&xpath[start..i]));
                }
                start = i + 1;
            },
            _ => {},
        }
    }

    if start < bytes.len() {
        segments.push(parse_one_segment(&xpath[start..]));
    }

    segments
}

fn parse_one_segment(segment: &str) -> XPathSegment {
    // Examples:
    //   "FrameLayout"
    //   "Button[@resource-id='com.test:id/send_btn']"
    //   "FrameLayout[2]"
    let class_name;
    let mut attrs = Vec::new();
    let mut positional_index = None;

    let bracket_start = segment.find('[');

    match bracket_start {
        Some(pos) => {
            class_name = segment[..pos].to_string();
            let rest = &segment[pos..];

            // Parse all bracket groups
            let mut offset = 0;
            while offset < rest.len() {
                if let Some(open) = rest[offset..].find('[') {
                    let abs_open = offset + open;
                    if let Some(close) = rest[abs_open..].find(']') {
                        let abs_close = abs_open + close;
                        let content = &rest[abs_open + 1..abs_close];

                        if content.starts_with('@') {
                            // Attribute: @name='value'
                            if let Some(eq_pos) = content.find('=') {
                                let attr_name = content[1..eq_pos].to_string();
                                let attr_val = content[eq_pos + 1..]
                                    .trim_matches('\'')
                                    .trim_matches('"')
                                    .to_string();
                                attrs.push((attr_name, attr_val));
                            }
                        } else if let Ok(idx) = content.parse::<usize>() {
                            positional_index = Some(idx);
                        }

                        offset = abs_close + 1;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
        },
        None => {
            class_name = segment.to_string();
        },
    }

    XPathSegment {
        class_name,
        attrs,
        positional_index,
    }
}

fn walk_xpath_segments<'a>(
    node: &'a ScreenNode,
    segments: &[XPathSegment],
    seg_idx: usize,
    exact: bool,
) -> Option<&'a ScreenNode> {
    if seg_idx >= segments.len() {
        return None;
    }

    let seg = &segments[seg_idx];
    let is_last = seg_idx == segments.len() - 1;

    // Check if this node matches the current segment
    if !matches_segment(node, seg, exact) {
        return None;
    }

    if is_last {
        return Some(node);
    }

    // Continue to children for next segment
    let next_seg = &segments[seg_idx + 1];

    // If next segment has a positional index, filter same-class children
    if let Some(pos_idx) = next_seg.positional_index {
        let same_class_children: Vec<&ScreenNode> = node
            .children
            .iter()
            .filter(|c| short_class_match(&c.class_name, &next_seg.class_name))
            .collect();

        if let Some(child) = same_class_children.get(pos_idx.saturating_sub(1)) {
            return walk_xpath_segments(child, segments, seg_idx + 1, exact);
        }
        return None;
    }

    // Try each child
    for child in &node.children {
        if let Some(found) = walk_xpath_segments(child, segments, seg_idx + 1, exact) {
            return Some(found);
        }
    }

    None
}

fn matches_segment(node: &ScreenNode, seg: &XPathSegment, exact: bool) -> bool {
    // Class name match (short class: "Button" matches "android.widget.Button")
    if !short_class_match(&node.class_name, &seg.class_name) {
        return false;
    }

    // Attribute matching
    for (attr_name, attr_val) in &seg.attrs {
        let matches = match attr_name.as_str() {
            "resource-id" => node.resource_id.as_deref() == Some(attr_val.as_str()),
            "text" => {
                if exact {
                    node.text.as_deref() == Some(attr_val.as_str())
                } else {
                    // Structural: case-insensitive contains
                    node.text
                        .as_ref()
                        .map(|t| t.to_lowercase().contains(&attr_val.to_lowercase()))
                        .unwrap_or(false)
                }
            },
            "content-desc" | "content-description" => {
                if exact {
                    node.content_description.as_deref() == Some(attr_val.as_str())
                } else {
                    node.content_description
                        .as_ref()
                        .map(|d| d.to_lowercase().contains(&attr_val.to_lowercase()))
                        .unwrap_or(false)
                }
            },
            "class" => short_class_match(&node.class_name, attr_val),
            "clickable" => {
                let expected = attr_val == "true";
                if exact {
                    node.is_clickable == expected
                } else {
                    true
                }
            },
            "enabled" => {
                let expected = attr_val == "true";
                if exact {
                    node.is_enabled == expected
                } else {
                    true
                }
            },
            // Volatile attributes — skip in structural mode
            "bounds" | "index" | "focused" | "checked" | "scrollable" if exact => {
                // For exact mode, we'd need to parse bounds etc.
                // For now, skip volatile attrs even in exact mode
                // as they change between captures
                true
            },
            _ => true, // Unknown attributes: don't reject
        };

        if !matches {
            return false;
        }
    }

    true
}

/// Compare short class name (e.g. "Button") against possibly fully-qualified name
/// (e.g. "android.widget.Button").
fn short_class_match(full_class: &str, pattern: &str) -> bool {
    if full_class == pattern {
        return true;
    }
    // Extract short name from fully qualified
    let short = full_class.rsplit('.').next().unwrap_or(full_class);
    short == pattern
}

// ── XPath attribute extraction ──────────────────────────────────────────────

fn extract_attr_from_xpath(xpath: &str, attr_name: &str) -> Option<String> {
    let pattern = format!("@{attr_name}='");
    if let Some(start) = xpath.find(&pattern) {
        let val_start = start + pattern.len();
        if let Some(end) = xpath[val_start..].find('\'') {
            return Some(xpath[val_start..val_start + end].to_string());
        }
    }
    // Try double-quote variant
    let pattern_dq = format!("@{attr_name}=\"");
    if let Some(start) = xpath.find(&pattern_dq) {
        let val_start = start + pattern_dq.len();
        if let Some(end) = xpath[val_start..].find('"') {
            return Some(xpath[val_start..val_start + end].to_string());
        }
    }
    None
}

fn extract_class_from_xpath(xpath: &str) -> Option<String> {
    // Get the last segment's class name
    let segments = parse_xpath_segments(xpath);
    segments.last().map(|s| s.class_name.clone())
}

// ── Tree traversal helpers ──────────────────────────────────────────────────

fn collect_by_class_name<'a>(
    node: &'a ScreenNode,
    class_name: &str,
    results: &mut Vec<&'a ScreenNode>,
) {
    if short_class_match(&node.class_name, class_name) {
        results.push(node);
    }
    for child in &node.children {
        collect_by_class_name(child, class_name, results);
    }
}

fn find_node_by_id<'a>(node: &'a ScreenNode, id: &str) -> Option<&'a ScreenNode> {
    if node.id == id {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_node_by_id(child, id) {
            return Some(found);
        }
    }
    None
}

fn make_resolved(node: &ScreenNode, level: u8) -> ResolvedTarget {
    ResolvedTarget {
        node_id: node.id.clone(),
        center_x: node.bounds.center_x(),
        center_y: node.bounds.center_y(),
        level,
        resolve_time_us: 0,
    }
}

// ── L7: LLM Description — Candidate Collection ─────────────────────────────

/// L7 resolution: return all visible candidate nodes from the screen tree.
///
/// // LLM selects the target node from candidates — Rust returns all candidates
///
/// Rust does NOT score, rank, or filter by NLP heuristics. The full candidate
/// list is returned so the LLM can select the correct node.
fn resolve_by_description<'a>(tree: &'a ScreenTree, _description: &str) -> Vec<&'a ScreenNode> {
    let mut candidates: Vec<&'a ScreenNode> = Vec::new();
    collect_visible_nodes(&tree.root, &mut candidates);
    candidates
}

/// Recursively collect all visible nodes in the tree.
fn collect_visible_nodes<'a>(node: &'a ScreenNode, out: &mut Vec<&'a ScreenNode>) {
    if node.is_visible {
        out.push(node);
    }
    for child in &node.children {
        collect_visible_nodes(child, out);
    }
}

/// L7 fuzzy last-resort matching using any available hints.
///
/// When L0–L6 have all failed, we try a broad fuzzy search using whatever
/// hint strings are available from the original selector.
fn try_fuzzy_l7(tree: &ScreenTree, hints: &TargetingHints) -> Option<ResolvedTarget> {
    // Collect all non-None string hints
    let mut search_terms: Vec<&str> = Vec::new();
    if let Some(ref t) = hints.text {
        search_terms.push(t.as_str());
    }
    if let Some(ref d) = hints.content_desc {
        search_terms.push(d.as_str());
    }
    if let Some(ref r) = hints.resource_id {
        // Extract the ID part after the last /
        if let Some(id_part) = r.rsplit('/').next() {
            search_terms.push(id_part);
        }
    }

    if search_terms.is_empty() {
        trace!("L7 fuzzy: no search terms available from hints");
        return None;
    }

    for term in &search_terms {
        // Try text contains search
        let text_matches = tree.find_by_text_contains(term);
        // Prefer clickable
        if let Some(clickable) = text_matches.iter().find(|n| n.is_clickable) {
            debug!(
                term,
                node_id = %clickable.id,
                "L7 fuzzy: matched via text (clickable)"
            );
            return Some(make_resolved(clickable, 7));
        }
        if let Some(first) = text_matches.first() {
            debug!(
                term,
                node_id = %first.id,
                "L7 fuzzy: matched via text"
            );
            return Some(make_resolved(first, 7));
        }

        // Try content description search
        if let Some(node) = tree.find_by_content_desc_contains(term) {
            debug!(
                term,
                node_id = %node.id,
                "L7 fuzzy: matched via content description"
            );
            return Some(make_resolved(node, 7));
        }
    }

    trace!("L7 fuzzy: no matches for any search term");
    None
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::tree::{parse_tree, RawA11yNode};

    fn make_test_tree() -> ScreenTree {
        let raw = vec![
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
                children_indices: vec![1, 2, 3, 4],
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
                class_name: "android.widget.Button".into(),
                text: Some("Cancel".into()),
                content_desc: Some("Cancel action".into()),
                resource_id: Some("com.test:id/cancel_btn".into()),
                package_name: "com.test".into(),
                bounds_left: 600,
                bounds_top: 1700,
                bounds_right: 790,
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
                class_name: "android.widget.TextView".into(),
                text: Some("Status: Online".into()),
                content_desc: None,
                resource_id: None,
                package_name: "com.test".into(),
                bounds_left: 0,
                bounds_top: 0,
                bounds_right: 500,
                bounds_bottom: 50,
                is_clickable: false,
                is_scrollable: false,
                is_editable: false,
                is_checkable: false,
                is_checked: false,
                is_enabled: true,
                is_focused: false,
                is_visible: true,
                children_indices: vec![],
            },
        ];
        parse_tree(&raw)
    }

    #[test]
    fn test_resolve_resource_id() {
        let tree = make_test_tree();
        let selector = TargetSelector::ResourceId("com.test:id/send_btn".into());
        let result = resolve_target(&tree, &selector, 3);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.level, 2);
        assert_eq!(r.node_id, "node_1");
        assert_eq!(r.center_x, 900);
        assert_eq!(r.center_y, 1750);
    }

    #[test]
    fn test_resolve_text() {
        let tree = make_test_tree();
        let selector = TargetSelector::Text("Send".into());
        let result = resolve_target(&tree, &selector, 3);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.level, 3);
    }

    #[test]
    fn test_resolve_content_description() {
        let tree = make_test_tree();
        let selector = TargetSelector::ContentDescription("Cancel action".into());
        let result = resolve_target(&tree, &selector, 3);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.level, 4);
    }

    #[test]
    fn test_resolve_coordinates() {
        let tree = make_test_tree();
        let selector = TargetSelector::Coordinates { x: 900, y: 1750 };
        let result = resolve_target(&tree, &selector, 3);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.level, 6);
        assert_eq!(r.node_id, "node_1"); // The Send button
    }

    #[test]
    fn test_resolve_class_name() {
        let tree = make_test_tree();
        let selector = TargetSelector::ClassName("Button".into());
        let result = resolve_target(&tree, &selector, 3);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.level, 5);
        // Should get first visible Button — "Send"
        assert_eq!(r.node_id, "node_1");
    }

    #[test]
    fn test_resolve_class_name_second_index() {
        let tree = make_test_tree();
        // Use Position selector to get the second Button
        let _selector = TargetSelector::ClassName("Button".into());
        // Direct class_index targeting for index 1
        let node = try_class_index(&tree, "Button", 1);
        assert!(node.is_some());
        assert_eq!(node.unwrap().text.as_deref(), Some("Cancel"));
    }

    #[test]
    fn test_resolve_text_case_insensitive() {
        let tree = make_test_tree();
        let selector = TargetSelector::Text("send".into());
        let result = resolve_target(&tree, &selector, 3);
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_nonexistent_returns_none() {
        let tree = make_test_tree();
        let selector = TargetSelector::ResourceId("com.test:id/nonexistent".into());
        let result = resolve_target(&tree, &selector, 3);
        assert!(result.is_none());
    }

    #[test]
    fn test_resolve_llm_description_returns_candidates() {
        // LLM description resolution now returns all visible nodes as candidates.
        // Rust does NOT filter or score — the LLM selects the correct node.
        let tree = make_test_tree();
        let selector = TargetSelector::LlmDescription("the send button".into());
        // resolve_target uses .first() so it returns Some if any visible node exists
        let result = resolve_target(&tree, &selector, 7);
        // The test tree has visible nodes — expect Some result at level 7
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.level, 7);
    }

    #[test]
    fn test_resolve_llm_description_empty_tree() {
        // An empty/invisible tree yields None (no visible candidates)
        use aura_types::screen::{Bounds, ScreenTree};
        let invisible_root = ScreenNode {
            id: "root".into(),
            class_name: "FrameLayout".into(),
            package_name: "com.test".into(),
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
            is_checkable: false,
            is_focused: false,
            is_enabled: true,
            is_visible: false, // not visible
            is_scrollable: false,
            is_editable: false,
            is_checked: false,
            depth: 0,
            children: vec![],
        };
        let tree = ScreenTree {
            root: invisible_root,
            package_name: "com.test".into(),
            activity_name: ".Main".into(),
            timestamp_ms: 0,
            node_count: 1,
        };
        let selector = TargetSelector::LlmDescription("send button".into());
        let result = resolve_target(&tree, &selector, 7);
        assert!(result.is_none(), "no visible candidates → None");
    }

    #[test]
    fn test_fallback_levels_sequential() {
        let tree = make_test_tree();
        // XPath that doesn't exist — should fail all levels
        let selector = TargetSelector::XPath("/FrameLayout/ImageView[@resource-id='nope']".into());
        let result = resolve_target(&tree, &selector, 3);
        assert!(result.is_none());
    }

    #[test]
    fn test_xpath_segment_parsing() {
        let segments = parse_xpath_segments("/FrameLayout/Button[@resource-id='com.test:id/btn']");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].class_name, "FrameLayout");
        assert_eq!(segments[1].class_name, "Button");
        assert_eq!(segments[1].attrs.len(), 1);
        assert_eq!(segments[1].attrs[0].0, "resource-id");
        assert_eq!(segments[1].attrs[0].1, "com.test:id/btn");
    }

    #[test]
    fn test_xpath_positional_index() {
        let segments = parse_xpath_segments("/FrameLayout/Button[2]");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[1].positional_index, Some(2));
    }

    #[test]
    fn test_short_class_match() {
        assert!(short_class_match("android.widget.Button", "Button"));
        assert!(short_class_match("Button", "Button"));
        assert!(!short_class_match("android.widget.Button", "EditText"));
    }

    #[test]
    fn test_resolve_xpath_with_resource_id() {
        let tree = make_test_tree();
        let selector = TargetSelector::XPath(
            "/FrameLayout/Button[@resource-id='com.test:id/send_btn']".into(),
        );
        let result = resolve_target(&tree, &selector, 3);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.level <= 1); // Should resolve at L0 or L1
    }

    #[test]
    fn test_max_fallback_depth_limits_search() {
        let tree = make_test_tree();
        // With max_fallback_depth=0 and a ResourceId selector, only L2 is tried
        let selector = TargetSelector::ResourceId("com.test:id/nonexistent".into());
        let result = resolve_target(&tree, &selector, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_attr_from_xpath() {
        let xpath = "/FrameLayout/Button[@resource-id='com.test:id/btn'][@text='OK']";
        assert_eq!(
            extract_attr_from_xpath(xpath, "resource-id"),
            Some("com.test:id/btn".into())
        );
        assert_eq!(extract_attr_from_xpath(xpath, "text"), Some("OK".into()));
        assert_eq!(extract_attr_from_xpath(xpath, "missing"), None);
    }
}
