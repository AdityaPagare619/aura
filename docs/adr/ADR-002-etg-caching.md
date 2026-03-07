# ADR-002: Execution Trace Graph (ETG) for Action Caching

**Status:** Accepted  
**Date:** 2026-03-01  
**Deciders:** AURA Core Team

## Context

AURA automates UI tasks on Android by performing sequences of accessibility actions (tap, type, scroll, etc.). Many tasks recur — "open WhatsApp", "set alarm for 7am", "turn on WiFi". Without caching, every occurrence requires either:

1. Full LLM planning (1-30s, high battery cost), or
2. Brittle hardcoded scripts that break on app updates

We need a caching mechanism that:
- Learns from successful executions automatically
- Degrades gracefully when UIs change (app updates)
- Self-heals by pruning unreliable paths
- Stays bounded in memory on a mobile device

## Decision

Implement an **Execution Trace Graph (ETG)** — a directed graph where nodes are UI states and edges are actions, backed by SQLite for persistence and bounded by LRU eviction.

### Graph Structure

```
Types: crates/aura-types/src/etg.rs
Store: crates/aura-daemon/src/execution/etg.rs

    ┌──────────┐   tap(#send_btn)   ┌──────────┐
    │  State A  │──────────────────►│  State B  │
    │ WhatsApp  │   reliability=0.92 │  Chat     │
    │ Home      │   count=47         │  Screen   │
    └──────────┘   last=2026-03-01   └──────────┘
         │                                │
         │  tap(#contacts)                │  type(#msg_input)
         │  reliability=0.85             │  reliability=0.78
         ▼                                ▼
    ┌──────────┐                    ┌──────────┐
    │  State C  │                    │  State D  │
    │ Contacts  │                    │  Msg Sent │
    └──────────┘                    └──────────┘
```

### Key Parameters

| Parameter | Value | Location |
|-----------|-------|----------|
| Max nodes | 10,000 | `execution/etg.rs` |
| Max edges | 50,000 | `execution/etg.rs` |
| Eviction policy | LRU | `execution/etg.rs` |
| Freshness half-life | 14 days | `execution/etg.rs` |
| Prune threshold | 0.3 effective reliability | `execution/etg.rs` |
| BFS max depth | 20 | `execution/etg.rs` |
| Plan cache size | 256 entries | `routing/system1.rs` |
| Cache confidence threshold | 0.70 | `routing/system1.rs` |

### Reliability Scoring

Each edge tracks raw reliability (success/total ratio) and a time-decayed effective reliability:

```
effective_reliability = raw_reliability × freshness_factor
freshness_factor = 2^(-days_since_last_success / 14)
```

This models the real-world observation that UI paths become less reliable over time as apps update. A path used successfully yesterday has a freshness_factor of ~0.95; one unused for 28 days has ~0.25 and gets pruned.

```
Reliability over time (14-day half-life):

1.0 │****
    │    ****
0.8 │        ***
    │           ***
0.6 │              **
    │                **
0.4 │                  **
    │                    **
0.3 │─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─** ─ ─ prune threshold
0.2 │                        **
    │                          ***
0.0 └──────────────────────────────►
    0    7    14    21    28   days
```

### Plan Sources

Plans fed into the execution engine come from four sources (`aura-types/src/etg.rs`):

| Source | Meaning |
|--------|---------|
| `EtgPath` | BFS path through the graph — System1, zero LLM cost |
| `LlmGenerated` | Neocortex planned the steps — System2 |
| `UserDefined` | User explicitly scripted the action sequence |
| `Merged` | Hybrid: ETG provided partial path, LLM filled gaps |

### Pathfinding

BFS with reliability-weighted scoring, max depth 20. The path score is the product of edge reliabilities along the path. System1 only executes plans above the 0.70 confidence threshold.

### Persistence

- In-memory graph for fast lookups during execution
- SQLite backend for persistence across daemon restarts
- On startup, graph is restored from SQLite (Phase 4: Checkpoint restore, <50ms)

## Consequences

### Positive

- **Zero-cost repeat tasks:** Once a task succeeds and its ETG path reliability exceeds 0.70, it executes via System1 with no LLM inference
- **Self-healing:** Reliability decay automatically deprioritizes stale paths. Failed executions reduce edge reliability. Edges below 0.3 are pruned
- **Bounded memory:** Hard caps (10K nodes, 50K edges) with LRU eviction prevent unbounded growth on mobile
- **Incremental learning:** Every successful System2 execution adds/reinforces ETG edges, growing System1's coverage over time

### Negative

- **Cold start:** New installations have an empty ETG. All tasks route to System2 initially. Performance improves over days/weeks of use
- **State representation:** UI states must be fingerprinted consistently. App updates that change screen structure can invalidate state nodes
- **Graph maintenance cost:** Pruning, eviction, and SQLite sync add background overhead (mitigated by consolidation scheduling)

## Alternatives Considered

### 1. Flat Action Sequence Cache (HashMap<TaskHash, Vec<Action>>)
- **Rejected:** No partial matching. If step 3 of 5 changes, the entire cached sequence is useless. ETG allows BFS to find alternative paths through shared intermediate states.

### 2. Decision Tree / Rule Engine
- **Rejected:** Requires manual rule authoring. ETG learns automatically from successful executions. Rules also can't handle the combinatorial explosion of app states.

### 3. Full LLM Every Time (No Caching)
- **Rejected:** Unacceptable latency (1-30s per task) and battery drain for common operations. Users would abandon the agent.

### 4. Macro Recording (Exact Replay)
- **Rejected:** Brittle — any UI change (different screen resolution, updated app layout, changed element IDs) breaks the macro. ETG's multi-level selector cascade (L0-L7) and reliability scoring handle UI drift gracefully.

## References

- `crates/aura-types/src/etg.rs` — EtgNode, EtgEdge, ActionPlan, PlanSource types
- `crates/aura-daemon/src/execution/etg.rs` — EtgStore, BFS pathfinding, SQLite persistence, LRU eviction
- `crates/aura-daemon/src/routing/system1.rs` — Plan cache, confidence threshold
- `crates/aura-daemon/src/daemon_core/react.rs` — Execution modes consuming ETG plans
