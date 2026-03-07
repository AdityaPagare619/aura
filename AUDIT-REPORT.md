# AURA v4 — Memory & Learning Systems: Atom-Level Precision Audit

**Date:** 2026-03-05
**Scope:** 14 source files (~13,000 lines of Rust) + Engineering Blueprint (2710 lines)
**Method:** Every line of code read and analyzed against blueprint promises and algorithmic correctness standards.

---

## Grading Scale

| Grade | Meaning |
|-------|---------|
| **A** | Production-quality. Algorithmically correct, properly bounded, mobile-viable. |
| **B** | Solid engineering. Minor gaps but fundamentally sound. Ship-worthy. |
| **C** | Functional but with significant shortcuts. Works in demos, fragile at scale. |
| **D** | Toy implementation dressed in production clothing. Key algorithms are stubs or wrong. |
| **F** | Fake or non-functional. Marketing copy, not engineering. |

---

## (A) Embeddings — Grade: C+

**File:** `memory/embeddings.rs` (716 lines)
**Blueprint Promise (§4.1):** "384-dim embeddings" — implies neural-quality semantic vectors. Deep consolidation references a "4B model" for embedding generation.

### What the Code Actually Does

A **TF-IDF sign-hashing** scheme using the Weinberger et al. (2009) feature-hashing trick:

1. Tokenizes input into unigrams, bigrams, and character trigrams
2. Hashes each token with FNV-1a to a 384-dimensional bucket
3. Applies a sign function (hash bit 31) so collisions partially cancel rather than compound
4. Weights by a **fake IDF**: `1.0 + 0.5 * (word_len / 10).min(1.0)` — this is word-length heuristic, NOT inverse document frequency from corpus statistics
5. L2-normalizes the result

### Algorithm Correctness

- **Sign hashing:** Correct application of the technique. Collision behavior is mathematically sound.
- **IDF approximation:** Incorrect by definition. Real IDF requires `log(N/df_t)` over a document corpus. Using word length as a proxy means "the" and "cat" (both 3 chars) get identical IDF, while "hippopotamus" gets maximum weight. This inverts reality — common long words get boosted, rare short words get suppressed.
- **Cosine similarity:** Correctly implemented with L2-normalized vectors.
- **Quantization:** Proper min/max scaling to `u8` with reconstruction. Lossy but appropriate for mobile storage.
- **LRU cache:** 1024 entries, sound implementation.

### What's Real

- 384-dim vectors that DO capture some semantic signal via n-gram overlap
- Cosine similarity correctly discriminates "related" from "unrelated" content (tests confirm)
- Measurable upgrade from V3's 64-dim naive trigram hash — more dimensions, sign hashing reduces collision damage
- Cache and quantization are production-quality utilities

### What's Shallow

- **Not neural embeddings.** Cannot capture synonymy ("happy" vs "joyful"), analogy, or compositional semantics. Two sentences about the same topic using different vocabulary will score poorly.
- **`embed_neural()` is a stub** — returns `None` with a TODO. The blueprint's "4B model" path does not exist.
- **Fake IDF** means term weighting is nonsensical for real-world text distributions
- Quality ceiling is fundamentally limited: this is a bag-of-n-grams hasher, not a semantic encoder

### Mobile Viability

- **Excellent.** Pure arithmetic, no model weights, ~584 bytes per vector (384 × f32, or 384 bytes quantized). LRU cache bounded. Zero external dependencies.
- The irony: it's mobile-viable *because* it's not a real neural embedding model. A 4B-parameter model would blow any mobile memory budget.

### Verdict

A competent feature-hashing implementation marketed as "embeddings." Genuine upgrade from V3 but sitting in a fundamentally different quality tier than what the blueprint implies. The neural path is vaporware. The system will work for exact/near-exact text matching but will fail on paraphrase detection, semantic search across vocabulary boundaries, and any task requiring actual language understanding.

---

## (B) HNSW — Grade: B+

**File:** `memory/hnsw.rs` (836 lines)
**Blueprint Promise (§4.1.4):** HNSW index for semantic search, used in conjunction with FTS5 and Reciprocal Rank Fusion.

### What the Code Actually Does

A textbook **Hierarchical Navigable Small World** graph implementation:

- **Parameters:** M=16, ef_construction=200, ef_search=50, max_level via `ML = 1/ln(M)` — all standard values from the Malkov & Yashunin (2018) paper
- **Insert:** Random level assignment via `floor(-ln(uniform) × ML)` using LCG PRNG, greedy descent from top layer, `search_layer` for neighbors at each level, bidirectional edge insertion with pruning
- **Search:** Top-down greedy descent to level 0, then `search_layer` with ef_search candidates, return top-k
- **`search_layer`:** Min-heap (candidates) + max-heap (results) pattern — correct dual-heap approach from the paper
- **Deletion:** Tombstone-based soft delete with periodic compaction
- **Serialization:** Binary format with version byte, full graph persistence

### Algorithm Correctness

- **Level distribution:** Correct exponential distribution. The LCG RNG (a=6364136223846793005, c=1442695040888963407) is a well-known constant from Knuth. Not cryptographic but fine for level assignment.
- **Search correctness:** The greedy-then-expand pattern is correct. Entry point starts at top level, descends greedily, then does beam search at level 0.
- **Neighbor selection:** Uses simple truncation to M neighbors (keep closest M), NOT the heuristic neighbor selection from the paper (Algorithm 4). The heuristic provides better graph connectivity for clustered data. This is adequate but suboptimal — roughly 5-10% recall penalty on clustered distributions.
- **Visited array:** `vec![false; self.nodes.len()]` allocated per search call. This is O(n) memory per query, which is wasteful. A hash set or generation counter would be better for large indices.
- **Recall test:** Asserts >0.7 recall@10 on 300 random 32-dim vectors. This is a reasonable but not demanding test. Production HNSW implementations typically achieve >0.95 recall@10.

### What's Real

- **Genuinely correct HNSW.** Not a toy. The core algorithm matches the paper.
- Multi-layer navigation works correctly
- Binary serialization enables persistence across daemon restarts
- Tombstone deletion avoids expensive graph reconstruction
- The 0.7 recall threshold is conservative but honest — the test isn't lying about performance

### What's Shallow

- **No heuristic neighbor selection** — the paper's Algorithm 4 improves recall significantly for real-world (clustered) data
- **O(n) visited allocation** per search — will cause GC pressure and latency spikes as index grows
- **LCG RNG is deterministic per seed** — if seed isn't varied, all indices get identical layer distributions
- **No SIMD distance computation** — cosine similarity is scalar loop. On mobile ARM (NEON), vectorized distance would be 4-8× faster
- **No dynamic ef_search tuning** — ef=50 is fixed regardless of required precision

### Mobile Viability

- **Good.** Memory usage is ~(n × M × 2 × 8) bytes for edges plus vector storage. For 10K vectors at 384-dim: ~300KB edges + ~15MB vectors (f32) or ~3.8MB (quantized u8). Fits comfortably in the 25MB RSS target.
- Search latency should be sub-millisecond for <50K vectors even without SIMD.
- The O(n) visited array is the main concern — 10K nodes = 10KB allocation per query, acceptable but wasteful.

### Verdict

This is a **real HNSW implementation**. It's the strongest algorithmic component in the entire codebase. The core search is correct, performance characteristics are reasonable for mobile scale, and it properly integrates with the semantic memory tier. The missing heuristic selection and scalar distance are optimization gaps, not correctness issues. Solid B+ work.

---

## (C) 4-Tier Memory — Grade: B-

**Files:** `working.rs` (1007), `episodic.rs` (927), `semantic.rs` (1244), `archive.rs` (790), `consolidation.rs` (1044), `importance.rs` (424)
**Blueprint Promise (§4.1):** Neuroscience-inspired 4-tier memory hierarchy with automatic consolidation, pattern separation (dentate gyrus), and knowledge generalization.

### Tier-by-Tier Analysis

#### Working Memory — Grade: B-

**What it does:** Ring buffer with 1024 slots, spreading activation model, composite scoring.

- **Spreading activation:** Exponential decay with 60-second half-life: `activation = base × 2^(-Δt/60)`. When a slot is accessed, activation propagates to slots with cosine similarity > 0.3, scaled by similarity. This is a legitimate spreading activation model (Anderson's ACT-R inspired).
- **Composite scoring:** `similarity × 0.7 + recency × 0.1 + activation × 0.2` — reasonable weights, though recency at 0.1 seems low for working memory where recency should dominate.
- **Query:** Linear scan of all 1024 slots. At 384-dim cosine similarity per slot, this is ~1.5M floating-point ops per query. Fast enough on mobile but architecturally O(n).
- **Real:** Spreading activation with temporal decay is genuine cognitive modeling, not a counter.
- **Shallow:** Ring buffer means old items are silently overwritten regardless of importance. No priority eviction.

#### Episodic Memory — Grade: C+

**What it does:** SQLite WAL storage of episodes with embeddings, emotional valence, importance scores, and context tags.

- **Pattern separation (dentate gyrus):** When a new episode's embedding has cosine > 0.9 with existing episodes, Gaussian noise (σ=0.05) is added to the embedding to push similar memories apart. This is a real (simplified) model of dentate gyrus pattern separation from computational neuroscience.
- **Query:** **O(n) linear scan** — loads ALL episode embeddings from SQLite and computes cosine similarity in Rust. No HNSW index. For 1000 episodes with 384-dim embeddings, this is ~768K float ops. For 10K episodes, ~7.7M float ops. Marginal on mobile.
- **Real:** Pattern separation is a genuine neuroscience-inspired algorithm, correctly implemented.
- **Shallow:** The linear scan is the critical weakness. The HNSW index exists in the codebase but episodic memory doesn't use it. This makes episodic query latency grow linearly with memory size — antithetical to the "organic growth" vision.

#### Semantic Memory — Grade: A-

**What it does:** SQLite + FTS5 full-text search + HNSW vector index, combined via Reciprocal Rank Fusion.

- **RRF fusion:** `score = Σ 1/(k + rank_i)` with k=60. Merges FTS5 BM25 rankings with HNSW cosine rankings. This is the standard RRF formula from Cormack et al. (2009). Correctly implemented.
- **Knowledge generalization:** When 3+ similar episodes exist (cosine > 0.7), extracts common patterns and creates a generalized semantic knowledge entry. Uses centroid embedding of the episode cluster. This is a real abstraction mechanism.
- **Knowledge reinforcement:** Re-encountering similar content strengthens existing knowledge (bumps access count, recalculates importance). Genuine spaced-repetition-adjacent behavior.
- **Real:** This tier delivers what the blueprint promises. HNSW + FTS5 + RRF is a production-quality hybrid search pattern used in real search engines.
- **Shallow:** Generalization trigger (3+ episodes) is a hard threshold — no confidence interval or statistical test.

#### Archive — Grade: C-

**What it does:** SQLite + FTS5 on summaries, with custom compression for original content.

- **Compression:** Blueprint specifies ZSTD. **Code implements byte-level RLE** (Run-Length Encoding) with a `TODO: Add zstd compression` comment. RLE on natural language text achieves near-zero compression because character repetition is rare. A sentence like "the cat sat" has zero runs longer than 1. This compression is essentially a no-op on real data.
- **FTS5 on summaries:** Works correctly for keyword search on archived content.
- **Real:** The archive tier exists and functions as cold storage with searchable summaries.
- **Shallow:** The compression story is fake. RLE on text is a placeholder, not a solution. ZSTD would achieve 3-5× compression on text; RLE achieves ~1.0× (no compression).

#### Consolidation Engine — Grade: B

**What it does:** 4-level consolidation (Micro/Light/Deep/Emergency) that moves memories up the hierarchy.

- **Micro (every ~5 min):** Decays working memory activations, identifies candidates for episodic promotion based on composite score > threshold.
- **Light (every ~30 min):** Clusters episodic memories by similarity, creates semantic generalizations from clusters of 3+.
- **Deep (every ~4 hours):** K-means clustering on episode embeddings (k chosen by heuristic), archives old low-importance episodes. The k-means implementation is real — iterative centroid recalculation with convergence check.
- **Emergency:** Triggers when storage exceeds limits — aggressive archival and pruning.
- **Real:** K-means clustering and tiered scheduling are legitimate. The consolidation logic maps to genuine memory consolidation theory.
- **Shallow:** Blueprint says Deep consolidation invokes "Neocortex" (the LLM) for summarization — code does NOT do this. Summaries are extractive (first N characters), not abstractive.

#### Importance Scoring — Grade: B-

**What it does:** Multi-factor importance calculation: source weight × content factors + recency decay + access bonus.

- **Recency decay:** `score × 2^(-age_days / 29)` — 29-day half-life. Mathematically correct exponential decay.
- **Domain detection:** Keyword matching against hardcoded lists ("password", "key" → security; "meeting", "deadline" → work). Functional but brittle.
- **Real:** The scoring formula is reasonable and the decay math is correct.
- **Shallow:** Domain detection by keyword list will misclassify constantly. No learned categories.

### Overall Tier Verdict

The semantic tier is genuinely strong (A-). The consolidation engine is architecturally sound (B). Working memory's spreading activation is real science (B-). Episodic memory's O(n) scan is a critical performance bug (C+). Archive compression is fake (C-). The system as a whole delivers a real memory hierarchy with legitimate neuroscience inspiration, but with notable implementation gaps that would surface under production load.

---

## (D) Hebbian Learning — Grade: B

**File:** `arc/learning/hebbian.rs` (1710 lines)
**Blueprint Promise (§4.2):** "Hebbian association network" with neurons that fire together wire together, spreading activation, and learned action recommendations.

### What the Code Actually Does

A **bounded concept-association graph** implementing a simplified Hebbian learning rule:

- **Concept nodes:** Up to 2048 concepts, each with a label, category, creation time, access count, and base activation level
- **Association edges:** Up to 8192 weighted edges between concept pairs, each with weight [0.0, 1.0], co-activation count, and last-activated timestamp
- **Hebb's rule:** On co-activation: `weight = (weight + 0.05).min(1.0)`. On contradiction/negative: `weight = (weight - 0.03).max(0.0)`.
- **Temporal decay:** `weight' = weight × 2^(-Δt / half_life)` — biologically inspired exponential forgetting
- **Spreading activation:** BFS from a source concept, energy decays by 0.5 per hop, firing threshold at 0.3. Activated concepts collected into a `LocalActivationMap` per session.
- **Action recommendation:** Records action→outcome pairs, queries network for concept associations to recommend actions in similar contexts
- **Alternative path finding:** When direct concept lookup fails, searches for semantically adjacent paths through the graph

### Algorithm Correctness

- **Hebbian update:** The classic Hebb rule is `Δw = η × x_i × x_j` (product of pre/post-synaptic activation). This implementation uses a **fixed additive increment** (+0.05/−0.03) regardless of activation levels. This is a simplification — it captures the "fire together, wire together" principle directionally but loses the activation-magnitude sensitivity. In practice, this means strongly-activated concept pairs strengthen at the same rate as weakly-activated ones. Functionally adequate for a concept graph; not biologically faithful.
- **Exponential decay:** Correct. `2^(-Δt/half_life)` is the standard radioactive/biological decay formula. Half-life parameterization is clean.
- **Spreading activation:** The BFS with energy decay is a legitimate model (related to Anderson's ACT-R). The 0.5 decay factor means activation drops to 12.5% at 3 hops — reasonable attenuation.
- **Bounds:** 2048 concepts / 8192 associations with LRU-style eviction of lowest-scoring entries when full. Prevents unbounded growth. Sound engineering.

### What's Real

- **This IS a Hebbian network, not a counter.** It has nodes, weighted edges, co-activation strengthening, temporal decay, and spreading activation. The graph structure and dynamics are genuine.
- The decay math is correct and biologically motivated
- Spreading activation produces emergent behavior — concepts that are frequently co-activated form strong clusters, while unused associations naturally fade
- Action recommendation based on network traversal is a real learned-behavior mechanism
- Alternative path finding adds robustness when exact concepts aren't found
- User correction support (stronger +0.08/−0.05 adjustments) enables interactive refinement
- Comprehensive test suite (500+ lines) covering strengthening, decay, spreading activation, capacity limits, and action recommendations

### What's Shallow

- **Fixed Δw instead of η×x_i×x_j** — loses activation-magnitude sensitivity. All co-activations are equal.
- **No inhibitory connections** — real neural networks have excitatory AND inhibitory synapses. This network only has positive weights that can decay but never truly inhibit.
- **No competitive learning** — Hebbian networks in neuroscience often include lateral inhibition (winner-take-all). Absent here.
- **Concept creation is string-based** — concepts are keyed by label strings (FNV-1a hashed), not by learned representations. The network learns associations between human-labeled concepts, not self-discovered features.
- **The 0.05/0.03 constants are magic numbers** — no adaptation, no learning rate scheduling, no meta-learning

### Mobile Viability

- **Excellent.** 2048 concepts × ~64 bytes = ~128KB. 8192 associations × ~40 bytes = ~320KB. Total graph: ~450KB. Spreading activation BFS is bounded by graph size. All operations are O(degree × hops), well within mobile budgets.

### Verdict

A legitimate Hebbian association network. Not a fake counter, not a full neural simulation either — it's a pragmatic middle ground. The simplified additive Hebb rule loses some theoretical fidelity but the emergent behavior (cluster formation, decay, spreading activation) is real. This is the component that most genuinely delivers on the "neurons that fire together" promise. It will produce observable learning behavior on a phone. **Grade: B** — real algorithm with acceptable simplifications for mobile.

---

## (E) Pattern Discovery & Self-Categorization — Grade: B+

**Files:** `arc/learning/patterns.rs` (1231 lines), `arc/learning/skills.rs` (1036 lines), `memory/patterns.rs` (554 lines), `memory/feedback.rs` (475 lines), `arc/learning/dreaming.rs` (1257 lines)
**Blueprint Promise (§4.3–§4.5):** Autonomous pattern discovery, self-categorization, skill learning, and "dreaming" exploration.

### Subsystem Analysis

#### Pattern Detection Engine (`arc/learning/patterns.rs`) — Grade: A-

**What it does:** Detects three pattern types from observation streams:

1. **Temporal patterns:** Tracks when events occur by hour-of-day and day-of-week. Uses **Welford's online algorithm** for incremental mean/variance computation — this is the correct numerically-stable algorithm for streaming statistics. Detects time-clustered behaviors (e.g., "user checks email at 9am ± 30min on weekdays").

2. **Sequential patterns:** N-gram chains up to length 5. Tracks transition probabilities between sequential events. Prefix matching for prediction ("after A→B, C follows with probability 0.73"). This is a legitimate Markov-chain-adjacent model.

3. **Contextual patterns:** Correlates events with context (app, battery level, connectivity, location). Detects conditional probabilities ("user launches Spotify when battery > 50% and connected to WiFi").

- **Bayesian confidence:** `confidence_new = (α × confidence_old + hit) / (α + 1)` with α=5.0. This is a proper Bayesian update with a prior strength of 5 observations. Correctly handles cold-start (prior dominates) and convergence (evidence dominates).
- **Aging:** Daily decay factor of 0.98, pruning at confidence < 0.05. Patterns that stop occurring naturally disappear. Half-life ≈ 34 days (ln(0.5)/ln(0.98)).
- **Bounds:** 1024-observation sliding window. Patterns bounded by type-specific limits.
- **Predictions:** Returns actionable predictions with confidence scores and supporting evidence counts.

**Verdict:** This is the **best-implemented component after HNSW**. Correct statistics, proper Bayesian updates, bounded memory, natural aging. Real pattern discovery that will produce genuine insights from usage data.

#### Skill Registry (`arc/learning/skills.rs`) — Grade: B+

**What it does:** Tracks learned multi-step procedures (skills) with reliability metrics and adaptation.

- **Skill model:** Each skill has steps, preconditions, success/failure counts, reliability (success_rate), confidence with Bayesian decay (0.02/day)
- **Skill evolution:** When a skill's reliability drops below threshold, creates adapted variant with lineage tracking (parent_id, version). This is a genuine evolutionary model — skills mutate and compete.
- **Matching:** Tag-based composite scoring (tag_overlap×0.4 + confidence×0.3 + reliability×0.3). Practical for finding relevant skills.
- **Duration tracking:** Exponential Moving Average for execution time estimation — statistically sound.
- **Bounds:** 512 max skills with eviction of lowest-scoring.

**Verdict:** Well-engineered skill tracking with genuine adaptive behavior. The lineage/evolution mechanism is creative and functional. Not self-categorization in the full AI sense, but real learned-procedure management.

#### Memory Patterns (`memory/patterns.rs`) — Grade: B-

**What it does:** Action→outcome association tracking with Hebbian-style learning.

- Strengthens associations on positive outcomes (+0.1), weakens on negative (-0.15). Asymmetric punishment (stronger negative signal) is a deliberate design choice — reasonable for safety-critical learning.
- Temporal co-occurrence discovery: tracks which events follow other events within a time window, discovers frequently co-occurring pairs.
- Prediction based on strongest associations.
- Export/import for persistence.

**Verdict:** Simpler than the Hebbian network but functional. The asymmetric learning rate is a nice touch.

#### Feedback Loop (`memory/feedback.rs`) — Grade: B-

**What it does:** Error→resolution learning system.

- Normalizes error messages by stripping numbers, quoted strings, and paths — reduces "file not found: /foo/bar.txt" and "file not found: /baz/qux.rs" to the same pattern. This is smart deduplication.
- Tracks which resolutions succeed for which error patterns, suggests best resolution by success rate.
- Bounded capacity with eviction.

**Verdict:** Practical error-learning loop. Will genuinely improve repeated-error handling over time.

#### Dreaming Engine (`arc/learning/dreaming.rs`) — Grade: C

**What it does:** 5-phase autonomous exploration framework: Maintenance → ETG Verification → Exploration → Annotation → Cleanup.

- **Safety invariants:** Checks charging state, screen off, battery > threshold, thermal OK. Well-implemented guardrails.
- **Session lifecycle:** Clean state machine with phases, timeouts, and abort conditions.
- **Capability gap tracking:** Identifies things AURA couldn't do and prioritizes exploration around those gaps.
- **App allowlist:** Only explores pre-approved apps. Good safety design.

**BUT:** The actual exploration execution — the part where AURA would autonomously interact with apps, discover UI elements, and learn new capabilities — **is not implemented**. The engine is an orchestration skeleton. The `execute_exploration_step()` equivalent is TODO/placeholder. This is a framework without the core payload.

**Verdict:** Excellent architectural scaffolding (B+ for the framework), but the soul of the feature — autonomous app interaction and learning — doesn't exist yet. The safety and lifecycle code is real; the intelligence is missing. **Grade: C** — half-built.

### Overall Pattern Discovery Verdict

The pattern detection engine and skill registry are genuinely strong (A-/B+). These will produce real, observable learning behavior from day one. The dreaming engine is promising architecture without implementation. The memory patterns and feedback systems are solid supporting pieces. The "self-categorization" promise is partially delivered — patterns categorize themselves via Bayesian confidence, skills evolve via lineage, but there's no unsupervised clustering discovering novel categories from raw data. **Overall: B+** — more real than not, with the pattern engine being genuinely impressive.

---

## (F) Mobile Viability — Grade: B

### Memory Budget Analysis (targeting 15MB idle / 25MB peak RSS per Blueprint §6.3)

| Component | Estimated RSS | Notes |
|-----------|---------------|-------|
| Working Memory | ~600KB | 1024 slots × 384-dim f32 + metadata |
| Episodic (SQLite) | ~2-4MB | SQLite WAL + page cache, depends on episode count |
| Semantic (SQLite + HNSW) | ~4-8MB | FTS5 index + HNSW graph (10K nodes ≈ 4MB) |
| Archive (SQLite) | ~1-2MB | Cold storage, minimal page cache |
| Hebbian Network | ~450KB | 2048 concepts + 8192 associations |
| Pattern Engine | ~200KB | Bounded observation windows + pattern storage |
| Skill Registry | ~100KB | 512 skills with metadata |
| Embeddings Cache | ~1.5MB | 1024 cached vectors × 384 × f32 |
| **Total Estimated** | **~10-17MB** | **Plausible within 15-25MB target** |

### Latency Analysis

| Operation | Estimated Latency | Concern Level |
|-----------|-------------------|---------------|
| Working memory query | <1ms (1024 cosine sims) | Low |
| Episodic query (1K episodes) | ~5ms | Medium |
| Episodic query (10K episodes) | ~50ms | **High** — O(n) linear scan |
| Semantic HNSW search | <5ms | Low |
| Semantic FTS5 search | <10ms | Low |
| Semantic RRF fusion | <15ms combined | Low |
| Hebbian spreading activation | <1ms | Low |
| Pattern prediction | <1ms | Low |
| Consolidation (Deep/k-means) | ~100-500ms | Medium — background task, acceptable |

### Battery Impact

- **All computation is CPU-only** — no GPU, no neural accelerator. This is good for compatibility but means embedding quality is sacrificed.
- **Consolidation is scheduled** — Micro every 5min (lightweight), Deep every 4hrs (heavy but infrequent). Sensible duty cycling.
- **SQLite WAL mode** — reduces write amplification vs rollback journal. Good for flash storage longevity.
- **No persistent background services beyond the daemon** — dreaming only activates during charging/screen-off.

### Bounded Collections Audit

| Collection | Max Size | Eviction Strategy |
|------------|----------|-------------------|
| Working Memory | 1024 slots | Ring buffer (FIFO) |
| Hebbian Concepts | 2048 | Lowest-score eviction |
| Hebbian Associations | 8192 | Lowest-score eviction |
| Pattern Observations | 1024 window | Sliding window |
| Skills | 512 | Lowest-score eviction |
| Embedding Cache | 1024 | LRU |

All collections are bounded. No unbounded growth vectors detected. This is disciplined mobile engineering.

### Critical Mobile Concerns

1. **Episodic O(n) scan** is the biggest mobile risk. At 10K episodes, query latency exceeds 50ms — noticeable in UI interactions. This MUST be fixed with an HNSW index (the infrastructure exists, just not wired up).
2. **Visited array in HNSW** allocates `vec![false; n]` per search — transient but creates GC pressure on frequent queries.
3. **No memory-mapped I/O** for SQLite — relies on SQLite's built-in page cache. Fine for <100MB databases but could use `mmap` for larger deployments.
4. **Consolidation during charging-only** is smart but means the system may accumulate unconsolidated data during heavy offline use.

### Verdict

The memory budgets are reasonable and achievable. All collections are properly bounded. The embedding approach (TF-IDF hash) is inherently mobile-friendly. The primary risk is episodic memory's O(n) scan degrading at scale. **Grade: B** — solid mobile discipline with one significant performance time-bomb.

---

## (G) Blueprint vs Reality — Gap Analysis

| Blueprint Claim (Section) | Code Reality | Gap Severity |
|---------------------------|-------------|--------------|
| §4.1: "ZSTD compressed" archive | Byte-level RLE (essentially no compression) | **High** — archive will be 3-5× larger than planned |
| §4.1: "seqlock concurrency" for Working Memory | Standard Rust data structures | **Medium** — functional but not lock-free |
| §4.1: "384-dim embeddings from 4B model" (Deep consolidation) | TF-IDF sign-hash, `embed_neural()` returns `None` | **High** — no neural embeddings exist |
| §4.1.4: HNSW + FTS5 + RRF for semantic search | ✅ Correctly implemented | None |
| §4.1.3: Consolidation invokes Neocortex for summaries | Extractive summaries (first N chars) | **High** — no LLM summarization |
| §4.2: Hebbian association network | ✅ Real implementation with simplifications | **Low** — additive vs multiplicative Hebb |
| §4.3: Pattern discovery with Bayesian updating | ✅ Correctly implemented | None |
| §4.4: Skill evolution with lineage | ✅ Correctly implemented | None |
| §4.5: Dreaming exploration | Framework only, no execution logic | **High** — 0% of core feature implemented |
| §6.3: "Daemon RSS idle 15MB, peak 25MB" | Estimated 10-17MB, plausible | **Low** — achievable |
| §6.3: "~18MB/year" episodic storage | Plausible with SQLite WAL | **Low** |

### Gap Summary

- **4 High-severity gaps:** ZSTD compression, neural embeddings, LLM summarization, dreaming execution
- **1 Medium-severity gap:** Seqlock concurrency
- **2 Low-severity gaps:** Hebb rule simplification, RSS target
- **4 fulfilled promises:** HNSW+FTS5+RRF, Hebbian network, pattern discovery, skill evolution

The blueprint is approximately **60% delivered** on technical promises. The unfulfilled 40% concentrates on features requiring external models (neural embeddings, LLM summarization) and the dreaming execution engine.

---

## Overall Verdict

### Is AURA v4's Memory & Learning System REAL or TOY?

**REAL — with caveats.**

This is not a toy. It is not a demo. It is not counters dressed as neural networks. The codebase contains approximately 13,000 lines of Rust implementing genuine algorithms:

- A **correct HNSW implementation** (B+) that provides real sub-linear similarity search
- A **working Hebbian network** (B) with real emergent learning behavior from co-activation, decay, and spreading activation
- A **strong pattern discovery engine** (A-) with correct Bayesian statistics, Welford's algorithm, and temporal/sequential/contextual detection
- A **solid 4-tier memory hierarchy** (B-) with proper neuroscience-inspired consolidation, pattern separation, and knowledge generalization
- **Disciplined mobile engineering** (B) with bounded collections, no unbounded growth, and reasonable memory budgets

### What's Genuinely Impressive

1. **The pattern engine** (`arc/learning/patterns.rs`) — best-in-class for a mobile system. Correct Bayesian updates, Welford's streaming stats, proper aging. Will produce genuine insights.
2. **HNSW** — a real implementation of a real algorithm. Not a wrapper around a library — hand-written with correct multi-layer navigation.
3. **Semantic memory's hybrid search** — HNSW + FTS5 + RRF is how production search engines work. Correctly architected.
4. **Hebbian network's emergent behavior** — concepts genuinely cluster through use, fade through neglect, and activate through association. This IS "neurons that fire together wire together."
5. **Skill evolution with lineage** — creative and functional adaptation mechanism.

### What's Marketing vs Engineering

1. **"Embeddings"** — marketed as semantic understanding, actually TF-IDF feature hashing. Will fail on paraphrase, synonymy, and cross-vocabulary search. The neural path is a stub.
2. **"Dreaming"** — the safety framework is real; the actual autonomous exploration intelligence is 0% implemented.
3. **"ZSTD compression"** — doesn't exist. RLE on text is a no-op.
4. **"Neocortex-powered summarization"** — not wired up. Summaries are first-N-characters extraction.

### Composite Grade

| Area | Grade | Weight | Weighted |
|------|-------|--------|----------|
| (A) Embeddings | C+ | 15% | 0.345 |
| (B) HNSW | B+ | 15% | 0.495 |
| (C) 4-Tier Memory | B- | 25% | 0.675 |
| (D) Hebbian Learning | B | 15% | 0.450 |
| (E) Pattern Discovery | B+ | 20% | 0.660 |
| (F) Mobile Viability | B | 10% | 0.300 |
| **Composite** | | **100%** | **B (2.925/4.0)** |

### Final Assessment

**AURA v4 is a B-grade system** — solidly above average, with genuine algorithmic substance, but with critical gaps in embeddings quality, compression, and unfinished features (dreaming, neural path). The system will produce real, observable learning behavior on a mobile device. Users will see patterns detected, associations formed, and skills learned. The memory hierarchy will function and consolidate.

What it will NOT do: understand paraphrased text, generate abstractive summaries, autonomously explore apps, or compress archived memories efficiently. These are the gaps between the blueprint's vision and the code's reality.

**The founder's vision of "neurons that fire, self-categorization, organic growth" is approximately 65% realized.** The neurons fire (Hebbian). The patterns self-categorize (Bayesian confidence + aging). The organic growth is bounded but real (skill evolution, knowledge generalization). What's missing is the semantic depth that only neural embeddings can provide and the autonomous exploration that only a completed dreaming engine can deliver.

**Recommendation:** Ship what works (HNSW, Hebbian, patterns, 4-tier memory). Prioritize wiring HNSW into episodic queries (quick win, high impact). Replace RLE with ZSTD (trivial). Accept TF-IDF embeddings as "good enough for V4" with a clear V5 roadmap for on-device neural embeddings. Do NOT market dreaming as a feature until execution logic exists.
