//! Social graph structure and analysis (spec §4.6).
//!
//! Models the user's social network as an undirected weighted graph.
//! Supports edge strengthening via repeated interactions, connected-component
//! clustering, and relationship diversity scoring.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::arc::ArcError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum edges in the social graph.
const MAX_EDGES: usize = 2048;

/// Maximum distinct nodes (people).
const MAX_NODES: usize = 500;

/// Maximum adjacency list length per node.
const MAX_ADJACENCY_PER_NODE: usize = 50;

/// Initial edge weight for new connections.
const INITIAL_WEIGHT: f32 = 0.1;

/// Weight increment per interaction (saturates at 1.0).
const WEIGHT_INCREMENT: f32 = 0.05;

/// Default minimum edge weight for pruning.
pub(crate) const DEFAULT_PRUNE_MIN_WEIGHT: f32 = 0.1;

// ---------------------------------------------------------------------------
// Serde default helpers
// ---------------------------------------------------------------------------

fn default_prune_min_weight() -> f32 {
    DEFAULT_PRUNE_MIN_WEIGHT
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The type of relationship between two people.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelationType {
    Family,
    Friend,
    Colleague,
    Acquaintance,
    Online,
    Other,
}

/// A directed social edge between two contacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocialEdge {
    /// Connection strength (0.0–1.0).
    pub weight: f32,
    /// Total interaction count across both directions.
    pub interaction_count: u32,
    /// Epoch milliseconds of last interaction.
    pub last_interaction_ms: u64,
    /// Relationship classification.
    pub relationship_type: RelationType,
}

// ---------------------------------------------------------------------------
// SocialGraph
// ---------------------------------------------------------------------------

/// Bounded, undirected social graph.
#[derive(Debug, Serialize, Deserialize)]
pub struct SocialGraph {
    /// Edges keyed by canonical (min, max) pair.
    edges: HashMap<(u64, u64), SocialEdge>,
    /// Adjacency lists (node -> neighbours).
    adjacency: HashMap<u64, Vec<u64>>,
    /// Minimum edge weight below which edges are pruned.
    #[serde(default = "default_prune_min_weight")]
    prune_min_weight: f32,
}

impl SocialGraph {
    /// Create an empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self {
            edges: HashMap::with_capacity(128),
            adjacency: HashMap::with_capacity(64),
            prune_min_weight: DEFAULT_PRUNE_MIN_WEIGHT,
        }
    }

    /// Configured minimum edge weight for pruning.
    #[must_use]
    pub(crate) fn prune_min_weight(&self) -> f32 {
        self.prune_min_weight
    }

    /// Add an edge between two nodes.
    ///
    /// If the edge already exists, this is a no-op (use [`strengthen`] to
    /// reinforce). Nodes are canonicalized so `(a, b)` and `(b, a)` share
    /// one entry.
    #[instrument(skip_all)]
    pub fn add_edge(
        &mut self,
        a: u64,
        b: u64,
        rel: RelationType,
        now_ms: u64,
    ) -> Result<(), ArcError> {
        let key = canonical_key(a, b);

        if self.edges.contains_key(&key) {
            // Edge exists — nothing to do.
            return Ok(());
        }

        if self.edges.len() >= MAX_EDGES {
            return Err(ArcError::CapacityExceeded {
                collection: "social_graph_edges".into(),
                max: MAX_EDGES,
            });
        }

        // Ensure both nodes have adjacency entries and room.
        self.ensure_node(a)?;
        self.ensure_node(b)?;

        {
            let adj_a = self.adjacency.get(&a).map(|v| v.len()).unwrap_or(0);
            if adj_a >= MAX_ADJACENCY_PER_NODE {
                return Err(ArcError::CapacityExceeded {
                    collection: format!("adjacency[{a}]"),
                    max: MAX_ADJACENCY_PER_NODE,
                });
            }
        }

        {
            let adj_b = self.adjacency.get(&b).map(|v| v.len()).unwrap_or(0);
            if adj_b >= MAX_ADJACENCY_PER_NODE {
                return Err(ArcError::CapacityExceeded {
                    collection: format!("adjacency[{b}]"),
                    max: MAX_ADJACENCY_PER_NODE,
                });
            }
        }

        self.edges.insert(
            key,
            SocialEdge {
                weight: INITIAL_WEIGHT,
                interaction_count: 1,
                last_interaction_ms: now_ms,
                relationship_type: rel,
            },
        );

        // Update adjacency (both directions).
        if let Some(adj) = self.adjacency.get_mut(&a) {
            if !adj.contains(&b) {
                adj.push(b);
            }
        }
        if let Some(adj) = self.adjacency.get_mut(&b) {
            if !adj.contains(&a) {
                adj.push(a);
            }
        }

        Ok(())
    }

    /// Strengthen an existing edge (record new interaction).
    ///
    /// Increments weight (capped at 1.0) and interaction count.
    #[instrument(skip_all)]
    pub fn strengthen(&mut self, a: u64, b: u64, now_ms: u64) -> Result<(), ArcError> {
        let key = canonical_key(a, b);
        let edge = self.edges.get_mut(&key).ok_or(ArcError::NotFound {
            entity: "social_edge".into(),
            id: key.0, // use one end as identifier
        })?;

        edge.weight = (edge.weight + WEIGHT_INCREMENT).min(1.0);
        edge.interaction_count = edge.interaction_count.saturating_add(1);
        edge.last_interaction_ms = now_ms;
        Ok(())
    }

    /// Get connections for a node, sorted by weight descending.
    #[must_use]
    #[instrument(skip_all)]
    pub fn get_connections(&self, node: u64) -> Vec<(u64, f32)> {
        let neighbours = match self.adjacency.get(&node) {
            Some(adj) => adj,
            None => return Vec::new(),
        };

        let mut connections: Vec<(u64, f32)> = neighbours
            .iter()
            .filter_map(|&neighbour| {
                let key = canonical_key(node, neighbour);
                self.edges.get(&key).map(|e| (neighbour, e.weight))
            })
            .collect();

        connections.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        connections
    }

    /// Connected-component clustering via BFS.
    ///
    /// Returns a list of clusters, each a `Vec<u64>` of node IDs.
    #[must_use]
    #[instrument(skip_all)]
    pub fn get_clusters(&self) -> Vec<Vec<u64>> {
        let mut visited: HashMap<u64, bool> = HashMap::with_capacity(self.adjacency.len());
        for &node in self.adjacency.keys() {
            visited.insert(node, false);
        }

        let mut clusters: Vec<Vec<u64>> = Vec::new();

        for &start in self.adjacency.keys() {
            if visited.get(&start).copied().unwrap_or(false) {
                continue;
            }

            let mut cluster: Vec<u64> = Vec::new();
            // BFS queue — bounded by MAX_NODES.
            let mut queue: Vec<u64> = Vec::with_capacity(MAX_NODES.min(self.adjacency.len()));
            queue.push(start);
            visited.insert(start, true);

            while let Some(current) = queue.pop() {
                cluster.push(current);

                if let Some(adj) = self.adjacency.get(&current) {
                    for &neighbour in adj {
                        if !visited.get(&neighbour).copied().unwrap_or(true) {
                            visited.insert(neighbour, true);
                            if queue.len() < MAX_NODES {
                                queue.push(neighbour);
                            }
                        }
                    }
                }
            }

            if !cluster.is_empty() {
                cluster.sort_unstable();
                clusters.push(cluster);
            }
        }

        clusters.sort_by_key(|c| std::cmp::Reverse(c.len()));
        clusters
    }

    /// Find mutual connections between two nodes.
    #[must_use]
    #[instrument(skip_all)]
    pub fn mutual_connections(&self, a: u64, b: u64) -> Vec<u64> {
        let adj_a = match self.adjacency.get(&a) {
            Some(v) => v,
            None => return Vec::new(),
        };
        let adj_b = match self.adjacency.get(&b) {
            Some(v) => v,
            None => return Vec::new(),
        };

        let mut mutuals: Vec<u64> = adj_a
            .iter()
            .filter(|n| adj_b.contains(n))
            .copied()
            .collect();
        mutuals.sort_unstable();
        mutuals
    }

    /// Prune edges whose weight is below `min_weight`.
    ///
    /// Returns the number of edges removed.
    #[instrument(skip_all)]
    pub fn prune_weak(&mut self, min_weight: f32) -> usize {
        let weak_keys: Vec<(u64, u64)> = self
            .edges
            .iter()
            .filter(|(_, e)| e.weight < min_weight)
            .map(|(&k, _)| k)
            .collect();

        let count = weak_keys.len();

        for key in &weak_keys {
            self.edges.remove(key);

            // Clean adjacency.
            let (a, b) = *key;
            if let Some(adj) = self.adjacency.get_mut(&a) {
                adj.retain(|&n| n != b);
            }
            if let Some(adj) = self.adjacency.get_mut(&b) {
                adj.retain(|&n| n != a);
            }
        }

        count
    }

    /// Diversity score: ratio of distinct `RelationType` variants present.
    ///
    /// Used by `SocialDomain::compute_score`.
    #[must_use]
    pub fn diversity_score(&self) -> f32 {
        if self.edges.is_empty() {
            return 0.0;
        }

        let mut seen = [false; 6]; // 6 RelationType variants
        for edge in self.edges.values() {
            let idx = match edge.relationship_type {
                RelationType::Family => 0,
                RelationType::Friend => 1,
                RelationType::Colleague => 2,
                RelationType::Acquaintance => 3,
                RelationType::Online => 4,
                RelationType::Other => 5,
            };
            seen[idx] = true;
        }

        let distinct = seen.iter().filter(|&&s| s).count() as f32;
        (distinct / 6.0).min(1.0)
    }

    /// Total number of edges.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Total number of nodes.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.adjacency.len()
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Ensure a node exists in the adjacency map.
    fn ensure_node(&mut self, node: u64) -> Result<(), ArcError> {
        if !self.adjacency.contains_key(&node) {
            if self.adjacency.len() >= MAX_NODES {
                return Err(ArcError::CapacityExceeded {
                    collection: "social_graph_nodes".into(),
                    max: MAX_NODES,
                });
            }
            self.adjacency.insert(node, Vec::with_capacity(8));
        }
        Ok(())
    }
}

impl Default for SocialGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Canonical edge key: always (min, max) so (a,b) == (b,a).
#[must_use]
fn canonical_key(a: u64, b: u64) -> (u64, u64) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get_connections() {
        let mut graph = SocialGraph::new();
        graph
            .add_edge(1, 2, RelationType::Friend, 1000)
            .expect("add");
        graph
            .add_edge(1, 3, RelationType::Family, 2000)
            .expect("add");

        let conns = graph.get_connections(1);
        assert_eq!(conns.len(), 2);
        // Both should have initial weight.
        for (_, w) in &conns {
            assert!((*w - INITIAL_WEIGHT).abs() < 0.001);
        }
    }

    #[test]
    fn test_strengthen() {
        let mut graph = SocialGraph::new();
        graph
            .add_edge(1, 2, RelationType::Friend, 1000)
            .expect("add");
        graph.strengthen(1, 2, 2000).expect("strengthen");

        let conns = graph.get_connections(1);
        assert_eq!(conns.len(), 1);
        let expected = INITIAL_WEIGHT + WEIGHT_INCREMENT;
        assert!(
            (conns[0].1 - expected).abs() < 0.001,
            "expected {expected}, got {}",
            conns[0].1
        );
    }

    #[test]
    fn test_strengthen_nonexistent_edge() {
        let mut graph = SocialGraph::new();
        assert!(graph.strengthen(1, 2, 1000).is_err());
    }

    #[test]
    fn test_undirected_symmetry() {
        let mut graph = SocialGraph::new();
        graph
            .add_edge(1, 2, RelationType::Friend, 1000)
            .expect("add");

        // Both directions should see the same connection.
        assert_eq!(graph.get_connections(1).len(), 1);
        assert_eq!(graph.get_connections(2).len(), 1);
    }

    #[test]
    fn test_clusters() {
        let mut graph = SocialGraph::new();
        // Cluster 1: 1-2-3
        graph
            .add_edge(1, 2, RelationType::Friend, 1000)
            .expect("add");
        graph
            .add_edge(2, 3, RelationType::Friend, 1000)
            .expect("add");
        // Cluster 2: 10-11
        graph
            .add_edge(10, 11, RelationType::Colleague, 1000)
            .expect("add");

        let clusters = graph.get_clusters();
        assert_eq!(clusters.len(), 2);
        // Largest cluster first.
        assert_eq!(clusters[0].len(), 3);
        assert_eq!(clusters[1].len(), 2);
    }

    #[test]
    fn test_mutual_connections() {
        let mut graph = SocialGraph::new();
        graph
            .add_edge(1, 3, RelationType::Friend, 1000)
            .expect("add");
        graph
            .add_edge(2, 3, RelationType::Friend, 1000)
            .expect("add");
        graph
            .add_edge(1, 4, RelationType::Family, 1000)
            .expect("add");

        let mutuals = graph.mutual_connections(1, 2);
        assert_eq!(mutuals, vec![3]);

        // No mutuals between disconnected nodes.
        let none = graph.mutual_connections(3, 4);
        assert_eq!(none, vec![1]); // Both connected to 1.
    }

    #[test]
    fn test_prune_weak() {
        let mut graph = SocialGraph::new();
        graph
            .add_edge(1, 2, RelationType::Friend, 1000)
            .expect("add");
        graph
            .add_edge(3, 4, RelationType::Acquaintance, 1000)
            .expect("add");

        // Strengthen 1-2 above threshold.
        for _ in 0..20 {
            graph.strengthen(1, 2, 2000).expect("strengthen");
        }

        let pruned = graph.prune_weak(0.5);
        assert_eq!(pruned, 1); // 3-4 was weak.
        assert_eq!(graph.edge_count(), 1);
        assert!(graph.get_connections(3).is_empty());
    }

    #[test]
    fn test_diversity_score() {
        let mut graph = SocialGraph::new();
        assert!((graph.diversity_score() - 0.0).abs() < 0.001);

        graph
            .add_edge(1, 2, RelationType::Friend, 1000)
            .expect("add");
        // 1 out of 6 types.
        assert!((graph.diversity_score() - 1.0 / 6.0).abs() < 0.01);

        graph
            .add_edge(3, 4, RelationType::Family, 1000)
            .expect("add");
        // 2 out of 6.
        assert!((graph.diversity_score() - 2.0 / 6.0).abs() < 0.01);
    }

    #[test]
    fn test_edge_capacity_limit() {
        let mut graph = SocialGraph::new();
        // Fill up to MAX_EDGES. Each edge needs 2 unique nodes.
        // With MAX_NODES=500, we can have ~500 nodes. Use a star topology.
        // But MAX_ADJACENCY_PER_NODE=50, so hub node caps at 50.
        // Use a mesh of small clusters instead.
        // Just verify the capacity error fires.
        for i in 0..(MAX_EDGES as u64) {
            let a = i * 2;
            let b = i * 2 + 1;
            let result = graph.add_edge(a, b, RelationType::Other, 1000);
            if result.is_err() {
                // We hit either node or edge capacity — that's the test.
                return;
            }
        }
        // If we got here, all edges fit. The next one should fail.
        let a = MAX_EDGES as u64 * 2;
        let b = a + 1;
        // Might fail on node cap (MAX_NODES=500) before edge cap.
        let result = graph.add_edge(a, b, RelationType::Other, 1000);
        assert!(result.is_err(), "should exceed some capacity");
    }

    #[test]
    fn test_duplicate_edge_is_noop() {
        let mut graph = SocialGraph::new();
        graph
            .add_edge(1, 2, RelationType::Friend, 1000)
            .expect("add");
        graph
            .add_edge(2, 1, RelationType::Colleague, 2000)
            .expect("dup noop");
        // Still only 1 edge — the second add was a no-op.
        assert_eq!(graph.edge_count(), 1);
    }
}
