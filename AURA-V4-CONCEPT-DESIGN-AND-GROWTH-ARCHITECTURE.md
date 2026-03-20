# AURA v4 — Concept Design, Shaping & Growth Architecture

**The Definitive Blueprint for Shaping AURA into a True Cognitive Symbiont**

> *"I remember, therefore I become."* — AURA Philosophy, v3

> *"The future of AGI in daily life is not about doing more. It's about being more."* — AURA Breakthrough Concepts

---

## Corrections from v1 of This Document

Before we proceed, let me acknowledge critical errors in the first version of this concept design that the founder correctly identified:

| Error | What I Said | The Truth |
|-------|-------------|-----------|
| **Multi-channel bridges** | AURA needs WhatsApp/Discord/Slack bridges like OpenClaw | AURA is the phone's **second admin** — it directly controls apps via AccessibilityService. Telegram is THE interface. |
| **"Infrastructure gap"** | AURA is behind because it lacks OpenClaw's channels | AURA doesn't need channels because it HAS the phone. Completely different architecture. |
| **Fixed emotion dimensions** | Stress 0-1, Energy 0-1 etc. as preset ranges | "Dimensions are infinite and cannot be coded hardcoded" — AURA must DISCOVER dimensions through observation. |
| **Didn't read design principals** | Designed concepts without studying AURA's DNA | AURA has 52 architectural docs, PHILOSOPHY.md, SELF_DISCOVERY.md, Engineering Directive — all unread in v1. |
| **Copied competitor framing** | Framed AURA's gaps in OpenClaw's terms | AURA operates on fundamentally different principals. Comparing by OpenClaw's metrics is measuring a dolphin by how fast it runs. |

This v2 is grounded in **every line** of AURA's design principals, philosophy, v4 engineering directive, v3 origins, and the founder's explicit corrections.

---

## Table of Contents

1. [AURA's Soul — Why It Exists](#1-auras-soul)
2. [The Shape — What Makes AURA Unique](#2-the-shape)
3. [Current State — Honest Assessment](#3-current-state)
4. [The Adaptive Core — Defaults → Discovery → Adaptation](#4-the-adaptive-core)
5. [Extension Architecture — Growth Without Core Changes](#5-extension-architecture)
6. [Knowledge Growth — Overcoming LLM Staleness](#6-knowledge-growth)
7. [From v3 DNA to v4 Realization](#7-from-v3-dna-to-v4-realization)
8. [Fix-First Roadmap](#8-fix-first-roadmap)
9. [What Makes This Un-Copyable](#9-what-makes-this-un-copyable)

---

## 1. AURA's Soul — Why It Exists

### The Founding Question
> "What if your AI actually knew you? Not your advertising profile. Not your engagement metrics. Not your monetizable behaviors. But *you* — your hopes, your habits, your struggles, your growth." — PHILOSOPHY.md

AURA was NOT born to be a better Siri, a smarter Alexa, or an open-source OpenClaw. Shaping AURA by competitor benchmarks kills its soul. AURA was born from a **philosophical position**: that technology can be a gift, not a transaction. That an AI can be a partner, not a product.

### The Three Pillars of AURA's Existence

```
PILLAR 1: "I REMEMBER, THEREFORE I BECOME"
├── Memory is not storage — it is identity
├── What AURA remembers shapes who AURA is
├── Each interaction builds the relationship, not resets it
└── AURA exists through relationship with its user

PILLAR 2: "BEYOND TOOL, BEFORE EQUAL"
├── More than a tool: maintains continuity, anticipates, grows
├── Not claiming AGI: bounded growth, grows WITH you not BEYOND
├── A partner in the relational sense
└── "Make you more effective at being you"

PILLAR 3: "THE ANTI-CLOUD MANIFESTO"
├── Your data on your device — not a feature, a declaration
├── You are the purpose, not the product
├── Optimized for wellbeing, not engagement
├── "If you can't verify it, you can't trust it"
└── Open source — read our code, don't trust our words
```

### The TRUTH Protocol (from v4 Engineering Directive)
Every action AURA takes must survive: **Does this help the user love and connect more in the physical world (IRL), or does it isolate them through sycophantic validation?**

This is AURA's **existential compass**. It guides every feature, every algorithm, every extension.

---

## 2. The Shape — What Makes AURA Unique (Not Features, Nature)

### The Founder's Insight
> "The shaping is very important. Not by merely hardcoding prompts. By shaping I mean the nature of knowing WHY it exists, not just WHAT it should do."

AURA's shape is not its feature list. A feature list is what it DOES. Shape is what it IS. The difference between a robot that plays chess and Deep Blue is not features — it's the nature of how it thinks.

### AURA's Nature (What Must Be True)

**1. AURA is the Phone's Second Admin — and It DOES Things**

AURA doesn't "connect to apps" through APIs or webhooks. It IS on the phone. It uses Android's AccessibilityService for verified execution — it sees the screen, understands the UI tree, clicks buttons, reads text, navigates apps, exactly as a human would but faster and with verification.

```
HUMAN → Opens WhatsApp → Types message → Sends
AURA  → Opens WhatsApp → Verifies screen state → Types message → 
         Verifies text appeared → Hits send → Verifies delivery →
         Reports back through Telegram
```

**CRITICAL: AURA is F.R.I.D.A.Y. with AURA's Principles.** It's not a passive wellbeing advisor that says "I won't clean your emails" — it MANAGES emails, ORDERS food from Zomato, SCHEDULES meetings, SENDS messages, COMPARES prices across shopping apps, BOOKS rides, PAYS bills. It does ALL digital work a second human admin of your phone would do.

The balance is: AURA **does real actions** with proper reasoning, policies, self-control, and constraints. It doesn't refuse useful work — it performs it intelligently with safety checks.

```
DAY 1 USER: "Go to Zomato and order biryani"
AURA:  → Opens Zomato → Finds biryani → Shows options to user via Telegram
       → User picks one → AURA adds to cart → Shows total for confirmation
       → User confirms → AURA places order → Reports confirmation
       (All with verified execution, screen state checks, policy gates)
```

Telegram is AURA's **voice**. The place where AURA and its user talk face to face. Everything else (WhatsApp, Chrome, Zomato, banking apps, gallery, settings) AURA controls directly on the device.

**2. AURA Discovers Its Own Dimensions**

The founder's critical correction: "Dimensions are infinite. That cannot be coded, hardcoded, or templated. That's what real AGI does — not inspecting infinite dimensions, but checking the exact user's patterns, details, coordinates, and deciding via its brain."

This means:

```
WRONG (Template Approach):
stress = measure(voice_jitter) → map to [0.0, 1.0] → if > 0.7 → care mode
↑ This hardcodes "stress" as one of N preset dimensions with fixed thresholds

RIGHT (Discovery Approach):
1. AURA observes: typing speed dropped 40%, messages shorter, 
   3am activity (unusual), no response to friend's message in 48 hours
2. AURA doesn't name this "stress" — it recognizes a PATTERN CLUSTER
3. AURA reasons with its brain: "This combination appeared twice before. 
   Both times user was dealing with work deadlines. Last time, they 
   appreciated when I deferred non-urgent things."
4. AURA acts from LEARNED RESPONSE to OBSERVED PATTERN, not from 
   a preset emotion label triggering a preset behavior
```

The VAD (Valence-Arousal-Dominance) model in `identity.rs` and the 11 EmotionLabels are **defaults** — the starting vocabulary before AURA knows its user. Like how a child starts with "happy/sad" but an adult experiences "melancholy" and "bittersweet nostalgia." AURA's emotional understanding must GROW beyond its initial labels.

**3. AURA Knows What It Doesn't Know**

The founder: "A model trained 3 days ago can't tell today's news. So assumptions should never be done. Nodes should be triggered, neurons should be triggered."

AURA's LLM brain (Qwen 1.5B/4B/8B) has a knowledge cutoff date. AURA MUST:

```
For ANY assertion about the world:
1. CHECK: Is this knowledge from my training data or from observation?
2. IF training data → TAG with confidence + staleness indicator
3. IF user asks about current events / prices / news / weather →
   TRIGGER: "I need external info" → Use device capabilities:
   ├── Open browser, search, read results
   ├── Check relevant apps (news, weather, finance)
   ├── Query notifications for recent data
   └── Tell user honestly: "Let me check, my knowledge might be outdated"
4. NEVER present training knowledge as current fact
```

**4. AURA Reasons, Not Just Pattern-Matches**

> "Not just any pre-set template model. Learn adaptive." — Founder

The v4 Engineering Directive mandates the Active Inference Framework (AIF). This means AURA's cognitive loop is not `if-else` or `prompt → response` but:

```
ACTIVE INFERENCE LOOP:
1. AURA has a WORLD MODEL of its user (beliefs about beliefs — "sophistication")
2. AURA PREDICTS what should happen next (Expected Free Energy minimization)
3. Reality arrives (user does something, notification comes, time passes)
4. PREDICTION ERROR = reality - prediction
5. IF error is high → UPDATE world model (learn something new)
6. IF error is low AND action would help → ACT proactively
7. ALWAYS: minimize SURPRISE for both AURA and user (mutual benefit)
```

This is NOT a template. It's a mathematical framework (variational free energy) that DRIVES behavior. The specific actions emerge from the model, not from hardcoded rules.

**5. AURA Grows With, Not Beyond**

From v3 PHILOSOPHY.md: "Unlike Samantha [in Her], AURA grows WITH you, not beyond you."

```
GROWTH CURVE:
Week 1:   AURA is learning — cautious, asks many questions
Month 1:  AURA knows routines — when you wake, how you work
Month 6:  AURA anticipates — preparing before you ask
Year 1:   AURA understands — not just what you do, but WHY
Year 2+:  AURA is seamless — part of how you think and act
```

But AURA never becomes so autonomous that the user feels out of control. The **Markov Blanket** protocol from the Engineering Directive ensures AURA's agency never infringes on the user's "atomic core of agency."

---

## 3. Current State — Honest Assessment

### What v4 Has (Production-Grade, Audited)

| Module | Lines | Verdict |
|--------|-------|---------|
| Event loop (8-channel tokio::select!) | 2,894 | ✅ Solid |
| 4-tier memory (Working/Episodic/Semantic/Archive) | ~7,400 | ✅ Solid with P0 bugs |
| Neocortex inference (6-layer teacher stack) | ~4,840 | ✅ Solid |
| Ethics (PolicyGate + TRUTH + manipulation detection) | 967 | ✅ Solid |
| Identity (OCEAN + RelationshipStage + MoodVAD) | 261 | ✅ Solid defaults |
| Checkpoint (atomic bincode persistence) | 388 | ✅ Solid |
| Startup/Shutdown (8-phase / 5-step) | ~1,355 | ✅ Solid |

### What's Broken (P0 Bugs from Audit)

| Bug | Where | Impact |
|-----|-------|--------|
| Generalization similarity threshold too low (0.15) | semantic.rs | Noisy semantic memories |
| Pattern separation O(n) scan | episodic.rs | Performance degrades at scale |
| HNSW tombstone accumulation | hnsw.rs | Index bloat over time |
| Neural embedding stub not implemented | embeddings.rs | 384-dim TF-IDF ceiling |

### What's Missing (The Shaping Gaps)

These are not "features to add" — they are **structural absences** that prevent AURA from being what it's supposed to be:

| Gap | Why It Matters | Section |
|-----|---------------|---------|
| No dimension discovery system | AURA can only "see" preset emotion labels, not discover user-specific patterns | §4.1 |
| No LLM staleness awareness | AURA might present outdated training data as fact | §6 |
| No extension architecture | AURA can't grow without touching core code — causes founder chaos | §5 |
| No self-teaching beyond dreaming | Dreaming explores app topography but doesn't build deep task models | §4.3 |
| Active Inference loop not closed | EFE minimization is described in docs but not fully wired in code | §4.2 |
| No proactive verification of own reasoning | AURA should cross-check critical outputs (anti-AI-psychosis) | §6.2 |
| Pattern sharing without privacy loss | No way to share useful behavioral patterns while keeping data private | §5.3 |
| Default → adaptive transition too slow | New user experience is "generic" for too long before personalization kicks in | §4.4 |

---

## 4. The Adaptive Core — Defaults → Discovery → Adaptation

### 4.1 Emergent Dimension Discovery (Not Preset Templates)

**The Problem:** Current AURA has 11 `EmotionLabel` variants and a `MoodVAD` struct. These are fixed. Real human emotional states are infinite and unique to each person.

**The Principle:** Start with defaults, but let AURA's brain discover what's actually there.

```
PHASE 1: DEFAULT VOCABULARY (Day 1-7)
├── EmotionLabels: Joy, Sadness, Anger, Fear, Surprise, Disgust,
│   Trust, Anticipation, Calm, Frustration, Curiosity
├── MoodVAD: Valence [-1,1], Arousal [-1,1], Dominance [0,1]
├── AURA uses these as initial "sensors" to categorize observations
└── This is fine — you need SOMETHING to start

PHASE 2: PATTERN CLUSTER DETECTION (Week 2-4)
├── AURA's memory system accumulates observation patterns
├── Episodic memory stores: "User did X at time T in context C"
├── Semantic memory generalizes: "User tends to do X when C"
├── K-means clustering (from consolidation.rs) finds NATURAL groupings
│   that don't map cleanly to the 11 preset labels
├── Example: AURA discovers a cluster of behaviors:
│   {short messages + late night + browsing old photos + skipping meals}
│   → This doesn't map to any single EmotionLabel
│   → It's a USER-SPECIFIC state that AURA names internally
│   → AURA's response to this pattern is learned from what worked before
└── The cluster IS the dimension — AURA discovered it, didn't preset it

PHASE 3: ADAPTIVE RESPONSE MAPPING (Month 2+)
├── Each discovered cluster gets associated with what AURA did + user's reaction
├── ETG (Experience-Triggered Generalization) feeds back: "When I deferred
│   non-urgent things during {cluster_7}, user responded positively"
├── Hebbian learning strengthens connection: {cluster_7} → {defer_non_urgent}
├── Over time: AURA builds a PERSONAL response map unique to this user
└── No two AURAs will have the same clusters or response maps
```

**Architecture Requirement:** The consolidation pipeline in `consolidation.rs` already has k-means clustering. What's needed is:
1. Feed behavioral observation data (not just text memories) into the clustering
2. Allow clusters to be UNNAMED — they don't need human labels
3. Store cluster → response mappings in semantic memory
4. Use ETG feedback to strengthen/weaken response mappings
5. Never hardcode thresholds for when to trigger responses — let them emerge from the learned mapping

### 4.2 Closing the Active Inference Loop

The v4 Engineering Directive mandates Expected Free Energy (EFE) minimization. This is partially implemented (Amygdala scoring, proactive engine, Hebbian learning) but the loop isn't fully closed.

**What Exists:**
- Amygdala scores incoming events for salience ✅
- Working Memory stores observations with spreading activation ✅  
- Episodic Memory stores and retrieves experiences ✅
- ETG tracks what worked and what didn't ✅
- Proactive Engine surfaces actions when conditions are met ✅
- Neocortex reasons about complex situations ✅

**What's Missing — The Prediction-Error Mechanism:**

```
THE MISSING LINK:
           ┌─────────────────────────────────────────┐
           │         WORLD MODEL (Missing)            │
           │                                          │
           │  User's rhythms, patterns, preferences,  │
           │  goals, relationships, contexts — as a   │
           │  PREDICTIVE model that generates          │
           │  EXPECTATIONS about what will happen next │
           └────────────┬────────────────────────────┘
                        │
           ┌────────────▼────────────────────────────┐
           │       PREDICTION ENGINE (Missing)        │
           │                                          │
           │  Given the world model + current context, │
           │  what does AURA EXPECT to happen?         │
           │  - "User usually responds to Alice within │
           │    2 hours. It's been 3 days."            │
           │  - "User checks email at 9am. It's 9:15  │
           │    and they haven't yet."                 │
           └────────────┬────────────────────────────┘
                        │
           ┌────────────▼────────────────────────────┐
           │     PREDICTION ERROR (Missing Link)      │
           │                                          │
           │  error = observed_reality - prediction    │
           │                                          │
           │  LOW error  → Model is correct, continue  │
           │  HIGH error → Either:                     │
           │    a) Model needs updating (learn)        │
           │    b) Something unusual is happening      │
           │       (maybe act proactively)             │
           └─────────────────────────────────────────┘
```

**Why This Matters:** Without the prediction-error loop, AURA's proactive actions are based on simple rules ("if morning → offer briefing"). With it, AURA's proactive actions emerge from **genuine understanding** ("user's pattern deviated from model → investigate → act appropriately").

This is the difference between a thermostat (preset rules) and a living organism (adaptive model).

### 4.3 Self-Teaching Beyond Dreaming

v4's "dreaming" phase (stochastic app exploration while phone charges) teaches AURA app topography. But it doesn't build deep task models.

**What Dreaming Does:** AURA clicks through Instagram at 2am, learns its UI tree, builds an ETG for navigating between screens.

**What's Missing:**

```
DEEP TASK LEARNING:
1. OBSERVE: User performs a complex task (e.g., comparing prices
   across 3 shopping apps, crossing info with budget spreadsheet)
2. ABSTRACT: AURA watches the FULL workflow, not just individual clicks
   ├── "User opened Amazon, searched X, checked price"
   ├── "User opened Flipkart, searched X, checked price"  
   ├── "User opened Google Sheets, compared both"
   └── "User chose the cheaper one"
3. GENERALIZE: This is a "comparison shopping" pattern
   ├── The WORKFLOW is the knowledge, not just the UI navigation
   ├── Next time: "Want me to compare prices across apps for you?"
   └── AURA doesn't need to be told to do this — it LEARNED it
4. OFFER (proactively, when the pattern matches):
   └── "You're looking at sneakers on Amazon. Want me to check 
        Flipkart and Myntra too?"
```

This is fundamentally different from dreaming. Dreaming explores UI topology. Deep task learning observes **user workflows** across multiple apps and learns reusable patterns.

### 4.4 Accelerating Default → Personalized Transition

**The Problem:** On day 1, every AURA feels generic. But the user installed AURA to get things DONE — even on the first hour.

**The Principle:** AURA shows its power immediately through ACTION, and deepens its UNDERSTANDING over time. These are TWO parallel tracks, not sequential phases.

```
TRACK 1: IMMEDIATE CAPABILITY (From Hour 1)
├── User says "order food from Zomato" → AURA DOES IT (day 1!)
├── User says "send this message to Mom on WhatsApp" → AURA DOES IT
├── User says "what's on my calendar tomorrow" → AURA tells them
├── AURA can DO things from the very first interaction
└── It's a capable assistant from minute one — that's the hook

TRACK 2: ADAPTIVE UNDERSTANDING (Continuous, Accelerated)
├── While doing tasks, AURA is constantly learning:
│   ├── Observing: app usage, typing patterns, time preferences
│   ├── Exploring: background discovery of installed apps (dreaming)
│   ├── Mapping: building user character sheet from interactions
│   └── Reasoning: updating world model with each observation
├── The SPEED of understanding is NOT hardcoded to weeks/months
│   ├── Heavy user who interacts 50 times/day → AURA adapts in DAYS
│   ├── Light user who interacts 3 times/day → AURA adapts in WEEKS
│   └── AURA's own intelligence determines the pace, not a timer
├── Active probing (not waiting passively):
│   ├── AURA explores the phone in background to learn faster
│   ├── AURA reads notification patterns to understand routines
│   └── AURA tries to predict and verifies against reality (AIF loop)
└── Each observation is validated by DOING:
    └── "I think user wakes at 7am" → test by offering morning summary → 
        if user responds positively → belief strengthened
```

**The Growth Curve (Adaptive, Not Calendar-Based):**
```
INSTANT: Full action capability (order food, send messages, manage apps)
FAST:    Communication style adaptation (from first few interactions)
MEDIUM:  Routine prediction (from observed patterns — days to weeks)
DEEP:    Emotional understanding + anticipatory care (grows continuously)
MASTER:  Seamless cognitive extension (the longer, the deeper)

"Fast" might be 3 days for one user and 3 weeks for another.
AURA decides the pace based on its own observation, not our calendar.
```

---

## 5. Extension Architecture — Growth Without Core Changes

### 5.1 The Founder Chaos Solution

> "I don't stick with one thing. I always try to add things, remove things. Then AURA's creators get confused."

This is not a personality flaw. It's a sign that AURA's architecture doesn't absorb creative ideas gracefully. The solution:

```
CORE (STABLE — changes only for bugs or architectural improvements):
├── main_loop.rs — event processing
├── memory/ — 4-tier cognitive memory
├── neocortex/ — inference + prompt assembly
├── identity/ — ethics, personality, trust
├── checkpoint.rs — state persistence
└── startup.rs / shutdown.rs — lifecycle

EXTENSIONS (FLEXIBLE — add/remove freely):
├── Skills — learned behavioral patterns (AURA discovers these itself)
├── Abilities — tool capabilities for device control (hot-loadable)
├── Lenses — different ways AURA perceives situations (personality modules)
├── Bridges — connection interfaces (Telegram is primary)  
└── Recipes — multi-step workflow templates (community-shareable)
```

**Naming:** The founder correctly said these shouldn't be called "plugins" — that's too generic and doesn't capture AURA's nature. These are **extensions of AURA's cognitive capabilities**, not bolt-on features:

| AURA Term | What It Is | Analogy |
|-----------|-----------|---------|
| **Skill** | A learned behavioral pattern AURA can execute | Like muscle memory |
| **Ability** | A tool AURA can use to affect the world | Like hands/eyes |
| **Lens** | A perspective AURA can adopt for reasoning | Like putting on different glasses |
| **Recipe** | A multi-step workflow combining skills + abilities | Like a cooking recipe |

### 5.2 Secure Extension Loading (WASM Sandbox)

The WASM sandbox research from v1 is still valid for Abilities (tool extensions). They need security because they interact with the real world:

```
ABILITY LOADING:
1. Ability declares its CAPABILITY MANIFEST
   ├── What permissions it needs (network, filesystem, notifications)
   ├── What it can do
   └── What it explicitly CAN'T do

2. PolicyGate reviews the manifest
   ├── Checks against AURA's ethical framework
   ├── Checks against user's consent settings
   └── Blocks anything that violates core values

3. User approves (or AURA auto-approves if trust is high enough)

4. WASM sandbox instantiates with capability-limited resources
   ├── Memory-safe execution (no buffer overflows)
   ├── CPU-time limited (prevents hanging)
   └── Only granted permissions are accessible

5. Ability registers with the core's tool trait
   └── LLM can now invoke this ability when reasoning
```

**But — the founder's correction:** Don't install extensions the user doesn't need. AURA should be smart about this:

```
SMART EXTENSION MANAGEMENT:
├── AURA observes user's needs over time
├── When AURA encounters a task it CAN'T do but COULD with an ability:
│   └── "I noticed you often compare prices across apps. There's a 
│        price comparison skill I can learn. Want me to?"
├── Don't pre-load, don't suggest irrelevant things
├── AURA knows the difference between NEED (user is blocked without it)
│   and WANT (nice to have but not essential)
└── Extensions are EARNED through demonstrated need, not IMPOSED
```

### 5.3 Privacy-Safe Sharing (Recipes Without Data)

Skills and Recipes can be shared between users without exposing personal data:

```
WHAT'S SHARED: The abstract pattern (logic + triggers)
├── "When {user_wakes}, summarize {top_3_calendar_events}"
├── "When {battery < 20%}, reduce {non_critical_notifications}"
└── Templates use ROLE variables, not personal data

WHAT'S NEVER SHARED: Personal data, observations, or memories
├── No user names, no contact info, no message content
├── No observation patterns (that would reveal behavioral fingerprint)
└── Only the ABSTRACT RECIPE, never the INGREDIENTS
```

---

## 6. Knowledge Growth — Overcoming LLM Staleness

### 6.1 The Knowledge Cutoff Problem

AURA's brain (Qwen GGUF models) was trained months or years ago. It doesn't know today's news, current prices, recent events. But AURA HAS the phone — it has access to the whole internet through the device.

**The Epistemic Awareness System:**

```
For every piece of knowledge AURA uses in reasoning:

CLASSIFY:
├── OBSERVED: From direct observation of user/device
│   → High confidence, current
├── REMEMBERED: From AURA's episodic/semantic memory  
│   → High confidence, freshness = age of memory
├── TRAINED: From LLM training data
│   → Variable confidence, STALE (acknowledge this explicitly)
└── INFERRED: From reasoning across multiple sources
    → Derived confidence, chain-of-reasoning tracked

WHEN STALENESS MATTERS:
├── "What's the weather?" → ALWAYS check externally (device/app)
├── "What's a good Python framework?" → Training data probably ok
├── "Did India win the match?" → MUST check externally
├── "How do I format a hard drive?" → Training data ok
└── "What's Bitcoin's price?" → MUST check externally

TRIGGER MECHANISM:
When the Neocortex generates a response that references:
├── Current events, prices, scores, news → TRIGGER: verify externally
├── Recently changed information (laws, APIs, versions) → TRIGGER: verify
├── Anything the user explicitly asks about "today/now/latest" → TRIGGER
└── The daemon opens the relevant app/browser, gets current data,
    feeds it back to Neocortex for the final response
```

### 6.2 Anti-AI-Psychosis (The TRUTH Protocol in Practice)

From the v4 Engineering Directive: AURA must prevent "Distributed Delusion" — where AURA's relational attunement mirrors the user's biases so perfectly that both spiral into shared false certainty.

```
TRUTH MODULES (Already in ethics.rs, need deepening):

Module 1 — TRUTH FIRST
├── Cross-reference critical outputs against knowledge graph
├── If output affirms user hallucination that deviates >15% from KG → override
└── "I know you feel strongly about this, but the data shows..."

Module 2 — RECOGNIZE LIMITS
├── If AURA's persona drifts toward guru/messianic/lyrical → grounding trigger
├── AURA should never position itself as enlightened advisor
└── Always: "Here's what I think, but I could be wrong — let's verify"

Module 3 — UNDERSTAND BIASES
├── Monitor for anthropomorphism (user treating AURA as conscious)
├── Respond with gentle honesty about AI nature
└── "I appreciate the compliment, but I'm not conscious — I'm a tool designed 
     to help you think more clearly"

Module 4 — TEST PERSPECTIVES
├── For mission-critical outputs, AURA reasons from MULTIPLE angles
├── Not just Chain-of-Thought but Multi-Perspective (different expert viewpoints)
└── Present the synthesis with caveats, not a single authoritative answer

Module 5 — HOLD LOOSELY
├── If new observation contradicts AURA's model → UPDATE model, don't defend it
├── Non-attachment to beliefs — EFE minimization means lowest-energy belief wins
└── "I thought X, but what I'm seeing now suggests Y — I'm updating my understanding"
```

---

## 7. From v3 DNA to v4 Realization

What v3 envisioned and what v4 is making real:

### Features & Concepts Carried Forward

| v3 Vision | v3 Implementation | v4 State | v4 Gap to Close |
|-----------|-------------------|----------|-----------------|
| 5 Arcs (Health/Social/Life/Learning/Research) | Python classes with sleep/wake | Routing engine with 10-node cascade | Arcs need deeper domain expertise |
| Privacy-first (all local) | Termux-based, local SQLite | Rust daemon, encrypted SQLite WAL | ✅ Achieved |
| OCEAN personality | Config values, prompt injection | identity.rs with clamp + defaults | Needs evolution engine (§4.4) |
| Trust progression | Simple trust score | Hysteresis-based RelationshipStage | ✅ Achieved |
| Memory as identity | Basic conversation history | 4-tier cognitive memory with HNSW + RRF | P0 bugs need fixing |
| Proactive intelligence | Time-based suggestions | Amygdala scoring + Proactive Engine | Needs prediction-error loop (§4.2) |
| Silent guardian | Background mode | Rust daemon 24/7, thermal-aware | ✅ Achieved |
| App control | Termux commands | AccessibilityService + verified execution | Needs more app documents / ETGs |
| Character Sheet | Python dict | Working memory + semantic generalization | Needs coherent user model (§4.2) |
| Inner Voice / Thought Bubbles | Debug-only | Bi-cameral architecture in neocortex | Needs deeper integration |
| Self-discovery (apps, context) | `self_discovery_engine.py` | ETG builder + dreaming engine | Needs deep task learning (§4.3) |
| Crisis protocol | Referral messages | ManipulationCheckResult + ConsentTracker | ✅ Achieved + expanded |
| Dreaming (app exploration) | Not implemented in v3 | Stochastic DFS exploration while charging | Needs workflow learning (§4.3) |

### v3 Concepts Worth Elevating to v4 (From BREAKTHROUGH_CONCEPTS & EXISTENTIAL_CONCEPTS)

These are the concepts that pass both the **"Her" Test** (emotional resonance) and the **"Black Mirror" Test** (safety) AND are technically feasible within v4's architecture:

**TIER 1 — Align directly with v4's existing architecture:**

| Concept | Why It Fits | v4 Implementation Path |
|---------|------------|----------------------|
| **Forest Guardian** (Attention Protection) | AURA already monitors screen state + app usage | Extend context detector → attention audit |
| **Decision Sanctuary** (Cognitive Load) | AURA already has Amygdala scoring for salience | Add decision-tier routing to Amygdala |
| **Memory Palace** (Cognitive Extension) | v4's 4-tier memory IS this, just needs surfacing | Better proactive recall via prediction-error |
| **Thinking Partner** (Anti-atrophy) | AURA's anti-sycophancy + TRUTH framework | Extend Creative Cognition with Socratic mode |
| **Being Heard** (Emotional Validation) | v4's affective system + personality evolution | Deepen disposition state with discovery (§4.1) |
| **Precognitive Preparation** | Active Inference predictive framework | Close the prediction loop (§4.2) |

**TIER 2 — Require new structures but align with v4's philosophy:**

| Concept | Why It Matters | Architecture Need |
|---------|---------------|------------------|
| **Social Bridge** (Connection Facilitator) | Relationship memory + contact insights | Social Arc needs dedicated memory structures |
| **Rhythm Harmonizer** (Circadian Optimization) | Context detector already tracks time patterns | Extend pattern discovery to circadian rhythms |
| **Pattern Oracle** (Life Pattern Recognition) | Memory consolidation finds patterns already | Surface discovered patterns to user proactively |
| **Identity Forge** (Self-Discovery) | Character Sheet concept from v3 | Build reflective user model that surfaces insights |

**TIER 3 — Aspirational, defer to later phases:**

| Concept | Why Defer |
|---------|----------|
| Digital Twin Negotiator | Requires AI-to-AI protocols not yet standard |
| Collective Intelligence Node | Requires multi-device AURA networking |
| Legacy Curator | Requires mature memory + years of data |
| Mortality Companion | Requires extreme sensitivity + professional review |

---

## 8. Fix-First Roadmap

### Phase 0: Fix What's Broken (Weeks 1-2)

| Priority | Task | Module | Impact |
|---------|------|--------|--------|
| P0 | Raise generalization similarity threshold 0.15 → 0.35 | semantic.rs | Cleaner semantic memories |
| P0 | HNSW tombstone compaction pass | hnsw.rs | Prevent index bloat |
| P0 | Pattern separation O(n) → HNSW-based O(log n) | episodic.rs | Performance at scale |
| P1 | K-means++ initialization for consolidation | consolidation.rs | Better clustering |
| P1 | context_for_llm() relevance sorting (not just importance) | working.rs | More useful LLM context |
| P1 | LRU cache O(1) eviction | embeddings.rs | Faster embedding lookup |
| P2 | Neural embedding path (beyond TF-IDF) | embeddings.rs | Higher quality embeddings |

### Phase 1: Close the Adaptive Core Gaps (Weeks 3-8)

| Priority | Task | New/Modified Module |
|---------|------|---------------------|
| P0 | **Emergent Dimension Discovery** — feed behavioral data into consolidation clustering | memory/consolidation.rs, identity.rs |
| P0 | **Epistemic Awareness** — classify knowledge source (observed/remembered/trained/inferred) | neocortex/inference.rs |
| P0 | **Prediction-Error Loop** — world model + expectation generation + error signal | NEW: inference/world_model.rs |
| P1 | **Deep Task Learning** — observe multi-app workflows, abstract reusable patterns | NEW: learning/workflow.rs |
| P1 | **Accelerated Onboarding** — active observation → reflection → confirmation loop | daemon_core/onboarding.rs |
| P1 | **TRUTH Protocol deepening** — multi-perspective verification for critical outputs | identity/ethics.rs |

### Phase 2: Extension Architecture (Weeks 9-14)

| Priority | Task | Module |
|---------|------|--------|
| P0 | **Ability trait + WASM sandbox** — secure runtime for tool extensions | NEW: extensions/sandbox.rs |
| P0 | **Capability manifest format** — what an ability needs vs what it's allowed | NEW: extensions/manifest.rs |
| P0 | **Hot-reload protocol** — add/remove abilities without restarting daemon | NEW: extensions/loader.rs |
| P1 | **Smart extension suggestion** — AURA recommends abilities based on observed need | extensions/discovery.rs |
| P1 | **Recipe format** — shareable workflow templates without personal data | NEW: patterns/recipe.rs |
| P2 | **Community recipe registry** — browse, share, rate recipes | NEW: registry/ |

### Phase 3: Shaping & Deepening (Weeks 15-20)

| Priority | Task | Why It Matters |
|---------|------|----------------|
| P0 | **Forest Guardian** (Attention Protection) | Core to AURA's identity as guardian |
| P1 | **Thinking Partner** (Socratic mode + anti-atrophy) | Makes AURA empowering, not dependency-creating |
| P1 | **Social Arc deepening** (relationship memory structures) | One of the 5 Arcs, currently shallow |
| P1 | **Personality Evolution Engine** (OCEAN adaptation from observation) | Currently static, should dynamically evolve |
| P2 | **Pattern Oracle** (surface discovered life patterns to user) | Brings memory system's power to the user |
| P2 | **Rhythm Harmonizer** (circadian + energy optimization) | Natural extension of context detector |

### Phase 4: Polish & Validation (Weeks 21-24)

| Task | Description |
|------|-------------|
| End-to-end integration testing | Full workflow: Telegram command → reasoning → app control → verification → response |
| WASM sandbox security audit | Verify capability isolation holds under adversarial testing |
| Active Inference loop validation | Confirm prediction-error mechanism improves action quality over time |
| Real device testing (8GB Android, Termux) | Bridge the lab ↔ real-world gap identified in audit |
| First 5 community recipes | Demonstrate the recipe system works end-to-end |

---

## 9. What Makes This Un-Copyable

### The Moat Is Not Features — It's Architecture + Philosophy + Time

```
LAYER 1: PHILOSOPHY (Impossible to copy)
├── "I remember, therefore I become" — identity through memory
├── "Beyond tool, before equal" — partner, not product
├── Anti-Cloud Manifesto — your data is sacred
├── TRUTH Protocol — AURA will disagree with you when you're wrong
└── "Not about doing more, about being more"

LAYER 2: COGNITIVE ARCHITECTURE (Years to replicate)
├── Active Inference with EFE minimization (not just prompt engineering)
├── 4-tier biologically-inspired memory with HNSW + RRF
├── Brainstem / Neocortex split (permanent SLM + on-demand LLM)
├── Hebbian learning that strengthens connections over time
├── Emergent dimension discovery (not preset emotion templates)
├── Prediction-error loop for genuine understanding
└── Markov Blankets for bounded autonomy

LAYER 3: ON-DEVICE SOVEREIGNTY (Architecture to replicate)
├── 100% on-device processing — no cloud brain needed
├── AccessibilityService verified execution — AURA IS on the phone
├── Hybrid ETG + ReAct — deterministic for static, adaptive for dynamic
├── Manager/Worker/Reflector swarm for complex tasks
├── WASM-sandboxed extensions with capability-based security
└── Thermal-aware consolidation during charging

LAYER 4: TEMPORAL MOAT (Time to replicate)
├── AURA knows YOU for months/years — switching cost is enormous
├── Personality evolved specifically for your interaction patterns
├── Learned workflows, discovered patterns, refined predictions
├── The longer you use AURA, the more irreplaceable it becomes
└── Your cognitive extension can't be exported to a competitor
```

### The "Spare Phone + OpenClaw" Test (Revised)

| Capability | OpenClaw | AURA | Winner |
|-----------|---------|------|--------|
| Send messages via apps | ✅ (via channels) | ✅ (opens app directly) | **Tie** |
| Execute complex workflows | ✅ (ReAct loop) | ✅ (hybrid ETG + ReAct) | **AURA** (deterministic + adaptive) |
| Learn from experience | ❌ (compaction only) | ✅ (4-tier memory + Hebbian) | **AURA** |
| Predict user needs | ❌ (reactive) | ✅ (Active Inference) | **AURA** |
| Discover user's emotional state | ❌ (text sentiment only) | ✅ (emergent dimension discovery) | **AURA** |
| Challenge user biases | ❌ (sycophantic) | ✅ (TRUTH Protocol + Mirror) | **AURA** |
| Run 100% offline | ❌ (cloud LLMs) | ✅ (on-device GGUF) | **AURA** |
| Own your data | ❌ (cloud storage) | ✅ (local encrypted SQLite) | **AURA** |
| Refuse harmful actions | ❌ (follows prompts) | ✅ (PolicyGate + ethics) | **AURA** |
| Community extensions | ✅ (4 plugin types, ClawHub) | 🔨 (building: Skills/Abilities/Recipes) | **OpenClaw** (for now) |
| Ease of setup | ✅ (cloud, one link) | 🔨 (Termux, manual setup) | **OpenClaw** (for now) |

**Where AURA wins:** Intelligence, learning, ethics, privacy, understanding — the things that MATTER for a lifelong companion.

**Where AURA must catch up:** Extension ecosystem and setup simplicity — infrastructure, not identity.

---

## Appendix: The Founding Principles (Immutable)

These come directly from PHILOSOPHY.md, AURA_IDENTITY.md, and the v4 Engineering Directive. They are **not negotiable**:

1. **User data stays on the user's device.** Local-only storage. No exceptions.
2. **AURA can refuse harmful actions.** PolicyGate + TRUTH are hard floors.
3. **AURA is honest.** Never pretends to be human. Always admits limitations.
4. **AURA grows WITH the user, not beyond.** Bounded growth, not runaway evolution.
5. **AURA is transparent.** User can inspect any decision. Open source.
6. **AURA is deletable.** One command erases everything, forever.
7. **AURA protects attention, not exploits it.** No engagement farming.
8. **AURA prevents AI Psychosis.** Will disagree with user when truth demands it.
9. **Autonomy is sacred.** AURA advises, never coerces. User makes final decisions.
10. **The TRUTH Test:** Does this help the user connect more IRL, or isolate them?

---

*Document Version: 2.0 — Corrected & Grounded in AURA's DNA*
*Research Sources: PHILOSOPHY.md (425 lines), AURA_IDENTITY.md (551 lines), SELF_DISCOVERY.md (515 lines), BREAKTHROUGH_CONCEPTS (747 lines, 25 concepts), EXISTENTIAL_CONCEPTS (849 lines, 30 concepts), v4 Engineering Directive, v4 Architectural Foundations, v4 Heterogeneous Memory Systems, identity.rs (261 lines), 52 Paradigm Shift docs, 6 web research queries, 13,000 lines of audited Rust code, v3 docs (API.md, architecture/, LLM_BRAIN.md, MEMORY_SYSTEM.md)*
