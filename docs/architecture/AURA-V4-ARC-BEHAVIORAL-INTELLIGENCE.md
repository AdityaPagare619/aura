# AURA v4 — ARC Behavioral Intelligence

> **Document Type:** Architecture Reference — Deep Dive  
> **System:** AURA v4 — On-Device Android AI Assistant  
> **Status:** Living Document  
> **Source truth:** `crates/aura-daemon/src/arc/`

---

## Table of Contents

1. [What Is ARC and Why Bio-Inspired?](#1-what-is-arc-and-why-bio-inspired)
2. [10 Life Domains and Scoring Model](#2-10-life-domains-and-scoring-model)
3. [8 Context Modes and Transitions](#3-8-context-modes-and-transitions)
4. [Initiative Budget System](#4-initiative-budget-system)
5. [ForestGuardian: Attention Protection](#5-forestguardian-attention-protection)
6. [Routine Learning: How Patterns Become Suggestions](#6-routine-learning-how-patterns-become-suggestions)
7. [Proactive Engine: Triggers and Consent Gates](#7-proactive-engine-triggers-and-consent-gates)
8. [Social Awareness Subsystem](#8-social-awareness-subsystem)
9. [ARC ↔ Goal System Integration](#9-arc--goal-system-integration)
10. [Life Quality Index Formula](#10-life-quality-index-formula)
11. [Privacy: ARC Data Classification](#11-privacy-arc-data-classification)

---

## 1. What Is ARC and Why Bio-Inspired?

ARC (Autonomous Reasoning Core) is the behavioral intelligence layer of AURA. It is the subsystem
that watches the texture of the user's daily life — not individual requests, but patterns across
hours and days — and uses those patterns to offer proactive assistance, protect attention, and track
wellbeing.

### 1.1 Design Philosophy

ARC exists because reactive AI is insufficient for a personal assistant. A purely reactive system
answers questions. ARC makes AURA proactive: it notices that the user hasn't exercised in three
days, that they've been scrolling for 40 minutes, that their work stress indicators are elevated, and
it acts — gently, with consent — to help.

**Why bio-inspired?**

The human brain does not operate purely in response to external stimuli. It runs background
monitoring processes — hunger, fatigue, social awareness, mood regulation — that operate below
conscious attention and surface as signals only when they cross a threshold. ARC mirrors this
architecture:

- **Background monitoring** — continuous domain health tracking at low compute cost
- **Threshold-gated surfacing** — proactive actions only trigger when conditions are clearly met
- **Homeostatic regulation** — the initiative budget prevents ARC from over-interrupting
- **Hebbian reinforcement** — patterns that lead to positive outcomes get stronger

### 1.2 Iron Law Compliance

ARC is subject to all Iron Laws, with one critical application:

> **ARC never decides what the user should do. ARC notices patterns and presents observations to the
> LLM. The LLM decides whether and how to surface them.**

ARC's domain scores, routine detections, and trigger conditions are **raw structured data** that flow
into the `ContextPackage` sent to the Neocortex. The LLM reasons about this data and decides whether
to proactively speak, what to say, and how to phrase it. Rust code in ARC never generates
user-facing text.

### 1.3 Module Map

| Module | File | Responsibility |
|--------|------|---------------|
| Life Arc | `arc/life_arc.rs` | 10-domain scoring, Life Quality Index |
| Health | `arc/health.rs` | Device health, battery, thermal, app activity |
| Proactive Engine | `arc/proactive.rs` | Trigger evaluation, consent gating |
| Routine Learning | `arc/routines.rs` | Pattern detection, suggestion readiness |
| Social Awareness | `arc/social.rs` | Interaction patterns, relationship health |
| ForestGuardian | `arc/forest_guardian.rs` | Attention protection, doomscroll detection |

---

## 2. 10 Life Domains and Scoring Model

ARC tracks the user's life across 10 domains. Each domain has a **health score** (0.0 – 1.0) that
is updated continuously from observable signals.

### 2.1 Domain Table

| # | Domain | What It Tracks | Key Signals |
|---|--------|---------------|-------------|
| 1 | **Health** | Physical wellbeing | Exercise app usage, sleep app data, step count signals |
| 2 | **Finance** | Financial health | Banking app frequency, budget app interactions |
| 3 | **Relationships** | Social connection | Message frequency, call patterns, contact diversity |
| 4 | **Career** | Work engagement | Work app usage time, task completion patterns |
| 5 | **Learning** | Knowledge growth | Educational app usage, reading time, note-taking |
| 6 | **Creativity** | Creative expression | Creative app usage, content creation frequency |
| 7 | **Mindfulness** | Mental peace | Meditation app usage, screen-free periods |
| 8 | **Environment** | Physical space | Smart home interactions (if accessible), habits |
| 9 | **Social** | Community engagement | Social app breadth, community interaction quality |
| 10 | **Leisure** | Rest and recreation | Entertainment balance, hobby app usage |

### 2.2 Domain Score Computation

Each domain score is a **weighted moving average** of signal inputs over a rolling time window.

```
domain_score(d, t) = Σ(signal_i × weight_i × recency_factor(t - signal_time_i)) / Σ(weight_i)

recency_factor(Δt) = exp(-λ × Δt)
  where λ = domain_decay_rate[d]   # faster decay for Health, slower for Career
```

**Score interpretation:**

| Score | Meaning | ARC Behavior |
|-------|---------|-------------|
| 0.8 – 1.0 | Thriving | No intervention |
| 0.5 – 0.8 | Healthy | Monitor only |
| 0.3 – 0.5 | Needs attention | Queue proactive trigger |
| 0.0 – 0.3 | Neglected | High-priority trigger, check consent |

### 2.3 Domain Weights in the LQI

Each domain has a weight in the Life Quality Index formula. Default weights reflect typical human
priorities but are adjustable via user profile:

```
// Default weights (sum to 1.0)
Health:        0.20
Relationships: 0.18
Career:        0.15
Finance:       0.12
Learning:      0.10
Mindfulness:   0.08
Social:        0.07
Creativity:    0.04
Environment:   0.03
Leisure:       0.03
```

---

## 3. 8 Context Modes and Transitions

ARC maintains a **context mode** that represents the user's current life context. This mode is
injected into the `ContextPackage` and shapes how the LLM interprets the user's state.

### 3.1 Context Mode Table

| Mode | Trigger Conditions | LLM Behavior Hint |
|------|-------------------|------------------|
| `WorkFocused` | Work apps active, business hours, low notification engagement | Prioritize efficiency; minimize interruption |
| `Relaxing` | Entertainment apps, evening time, low task urgency | Warmer tone; leisure suggestions acceptable |
| `SociallyActive` | High message throughput, communication apps dominant | Social context awareness; relationship sensitivity |
| `HealthFocused` | Exercise/wellness apps active, health domain score rising | Reinforce healthy behaviors |
| `Learning` | Educational apps, reading apps, note-taking active | Support focus; knowledge connections |
| `StressedOrBusy` | Rapid app switching, high notification density, stress signals | Minimal interruption; offer simplification |
| `Transitioning` | Travel apps, maps, unfamiliar location patterns | Contextual location awareness |
| `Resting` | Screen-off periods, alarm apps, no significant activity | No proactive triggers; low-power monitoring |

### 3.2 Mode Transition Logic

Transitions are computed every **60 seconds** from the current signal snapshot. A mode change
requires the new mode's conditions to be satisfied for **3 consecutive evaluation cycles**
(3 minutes) before transitioning — preventing flicker from brief anomalies.

```
current_mode × signal_snapshot → candidate_mode
if candidate_mode == current_mode:
    hold_cycles = 0
else:
    hold_cycles += 1
    if hold_cycles >= 3:
        current_mode = candidate_mode
        hold_cycles = 0
        emit ContextModeTransition event → OutcomeBus
```

---

## 4. Initiative Budget System

The initiative budget is the mechanism that prevents ARC from becoming annoying. Without it, ARC
would fire proactive suggestions constantly. The budget enforces **scarcity of interruption**.

### 4.1 Budget Parameters

| Parameter | Value | Meaning |
|-----------|-------|---------|
| `MAX_INITIATIVE` | `1.0` | Full budget |
| `REGEN_RATE` | `0.001 / second` | Regenerates 1 full unit per ~16 minutes |
| `BATTERY_PENALTY_THRESHOLD` | `20%` | Below this, regen rate halves |
| `THERMAL_PENALTY_THRESHOLD` | `45°C` | Above this, regen rate halves |
| `LOW_BATTERY_REGEN` | `0.0005 / second` | Regen rate when battery ≤ 20% |
| `THERMAL_REGEN` | `0.0005 / second` | Regen rate when temp ≥ 45°C |

### 4.2 Cost Per Action Type

| Proactive Action | Initiative Cost | Rationale |
|-----------------|----------------|-----------|
| Routine reminder | `0.10` | Low interruption |
| Domain health nudge | `0.20` | Moderate interruption |
| ForestGuardian break suggestion | `0.15` | Protective, not demanding |
| Goal progress check-in | `0.25` | Requires user attention |
| Social engagement reminder | `0.20` | Relationship maintenance |
| Emergency health alert | `0.00` | Critical — never blocked by budget |

### 4.3 Budget Flow

```
Every second:
  budget = min(MAX_INITIATIVE, budget + effective_regen_rate())

effective_regen_rate():
  base = REGEN_RATE
  if battery_level < BATTERY_PENALTY_THRESHOLD: base *= 0.5
  if device_temp > THERMAL_PENALTY_THRESHOLD: base *= 0.5
  return base

Before firing a proactive trigger:
  if budget >= trigger.cost:
    budget -= trigger.cost
    fire trigger
  else:
    queue trigger for retry when budget recovers
```

### 4.4 Why Battery and Thermal Affect Initiative

When the device is low on battery or thermally stressed, the user is in a constrained situation.
Proactive interruptions in this state are more disruptive and less welcome. The budget slowdown is a
proxy for "the user's available attention is reduced when their device is struggling."

---

## 5. ForestGuardian: Attention Protection

ForestGuardian is ARC's attention protection subsystem. Its job is to detect harmful digital
consumption patterns and gently intervene before they become entrenched.

### 5.1 What ForestGuardian Detects

| Pattern | Detection Method | Threshold |
|---------|----------------|-----------|
| **Doomscrolling** | Continuous scroll events in feed apps without navigation | 15 min continuous, or 30 min cumulative in 2 hours |
| **Notification spiral** | High-frequency notification dismissal without meaningful engagement | >30 dismissals/hour |
| **App compulsive return** | Opening same app repeatedly within short window | Same app opened >5× in 20 minutes |
| **Pre-sleep screen exposure** | Device use within 45 min of typical sleep time | User's inferred sleep time ± 30 min |
| **Context switching overload** | Rapid task-switching with no focused attention period | >8 app switches in 5 min, sustained 30 min |

### 5.2 Intervention Levels

ForestGuardian uses escalating interventions:

| Level | Trigger | Action | Initiative Cost |
|-------|---------|--------|----------------|
| **L1 — Gentle notice** | First threshold breach | LLM given context; may suggest break | `0.15` |
| **L2 — Soft boundary** | Second breach within 2 hours | LLM offers specific alternative activity | `0.20` |
| **L3 — Clear concern** | Third breach or extended session | LLM presents pattern data; asks about wellbeing | `0.30` |
| **L4 — Mindfulness prompt** | 4+ breaches or user explicitly asks | LLM guides 2-minute breathing exercise or reflection | `0.40` |

### 5.3 What ForestGuardian Does NOT Do

- It does not block apps or interfere with the UI — that would be paternalistic
- It does not report patterns to anyone — all data stays on-device (IL-5, IL-6)
- It does not trigger if the user has disabled attention protection in their profile
- It does not evaluate content — it measures time and interaction patterns only (IL-1, IL-2)

### 5.4 Doomscroll Detection Algorithm

```
// Pure time-and-interaction measurement — no content analysis
struct ScrollSession {
    app_package: String,
    start_time: Instant,
    scroll_events: u32,
    navigation_events: u32,      // page loads, taps to new content
    meaningful_engagement: u32,  // long pauses, typing, sharing
}

fn is_doomscrolling(session: &ScrollSession) -> bool {
    let duration = session.start_time.elapsed();
    let engagement_ratio = session.meaningful_engagement as f32
        / (session.scroll_events as f32 + 1.0);

    duration > Duration::from_secs(15 * 60)  // 15 min
    && engagement_ratio < 0.05               // <5% meaningful engagement
    && session.navigation_events < 3         // not browsing, scrolling in place
}
```

---

## 6. Routine Learning: How Patterns Become Suggestions

ARC's routine learning system observes the user's temporal action patterns and converts recurring
sequences into suggestions.

### 6.1 Pattern Detection

Patterns are detected using a **sliding window co-occurrence counter** implemented in
`arc/routines.rs`. Each time an action occurs, the system checks the recent action history (last 30
minutes) for preceding actions that co-occurred on previous days at similar times.

```
Routine = {
    trigger_action: ActionType,
    follow_on_action: ActionType,
    time_window: (HourOfDay, ±30min),
    day_pattern: [Mon, Tue, Wed, Thu, Fri],  // or weekends, or all
    co_occurrence_count: u32,
    confidence: f32,  // co_occurrence_count / total_trigger_occurrences
}
```

### 6.2 Promotion to Suggestion

A pattern becomes an active suggestion candidate when:
- `confidence >= 0.7` (occurred 7 out of 10 times)
- `co_occurrence_count >= 5` (at least 5 observations)
- `time_window.hour` matches current time within ±30 minutes

### 6.3 Suggestion Lifecycle

```
Pattern observed (not yet confident)
    ↓ (5+ observations, 70%+ confidence)
Suggestion candidate
    ↓ (time window matches, initiative budget available)
Proactive trigger → LLM context
    ↓ (LLM presents suggestion)
User response:
    ├── Accepts → co_occurrence_count++ → pattern strengthens (Hebbian)
    ├── Declines once → no change
    ├── Declines 3× → pattern suppressed (below suggestion threshold)
    └── "Never suggest this" → pattern archived permanently
```

### 6.4 Example Learned Routines

| Pattern | Confidence | Suggestion |
|---------|-----------|------------|
| Opens calendar every Monday 9am | 0.92 | "Your weekly planning time — opening calendar" |
| Checks fitness app after morning alarm | 0.85 | "Good morning — logging today's workout?" |
| Opens notes before video calls | 0.78 | "Looks like you have a call soon — notes ready?" |

---

## 7. Proactive Engine: Triggers and Consent Gates

### 7.1 Trigger Conditions

A proactive trigger fires when ALL of these are true:

1. **Domain condition met** — the relevant domain score crosses its threshold
2. **Context mode compatible** — the trigger type is allowed in the current context mode
3. **Initiative budget sufficient** — `budget >= trigger.cost`
4. **User not in a sensitive state** — not in a call, not at max screen brightness in dark context
5. **Consent gate passed** — user has not disabled this trigger category

### 7.2 Consent Gate Architecture

Every category of proactive behavior has a consent gate — a user-controllable preference that can
permanently or temporarily disable it.

```rust
pub struct ProactiveConsent {
    pub routine_suggestions: bool,        // default: true
    pub attention_protection: bool,       // default: true
    pub domain_health_nudges: bool,       // default: true
    pub goal_check_ins: bool,             // default: true
    pub social_reminders: bool,           // default: true
    pub quiet_hours: Option<(u8, u8)>,    // e.g., (22, 7) = 10pm to 7am
}
```

The consent gate is checked as the **first filter** before any initiative budget check. A disabled
category costs nothing — the trigger is never even evaluated.

### 7.3 Proactive Flow Diagram

```
ARC evaluation loop (60-second cycle):
    ↓
Compute domain scores
    ↓
Evaluate trigger conditions for each domain
    ↓
Filter by consent gates
    ↓
Filter by context mode compatibility
    ↓
Filter by initiative budget
    ↓
Queue passing triggers → OutcomeBus
    ↓
daemon_core sees ProactiveTriggerReady event
    ↓
Context assembled with trigger data
    ↓
Neocortex (LLM) decides: speak now / defer / skip
    ↓
If speak: response returned to user
```

### 7.4 The LLM's Role in Proactive Actions

ARC provides raw data. The LLM decides:
- Whether the trigger is actually worth surfacing given the full context
- The tone and phrasing of the suggestion
- Whether to ask permission before proceeding
- Whether to defer ("you seem busy — I'll check in later")

This is IL-1 compliance: Rust measures and signals; LLM reasons and speaks.

---

## 8. Social Awareness Subsystem

The social awareness subsystem in `arc/social.rs` tracks the health of the user's social
connections.

### 8.1 What It Tracks

| Signal | Measurement | Privacy Note |
|--------|------------|-------------|
| Message frequency per contact | Count of messaging events (not content) | No message content stored |
| Response latency patterns | Time between received and sent messages | Pattern only, not timestamps |
| Communication app diversity | Number of distinct communication apps used | App names only |
| Call duration trends | Trend of call lengths over time | Duration only, not participants |
| Contact breadth | Number of distinct contacts engaged per week | Contact IDs hashed |

> **Privacy guarantee:** The social awareness subsystem never stores message content, call
> participants by name, or any identifiable information about the user's contacts. It tracks
> **interaction patterns**, not interaction content.

### 8.2 Social Domain Scoring

The **Relationships** and **Social** domain scores are derived from social awareness signals:

```
relationships_score = weighted_average(
    contact_breadth_score × 0.30,
    response_quality_score × 0.25,     // are responses timely and engaged?
    communication_trend_score × 0.25,  // improving or declining?
    call_depth_score × 0.20            // calls > texts indicate deeper connections
)
```

### 8.3 Social Triggers

| Condition | Trigger | Example Suggestion |
|-----------|---------|-------------------|
| No meaningful contact in 5+ days | Social isolation nudge | "You haven't connected with anyone in a while — want to reach out?" |
| Sharp drop in social domain score | Relationship health check | LLM may gently ask how the user is doing |
| Pattern of avoided contacts | Unresolved tension indicator | LLM flags pattern as context (not diagnosis) |

---

## 9. ARC ↔ Goal System Integration

ARC integrates bidirectionally with the BDI goal system (`goals/registry.rs`).

### 9.1 ARC → Goals: Goal Injection

When ARC detects a domain at critical health (< 0.3), it can **inject a suggested goal** into the
goal registry. This is subject to:

1. User consent gate for goal injection (configurable)
2. Initiative budget (`cost = 0.35` for goal injection)
3. LLM confirmation — the daemon sends the potential goal to the LLM with a "should I create this
   goal?" framing before adding it to the registry

Goal injection is **never silent**. The user always sees goals that ARC created on their behalf, can
review them, and can delete them.

### 9.2 Goals → ARC: Feedback Loop

When a goal completes or fails, the `OutcomeBus` dispatches an `ActionOutcome` that ARC subscribes
to:

- **Goal completed** → relevant domain score gets a positive pulse (+0.15 × goal_importance)
- **Goal abandoned** → domain score gets a small negative pulse (-0.05)
- **Goal deadline missed** → domain score gets a moderate negative pulse (-0.10)

### 9.3 Goal-Domain Mapping

| Goal Type | Affected Domain(s) |
|-----------|-------------------|
| Exercise goal | Health |
| Financial goal | Finance |
| Learning goal | Learning, Career |
| Relationship maintenance | Relationships, Social |
| Creative project | Creativity |
| Mindfulness practice | Mindfulness |

---

## 10. Life Quality Index Formula

The Life Quality Index (LQI) is a single number (0.0 – 1.0) representing the overall health of the
user's life as ARC perceives it. It is a **weighted mean of domain scores**.

```
LQI = Σ(domain_score[d] × weight[d])  for d in {1..10}

Default weights:
  Health:        0.20
  Relationships: 0.18
  Career:        0.15
  Finance:       0.12
  Learning:      0.10
  Mindfulness:   0.08
  Social:        0.07
  Creativity:    0.04
  Environment:   0.03
  Leisure:       0.03
  ─────────────────────
  Sum:           1.00
```

### 10.1 LQI Usage

| LQI Range | Interpretation | System Response |
|-----------|---------------|----------------|
| 0.75 – 1.0 | Thriving | ARC in background monitoring mode |
| 0.55 – 0.75 | Good | Routine suggestions enabled |
| 0.40 – 0.55 | Moderate concern | All proactive triggers enabled |
| 0.25 – 0.40 | Significant concern | Higher-priority triggers, LLM given LQI context |
| 0.00 – 0.25 | Critical concern | LQI value included in all ContextPackages; LLM may check in proactively |

### 10.2 LQI Is Informational, Not Prescriptive

The LQI is passed to the LLM as **raw data** in the `ContextPackage`. The LLM decides what (if
anything) to do with it. A low LQI does not automatically trigger interventions — it shifts the
context in which the LLM reasons. The LLM might ask how the user is doing. Or it might determine the
user is fine and the LQI is temporarily low due to a busy work week.

This is IL-1 compliance: the LQI is a measurement, not a directive.

---

## 11. Privacy: ARC Data Classification

All ARC data is classified under AURA's 4-tier data classification system
(see MEMORY-AND-DATA-ARCHITECTURE §10).

### 11.1 ARC Data Classification Table

| Data Type | Classification | Storage | Encryption | GDPR Right |
|-----------|---------------|---------|-----------|------------|
| Domain scores (current) | Tier 2 — Personal | SQLite | Standard | Export + Delete |
| Domain score history | Tier 2 — Personal | SQLite (rolling 90d) | Standard | Export + Delete |
| Context mode history | Tier 1 — Operational | Memory only | None | Delete (ephemeral) |
| Routine patterns | Tier 2 — Personal | SQLite | Standard | Export + Delete |
| ForestGuardian sessions | Tier 2 — Personal | SQLite (rolling 30d) | Standard | Export + Delete |
| Social interaction patterns | Tier 3 — Sensitive | SQLite | AES-256-GCM vault | Export + Delete |
| LQI history | Tier 2 — Personal | SQLite (rolling 365d) | Standard | Export + Delete |
| Initiative budget state | Tier 1 — Operational | Memory only | None | N/A (ephemeral) |

### 11.2 What ARC Never Stores

- Message content from any communication app
- Names or identities of the user's contacts
- Conversation transcripts related to social interactions
- Any raw content from apps ARC observes (interaction patterns only)

### 11.3 User Control Over ARC Data

Users can:
- View their current domain scores and LQI via the AURA UI
- Export all ARC data as JSON via the GDPR export flow
- Delete all ARC learning data (resets domain scores, clears routine patterns)
- Disable individual ARC subsystems via `ProactiveConsent` settings
- Set quiet hours when no proactive triggers fire

---

*This document describes the design and intent of the ARC layer. For implementation status, see
[AURA-V4-PRODUCTION-STATUS.md](AURA-V4-PRODUCTION-STATUS.md). Several ARC subsystems are currently
stubs being implemented.*
