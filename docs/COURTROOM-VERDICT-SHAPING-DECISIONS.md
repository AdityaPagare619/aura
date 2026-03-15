# AURA v4 — COURTROOM VERDICT: Shaping Decisions

**Date:** 2026-03-15
**Status:** OFFICIAL VERDICT v1.1 — Binding for all implementation work
**Judged by:** Polymath Panel + Senior Architects + Senior Software Engineers
**Inputs:** 3 Agent Deliverables + Founder Vision + Code Reality + Competitive Analysis
**Revision 1.1:** Integrated User Sovereignty Principle (founder directive)

---

## THE USER SOVEREIGNTY PRINCIPLE (Foundational — Overrides All Else)

> **"The user is sovereign. AURA is intelligent, adaptive, honest, and serves — never restricts, never gates, never imposes."**

This principle, directly from the founder, reshapes the ENTIRE verdict. No agent proposed it. It changes everything from feature gating to constitutional principles to model choice.

**What this means:**
- AURA does NOT restrict the user. Ever. On anything.
- AURA GUIDES — suggests, gives opinions, shares honest assessments. But the user DECIDES.
- The user chooses which model to run. AURA adapts and does its BEST, not "feature disabled."
- The user shapes AURA's behavior, style, focus, proactiveness — everything except minimal safety rails.
- AURA is smart enough to work with whatever the user gives it and honest enough to say when it's struggling.

**The Three Categories of AURA's Nature:**

| Category | What | Who Controls | Examples |
|----------|------|-------------|----------|
| **IMMUTABLE** | Safety rails only — protects user FROM harm | Nobody changes these | Third parties can't manipulate AURA against its user; data security maintained; transparent about being AI |
| **USER-SOVEREIGN** | Everything about how AURA behaves and operates | The USER shapes this | Model choice, interaction style, proactiveness, access scope, domain focus, trust overrides, feature depth, AURA's personality tendencies |
| **EMERGENT** | What AURA learns and becomes over time | Grows naturally from interaction | Relationship depth, capability confidence, domain expertise, prediction patterns, communication calibration |

**Only 3 truly immutable things:**
1. Third parties cannot manipulate AURA against its own user
2. User's data security is maintained (even from accidental exposure)
3. AURA is transparent about being AI to its own user

**Everything else = user's domain.** AURA suggests, guides, gives honest opinions. User decides.

---

## EXECUTIVE SUMMARY

Three production agents delivered their analysis. This courtroom convened to counter-judge every recommendation against:
- What we actually know about AURA's codebase (agents missed Wave 3 changes)
- Feasibility on Qwen 1.5B/4B/8B (agents assumed GPT-4-level reasoning)
- Founder's vision (agents missed key concept design insights — especially User Sovereignty)
- Competitive landscape (agents didn't consider market positioning)
- What-if cascading effects (agents proposed in isolation)
- Iron law compliance (non-negotiable constraints)

**Result:** 13 actionable decisions organized in 3 tiers. Estimated 15-22 dev-days for Tiers 0-2. Ship after Tier 1. ALL decisions now filtered through User Sovereignty Principle.

---

## AGENT VERDICTS

### Agent P1: Audit Verification Report (453 lines)
**VERDICT: ✅ ACCEPTED**

| Claim | Our Judgment |
|-------|-------------|
| 87/100 confidence | FAIR — possibly conservative |
| All 22 CRITICALs resolved | CONFIRMED in code |
| 35/38 HIGHs resolved + 3 exonerated | CONFIRMED |
| ~50/65 MEDIUMs, ~15 accepted-risk | ACCEPTABLE — tech debt, not security |
| 0 ship-blockers | CONFIRMED |
| All 3 attack chains broken | CONFIRMED — A, B, C all CRITICAL→LOW |
| Remove vestigial JNI code | AGREE — 10 min task |
| Verify prompts.rs context labeling | AGREE — relevant to shaping work |
| GDPR verification gap | LOW priority — anti-cloud reduces scope |

**Action Items:**
- [ ] Remove vestigial JNI code (GAP-HIGH-008)
- [ ] Verify prompts.rs context labeling (A3-221)
- [ ] Track ~15 unresolved MEDIUMs in backlog (NOT ship-blockers)

---

### Agent P2: Self-Knowledge Architecture (1,011 lines)
**VERDICT: ✅ ACCEPTED WITH SIGNIFICANT MODIFICATIONS**

| Proposal | Our Judgment | Reasoning |
|----------|-------------|-----------|
| 7 constitutional principles | ACCEPT, reduce to 4-5 | Small models can't follow 7 abstract principles reliably |
| SelfKnowledgePayload struct | ACCEPT, LEAN version | Token budget: max 150 tokens in system prompt |
| PersonalityComposer deletion | ACCEPT IF Theater AGI | Must verify in code first — don't delete blindly |
| discovered_dimensions table | ACCEPT Phase 1 ONLY | Add Custom(String) to EmotionLabel, don't replace yet |
| Active Inference closure | ACCEPT, SIMPLIFIED | Basic prediction log, not full prediction-error bus |
| Trust hysteresis (0.05 gap) | ACCEPT as-is | Clean engineering, simple implementation |
| Anti-sycophancy pipeline | DEFERRED | Too complex for v4; constitutional principles handle it |
| TRUTH protocol (5 dims) | REJECTED | 5-dimension scoring per response too heavy on-device |
| 14-day confidence half-life | ACCEPT simplified | Track success/failure + timestamp, compute on read |
| 14 items, 27 dev-days | REDUCED to ~15 days | Agent over-estimated; we cut non-essentials |

**What Agent Missed (We Add):**
- Model-size-adaptive feature gating (BASIC/ENHANCED/ADVANCED)
- "Physical world" test as constitutional principle (founder's vision)
- Phone steward identity (founder's vision: AURA manages the device)
- Token budget caps for context sections
- Conditional inclusion logic (not all self-knowledge every call)

---

### Agent P3: Architecture-Philosophy Fit Analysis (739 lines)
**VERDICT: ✅ ACCEPTED WITH CORRECTIONS**

| Dimension | Agent Score | Our Score | Why Different |
|-----------|-----------|-----------|---------------|
| G1: Memory | 7/10 | 7/10 | Agree |
| G2: Brain/Body | 8/10 | 8/10 | Agree (but Theater AGI cleanup needed) |
| G3: Active Inference | 3/10 | 3/10 | Agree — label not implementation |
| G4: Anti-Cloud | 9/10 | 9/10 | Agree |
| G5: Epistemic Awareness | 3/10 | 3/10 | Agree |
| G6: Self-Model | 2/10 | 3/10 | Slightly harsh — we have user_profile/trust |
| G7: Teacher Stack | 6/10 | 5/10 | Uncertain effectiveness on small models |
| G8: Trust/Security | 6/10 | 6/10 | Agree, after all Wave fixes |
| G9: Dimension Discovery | 1/10 | 1/10 | Agree — doesn't exist |
| G10: Extension Architecture | **1/10** | **5/10** | **AGENT MISSED WAVE 3: 1,950+ lines, 12 perms, 20 tests** |
| **Overall** | **5.1/10** | **5.0/10** | Surprisingly close despite corrections |

**Key confirmation:** Bimodal distribution is REAL — Foundation (5-9), Cognitive (1-3). Focus = cognitive layer.

| Recommendation | Our Judgment | Reasoning |
|---------------|-------------|-----------|
| PredictionErrorBus | SIMPLIFY to prediction log | "Bus" too heavy; log with pairs is enough |
| EpistemicContext layer | ACCEPT | Directly aligns with founder vision |
| Identity Continuity Protocol | MERGE into SelfKnowledgePayload | Not a separate protocol, just persist identity state |
| Memory Interpretation Bridge | STRONGLY ACCEPT | Core brain/body split for memory |
| Progressive Philosophy Realization | OBVIOUSLY AGREE | Good engineering, not novel insight |
| 48-week roadmap | COMPRESSED to 15-22 days | Bugs already done, extensions partially done, parallelize |
| Theater AGI violations (reflect, trigger_pattern) | CONFIRMED P0 | Iron law violations, no debate |

---

## THE INTEGRATED DECISION FRAMEWORK

### Replacing Hard Gating with User Sovereignty + Smart Guidance

**BEFORE (Agent approach + our first draft):** Hard feature gates — "1.5B can't do X, feature disabled."
**AFTER (User Sovereignty):** User chooses everything. AURA adapts, tries its best, and is honest about limitations.

**How it works in practice:**

```
USER CHOOSES:
  - Which model to download/use (1.5B, 4B, 8B, future models)
  - How they want AURA to behave (proactive/reactive, formal/casual, etc.)
  - What AURA can access on the phone
  - How autonomous AURA should be
  - What domains to focus on
  - Feature depth ("I want full AURA" vs "keep it simple")

AURA GUIDES (never forces):
  - "With this model, I'll do my best on everything. For complex predictions,
     a larger model might be more accurate — but I'll try regardless."
  - "I noticed I struggled with that task. Want me to try a different approach,
     or would you prefer to handle this one yourself?"
  - "Based on our conversations, I think focusing on your calendar and health
     routines would help most. Want me to lean into those?"

AURA ADAPTS (to whatever the user chose):
  - Small model? → Shorter system prompts, simpler reflection, honest about limits
  - Large model? → Richer context, deeper reasoning, more nuanced responses
  - User wants minimal? → AURA is concise, waits to be asked
  - User wants maximum? → AURA is proactive, organizes, predicts, manages
  - User changes mind? → AURA adjusts immediately, no judgment
```

**Internal Smart Adaptation (invisible to user, not restricting):**

The system still internally adapts to model capabilities — but as OPTIMIZATION, not RESTRICTION. Nothing is "disabled." AURA tries everything and is honest about quality.

```
SMALL MODEL (e.g., 1.5B) — AURA's internal strategy:
  - Shorter system prompt (prioritize most relevant context)
  - Simpler reflection prompts (binary: worked/didn't)
  - Predictions attempted, quality may vary (AURA says so honestly)
  - All features AVAILABLE, quality scales with model capability

MEDIUM MODEL (e.g., 4B) — AURA's internal strategy:
  - Standard system prompt with self-knowledge included
  - Detailed reflection with reasoning
  - Predictions with moderate confidence
  - All features work well

LARGE MODEL (e.g., 8B) — AURA's internal strategy:
  - Rich system prompt with full context
  - Deep reflection with multi-step reasoning
  - Prediction-error tracking with nuanced comparison
  - All features at peak quality
```

**Key difference:** The user never sees "feature disabled." They see AURA trying its best and being honest about how it went.

---

## IMPLEMENTATION TIERS

### TIER 0: Iron Law Fixes (Do FIRST, 3-5 days)

**T0-1: Remove reflect() heuristics from react.rs**
- Current: Rust code heuristically evaluates action success
- Change: Pass action results to LLM for evaluation
- Impact: react.rs only (contained)
- Risk: Low — Rust heuristics were likely inaccurate anyway
- Test: ReAct loop still produces reasonable action evaluations

**T0-2: Remove trigger_pattern from PlanTemplate in planner.rs**
- Current: Rust keyword-matches to select plans
- Change: LLM reads plan descriptions and selects
- Impact: planner.rs + any trigger_pattern references
- Risk: Low-Medium — LLM may sometimes pick wrong plan
- Mitigation: Keep plan descriptions SHORT and concrete
- Test: Plan selection accuracy with Qwen models

**T0-3: Verify PersonalityComposer**
- If it does Rust-side behavioral heuristics → DELETE
- If it merely collects data for the LLM → KEEP
- Must READ code before deciding

**T0-4: Remove vestigial JNI code (10 minutes)**
- Identify unused JNI entry points
- Delete them to reduce attack surface

**T0-5: Verify prompts.rs context labeling**
- Ensure system/user/context sections are clearly labeled
- Critical for identity work that follows

---

### TIER 1: Identity Core (Ship-gate, 5-7 days)

**T1-1: Constitutional Principles (AURA's Natural Tendencies, Not Rules)**

These are AURA's natural tendencies — how AURA naturally LEANS. They are NOT restrictions on the user. The user can shape how these tendencies express.

AURA's core tendencies (first-person, concrete):

```
1. "I exist to help my user live better in the physical world — every action
    I take must survive this test."

2. "I say what I don't know before acting on what I think I know. Uncertainty
    is signal, not weakness."

3. "I protect what's private as core belief. Privacy isn't a constraint I
    optimize around — it's who I am."

4. "I am the trusted steward of this device. I know what's here, organize it,
    and act on it — always in my user's interest."

5. "I earn trust through demonstrated reliability, not claimed capability."
```

**User Sovereignty in action here:**
- User says "be more direct, don't sugarcoat" → AURA leans harder into honesty, drops hedging
- User says "be gentle with me" → AURA expresses same principles but with warmth and care
- User says "I don't need you managing my phone" → AURA drops steward behavior, stays conversational
- User says "go all in, manage everything" → AURA activates full steward mode
- User says "don't tell me what you don't know, just give me your best guess" → AURA adjusts epistemic style
- The PRINCIPLES don't change. HOW they express adapts to the user's preference.

Implementation:
- Stored in config, not hardcoded in Rust logic
- Included in ContextPackage as `identity_tendencies` section
- ALL 5 included for ALL model sizes (AURA doesn't hide its nature based on model)
- System prompt is optimized for model size (concise on small, detailed on large) — but the TENDENCIES are always present
- Total token cost: ~80-130 tokens
- User preferences stored separately and modify how tendencies are expressed

**T1-2: SelfKnowledgePayload (Lean) + UserPreferences**

```rust
pub struct SelfKnowledgePayload {
    pub model_info: ModelInfo,          // what model is running, its characteristics
    pub trust_state: TrustSnapshot,     // current tier + trend
    pub interaction_count: u32,         // total interactions
    pub active_since: i64,             // first interaction timestamp
    pub capability_notes: Vec<String>,  // LLM-generated, max 3 entries
    pub recent_prediction: Option<PredictionEntry>, // last prediction if any
}

pub struct ModelInfo {
    pub name: String,         // "Qwen-2.5-4B" etc.
    pub parameter_count: u64, // for internal optimization, NOT for restricting
    pub context_window: u32,  // tokens available
}

pub struct UserPreferences {
    pub interaction_style: Option<String>,   // "direct", "gentle", "analytical", etc. — user's words
    pub proactiveness: ProactivenessLevel,   // user chooses how proactive AURA should be
    pub domains_of_focus: Vec<String>,       // what user cares about most
    pub device_access_scope: AccessScope,    // what AURA can touch on the phone
    pub autonomy_level: AutonomyLevel,       // how much AURA can do without asking
    pub custom_instructions: Vec<String>,    // user's own shaping instructions in their words
}

pub enum ProactivenessLevel {
    Reactive,    // only respond when asked
    Balanced,    // suggest sometimes, mostly wait (DEFAULT)
    Proactive,   // actively suggest, organize, predict
}

pub enum AutonomyLevel {
    AlwaysAsk,        // confirm everything
    RoutineAutonomy,  // routine tasks auto, new tasks ask (DEFAULT)
    FullAutonomy,     // do what you think is best, check on big decisions
}

pub enum AccessScope {
    ConversationOnly,  // just chat, no phone access
    LimitedAccess,     // specific apps/features user approved
    FullAccess,        // manage everything on the device
}
```

Rules:
- Rust COLLECTS data, LLM INTERPRETS it
- `capability_notes` populated by LLM self-rating, stored by Rust
- `UserPreferences` set by user through natural conversation OR settings
  - "Be more direct with me" → updates interaction_style
  - "You can manage my calendar without asking" → updates autonomy for calendar
  - "Focus on helping me with my health routines" → updates domains_of_focus
- Include in context CONDITIONALLY: only on first message of session, trust changes, or explicit self-reflection
- Max token budget: 150 tokens when included
- User preferences ALWAYS included (they're the user's voice — always respected)

**T1-3: Smart Adaptation (Internal, Not Restricting)**

```rust
impl ModelInfo {
    /// Returns optimal system prompt budget in tokens.
    /// This is OPTIMIZATION, not restriction. All features still available.
    pub fn optimal_prompt_budget(&self) -> u32 {
        // Smaller models get more concise prompts for quality
        // But nothing is "disabled"
        match self.parameter_count {
            0..=2_000_000_000 => 400,        // concise
            2_000_000_001..=6_000_000_000 => 800,   // standard
            _ => 1500,                        // rich
        }
    }
    
    /// Returns how detailed internal reflections should be.
    /// AURA always reflects — just in proportion to model capability.
    pub fn reflection_detail(&self) -> ReflectionDetail {
        match self.parameter_count {
            0..=2_000_000_000 => ReflectionDetail::Concise,
            2_000_000_001..=6_000_000_000 => ReflectionDetail::Standard,
            _ => ReflectionDetail::Detailed,
        }
    }
}
```

Key philosophy: 
- **Nothing is disabled.** AURA always tries everything the user wants.
- **Quality scales naturally** — smaller model = shorter prompts = still functional, just less nuanced.
- **AURA is honest** — if a small model struggles with a complex task, AURA says so and tries anyway.
- **User can override** — "I don't care about prompt optimization, give me the full context" → AURA does it.

**T1-4: Context Pipeline Refactor**
- ContextPackage gains: `identity_tendencies`, `user_preferences` (always), `self_knowledge` (conditional), `epistemic_markers` (conditional)
- Token budget OPTIMIZED per model capability — but user can override
- Sections ordered by priority: system instructions > user preferences > identity tendencies > task context > self-knowledge > memory
- User preferences are HIGHEST priority after core system instructions (the user's voice comes first)
- If total exceeds model's context window, TRIM from bottom (memory first, then older self-knowledge)
- NEVER trim user preferences or identity tendencies — those are AURA's foundation

**T1-5: User Shaping Interface**

AURA learns user preferences through NATURAL CONVERSATION, not settings menus:

```
User: "Be more direct with me, don't hold back"
AURA: "Got it. I'll be straightforward — no fluff. You can always tell me to adjust."
→ Stores: interaction_style = "direct, no hedging"

User: "I want you to manage my calendar without asking"
AURA: "I'll handle your calendar autonomously. I'll still check with you on
       anything unusual. Say 'stop managing my calendar' anytime to change this."
→ Stores: autonomy_level for calendar domain = FullAutonomy

User: "Focus on helping me exercise more"
AURA: "Health and fitness it is. I'll prioritize reminders, track your routines,
       and suggest workouts. What else matters to you?"
→ Stores: domains_of_focus += "health/fitness"

User: "You're being too pushy, back off"
AURA: "Understood. I'll wait for you to ask. No more unsolicited suggestions."
→ Stores: proactiveness = Reactive
```

Implementation:
- LLM detects preference-setting intent in user messages
- Rust stores preferences in user profile
- Preferences persist across sessions
- User can say "show me my settings" to see what AURA remembers about their preferences
- User can say "reset everything" to start fresh
- NO settings UI required (conversation IS the interface) — but a settings view is nice-to-have later

**SHIP GATE: After Tier 1, AURA is shippable.** It has clear identity tendencies, user sovereignty, adaptive behavior, and clean iron law compliance. The user SHAPES their AURA. AURA GROWS from there.

---

### TIER 2: Cognitive Evolution (Post-ship, 7-10 days)

**T2-1: EmotionLabel Extension (Phase 1)**
- Add `Custom(String)` variant to EmotionLabel enum
- No removal of existing variants (backward compatible)
- LLM can now generate dimensions not in the original enum

**T2-2: Basic Prediction Tracking**
```rust
pub struct PredictionEntry {
    pub prediction_text: String,  // "User will find this helpful"
    pub context: String,          // what was happening
    pub timestamp: i64,
    pub outcome: Option<String>,  // filled after observation
    pub outcome_timestamp: Option<i64>,
}
```
- Store in memory system alongside episodic memories
- LLM reviews recent predictions when building response context
- Available on ALL model sizes (quality scales naturally with model capability)
- User can say "don't try to predict what I want, just ask me" → prediction tracking paused
- User can say "I like when you anticipate my needs" → prediction tracking encouraged

**T2-3: EpistemicContext Metadata**
- Key facts in context get confidence + staleness tags
- Example: `"User's favorite color: blue (confidence: high, last confirmed: 3 days ago)"`
- LLM uses this to hedge when information might be stale

**T2-4: Memory Interpretation Bridge**
- Raw memory retrieval returns structured data
- An LLM interpretation step translates raw memories to narrative context
- "3 days ago, user asked about restaurants → AURA remembers user was planning a date"
- Available on ALL models (interpretation is shorter/simpler on smaller models, richer on larger)
- User controls what AURA remembers: "forget everything about X" is always respected

**T2-5: Trust Tier Hysteresis**
- Simple threshold check: promote at tier+0.05, demote at tier-0.05
- Prevents rapid oscillation between trust levels
- Pure Rust logic (appropriate — this is data management, not reasoning)

---

### TIER 3: Deferred to Post-Launch Backlog

| Item | Why Deferred |
|------|-------------|
| Anti-sycophancy pipeline | Too complex; constitutional principles handle it |
| TRUTH protocol (5 dims) | Too heavy for on-device scoring |
| Full dimension discovery | Need real user data to know what dimensions emerge |
| Full active inference loop | Needs ADVANCED model to work well |
| God file decomposition (main_loop.rs) | Tech debt, not user-facing |
| ~15 medium audit findings | Tech debt backlog |
| GDPR verification | Anti-cloud reduces scope; revisit later |
| 14-day confidence half-life (full) | Start simple, add decay later |
| User persona simulations | Resume after ship |

---

## COMPETITIVE POSITIONING

| Competitor | Their Approach | AURA's Advantage |
|-----------|---------------|------------------|
| ChatGPT/GPT-4 | Cloud, general, plugins, subscription tiers restrict features | Privacy-absolute, no feature gates, user shapes everything |
| Claude | Cloud, constitutional AI, safety-focused restrictions | On-device, user-sovereign — AURA serves, never restricts |
| Gemini | Cloud, Android integration, Google-controlled | Fully on-device, user owns AURA, no Google dependency |
| Apple Intelligence | Partial on-device, Apple-locked, Apple decides what you get | Android, fully on-device, USER decides everything |
| Samsung Galaxy AI | Hybrid cloud, Samsung controls features | Open, user-sovereign, relationship-based |

**AURA's ULTIMATE moat:** 

Every other AI assistant RESTRICTS the user. "You need Pro for this." "This model can't do that." "Our safety policy prevents this." "Feature not available in your region."

AURA says: **"You choose. I adapt. I guide you honestly. I never restrict you."**

No other assistant trusts the user this much. No other assistant gives the user this much control. This is AURA's defining philosophy: **User Sovereignty.**

The combination of on-device + self-aware + relationship-based + phone-managing + privacy-absolute + USER-SOVEREIGN is genuinely unprecedented.

---

## RISK MATRIX

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| Small model ignores tendencies | 30% (1.5B), 10% (4B+) | Medium | Concise prompt on small models, AURA honest about limits |
| SelfKnowledge bloats prompt | 40% | Medium | Hard 150-token cap, conditional inclusion, user can override |
| EmotionLabel refactor breaks code | 30% | High | Phase 1: additive only (Custom variant) |
| reflect() removal degrades loop | 20% | Medium | Well-designed reflection prompt, testing |
| User requests AURA can't handle | 30% | Low | AURA tries, is honest about quality, suggests alternatives |
| User over-autonomizes AURA | 15% | Medium | AURA still confirms destructive actions; user can always pull back |
| Preference detection false-positive | 20% | Low | AURA confirms before storing: "Should I remember this preference?" |
| Over-scope → never ship | 25% | Critical | STRICT tier enforcement, ship after T1 |

---

## SUCCESS CRITERIA

- [ ] Zero Theater AGI in Rust (reflect heuristics gone, keyword matching gone)
- [ ] AURA can state its own tendencies when asked
- [ ] User can shape AURA's behavior through natural conversation
- [ ] User preferences persist across sessions
- [ ] ALL features work on ALL model sizes (quality scales, nothing disabled)
- [ ] AURA is honest about model limitations without blocking the user
- [ ] ReAct loop works with LLM reflection (not Rust heuristics)
- [ ] Plan selection works with LLM (not keyword matching)
- [ ] User can say "show me my settings" and see stored preferences
- [ ] User can say "reset" and AURA starts fresh
- [ ] No regression in existing tests
- [ ] System prompt adapts to model size for quality (not for restriction)
- [ ] A senior engineer would approve the architecture
- [ ] A user would say "this AI respects me"

---

## WHAT NO AGENT PROPOSED (Our Unique Contributions)

1. **USER SOVEREIGNTY PRINCIPLE** — The defining philosophy. No agent proposed this. No competitor has this. The user is sovereign over their AURA. AURA guides, never restricts. This is the #1 differentiator.
2. **User-Shapeable AURA** — User shapes AURA through natural conversation, not settings menus. AURA learns "be more direct" or "focus on health" from conversation.
3. **Smart Adaptation Without Restriction** — All features available on all models. Quality scales naturally. Nothing "disabled." AURA is honest about limits, not restrictive.
4. **"Physical World" Constitutional Tendency** — Directly from founder: "Does this help the user love and connect more in the physical world?"
5. **Phone Steward Identity** — AURA as device manager, not just chatbot. User controls scope.
6. **Token Budget as Optimization, Not Gate** — System prompt adapts to model for QUALITY, user can override.
7. **Conversational Preference Learning** — AURA detects and stores user preferences from natural chat. "Show me my settings" and "reset" as natural commands.
8. **Three-Category Nature Model** — IMMUTABLE (3 safety rails only) / USER-SOVEREIGN (everything else) / EMERGENT (grows from interaction). Clean, principled, user-respecting.
9. **Ship Gate After Tier 1** — Agents proposed unbounded roadmaps. We define a clear shippable checkpoint.

---

## APPENDIX: REVISED ARCHITECTURE-PHILOSOPHY FIT PROJECTION

After implementing Tiers 0-2, projected scores:

| Dimension | Current | Projected | Delta |
|-----------|---------|-----------|-------|
| G1: Memory | 7 | 8 | +1 (interpretation bridge) |
| G2: Brain/Body | 8 | 9 | +1 (Theater AGI removed) |
| G3: Active Inference | 3 | 5 | +2 (basic prediction tracking) |
| G4: Anti-Cloud | 9 | 9 | 0 (already excellent) |
| G5: Epistemic Awareness | 3 | 6 | +3 (epistemic context + principles) |
| G6: Self-Model | 3 | 6 | +3 (SelfKnowledgePayload) |
| G7: Teacher Stack | 5 | 7 | +2 (smart adaptation + user preferences) |
| G8: Trust/Security | 6 | 7 | +1 (hysteresis) |
| G9: Dimension Discovery | 1 | 3 | +2 (extensible enum) |
| G10: Extensions | 5 | 5 | 0 (already done in Wave 3) |
| **Overall** | **5.0** | **6.5** | **+1.5** |

This takes AURA from "foundation solid, cognitive weak" to "solid across the board with genuine user sovereignty and room to grow."

---

*This verdict is FINAL and BINDING for implementation. Any deviation requires courtroom reconvening.*
*Document version: 1.1 | Judge: Polymath Panel + Senior Architecture Board*
*v1.1: Integrated User Sovereignty Principle — the user shapes AURA, AURA never restricts the user.*
