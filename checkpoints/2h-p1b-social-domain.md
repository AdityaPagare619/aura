# Agent 2h-P1b: Social Intelligence Domain Audit (Rigorous Re-Audit)
## Completed: 2026-03-10 Session 2

---

## STATUS: COMPLETE

## PRIOR AUDIT NOTE
This supersedes `2h-p1a-social-domain.md` with stricter grading. Key differences:
- P1a gave A- overall; this audit gives **B** — accounting for stub code, missing temporal decay, and broken integration paths.
- P1a graded graph.rs A-; revised to **B-** due to no temporal decay making the graph fundamentally unable to model evolving relationships.
- P1a graded gap.rs B+; revised to **C+** because `health_score()` always returning 1.0 poisons 25% of the composite social score.

---

## FILE GRADES

| File | Lines | Grade | Real/Stub | Summary |
|------|-------|-------|-----------|---------|
| graph.rs | 578 | **B-** | 95% Real | Undirected weighted social graph, BFS clustering, pruning, diversity scoring. 10 tests. CRITICAL: No temporal decay — edges only strengthen, never weaken. A year-old relationship stays "strong" forever. Doc inconsistency: line 3 says "undirected" but line 59 says "directed social edge" (implementation IS undirected via canonical key at line 389-395). |
| contacts.rs | 391 | **B** | 100% Real | Rich contact profiles with 9 platforms (SMS/WhatsApp/Telegram/Signal/Email/Phone/Instagram/Twitter/Other), 9 categories, EMA depth tracking, explicit importance override, alias lookup. 8 tests. No deduplication logic. No Android ContactsProvider import. O(n) lookups. |
| gap.rs | 306 | **C+** | ~85% Real | detect_gaps() works with per-contact thresholds and urgency scoring (gap/threshold, capped 3.0). **CRITICAL STUB: `health_score()` at lines 150-165 always returns `1.0`** — comment says "this satisfies the interface contract for the initial skeleton." This poisons mod.rs composite score (25% weight). No escalation, no suppression. 8 tests. |
| importance.rs | 386 | **A-** | 100% Real | Best file in the domain. Adaptive 4-factor weighted scoring: sigmoid for frequency, exponential decay for recency. Weights shift based on contact profile — high-frequency contacts boost frequency weight, deep-but-rare contacts (mentor case) boost depth weight. Renormalized to sum=1.0. 7 tests. Minor: tier assignment almost entirely recency-based despite rich scoring. |
| birthday.rs | 380 | **B** | 100% Real | Birthday tracking with correct year-wrap (birthday.rs:256-261: Dec 28 → Jan 3 = 6 days). Optional birth year, name truncation, yearly reset flags, reminder cycle management. **BUG: Feb 31 accepted as valid** (line 107 only checks month 1-12, day 1-31). No multi-stage reminders (only one notification). 8 tests. |
| health.rs | 451 | **A-** | 100% Real | Gaussian adherence formula (lines 134-155): `exp(-(excess²)/(2*tolerance²))` — early contact = perfect 1.0, late contact degrades smoothly. ECI learning via EMA (alpha=0.15) personalizes to each relationship's natural rhythm. Category-weighted composite: adherence × sentiment_modifier × category_weight. Trend detection (Improving/Stable/Declining/Critical). Hardcoded tolerance_fraction. 10 tests. |
| mod.rs | 148 | **B-** | 95% Real | Clean 4-component composite: rel_health×0.35 + gap_score×0.25 + contact_coverage×0.20 + diversity×0.20. 25% based on gap stub (always 1.0). contact_coverage is anti-quality metric (more contacts = better, capped at count/50). Only 3 tests — minimal for an aggregator. |

## OVERALL GRADE: **B**

2,640 lines of social intelligence code. Algorithmically sophisticated in places (health.rs, importance.rs) but undermined by a critical stub (gap.health_score), missing temporal decay (graph.rs), and incomplete cron integration (4 of 7 cron jobs are stubs or partial).

---

## KEY FINDINGS

### What's Real and Good (Evidence)
1. **Gaussian Adherence** (health.rs:134-155): Textbook-correct probabilistic decay model. Early contact = 1.0, late contact degrades smoothly via Gaussian. Combined with ECI learning, this is genuinely personalized.
2. **Adaptive Importance Weighting** (importance.rs:106-149): Weights dynamically adjust based on observed contact patterns. High-frequency contacts shift weight toward frequency; deep-but-rare contacts shift toward depth. This reflects real relationship psychology.
3. **Birthday Year-Wrap** (birthday.rs:256-261): Handles Dec→Jan rollover correctly. Not trivial.
4. **Social Graph BFS Clustering** (graph.rs:224-266): Groups contacts into social clusters. Enables "reconnect with your college friends" type suggestions.
5. **Multi-Platform Model** (contacts.rs:55-66): 9 platforms with alias-based resolution. Captures real-world multi-channel communication.
6. **54 Total Tests**: 10+8+8+7+8+10+3 = 54 tests across the domain.

### Critical Issues (Evidence)
1. **STUB: gap.rs:150-165 `health_score()`** — Always returns 1.0. Comment: "this satisfies the interface contract for the initial skeleton." Consumed by mod.rs:74 at 25% composite weight. **Impact: Social score is systematically inflated.**
2. **NO TEMPORAL DECAY** (graph.rs): Edge weights only increase via `strengthen()` (line 192), never decay. The ONLY pruning is `prune_weak()` during deep_consolidation (main_loop.rs:5764-5769), which is threshold-based not time-based. **Impact: Relationships can't "fade" naturally.**
3. **STUB CRON: `contact_update`** (main_loop.rs:5382-5388): 300s interval, just logs total contacts. Does nothing useful.
4. **PARTIAL CRON: `relationship_health`** (main_loop.rs:5400-5406): Only calls `average_health()`, never evaluates individual contacts.
5. **PARTIAL CRON: `social_gap_scan`** (main_loop.rs:5407-5416): Detects gaps but only logs them (tracing::debug). Gap alerts NOT sent to user.
6. **BUG: Feb 31 accepted** (birthday.rs:107): Validates month 1-12 and day 1-31 but no per-month day validation.
7. **Doc inconsistency** (graph.rs:3 vs :59): "undirected" vs "directed social edge."

### Integration Wiring (7 Cron Jobs)
| Cron Job | Interval | Status | Evidence |
|----------|----------|--------|----------|
| contact_update | 300s | **STUB** | main_loop.rs:5382-5388, logs count only |
| importance_recalc | 3600s | **REAL** | calls score_all() on contacts |
| relationship_health | 21600s | **PARTIAL** | only reads average_health() |
| social_gap_scan | 21600s | **PARTIAL** | detects but only logs, no user notification |
| birthday_check | daily | **REAL** | scans and sends via send_response() |
| social_score_compute | hourly | **REAL** | stores in DomainStateStore |
| social_weekly_report | weekly | **REAL** | sends formatted report to user |

Proactive integration: suggestions.rs:96-97,1084-1085 has `SocialGap` trigger (urgency 0.7), but gap_scan cron doesn't feed into it.

---

## SOCIAL SCIENCE ACCURACY ASSESSMENT

### Dunbar's Number/Layers (5/15/50/150)
- contacts.rs supports up to 500 contacts — exceeds Dunbar's 150 active relationships
- importance.rs tier system (Critical/High/Medium/Low/Dormant) loosely maps to Dunbar layers but is driven by recency, not scientifically calibrated
- **Gap**: No explicit mapping of Dunbar's 5 (intimate), 15 (close), 50 (good friends), 150 (acquaintances)

### Granovetter's "Strength of Weak Ties"
- importance.rs adaptive weighting partially captures this: deep-but-rare contacts get boosted depth weight (the "mentor" case)
- graph.rs clustering + diversity scoring could surface weak-tie bridges
- **Gap**: No explicit weak-tie identification or bridge detection

### Relationship Maintenance (Canary & Stafford)
- health.rs Gaussian adherence models contact frequency maintenance
- ECI learning captures each relationship's natural maintenance rhythm
- **Gap**: No differentiation between maintenance behaviors (e.g., "liked a post" vs "had dinner together")

### UCLA Loneliness Scale / Social Network Index (Cohen)
- No implementation. Weekly report is nearest analog but doesn't use validated instruments
- **Gap**: No loneliness risk detection, no social network diversity assessment

**Social Science Grade: B-** — Solid intuitive alignment with relationship science, but no validated instruments, no explicit Dunbar mapping, no weak-tie detection.

---

## TRUTH PROTOCOL: DOES THIS HELP CONNECT IRL?

| Feature | IRL Connection? | Evidence |
|---------|----------------|----------|
| Birthday reminders | **YES** - actively nudges user to reach out | main_loop.rs:5437 sends user-facing message |
| Gap detection | **YES** (intent) — but alerts NOT delivered | gap.rs:106-135 detects; main_loop.rs only logs |
| Relationship health decay | **YES** - models relationship entropy | health.rs Gaussian formula tracks decline |
| Weekly social report | **YES** - awareness of social health | main_loop.rs:5467-5484 sends to user |
| Importance scoring | **NEUTRAL** - internal ranking engine | importance.rs:95-150 drives tier assignment |
| Social graph clustering | **PRO-CONNECTION potential** | graph.rs:224-266 enables group reconnection |
| Contact coverage metric | **ANTI-QUALITY** - rewards contact hoarding | mod.rs contact_coverage = count/50 |

**Verdict: STRONGLY PRO-CONNECTION in design intent. Partially crippled by missing last-mile delivery (gap alerts) and stub code.**

---

## PRIVACY RISKS

1. **Social metadata is surveillance-adjacent**: Who, when, how often, which platform — reveals intimate patterns even without content
2. **No encryption at rest**: Social data persists via serde Serialize/Deserialize, no encryption layer in these files
3. **No per-contact consent**: No mechanism for opt-in/out of tracking per contact
4. **No data export/deletion**: No GDPR "right to be forgotten" for individual contacts
5. **Sentiment tracking**: health.rs stores SentimentLevel per relationship
6. **Mitigating factors**: All on-device (no cloud), bounded (500 contacts), no message content stored, 8 alias limit

**Privacy Grade: B-**

---

## DAY-IN-THE-LIFE SCENARIO

**6:30 AM** — AURA cron fires `social_score_compute`. Composite = rel_health(0.72) × 0.35 + gap_score(1.0 STUB!) × 0.25 + coverage(0.30) × 0.20 + diversity(0.65) × 0.20 = **0.632**. Stored to state_store.

**7:00 AM** — Morning report includes social score of 0.63. User sees "Social Health: Moderate." Doesn't know gap_score is fake.

**9:00 AM** — `social_gap_scan` fires. Detects: Mom (14 days overdue, urgency 2.1), College Friend Jake (21 days, urgency 1.4). Logs to tracing::debug. **User sees nothing.** This is the biggest missed opportunity.

**12:00 PM** — `birthday_check` fires. "Upcoming birthdays in next 3 days: Sarah Chen." AURA sends via `send_response()`. User sees it. **This works end-to-end.**

**3:00 PM** — User texts Mom via WhatsApp. If accessibility bridge were wired, this would flow to `record_interaction()` → update graph edge weight → update contact's last_seen → reset gap timer. **Currently no bridge exists in these files.**

**6:00 PM** — `importance_recalc` fires. Recalculates all contact importance scores with updated EMA depth and adaptive weighting. Mom's score adjusts.

**Sunday 8:00 PM** — `social_weekly_report` fires. User receives: "This week: 12 conversations, 3 contacts engaged. Social health: 0.63." Useful awareness but no actionable suggestions.

---

## COMPETITOR COMPARISON

| Feature | AURA Social | Apple Contacts | Google Contacts | LinkedIn | HubSpot CRM |
|---------|------------|----------------|-----------------|----------|-------------|
| Relationship health tracking | **Yes** (Gaussian) | No | No | No | Manual |
| Adaptive importance | **Yes** (4-factor) | No | "Frequent" only | Endorsements | Manual |
| Gap detection | **Partial** (no alerts) | No | No | "Reconnect" | Activity timeline |
| Birthday reminders | **Yes** (proactive) | Calendar sync | Calendar sync | Yes | Yes |
| Cross-platform | **9 platforms** | iMessage-centric | Gmail-centric | LinkedIn only | Multi-channel |
| Privacy model | **100% on-device** | iCloud sync | Cloud | Cloud | Cloud |
| Temporal decay | **Missing** | N/A | N/A | N/A | N/A |
| Contact dedup | **Missing** | Phone/email merge | Auto-merge | N/A | Auto-merge |

**AURA's advantage**: Privacy + algorithmic sophistication + cross-platform. No competitor does adaptive importance or Gaussian adherence locally.
**AURA's disadvantage**: Missing basic features (dedup, import) that competitors solved years ago.

---

## IRREPLACEABILITY ASSESSMENT

**Current: MODERATE** — Too many stubs and missing bridges to be irreplaceable today.

**If fully wired: HIGH** — No competitor offers private, on-device, cross-platform relationship health tracking with personalized contact rhythm learning.

### What prevents irreplaceability today:
1. Gap alerts not delivered to user (the "call grandma" feature doesn't work)
2. No accessibility bridge → social module pipeline
3. health_score() stub inflates composite score
4. No temporal decay means graph never reflects reality
5. No contact dedup or import from Android

### Path to irreplaceable:
1. Fix gap_scan cron to send alerts via send_response() (~5 lines)
2. Fix gap.health_score() to compute real score (~10 lines)
3. Add exponential decay to graph edge weights (~20 lines)
4. Wire accessibility notifications → record_interaction() (~50 lines)
5. Import from Android ContactsProvider (~100 lines)

---

## CREATIVE SOLUTIONS

### 1. Temporal Decay Engine
Add to graph.rs: `decay_edges(now_ms)` with RelationType-specific half-lives:
- Family: 90-day half-life (slow decay)
- Close Friend: 45-day half-life
- Colleague: 30-day half-life
- Acquaintance: 14-day half-life
Formula: `weight *= 0.5^(elapsed_days / half_life_days)`

### 2. Contact Deduplication via Platform Graph
Cross-reference aliases across contacts using:
- Exact phone number match (normalized)
- Jaccard similarity on name tokens (> 0.6 = likely match)
- Temporal co-occurrence (same person messaging on two platforms within seconds)

### 3. Graduated Gap Escalation (4-Stage Nudge)
- Stage 1 (1.0× threshold): "You haven't talked to [name] in a while" — gentle awareness
- Stage 2 (1.5× threshold): "It's been [N] days since you talked to [name]" — specific
- Stage 3 (2.0× threshold): "Conversation starter for [name]: [topic from last interaction]" — actionable
- Stage 4 (3.0× threshold): "Consider archiving [name] or scheduling a catch-up" — decision point

### 4. Social Rhythm Visualization
Export ECI patterns as a "social heartbeat" — your personalized contact rhythm across relationships, showing expected vs actual contact frequency as a waveform.

### 5. Weak Tie Bridge Detection
Use graph.rs clustering to identify contacts who bridge two otherwise disconnected social clusters. These are Granovetter's "weak ties" — enormously valuable for information flow and opportunity. Flag them as high-value even if contact frequency is low.

---

## ARTIFACTS
- 7 source files audited (2,640 lines)
- 54 unit tests cataloged
- 7 cron jobs analyzed
- 1 critical stub identified (gap.health_score)
- 1 critical architectural gap (no temporal decay)
- 2 bugs found (Feb 31, doc inconsistency)
- 5 creative solutions proposed

## NEXT STEPS
- [ ] Audit learning/ domain (9 files)
- [ ] Audit proactive/ domain (6 files)
- [ ] Audit orchestration files (cron.rs, arc/mod.rs)
