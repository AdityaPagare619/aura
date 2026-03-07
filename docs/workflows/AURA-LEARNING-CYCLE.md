# AURA Learning Cycle

How AURA improves over time through execution, reinforcement, and consolidation.

## Overview

AURA's learning is a continuous cycle: **execute → observe → reinforce → consolidate → cache**. Over days and weeks, novel tasks (expensive System2/LLM) become cached patterns (free System1/ETG), and the agent becomes faster, more reliable, and cheaper to operate.

```
         ┌──────────────────────────────────────────┐
         │           THE LEARNING FLYWHEEL           │
         │                                           │
         │  Novel Task ──► System2 (LLM plans it)   │
         │       │                                   │
         │       ▼                                   │
         │  Execute ──► Record in ETG + Patterns     │
         │       │                                   │
         │       ▼                                   │
         │  Repeat ──► ETG reliability grows         │
         │       │                                   │
         │       ▼                                   │
         │  Threshold ──► System1 cache hit (0.70)   │
         │       │                                   │
         │       ▼                                   │
         │  Same Task ──► System1 (<10ms, no LLM)   │
         │                                           │
         └──────────────────────────────────────────┘
```

## Learning Mechanisms

### 1. ETG Edge Reinforcement

**File:** `crates/aura-daemon/src/execution/etg.rs`

Every time an action step succeeds or fails, the corresponding ETG edge is updated:

**On success:**
- Edge `success_count` incremented
- `raw_reliability = successes / total_attempts`
- `last_success` timestamp updated (resets freshness decay)

**On failure:**
- Edge `failure_count` incremented
- `raw_reliability` decreases
- If `effective_reliability` drops below 0.3, edge is pruned

**Effective reliability with time decay:**

```
effective_reliability = raw_reliability × 2^(-days_since_last_success / 14)
```

Example evolution of an edge over time:

```
Day  Action          Raw    Freshness  Effective  Cached?
───  ──────────────  ─────  ─────────  ─────────  ───────
 1   First success   1.00   1.00       1.00       No (need more data)
 3   Second success  1.00   0.87       0.87       Yes (≥0.70)
 7   Third success   1.00   0.71       0.71       Yes
10   Failure          0.75   0.61       0.46       No (dropped below)
12   Success          0.80   0.55       0.44       No
14   Success          0.83   1.00       0.83       Yes (freshness reset)
28   No usage         0.83   0.25       0.21       No (pruned at <0.3)
```

### 2. Hebbian Pattern Reinforcement

**File:** `crates/aura-daemon/src/memory/patterns.rs`

Operates on abstract action patterns (not specific ETG edges):

```
Pattern: "open_app → navigate_to_section → interact_with_element"

Success: strength += 0.10    (reinforcement)
Failure: strength -= 0.15    (asymmetric penalty)

Lifecycle:
  Created at strength 0.0
  → 7 successes → 0.70 (reliable pattern)
  → 3 failures  → 0.25 (weakened)
  → Unused 7 days + strength < 0.05 → PRUNED
```

**Temporal pattern example:**
```
Pattern: "user checks email → user opens calendar" (within 5 min window)
  Observed 15 times → strength 0.85
  → AURA pre-loads calendar context when email is opened
```

### 3. Consolidation Cycle

**File:** `crates/aura-daemon/src/memory/consolidation.rs`

Runs in the background, promoting valuable episodic memories to durable semantic knowledge:

```
┌───────────────────────────────────────────────────────┐
│                  CONSOLIDATION FLOW                    │
│                                                        │
│  Episodic Memory (raw experiences)                     │
│       │                                                │
│       ▼ [Light consolidation, every ~15 min]           │
│  Dedup + score                                         │
│  score = recency(0.3) + frequency(0.3) + importance(0.4)│
│       │                                                │
│       ├── Low score ──► Candidate for archival          │
│       └── High score ──► Candidate for promotion        │
│                                                        │
│       ▼ [Deep consolidation, idle + charging]           │
│  k-means clustering (k=8, 10 iterations)               │
│  on episode embedding vectors                           │
│       │                                                │
│       ▼                                                │
│  Cluster analysis:                                      │
│  ├── Dense cluster ──► Extract semantic fact            │
│  │   "User prefers dark mode in all apps"              │
│  │   → Promote to Semantic tier                         │
│  │                                                     │
│  └── Sparse cluster ──► Keep as episodic                │
│      (not enough evidence to generalize)                │
│                                                        │
│       ▼ [Emergency, on memory pressure]                 │
│  Aggressive archival: old episodes → ZSTD compressed    │
│  Free 2.8-3.6MB immediately                             │
└───────────────────────────────────────────────────────┘
```

### 4. Error→Learning Feedback

**File:** `crates/aura-daemon/src/memory/mod.rs` (MemoryIntelligence)

When an action fails, the error doesn't just weaken patterns — it generates learning:

```
Action Failed
    │
    ├── ETG: edge reliability decreased
    ├── Patterns: Hebbian -0.15
    ├── Episodic: failure event recorded with context
    │
    └── MemoryIntelligence:
        ├── Analyze: why did it fail? (element not found, wrong screen, etc.)
        ├── Pattern discovery: is this failure correlated with other failures?
        └── Spreading activation: boost related memories for next attempt
```

### 5. ReAct Reflection Loop

**File:** `crates/aura-daemon/src/daemon_core/react.rs`

After each action step:

```
reflection_score = base_confidence
                 + screen_change_bonus    (0.1-0.3 if UI changed)
                 + element_bonus          (0.1-0.2 if target found)
                 - strategy_penalty       (higher strategy = less confident)
```

This score directly feeds into ETG edge updates and Hebbian pattern adjustments, creating tight feedback between execution and learning.

## Learning Over Time

### Week 1 (Cold Start)
- ETG is empty. All tasks route to System2
- Every execution records new ETG nodes and edges
- Pattern engine begins accumulating action sequences
- Consolidation has little to do (few episodes)

### Week 2-4 (Warm Up)
- Common tasks (open app, send message) have ETG paths above 0.70
- System1 handles 30-50% of tasks
- Temporal patterns emerging (morning routine, work patterns)
- Light consolidation deduplicating similar episodes

### Month 2-3 (Steady State)
- System1 handles ~80% of tasks
- Deep consolidation has promoted key patterns to semantic memory
- ETG has stable, high-reliability paths for routine operations
- Novel tasks still go to System2, enriched by semantic context

### Month 6+ (Mature)
- ETG well-populated with reliable paths
- Stale edges auto-pruned (14-day freshness half-life)
- Pattern engine at capacity (2048 action, 512 temporal)
- Weak patterns continuously pruned, replaced by stronger ones
- System1/System2 ratio stabilized around 80/20

## Capacity Bounds

| Resource | Limit | Eviction |
|----------|-------|----------|
| ETG nodes | 10,000 | LRU |
| ETG edges | 50,000 | LRU + reliability prune (<0.3) |
| Action patterns | 2,048 | Strength prune (<0.05 after 7d) |
| Temporal patterns | 512 | Same as action patterns |
| System1 plan cache | 256 | LRU |
| Working memory | 1,024 slots | Ring buffer (oldest evicted) |
| HNSW index | Unbounded* | Lazy deletion (tombstones) |

*HNSW cleaned during deep consolidation.

## Key Insight: The Virtuous Cycle

```
More usage → More ETG data → Higher reliability → More System1 hits
    → Less LLM cost → Faster responses → Better user experience
    → More usage → ...
```

The critical transition: when a task's ETG path reliability crosses **0.70**, it shifts from System2 (1-30s, LLM cost) to System1 (<10ms, zero cost). This is the fundamental learning dividend.

## References

- `crates/aura-daemon/src/execution/etg.rs` — ETG store, BFS, reliability scoring
- `crates/aura-daemon/src/memory/patterns.rs` — Hebbian learning, pattern lifecycle
- `crates/aura-daemon/src/memory/consolidation.rs` — 4-level consolidation
- `crates/aura-daemon/src/memory/mod.rs` — MemoryIntelligence, error→learning
- `crates/aura-daemon/src/daemon_core/react.rs` — Reflection scoring, strategy escalation
- `crates/aura-daemon/src/routing/system1.rs` — Plan cache, 0.70 threshold
