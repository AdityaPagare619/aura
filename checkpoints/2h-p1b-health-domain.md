# AURA v4 Audit: Agent 2h-P1b — Health & Wellbeing Domain

**Auditor Role**: Health informatics expert, biomedical engineer, UX researcher
**Date**: 2026-03-10
**Status**: COMPLETE
**Files Audited**: 4 files, 2,727 LOC (source) + 219 LOC (mod.rs aggregator) = 2,946 total LOC

---

## 1. FILE-BY-FILE ANALYSIS

### 1.1 sleep.rs — Sleep Tracking & PSQI-Adapted Scoring

- **Lines**: 810 (580 source + 230 test)
- **Purpose**: Models sleep records, computes adapted PSQI quality score, detects late-night device usage, generates personalized sleep recommendations.
- **Real-world problem**: Sleep deprivation is the #1 modifiable health risk factor. A system that learns YOUR sleep patterns (not population averages) is genuinely valuable.

#### What's REAL vs STUB

| Feature | Status | Evidence |
|---------|--------|----------|
| Sleep record model (onset/wake/latency/WASO/awakenings) | **REAL** | `sleep.rs:73-90` — full SleepRecord struct with all clinical fields |
| Duration/efficiency computation | **REAL** | `sleep.rs:93-127` — mathematically correct (TIB - WASO - latency) |
| PSQI-adapted composite scoring | **REAL** | `sleep.rs:262-289` — 5 weighted sub-scores, tested |
| EMA-based learning of user's ideal hours | **REAL** | `sleep.rs:202-212` — alpha=0.15 EMA updates on each record |
| Late-night device usage detection | **REAL** | `sleep.rs:235-254` — streak counting with adaptive threshold |
| Recommendation engine | **REAL** | `sleep.rs:441-533` — 6 categories, priority-sorted, capped at 5 |
| Ring buffer with 365-day capacity | **REAL** | `sleep.rs:221-228`, tested at `sleep.rs:709-717` |
| Actual sleep inference from sensors | **STUB** | Data must be externally provided — `source` field accepts "sensor"/"manual"/"inferred" but no sensor integration code exists in this file |
| Circadian rhythm analysis | **ABSENT** | No chronotype detection despite onset_time_of_day being tracked |

#### Medical Accuracy

**PSQI Adaptation** (sleep.rs:7-15): The Pittsburgh Sleep Quality Index has 7 components. AURA uses 5 adapted components with custom weights:
- Duration (0.35) — maps to PSQI Component 3
- Efficiency (0.25) — maps to PSQI Component 4 (sleep efficiency)
- Consistency (0.20) — NOT in original PSQI (but clinically validated by Buysse et al., 2006)
- Latency (0.10) — maps to PSQI Component 2
- Disturbance (0.10) — maps to PSQI Component 5

**Assessment**: The adaptation is reasonable. The original PSQI uses equal-weight 0-3 scoring across 7 components, totaling 0-21. AURA's weighted 0-1 normalization loses some granularity but gains interpretability. The addition of "consistency" is well-supported by circadian research (Wittmann et al., 2006). Missing PSQI components (subjective quality, daytime dysfunction, medication use) are partially covered: subjective quality exists as optional field (`sleep.rs:86-87`) but isn't used in scoring.

**Concern — Default ideal of 7.5h** (`sleep.rs:32`): NSF recommends 7-9h for adults 18-64, 7-8h for 65+. The 7.5h default is clinically appropriate as a midpoint. The adaptive EMA (`sleep.rs:209-212`) will personalize this, which is good medicine — the right amount of sleep varies per individual.

**Concern — Efficiency threshold** (`sleep.rs:332`): 85% efficiency as the ceiling for scoring is consistent with clinical sleep medicine (>85% = normal per AASM standards). Correct.

**Concern — Latency scoring** (`sleep.rs:377-388`): <= 10 min = perfect, >= 30 min = zero. This aligns with PSQI Component 2 thresholds. However, very short latency (<5 min) can indicate sleep deprivation — this is NOT flagged.

#### Test Coverage
15 tests covering: duration calculation, efficiency, quality scoring (good/bad/no-data), late-night detection/reset, record validation, ring buffer overflow, recommendations (insufficient data, good sleep, poor duration, late night, max cap). **Comprehensive.**

---

### 1.2 medication.rs — Medication Schedule & Adherence Tracking

- **Lines**: 756 (525 source + 231 test)
- **Purpose**: Multi-medication scheduling, escalation-based reminders, adherence scoring with importance weighting, behavioral nudges.
- **Real-world problem**: Medication non-adherence costs $290 billion/year in the US alone. A proactive, escalating reminder system is life-saving for chronic conditions.

#### What's REAL vs STUB

| Feature | Status | Evidence |
|---------|--------|----------|
| Medication schedule model (Daily/MultiDaily/EveryNHours/AsNeeded) | **REAL** | `medication.rs:53-63` — covers 95% of real prescriptions |
| 4-level escalation (Silent/Gentle/Urgent/Missed) | **REAL** | `medication.rs:39-50`, compute: `medication.rs:126-141` |
| Weighted adherence scoring | **REAL** | `medication.rs:258-284` — importance-weighted average |
| Dose history ring buffer (365 entries) | **REAL** | `medication.rs:212-219`, overflow tested |
| Nudge engine (adherence alerts, timing optimization, positive reinforcement) | **REAL** | `medication.rs:405-476` — 3 nudge types with confidence |
| Actual notification delivery | **STUB** | Escalation levels are COMPUTED but no Android notification bridge. The `check_pending_doses()` method at `medication.rs:306-322` returns DoseWindow objects but nothing sends them to the user's screen |
| Drug interaction checking | **ABSENT** | No interaction database or FDA API integration |
| Pharmacy/prescription integration | **ABSENT** | Purely manual entry |
| Timezone handling | **WEAK** | `medication.rs:331`: `day_start = (now / 86400) * 86400` is UTC midnight, not local midnight. This is a **critical bug** for users in non-UTC timezones |

#### Medical Accuracy

**Escalation levels** (`medication.rs:9-14`): The 50%/75%/100% window progression is clinically reasonable. Standard practice in medication management apps (Medisafe, MyTherapy) uses similar tiered alerting.

**Adherence scoring** (`medication.rs:258-284`): Weighted by medication importance is excellent — missing a statin is less urgent than missing immunosuppressants. The WHO defines adherence as "the extent to which a person's behavior corresponds with agreed recommendations from a health care provider." The taken/total ratio matches this definition.

**Concern — 30-minute default window** (`medication.rs:28`): Many medications have wider therapeutic windows (e.g., daily statins can be taken within a 2-hour window). Some are time-critical (insulin, antibiotics). The default should perhaps vary by medication type, or the system should prompt the user to set it.

**Concern — No PRN tracking** (`medication.rs:373`): `AsNeeded` returns `None` from `next_scheduled_time`, meaning PRN medications are completely ignored by `check_pending_doses`. For pain management patients, PRN tracking (max daily doses, minimum intervals) is critical for safety.

**Concern — Lateness tracking has a gap** (`medication.rs:209`): `record_dose()` hardcodes `lateness_secs: 0`, while `record_dose_with_lateness()` at line 226 properly takes the parameter. If the caller uses the simpler version, timing optimization nudges won't work.

#### Test Coverage
13 tests covering: escalation levels, adherence scores (none/perfect/half), capacity limits, ring buffer, remove schedule, weighted adherence, nudges (none/poor/good/late timing/max cap). **Comprehensive.**

---

### 1.3 fitness.rs — Fitness Tracking & Activity Scoring

- **Lines**: 603 (438 source + 165 test)
- **Purpose**: Step counting, workout sessions, calorie estimation via MET values, activity scoring with WHO-referenced targets.
- **Real-world problem**: Physical inactivity is the 4th leading risk factor for mortality (WHO). Tracking steps and activity minutes with actionable recommendations addresses this directly.

#### What's REAL vs STUB

| Feature | Status | Evidence |
|---------|--------|----------|
| Step recording with day rollover | **REAL** | `fitness.rs:159-173` — accumulates incremental counts, flushes on day change |
| Workout session model (7 activity types) | **REAL** | `fitness.rs:42-67` — Walking/Running/Cycling/Strength/Swimming/Yoga/Other |
| MET-based calorie estimation | **REAL** | `fitness.rs:192-195` — `MET x weight_kg x hours`, textbook formula |
| Activity scoring (4 factors, WHO-aligned) | **REAL** | `fitness.rs:205-234` — steps 40%, active mins 30%, workouts 20%, sedentary 10% |
| Sedentary detection (2h threshold) | **REAL** | `fitness.rs:319-329` — checks elapsed since last activity |
| Recommendation engine | **REAL** | `fitness.rs:341-414` — 4 categories, priority-sorted, capped at 4 |
| Actual step counting from accelerometer | **STUB** | `record_steps()` at `fitness.rs:159` receives externally-computed step counts. No accelerometer integration. |
| GPS/distance tracking | **ABSENT** | No location integration for run/walk distance |
| Heart rate integration during workouts | **PARTIAL** | `WorkoutSession.avg_heart_rate` at `fitness.rs:80` exists as `Option<f32>` but isn't used in scoring |
| Sedentary scoring | **STUB** | `fitness.rs:224-228`: hardcoded `1.0` if any activity exists — the `max_sedentary_secs` field in DailyActivity (`fitness.rs:99`) is always 0 (`fitness.rs:267`) |

#### Medical Accuracy

**MET values** (`fitness.rs:25-29`):
| Activity | AURA MET | Compendium 2011 MET | Verdict |
|----------|----------|---------------------|---------|
| Walking | 3.5 | 3.0-3.5 (3.0 mph) | Correct |
| Running | 8.0 | 8.0-9.8 (5-6 mph) | Correct (conservative) |
| Cycling | 6.0 | 6.8 (12-14 mph) | Slightly low but acceptable |
| Strength | 5.0 | 3.5-6.0 | Middle range, correct |
| Swimming | 7.0 | 5.8-10.0 | Middle range, correct |
| Yoga | 3.0 | 2.5-4.0 | Correct |

**Source**: Ainsworth et al., "2011 Compendium of Physical Activities." All values are within acceptable clinical ranges.

**Calorie per step** (`fitness.rs:32`): 0.04 cal/step for 70kg person = 400 cal/10k steps. Published estimates range 0.03-0.05 depending on pace and weight. The 0.04 default is acceptable but should scale with weight_kg (it currently doesn't — `flush_today` at `fitness.rs:264` uses the constant, not weight-adjusted).

**WHO recommendation** (`fitness.rs:217`): 150 min/week of moderate-intensity activity is directly from WHO 2020 guidelines. Correct.

**Concern — Step goal**: 10,000 steps/day (`fitness.rs:22`) is popular but not evidence-based. Meta-analyses (Paluch et al., 2022 JAMA) show mortality benefit plateaus at ~7,000-8,000 steps/day, and is age-dependent. However, 10k as a default motivational target is culturally established and harmless.

#### Test Coverage
13 tests covering: MET values, calorie estimation, step recording, day rollover, activity score default, sedentary check, workout recording, step goal customization, recommendations (no activity, good activity, sedentary, partial steps, max cap). **Comprehensive.**

---

### 1.4 vitals.rs — Vital Signs Monitoring with Anomaly Detection

- **Lines**: 558 (431 source + 127 test)
- **Purpose**: Online statistics via Welford's algorithm for multiple vital sign channels, z-score-based anomaly detection.
- **Real-world problem**: Detecting abnormal vital trends (resting HR creeping up, SpO2 dropping) can provide early warning for cardiovascular events, infections, or respiratory conditions.

#### What's REAL vs STUB

| Feature | Status | Evidence |
|---------|--------|----------|
| Welford's algorithm for online mean/variance | **REAL** | `vitals.rs:89-177` — textbook implementation, numerically stable |
| Z-score anomaly detection | **REAL** | `vitals.rs:159-176` — threshold-configurable, minimum sample guard |
| Multi-channel vital monitoring (8 types) | **REAL** | `vitals.rs:34-44` — HR, BP sys/dia, SpO2, temp, resp rate, glucose, HRV |
| Ring buffer per channel (1440 = 24h at 1/min) | **REAL** | `vitals.rs:20`, implemented at `vitals.rs:219-225` |
| Anomaly recording and alerting | **REAL** | `vitals.rs:291-318` — anomalies stored, `warn!` tracing emitted |
| Composite score from channel stability | **REAL** | `vitals.rs:356-380` — anomaly rate per channel |
| Actual sensor data ingestion (watch/wearable) | **STUB** | `VitalReading.source` at `vitals.rs:78` accepts "watch" but no BLE/wearable bridge exists |
| Physiological range validation | **ABSENT** | No hard limits (e.g., HR < 30 or > 250 should always alert regardless of z-score) |
| Emergency detection | **ABSENT** | No critical thresholds for life-threatening values (SpO2 < 90%, HR > 180 sustained) |
| Trend analysis (slope over time) | **ABSENT** | Only point anomalies detected, no gradual drift detection |

#### Medical Accuracy

**Welford's algorithm** (`vitals.rs:99-177`): Mathematically correct. The `update()` method at lines 111-117 implements the standard single-pass formulation. Population variance at line 137, sample variance at line 146 (Bessel's correction). This is a solid foundation.

**Z-score threshold of 2.5** (`vitals.rs:23`): For a normal distribution, |z| >= 2.5 corresponds to values outside the 98.76th percentile. This is more conservative than the typical 2.0 (95th percentile) used in clinical alerting, which means fewer false alarms but potentially missed early warnings.

**CRITICAL CONCERN — No absolute bounds**: Z-score-only detection is **medically dangerous** for vital signs. Example: If a user consistently runs HR of 140-160 bpm (undiagnosed tachycardia), the Welford accumulator will learn this as "normal." A reading of 72 bpm (actually healthy) would trigger an anomaly alert, while the chronically elevated baseline would never be flagged. **Vital signs require both statistical anomaly detection AND absolute physiological bounds.**

Recommended absolute bounds (from AHA/ACC/WHO):
- Heart Rate: < 40 or > 180 bpm
- SpO2: < 92%
- Systolic BP: < 80 or > 180 mmHg
- Temperature: < 35.0 or > 39.5 C
- Respiratory Rate: < 8 or > 30 breaths/min
- Blood Glucose: < 54 or > 400 mg/dL

**CRITICAL CONCERN — No minimum sample period for initial readings**: The first 10 readings (`MIN_SAMPLES_FOR_ZSCORE = 10` at `vitals.rs:27`) bypass anomaly detection entirely. If a user's first 9 readings are critically abnormal, they all pass silently.

#### Test Coverage
8 tests covering: Welford basic stats, z-score computation, insufficient samples, anomaly detection, monitor ingestion, anomaly flagging, composite score neutral, channel capacity. **Adequate but missing edge cases (zero variance, NaN, extreme values).**

---

### 1.5 mod.rs — Health Domain Aggregator

- **Lines**: 219 (157 source + 62 test)
- **Purpose**: Combines all 4 sub-domains into a weighted composite score with trend EMA and lifecycle management.

#### Composite Formula (mod.rs:8-13)
```
health_score = 0.30 * med_adherence
             + 0.25 * sleep_quality
             + 0.20 * activity_score
             + 0.15 * vitals_score
             + 0.10 * trend_bonus
```

**Assessment**: Weights sum to 1.0 (verified by test at `mod.rs:204-207`). Medication adherence being highest-weighted (0.30) is defensible — medication non-adherence is the most immediately dangerous health behavior. Sleep at 0.25 reflects its outsized impact on overall health. The trend bonus (0.10) rewards consistent improvement, which is behaviorally sound (positive reinforcement loop).

---

## 2. KEY QUESTIONS — ANSWERED

### Q1: Is sleep tracking based on screen-off inference or actual sensor data?

**Answer**: Neither, currently. Sleep data enters via `SleepTracker::record()` at `sleep.rs:194` which accepts externally-constructed `SleepRecord` objects. The `source` field (`sleep.rs:89`) is a string that can be "sensor", "manual", or "inferred" — but this is informational only; no actual sensor integration exists. The main loop at `main_loop.rs:5335-5341` calls `sleep.quality_score()` on a cron job called `sleep_infer`, but the inference logic itself would live elsewhere (not in these files).

**Accuracy potential**: Screen-off inference can reasonably estimate onset/wake times (phone goes still + screen off at 11pm, first interaction at 7am). But latency, WASO, and awakenings require either: (a) wearable sensor data, or (b) sophisticated phone-based motion inference via accelerometer. The SleepRecord model is ready for rich data; the pipeline to fill it is missing.

### Q2: Can medication reminders actually prevent a missed dose?

**Answer**: Partially. The escalation system (`medication.rs:126-141`) correctly computes when to remind (50%, 75%, 100% of window), but the **notification delivery mechanism is absent**. The `check_pending_doses()` method at `medication.rs:306-322` returns `Vec<DoseWindow>` with escalation levels, but nothing in these files bridges to Android notifications. The proactive suggestion engine (`suggestions.rs:697`) references "Take medication" as a trigger category, suggesting integration exists at a higher level — but the actual push notification chain is incomplete.

**To truly prevent missed doses**: The cron job must call `check_pending_doses()` at sub-minute intervals, map escalation levels to Android notification priorities (Silent=none, Gentle=low, Urgent=high with sound, Missed=persistent+vibrate), and handle the "dose taken" confirmation flow back into `record_dose()`.

### Q3: Is fitness tracking real step counting or estimation?

**Answer**: Step counting is **receiver-only**. `FitnessTracker::record_steps()` at `fitness.rs:159` accepts externally-counted step increments. There is no direct accelerometer access or step counting algorithm in this code. The main loop (`main_loop.rs:5328-5334`) has a `step_sync` cron job that reads `activity_score()` but the actual step source isn't visible in these files.

On Android, the `TYPE_STEP_COUNTER` sensor provides hardware step counts without Google Fit API. This is the likely intended source, but the bridge code would live in the Android/JNI layer, not here in Rust.

### Q4: Are vital trends (heart rate, etc.) actually tracked or aspirational?

**Answer**: **The infrastructure is real; the data source is aspirational.** The Welford accumulator and VitalsMonitor are production-quality code. But `VitalReading.source` accepting "watch" (`vitals.rs:78`) implies wearable integration that doesn't exist in this codebase. The main loop at `main_loop.rs:5320` calls `vitals.ingest()` — but without BLE pairing to a smartwatch/fitness band, there's nothing to ingest.

### Q5: What happens if a user has a medical emergency?

**Answer**: **AURA would NOT detect or respond to a medical emergency.** There is:
- No absolute vital sign thresholds (see Section 1.4 Critical Concern)
- No emergency contact notification system
- No integration with emergency services (SOS)
- No fall detection
- No "user hasn't moved phone in 24h" check
- The anomaly detection at `vitals.rs:291` logs a `warn!` trace — this goes to the developer log, not to the user or their emergency contacts

**This is the single biggest gap in the health domain.**

### Q6: How does health data survive a phone restart or app update?

**Answer**: All health structs derive `Serialize, Deserialize` (serde), enabling persistence. The `HealthDomain` struct at `mod.rs:52` is `Serialize, Deserialize`. The vault system (`vault.rs:141-142`) classifies health data as **Tier 2: Sensitive** — "Keystore + encrypted DB." This means health data is encrypted at rest using Android Keystore-derived keys and persisted in SQLite. This survives restarts and updates.

**However**: The actual serialization call (writing `HealthDomain` to the vault) is not visible in these 4 files. It would be in the persistence layer. The data model is persistence-ready; whether it's actually persisted depends on the integration layer.

### Q7: How does this compare to Apple Health / Google Fit / Samsung Health?

| Capability | AURA v4 | Apple Health | Google Fit | Samsung Health |
|-----------|---------|-------------|-----------|----------------|
| Sleep tracking | Model ready, no sensor | Watch-based, automatic | Estimated via phone | Watch-based, automatic |
| Sleep quality scoring | PSQI-adapted, personalized | Simple duration-based | Basic | Multi-stage analysis |
| Medication reminders | Schedule + escalation model | Via Health app (iOS 16+) | No native support | Samsung Health+ (paid) |
| Step counting | Receiver-only | HW sensor, automatic | HW sensor, automatic | HW sensor, automatic |
| Vital signs | Statistical framework, no sensor | Watch-integrated | Limited | Watch-integrated |
| Anomaly detection | Welford z-score, statistical | Irregular rhythm detection | No | Irregular HR detection |
| Calorie estimation | MET-based, per-activity | Multiple sensors + ML | Estimated | Multiple sensors + ML |
| On-device privacy | 100% local, encrypted | Mostly local, iCloud optional | Cloud-synced | Cloud-synced |
| Personalization | EMA-based learning of patterns | Population-based | Population-based | Population-based |
| Proactive recommendations | Per-domain engines | Minimal | Trends only | Coaching features |
| Emergency detection | **ABSENT** | Fall detection, crash detection | No | Fall detection (watch) |

**Honest Assessment**: AURA's **algorithmic sophistication** (personalized PSQI, Welford anomaly detection, escalating medication management, importance-weighted adherence) exceeds what Apple Health and Google Fit offer natively. But its **data acquisition** is its Achilles heel — without reliable sensor input, the algorithms run on nothing. Apple and Samsung win because they have tightly integrated hardware (Watch/Galaxy Watch). AURA's privacy advantage is real and significant — no health data leaves the device, ever.

---

## 3. REAL-WORLD SCENARIO: A Day in the Life

### 6:45 AM — User wakes up

**What should happen**: AURA detects phone motion after stillness period, infers wake time, auto-records sleep onset (from last night's screen-off + stillness at ~11pm) and wake time. Computes: 7h 45m sleep, 92% efficiency, quality score 0.78/1.0.

**What actually happens**: Sleep data must be manually entered or pushed from an external source. If someone entered yesterday's data, `quality_score()` works correctly. The recommendation engine would say nothing (good sleep). But AURA can't auto-detect wake-up.

**Grade for this moment**: C- (algorithm ready, automation missing)

### 7:15 AM — Morning medication (Levothyroxine + Vitamin D)

**What should happen**: At 7:00 AM, `check_pending_doses()` returns two DoseWindows with `EscalationLevel::Silent`. At 7:15 (50% of 30-min window), escalation becomes `Gentle` — AURA sends a soft notification. User marks both as taken.

**What actually happens**: The escalation computation is correct (`medication.rs:126-141`). But the notification delivery chain is missing. If a cron job calls `check_pending_doses()` and some integration code maps this to Android notifications, it works. The `record_dose()` at `medication.rs:199` correctly logs the dose. Adherence scoring updates immediately.

**Grade for this moment**: B- (logic works, notification delivery uncertain)

### 12:30 PM — Lunch walk (30 minutes)

**What should happen**: AURA counts steps via accelerometer during the walk. At the end, `record_steps(now, 3500)` updates the daily count. If this is a detected workout (sustained walking pace), a WorkoutSession is created with MET-based calorie estimation: `3.5 * 70 * 0.5 = 122.5 kcal`.

**What actually happens**: If step data arrives from the Android sensor layer, `record_steps()` at `fitness.rs:159` correctly accumulates. `estimate_calories(Walking, 1800)` at `fitness.rs:192` correctly returns ~122.5 kcal. The sedentary timer resets. But automatic workout detection doesn't exist — the workout must be manually started/stopped.

**Grade for this moment**: B (step counting plumbing exists, auto-workout detection missing)

### 3:00 PM — Afternoon check (sedentary alert)

**What should happen**: If the user has been at their desk since 1:00 PM (2 hours), `sedentary_check(now)` at `fitness.rs:319` should return `Some(7200)` and trigger a "time to move" notification.

**What actually happens**: If `last_activity_at` was set by the noon walk, and now it's 3:00 PM (2.5 hours later), `sedentary_check()` correctly returns `Some(9000)`. The recommendation engine at `fitness.rs:402-409` generates a sedentary alert with confidence 0.85. But delivery to the user depends on the proactive engine invoking `generate_recommendations()`.

**Grade for this moment**: B (detection correct, delivery depends on integration)

### 6:00 PM — Evening medication (Metformin)

**What should happen**: Escalation at 6:00 = Silent, at 6:15 = Gentle, at 6:22 = Urgent, at 6:30 = Missed.

**What actually happens**: `DoseWindow::compute_escalation(scheduled, 1800, now)` at `medication.rs:126` correctly computes all 4 levels. The nudge engine (`medication.rs:405`) could detect a pattern of consistently late doses and suggest adjusting the schedule.

**Grade for this moment**: B (same notification delivery gap)

### 10:45 PM — Bedtime (late-night usage detection)

**What should happen**: At 10:45 PM, AURA notices the user is still on their phone. If this is the 3rd consecutive night, `is_late_night_concerning()` returns true and a high-priority recommendation fires.

**What actually happens**: `record_late_night_usage(timestamp)` at `sleep.rs:235` correctly compares against the adaptive threshold (1h before learned bedtime). After 3 days, `is_late_night_concerning()` returns true. The recommendation engine at `sleep.rs:510-519` generates a screen_time alert with priority High, confidence 0.9.

**Grade for this moment**: A- (fully functional if the integration layer calls it)

### 11:30 PM — Daily health score computation

**What should happen**: The cron job `health_score_compute` runs. All 4 sub-scores are computed and combined.

**What actually happens**: `main_loop.rs:5342-5358` shows this IS wired up. `compute_score()` at `mod.rs:93-128` correctly combines all sub-scores, updates trend EMA, transitions lifecycle, and stores the result in the state store. The weekly report at `main_loop.rs:5360-5379` generates a formatted health summary.

**Grade for this moment**: A (fully wired and functional)

---

## 4. PRIVACY & LIABILITY ASSESSMENT

### Privacy

**Strengths**:
- All health data stays on-device (AURA's core philosophy)
- Health data classified as Tier 2: Sensitive in the vault (`vault.rs:141-142`)
- Tier 2 requires Keystore + encrypted DB (`vault.rs:150-152`)
- No telemetry, no cloud sync, no data sharing APIs
- Serde serialization means data format is controlled

**Weaknesses**:
- No data export functionality (GDPR/CCPA right to data portability)
- No explicit data retention policy visible in these files (365-day ring buffers are implicit retention)
- No user consent flow for health data collection visible here
- No audit log of who/what accessed health data

### Liability

**HIGH RISK ITEMS**:
1. **Medication advice without medical disclaimer**: The nudge engine (`medication.rs:405-476`) gives advice like "try setting a reminder" and "consider adjusting your reminder time." These are behavioral suggestions, not medical advice — this is correct framing.

2. **No "AURA is not a medical device" disclaimer**: This should be displayed whenever health data is shown to the user. Not visible in these files.

3. **Vital sign anomaly alerts could cause false panic or false reassurance**: A z-score alert for elevated heart rate could send a hypochondriac to the ER unnecessarily. Conversely, the absence of absolute thresholds means a genuinely dangerous reading (SpO2=85%) might not alert at all.

4. **Medication tracking is NOT a medical device**: Under FDA 21 CFR 820 / EU MDR, a system that actively manages medication dosing could be classified as a Software as Medical Device (SaMD). AURA's current implementation (reminders + tracking) likely falls under FDA's "wellness" exemption, but any claims about health outcomes would change this.

---

## 5. INTEGRATION ANALYSIS

### How health data flows through AURA

```
Sensor/User Input
      |
      v
[SleepTracker / MedicationManager / FitnessTracker / VitalsMonitor]
      |
      v
[HealthDomain.compute_score()] -- mod.rs:93
      |
      v
[ArcManager.state_store] -- main_loop.rs:5346-5351
      |
      v
[Proactive Suggestion Engine] -- suggestions.rs:94 (health threshold triggers)
      |
      v
[User Notification / Weekly Report] -- main_loop.rs:5360-5379
```

**Evidence of integration**:
- Cron jobs wired: `vital_ingest`, `step_sync`, `sleep_infer`, `health_score_compute`, `health_weekly_report` (`main_loop.rs:5320-5379`, `cron.rs:357,720`)
- State store updates health score for cross-domain use (`main_loop.rs:5346-5351`)
- Proactive engine references health thresholds (`suggestions.rs:94`)
- Welcome system mentions health features (`welcome.rs:528`)
- Learning dimensions track "health" as app category (`dimensions.rs:81`)

**Gaps in integration**:
- Sleep quality doesn't adjust notification timing (AURA should be quieter when user is sleeping poorly)
- Medication adherence doesn't feed into the proactive engine's urgency calculation
- Fitness data doesn't influence sleep recommendations (exercise timing affects sleep quality)
- No cross-domain insights ("You slept better on days you exercised" — this would be transformative)

---

## 6. GRADES

### Per-File Grades

| File | Grade | Justification |
|------|-------|---------------|
| `sleep.rs` | **B+** | Excellent PSQI adaptation, EMA-based personalization, comprehensive recommendation engine, solid tests. Loses points for no actual sensor integration, no circadian analysis, and missing detection of pathologically short latency. |
| `medication.rs` | **B** | Robust scheduling model, smart escalation system, importance-weighted adherence, behavioral nudges. Loses points for UTC-only timezone handling (critical bug), absent notification delivery, no drug interactions, no PRN safety tracking. |
| `fitness.rs` | **B-** | Good activity model, correct MET values, WHO-aligned targets, sedentary detection. Loses points for stub sedentary scoring (`fitness.rs:224-228`), no auto-workout detection, calories-per-step not weight-adjusted, no HR integration in scoring. |
| `vitals.rs` | **B-** | Excellent Welford implementation, clean multi-channel architecture, proper anomaly detection framework. **Critically** loses points for no absolute physiological bounds (z-score-only is medically dangerous), no emergency detection, no trend/slope analysis, no wearable bridge. |
| `mod.rs` | **A-** | Clean aggregation, correct weight balance, trend EMA for momentum, lifecycle state machine, well-tested. Only loses points for no cross-domain insight generation. |

### Overall Health Domain Grade: **B**

**Justification**: The algorithmic foundation is genuinely impressive — PSQI adaptation, Welford's algorithm, EMA personalization, weighted adherence, behavioral nudges with confidence scores. This is not toy code; it reflects real clinical knowledge. However, the domain is fundamentally limited by two gaps:

1. **Data acquisition**: All 4 sub-engines are receivers only. Without reliable sensor bridges (accelerometer, BLE wearable, screen state), the algorithms compute over nothing. This is fixable with Android sensor integration, but it's the difference between "impressive framework" and "working health system."

2. **Medical safety**: The absence of absolute vital sign thresholds and emergency detection is a liability. A user trusting AURA for health monitoring who has a medical emergency would receive no alert. This must be addressed before any health feature is promoted to users.

---

## 7. IRREPLACEABILITY ASSESSMENT

**Would these features make someone depend on AURA in a healthy way?**

**Potential: HIGH.** If fully connected to sensors, AURA's health domain would be genuinely superior to Apple Health / Google Fit in three ways:

1. **Personalization**: The EMA-based learning (learned ideal sleep hours, adaptive late-night thresholds) means AURA's recommendations get better over time. Apple Health uses population norms. AURA learns YOU.

2. **Privacy**: Health data never leaves the device. For users with sensitive conditions (mental health, HIV, reproductive health), this is not just a feature — it's a fundamental right.

3. **Cross-domain intelligence**: AURA can (once wired) correlate sleep, medication, fitness, and vitals to produce insights no siloed app can. "Your blood pressure is lower on days you sleep >7 hours and exercise" is the kind of insight that changes behavior.

**Current state**: AURA is at "impressive prototype" stage for health. The algorithms are publication-quality. The integration is framework-level. The sensor acquisition is absent. The medical safety guardrails need work.

**To cross the irreplaceability threshold**, AURA needs:
1. Reliable data acquisition (even just phone accelerometer + screen state)
2. Absolute vital sign safety bounds
3. Cross-domain correlation engine
4. Medical disclaimer system
5. Emergency contact/SOS integration

---

## 8. MEDICAL ACCURACY ASSESSMENT (Summary)

| Standard/Guideline | Referenced In AURA | Accuracy |
|--------------------|--------------------|----------|
| Pittsburgh Sleep Quality Index (Buysse, 1989) | sleep.rs:1-15 | Adapted correctly, 5 of 7 components mapped |
| WHO Physical Activity Guidelines (2020) | fitness.rs:217, 364-381 | 150 min/week target correctly applied |
| Compendium of Physical Activities (Ainsworth, 2011) | fitness.rs:25-29, 54-67 | MET values within published ranges |
| Welford's Online Algorithm (1962) | vitals.rs:85-177 | Mathematically correct implementation |
| AASM Sleep Efficiency Threshold (>85%) | sleep.rs:332 | Correctly applied |
| NSF Sleep Duration (7-9h adults) | sleep.rs:32-38 | Default 7.5h appropriate |

**Overall Medical Accuracy**: **Good with critical gaps.** The science that IS implemented is correct. What's missing (absolute vital bounds, drug interactions, emergency detection) is more dangerous than what's wrong.

---

## 9. CREATIVE SOLUTIONS — WHAT AURA SHOULD BUILD NEXT

### 1. "Sleep Inference Engine" (No wearable needed)
Using only phone sensors available to an AccessibilityService:
- Screen on/off events → onset/wake time estimation
- Last app interaction → bedtime approximation
- Charging state + stillness → sleep confirmation
- Alarm app detection → planned wake time
- This alone would make sleep tracking functional for 90% of users

### 2. "Vital Bounds Table" (2 hours of work, saves lives)
Add a `VitalBounds` struct with min/max acceptable ranges per VitalType. Check BEFORE z-score. Any reading outside absolute bounds triggers immediate alert regardless of statistical history.

### 3. "Health Correlation Engine" (The irreplaceability feature)
Every night at health_score_compute time, run a simple correlation analysis:
- Sleep quality vs. exercise that day
- Medication adherence vs. sleep quality
- Steps vs. mood (if AURA has mood data)
- After 30 days, surface the top 3 correlations as insights

### 4. "Emergency Buddy System"
Allow user to designate 1-3 emergency contacts. If:
- Vital anomaly detected AND no phone interaction for 1+ hours → SMS emergency contact
- Medication missed for 2+ consecutive days on critical med → notify caregiver
- This requires minimal code but could literally save a life

### 5. "Medication Smart Window"
Instead of fixed 30-minute windows, learn from the user's actual timing patterns via EMA (like sleep already does). If a user consistently takes their evening med at 6:45 PM instead of 6:00 PM, shift the window center. The nudge engine already detects this pattern (`medication.rs:446-469`) — just close the loop.

---

## RETURN FORMAT

```json
{
  "status": "ok",
  "skill_loaded": ["health-informatics-audit"],
  "file_grades": {
    "sleep.rs": "B+",
    "medication.rs": "B",
    "fitness.rs": "B-",
    "vitals.rs": "B-",
    "mod.rs": "A-"
  },
  "overall_grade": "B",
  "key_findings": [
    "All 4 health sub-engines have production-quality algorithms with correct medical science",
    "Data acquisition is the critical gap — all engines are receivers only, no sensor integration",
    "CRITICAL: vitals.rs has NO absolute physiological bounds — z-score-only detection is medically dangerous",
    "CRITICAL: medication.rs uses UTC midnight (line 331), broken for non-UTC timezone users",
    "Medication escalation system is well-designed but notification delivery chain is absent",
    "Sleep PSQI adaptation is excellent — 5 weighted components with EMA personalization",
    "Welford's algorithm implementation is textbook-correct and memory-efficient O(1)",
    "Health composite scoring is properly wired into main loop cron jobs and state store",
    "Weekly health report generation exists and is functional (main_loop.rs:5360-5379)",
    "All files have comprehensive test suites (54 total tests across the domain)"
  ],
  "medical_accuracy_assessment": "Good with critical gaps. Implemented science is correct (PSQI, MET values, Welford, WHO guidelines). Missing absolute vital bounds and emergency detection are dangerous omissions.",
  "real_world_scenario": "Day-in-the-life shows algorithms work correctly when data is provided. Main gaps: no auto sleep inference, notification delivery uncertain, no emergency response. Cron integration for score computation and weekly reports IS wired and functional.",
  "privacy_risks": [
    "Health data correctly classified as Tier 2 Sensitive with Keystore encryption",
    "Missing: data export for GDPR portability",
    "Missing: explicit user consent flow for health data collection",
    "Missing: 'not a medical device' disclaimer",
    "STRENGTH: 100% on-device, no cloud, no telemetry — best-in-class privacy"
  ],
  "irreplaceability_potential": "HIGH if sensor integration is completed. EMA-based personalization + on-device privacy + cross-domain correlation potential = genuinely superior to Apple Health/Google Fit/Samsung Health. Currently at 'impressive prototype' stage.",
  "comparison_to_competitors": "Algorithmic sophistication exceeds Apple Health and Google Fit. Data acquisition far behind all competitors. Privacy is best-in-class. Emergency detection absent (competitors: Apple Watch has fall detection + crash detection + irregular rhythm).",
  "creative_solutions": [
    "Sleep Inference Engine using screen-off + charging + stillness signals (no wearable needed)",
    "Vital Bounds Table with absolute physiological limits (critical safety feature, 2h of work)",
    "Health Correlation Engine for cross-domain insights (the irreplaceability feature)",
    "Emergency Buddy System with SMS to emergency contacts on vital anomalies",
    "Medication Smart Window — EMA-based dose window adaptation (pattern already detected by nudge engine)"
  ],
  "total_loc_audited": 2946,
  "total_tests_found": 54,
  "artifacts": ["checkpoints/2h-p1b-health-domain.md"],
  "token_cost_estimate": 12000,
  "time_spent_secs": 300,
  "next_steps": [
    "Implement VitalBounds absolute threshold system (blocks medical safety risk)",
    "Fix medication.rs UTC timezone bug at line 331",
    "Build sleep inference from AccessibilityService screen/motion events",
    "Wire medication escalation levels to Android notification priorities",
    "Add cross-domain correlation engine for health insights"
  ]
}
```
