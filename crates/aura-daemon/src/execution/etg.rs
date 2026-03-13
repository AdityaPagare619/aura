//! Element-Transition Graph (ETG) — AURA's learning engine for screen navigation.
//!
//! The ETG records which actions transition between screen states, so AURA can
//! reuse known paths instead of asking the LLM every time. Over time, AURA
//! "learns" the app's navigation graph.
//!
//! ## Design:
//! - In-memory graph for fast lookup, backed by SQLite for persistence
//! - Max 10,000 nodes, 50,000 edges (LRU eviction when full)
//! - Edge reliability: `raw_reliability * freshness_factor` where
//!   `freshness_factor = 2^(-days_since_use / 14)` (14-day half-life)
//! - Edges below 0.3 effective reliability are pruned
//! - BFS pathfinding with reliability-weighted scoring

use std::collections::{HashMap, VecDeque};

use aura_types::actions::ActionType;
use aura_types::etg::{EtgEdge, EtgNode, EtgPath};
use tracing::{debug, warn};

/// Maximum number of nodes in the ETG.
const MAX_NODES: usize = 10_000;
/// Maximum number of edges in the ETG.
const MAX_EDGES: usize = 50_000;
/// Half-life for reliability freshness decay (days).
const FRESHNESS_HALF_LIFE_DAYS: f64 = 14.0;
/// Minimum effective reliability before an edge is pruned.
const MIN_RELIABILITY: f32 = 0.3;
/// Maximum BFS search depth.
const MAX_BFS_DEPTH: u32 = 20;

/// In-memory Element-Transition Graph with SQLite persistence.
pub struct EtgStore {
    /// Node map: state_hash -> EtgNode
    nodes: HashMap<u64, EtgNode>,
    /// Edge map: (from_hash, to_hash) -> EtgEdge
    edges: HashMap<(u64, u64), EtgEdge>,
    /// Adjacency list: from_hash -> Vec<to_hash>
    adjacency: HashMap<u64, Vec<u64>>,
    /// Next node ID counter.
    next_node_id: u64,
    /// Current timestamp (ms) for freshness calculations.
    current_time_ms: u64,
    /// SQLite connection (optional — None in tests without DB).
    db: Option<rusqlite::Connection>,
}

impl EtgStore {
    /// Create a new in-memory ETG store without SQLite backing.
    pub fn in_memory() -> Self {
        Self {
            nodes: HashMap::with_capacity(1024),
            edges: HashMap::with_capacity(4096),
            adjacency: HashMap::with_capacity(1024),
            next_node_id: 1,
            current_time_ms: 0,
            db: None,
        }
    }

    /// Create an ETG store backed by a SQLite database at the given path.
    pub fn with_sqlite(db_path: &str) -> Result<Self, String> {
        let conn = rusqlite::Connection::open(db_path)
            .map_err(|e| format!("failed to open ETG database: {e}"))?;

        // Create tables if they don't exist
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS etg_nodes (
                id INTEGER PRIMARY KEY,
                package_name TEXT NOT NULL,
                activity_name TEXT NOT NULL,
                state_hash INTEGER NOT NULL UNIQUE,
                interactive_elements TEXT NOT NULL,
                visit_count INTEGER NOT NULL DEFAULT 0,
                last_visit_ms INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS etg_edges (
                from_node INTEGER NOT NULL,
                to_node INTEGER NOT NULL,
                action_json TEXT NOT NULL,
                success_count INTEGER NOT NULL DEFAULT 0,
                fail_count INTEGER NOT NULL DEFAULT 0,
                avg_duration_ms INTEGER NOT NULL DEFAULT 0,
                last_used_ms INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (from_node, to_node)
            );
            CREATE INDEX IF NOT EXISTS idx_etg_nodes_hash ON etg_nodes(state_hash);
            CREATE INDEX IF NOT EXISTS idx_etg_edges_from ON etg_edges(from_node);",
        )
        .map_err(|e| format!("failed to create ETG tables: {e}"))?;

        let mut store = Self {
            nodes: HashMap::with_capacity(1024),
            edges: HashMap::with_capacity(4096),
            adjacency: HashMap::with_capacity(1024),
            next_node_id: 1,
            current_time_ms: 0,
            db: Some(conn),
        };

        // Load existing data from SQLite
        store.load_from_db()?;

        Ok(store)
    }

    /// Set the current time (ms) for freshness calculations.
    pub fn set_current_time_ms(&mut self, ms: u64) {
        self.current_time_ms = ms;
    }

    /// Number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    // ── Node operations ─────────────────────────────────────────────────────

    /// Get or create a node for the given screen state.
    pub fn get_or_create_node(
        &mut self,
        state_hash: u64,
        package_name: &str,
        activity_name: &str,
        interactive_elements: &[String],
    ) -> u64 {
        // Return existing node if found
        if let Some(node) = self.nodes.get_mut(&state_hash) {
            node.visit_count += 1;
            node.last_visit_ms = self.current_time_ms;
            return node.id;
        }

        // Evict LRU node if at capacity
        if self.nodes.len() >= MAX_NODES {
            self.evict_lru_node();
        }

        // Create new node
        let id = self.next_node_id;
        self.next_node_id += 1;

        let node = EtgNode {
            id,
            package_name: package_name.to_string(),
            activity_name: activity_name.to_string(),
            state_hash,
            interactive_elements: interactive_elements.to_vec(),
            visit_count: 1,
            last_visit_ms: self.current_time_ms,
        };

        self.nodes.insert(state_hash, node);
        self.adjacency.entry(state_hash).or_default();

        debug!(node_id = id, state_hash, "ETG: created node");
        id
    }

    /// Look up a node by state hash.
    pub fn get_node(&self, state_hash: u64) -> Option<&EtgNode> {
        self.nodes.get(&state_hash)
    }

    // ── Edge operations ─────────────────────────────────────────────────────

    /// Record a transition between two states.
    ///
    /// If the edge already exists, updates its statistics.
    /// If not, creates a new edge.
    pub fn record_transition(
        &mut self,
        from_hash: u64,
        to_hash: u64,
        action: &ActionType,
        success: bool,
        duration_ms: u32,
    ) {
        let key = (from_hash, to_hash);

        if let Some(edge) = self.edges.get_mut(&key) {
            // Update existing edge
            if success {
                edge.success_count += 1;
            } else {
                edge.fail_count += 1;
            }
            // Running Welford online average of duration (EtgEdge uses f32)
            let total = edge.success_count + edge.fail_count;
            let delta = duration_ms as f32 - edge.avg_duration_ms;
            edge.avg_duration_ms += delta / total as f32;
            let delta2 = duration_ms as f32 - edge.avg_duration_ms;
            edge.m2_duration_ms += delta * delta2;
            edge.last_used_ms = self.current_time_ms;
        } else {
            // Evict if at capacity
            if self.edges.len() >= MAX_EDGES {
                self.evict_lru_edges(100); // evict 100 least recently used
            }

            // Create new edge
            let edge = EtgEdge {
                from_node: self.nodes.get(&from_hash).map(|n| n.id).unwrap_or(0),
                to_node: self.nodes.get(&to_hash).map(|n| n.id).unwrap_or(0),
                action: action.clone(),
                success_count: if success { 1 } else { 0 },
                fail_count: if success { 0 } else { 1 },
                avg_duration_ms: duration_ms as f32,
                m2_duration_ms: 0.0,
                last_used_ms: self.current_time_ms,
            };

            self.edges.insert(key, edge);

            // Update adjacency list
            self.adjacency
                .entry(from_hash)
                .or_default()
                .push(to_hash);
        }
    }

    /// Get the edge between two states.
    pub fn get_edge(&self, from_hash: u64, to_hash: u64) -> Option<&EtgEdge> {
        self.edges.get(&(from_hash, to_hash))
    }

    /// Get all outgoing edges from a state.
    pub fn outgoing_edges(&self, from_hash: u64) -> Vec<(u64, &EtgEdge)> {
        let neighbors = match self.adjacency.get(&from_hash) {
            Some(n) => n,
            None => return Vec::new(),
        };

        neighbors
            .iter()
            .filter_map(|to| {
                self.edges
                    .get(&(from_hash, *to))
                    .map(|edge| (*to, edge))
            })
            .collect()
    }

    /// Compute the effective reliability of an edge, accounting for freshness decay.
    pub fn effective_reliability(&self, edge: &EtgEdge) -> f32 {
        let raw = edge.reliability();
        let days_since_use = (self.current_time_ms.saturating_sub(edge.last_used_ms)) as f64
            / (1000.0 * 60.0 * 60.0 * 24.0);
        let freshness = 2.0_f64.powf(-days_since_use / FRESHNESS_HALF_LIFE_DAYS) as f32;
        raw * freshness
    }

    // ── BFS Pathfinding ─────────────────────────────────────────────────────

    /// Find the best path from `from_hash` to `to_hash` using BFS with
    /// reliability-weighted scoring.
    ///
    /// Returns `None` if no path exists within MAX_BFS_DEPTH.
    pub fn find_path(&self, from_hash: u64, to_hash: u64) -> Option<EtgPath> {
        if from_hash == to_hash {
            return Some(EtgPath {
                nodes: vec![from_hash],
                edges: vec![],
                total_reliability: 1.0,
                estimated_duration_ms: 0,
            });
        }

        // BFS with path tracking
        let mut queue: VecDeque<(u64, Vec<u64>, f32, u32)> = VecDeque::new();
        let mut visited: HashMap<u64, f32> = HashMap::new();

        queue.push_back((from_hash, vec![from_hash], 1.0, 0));
        visited.insert(from_hash, 1.0);

        let mut best_path: Option<(Vec<u64>, f32, u32)> = None;

        while let Some((current, path, reliability, duration)) = queue.pop_front() {
            if path.len() as u32 > MAX_BFS_DEPTH {
                continue;
            }

            let neighbors = match self.adjacency.get(&current) {
                Some(n) => n,
                None => continue,
            };

            for &next in neighbors {
                let edge = match self.edges.get(&(current, next)) {
                    Some(e) => e,
                    None => continue,
                };

                let edge_reliability = self.effective_reliability(edge);
                if edge_reliability < MIN_RELIABILITY {
                    continue; // skip unreliable edges
                }

                let path_reliability = reliability * edge_reliability;
                let path_duration = duration + edge.avg_duration_ms as u32;

                // Only visit if we haven't found a better path to this node
                if let Some(&prev_reliability) = visited.get(&next) {
                    if path_reliability <= prev_reliability {
                        continue;
                    }
                }

                visited.insert(next, path_reliability);

                let mut new_path = path.clone();
                new_path.push(next);

                if next == to_hash {
                    // Found target — keep if it's the best so far
                    let is_better = match &best_path {
                        None => true,
                        Some((_, best_rel, _)) => path_reliability > *best_rel,
                    };
                    if is_better {
                        best_path = Some((new_path, path_reliability, path_duration));
                    }
                } else {
                    queue.push_back((next, new_path, path_reliability, path_duration));
                }
            }
        }

        best_path.map(|(nodes, total_reliability, estimated_duration_ms)| {
            // Build edge ID list
            let edges = nodes
                .windows(2)
                .filter_map(|pair| {
                    self.edges
                        .get(&(pair[0], pair[1]))
                        .map(|e| e.from_node)
                })
                .collect();

            EtgPath {
                nodes,
                edges,
                total_reliability,
                estimated_duration_ms,
            }
        })
    }

    // ── Maintenance ─────────────────────────────────────────────────────────

    /// Prune edges with effective reliability below MIN_RELIABILITY.
    pub fn prune_stale_edges(&mut self) -> usize {
        let mut to_remove = Vec::new();

        for (key, edge) in &self.edges {
            if self.effective_reliability(edge) < MIN_RELIABILITY {
                to_remove.push(*key);
            }
        }

        let count = to_remove.len();
        for key in &to_remove {
            self.edges.remove(key);
            // Remove from adjacency
            if let Some(neighbors) = self.adjacency.get_mut(&key.0) {
                neighbors.retain(|n| *n != key.1);
            }
        }

        if count > 0 {
            debug!(pruned = count, "ETG: pruned stale edges");
        }
        count
    }

    /// Persist the current in-memory state to SQLite.
    pub fn flush_to_db(&self) -> Result<(), String> {
        let conn = match &self.db {
            Some(c) => c,
            None => return Ok(()), // no DB, nothing to flush
        };

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("transaction start failed: {e}"))?;

        // Upsert nodes
        for node in self.nodes.values() {
            let elements_json = serde_json::to_string(&node.interactive_elements)
                .unwrap_or_else(|_| "[]".to_string());
            tx.execute(
                "INSERT OR REPLACE INTO etg_nodes (id, package_name, activity_name, state_hash, interactive_elements, visit_count, last_visit_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    node.id as i64,
                    node.package_name,
                    node.activity_name,
                    node.state_hash as i64,
                    elements_json,
                    node.visit_count,
                    node.last_visit_ms as i64,
                ],
            ).map_err(|e| format!("node upsert failed: {e}"))?;
        }

        // Upsert edges
        for ((from, to), edge) in &self.edges {
            let action_json = serde_json::to_string(&edge.action)
                .unwrap_or_else(|_| "null".to_string());
            tx.execute(
                "INSERT OR REPLACE INTO etg_edges (from_node, to_node, action_json, success_count, fail_count, avg_duration_ms, last_used_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    *from as i64,
                    *to as i64,
                    action_json,
                    edge.success_count,
                    edge.fail_count,
                    edge.avg_duration_ms,
                    edge.last_used_ms as i64,
                ],
            ).map_err(|e| format!("edge upsert failed: {e}"))?;
        }

        tx.commit()
            .map_err(|e| format!("commit failed: {e}"))?;

        debug!(
            nodes = self.nodes.len(),
            edges = self.edges.len(),
            "ETG: flushed to SQLite"
        );
        Ok(())
    }

    // ── Internals ───────────────────────────────────────────────────────────

    /// Load all nodes and edges from SQLite into memory.
    fn load_from_db(&mut self) -> Result<(), String> {
        let conn = match &self.db {
            Some(c) => c,
            None => return Ok(()),
        };

        // Load nodes
        let mut stmt = conn
            .prepare("SELECT id, package_name, activity_name, state_hash, interactive_elements, visit_count, last_visit_ms FROM etg_nodes")
            .map_err(|e| format!("prepare nodes query failed: {e}"))?;

        let node_rows = stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let package_name: String = row.get(1)?;
                let activity_name: String = row.get(2)?;
                let state_hash: i64 = row.get(3)?;
                let elements_json: String = row.get(4)?;
                let visit_count: u32 = row.get(5)?;
                let last_visit_ms: i64 = row.get(6)?;

                let interactive_elements: Vec<String> =
                    serde_json::from_str(&elements_json).unwrap_or_default();

                Ok(EtgNode {
                    id: id as u64,
                    package_name,
                    activity_name,
                    state_hash: state_hash as u64,
                    interactive_elements,
                    visit_count,
                    last_visit_ms: last_visit_ms as u64,
                })
            })
            .map_err(|e| format!("node query failed: {e}"))?;

        let mut max_id = 0u64;
        for row in node_rows {
            let node = row.map_err(|e| format!("node row parse failed: {e}"))?;
            max_id = max_id.max(node.id);
            self.adjacency.entry(node.state_hash).or_default();
            self.nodes.insert(node.state_hash, node);
        }
        self.next_node_id = max_id + 1;

        // Load edges
        let mut stmt = conn
            .prepare("SELECT from_node, to_node, action_json, success_count, fail_count, avg_duration_ms, last_used_ms FROM etg_edges")
            .map_err(|e| format!("prepare edges query failed: {e}"))?;

        let edge_rows = stmt
            .query_map([], |row| {
                let from: i64 = row.get(0)?;
                let to: i64 = row.get(1)?;
                let action_json: String = row.get(2)?;
                let success_count: u32 = row.get(3)?;
                let fail_count: u32 = row.get(4)?;
                let avg_duration_ms: f32 = row.get::<_, f64>(5)? as f32;
                let last_used_ms: i64 = row.get(6)?;

                let action: ActionType =
                    serde_json::from_str(&action_json).unwrap_or(ActionType::Back);

                Ok(((from as u64, to as u64), EtgEdge {
                    from_node: from as u64,
                    to_node: to as u64,
                    action,
                    success_count,
                    fail_count,
                    avg_duration_ms,
                    m2_duration_ms: 0.0,
                    last_used_ms: last_used_ms as u64,
                }))
            })
            .map_err(|e| format!("edge query failed: {e}"))?;

        for row in edge_rows {
            let ((from, to), edge) = row.map_err(|e| format!("edge row parse failed: {e}"))?;
            self.edges.insert((from, to), edge);
            self.adjacency.entry(from).or_default().push(to);
        }

        debug!(
            nodes = self.nodes.len(),
            edges = self.edges.len(),
            "ETG: loaded from SQLite"
        );
        Ok(())
    }

    /// Evict the least-recently-visited node (and all its edges).
    fn evict_lru_node(&mut self) {
        let lru_hash = self
            .nodes
            .iter()
            .min_by_key(|(_, n)| n.last_visit_ms)
            .map(|(hash, _)| *hash);

        if let Some(hash) = lru_hash {
            // Remove all edges involving this node
            let edge_keys: Vec<(u64, u64)> = self
                .edges
                .keys()
                .filter(|(f, t)| *f == hash || *t == hash)
                .cloned()
                .collect();
            for key in &edge_keys {
                self.edges.remove(key);
            }

            // Remove from adjacency
            self.adjacency.remove(&hash);
            for neighbors in self.adjacency.values_mut() {
                neighbors.retain(|n| *n != hash);
            }

            self.nodes.remove(&hash);
            debug!(state_hash = hash, "ETG: evicted LRU node");
        }
    }

    /// Evict the N least-recently-used edges.
    fn evict_lru_edges(&mut self, n: usize) {
        let mut edges_by_age: Vec<((u64, u64), u64)> = self
            .edges
            .iter()
            .map(|(k, e)| (*k, e.last_used_ms))
            .collect();
        edges_by_age.sort_by_key(|(_, ms)| *ms);

        let to_remove: Vec<(u64, u64)> = edges_by_age
            .iter()
            .take(n)
            .map(|(k, _)| *k)
            .collect();

        for key in &to_remove {
            self.edges.remove(key);
            if let Some(neighbors) = self.adjacency.get_mut(&key.0) {
                neighbors.retain(|n| *n != key.1);
            }
        }

        if !to_remove.is_empty() {
            warn!(evicted = to_remove.len(), "ETG: evicted LRU edges");
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_in_memory() {
        let store = EtgStore::in_memory();
        assert_eq!(store.node_count(), 0);
        assert_eq!(store.edge_count(), 0);
    }

    #[test]
    fn test_get_or_create_node() {
        let mut store = EtgStore::in_memory();
        let id1 = store.get_or_create_node(0xAAAA, "com.test", ".Main", &["btn".into()]);
        assert_eq!(store.node_count(), 1);

        // Same hash should return same node
        let id2 = store.get_or_create_node(0xAAAA, "com.test", ".Main", &["btn".into()]);
        assert_eq!(id1, id2);
        assert_eq!(store.node_count(), 1);

        // Visit count should increment
        let node = store.get_node(0xAAAA).unwrap();
        assert_eq!(node.visit_count, 2);
    }

    #[test]
    fn test_record_transition() {
        let mut store = EtgStore::in_memory();
        store.get_or_create_node(0x1111, "com.test", ".A", &[]);
        store.get_or_create_node(0x2222, "com.test", ".B", &[]);

        store.record_transition(
            0x1111,
            0x2222,
            &ActionType::Tap { x: 100, y: 200 },
            true,
            150,
        );

        assert_eq!(store.edge_count(), 1);

        let edge = store.get_edge(0x1111, 0x2222).unwrap();
        assert_eq!(edge.success_count, 1);
        assert_eq!(edge.fail_count, 0);
        assert_eq!(edge.avg_duration_ms, 150.0_f32);
    }

    #[test]
    fn test_record_transition_update() {
        let mut store = EtgStore::in_memory();
        store.get_or_create_node(0x1111, "com.test", ".A", &[]);
        store.get_or_create_node(0x2222, "com.test", ".B", &[]);

        store.record_transition(0x1111, 0x2222, &ActionType::Back, true, 100);
        store.record_transition(0x1111, 0x2222, &ActionType::Back, true, 200);
        store.record_transition(0x1111, 0x2222, &ActionType::Back, false, 300);

        let edge = store.get_edge(0x1111, 0x2222).unwrap();
        assert_eq!(edge.success_count, 2);
        assert_eq!(edge.fail_count, 1);
        assert_eq!(store.edge_count(), 1); // still only one edge
    }

    #[test]
    fn test_bfs_direct_path() {
        let mut store = EtgStore::in_memory();
        store.get_or_create_node(0xA, "com.test", ".A", &[]);
        store.get_or_create_node(0xB, "com.test", ".B", &[]);

        store.record_transition(0xA, 0xB, &ActionType::Back, true, 100);
        // Record many successes to boost reliability
        for _ in 0..9 {
            store.record_transition(0xA, 0xB, &ActionType::Back, true, 100);
        }

        let path = store.find_path(0xA, 0xB);
        assert!(path.is_some(), "should find direct path");
        let p = path.unwrap();
        assert_eq!(p.nodes, vec![0xA, 0xB]);
    }

    #[test]
    fn test_bfs_multi_hop() {
        let mut store = EtgStore::in_memory();
        store.get_or_create_node(0xA, "com.test", ".A", &[]);
        store.get_or_create_node(0xB, "com.test", ".B", &[]);
        store.get_or_create_node(0xC, "com.test", ".C", &[]);

        // A -> B -> C with high reliability
        for _ in 0..10 {
            store.record_transition(0xA, 0xB, &ActionType::Back, true, 50);
            store.record_transition(0xB, 0xC, &ActionType::Back, true, 50);
        }

        let path = store.find_path(0xA, 0xC);
        assert!(path.is_some());
        let p = path.unwrap();
        assert_eq!(p.nodes, vec![0xA, 0xB, 0xC]);
    }

    #[test]
    fn test_bfs_no_path() {
        let mut store = EtgStore::in_memory();
        store.get_or_create_node(0xA, "com.test", ".A", &[]);
        store.get_or_create_node(0xB, "com.test", ".B", &[]);
        // No edge between them

        let path = store.find_path(0xA, 0xB);
        assert!(path.is_none());
    }

    #[test]
    fn test_same_node_path() {
        let store = EtgStore::in_memory();
        let path = store.find_path(0xA, 0xA);
        assert!(path.is_some());
        let p = path.unwrap();
        assert_eq!(p.nodes.len(), 1);
        assert!((p.total_reliability - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_effective_reliability_fresh() {
        let mut store = EtgStore::in_memory();
        store.set_current_time_ms(1_000_000);

        let edge = EtgEdge {
            from_node: 1,
            to_node: 2,
            action: ActionType::Back,
            success_count: 10,
            fail_count: 0,
            avg_duration_ms: 100.0,
            m2_duration_ms: 0.0,
            last_used_ms: 1_000_000, // just used now
        };

        let eff = store.effective_reliability(&edge);
        assert!(
            (eff - 1.0).abs() < 0.01,
            "fresh edge should have ~1.0 reliability, got {}",
            eff
        );
    }

    #[test]
    fn test_effective_reliability_decayed() {
        let mut store = EtgStore::in_memory();
        // Set current time to 14 days later
        let fourteen_days_ms = 14 * 24 * 60 * 60 * 1000u64;
        store.set_current_time_ms(fourteen_days_ms);

        let edge = EtgEdge {
            from_node: 1,
            to_node: 2,
            action: ActionType::Back,
            success_count: 10,
            fail_count: 0,
            avg_duration_ms: 100.0,
            m2_duration_ms: 0.0,
            last_used_ms: 0, // used 14 days ago
        };

        let eff = store.effective_reliability(&edge);
        // After 1 half-life, should be ~0.5
        assert!(
            (eff - 0.5).abs() < 0.05,
            "14-day-old edge should have ~0.5 reliability, got {}",
            eff
        );
    }

    #[test]
    fn test_prune_stale_edges() {
        let mut store = EtgStore::in_memory();
        let thirty_days_ms = 30 * 24 * 60 * 60 * 1000u64;
        store.set_current_time_ms(thirty_days_ms);

        store.get_or_create_node(0xA, "com.test", ".A", &[]);
        store.get_or_create_node(0xB, "com.test", ".B", &[]);

        // Edge with low reliability (many failures, old)
        let edge = EtgEdge {
            from_node: 1,
            to_node: 2,
            action: ActionType::Back,
            success_count: 1,
            fail_count: 9,
            avg_duration_ms: 100.0,
            m2_duration_ms: 0.0,
            last_used_ms: 0, // 30 days ago
        };
        store.edges.insert((0xA, 0xB), edge);
        store.adjacency.entry(0xA).or_default().push(0xB);

        assert_eq!(store.edge_count(), 1);
        let pruned = store.prune_stale_edges();
        assert_eq!(pruned, 1);
        assert_eq!(store.edge_count(), 0);
    }

    #[test]
    fn test_outgoing_edges() {
        let mut store = EtgStore::in_memory();
        store.get_or_create_node(0xA, "com.test", ".A", &[]);
        store.get_or_create_node(0xB, "com.test", ".B", &[]);
        store.get_or_create_node(0xC, "com.test", ".C", &[]);

        store.record_transition(0xA, 0xB, &ActionType::Back, true, 100);
        store.record_transition(0xA, 0xC, &ActionType::Home, true, 100);

        let outgoing = store.outgoing_edges(0xA);
        assert_eq!(outgoing.len(), 2);
    }

    #[test]
    fn test_sqlite_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test_etg.db");
        let db_path_str = db_path.to_str().unwrap();

        // Create and populate
        {
            let mut store = EtgStore::with_sqlite(db_path_str).unwrap();
            store.get_or_create_node(0xAAAA, "com.test", ".Main", &["button".into()]);
            store.get_or_create_node(0xBBBB, "com.test", ".Settings", &[]);
            store.record_transition(0xAAAA, 0xBBBB, &ActionType::Back, true, 200);
            store.flush_to_db().unwrap();
        }

        // Reload and verify
        {
            let store = EtgStore::with_sqlite(db_path_str).unwrap();
            assert_eq!(store.node_count(), 2);
            assert_eq!(store.edge_count(), 1);
            let node = store.get_node(0xAAAA).unwrap();
            assert_eq!(node.package_name, "com.test");
        }
    }

    #[test]
    fn test_node_eviction() {
        let mut store = EtgStore::in_memory();

        // Fill to capacity + 1
        for i in 0..=MAX_NODES {
            store.set_current_time_ms(i as u64 * 1000);
            store.get_or_create_node(i as u64, "com.test", ".Main", &[]);
        }

        // Should have evicted the oldest
        assert!(store.node_count() <= MAX_NODES);
    }
}
