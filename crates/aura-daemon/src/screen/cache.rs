//! Screen State Cache — LRU cache of recent screen states with TTL.
//!
//! Provides fast lookup of previously-seen screens to avoid redundant
//! processing. Supports diff detection (did the screen change?), predictive
//! caching from ETG edges, and strict memory bounding.
//!
//! ## Limits
//! - Max 50 entries (configurable)
//! - Max 10 MB total memory (configurable)
//! - TTL 2 seconds (configurable)
//! - LRU eviction when over capacity

use aura_types::etg::{EtgEdge, EtgNode};
use aura_types::screen::{ScreenNode, ScreenTree};
use tracing::{debug, trace};

use super::verifier::hash_screen_state;

// ── Defaults ────────────────────────────────────────────────────────────────

const DEFAULT_MAX_ENTRIES: usize = 50;
const DEFAULT_MAX_BYTES: usize = 10 * 1024 * 1024; // 10 MB
const DEFAULT_TTL_MS: u64 = 2_000;

// ── Public types ────────────────────────────────────────────────────────────

/// Lightweight diff comparing current screen to the most recently cached one.
#[derive(Debug, Clone)]
pub struct QuickDiff {
    /// Whether any change was detected.
    pub changed: bool,
    /// Whether the foreground app changed.
    pub app_changed: bool,
    /// Whether the activity changed.
    pub activity_changed: bool,
    /// Whether any visible text changed.
    pub text_changed: bool,
    /// Difference in node count (current − previous).
    pub node_count_delta: i32,
    /// Hash of the previously cached screen.
    pub previous_hash: u64,
    /// Hash of the current screen.
    pub current_hash: u64,
}

/// Cache performance statistics.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Number of entries currently held.
    pub entries: usize,
    /// Total estimated memory consumption in bytes.
    pub memory_bytes: usize,
    /// Total cache hits.
    pub hits: u64,
    /// Total cache misses.
    pub misses: u64,
    /// Hit rate (0.0–1.0).
    pub hit_rate: f32,
    /// Number of predicted next-state entries.
    pub prediction_count: usize,
}

/// A predicted next screen state derived from ETG edges.
#[derive(Debug, Clone)]
pub struct PredictedState {
    /// State hash of the predicted target.
    pub target_hash: u64,
    /// Confidence derived from edge reliability.
    pub confidence: f32,
    /// Expected package name.
    pub package_name: String,
    /// Expected activity name.
    pub activity_name: String,
}

// ── Internal types ──────────────────────────────────────────────────────────

/// A single cached screen state.
struct CacheEntry {
    state_hash: u64,
    tree: ScreenTree,
    inserted_at_ms: u64,
    last_access_ms: u64,
    estimated_bytes: usize,
}

// ── ScreenCache ─────────────────────────────────────────────────────────────

/// Screen state cache with LRU eviction, TTL, and memory bounding.
///
/// The cache keeps the most-recently-used screen states in memory so the
/// targeting and verification subsystems can quickly check whether the
/// screen has changed, avoiding redundant accessibility tree processing.
pub struct ScreenCache {
    entries: Vec<CacheEntry>, // Sorted by last_access_ms (MRU first)
    max_entries: usize,
    max_bytes: usize,
    current_bytes: usize,
    ttl_ms: u64,
    predicted_next: Vec<PredictedState>,
    hits: u64,
    misses: u64,
    /// Injectable clock for testing. Returns epoch milliseconds.
    clock: Box<dyn Fn() -> u64 + Send>,
}

impl std::fmt::Debug for ScreenCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenCache")
            .field("entries", &self.entries.len())
            .field("max_entries", &self.max_entries)
            .field("current_bytes", &self.current_bytes)
            .field("max_bytes", &self.max_bytes)
            .field("ttl_ms", &self.ttl_ms)
            .field("hits", &self.hits)
            .field("misses", &self.misses)
            .finish()
    }
}

impl ScreenCache {
    /// Create a cache with default limits (50 entries, 10 MB, 2 s TTL).
    pub fn new() -> Self {
        Self::with_config(DEFAULT_MAX_ENTRIES, DEFAULT_MAX_BYTES, DEFAULT_TTL_MS)
    }

    /// Create a cache with custom limits.
    pub fn with_config(max_entries: usize, max_bytes: usize, ttl_ms: u64) -> Self {
        Self {
            entries: Vec::with_capacity(max_entries.min(64)),
            max_entries,
            max_bytes,
            current_bytes: 0,
            ttl_ms,
            predicted_next: Vec::new(),
            hits: 0,
            misses: 0,
            clock: Box::new(default_clock),
        }
    }

    /// Create a cache with an injectable clock (for testing).
    #[cfg(test)]
    fn with_clock(
        max_entries: usize,
        max_bytes: usize,
        ttl_ms: u64,
        clock: Box<dyn Fn() -> u64 + Send>,
    ) -> Self {
        Self {
            entries: Vec::with_capacity(max_entries.min(64)),
            max_entries,
            max_bytes,
            current_bytes: 0,
            ttl_ms,
            predicted_next: Vec::new(),
            hits: 0,
            misses: 0,
            clock,
        }
    }

    /// Current wall-clock time in milliseconds.
    fn now_ms(&self) -> u64 {
        (self.clock)()
    }

    // ── Lookup ──────────────────────────────────────────────────────────────

    /// Look up a cached screen by state hash.
    ///
    /// Returns `None` if the entry is missing or has expired. Updates the
    /// entry's last-access time and promotes it to MRU on hit.
    pub fn get(&mut self, state_hash: u64) -> Option<&ScreenTree> {
        let now = self.now_ms();
        let pos = self.entries.iter().position(|e| e.state_hash == state_hash);

        match pos {
            Some(idx) => {
                // Check TTL
                if now.saturating_sub(self.entries[idx].inserted_at_ms) > self.ttl_ms {
                    trace!(state_hash, "cache entry expired");
                    self.misses += 1;
                    // Remove expired entry
                    let removed = self.entries.remove(idx);
                    self.current_bytes = self.current_bytes.saturating_sub(removed.estimated_bytes);
                    return None;
                }
                // Promote to MRU
                self.entries[idx].last_access_ms = now;
                if idx != 0 {
                    let entry = self.entries.remove(idx);
                    self.entries.insert(0, entry);
                }
                self.hits += 1;
                trace!(state_hash, "cache hit");
                Some(&self.entries[0].tree)
            }
            None => {
                self.misses += 1;
                trace!(state_hash, "cache miss");
                None
            }
        }
    }

    /// Check if a state is cached (does not update access time or check TTL).
    pub fn is_cached(&self, state_hash: u64) -> bool {
        self.entries.iter().any(|e| e.state_hash == state_hash)
    }

    // ── Insert ──────────────────────────────────────────────────────────────

    /// Insert a new screen state into the cache.
    ///
    /// If an entry with the same hash already exists, it is replaced.
    /// Evicts LRU entries if over max-entries or max-bytes capacity.
    pub fn insert(&mut self, tree: ScreenTree) {
        let hash = hash_screen_state(&tree);
        let estimated = estimate_tree_bytes(&tree);
        let now = self.now_ms();

        // Remove existing entry with same hash (dedup)
        if let Some(idx) = self.entries.iter().position(|e| e.state_hash == hash) {
            let removed = self.entries.remove(idx);
            self.current_bytes = self.current_bytes.saturating_sub(removed.estimated_bytes);
            trace!(state_hash = hash, "replaced existing cache entry");
        }

        // Evict expired entries first
        self.evict_expired_inner(now);

        // Evict LRU entries while over capacity
        while self.entries.len() >= self.max_entries && !self.entries.is_empty() {
            let removed = self.entries.pop(); // pop last = LRU (least recently used)
            if let Some(r) = removed {
                self.current_bytes = self.current_bytes.saturating_sub(r.estimated_bytes);
                debug!(
                    evicted_hash = r.state_hash,
                    reason = "max_entries",
                    "evicted LRU cache entry"
                );
            }
        }

        // Evict LRU while over memory budget
        while self.current_bytes + estimated > self.max_bytes && !self.entries.is_empty() {
            let removed = self.entries.pop();
            if let Some(r) = removed {
                self.current_bytes = self.current_bytes.saturating_sub(r.estimated_bytes);
                debug!(
                    evicted_hash = r.state_hash,
                    reason = "max_bytes",
                    "evicted LRU cache entry"
                );
            }
        }

        // If a single entry exceeds budget and cache is empty, still store it
        // (we always allow at least one entry).
        let entry = CacheEntry {
            state_hash: hash,
            tree,
            inserted_at_ms: now,
            last_access_ms: now,
            estimated_bytes: estimated,
        };

        self.current_bytes += estimated;
        self.entries.insert(0, entry); // MRU at front

        trace!(
            state_hash = hash,
            estimated_bytes = estimated,
            total_entries = self.entries.len(),
            total_bytes = self.current_bytes,
            "inserted cache entry"
        );
    }

    // ── Diff ────────────────────────────────────────────────────────────────

    /// Compare the given screen to the most recently cached screen (MRU).
    ///
    /// Returns `None` if the cache is empty.
    pub fn diff_from_last(&self, current: &ScreenTree) -> Option<QuickDiff> {
        let last = self.entries.first()?;
        let current_hash = hash_screen_state(current);

        let changed = current_hash != last.state_hash;
        let app_changed = current.package_name != last.tree.package_name;
        let activity_changed = current.activity_name != last.tree.activity_name;

        // Quick text comparison: just compare the set of all_text
        let before_text = last.tree.all_text();
        let after_text = current.all_text();
        let text_changed = before_text != after_text;

        let node_count_delta = current.node_count as i32 - last.tree.node_count as i32;

        Some(QuickDiff {
            changed,
            app_changed,
            activity_changed,
            text_changed,
            node_count_delta,
            previous_hash: last.state_hash,
            current_hash,
        })
    }

    // ── Predictions ─────────────────────────────────────────────────────────

    /// Update predicted next states from ETG outgoing edges.
    ///
    /// `edges` is the list of `(target_hash, &EtgEdge)` pairs from
    /// `EtgStore::outgoing_edges`, and `nodes` is the corresponding ETG
    /// nodes for those target hashes (for package/activity metadata).
    pub fn update_predictions(&mut self, edges: &[(u64, &EtgEdge)], nodes: &[&EtgNode]) {
        self.predicted_next.clear();

        for (target_hash, edge) in edges {
            let reliability = edge.reliability();
            if reliability < 0.1 {
                continue; // Skip very unreliable edges
            }

            // Find matching ETG node for metadata
            let (pkg, act) = nodes
                .iter()
                .find(|n| n.state_hash == *target_hash)
                .map(|n| (n.package_name.clone(), n.activity_name.clone()))
                .unwrap_or_else(|| (String::new(), String::new()));

            self.predicted_next.push(PredictedState {
                target_hash: *target_hash,
                confidence: reliability,
                package_name: pkg,
                activity_name: act,
            });
        }

        // Sort by confidence descending
        self.predicted_next.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Keep top 10 predictions
        self.predicted_next.truncate(10);

        debug!(
            prediction_count = self.predicted_next.len(),
            "updated predicted next states"
        );
    }

    /// Check if a state hash is among the current predictions.
    pub fn is_predicted(&self, state_hash: u64) -> bool {
        self.predicted_next
            .iter()
            .any(|p| p.target_hash == state_hash)
    }

    /// Get a reference to the current predictions.
    pub fn predictions(&self) -> &[PredictedState] {
        &self.predicted_next
    }

    // ── Eviction ────────────────────────────────────────────────────────────

    /// Evict all entries whose TTL has expired.
    pub fn evict_expired(&mut self) {
        let now = self.now_ms();
        self.evict_expired_inner(now);
    }

    fn evict_expired_inner(&mut self, now: u64) {
        let ttl = self.ttl_ms;
        let before_count = self.entries.len();

        self.entries.retain(|e| {
            let alive = now.saturating_sub(e.inserted_at_ms) <= ttl;
            if !alive {
                // Can't modify self.current_bytes in retain closure,
                // we'll recompute below.
            }
            alive
        });

        if self.entries.len() < before_count {
            // Recompute total bytes
            self.current_bytes = self.entries.iter().map(|e| e.estimated_bytes).sum();
            let evicted = before_count - self.entries.len();
            debug!(
                evicted,
                remaining = self.entries.len(),
                "evicted expired entries"
            );
        }
    }

    /// Clear the entire cache and reset statistics.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.current_bytes = 0;
        self.predicted_next.clear();
        self.hits = 0;
        self.misses = 0;
        debug!("cache cleared");
    }

    // ── Stats ───────────────────────────────────────────────────────────────

    /// Get current cache performance statistics.
    pub fn stats(&self) -> CacheStats {
        let total = self.hits + self.misses;
        let hit_rate = if total > 0 {
            self.hits as f32 / total as f32
        } else {
            0.0
        };

        CacheStats {
            entries: self.entries.len(),
            memory_bytes: self.current_bytes,
            hits: self.hits,
            misses: self.misses,
            hit_rate,
            prediction_count: self.predicted_next.len(),
        }
    }

    /// Total memory usage in bytes (estimated).
    pub fn memory_usage(&self) -> usize {
        self.current_bytes
    }

    /// Number of entries in the cache.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for ScreenCache {
    fn default() -> Self {
        Self::new()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Default wall-clock function (epoch milliseconds).
fn default_clock() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Estimate the heap size of a `ScreenTree` in bytes.
///
/// This is a conservative approximation used for memory budgeting.
/// Counts string lengths + fixed-size fields + child overhead recursively.
pub fn estimate_tree_bytes(tree: &ScreenTree) -> usize {
    let mut bytes: usize = 0;

    // Top-level strings
    bytes += tree.package_name.len();
    bytes += tree.activity_name.len();

    // Fixed fields: timestamp_ms (8) + node_count (4)
    bytes += 12;

    // Recurse into the node tree
    bytes += estimate_node_bytes(&tree.root);

    bytes
}

/// Estimate heap size of a single `ScreenNode` and all its descendants.
fn estimate_node_bytes(node: &ScreenNode) -> usize {
    let mut bytes: usize = 0;

    // Strings
    bytes += node.id.len();
    bytes += node.class_name.len();
    bytes += node.package_name.len();
    bytes += node.text.as_ref().map_or(0, |s| s.len() + 24); // Option overhead
    bytes += node
        .content_description
        .as_ref()
        .map_or(0, |s| s.len() + 24);
    bytes += node.resource_id.as_ref().map_or(0, |s| s.len() + 24);

    // Bounds (4 × i32) + bool flags (11) + depth (1)
    bytes += 16 + 11 + 1;

    // Vec overhead for children
    bytes += 24; // Vec<ScreenNode> overhead (ptr + len + cap)

    // Recurse
    for child in &node.children {
        bytes += estimate_node_bytes(child);
    }

    bytes
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use aura_types::screen::Bounds;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

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

    /// Create a test clock backed by an atomic counter.
    fn test_clock(start_ms: u64) -> (Arc<AtomicU64>, Box<dyn Fn() -> u64 + Send>) {
        let time = Arc::new(AtomicU64::new(start_ms));
        let time_clone = Arc::clone(&time);
        let clock = Box::new(move || time_clone.load(Ordering::Relaxed));
        (time, clock)
    }

    // ── Basic insert / get ──────────────────────────────────────────────────

    #[test]
    fn test_insert_and_get() {
        let (time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 5000, clock);

        let tree = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        let hash = hash_screen_state(&tree);

        cache.insert(tree);
        assert_eq!(cache.len(), 1);

        let _ = time; // keep alive
        let result = cache.get(hash);
        assert!(result.is_some());
        assert_eq!(result.map(|t| &t.package_name as &str), Some("com.test"));
    }

    #[test]
    fn test_get_missing_returns_none() {
        let (_time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 5000, clock);
        assert!(cache.get(12345).is_none());
    }

    #[test]
    fn test_is_cached() {
        let (_time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 5000, clock);

        let tree = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        let hash = hash_screen_state(&tree);

        assert!(!cache.is_cached(hash));
        cache.insert(tree);
        assert!(cache.is_cached(hash));
    }

    // ── TTL expiration ──────────────────────────────────────────────────────

    #[test]
    fn test_ttl_expiration_on_get() {
        let (time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 2000, clock);

        let tree = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        let hash = hash_screen_state(&tree);

        cache.insert(tree);
        assert!(cache.get(hash).is_some());

        // Advance time past TTL
        time.store(4000, Ordering::Relaxed);
        assert!(cache.get(hash).is_none());
    }

    #[test]
    fn test_evict_expired() {
        let (time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 2000, clock);

        let tree1 = make_tree(make_node("r1", Some("A"), vec![]), "com.a", ".A");
        let tree2 = make_tree(make_node("r2", Some("B"), vec![]), "com.b", ".B");
        cache.insert(tree1);
        cache.insert(tree2);
        assert_eq!(cache.len(), 2);

        // Advance past TTL
        time.store(4000, Ordering::Relaxed);
        cache.evict_expired();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_ttl_not_expired_within_window() {
        let (time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 5000, clock);

        let tree = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        let hash = hash_screen_state(&tree);
        cache.insert(tree);

        // Still within TTL
        time.store(5000, Ordering::Relaxed);
        assert!(cache.get(hash).is_some());
    }

    // ── LRU eviction ────────────────────────────────────────────────────────

    #[test]
    fn test_lru_eviction_max_entries() {
        let (time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(3, DEFAULT_MAX_BYTES, 100_000, clock);

        // Insert 3 entries
        let t1 = make_tree(make_node("r1", Some("A"), vec![]), "com.a", ".A");
        let h1 = hash_screen_state(&t1);
        cache.insert(t1);

        time.store(1001, Ordering::Relaxed);
        let t2 = make_tree(make_node("r2", Some("B"), vec![]), "com.b", ".B");
        let h2 = hash_screen_state(&t2);
        cache.insert(t2);

        time.store(1002, Ordering::Relaxed);
        let t3 = make_tree(make_node("r3", Some("C"), vec![]), "com.c", ".C");
        cache.insert(t3);

        assert_eq!(cache.len(), 3);

        // Insert 4th — should evict LRU (t1)
        time.store(1003, Ordering::Relaxed);
        let t4 = make_tree(make_node("r4", Some("D"), vec![]), "com.d", ".D");
        cache.insert(t4);

        assert_eq!(cache.len(), 3);
        assert!(!cache.is_cached(h1)); // t1 evicted (LRU)
        assert!(cache.is_cached(h2)); // t2 still present
    }

    #[test]
    fn test_lru_access_promotes() {
        let (time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(3, DEFAULT_MAX_BYTES, 100_000, clock);

        let t1 = make_tree(make_node("r1", Some("A"), vec![]), "com.a", ".A");
        let h1 = hash_screen_state(&t1);
        cache.insert(t1);

        time.store(1001, Ordering::Relaxed);
        let t2 = make_tree(make_node("r2", Some("B"), vec![]), "com.b", ".B");
        let h2 = hash_screen_state(&t2);
        cache.insert(t2);

        time.store(1002, Ordering::Relaxed);
        let t3 = make_tree(make_node("r3", Some("C"), vec![]), "com.c", ".C");
        cache.insert(t3);

        // Access t1 to promote it to MRU
        time.store(1003, Ordering::Relaxed);
        let _ = cache.get(h1);

        // Insert 4th — should evict LRU (now t2, since t1 was promoted)
        time.store(1004, Ordering::Relaxed);
        let t4 = make_tree(make_node("r4", Some("D"), vec![]), "com.d", ".D");
        cache.insert(t4);

        assert_eq!(cache.len(), 3);
        assert!(cache.is_cached(h1)); // t1 was promoted
        assert!(!cache.is_cached(h2)); // t2 evicted (now LRU)
    }

    // ── Memory-bounded eviction ─────────────────────────────────────────────

    #[test]
    fn test_memory_bounded_eviction() {
        let (_time, clock) = test_clock(1000);
        // Set a very small memory budget
        let mut cache = ScreenCache::with_clock(50, 500, 100_000, clock);

        // Each tree is ~100-200 bytes estimated
        let t1 = make_tree(make_node("r1", Some("A"), vec![]), "com.a", ".A");
        cache.insert(t1);

        let t2 = make_tree(make_node("r2", Some("B"), vec![]), "com.b", ".B");
        cache.insert(t2);

        let t3 = make_tree(make_node("r3", Some("C"), vec![]), "com.c", ".C");
        cache.insert(t3);

        // Eventually memory should be bounded
        assert!(cache.memory_usage() <= 500 + 300); // Allow some slack for single large entry
    }

    #[test]
    fn test_single_entry_exceeds_budget_still_stored() {
        let (_time, clock) = test_clock(1000);
        // Budget smaller than a single entry
        let mut cache = ScreenCache::with_clock(50, 1, 100_000, clock);

        let tree = make_tree(make_node("r1", Some("data"), vec![]), "com.a", ".A");
        let hash = hash_screen_state(&tree);
        cache.insert(tree);

        assert_eq!(cache.len(), 1);
        assert!(cache.is_cached(hash));
    }

    // ── QuickDiff ───────────────────────────────────────────────────────────

    #[test]
    fn test_diff_from_last_no_change() {
        let (_time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 100_000, clock);

        let tree = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        let same = make_tree(
            make_node("root", Some("Hello"), vec![]),
            "com.test",
            ".Main",
        );
        cache.insert(tree);

        let diff = cache.diff_from_last(&same);
        assert!(diff.is_some());
        let diff = diff.expect("diff should exist");
        assert!(!diff.changed);
        assert!(!diff.app_changed);
        assert!(!diff.activity_changed);
        assert!(!diff.text_changed);
        assert_eq!(diff.node_count_delta, 0);
    }

    #[test]
    fn test_diff_from_last_text_changed() {
        let (_time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 100_000, clock);

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
        cache.insert(before);

        let diff = cache.diff_from_last(&after);
        assert!(diff.is_some());
        let diff = diff.expect("diff should exist");
        assert!(diff.changed);
        assert!(diff.text_changed);
        assert!(!diff.app_changed);
    }

    #[test]
    fn test_diff_from_last_app_changed() {
        let (_time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 100_000, clock);

        let before = make_tree(make_node("root", None, vec![]), "com.app1", ".Main");
        let after = make_tree(make_node("root", None, vec![]), "com.app2", ".Main");
        cache.insert(before);

        let diff = cache.diff_from_last(&after);
        assert!(diff.is_some());
        let diff = diff.expect("diff should exist");
        assert!(diff.changed);
        assert!(diff.app_changed);
    }

    #[test]
    fn test_diff_from_last_activity_changed() {
        let (_time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 100_000, clock);

        let before = make_tree(make_node("root", None, vec![]), "com.test", ".Main");
        let after = make_tree(make_node("root", None, vec![]), "com.test", ".Settings");
        cache.insert(before);

        let diff = cache.diff_from_last(&after);
        assert!(diff.is_some());
        let diff = diff.expect("diff should exist");
        assert!(diff.changed);
        assert!(diff.activity_changed);
    }

    #[test]
    fn test_diff_from_last_node_count_delta() {
        let (_time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 100_000, clock);

        let before = make_tree(make_node("root", None, vec![]), "com.test", ".Main");
        let after = make_tree(
            make_node("root", None, vec![make_node("child", Some("x"), vec![])]),
            "com.test",
            ".Main",
        );
        cache.insert(before);

        let diff = cache.diff_from_last(&after);
        assert!(diff.is_some());
        let diff = diff.expect("diff should exist");
        assert_eq!(diff.node_count_delta, 1);
    }

    #[test]
    fn test_diff_from_last_empty_cache() {
        let cache = ScreenCache::new();
        let tree = make_tree(make_node("root", None, vec![]), "com.test", ".Main");
        assert!(cache.diff_from_last(&tree).is_none());
    }

    // ── Predictions ─────────────────────────────────────────────────────────

    #[test]
    fn test_update_predictions() {
        let mut cache = ScreenCache::new();

        let edge = EtgEdge {
            from_node: 1,
            to_node: 2,
            action: aura_types::actions::ActionType::Tap { x: 100, y: 200 },
            success_count: 9,
            fail_count: 1,
            avg_duration_ms: 200,
            last_used_ms: 1_000_000,
        };

        let node = EtgNode {
            id: 2,
            package_name: "com.target".into(),
            activity_name: ".Target".into(),
            state_hash: 42,
            interactive_elements: vec![],
            visit_count: 5,
            last_visit_ms: 1_000_000,
        };

        cache.update_predictions(&[(42, &edge)], &[&node]);

        assert_eq!(cache.predictions().len(), 1);
        assert!(cache.is_predicted(42));
        assert!(!cache.is_predicted(99));
        assert!((cache.predictions()[0].confidence - 0.9).abs() < f32::EPSILON);
        assert_eq!(cache.predictions()[0].package_name, "com.target");
    }

    #[test]
    fn test_predictions_filter_unreliable() {
        let mut cache = ScreenCache::new();

        let edge = EtgEdge {
            from_node: 1,
            to_node: 2,
            action: aura_types::actions::ActionType::Back,
            success_count: 0,
            fail_count: 10,
            avg_duration_ms: 0,
            last_used_ms: 0,
        };

        let node = EtgNode {
            id: 2,
            package_name: "com.x".into(),
            activity_name: ".X".into(),
            state_hash: 99,
            interactive_elements: vec![],
            visit_count: 0,
            last_visit_ms: 0,
        };

        cache.update_predictions(&[(99, &edge)], &[&node]);
        // reliability = 0.0 < 0.1, should be filtered
        assert!(cache.predictions().is_empty());
    }

    #[test]
    fn test_predictions_sorted_by_confidence() {
        let mut cache = ScreenCache::new();

        let edge_low = EtgEdge {
            from_node: 1,
            to_node: 2,
            action: aura_types::actions::ActionType::Back,
            success_count: 3,
            fail_count: 7,
            avg_duration_ms: 100,
            last_used_ms: 1000,
        };
        let edge_high = EtgEdge {
            from_node: 1,
            to_node: 3,
            action: aura_types::actions::ActionType::Back,
            success_count: 9,
            fail_count: 1,
            avg_duration_ms: 100,
            last_used_ms: 1000,
        };

        let node2 = EtgNode {
            id: 2,
            package_name: "com.low".into(),
            activity_name: ".Low".into(),
            state_hash: 20,
            interactive_elements: vec![],
            visit_count: 1,
            last_visit_ms: 0,
        };
        let node3 = EtgNode {
            id: 3,
            package_name: "com.high".into(),
            activity_name: ".High".into(),
            state_hash: 30,
            interactive_elements: vec![],
            visit_count: 1,
            last_visit_ms: 0,
        };

        cache.update_predictions(&[(20, &edge_low), (30, &edge_high)], &[&node2, &node3]);

        assert_eq!(cache.predictions().len(), 2);
        // High confidence first
        assert_eq!(cache.predictions()[0].target_hash, 30);
        assert_eq!(cache.predictions()[1].target_hash, 20);
    }

    // ── Stats ───────────────────────────────────────────────────────────────

    #[test]
    fn test_stats_initial() {
        let cache = ScreenCache::new();
        let stats = cache.stats();
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
        assert!((stats.hit_rate - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_stats_after_operations() {
        let (_time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 100_000, clock);

        let tree = make_tree(make_node("root", Some("X"), vec![]), "com.test", ".Main");
        let hash = hash_screen_state(&tree);
        cache.insert(tree);

        let _ = cache.get(hash); // hit
        let _ = cache.get(hash); // hit
        let _ = cache.get(99999); // miss

        let stats = cache.stats();
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_rate - 2.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn test_memory_usage() {
        let (_time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 100_000, clock);

        assert_eq!(cache.memory_usage(), 0);
        let tree = make_tree(make_node("root", Some("Data"), vec![]), "com.test", ".Main");
        cache.insert(tree);
        assert!(cache.memory_usage() > 0);
    }

    // ── Clear ───────────────────────────────────────────────────────────────

    #[test]
    fn test_clear() {
        let (_time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 100_000, clock);

        let tree = make_tree(make_node("root", Some("A"), vec![]), "com.a", ".A");
        cache.insert(tree);
        let tree2 = make_tree(make_node("root", Some("B"), vec![]), "com.b", ".B");
        cache.insert(tree2);

        cache.clear();
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.memory_usage(), 0);
        assert!(cache.is_empty());

        let stats = cache.stats();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
    }

    // ── Edge cases ──────────────────────────────────────────────────────────

    #[test]
    fn test_duplicate_insert_replaces() {
        let (_time, clock) = test_clock(1000);
        let mut cache = ScreenCache::with_clock(50, DEFAULT_MAX_BYTES, 100_000, clock);

        let tree1 = make_tree(make_node("root", Some("A"), vec![]), "com.a", ".A");
        let hash = hash_screen_state(&tree1);
        let tree2 = make_tree(make_node("root", Some("A"), vec![]), "com.a", ".A");

        cache.insert(tree1);
        cache.insert(tree2);

        assert_eq!(cache.len(), 1);
        assert!(cache.is_cached(hash));
    }

    #[test]
    fn test_empty_cache_operations() {
        let mut cache = ScreenCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.memory_usage(), 0);
        assert!(!cache.is_cached(42));
        assert!(!cache.is_predicted(42));
        cache.evict_expired(); // should not panic
        cache.clear(); // should not panic
    }

    #[test]
    fn test_estimate_tree_bytes_reasonable() {
        let tree = make_tree(
            make_node(
                "root",
                Some("Hello World"),
                vec![
                    make_node("c1", Some("Child 1"), vec![]),
                    make_node("c2", Some("Child 2"), vec![]),
                ],
            ),
            "com.test.app",
            ".MainActivity",
        );
        let bytes = estimate_tree_bytes(&tree);
        // Should be non-trivial but not huge
        assert!(bytes > 50);
        assert!(bytes < 10_000);
    }

    #[test]
    fn test_default_creates_working_cache() {
        let mut cache = ScreenCache::default();
        let tree = make_tree(make_node("root", Some("X"), vec![]), "com.test", ".Main");
        cache.insert(tree);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_debug_format() {
        let cache = ScreenCache::new();
        let dbg = format!("{cache:?}");
        assert!(dbg.contains("ScreenCache"));
        assert!(dbg.contains("entries"));
    }
}
