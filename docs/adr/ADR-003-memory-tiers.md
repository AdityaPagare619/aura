# ADR-003: 4-Tier Memory with HNSW Vector Index

**Status:** Accepted  
**Date:** 2026-03-01  
**Deciders:** AURA Core Team

## Context

AURA is a persistent on-device agent that accumulates knowledge over months/years of use. It must:

1. Recall recent context instantly (working memory for current task)
2. Remember episodic interactions (what happened, when, with whom)
3. Build semantic knowledge (user preferences, app behaviors, learned facts)
4. Retain long-term archives without consuming active storage
5. Stay within mobile memory/storage constraints (~50-100MB total)
6. Support vector similarity search for context retrieval

A flat key-value store can't satisfy all these requirements simultaneously — the access patterns, latency budgets, and retention policies differ fundamentally across use cases.

## Decision

Implement a **4-tier memory hierarchy** with tier-appropriate storage engines and a unified query interface. Add a pure-Rust **HNSW vector index** for similarity search across tiers.

### Tier Architecture

```
                 ┌─────────────────────────────────┐
                 │         MemoryIntelligence       │
                 │   pattern discovery, spreading   │
                 │   activation, error→learning     │
                 └───────────────┬─────────────────┘
                                 │
                 ┌───────────────┴─────────────────┐
                 │          AuraMemory              │
                 │   cross-tier query, dedup by     │
                 │   (tier, source_id), merge by    │
                 │   relevance score                │
                 └───┬───────┬───────┬─────────┬───┘
                     │       │       │         │
              ┌──────┴─┐ ┌──┴────┐ ┌┴───────┐ ┌┴────────┐
              │Working │ │Episodic│ │Semantic│ │ Archive  │
              │        │ │       │ │        │ │          │
              │RAM ring│ │SQLite │ │SQLite  │ │ ZSTD     │
              │1MB     │ │WAL    │ │+FTS5   │ │ compress │
              │1024    │ │       │ │+HNSW   │ │          │
              │slots   │ │       │ │        │ │          │
              │<1ms    │ │2-8ms  │ │5-15ms  │ │50-200ms  │
              │        │ │~18MB/ │ │~50MB/  │ │~4MB/yr   │
              │        │ │  yr   │ │  yr    │ │          │
              └────────┘ └───────┘ └────────┘ └──────────┘
```

**Location:** `crates/aura-daemon/src/memory/mod.rs`

### Tier Details

| Tier | Storage | Capacity | Latency | Growth Rate | Purpose |
|------|---------|----------|---------|-------------|---------|
| Working | RAM ring buffer | 1MB / 1024 slots | <1ms | N/A (volatile) | Current task context, recent observations |
| Episodic | SQLite WAL mode | ~18MB/yr | 2-8ms | ~50KB/day | Timestamped interaction records |
| Semantic | SQLite + FTS5 | ~50MB/yr | 5-15ms | ~140KB/day | Learned facts, user prefs, app knowledge |
| Archive | ZSTD compressed | ~4MB/yr | 50-200ms | ~11KB/day | Cold storage, rarely accessed history |

### Cross-Tier Query

`AuraMemory` provides a unified query interface (`memory/mod.rs`):
1. Query dispatched to all relevant tiers in parallel
2. Results merged by relevance score
3. Deduplicated by `(tier, source_id)` tuple — same fact stored in multiple tiers returns only the highest-relevance copy

### HNSW Vector Index

**Location:** `crates/aura-daemon/src/memory/hnsw.rs`

Pure Rust implementation (no external dependencies) for on-device similarity search:

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| M (max connections) | 16 | Balance between recall and memory. Standard for medium-scale indices |
| ef_construction | 200 | Build quality. Higher = better graph but slower insert |
| ef_search | 50 | Query quality. Tuned for <5ms search on typical corpus |
| Distance metric | Cosine | Standard for text embeddings |
| Deletion | Lazy (tombstones) | Avoids expensive graph restructuring; cleaned during consolidation |
| Serialization | Binary | Direct memory-mapped load for fast startup |

### Consolidation

**Location:** `crates/aura-daemon/src/memory/consolidation.rs`

Inspired by sleep-stage neuroscience. Four levels run at different frequencies:

| Level | Budget | Trigger | Action |
|-------|--------|---------|--------|
| Micro | <1ms | Inline during ops | Working memory slot rotation |
| Light | ≤60s | Periodic timer | Episodic dedup, weak pattern prune |
| Deep | ≤30min | Idle/charging | k-means clustering (k=8, 10 iters) on episode embeddings, promote to semantic |
| Emergency | <5s | Memory pressure | Aggressive sweep, archive old episodes, free 2.8-3.6MB |

Scoring formula for consolidation decisions:

```
score = recency(0.3) + frequency(0.3) + importance(0.4)
recency uses 7-day half-life decay
```

## Consequences

### Positive

- **Latency-appropriate access:** Current task context in <1ms, historical context in 2-15ms, cold lookups only when explicitly needed
- **Bounded growth:** Each tier has its own storage budget. Consolidation actively manages size. Archive compression yields ~10:1 ratio
- **Semantic search:** HNSW enables "find similar past actions" without exact keyword matching. Critical for context enrichment
- **No external dependencies:** Pure Rust HNSW avoids NDK/FFI complexity for vector search. Binary serialization enables fast checkpoint restore

### Negative

- **Complexity:** 4 tiers + consolidation + HNSW is significant code surface. Bugs in tier transitions can lose data
- **Tuning required:** Consolidation thresholds and HNSW parameters (M, ef) need empirical tuning per device class
- **Storage overhead:** SQLite WAL mode doubles write-path storage temporarily. FTS5 index adds ~30% overhead to semantic tier

## Alternatives Considered

### 1. Single SQLite Database (All Tiers in One)
- **Rejected:** Can't meet <1ms working memory latency. SQLite minimum is 1-2ms even for indexed lookups. Also, a single large database makes consolidation harder to reason about.

### 2. Redis/RocksDB for Working Memory
- **Rejected:** Additional native library dependency for Android. RAM ring buffer is simpler, faster, and sufficient for 1024 slots.

### 3. External Vector Database (Qdrant, Milvus)
- **Rejected:** Requires a separate server process or cloud connection. Unacceptable for an on-device agent prioritizing privacy and offline operation.

### 4. Flat File Storage + grep
- **Rejected:** No indexing, O(n) search, no vector similarity. Unusable at scale.

## References

- `crates/aura-daemon/src/memory/mod.rs` — AuraMemory orchestrator, cross-tier query, MemoryIntelligence
- `crates/aura-daemon/src/memory/hnsw.rs` — HNSW index implementation
- `crates/aura-daemon/src/memory/consolidation.rs` — 4-level consolidation engine
- `crates/aura-daemon/src/memory/patterns.rs` — PatternEngine (Hebbian learning)
