# AURA V4 — RESEARCH FINDINGS
## AI Agent Architectures, Memory Systems & On-Device Optimization
**Date:** 2026-03-05 | **Agent:** Research Intelligence Team (Agent 18)

---

## EXECUTIVE SUMMARY

This research scanned 50+ repositories, documentation sets, and framework architectures across 6 target domains. The AI agent landscape has shifted dramatically since AURA v3's design. Key macro-trends:

1. **Multi-agent orchestration** is now table-stakes — single-agent architectures are being replaced by hierarchical director+worker patterns
2. **Memory has gone tiered** — the industry converged on 3-layer memory (working/core/dynamic) with typed memories and AI-driven extraction
3. **Soul/identity evolution** is an emerging paradigm — agents that can reflect on and evolve their own identity with governance controls
4. **MCP (Model Context Protocol)** has become the dominant tool integration standard (9K+ stars on mcp-use alone)
5. **On-device LLM** has advanced significantly — llama.cpp now supports Snapdragon Hexagon NPU, Adreno GPU via OpenCL, and has React Native/Flutter bindings
6. **Blackboard coordination** is preferred over direct agent-to-agent messaging for multi-agent systems

Below are the **Top 10 Actionable Findings**, ranked by impact on AURA v4.

---

## TOP 10 ACTIONABLE FINDINGS

### #1 — THREE-LAYER MEMORY WITH TYPED EXTRACTION (HIGH PRIORITY)
**Source:** OpenAkita (github.com/openakita/openakita) — 969 stars, Apache 2.0
**What it is:** OpenAkita implements a 3-layer memory system: Working Memory (current task context), Core Memory (user profile, preferences, long-term facts), and Dynamic Retrieval (past experience search). Memories are classified into 7 types: Fact, Preference, Skill, Error, Rule, Persona trait, Experience.

**Relevance to AURA:** AURA v4 already has 4-tier memory (immediate/short/long/permanent), but lacks **typed memory classification** and **AI-driven extraction**. OpenAkita automatically distills valuable information after each conversation and supports multi-path recall (semantic + full-text + temporal + attachment search).

**Actionable Insight:** 
- Add memory type tagging to AURA's HNSW store: tag each memory vector with a type enum (FACT, PREFERENCE, SKILL, ERROR, RULE, PERSONA, EXPERIENCE)
- Implement post-conversation extraction pipeline: after each interaction, run a lightweight prompt that extracts typed memories from the conversation
- Add temporal indexing alongside HNSW — OpenAkita's "multi-path recall" suggests maintaining a time-sorted index for "what happened last Tuesday" queries
- This maps directly to AURA's existing 4-tier system but adds **semantic richness** to retrieval

**Priority:** HIGH — Direct improvement to AURA's core memory subsystem with moderate implementation effort

---

### #2 — TIERED EXPERIENCE CLASSIFICATION + REFLECTION PIPELINE (HIGH PRIORITY)
**Source:** EvoClaw (github.com/slhleosun/EvoClaw) — 158 stars, MIT
**What it is:** A soul evolution framework that classifies experiences into 3 tiers: Routine (standard tasks), Notable (meaningful moments, feedback, insights), Pivotal (rare, high-impact events). Memories flow upward through a reflection pipeline: experience → reflect → evolve. Every soul change traces back through a full provenance chain.

**Relevance to AURA:** AURA v4's Hebbian learning system strengthens connections but doesn't have a **significance classifier** or **reflection pipeline**. EvoClaw's approach is directly analogous to how human memory consolidation works — trivial events are forgotten, significant ones get replayed and integrated into identity.

**Actionable Insight:**
- Add a significance scorer to AURA's dreaming system: when processing memories during "sleep", classify each as routine/notable/pivotal based on (a) emotional valence, (b) user feedback signals, (c) novelty vs existing patterns, (d) outcome impact
- Only run deep consolidation (Hebbian strengthening, pattern extraction) on notable/pivotal memories
- This dramatically reduces compute during "dreaming" — instead of processing ALL memories, only consolidate the ~5-10% that matter
- Add provenance chains: every pattern/skill should trace back to the experiences that created it
- **Maps to AURA's dreaming subsystem** — implement as a pre-filter before Hebbian weight updates

**Priority:** HIGH — Directly improves dreaming efficiency and learning quality

---

### #3 — HIERARCHICAL MULTI-AGENT WITH DIRECTOR PATTERN (HIGH PRIORITY)
**Source:** ClawSwarm (github.com/The-Swarm-Corporation/ClawSwarm) — 39 stars, MIT + OpenAkita architecture
**What it is:** A hierarchical multi-agent pattern where a **Director** agent receives all tasks, creates execution plans, and delegates to specialist **Worker** agents. The Director never executes tools itself — it only plans and orchestrates. Workers are specialists (Response, Search, Code, etc.) with focused roles and limited tool access. A **Summarizer** agent condenses combined output.

**Relevance to AURA:** AURA v4's ETG (Execution Task Graph) + ReAct loop is a single-agent pattern. The Director pattern offers a cleaner separation of concerns: planning vs execution. This maps well to AURA's teacher stack — the cloud LLM could act as Director, the local model as Worker.

**Actionable Insight:**
- Restructure AURA's execution engine: separate planning (which tool sequence to use) from execution (actually running tools)
- The **planning phase** runs on the teacher/cloud model (better reasoning), produces a structured task plan (like ClawSwarm's SwarmSpec)
- The **execution phase** runs on local model, following the plan step-by-step with ReAct for error handling
- Add specialist "modes" rather than full agents: AURA-Screen (GUI interaction), AURA-Search (web/info), AURA-File (device operations), AURA-Comm (messaging)
- This reduces local LLM cognitive load — workers get simpler, focused prompts

**Priority:** HIGH — Fundamental architecture improvement that leverages existing teacher stack

---

### #4 — BLACKBOARD COORDINATION PATTERN (MEDIUM-HIGH PRIORITY)
**Source:** OpenClaw Multi-Agent Team (github.com/Richchen-maker/openclaw-multi-agent-team) — 31 stars, MIT
**What it is:** Agents coordinate through a shared **Blackboard** (file-based shared state) instead of direct messaging. The CONDUCTOR decomposes tasks, agents read/write to shared state. Features: Event Bus for cross-team collaboration, priority queues, memory bridge for knowledge sharing, self-evolution via pattern extraction + shortcut matching for repeated tasks.

**Relevance to AURA:** AURA's current architecture has the execution engine directly invoking tools. The Blackboard pattern offers a cleaner way to manage complex multi-step tasks — each step writes its output to a shared state, subsequent steps read from it. The **shortcut matching** concept is especially relevant: if AURA detects it's doing a task similar to a past success, skip intermediate planning steps.

**Actionable Insight:**
- Implement a lightweight Blackboard in AURA's task execution: a JSON/SQLite scratchpad where each execution step writes intermediate results
- Add **pattern extraction from successful executions**: when a multi-step task succeeds, extract the action sequence as a reusable "shortcut"
- Store shortcuts in the skill library — next time a similar request comes in, skip planning and execute the shortcut directly
- This is conceptually similar to AURA's "macro recording" but automated: every successful execution potentially becomes a new skill
- **Event Bus** concept maps to AURA's notification system — when one subsystem detects something relevant, broadcast it

**Priority:** MEDIUM-HIGH — Adds execution learning and coordination infrastructure

---

### #5 — RUNTIME SUPERVISION + RESOURCE BUDGETS (MEDIUM-HIGH PRIORITY)
**Source:** OpenAkita — runtime supervision system
**What it is:** OpenAkita has a dedicated RuntimeSupervisor that detects: tool thrashing (agent calling same tool repeatedly), reasoning loops (going in circles), token anomalies, and enforces resource budgets (token/cost/duration/iteration/tool call limits per task). A policy engine (POLICIES.yaml) controls tool permissions, shell command blocklists, and path restrictions.

**Relevance to AURA:** AURA v4's ReAct loop doesn't have built-in loop detection or resource budgeting. On a mobile device with limited battery and compute, this is critical. An agent stuck in a reasoning loop burns battery and RAM for nothing.

**Actionable Insight:**
- Add a supervision layer to AURA's ReAct engine:
  - **Loop detection**: Track action history; if the same action+params appear 3 times, break and try alternative
  - **Token budget**: Set per-task token limits based on complexity estimate; kill tasks exceeding 2x budget
  - **Tool thrashing detection**: If >5 consecutive tool calls with no meaningful state change, escalate to user or try different approach
  - **Execution timeout**: Hard 60-second timeout per ReAct step, 5-minute timeout per task
- Add POLICIES.yaml equivalent for AURA: define which tools require user confirmation (e.g., sending messages, making purchases, deleting files)
- This is **critical for mobile safety** — prevents runaway inference that drains battery

**Priority:** MEDIUM-HIGH — Safety and efficiency feature essential for on-device operation

---

### #6 — MCP (MODEL CONTEXT PROTOCOL) INTEGRATION (MEDIUM PRIORITY)
**Source:** mcp-use (github.com/mcp-use/mcp-use) — 9,371 stars, TypeScript
**What it is:** MCP has become the de facto standard for LLM tool integration. mcp-use is a full framework for building MCP apps, providing a unified gateway for connecting AI agents to external tools/services. The OpenClaw ecosystem is built on MCP for tool extensibility.

**Relevance to AURA:** AURA v4 currently has a custom tool system (ETG + action definitions). MCP compatibility would allow AURA to tap into the rapidly growing ecosystem of MCP servers — hundreds of pre-built integrations (GitHub, file systems, databases, web search, etc.) without writing custom tool adapters.

**Actionable Insight:**
- Add an MCP client layer to AURA's tool system: when AURA needs a tool not in its built-in set, check for available MCP servers
- This doesn't replace AURA's existing tool system (A11y service, screen interaction) — those remain native
- MCP becomes the **extensibility layer**: users install MCP servers for specific integrations
- On Android, this could be a background service that hosts MCP servers for device APIs (contacts, calendar, files)
- Long-term: AURA's own capabilities could be exposed AS an MCP server for other agents

**Priority:** MEDIUM — Strategic extensibility play, not immediately critical but high future value

---

### #7 — LLAMA.CPP SNAPDRAGON HEXAGON + ADRENO BACKENDS (MEDIUM PRIORITY)
**Source:** llama.cpp (github.com/ggml-org/llama.cpp) — official repo
**What it is:** llama.cpp now has two mobile-specific GPU backends in development:
- **Hexagon backend** (in progress) — targets Qualcomm Snapdragon NPU (Hexagon DSP), which is present in most flagship Android devices
- **OpenCL backend** — targets Adreno GPU, also Qualcomm
- Additionally: 1.5-bit to 8-bit quantization, speculative decoding for faster inference, multimodal support

**Relevance to AURA:** AURA v4 currently runs llama.cpp on CPU (ARM NEON). Moving to Hexagon NPU or Adreno GPU would provide 3-10x speedup for inference on Snapdragon devices, which are the majority of flagship Android phones.

**Actionable Insight:**
- **Immediate**: Track the Hexagon backend development; when it stabilizes, integrate as the primary inference backend for Snapdragon 8 Gen 2+ devices
- **Immediate**: Test OpenCL/Adreno backend for batch embedding operations (memory retrieval) — GPU is often idle during agent operation
- Use **speculative decoding** for the teacher stack: when cloud is unavailable, use a tiny draft model (Q2-quantized) + main model for faster local generation
- Investigate 1.5-bit quantization for the always-on "fast check" model that handles simple queries without spinning up the full model
- Flutter/React Native bindings mean AURA's UI layer could integrate with llama.cpp directly

**Priority:** MEDIUM — Performance improvement, dependent on upstream development timeline

---

### #8 — SOUL DOCUMENT WITH GOVERNED EVOLUTION (MEDIUM PRIORITY)
**Source:** EvoClaw — soul management framework
**What it is:** EvoClaw structures agent identity into canonical sections (Personality, Philosophy, Boundaries, Continuity) with tagged beliefs: `[CORE]` (immutable foundations) and `[MUTABLE]` (beliefs that evolve through reflection). Three governance levels: Autonomous (auto-apply), Supervised (review next session), Gated (explicit approval required). All changes require provenance chains.

**Relevance to AURA:** AURA v4 has personality and learning but no structured **identity document** with immutable vs mutable beliefs. The governance model is especially important — users should control how much their AURA can change about itself.

**Actionable Insight:**
- Create a SOUL.md equivalent for AURA: structured identity document with sections for core personality, user-specific adaptations, learned preferences
- Tag core behaviors as immutable (safety rules, privacy protections, core personality traits)
- Tag adaptive behaviors as mutable (communication style, humor level, proactivity, tool preferences)
- Add governance settings to AURA's config: "How much should AURA evolve?" with Autonomous/Supervised/Gated modes
- During "dreaming", the reflection pipeline proposes identity updates based on accumulated experiences
- **Maps to AURA's Hebbian learning** — instead of just strengthening connections, also propose personality/behavior changes

**Priority:** MEDIUM — Important for long-term user trust and agent coherence

---

### #9 — SELF-EVOLUTION + FAILURE ROOT CAUSE ANALYSIS (MEDIUM PRIORITY)
**Source:** OpenAkita — self-evolution system
**What it is:** OpenAkita runs a daily self-check at 04:00: analyzes error logs, performs AI-driven diagnosis, attempts auto-fix, and pushes a report. After failures, it performs root cause analysis categorizing failures as: context loss, tool limitation, reasoning loop, or budget exhaustion. It can auto-generate skills to handle recurring failures and auto-install missing dependencies.

**Relevance to AURA:** AURA v4's learning system strengthens patterns but doesn't explicitly **diagnose failures** or **generate new skills** from failure analysis. OpenAkita's approach is more systematic: every failure is an opportunity to build a new capability.

**Actionable Insight:**
- Add a failure journal to AURA: log every failed task with context, attempted actions, and failure mode
- During "dreaming", analyze the failure journal:
  - **Pattern failures** (same task fails repeatedly) → generate a new execution pattern/skill
  - **Tool failures** (tool not available or wrong tool selected) → update tool selection heuristics
  - **Context failures** (lost track of multi-step task) → adjust context window management
  - **Loop failures** (got stuck in a loop) → add specific loop-break rules for that task type
- Implement a lightweight "skill generator": when AURA detects a missing capability, prompt the teacher model to generate a new tool/pattern definition
- This creates a **positive feedback loop**: failures → analysis → new capabilities → fewer failures

**Priority:** MEDIUM — Creates a self-improving system, moderate implementation complexity

---

### #10 — MARKDOWN-BASED MEMORY WITH RAG OVERFLOW (LOW-MEDIUM PRIORITY)
**Source:** ClawSwarm — memory architecture
**What it is:** ClawSwarm persists conversation history in a plain markdown file. When the file grows beyond context limits (configurable, default 100K chars), it switches to RAG: embeds the full memory and retrieves only relevant chunks for the current query. Simple but effective for cross-channel, cross-restart persistence.

**Relevance to AURA:** AURA v4's HNSW-based memory is more sophisticated, but the **adaptive overflow** concept is valuable. When AURA's context window fills up, instead of just truncating, it could embed and retrieve only what's relevant.

**Actionable Insight:**
- Implement adaptive context management in AURA:
  - When assembling context for a task, start with immediate memory (current conversation)
  - If context budget remains, add relevant retrievals from HNSW
  - If context is full, compress: summarize older context into a "session summary" and embed it
  - For long-running tasks that span multiple sessions, maintain a markdown "task journal" that can be RAG-searched
- The markdown format is also useful for **debuggability** — human-readable memory is easier to audit than pure vector stores
- Consider a dual-format approach: HNSW for fast retrieval, markdown journal for human inspection and session continuity

**Priority:** LOW-MEDIUM — Refinement to existing memory system, useful for long-running tasks

---

## ADDITIONAL FINDINGS (NOTABLE BUT LOWER PRIORITY)

### A. React Native / Flutter LLM Bindings
**Source:** llama.rn (mybigday/llama.rn), Fllama, llama_cpp_dart
**What:** Native mobile bindings for llama.cpp in React Native, Flutter, and Dart.
**AURA Relevance:** If AURA v4's Android app is built with Flutter/RN, these bindings provide direct integration without JNI bridge overhead.
**Priority:** LOW — Depends on AURA's UI framework choice

### B. PocketPal AI
**Source:** github.com/a-ghorbani/pocketpal-ai (MIT)
**What:** Open-source mobile LLM UI built on llama.cpp. Handles model management, download, and inference on mobile.
**AURA Relevance:** Reference implementation for mobile model management patterns.
**Priority:** LOW — Reference only

### C. GitAgent / Git-Native Agent Standard
**Source:** github.com/open-gitagent/gitagent (131 stars)
**What:** Framework-agnostic standard for defining AI agents where identity, rules, memory, tools, and skills are all version-controlled files in a git repo.
**AURA Relevance:** Interesting for AURA's skill/identity versioning — could version AURA's SOUL document and skills in git.
**Priority:** LOW — Nice to have for power users

### D. Skill Marketplace Pattern
**Source:** OpenAkita — skill marketplace
**What:** Searchable marketplace for agent skills with one-click install, GitHub direct install, and AI-generated skills on the fly.
**AURA Relevance:** Long-term extensibility for AURA — users could share and install new capabilities.
**Priority:** LOW — Post-v4 feature

### E. Sticker/Emoji Personality Expression
**Source:** OpenAkita — 5700+ sticker support, mood-aware, persona-matched
**What:** Personality expression through visual media — not just text responses.
**AURA Relevance:** Could add personality to AURA's responses in messaging contexts. But not core functionality.
**Priority:** LOW

---

## ARCHITECTURE IMPLICATIONS FOR AURA V4

Based on these findings, here's how AURA v4's architecture should evolve:

```
CURRENT AURA v4 Architecture:
┌────────────────────────┐
│    Teacher Stack        │ (Cloud LLM)
│    Student Model        │ (Local llama.cpp)
├────────────────────────┤
│    ReAct Loop           │ (Think → Act → Observe)
│    ETG Engine           │ (Task Graph execution)
├────────────────────────┤
│    4-Tier Memory        │ (HNSW vectors)
│    Hebbian Learning     │ (Pattern strengthening)
│    Dreaming             │ (Offline consolidation)
├────────────────────────┤
│    A11y Service         │ (Screen interaction)
│    Selector Engine      │ (Element targeting)
└────────────────────────┘

PROPOSED AURA v4+ Architecture (incorporating research):
┌────────────────────────────────────────────┐
│  SOUL Document (CORE + MUTABLE beliefs)    │  ← Finding #8
├────────────────────────────────────────────┤
│  Director (Cloud/Teacher) → Plan            │  ← Finding #3
│  Workers (Local/Student) → Execute          │
│  RuntimeSupervisor → Monitor + Budget       │  ← Finding #5
├────────────────────────────────────────────┤
│  Blackboard (Shared execution state)        │  ← Finding #4
│  + Shortcut matching for repeated tasks     │
├────────────────────────────────────────────┤
│  4-Tier Memory + Type Tags                  │  ← Finding #1
│  Significance Scorer (routine/notable/pivot) │  ← Finding #2
│  Multi-path Retrieval (semantic+time+type)  │
│  Adaptive Context (RAG overflow)            │  ← Finding #10
├────────────────────────────────────────────┤
│  Dreaming Engine (enhanced):                │
│  - Significance-filtered consolidation      │  ← Finding #2
│  - Failure journal analysis                 │  ← Finding #9
│  - Skill generation from failure patterns   │
│  - Soul evolution proposals                 │  ← Finding #8
│  - Provenance chains                        │
├────────────────────────────────────────────┤
│  Hebbian Learning (unchanged, now filtered) │
├────────────────────────────────────────────┤
│  Tool System:                               │
│  - Native: A11y, Screen, File, Comm        │
│  - MCP: Extensible integrations             │  ← Finding #6
├────────────────────────────────────────────┤
│  Inference (llama.cpp):                     │
│  - Hexagon NPU (when available)             │  ← Finding #7
│  - Adreno GPU for embeddings                │
│  - Speculative decoding for local inference │
│  - 1.5-bit micro-model for fast checks     │
└────────────────────────────────────────────┘
```

---

## IMPLEMENTATION PRIORITY MATRIX

| # | Finding | Impact | Effort | Priority | AURA Subsystem |
|---|---------|--------|--------|----------|---------------|
| 1 | Typed Memory + AI Extraction | HIGH | MEDIUM | **P0** | Memory |
| 2 | Significance Scoring + Reflection | HIGH | MEDIUM | **P0** | Dreaming |
| 3 | Director/Worker Pattern | HIGH | HIGH | **P0** | Execution |
| 5 | Runtime Supervision | HIGH | LOW | **P1** | ReAct Engine |
| 4 | Blackboard + Shortcuts | MEDIUM | MEDIUM | **P1** | Execution |
| 9 | Failure Analysis + Skill Gen | MEDIUM | MEDIUM | **P1** | Learning |
| 8 | Soul Document + Governance | MEDIUM | LOW | **P2** | Identity |
| 6 | MCP Integration | MEDIUM | MEDIUM | **P2** | Tools |
| 7 | Hexagon/Adreno Backends | MEDIUM | LOW* | **P2** | Inference |
| 10 | Adaptive Context / RAG Overflow | LOW-MED | LOW | **P3** | Memory |

*Effort is LOW because it's upstream work — we just need to integrate when ready.

---

## KEY REPOS TO WATCH

| Repository | Stars | Why Watch |
|-----------|-------|-----------|
| [openakita/openakita](https://github.com/openakita/openakita) | 969 | Most complete open-source multi-agent assistant — many patterns directly applicable |
| [mcp-use/mcp-use](https://github.com/mcp-use/mcp-use) | 9,371 | MCP is becoming the tool integration standard |
| [slhleosun/EvoClaw](https://github.com/slhleosun/EvoClaw) | 158 | Soul evolution framework — pioneering identity management |
| [ggml-org/llama.cpp](https://github.com/ggml-org/llama.cpp) | 80K+ | Track Hexagon/OpenCL backends for mobile performance |
| [Richchen-maker/openclaw-multi-agent-team](https://github.com/Richchen-maker/openclaw-multi-agent-team) | 31 | Blackboard coordination + event bus patterns |
| [The-Swarm-Corporation/ClawSwarm](https://github.com/The-Swarm-Corporation/ClawSwarm) | 39 | Hierarchical multi-agent + memory patterns |

---

## CONCLUSION

The AI agent ecosystem has matured significantly. The patterns emerging from OpenAkita, EvoClaw, and the ClawSwarm ecosystem directly address AURA v4's core challenges:

1. **Memory quality over quantity** — Type-tag memories and filter by significance before consolidation
2. **Planning ≠ execution** — Separate the director (cloud) from workers (local) for better resource use
3. **Safety via supervision** — Add loop detection, budgets, and policy enforcement before deploying on-device
4. **Learn from failure** — Every failed task should produce a lesson that prevents future failures
5. **Identity with guardrails** — Let AURA evolve, but with governance controls the user sets

The most impactful change for AURA v4 is combining findings #1, #2, and #3: **typed memory with significance scoring, fed through a director/worker execution pattern**. This addresses both the memory quality problem and the limited-compute-budget problem simultaneously.

---
*Research conducted 2026-03-05 by AURA Research Intelligence Team*
