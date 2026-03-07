# ADR-006: Bio-Inspired Learning (Hebbian Patterns + Consolidation)

**Status:** Accepted  
**Date:** 2026-03-01  
**Deciders:** AURA Core Team

## Context

AURA must improve over time without retraining its LLM. The agent needs to:

1. Strengthen action patterns that succeed and weaken those that fail
2. Discover temporal co-occurrences (user always checks email after calendar)
3. Consolidate fragmented episodic memories into durable semantic knowledge
4. Manage memory pressure on a mobile device with limited RAM/storage
5. Do all of this autonomously — no user intervention for "training"

Classical ML approaches (fine-tuning, gradient descent) require labeled datasets and compute budgets inappropriate for on-device real-time learning. We need a lightweight, biologically-inspired approach.

## Decision

Implement two complementary bio-inspired mechanisms:

1. **Hebbian pattern learning** for action/temporal pattern reinforcement
2. **Sleep-stage consolidation** for memory management and knowledge promotion

### Hebbian Pattern Learning

**Location:** `crates/aura-daemon/src/memory/patterns.rs`

"Neurons that fire together wire together" — patterns that co-occur with success get stronger; those associated with failure weaken.

```
  Action Executed
       │
       ├── Success ──► pattern.strength += 0.10
       │
       └── Failure ──► pattern.strength -= 0.15  (asymmetric!)
       
  ┌─────────────────────────────────────────┐
  │         Hebbian Feedback Loop            │
  │                                          │
  │  ┌────────┐    ┌──────────┐    ┌──────┐ │
  │  │Execute │───►│ Observe  │───►│Score │ │
  │  │Action  │    │ Result   │    │      │ │
  │  └────────┘    └──────────┘    └──┬───┘ │
  │       ▲                           │     │
  │       │    ┌──────────────┐       │     │
  │       └────│ Update       │◄──────┘     │
  │            │ Pattern      │             │
  │            │ Strengths    │             │
  │            └──────────────┘             │
  └─────────────────────────────────────────┘
```

**Key design choices:**

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Success reinforcement | +0.10 | Moderate positive signal |
| Failure penalty | -0.15 | **Asymmetric** — failures punished harder than successes rewarded. Prevents reinforcing unreliable patterns |
| Max action patterns | 2,048 | Memory bound for mobile |
| Max temporal patterns | 512 | Temporal co-occurrences are sparser |
| Max recent events | 256 | Sliding window for pattern detection |
| Prune threshold | strength < 0.05 | Dead patterns cleaned up |
| Prune age | after 7 days | Only prune if pattern hasn't been reinforced recently |

**Asymmetric reinforcement rationale:** A pattern that succeeds 60% of the time would slowly strengthen under symmetric ±0.10. With asymmetric -0.15/+0.10, it would weaken — which is correct, because a 60% reliable automation pattern is not reliable enough to cache for autonomous execution.

### Temporal Pattern Discovery

The PatternEngine also tracks temporal co-occurrences:
- When action B consistently follows action A within a time window, a temporal pattern forms
- Enables predictive pre-loading ("user usually opens Maps after checking Calendar")
- Bounded to 512 patterns with the same Hebbian strengthening/weakening

### Sleep-Stage Consolidation

**Location:** `crates/aura-daemon/src/memory/consolidation.rs`

Inspired by neuroscience research on memory consolidation during sleep stages:

```
  ┌─────────────────────────────────────────────────────┐
  │               Consolidation Levels                   │
  │                                                      │
  │  Micro     Light        Deep          Emergency      │
  │  (<1ms)    (≤60s)       (≤30min)      (<5s)         │
  │                                                      │
  │  ┌─────┐   ┌────────┐   ┌───────────┐  ┌─────────┐ │
  │  │Slot │   │Dedup   │   │k-means    │  │Aggressive│ │
  │  │rotat│   │Weak    │   │clustering │  │sweep +   │ │
  │  │ion  │   │pattern │   │k=8, 10iter│  │archive   │ │
  │  │     │   │prune   │   │Promote to │  │Free 2.8- │ │
  │  │     │   │        │   │semantic   │  │3.6MB     │ │
  │  └─────┘   └────────┘   └───────────┘  └─────────┘ │
  │    ▲          ▲             ▲              ▲        │
  │    │          │             │              │        │
  │  Always    Periodic     Idle/charge    Memory      │
  │  inline    timer        detected       pressure    │
  └─────────────────────────────────────────────────────┘
```

| Level | Time Budget | Trigger | What It Does |
|-------|-------------|---------|--------------|
| **Micro** | <1ms | Inline during normal ops | Working memory slot rotation. Evicts oldest entries as new ones arrive |
| **Light** | ≤60s | Periodic timer | Deduplicates episodic memories. Prunes weak patterns (strength < 0.05 AND age > 7 days) |
| **Deep** | ≤30min | Device idle + charging | k-means clustering (k=8, 10 iterations) on episode embeddings. Identifies clusters of related episodes, promotes frequent patterns to semantic memory |
| **Emergency** | <5s | Memory pressure signal | Aggressive sweep: archives old episodic entries with ZSTD compression, prunes below-threshold patterns. Frees 2.8-3.6MB |

### Consolidation Scoring

Memories are scored for retention priority:

```
score = recency × 0.3 + frequency × 0.3 + importance × 0.4

recency:    exponential decay, 7-day half-life
frequency:  access count, normalized
importance: event-assigned (e.g., user-initiated = high, background = low)
```

Low-scoring memories are candidates for archival (episodic→archive) or pruning.

### MemoryIntelligence Facade

**Location:** `crates/aura-daemon/src/memory/mod.rs`

Sits on top of the memory system and provides higher-level cognitive functions:

- **Pattern discovery:** Feeds execution results into PatternEngine
- **Error→learning feedback:** Failed actions generate learning signals that update both ETG edge weights and Hebbian pattern strengths
- **Spreading activation:** When a memory is accessed, related memories get a small activation boost, improving recall of contextually relevant information

### ReAct Engine Reflection

**Location:** `crates/aura-daemon/src/daemon_core/react.rs`

After each action step, the ReAct engine computes a reflection score:

```
reflection = base_confidence
           + screen_change_bonus    (UI changed → action had effect)
           + element_bonus          (target element found/interacted)
           - strategy_penalty       (higher strategies = less confident)
```

This score feeds back into:
- ETG edge reliability (reinforces/weakens the action path)
- Hebbian pattern strength (reinforces/weakens the action pattern)
- AgenticSession history (bounded to 32 entries, max 10 iterations, max 5 consecutive failures)

## Consequences

### Positive

- **Autonomous improvement:** No manual training, labeling, or user intervention. AURA gets better just by being used
- **Biologically sound:** Asymmetric reinforcement matches how biological neural networks handle reward/punishment. Consolidation mirrors known memory processes
- **Resource-aware:** Emergency consolidation prevents OOM. Tiered consolidation runs expensive operations only when the device is idle and charging
- **Self-pruning:** Dead patterns (unused for 7+ days, low strength) are automatically cleaned up

### Negative

- **Slow convergence:** Hebbian learning with ±0.10/0.15 increments requires many repetitions. A pattern needs ~7 consecutive successes to reach 0.70 strength from zero
- **No transfer learning:** Patterns learned for one app don't transfer to similar apps. Each app's UI is learned independently
- **k-means limitations:** Fixed k=8 may not match natural cluster count. Some consolidation cycles may produce poor clusters

## Alternatives Considered

### 1. On-Device Fine-Tuning (LoRA/QLoRA)
- **Rejected:** Even efficient fine-tuning requires GPU compute, training data management, and risks catastrophic forgetting. Too heavy for continuous on-device learning.

### 2. Reinforcement Learning (Q-Learning, PPO)
- **Rejected:** RL requires reward function design, exploration strategy, and significant compute for policy updates. Hebbian learning is simpler, faster, and sufficient for pattern strengthening.

### 3. No Learning (Static Rules + LLM)
- **Rejected:** The LLM handles novel tasks well but can't improve over time. Without caching learned paths, common tasks would always require expensive LLM inference.

### 4. Cloud-Based Learning (Federated Learning)
- **Rejected:** Requires network connectivity, raises privacy concerns (sharing action traces), and adds latency. All learning must be local.

## References

- `crates/aura-daemon/src/memory/patterns.rs` — PatternEngine, Hebbian learning, temporal co-occurrence
- `crates/aura-daemon/src/memory/consolidation.rs` — 4-level consolidation, k-means clustering
- `crates/aura-daemon/src/memory/mod.rs` — MemoryIntelligence facade, spreading activation
- `crates/aura-daemon/src/daemon_core/react.rs` — Reflection scoring, AgenticSession bounds
- `crates/aura-daemon/src/execution/etg.rs` — ETG edge reliability feedback
