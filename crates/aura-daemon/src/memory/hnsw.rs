//! Pure-Rust HNSW (Hierarchical Navigable Small World) approximate nearest
//! neighbor index for fast embedding similarity search.
//!
//! Parameters: M=16, ef_construction=200, ef_search=50.
//! Distance metric: cosine distance (1 - cosine_similarity).

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use aura_types::errors::{AuraError, MemError};
use tracing::{info, instrument, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default max connections per node per layer.
const DEFAULT_M: usize = 16;
/// Max connections at layer 0 (2 * M).
#[allow(dead_code)] // Phase 8: used by HNSW layer-0 topology configuration
const DEFAULT_M_MAX0: usize = 32;
/// Candidates to consider during construction.
const DEFAULT_EF_CONSTRUCTION: usize = 200;
/// Level generation parameter: 1 / ln(M).
const ML: f64 = 1.0 / 2.772_588_722_239_781; // 1/ln(16)

type NodeId = u32;

// ---------------------------------------------------------------------------
// Heap helpers — we need both min-heap and max-heap by distance
// ---------------------------------------------------------------------------

/// Entry in a max-heap ordered by distance (largest distance at top).
#[derive(Clone)]
struct MaxEntry {
    id: NodeId,
    distance: f32,
}

impl PartialEq for MaxEntry {
    fn eq(&self, other: &Self) -> bool {
        self.distance.to_bits() == other.distance.to_bits()
    }
}

impl Eq for MaxEntry {}

impl PartialOrd for MaxEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MaxEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Max-heap: larger distance comes first
        self.distance
            .partial_cmp(&other.distance)
            .unwrap_or(Ordering::Equal)
    }
}

/// Entry in a min-heap ordered by distance (smallest distance at top).
#[derive(Clone)]
struct MinEntry {
    id: NodeId,
    distance: f32,
}

impl PartialEq for MinEntry {
    fn eq(&self, other: &Self) -> bool {
        self.distance.to_bits() == other.distance.to_bits()
    }
}

impl Eq for MinEntry {}

impl PartialOrd for MinEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MinEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Min-heap: reverse ordering so smallest distance comes first
        other
            .distance
            .partial_cmp(&self.distance)
            .unwrap_or(Ordering::Equal)
    }
}

// ---------------------------------------------------------------------------
// HnswNode
// ---------------------------------------------------------------------------

/// A single node in the HNSW graph.
struct HnswNode {
    /// The embedding vector.
    embedding: Vec<f32>,
    /// Connections per level: connections[level] = vec of neighbor NodeIds.
    connections: Vec<Vec<NodeId>>,
}

// ---------------------------------------------------------------------------
// HnswIndex
// ---------------------------------------------------------------------------

/// Pure-Rust HNSW approximate nearest neighbor index.
///
/// Supports insert, search, lazy deletion (tombstones), and binary
/// serialization for persistence.
pub struct HnswIndex {
    nodes: Vec<HnswNode>,
    entry_point: Option<NodeId>,
    max_level: usize,
    dim: usize,
    m: usize,
    m_max0: usize,
    ef_construction: usize,
    deleted: Vec<bool>,
    count: usize,
    /// Simple LCG state for deterministic level generation.
    rng_state: u64,
    /// Reusable visited buffer — avoids O(n) allocation per search.
    /// Each entry stores the generation when it was last visited.
    /// PERF-MED-2: generation counter replaces `vec![false; n]` per search.
    visited: Vec<u64>,
    visited_gen: u64,
}

impl HnswIndex {
    /// Create a new empty index with default parameters (M=16, ef_construction=200).
    #[instrument(skip_all)]
    pub fn new(dim: usize) -> Self {
        Self::with_params(dim, DEFAULT_M, DEFAULT_EF_CONSTRUCTION)
    }

    /// Create a new empty index with custom parameters.
    pub fn with_params(dim: usize, m: usize, ef_construction: usize) -> Self {
        Self {
            nodes: Vec::new(),
            entry_point: None,
            max_level: 0,
            dim,
            m,
            m_max0: m * 2,
            ef_construction,
            deleted: Vec::new(),
            count: 0,
            rng_state: 42,
            visited: Vec::new(),
            visited_gen: 0,
        }
    }

    /// Insert a new embedding into the index. Returns its NodeId.
    #[instrument(skip(self, embedding))]
    pub fn insert(&mut self, embedding: &[f32]) -> Result<NodeId, AuraError> {
        if embedding.len() != self.dim {
            return Err(AuraError::Memory(MemError::EmbeddingFailed));
        }

        let new_id = self.nodes.len() as NodeId;
        let level = self.random_level();

        // Create the node with empty connection lists for each level.
        let node = HnswNode {
            embedding: embedding.to_vec(),
            connections: (0..=level).map(|_| Vec::new()).collect(),
        };
        self.nodes.push(node);
        self.deleted.push(false);
        self.count += 1;

        // First node — just set as entry point.
        if self.entry_point.is_none() {
            self.entry_point = Some(new_id);
            self.max_level = level;
            return Ok(new_id);
        }

        let ep = self.entry_point.unwrap_or(0);
        let mut current_ep = ep;

        // Phase 1: Greedily descend from top level to level+1.
        let top = self.max_level;
        if top > level {
            for lc in (level + 1..=top).rev() {
                current_ep = self.greedy_closest(embedding, current_ep, lc);
            }
        }

        // Phase 2: At each level from min(level, max_level) down to 0,
        // search and connect.
        let insert_top = level.min(self.max_level);
        for lc in (0..=insert_top).rev() {
            let neighbors = self.search_layer(embedding, current_ep, self.ef_construction, lc);

            let max_conn = if lc == 0 { self.m_max0 } else { self.m };
            let selected = self.select_neighbors(&neighbors, max_conn);

            // Connect new node to selected neighbors.
            self.nodes[new_id as usize].connections[lc] = selected.clone();

            // Connect selected neighbors back to new node, pruning if needed.
            for &neighbor_id in &selected {
                let n_max = if lc == 0 { self.m_max0 } else { self.m };
                let needs_prune = {
                    let conns = &mut self.nodes[neighbor_id as usize].connections;
                    if lc < conns.len() {
                        conns[lc].push(new_id);
                        conns[lc].len() > n_max
                    } else {
                        false
                    }
                };
                if needs_prune {
                    // Clone what we need while self.nodes is only immutably borrowed.
                    let n_emb = self.nodes[neighbor_id as usize].embedding.clone();
                    let conn_ids: Vec<NodeId> =
                        self.nodes[neighbor_id as usize].connections[lc].clone();
                    let mut scored: Vec<(NodeId, f32)> = conn_ids
                        .iter()
                        .map(|&cid| {
                            (
                                cid,
                                cosine_distance(&n_emb, &self.nodes[cid as usize].embedding),
                            )
                        })
                        .collect();
                    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
                    scored.truncate(n_max);
                    self.nodes[neighbor_id as usize].connections[lc] =
                        scored.into_iter().map(|(id, _)| id).collect();
                }
            }

            // Use the closest result as the entry point for the next level down.
            if let Some(first) = neighbors.first() {
                current_ep = first.0;
            }
        }

        // If new level is higher than max_level, update entry point.
        if level > self.max_level {
            self.max_level = level;
            self.entry_point = Some(new_id);
        }

        Ok(new_id)
    }

    /// Search for the k nearest neighbors to `query`. Returns (NodeId, similarity)
    /// pairs sorted by similarity descending (highest similarity first).
    #[instrument(skip(self, query))]
    pub fn search(
        &mut self,
        query: &[f32],
        k: usize,
        ef: usize,
    ) -> Result<Vec<(NodeId, f32)>, AuraError> {
        if query.len() != self.dim {
            return Err(AuraError::Memory(MemError::EmbeddingFailed));
        }
        let ep = match self.entry_point {
            Some(ep) => ep,
            None => return Ok(Vec::new()),
        };

        let mut current_ep = ep;

        // Greedily descend from top level to level 1.
        if self.max_level > 0 {
            for lc in (1..=self.max_level).rev() {
                current_ep = self.greedy_closest(query, current_ep, lc);
            }
        }

        // Search at layer 0 with ef candidates.
        let ef_actual = ef.max(k);
        let candidates = self.search_layer(query, current_ep, ef_actual, 0);

        // Convert distance to similarity, filter deleted, take top k.
        let mut results: Vec<(NodeId, f32)> = candidates
            .into_iter()
            .filter(|&(id, _)| !self.is_deleted(id))
            .map(|(id, dist)| (id, 1.0 - dist)) // distance → similarity
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
        results.truncate(k);
        Ok(results)
    }

    /// Lazily delete a node (tombstone). It will be skipped in search results.
    #[instrument(skip(self))]
    pub fn delete(&mut self, id: NodeId) -> Result<(), AuraError> {
        let idx = id as usize;
        if idx >= self.deleted.len() {
            return Err(AuraError::Memory(MemError::NotFound(format!(
                "hnsw node {id}"
            ))));
        }
        if !self.deleted[idx] {
            self.deleted[idx] = true;
            self.count = self.count.saturating_sub(1);
        }
        Ok(())
    }

    /// Number of non-deleted nodes in the index.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether the index contains no non-deleted nodes.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Get the embedding for a node by ID.
    pub fn get_embedding(&self, id: NodeId) -> Option<&[f32]> {
        let idx = id as usize;
        if idx < self.nodes.len() && !self.deleted[idx] {
            Some(&self.nodes[idx].embedding)
        } else {
            None
        }
    }

    /// Total number of nodes including tombstoned/deleted ones.
    /// Use this to assess tombstone bloat: `total_including_deleted() - len()`.
    pub fn total_including_deleted(&self) -> usize {
        self.nodes.len()
    }

    /// Compact the index by rebuilding without tombstoned entries.
    ///
    /// Returns a mapping from old NodeId → new NodeId for the caller to update
    /// any external references (e.g. episodic/semantic ID maps).
    ///
    /// Should be called during thermal-aware consolidation (charging, screen off).
    /// See: AURA-V4-BATCH7-MEMORY-INFERENCE-AUDIT §5.1 #3.
    #[instrument(skip(self))]
    pub fn compact(&mut self) -> Vec<(NodeId, NodeId)> {
        // Collect live embeddings with their old IDs.
        let live: Vec<(NodeId, Vec<f32>)> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, _)| !self.deleted[*i])
            .map(|(i, node)| (i as NodeId, node.embedding.clone()))
            .collect();

        let old_total = self.nodes.len();
        let live_count = live.len();

        if live_count == old_total {
            // No tombstones — nothing to compact.
            return Vec::new();
        }

        // Build a fresh index with the same parameters.
        let mut fresh = Self::with_params(self.dim, self.m, self.ef_construction);

        let mut id_map = Vec::with_capacity(live_count);
        for (old_id, embedding) in &live {
            match fresh.insert(embedding) {
                Ok(new_id) => {
                    id_map.push((*old_id, new_id));
                }
                Err(e) => {
                    warn!(old_id, ?e, "compact: failed to re-insert node, skipping");
                }
            }
        }

        info!(
            old_total,
            live_count,
            tombstones_removed = old_total - live_count,
            "HNSW compaction complete"
        );

        // Replace self with the fresh index.
        self.nodes = fresh.nodes;
        self.entry_point = fresh.entry_point;
        self.max_level = fresh.max_level;
        self.deleted = fresh.deleted;
        self.count = fresh.count;
        self.rng_state = fresh.rng_state;
        self.visited = Vec::new();
        self.visited_gen = 0;

        id_map
    }

    /// Serialize the index to bytes for persistence.
    #[instrument(skip(self))]
    pub fn to_bytes(&self) -> Result<Vec<u8>, AuraError> {
        let mut buf = Vec::new();

        // Header: dim, m, m_max0, ef_construction, max_level, entry_point, node_count, rng_state
        buf.extend_from_slice(&(self.dim as u32).to_le_bytes());
        buf.extend_from_slice(&(self.m as u32).to_le_bytes());
        buf.extend_from_slice(&(self.m_max0 as u32).to_le_bytes());
        buf.extend_from_slice(&(self.ef_construction as u32).to_le_bytes());
        buf.extend_from_slice(&(self.max_level as u32).to_le_bytes());
        buf.extend_from_slice(&(self.entry_point.unwrap_or(u32::MAX)).to_le_bytes());
        buf.extend_from_slice(&(self.nodes.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.rng_state.to_le_bytes());

        // Deleted flags.
        for &d in &self.deleted {
            buf.push(if d { 1 } else { 0 });
        }

        // Each node: num_levels, then for each level (num_connections, connection ids),
        // then embedding.
        for node in &self.nodes {
            let num_levels = node.connections.len() as u32;
            buf.extend_from_slice(&num_levels.to_le_bytes());
            for level_conns in &node.connections {
                let num_conns = level_conns.len() as u32;
                buf.extend_from_slice(&num_conns.to_le_bytes());
                for &conn_id in level_conns {
                    buf.extend_from_slice(&conn_id.to_le_bytes());
                }
            }
            for &val in &node.embedding {
                buf.extend_from_slice(&val.to_le_bytes());
            }
        }

        Ok(buf)
    }

    /// Deserialize an index from bytes.
    #[instrument(skip(data))]
    pub fn from_bytes(data: &[u8]) -> Result<Self, AuraError> {
        let err = || AuraError::Memory(MemError::SerializationFailed("hnsw index".into()));
        let mut pos = 0;

        let read_u32 = |pos: &mut usize| -> Result<u32, AuraError> {
            if *pos + 4 > data.len() {
                return Err(err());
            }
            let val =
                u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]]);
            *pos += 4;
            Ok(val)
        };

        let read_u64 = |pos: &mut usize| -> Result<u64, AuraError> {
            if *pos + 8 > data.len() {
                return Err(err());
            }
            let val = u64::from_le_bytes([
                data[*pos],
                data[*pos + 1],
                data[*pos + 2],
                data[*pos + 3],
                data[*pos + 4],
                data[*pos + 5],
                data[*pos + 6],
                data[*pos + 7],
            ]);
            *pos += 8;
            Ok(val)
        };

        let read_f32 = |pos: &mut usize| -> Result<f32, AuraError> {
            if *pos + 4 > data.len() {
                return Err(err());
            }
            let val =
                f32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]]);
            *pos += 4;
            Ok(val)
        };

        // Header.
        let dim = read_u32(&mut pos)? as usize;
        let m = read_u32(&mut pos)? as usize;
        let m_max0 = read_u32(&mut pos)? as usize;
        let ef_construction = read_u32(&mut pos)? as usize;
        let max_level = read_u32(&mut pos)? as usize;
        let ep_raw = read_u32(&mut pos)?;
        let entry_point = if ep_raw == u32::MAX {
            None
        } else {
            Some(ep_raw)
        };
        let node_count = read_u32(&mut pos)? as usize;
        let rng_state = read_u64(&mut pos)?;

        // Deleted flags.
        if pos + node_count > data.len() {
            return Err(err());
        }
        let deleted: Vec<bool> = data[pos..pos + node_count]
            .iter()
            .map(|&b| b != 0)
            .collect();
        pos += node_count;

        let count = deleted.iter().filter(|&&d| !d).count();

        // Nodes.
        let mut nodes = Vec::with_capacity(node_count);
        for _ in 0..node_count {
            let num_levels = read_u32(&mut pos)? as usize;
            let mut connections = Vec::with_capacity(num_levels);
            for _ in 0..num_levels {
                let num_conns = read_u32(&mut pos)? as usize;
                let mut conns = Vec::with_capacity(num_conns);
                for _ in 0..num_conns {
                    conns.push(read_u32(&mut pos)?);
                }
                connections.push(conns);
            }
            let mut embedding = Vec::with_capacity(dim);
            for _ in 0..dim {
                embedding.push(read_f32(&mut pos)?);
            }
            nodes.push(HnswNode {
                embedding,
                connections,
            });
        }

        Ok(Self {
            nodes,
            entry_point,
            max_level,
            dim,
            m,
            m_max0,
            ef_construction,
            deleted,
            count,
            rng_state,
            visited: Vec::new(),
            visited_gen: 0,
        })
    }

    // -----------------------------------------------------------------------
    // Internal methods
    // -----------------------------------------------------------------------

    /// Generate a random level for a new node using an LCG.
    fn random_level(&mut self) -> usize {
        // LCG: state = state * 6364136223846793005 + 1442695040888963407
        self.rng_state = self
            .rng_state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);

        // Map to [0, 1) and apply HNSW level formula.
        let uniform = (self.rng_state >> 33) as f64 / (1u64 << 31) as f64;
        // Avoid log(0).
        let clamped = uniform.max(1e-10);
        let level = (-clamped.ln() * ML).floor() as usize;
        // Cap to avoid degenerate graphs.
        level.min(16)
    }

    /// Greedy descent: find the single closest node to `query` at the given level.
    fn greedy_closest(&self, query: &[f32], mut current: NodeId, level: usize) -> NodeId {
        let mut best_dist = cosine_distance(query, &self.nodes[current as usize].embedding);

        loop {
            let mut changed = false;
            let conns = &self.nodes[current as usize].connections;
            if level < conns.len() {
                for &neighbor in &conns[level] {
                    if (neighbor as usize) < self.nodes.len() {
                        let d = cosine_distance(query, &self.nodes[neighbor as usize].embedding);
                        if d < best_dist {
                            best_dist = d;
                            current = neighbor;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        current
    }

    /// Standard HNSW search_layer: returns up to `ef` closest nodes at the given
    /// level, as (NodeId, distance) sorted by distance ascending.
    ///
    /// PERF-MED-2: Uses a generation counter instead of allocating a fresh
    /// `vec![false; n]` on every call. The `self.visited` buffer is reused
    /// across searches — only `self.visited_gen` is incremented (O(1) reset).
    fn search_layer(
        &mut self,
        query: &[f32],
        entry: NodeId,
        ef: usize,
        level: usize,
    ) -> Vec<(NodeId, f32)> {
        let ep_dist = cosine_distance(query, &self.nodes[entry as usize].embedding);

        // candidates: min-heap (closest first to pop), results: max-heap (farthest first to pop).
        let mut candidates = BinaryHeap::new();
        let mut results = BinaryHeap::new();

        // Bump generation counter; extend visited buffer if index grew.
        self.visited_gen = self.visited_gen.wrapping_add(1);
        if self.visited_gen == 0 {
            // Wrapped around — clear the buffer to avoid false positives.
            self.visited.iter_mut().for_each(|v| *v = 0);
            self.visited_gen = 1;
        }
        if self.visited.len() < self.nodes.len() {
            self.visited.resize(self.nodes.len(), 0);
        }

        candidates.push(MinEntry {
            id: entry,
            distance: ep_dist,
        });
        results.push(MaxEntry {
            id: entry,
            distance: ep_dist,
        });
        self.visited[entry as usize] = self.visited_gen;

        while let Some(MinEntry {
            id: c_id,
            distance: c_dist,
        }) = candidates.pop()
        {
            // If the closest candidate is farther than the farthest result, stop.
            if let Some(farthest) = results.peek() {
                if c_dist > farthest.distance && results.len() >= ef {
                    break;
                }
            }

            let conns = &self.nodes[c_id as usize].connections;
            if level < conns.len() {
                for &neighbor in &conns[level] {
                    let n_idx = neighbor as usize;
                    if n_idx < self.nodes.len() && self.visited[n_idx] != self.visited_gen {
                        self.visited[n_idx] = self.visited_gen;
                        let d = cosine_distance(query, &self.nodes[n_idx].embedding);

                        let should_add = if results.len() < ef {
                            true
                        } else if let Some(farthest) = results.peek() {
                            d < farthest.distance
                        } else {
                            true
                        };

                        if should_add {
                            candidates.push(MinEntry {
                                id: neighbor,
                                distance: d,
                            });
                            results.push(MaxEntry {
                                id: neighbor,
                                distance: d,
                            });
                            if results.len() > ef {
                                results.pop(); // Remove farthest.
                            }
                        }
                    }
                }
            }
        }

        // Collect results sorted by distance ascending.
        let mut result_vec: Vec<(NodeId, f32)> =
            results.into_iter().map(|e| (e.id, e.distance)).collect();
        result_vec.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
        result_vec
    }

    /// Select the M closest neighbors from candidates.
    fn select_neighbors(&self, candidates: &[(NodeId, f32)], max_conn: usize) -> Vec<NodeId> {
        candidates
            .iter()
            .take(max_conn)
            .map(|&(id, _)| id)
            .collect()
    }

    /// Check if a node is deleted.
    fn is_deleted(&self, id: NodeId) -> bool {
        let idx = id as usize;
        idx < self.deleted.len() && self.deleted[idx]
    }
}

// ---------------------------------------------------------------------------
// Distance function
// ---------------------------------------------------------------------------

/// Cosine distance: 1.0 - cosine_similarity. Lower is more similar.
#[inline]
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut mag_a = 0.0f32;
    let mut mag_b = 0.0f32;

    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        mag_a += x * x;
        mag_b += y * y;
    }

    let denom = mag_a.sqrt() * mag_b.sqrt();
    if denom < f32::EPSILON {
        return 1.0; // Maximally distant for zero vectors.
    }

    let sim = (dot / denom).clamp(-1.0, 1.0);
    1.0 - sim
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a random-ish unit vector of given dimension using a simple seed.
    fn random_unit_vec(dim: usize, seed: u64) -> Vec<f32> {
        let mut state = seed;
        let mut v: Vec<f32> = (0..dim)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                ((state >> 33) as f32 / (1u64 << 31) as f32) - 0.5
            })
            .collect();

        let mag: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if mag > f32::EPSILON {
            for x in &mut v {
                *x /= mag;
            }
        }
        v
    }

    #[test]
    fn test_insert_and_search_basic() {
        let mut idx = HnswIndex::new(4);

        let v1 = vec![1.0, 0.0, 0.0, 0.0];
        let v2 = vec![0.0, 1.0, 0.0, 0.0];
        let v3 = vec![0.9, 0.1, 0.0, 0.0]; // Close to v1.

        let id1 = idx.insert(&v1).expect("insert v1");
        let id2 = idx.insert(&v2).expect("insert v2");
        let id3 = idx.insert(&v3).expect("insert v3");

        let results = idx.search(&v1, 2, 50).expect("search");
        assert!(!results.is_empty());
        // Closest should be v1 itself, then v3.
        assert_eq!(results[0].0, id1);
        if results.len() > 1 {
            assert_eq!(results[1].0, id3);
        }

        // v2 should be furthest from v1.
        let results_all = idx.search(&v1, 3, 50).expect("search all");
        assert_eq!(results_all.len(), 3);
        assert_eq!(results_all.last().map(|r| r.0), Some(id2));
    }

    #[test]
    fn test_empty_index_search() {
        let mut idx = HnswIndex::new(8);
        let query = vec![0.5; 8];
        let results = idx.search(&query, 5, 50).expect("search empty");
        assert!(results.is_empty());
    }

    #[test]
    fn test_delete_node() {
        let mut idx = HnswIndex::new(4);
        let v1 = vec![1.0, 0.0, 0.0, 0.0];
        let v2 = vec![0.0, 1.0, 0.0, 0.0];

        let id1 = idx.insert(&v1).expect("insert v1");
        let _id2 = idx.insert(&v2).expect("insert v2");

        assert_eq!(idx.len(), 2);
        idx.delete(id1).expect("delete v1");
        assert_eq!(idx.len(), 1);

        // Search should not return deleted node.
        let results = idx.search(&v1, 5, 50).expect("search after delete");
        assert!(results.iter().all(|r| r.0 != id1));
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut idx = HnswIndex::new(8);
        for seed in 0..20u64 {
            let v = random_unit_vec(8, seed);
            idx.insert(&v).expect("insert");
        }
        idx.delete(3).expect("delete node 3");

        let bytes = idx.to_bytes().expect("serialize");
        let restored = HnswIndex::from_bytes(&bytes).expect("deserialize");

        assert_eq!(restored.dim, idx.dim);
        assert_eq!(restored.m, idx.m);
        assert_eq!(restored.len(), idx.len());
        assert_eq!(restored.max_level, idx.max_level);
        assert_eq!(restored.entry_point, idx.entry_point);

        // Check an embedding survived.
        let orig_emb = idx.get_embedding(0).expect("original embedding");
        let rest_emb = restored.get_embedding(0).expect("restored embedding");
        assert_eq!(orig_emb, rest_emb);
    }

    #[test]
    fn test_cosine_distance_accuracy() {
        let a = vec![1.0, 0.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0, 0.0];
        assert!((cosine_distance(&a, &b) - 0.0).abs() < 1e-5);

        let c = vec![0.0, 1.0, 0.0, 0.0];
        assert!((cosine_distance(&a, &c) - 1.0).abs() < 1e-5);

        let d = vec![-1.0, 0.0, 0.0, 0.0];
        assert!((cosine_distance(&a, &d) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn test_large_index_recall() {
        let dim = 32;
        let n = 300;
        let k = 10;
        let mut idx = HnswIndex::new(dim);

        let mut embeddings = Vec::with_capacity(n);
        for seed in 0..n as u64 {
            let v = random_unit_vec(dim, seed * 7 + 13);
            idx.insert(&v).expect("insert");
            embeddings.push(v);
        }

        // Test recall: for a few query points, check that the HNSW result
        // overlaps well with the brute-force ground truth.
        let mut total_recall = 0.0f64;
        let num_queries = 20;

        for q_seed in 0..num_queries {
            let query = random_unit_vec(dim, 10000 + q_seed);

            // Brute-force: compute all similarities.
            let mut brute: Vec<(usize, f32)> = embeddings
                .iter()
                .enumerate()
                .map(|(i, emb)| (i, 1.0 - cosine_distance(&query, emb)))
                .collect();
            brute.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
            let ground_truth: Vec<usize> = brute.iter().take(k).map(|&(i, _)| i).collect();

            // HNSW search.
            let hnsw_results = idx.search(&query, k, 50).expect("search");
            let hnsw_ids: Vec<usize> = hnsw_results.iter().map(|&(id, _)| id as usize).collect();

            // Recall = |intersection| / k.
            let hits = hnsw_ids
                .iter()
                .filter(|id| ground_truth.contains(id))
                .count();
            total_recall += hits as f64 / k as f64;
        }

        let avg_recall = total_recall / num_queries as f64;
        assert!(
            avg_recall > 0.7,
            "HNSW recall should be > 0.7, got {avg_recall:.3}"
        );
    }

    #[test]
    fn test_dimension_mismatch_returns_error() {
        let mut idx = HnswIndex::new(4);
        let wrong_dim = vec![1.0, 2.0]; // dim=2 instead of 4
        assert!(idx.insert(&wrong_dim).is_err());
        let _ = idx.insert(&[1.0, 0.0, 0.0, 0.0]).expect("insert ok");
        assert!(idx.search(&wrong_dim, 1, 50).is_err());
    }

    #[test]
    fn test_delete_nonexistent_returns_error() {
        let idx = HnswIndex::new(4);
        assert!(idx.is_empty());
        // Mutable needed for delete.
        let mut idx = idx;
        assert!(idx.delete(999).is_err());
    }

    #[test]
    fn test_get_embedding() {
        let mut idx = HnswIndex::new(4);
        let v = vec![0.6, 0.8, 0.0, 0.0];
        let id = idx.insert(&v).expect("insert");
        let retrieved = idx.get_embedding(id).expect("get_embedding");
        assert_eq!(retrieved, v.as_slice());
        assert!(idx.get_embedding(999).is_none());
    }

    #[test]
    fn test_compact_removes_tombstones() {
        let mut idx = HnswIndex::new(4);

        // Insert 10 random-ish vectors.
        let mut ids = Vec::new();
        for i in 0..10u32 {
            let v = vec![
                (i as f32 * 0.1).sin(),
                (i as f32 * 0.2).cos(),
                0.5,
                0.5,
            ];
            ids.push(idx.insert(&v).expect("insert"));
        }
        assert_eq!(idx.len(), 10);
        assert_eq!(idx.total_including_deleted(), 10);

        // Delete 3 nodes.
        for &id in &ids[0..3] {
            idx.delete(id).expect("delete");
        }
        assert_eq!(idx.len(), 7);
        assert_eq!(idx.total_including_deleted(), 10); // Tombstones still in memory.

        // Compact.
        let id_map = idx.compact();
        assert_eq!(idx.len(), 7);
        assert_eq!(idx.total_including_deleted(), 7); // Tombstones gone.
        assert_eq!(id_map.len(), 7);

        // Verify search still works.
        let results = idx.search(&[0.5, 0.5, 0.5, 0.5], 3, 50).expect("search");
        assert!(!results.is_empty());
    }
}
