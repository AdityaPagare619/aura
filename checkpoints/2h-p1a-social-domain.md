# Agent 2h-P1a: Social Intelligence Domain Audit
## Completed: 2026-03-10

---

## STATUS: COMPLETE

## FILE GRADES

| File | Lines | Grade | Real/Stub | Summary |
|------|-------|-------|-----------|---------|
| graph.rs | 578 | **A-** | 100% Real | Undirected weighted social graph, BFS clustering, pruning, diversity scoring. 10 tests. Missing: automatic time-based decay. |
| birthday.rs | 380 | **A** | 100% Real | Birthday tracking, year-wrap scan, reminder flags, yearly reset. 8 tests. Sends actual user-facing messages via main loop. |
| importance.rs | 386 | **A** | 100% Real | 4-factor weighted scoring (freq/recency/depth/explicit) with adaptive weighting, sigmoid normalization, exponential decay. 7 tests. Genuinely sophisticated. |
| contacts.rs | 391 | **A** | 100% Real | Contact profiles, 9 platforms (SMS/WhatsApp/Telegram/Signal/Email/Phone/Instagram/Twitter/Other), 9 categories, CRUD, alias lookup, EMA depth tracking. 7 tests. |
| gap.rs | 306 | **B+** | ~90% Real | Gap detection with per-contact thresholds and urgency scoring works. **STUB: `health_score()` at line 150-165 always returns 1.0** — propagates into composite social score. 8 tests. |
| health.rs | 451 | **A** | 100% Real | ECI learning via EMA (alpha=0.15), Gaussian adherence formula, sentiment modifiers, category weights, trend detection (Improving/Stable/Declining/Critical). 10 tests. Most sophisticated module. |
| mod.rs | 148 | **B+** | 95% Real | Aggregates all 6 sub-engines. Composite score: rel_health×0.35 + gap_score×0.25 + contact_coverage×0.20 + diversity×0.20. Lifecycle management. contact_coverage is simplistic (count/50). |

## OVERALL GRADE: **A-**

This is one of the strongest domains in the codebase. 2,640 total lines of real, tested, mathematically sound social intelligence code. One stub (gap.health_score) and a few integration gaps prevent a full A.

---

## KEY FINDINGS

### What's Real (Evidence)
1. **Social Graph** (graph.rs:88-395): Full undirected weighted graph with canonical edge keys, BFS clustering, mutual connection detection, diversity scoring, capacity-bounded pruning. 10 unit tests passing.
2. **Birthday Reminders** (birthday.rs:156-196 + main_loop.rs:5420-5445): Scans upcoming birthdays with year-wrap handling. Main loop SENDS ACTUAL MESSAGES to user: `send_response("Upcoming birthdays: ...")`. This is live proactive behavior.
3. **Importance Scoring** (importance.rs:95-150): Adaptive 4-factor weighted scoring with sigmoid for frequency, exponential decay for recency. Weights dynamically adjust: high-freq contacts boost frequency weight; deep-but-rare contacts boost depth weight. This is genuinely clever algorithm design.
4. **Relationship Health** (health.rs:134-265): Gaussian adherence formula `exp(-(excess^2)/(2*tolerance^2))` with ECI learning via EMA. Category-weighted composite: adherence * sentiment_modifier * category_weight. Trend detection with Critical threshold.
5. **Gap Detection** (gap.rs:106-135): Detects overdue contacts with urgency ratio (gap/threshold, capped at 3.0). Per-contact configurable thresholds. Main loop scans and logs alerts.
6. **Multi-platform Contacts** (contacts.rs:55-66): 9 platforms supported. Alias-based contact resolution. EMA for message depth. Rich metadata per contact.
7. **Cron Integration** (main_loop.rs:5381-5484): 7 cron jobs: contact_update, importance_recalc, relationship_health, social_gap_scan, birthday_check, social_score_compute, social_weekly_report. Graph pruning during deep_consolidation.

### What's Stub/Incomplete (Evidence)
1. **gap.rs:150-165 `health_score()`** — Returns hardcoded 1.0. Comment says: "A more accurate version would take now_ms". Used in composite score (mod.rs:74), making social score partially unreliable.
2. **main_loop.rs:5400-5406 `relationship_health` cron** — Only calls `average_health()`, never calls `evaluate()` on individual contacts. Health scores for individual relationships may never be updated via cron.
3. **main_loop.rs:5382-5388 `contact_update` cron** — Just counts contacts. Does nothing useful.
4. **main_loop.rs:5407-5416 `social_gap_scan`** — Detects gaps but only LOGS them (tracing::debug). Unlike birthdays, gap alerts are NOT sent to the user. This is the missing "call grandma" feature.
5. **No visible input pipeline** — How do contacts get created from AccessibilityService events? The data structures accept input but the bridge from observed conversations/calls is not in these files.

---

## TRUTH PROTOCOL ASSESSMENT

**Verdict: STRONGLY PRO-CONNECTION**

| Feature | IRL Connection? | Evidence |
|---------|----------------|----------|
| Birthday reminders | YES - actively nudges user to reach out | main_loop.rs:5437 sends user-facing message |
| Gap detection | YES - detects when you've gone silent | gap.rs:106-135 works; needs user-facing output |
| Relationship health decay | YES - models relationship entropy | health.rs Gaussian formula tracks decline |
| Importance scoring | NEUTRAL - internal ranking | importance.rs:95-150 drives tier assignment |
| Social graph clustering | PRO-CONNECTION potential | graph.rs:224-266 enables "reconnect with group" nudges |
| Weekly social report | YES - awareness of social health | main_loop.rs:5467-5484 sends user-facing report |

AURA's social features are philosophically aligned with the TRUTH Protocol. They model relationships to promote real-world connection. The birthday system is the best example: it proactively tells you who to reach out to. The gap detector has the same intent but needs the last mile (user-facing alerts).

**Risk of isolation/surveillance**: LOW. The system doesn't encourage digital engagement — it tracks the ABSENCE of contact to encourage real interaction. No message content is stored (only depth metric). This is anti-surveillance by design.

---

## PRIVACY RISKS

1. **Social metadata is surveillance-adjacent**: Who you talk to, when, how often, and on which platform reveals intimate relationship patterns. Even without message content, this is sensitive.
2. **No encryption at rest visible**: Social data (contacts, graph, health scores) persists via serde Serialize/Deserialize. No encryption layer mentioned in these files.
3. **No user consent flow**: No mechanism for users to opt into/out of social tracking per-contact.
4. **No data export/deletion**: No GDPR-style "right to be forgotten" for individual contacts.
5. **Sentiment tracking**: health.rs stores SentimentLevel per relationship — emotional metadata about relationships.
6. **Mitigating factors**: All data on-device (no cloud), bounded (500 contacts max), no message content stored, 8 alias limit per contact.

**Privacy Grade: B-** — On-device is excellent. Missing encryption, consent, and deletion mechanisms are concerning for social data.

---

## IRREPLACEABILITY POTENTIAL: **HIGH**

If fully wired:
- AURA knows all your birthdays and reminds you proactively
- AURA notices when you haven't talked to your mom in 3 weeks
- AURA learns YOUR contact patterns (not generic rules) via ECI
- AURA tracks relationship health across ALL platforms (SMS + WhatsApp + Instagram + ...)
- AURA gives you a weekly social health report
- AURA understands which relationships are important vs. fading

No other tool does this privately, locally, across all platforms. This is the "knows your LIFE" differentiator.

**What prevents irreplaceability today:**
1. Gap alerts not sent to user (only logged)
2. No input pipeline from accessibility events → social module
3. health_score() stub in gap.rs
4. Individual relationship health not evaluated on cron

---

## INTEGRATION ANALYSIS

### With ArcManager (arc/mod.rs:336)
- SocialDomain is a direct field on ArcManager, fully integrated
- Composite score stored to DomainStateStore (main_loop.rs:5453-5459)
- Lifecycle management: Dormant → Initializing (< 3 contacts) → Active

### With OutcomeBus
- **No direct integration**. OutcomeBus handles execution outcomes, not social events. Social domain is cron-driven, not event-driven. This is a design gap — social events (new message detected) should ideally flow through an event bus.

### With Memory/Identity
- No direct integration visible in social files. Social data is self-contained. Memory system doesn't reference social graph. Identity doesn't reference contacts.

### With Cron System
- 7 dedicated cron jobs drive all social features
- Graph pruning happens during deep_consolidation (shared with learning domain)

---

## RECOMMENDATIONS

### Critical (Fix Soon)
1. **Wire gap alerts to user** — Change main_loop.rs:5407-5416 to call `send_response()` like birthday_check does
2. **Fix gap.rs `health_score()`** — Accept `now_ms` parameter and compute real score based on tracked contacts' gap ratios
3. **Wire `relationship_health` cron to call `evaluate()`** on individual contacts, not just `average_health()`

### Important (Next Sprint)
4. Build input pipeline: AccessibilityService notification events → social module (record_interaction, add_edge)
5. Add at-rest encryption for social data
6. Add user consent per-contact for social tracking
7. Add automatic edge weight decay in graph.rs (time-based, not just manual prune)

### Nice-to-Have
8. Connect social graph clusters to proactive suggestions ("You haven't seen your college friends in a while")
9. Add data export/deletion mechanism
10. Improve contact_coverage formula (currently just count/50, should consider category diversity)
