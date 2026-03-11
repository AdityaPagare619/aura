# Agent 2e-P2a: AURA v4 Memory Subsystem — Deep Audit Report

**Auditor:** OpenCode (claude-opus-4.6)  
**Date:** 2026-03-10  
**Status:** COMPLETE  
**Files Audited:** 7 / 7  
**Total Lines of Code:** 6,121  

---

## Executive Summary

The AURA v4 memory subsystem is **surprisingly real**. Unlike many AI-agent projects where "memory" is a thin wrapper around a vector DB, this is a genuine 4-tier cognitive memory architecture implemented in pure Rust. The consolidation system (sleep-wake cycle) is genuinely implemented with k-means clustering. The HNSW index is a complete, correct ANN implementation. The embeddings use a clever TF-IDF sign-hashing trick that's deterministic, fast, and requires no GPU.

**However**, there are significant concerns:
- **No OutcomeBus integration** anywhere in the memory subsystem
- **No identity/personality engine integration** — memory is isolated from self-model
- **HNSW appears unwired** — it's built but may not be used in actual query paths (dependent on unaudited `episodic.rs`/`semantic.rs`)
- **Compaction is simplistic** — extractive concatenation, not LLM summarization
- **Archive FTS ignores min_relevance** — hardcoded 0.5 for all results
- **HNSW allocates O(n) visited-set per search** — performance concern at scale

**Overall System Grade: B-**  
Real engineering, but integration gaps and a few dead-code issues prevent a higher score.

---

## Per-File Analysis

### 1. mod.rs — Memory Orchestrator
- **Lines:** 1,369
- **Purpose:** Central orchestrator owning all 4 tiers (working, episodic, semantic, archive) plus PatternEngine, FeedbackLoop, and WorkflowMemory. Provides `AuraMemory` struct and `MemoryIntelligence` facade.
- **Grade: B+**

**What's REAL:**
- Cross-tier query with deduplication (`query_all_tiers`, lines 274-376): queries working, episodic, semantic, archive in parallel, deduplicates by content hash, merges scores. Genuinely implemented.
- `MemoryIntelligence` facade (lines 642-778): provides `learn_from_pattern()`, `remember_error_resolution()`, and `coordinate_activation()` that delegates spreading activation across tiers.
- Store + retrieve dispatching to correct tier based on `MemoryTier` enum.
- Extensive test suite (~400 lines, lines 900-1369).

**What's STUB/PLACEHOLDER:**
- Module exports (lines 23-47) reference `episodic`, `semantic`, `feedback`, `patterns`, `importance`, `workflows` — these files were NOT part of this audit, so integration quality is unknown.
- No OutcomeBus integration found (confirmed by grep across all 7 files).
- No identity engine references.

**Mobile Feasibility:**
- Orchestrator itself is lightweight — just routing and coordination.
- Real cost depends on tier implementations it owns.
- RAM: minimal overhead beyond what tiers consume.

**Critical Issues:**
- Zero integration with OutcomeBus means memory cannot learn from action outcomes.
- Zero integration with identity means memory has no concept of "self" or personality persistence.

---

### 2. working.rs — Working Memory (RAM Ring Buffer)
- **Lines:** 1,107
- **Purpose:** Fast, bounded, in-RAM working memory with spreading activation, TTL-based expiry, and importance-weighted eviction.
- **Grade: A-**

**What's REAL:**
- Ring buffer with hard cap: `MAX_SLOTS=1024`, `DEFAULT_TTL_MS=300000` (5 min) — lines 18-19.
- Spreading activation with 60-second half-life genuinely implemented (lines 460-527). Uses `activation_links` HashMap for connected slot relationships. Decay formula: `activation * (0.5_f64).powf(elapsed_secs / 60.0)` (line 489).
- Eviction logic (lines 569-602): prefers expired slots, then lowest-importance slots. Correctly maintains `SlotState::Evicted` tracking.
- Query scoring (lines 218-287): cosine similarity (weight 0.7) + recency (0.1) + activation (0.2). Uses `BinaryHeap` for efficient top-k.
- Context-for-LLM (lines 383-454): Reciprocal Rank Fusion across 3 independent rankings (relevance, recency, activation). Sophisticated and correct.
- Compaction trigger at 90% capacity (line 145): delegates to `ContextCompactor`.
- ~400 lines of tests.

**What's STUB/PLACEHOLDER:**
- Nothing significant — this is a complete implementation.

**Mobile Feasibility:**
- 1024 slots × (384-dim f32 embedding + metadata) ≈ 1024 × (1.5KB + 0.5KB) ≈ **2MB RAM**. Acceptable for mobile.
- Spreading activation is O(links) per trigger — bounded by link count, not slot count. Fine.
- TTL sweep is O(n) but n=1024 max. Negligible CPU.
- **Battery:** periodic TTL sweeps are cheap. No concern.

**Critical Issues:**
- None. This is the best file in the audit.

---

### 3. compaction.rs — Context Compactor
- **Lines:** 101
- **Purpose:** Compacts working memory when it exceeds 90% capacity by summarizing oldest slots.
- **Grade: C+**

**What's REAL:**
- `ContextCompactor` identifies slots beyond "keep recent 100" threshold (lines 28-41).
- Creates summary slot using extractive heuristic: keeps first 3 events + last 2 events, ellipsis in middle (lines 46-74).
- Summary gets 10-hour TTL and maximum importance (1.0).

**What's STUB/PLACEHOLDER:**
- `target_ratio` field (0.3) defined at line 12 but **never used** in any logic — dead code.
- Summarization is simple string concatenation, NOT LLM-based. This means compacted context loses nuance, relationships, and reasoning chains. It's better than nothing, but it's a lossy heuristic that could discard critical context.

**Mobile Feasibility:**
- Trivial CPU/RAM cost. No concern.

**Critical Issues:**
- `target_ratio` is dead code — should either be used or removed.
- Extractive summarization is a known-weak approach. A single long session could lose important early context because only first-3 + last-2 events survive.

---

### 4. hnsw.rs — HNSW Approximate Nearest Neighbor Index
- **Lines:** 937
- **Purpose:** Pure-Rust implementation of Hierarchical Navigable Small World graph for fast vector similarity search.
- **Grade: B+**

**What's REAL:**
- Complete HNSW with standard parameters: `M=16`, `ef_construction=200`, `ef_search=50`.
- Insert with proper 2-phase algorithm (lines 152-244): greedy descent to target layer, then layer-0 neighborhood selection with heuristic pruning.
- Search with greedy descent + layer-0 ef-bounded exploration (lines 250-287).
- Lazy deletion with tombstones (lines 291-303).
- Compaction/rebuild that removes tombstoned nodes (lines 339-388).
- Binary serialization/deserialization with magic bytes + version header (lines 392-536).
- Deterministic LCG RNG for layer assignment (lines 543-557) — avoids `rand` crate dependency.
- Recall test showing >0.7 recall at 300 nodes (lines 825-872).

**What's STUB/PLACEHOLDER:**
- Nothing — implementation is complete.

**Mobile Feasibility — THIS IS THE CRITICAL QUESTION:**
- **Memory footprint for 10K entries × 384 dims:**
  - Embeddings: 10,000 × 384 × 4 bytes = **14.7 MB**
  - Graph structure (M=16 neighbors per node, ~3.3 avg layers): ~10,000 × 16 × 8 bytes × 1.5 ≈ **1.9 MB**
  - Total: **~17 MB** for 10K entries. Feasible on mobile.
- **For 100K entries:** ~170 MB. Pushing mobile limits but possible on modern phones (4-8GB RAM).
- **Per-search allocation bug:** `search_layer` allocates `vec![false; self.nodes.len()]` (line 599) on EVERY search call. For 10K nodes = 10KB alloc/dealloc per search. For 100K = 100KB. This causes GC pressure and is avoidable with a reusable visited-set or epoch-based approach.
- **CPU:** Search is O(log n) hops × ef comparisons. Fast. Insert is heavier but amortized.
- **Battery:** Acceptable for infrequent queries. Batch inserts during charging recommended.

**Critical Issues:**
- O(n) visited-set allocation per search (line 599) — performance regression at scale.
- **Integration uncertainty:** HNSW appears to exist as standalone infrastructure. None of the 7 audited files reference it in their query paths. It may be used inside `episodic.rs` or `semantic.rs` (not audited), but this is unconfirmed. If it's not wired in, it's dead code.
- Recall >0.7 at 300 nodes is a low bar — should be tested at 10K+ for mobile validation.

---

### 5. embeddings.rs — TF-IDF Sign-Hash Embeddings
- **Lines:** 749
- **Purpose:** Generate 384-dimensional embeddings from text using TF-IDF with sign-hashing (feature hashing), without requiring a neural model or GPU.
- **Grade: B**

**What's REAL:**
- Sign-hashing trick (Weinberger et al. 2009): FNV-1a hash of n-grams mapped to 384 dimensions with ±1 sign (lines 120-180).
- Feature extraction: unigrams (weight 1.0), bigrams (weight 0.7), character trigrams (weight 0.3) — lines 85-118.
- TF-IDF weighting with corpus-derived IDF approximation.
- Stop word filtering.
- L2 normalization to unit vectors.
- LRU cache: 1024 entries behind global `Mutex<HashMap>` (lines 200-230).
- `quantize_u8` / `dequantize_u8` for compact storage (lines 290-310).
- `embed_best_effort()` async (lines 286-324): attempts IPC to Neocortex process for neural embeddings with TF-IDF fallback.
- Sign distribution validation test confirming approximate uniform distribution across dimensions.

**What's STUB/PLACEHOLDER:**
- `embed_best_effort()` IPC path to Neocortex (lines 286-324): the IPC `Embed` variant may not exist in the Neocortex process yet. This is a **future capability** — currently always falls back to TF-IDF.

**Mobile Feasibility:**
- CPU: FNV-1a hashing + n-gram extraction is extremely fast. Sub-millisecond per embedding.
- RAM: 384 × 4 bytes = 1.5KB per embedding. Cache of 1024 = ~1.5MB. Trivial.
- No GPU required. No model file needed. **Excellent for mobile.**
- Battery: negligible.

**Quality Concern:**
- TF-IDF sign-hashing captures lexical similarity but NOT semantic similarity. "happy" and "glad" will have near-zero similarity. "bank" (financial) and "bank" (river) will score identically. For a memory system, this means recall is limited to lexical overlap, which is a significant limitation for "did I see something like this before?" queries.
- The neural IPC fallback would fix this, but it's not wired yet.

**Critical Issues:**
- Global `Mutex` on LRU cache (line 200) is a contention point under concurrent access. Should be a `RwLock` or sharded.
- Semantic quality of TF-IDF embeddings is fundamentally limited compared to neural embeddings.

---

### 6. consolidation.rs — Memory Consolidation (Sleep-Wake Cycle)
- **Lines:** 1,084
- **Purpose:** 4-level memory consolidation inspired by cognitive sleep cycles: Micro, Light, Deep, Emergency.
- **Grade: A-**

**What's REAL:**
- **Micro** (lines 195-205): sweeps expired working memory slots. Trivial but real.
- **Light** (lines 210-319): promotes working→episodic based on dual scoring (`consolidation_priority` + legacy `importance`). Reinforces existing semantic entries by recency. Records patterns for `PatternEngine`. Genuinely implemented with proper threshold logic.
- **Deep** (lines 322-336 orchestration, 443-583 k-means, 634-728 generalization):
  - K-means clustering on episode embeddings: k=8 clusters, 10 iterations, k-means++ initialization. REAL and correct implementation.
  - Cluster generalization: discovers topic clusters from episodes, creates semantic generalizations, archives old episodes beyond 30-day threshold.
  - This is the most sophisticated piece of the memory subsystem.
- **Emergency** (lines 339-435): aggressive sweep with lower thresholds (importance < 0.5, age > 7 days vs normal 0.3/30 days). Triggered when storage is critically full.
- Cascading design: Deep includes Light which includes Micro. Well-designed.
- Consolidation priority scoring with 7-day half-life recency decay.

**What's STUB/PLACEHOLDER:**
- Nothing significant — all 4 levels are implemented.

**Mobile Feasibility:**
- K-means with k=8, 10 iterations on episodic embeddings: O(k × n × d × iterations) where n=episodes, d=384. For 1000 episodes: 8 × 1000 × 384 × 10 = ~30M float ops. Completes in <100ms on modern mobile CPU. Acceptable.
- Should be scheduled during charging/idle (like actual sleep consolidation).
- RAM: temporary allocation for k=8 centroids × 384 dims = negligible.
- **Battery:** Deep consolidation should NOT run on battery. Light/Micro are fine.

**Critical Issues:**
- No mechanism found to trigger consolidation automatically based on device state (charging, idle). The API exists but scheduling is external.
- K-means with fixed k=8 may not be optimal — should adapt to episode count.

---

### 7. archive.rs — Cold Storage (SQLite + FTS5 + Compression)
- **Lines:** 774
- **Purpose:** SQLite-backed archive tier with FTS5 full-text search and LZ4/ZSTD compression.
- **Grade: B**

**What's REAL:**
- SQLite with WAL mode for concurrent reads (line 45).
- FTS5 index on summaries for full-text search (lines 60-75).
- Dual compression: LZ4 (fast, for recent) and ZSTD (high ratio, for old) with custom wire format: 4-byte magic + 1-byte algorithm ID + 4-byte original length + payload (lines 120-180).
- `Arc<Mutex<Connection>>` with `tokio::task::spawn_blocking` for async interface (lines 30-40).
- `prune_before()` for age-based deletion (lines 400-420).
- `storage_stats()` returning entry count, disk size, compression ratio (lines 430-470).
- Retrieve with decompression (lines 350-390).
- FTS query properly escapes user input to prevent injection (lines 500-520).

**What's STUB/PLACEHOLDER:**
- `_min_relevance` parameter is accepted but **ignored** (line 353). All FTS results get hardcoded relevance 0.5 (line 518). This means the archive cannot filter by relevance quality — it returns everything FTS matches regardless of how weak the match is.

**Mobile Feasibility:**
- SQLite is the gold standard for mobile storage. Perfect fit.
- LZ4 decompression is extremely fast (~2GB/s). ZSTD is slower but still fast (~500MB/s).
- WAL mode is correct for mobile (avoids journal corruption on unexpected shutdown).
- Disk: depends on content volume. Compression helps. 10K archived entries × ~500 bytes compressed ≈ 5MB. Fine.
- **Battery:** SQLite queries are efficient. FTS5 is optimized. Compression/decompression adds small CPU cost.

**Critical Issues:**
- `_min_relevance` ignored — dead parameter. All FTS results treated equally. This could return low-quality matches that pollute recall results.
- `Arc<Mutex<Connection>>` means only ONE concurrent SQLite operation. Under heavy query load (e.g., cross-tier query from mod.rs), this becomes a bottleneck. Should consider connection pooling or `RwLock` for read-heavy workloads.

---

## Grades Summary

| File | Lines | Grade | Verdict |
|------|-------|-------|---------|
| mod.rs | 1,369 | **B+** | Real orchestrator, but missing OutcomeBus + identity integration |
| working.rs | 1,107 | **A-** | Best file. Bounded, activation-aware, well-tested |
| compaction.rs | 101 | **C+** | Real but simplistic. Dead code (`target_ratio`). Lossy heuristic. |
| hnsw.rs | 937 | **B+** | Complete HNSW, but O(n) alloc per search + integration uncertainty |
| embeddings.rs | 749 | **B** | Clever TF-IDF trick, but semantic quality limited. Neural path is stub. |
| consolidation.rs | 1,084 | **A-** | Genuinely impressive. K-means clustering for deep consolidation. |
| archive.rs | 774 | **B** | Solid SQLite+FTS5, but ignored min_relevance + Mutex bottleneck |

**Overall System Grade: B-**

---

## Answers to 6 Key Questions

### Q1: Can HNSW actually work on mobile? Memory footprint?

**Yes, with caveats.** At 10K entries × 384 dims, the HNSW index consumes ~17MB (14.7MB embeddings + 1.9MB graph). This is feasible on any modern phone. At 100K entries, it's ~170MB — possible on high-end phones (6-8GB RAM) but risky on budget devices. The O(n) visited-set allocation per search (line 599 of hnsw.rs) is a performance concern that should be fixed with a reusable bitset or epoch counter. Search latency is fast (O(log n) hops) but insert is heavier. **Recommendation:** Cap at 50K entries on mobile, use epoch-based visited tracking, and run index rebuilds during charging.

### Q2: How are embeddings generated without cloud API? Is it using local LLM?

**No LLM involved.** Embeddings use TF-IDF with sign-hashing (Weinberger et al. 2009) — a classical NLP technique. FNV-1a hash maps unigrams, bigrams, and character trigrams into a 384-dimensional space with ±1 signs. This is deterministic, requires no model file, no GPU, and completes in sub-millisecond time. The `embed_best_effort()` function (embeddings.rs:286-324) has an IPC path to request neural embeddings from a Neocortex process, but this appears to be a **future capability** — it falls back to TF-IDF. The tradeoff: embeddings capture lexical similarity only, NOT semantic similarity ("happy" ≠ "glad").

### Q3: Is memory consolidation (sleep-wake cycle) genuinely implemented or theater?

**Genuinely implemented.** This was the biggest positive surprise. The 4-level cascade (Micro → Light → Deep → Emergency) in consolidation.rs is real:
- **Micro** sweeps expired slots (trivial but functional)
- **Light** promotes important working memories to episodic tier with dual-score thresholds
- **Deep** runs k-means clustering (k=8, k-means++ init, 10 iterations) on episode embeddings to discover topic clusters, then generalizes clusters into semantic entries and archives old episodes
- **Emergency** uses aggressive thresholds when storage is critical

The k-means implementation (consolidation.rs:443-583) is correct and the generalization pipeline (lines 634-728) creates real semantic abstractions from episodic clusters. This is NOT theater — it's a functional cognitive consolidation system. The main limitation is that scheduling (when to trigger each level) appears to be external, with no built-in device-state awareness.

### Q4: What happens when storage fills up? Is compaction real?

**Yes, with layered defense:**
1. **Working memory** (working.rs): hard-capped at 1024 slots. At 90% capacity (line 145), `ContextCompactor` triggers and summarizes oldest slots. If still full, eviction removes lowest-importance expired slots first, then lowest-importance active slots.
2. **Consolidation** (consolidation.rs): Light consolidation promotes working→episodic. Deep consolidation archives old episodes (>30 days). Emergency consolidation uses aggressive thresholds (importance <0.5, age >7 days).
3. **Archive** (archive.rs): `prune_before()` deletes entries older than a given timestamp. `storage_stats()` provides monitoring.

**Weakness:** Compaction in compaction.rs is a simple extractive heuristic (first 3 + last 2 events), not LLM-based summarization. Long sessions lose mid-session context. The `target_ratio` field meant to control compaction aggressiveness is dead code (never read).

### Q5: Is working memory actually bounded or can it grow unbounded?

**It is bounded.** Hard cap at `MAX_SLOTS = 1024` (working.rs:18). Three enforcement mechanisms:
1. **TTL expiry** (default 5 minutes, working.rs:19): slots auto-expire and get reclaimed
2. **Compaction** at 90% capacity (working.rs:145): summarizes oldest slots
3. **Eviction** (working.rs:569-602): removes expired-then-lowest-importance when inserting into a full buffer

With 1024 slots × ~2KB each, worst-case RAM is ~2MB. Cannot grow unbounded. This is well-engineered.

### Q6: How does memory system interact with OutcomeBus and identity?

**It doesn't.** A grep across all 7 files returned zero matches for "OutcomeBus", "outcome_bus", "outcome", "identity", "personality", or "self_model". The memory subsystem is completely isolated from:
- **OutcomeBus:** Memory cannot learn which recalled memories led to successful vs failed actions. There's no reinforcement loop.
- **Identity:** Memory has no concept of which memories are "self-defining" vs incidental. There's no personality persistence through memory.

This is the **single biggest architectural gap** in the memory subsystem. Without OutcomeBus integration, the system can store and retrieve but cannot learn from outcomes. Without identity integration, there's no autobiographical memory or self-model maintenance.

The `MemoryIntelligence` facade in mod.rs (lines 642-778) has `learn_from_pattern()` and `remember_error_resolution()` methods, but these are pattern-matching within memory itself — they don't connect to external action outcomes.

---

## Critical Issues (Ordered by Severity)

1. **NO OutcomeBus integration** — Memory cannot learn from action success/failure. Severity: HIGH.
2. **NO identity integration** — No autobiographical or self-model memory. Severity: HIGH.
3. **HNSW possibly unwired** — Complete implementation may be dead code if episodic.rs/semantic.rs don't reference it. Severity: MEDIUM-HIGH (need to verify).
4. **TF-IDF embeddings lack semantic understanding** — "happy" ≠ "glad". Limits recall quality. Neural IPC path is stub. Severity: MEDIUM.
5. **Archive ignores min_relevance** (archive.rs:353/518) — All FTS results returned regardless of match quality. Severity: MEDIUM.
6. **HNSW O(n) alloc per search** (hnsw.rs:599) — Performance regression at scale. Severity: MEDIUM.
7. **Compaction target_ratio dead code** (compaction.rs:12) — Minor dead code. Severity: LOW.
8. **Archive Mutex bottleneck** (archive.rs:30) — Single concurrent SQLite op. Severity: LOW-MEDIUM.
9. **Embeddings cache global Mutex** (embeddings.rs:200) — Contention under concurrency. Severity: LOW.

---

## Mobile Feasibility Assessment

| Aspect | Status | Notes |
|--------|--------|-------|
| RAM (10K memories) | ✅ OK | ~20MB total (working 2MB + HNSW 17MB + overhead) |
| RAM (100K memories) | ⚠️ RISKY | ~175MB. High-end phones only. |
| CPU (queries) | ✅ OK | Sub-ms embeddings, O(log n) HNSW search |
| CPU (consolidation) | ✅ OK | K-means on 1K episodes: <100ms |
| Storage | ✅ OK | SQLite + compression. 10K entries ≈ 5MB |
| Battery (normal use) | ✅ OK | Light queries, TTL sweeps negligible |
| Battery (deep consolidation) | ⚠️ SCHEDULE | K-means should run during charging |
| No cloud dependency | ✅ OK | All computation local. TF-IDF, not neural. |
| Startup time | ✅ OK | SQLite WAL + in-memory HNSW rebuild from serialized |

**Verdict: Mobile-feasible at 10K-50K memory scale. Beyond 50K, RAM becomes the bottleneck (HNSW embeddings). Needs entry cap or quantized embeddings for larger scales.**

---

## Recommendations

1. **Wire OutcomeBus** into `AuraMemory` — add `on_outcome()` handler that adjusts importance of recently-recalled memories based on action success/failure.
2. **Wire identity** — tag memories with identity-relevance score, maintain autobiographical memory subset.
3. **Verify HNSW integration** — audit `episodic.rs` and `semantic.rs` to confirm HNSW is actually used in query paths.
4. **Fix HNSW visited-set** — replace per-search `Vec<bool>` allocation with epoch-based or reusable bitset.
5. **Implement min_relevance** in archive.rs FTS — use SQLite FTS5 rank function instead of hardcoded 0.5.
6. **Remove dead code** — `target_ratio` in compaction.rs.
7. **Replace Mutex with RwLock** — in archive.rs (Connection) and embeddings.rs (LRU cache) for better concurrency.
8. **Add quantized HNSW mode** — use `quantize_u8` from embeddings.rs to store uint8 embeddings in HNSW, reducing memory 4×.
9. **Schedule deep consolidation** — integrate with platform power management APIs to run during charging/idle.

---

## Structured JSON Report

```json
{
  "audit": "2e-P2a Memory Subsystem",
  "date": "2026-03-10",
  "auditor": "OpenCode (claude-opus-4.6)",
  "total_files": 7,
  "total_lines": 6121,
  "overall_grade": "B-",
  "files": {
    "mod.rs": {
      "lines": 1369,
      "grade": "B+",
      "purpose": "Central orchestrator: AuraMemory + MemoryIntelligence facade",
      "real_vs_stub": "REAL orchestrator with cross-tier query, dedup, spreading activation coordination. Missing OutcomeBus + identity.",
      "mobile_feasibility": "OK — lightweight routing layer",
      "critical_issues": ["No OutcomeBus integration", "No identity integration"]
    },
    "working.rs": {
      "lines": 1107,
      "grade": "A-",
      "purpose": "Bounded RAM ring buffer with spreading activation, TTL, eviction",
      "real_vs_stub": "FULLY REAL. 1024-slot ring buffer, spreading activation with 60s half-life, RRF scoring, compaction trigger at 90%.",
      "mobile_feasibility": "OK — ~2MB RAM, sub-ms queries",
      "critical_issues": []
    },
    "compaction.rs": {
      "lines": 101,
      "grade": "C+",
      "purpose": "Context compaction for long sessions",
      "real_vs_stub": "REAL but simplistic. Extractive (first 3 + last 2), not LLM-based. target_ratio is dead code.",
      "mobile_feasibility": "OK — trivial CPU/RAM",
      "critical_issues": ["target_ratio dead code", "Lossy extractive heuristic"]
    },
    "hnsw.rs": {
      "lines": 937,
      "grade": "B+",
      "purpose": "Pure-Rust HNSW ANN index for vector similarity search",
      "real_vs_stub": "FULLY REAL. Complete HNSW with M=16, ef_construction=200, serialization, lazy deletion, compaction.",
      "mobile_feasibility": "OK at 10K (~17MB), RISKY at 100K (~170MB). O(n) alloc per search needs fix.",
      "critical_issues": ["O(n) visited-set alloc per search", "Integration with query paths unverified"]
    },
    "embeddings.rs": {
      "lines": 749,
      "grade": "B",
      "purpose": "TF-IDF sign-hash embeddings (384-dim, no GPU, no model file)",
      "real_vs_stub": "REAL TF-IDF with sign-hashing. Neural IPC path (embed_best_effort) is STUB/FUTURE.",
      "mobile_feasibility": "EXCELLENT — sub-ms, no GPU, ~1.5MB cache",
      "critical_issues": ["Lexical-only similarity (no semantic)", "Global Mutex on cache", "Neural IPC path not wired"]
    },
    "consolidation.rs": {
      "lines": 1084,
      "grade": "A-",
      "purpose": "4-level cognitive consolidation: Micro/Light/Deep/Emergency",
      "real_vs_stub": "FULLY REAL. K-means clustering (k=8, k-means++ init) for deep consolidation. Cascading levels. Genuine cognitive architecture.",
      "mobile_feasibility": "OK — k-means on 1K episodes <100ms. Deep consolidation should run during charging.",
      "critical_issues": ["No device-state-aware scheduling", "Fixed k=8 may not suit all episode counts"]
    },
    "archive.rs": {
      "lines": 774,
      "grade": "B",
      "purpose": "SQLite + FTS5 + LZ4/ZSTD compressed cold storage",
      "real_vs_stub": "REAL. Dual compression, WAL mode, FTS5 search, prune/stats. min_relevance parameter IGNORED (hardcoded 0.5).",
      "mobile_feasibility": "EXCELLENT — SQLite is mobile-native. Compression saves disk.",
      "critical_issues": ["min_relevance ignored", "Arc<Mutex<Connection>> bottleneck"]
    }
  },
  "key_questions": {
    "q1_hnsw_mobile": {
      "answer": "YES with caveats",
      "detail": "17MB at 10K entries (feasible), 170MB at 100K (risky). Fix O(n) alloc per search. Cap at 50K on mobile."
    },
    "q2_embeddings_method": {
      "answer": "TF-IDF sign-hashing, NOT neural, NOT cloud",
      "detail": "Weinberger 2009 feature hashing with FNV-1a. Sub-ms, no GPU. Neural IPC path is future stub."
    },
    "q3_consolidation_real": {
      "answer": "GENUINELY IMPLEMENTED",
      "detail": "4-level cascade with real k-means clustering (k=8, k-means++, 10 iter) for deep consolidation. Not theater."
    },
    "q4_storage_compaction": {
      "answer": "YES, layered defense",
      "detail": "Working: 1024 cap + compaction + eviction. Consolidation: promote/archive/emergency. Archive: prune_before(). Compaction is extractive, not LLM-based."
    },
    "q5_working_bounded": {
      "answer": "YES, hard-bounded at 1024 slots",
      "detail": "MAX_SLOTS=1024, TTL=5min, compaction at 90%, eviction at 100%. ~2MB worst case."
    },
    "q6_outcomebus_identity": {
      "answer": "NO INTEGRATION EXISTS",
      "detail": "Zero references to OutcomeBus or identity in all 7 files. Memory is isolated — cannot learn from outcomes or maintain self-model."
    }
  },
  "mobile_feasibility": {
    "overall": "FEASIBLE at 10K-50K scale",
    "ram_10k": "~20MB (OK)",
    "ram_100k": "~175MB (RISKY)",
    "cpu": "OK (sub-ms queries, <100ms consolidation)",
    "battery": "OK for normal use, schedule deep consolidation during charging",
    "storage": "OK (SQLite + compression)",
    "cloud_dependency": "NONE"
  },
  "critical_issues_count": 9,
  "critical_issues_high": ["No OutcomeBus integration", "No identity integration"],
  "critical_issues_medium": ["HNSW possibly unwired", "TF-IDF semantic limitations", "Archive ignores min_relevance", "HNSW O(n) alloc per search"],
  "critical_issues_low": ["compaction target_ratio dead code", "Archive Mutex bottleneck", "Embeddings cache global Mutex"],
  "recommendations_count": 9
}
```

---

*Checkpoint saved by Agent 2e-P2a. Task: COMPLETE.*
