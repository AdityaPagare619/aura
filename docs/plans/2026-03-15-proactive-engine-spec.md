# AURA v4 Proactive Engine — Complete Specification

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Specify the complete Proactive Engine — the background intelligence that makes AURA reach out to the user, not just respond.

**Architecture:** Signal monitors (Rust) detect on-device events → Decision Engine (Rust) evaluates priority, batches, gates by trust/consent → LLM (Neocortex) generates natural language → Channel Selector (Rust) picks delivery method → User receives proactive message.

**Iron Laws:**
- LLM = brain (decides WHAT to say). Rust daemon = body (decides WHEN/WHERE to deliver).
- Daemon NEVER writes user-facing text. Only sends `ProactiveContext` IPC → Neocortex generates language.
- All signal monitoring is on-device. No cloud telemetry. Anti-cloud.
- User Sovereignty: user controls proactiveness level at all times.

---

## Deliverable 1: Proactive Engine Architecture

### Component Overview

```
┌─────────────────────────────────────────────────────────┐
│                   AURA Daemon (Rust)                     │
│                                                          │
│  ┌──────────────┐    ┌──────────────┐    ┌────────────┐ │
│  │   Signal      │───▶│  Decision    │───▶│  Channel   │ │
│  │   Monitors    │    │  Engine      │    │  Selector  │ │
│  └──────────────┘    └──────┬───────┘    └─────┬──────┘ │
│                             │                   │        │
│  Signals:                   │ Filters:          │ Routes: │
│  • Time/Cron               │ • Trust gate      │ • Voice │
│  • Device state            │ • Consent gate    │ • Telegram│
│  • Notification listener   │ • Budget gate     │ • Notification│
│  • Pattern detector        │ • Quiet hours     │ • Silent log│
│  • Goal/routine tracker    │ • Batch queue     │          │
│  • Social gap monitor      │ • Priority eval   │          │
│                             │                   │          │
│                     ┌───────▼───────┐           │        │
│                     │  IPC Message  │           │        │
│                     │ ProactiveCtx  │───────────┘        │
│                     └───────┬───────┘                    │
│                             │                            │
└─────────────────────────────┼────────────────────────────┘
                              │ DaemonToNeocortex::ProactiveContext
                              ▼
┌─────────────────────────────────────────────────────────┐
│                   Neocortex (LLM)                        │
│                                                          │
│  Receives: ProactiveTrigger + ContextPackage             │
│  Generates: Natural language message                     │
│  Returns: Response text → daemon routes to channel       │
└─────────────────────────────────────────────────────────┘
```

### Existing Components (Already Built)

| Component | File | Status |
|-----------|------|--------|
| `ProactiveEngine` | `arc/proactive/mod.rs` (661 lines) | ✅ Initiative budget, tick(), 4 action types |
| `ProactiveDispatcher` | `daemon_core/proactive_dispatcher.rs` (577 lines) | ✅ 6 trigger types, `should_dispatch()`, IPC conversion |
| `ProactiveConsent` | `identity/proactive_consent.rs` (117 lines) | ✅ Consent enum, quiet hours, max-per-hour |
| `MorningBriefing` | `arc/proactive/morning.rs` | ✅ Daily briefing generation |
| `SuggestionEngine` | `arc/proactive/suggestions.rs` | ✅ Context-aware suggestions |
| `RoutineManager` | `arc/proactive/routines.rs` | ✅ Routine tracking & deviation |
| `ForestGuardian` | `arc/proactive/attention.rs` | ✅ Anti-doomscrolling nudges |
| Main loop wiring | `daemon_core/main_loop.rs:5638-5773` | ✅ `cron_handle_proactive()` |
| IPC types | `aura-types/ipc.rs` | ✅ `ProactiveTrigger`, `ContextPackage` |

### Components to Build/Enhance

| Component | What | Priority |
|-----------|------|----------|
| **Signal Monitor Registry** | Unified signal bus that sub-monitors push to | Alpha |
| **Notification Listener** | Android `NotificationListenerService` bridge via JNI | Beta |
| **Pattern Detector** | Learns user behavior patterns over time | Beta |
| **Channel Selector** | Intelligent routing (voice/telegram/notification/silent) | Alpha |
| **Trust-Gated Scaler** | Scales proactive frequency/types by trust level | Alpha |
| **Batch Queue** | Accumulates low-priority triggers, delivers as digest | Beta |
| **User Preference Controls** | Always/Smart/Minimal/Never modes replacing simple consent | Alpha |

### Data Flow (Detailed)

1. **Signal fires** — A cron job, device event, or pattern match produces a `RawSignal`
2. **Monitor classifies** — The signal monitor categorizes it as one of the `ProactiveTrigger` types
3. **Decision Engine evaluates:**
   - `consent_gate()` — Is proactivity enabled at all? (consent + mode)
   - `trust_gate()` — Does current trust level allow this trigger type?
   - `budget_gate()` — Is there initiative budget remaining?
   - `quiet_hours_gate()` — Are we in quiet hours?
   - `should_dispatch()` — Does this specific trigger meet its thresholds? (e.g., goal stalled ≥ 3 days)
   - `power_gate()` — Is device power tier sufficient for this action?
4. **Batch or dispatch** — Low-priority triggers enter batch queue; high-priority dispatch immediately
5. **IPC to Neocortex** — `DaemonToNeocortex::ProactiveContext { trigger, context }` sent via IPC
6. **LLM generates response** — Neocortex receives typed trigger + full `ContextPackage` (personality, mood, memories, conversation history) and generates natural language
7. **Channel Selector routes** — Based on urgency, user state, and preference, selects delivery channel
8. **Delivery** — Message sent via selected channel
9. **Feedback loop** — User response (accept/dismiss/reject) feeds back to initiative budget, threat score, Hebbian learning

---

## Deliverable 2: Signal Monitoring Specification

### Signal Categories

| Signal | Source | Collection Method | Frequency | Power Cost |
|--------|--------|-------------------|-----------|------------|
| **Time/Cron** | System clock | Cron job ticks | Every 60s (P2), 300s (P1) | Negligible |
| **Battery/Charging** | Android BatteryManager | JNI callback | On-change events | Negligible |
| **Thermal** | Android thermal API | JNI callback | On-change + 60s poll | Negligible |
| **Screen state** | Android PowerManager | JNI broadcast receiver | On-change events | Negligible |
| **Foreground app** | Android UsageStatsManager | JNI poll | Every 30s when screen on | Low |
| **Step count** | Android SensorManager | JNI batched sensor | Every 300s batched | Low |
| **Network** | Android ConnectivityManager | JNI callback | On-change events | Negligible |
| **Location type** | Inferred from WiFi SSID | JNI callback | On WiFi change | Negligible |
| **Notifications** | NotificationListenerService | JNI bridge (Beta) | On-receive events | Low |
| **User interaction** | App foreground/input | Internal event | On-action | Negligible |
| **Goal deadlines** | Goal store DB | Cron check | Every 3600s | Low |
| **Routine deviation** | RoutineManager | Pattern comparison | Every 300s | Low |
| **Social gap** | Relationship tracker | Cron check | Every 3600s | Negligible |
| **Medication time** | Schedule store | Cron check | Every 60s | Negligible |
| **Birthday/events** | Calendar/memory store | Daily cron | Once/day | Negligible |

### Power Budget Rules

These map to the existing `PowerTier` system:

| PowerTier | Battery | Charging | Signal Polling | Proactive Actions |
|-----------|---------|----------|----------------|-------------------|
| `P0Always` | Any | Any | Critical only (medication, urgent alerts) | Emergency only |
| `P1IdlePlus` | <20% | No | Cron only (300s), no sensor polling | Low-priority suppressed |
| `P2Normal` | 20-80% | No | Full polling (60s cron, 30s foreground) | All gated by budget |
| `P3Charging` | Any | Yes | Full polling + dreaming + pattern analysis | All + background tasks |

### Signal Data Structure (New)

```rust
/// Raw signal from any monitor — feeds into Decision Engine
#[derive(Debug, Clone)]
pub struct RawSignal {
    pub source: SignalSource,
    pub timestamp_ms: u64,
    pub payload: SignalPayload,
    pub power_tier_required: PowerTier,  // Minimum tier to process this
}

#[derive(Debug, Clone)]
pub enum SignalSource {
    Cron(CronJobId),
    DeviceState,        // Battery, thermal, screen
    NotificationBridge, // Android NotificationListenerService (Beta)
    PatternDetector,    // Learned patterns (Beta)
    GoalTracker,
    RoutineManager,
    SocialMonitor,
    ScheduleStore,      // Medications, appointments
}

#[derive(Debug, Clone)]
pub enum SignalPayload {
    TimeEvent { hour: u8, minute: u8 },
    BatteryChange { level: f32, charging: bool },
    ScreenChange { on: bool },
    AppForeground { package: String, duration_secs: u64 },
    NotificationReceived { app: String, title: String, priority: i32 },
    GoalDeadlineApproaching { goal_id: String, hours_remaining: f64 },
    RoutineDeviation { routine: String, deviation_minutes: i32 },
    SocialGap { contact: String, days_since_contact: u32 },
    MedicationDue { name: String, scheduled_time_ms: u64 },
    PatternMatch { pattern_id: String, confidence: f32 },
}
```

### Collection Architecture

```
┌─────────────────────────────────────────┐
│           Signal Monitor Registry        │
│                                          │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐ │
│  │  Cron   │  │ Device  │  │ Android │ │
│  │ Monitor │  │ Monitor │  │ Bridge  │ │
│  └────┬────┘  └────┬────┘  └────┬────┘ │
│       │            │            │        │
│       ▼            ▼            ▼        │
│  ┌──────────────────────────────────┐   │
│  │     signal_tx: Sender<RawSignal> │   │
│  └──────────────┬───────────────────┘   │
│                 │                        │
└─────────────────┼────────────────────────┘
                  │
                  ▼
          Decision Engine
     (receives via signal_rx)
```

Each monitor is a lightweight struct that pushes `RawSignal`s into a shared `mpsc` channel. The Decision Engine drains the channel each tick.

**Alpha monitors:** Cron (already exists), Device state (partially exists via `UserStateSignals`), Goal/Routine/Social (already exist as cron handlers).

**Beta monitors:** Notification bridge (new JNI work), Pattern detector (new ML-lite system).

---

## Deliverable 3: Decision Algorithm

### Pipeline

```
RawSignal
    │
    ▼
[1. Consent Gate] ──── Blocked? → Drop signal
    │
    ▼
[2. Mode Filter] ──── ProactiveMode::Never? → Drop
    │                  ProactiveMode::Minimal? → Only critical
    │
    ▼
[3. Trust Gate] ──── Trust too low for this trigger type? → Drop
    │
    ▼
[4. Power Gate] ──── PowerTier insufficient? → Defer to queue
    │
    ▼
[5. Budget Gate] ──── Initiative budget depleted? → Defer to queue
    │
    ▼
[6. Threshold Check] ──── `should_dispatch()` — trigger-specific thresholds not met? → Drop
    │
    ▼
[7. Priority Evaluation]
    │
    ├── Critical (medication, urgent alert) ──── Dispatch immediately
    ├── High (goal deadline <24h, health alert) ──── Dispatch if budget allows
    ├── Medium (routine deviation, social gap) ──── Batch if >2 pending, else dispatch
    └── Low (suggestion, insight, pattern) ──── Always batch
    │
    ▼
[8. Batch Queue] ──── Timer expires (30 min) OR batch size ≥ 3? → Flush as digest
    │
    ▼
[9. Build IPC Message] ──── ProactiveContext { trigger, context_package }
    │
    ▼
[10. Send to Neocortex] ──── LLM generates natural language response
    │
    ▼
[11. Channel Selection] ──── Pick delivery method (see Deliverable 4)
    │
    ▼
[12. Deliver + Log]
    │
    ▼
[13. Feedback] ──── User response updates budget, threat score, Hebbian weights
```

### Gate Implementation (Rust)

```rust
impl DecisionEngine {
    /// Main evaluation pipeline. Returns None if signal should be dropped.
    pub fn evaluate(&mut self, signal: RawSignal, ctx: &EngineContext) -> Option<ProactiveAction> {
        // Gate 1: Consent
        if !ctx.consent.can_proact(signal.timestamp_ms) {
            return None;
        }

        // Gate 2: Mode filter
        let priority = self.classify_priority(&signal);
        match ctx.proactive_mode {
            ProactiveMode::Never => return None,
            ProactiveMode::Minimal => if priority < Priority::Critical { return None; },
            ProactiveMode::Smart => {}, // All pass, budget handles throttling
            ProactiveMode::Always => {}, // All pass, reduced budget cost
        }

        // Gate 3: Trust
        if !self.trust_allows(&signal, ctx.trust_level) {
            return None;
        }

        // Gate 4: Power
        if signal.power_tier_required > ctx.power_tier {
            self.defer_queue.push(signal);
            return None;
        }

        // Gate 5: Budget
        let cost = self.action_cost(priority);
        if !ctx.initiative_budget.can_afford(cost) && priority < Priority::Critical {
            self.batch_queue.push(signal);
            return None;
        }

        // Gate 6: Trigger-specific thresholds (existing should_dispatch logic)
        if !self.should_dispatch(&signal) {
            return None;
        }

        // Gate 7-8: Priority routing
        match priority {
            Priority::Critical => {
                ctx.initiative_budget.spend(cost);
                Some(self.build_action(signal, ctx))
            }
            Priority::High => {
                ctx.initiative_budget.spend(cost);
                Some(self.build_action(signal, ctx))
            }
            Priority::Medium => {
                if self.batch_queue.pending_medium() >= 2 {
                    self.batch_queue.push(signal);
                    None // Will flush as digest later
                } else {
                    ctx.initiative_budget.spend(cost);
                    Some(self.build_action(signal, ctx))
                }
            }
            Priority::Low => {
                self.batch_queue.push(signal);
                None // Always batched
            }
        }
    }
}
```

### Priority Classification

| Trigger Type | Priority | Budget Cost | Batch? |
|-------------|----------|-------------|--------|
| MedicationDue | Critical | 0.0 (free) | Never |
| HealthAlert (high urgency) | Critical | 0.0 | Never |
| GoalDeadlineApproaching (<24h) | High | 0.05 | Never |
| HealthAlert (medium) | High | 0.05 | Never |
| RoutineDeviation (≥30 min) | Medium | 0.08 | If ≥2 pending |
| SocialGap (≥7 days) | Medium | 0.08 | If ≥2 pending |
| GoalStalled (≥3 days) | Medium | 0.08 | If ≥2 pending |
| Suggestion | Low | 0.10 | Always |
| PatternMatch | Low | 0.10 | Always |
| MemoryInsight | Low | 0.12 | Always |

### Batch Queue Flush Rules

```rust
pub struct BatchQueue {
    items: Vec<(RawSignal, Instant)>,
    max_age: Duration,      // 30 minutes
    max_size: usize,        // 5 items
    min_flush_size: usize,  // 2 items (don't send digest for 1 item)
}

impl BatchQueue {
    /// Returns signals ready for digest delivery
    pub fn try_flush(&mut self, now: Instant) -> Option<Vec<RawSignal>> {
        let oldest = self.items.first().map(|(_, t)| *t)?;

        let should_flush = self.items.len() >= self.max_size
            || (now - oldest >= self.max_age && self.items.len() >= self.min_flush_size);

        if should_flush {
            Some(self.items.drain(..).map(|(s, _)| s).collect())
        } else {
            None
        }
    }
}
```

When flushed, batched signals are sent as a single `ProactiveTrigger::Digest` to the LLM, which generates a combined message like: *"A few things: your morning run was 45 min late today, you haven't talked to Mom in 8 days, and I noticed you tend to be more productive when you start with deep work before checking messages."*

---

## Deliverable 4: Channel Selection Matrix

### Channels

| Channel | Mechanism | When Available |
|---------|-----------|----------------|
| **Voice** | TTS via `VoiceBridge` | Device speaker/BT connected, not in DND |
| **Telegram** | `TelegramPoller` message queue | Telegram configured, network available |
| **Notification** | Android notification (future) | Always (Android-level) |
| **Silent Log** | Written to proactive log, visible in AURA UI | Always |

### Selection Matrix

| Condition | Critical | High | Medium/Low |
|-----------|----------|------|------------|
| **Screen ON + AURA foreground** | Voice | Voice (short) or Telegram | Telegram |
| **Screen ON + other app** | Notification | Notification | Silent Log |
| **Screen OFF + not DND** | Notification → Telegram | Telegram | Silent Log |
| **Screen OFF + DND** | Notification (silent) | Silent Log | Silent Log |
| **Driving (inferred)** | Voice | Voice | Silent Log |
| **Voice session active** | Voice (interrupt) | Voice (queue after current) | Silent Log |
| **Telegram-only mode** | Telegram | Telegram | Telegram |
| **No network** | Voice / Silent Log | Silent Log | Silent Log |

### Selection Algorithm

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum DeliveryChannel {
    Voice,
    Telegram,
    Notification,
    SilentLog,
}

pub struct ChannelSelector {
    voice_available: bool,
    telegram_available: bool,
    notification_available: bool,
}

impl ChannelSelector {
    pub fn select(
        &self,
        priority: Priority,
        user_state: &UserStateSignals,
        voice_pref: VoiceModePreference,
        input_source: Option<InputSource>,
    ) -> DeliveryChannel {
        let screen_on = user_state.is_screen_on.unwrap_or(false);
        let is_dnd = matches!(user_state.context_mode(), Some(ContextMode::DoNotDisturb));
        let aura_foreground = user_state.foreground_app.as_deref() == Some("com.aura.app");
        let driving = self.infer_driving(user_state);

        // Critical: always reaches user somehow
        if priority == Priority::Critical {
            if driving && self.voice_available {
                return DeliveryChannel::Voice;
            }
            if self.voice_available && (aura_foreground || screen_on) && !is_dnd {
                return DeliveryChannel::Voice;
            }
            if self.telegram_available {
                return DeliveryChannel::Telegram;
            }
            if self.notification_available {
                return DeliveryChannel::Notification;
            }
            return DeliveryChannel::SilentLog; // Last resort
        }

        // DND: only critical gets through (handled above), rest silenced
        if is_dnd {
            return DeliveryChannel::SilentLog;
        }

        // Driving: voice for high, silent for rest
        if driving {
            if priority >= Priority::High && self.voice_available {
                return DeliveryChannel::Voice;
            }
            return DeliveryChannel::SilentLog;
        }

        // Voice session active: queue voice for high, else log
        if matches!(input_source, Some(InputSource::Voice)) {
            if priority >= Priority::High && self.voice_available {
                return DeliveryChannel::Voice;
            }
            return DeliveryChannel::SilentLog;
        }

        // Screen on + AURA foreground: use voice or telegram
        if screen_on && aura_foreground {
            if priority >= Priority::High && self.voice_available
                && voice_pref != VoiceModePreference::Never
            {
                return DeliveryChannel::Voice;
            }
            if self.telegram_available {
                return DeliveryChannel::Telegram;
            }
        }

        // Screen on + other app: notification for high, log for low
        if screen_on {
            if priority >= Priority::High && self.notification_available {
                return DeliveryChannel::Notification;
            }
            return DeliveryChannel::SilentLog;
        }

        // Screen off: telegram for high, log for rest
        if priority >= Priority::High && self.telegram_available {
            return DeliveryChannel::Telegram;
        }

        DeliveryChannel::SilentLog
    }

    fn infer_driving(&self, state: &UserStateSignals) -> bool {
        // Heuristic: location type is "vehicle" or specific apps in foreground
        matches!(
            state.estimated_location_type.as_deref(),
            Some("vehicle") | Some("driving")
        )
    }
}
```

### Voice-Specific Rules

Inherited from existing `VoiceModePreference`:
- **Always**: Voice used whenever available
- **Smart**: Voice used if message is short AND non-technical AND last interaction was voice
- **Never**: Voice never used for proactive messages

The Channel Selector respects `VoiceModePreference` — if `Never`, voice is never selected even for Critical priority (falls through to Telegram/Notification).

---

## Deliverable 5: Trust-Gated Behavior

### Trust Levels and Proactive Capabilities

The existing trust system (0.0–1.0) maps to proactive capabilities:

| Trust Range | Relationship Stage | Proactive Capabilities |
|-------------|-------------------|----------------------|
| **0.00–0.15** | Stranger | **None.** Only respond when spoken to. No proactive messages at all. |
| **0.15–0.35** | Acquaintance | **Reminders only.** Explicit user-set timers, medication reminders. Nothing inferred. |
| **0.35–0.60** | Friend | **Scheduled + gentle.** Morning briefings, goal deadline reminders, routine deviations (>60 min only). Max 5 proactive/day. |
| **0.60–0.85** | Close Friend | **Pattern-aware.** All above + social gap nudges, suggestions based on patterns, attention guardianship, health observations. Max 15 proactive/day. |
| **0.85–1.00** | Soulmate | **Full initiative.** All above + proactive task execution (with confirmation), memory insights, behavioral observations, "I noticed..." messages. Max 30 proactive/day. |

### Implementation

```rust
/// Maps trust level to allowed trigger types
pub fn trust_allows(trust: f32, trigger: &ProactiveTrigger) -> bool {
    match trigger {
        // Always allowed if user explicitly set them (regardless of trust)
        ProactiveTrigger::MedicationDue | ProactiveTrigger::UserSetReminder => true,

        // Acquaintance+ (0.15)
        ProactiveTrigger::GoalOverdue => trust >= 0.15,

        // Friend+ (0.35)
        ProactiveTrigger::MorningBriefing => trust >= 0.35,
        ProactiveTrigger::GoalDeadlineApproaching => trust >= 0.35,
        ProactiveTrigger::RoutineDeviation { deviation_minutes, .. } => {
            if trust >= 0.60 { true }
            else if trust >= 0.35 { *deviation_minutes >= 60 }
            else { false }
        },

        // Close Friend+ (0.60)
        ProactiveTrigger::SocialGap => trust >= 0.60,
        ProactiveTrigger::Suggestion => trust >= 0.60,
        ProactiveTrigger::AttentionGuardian => trust >= 0.60,
        ProactiveTrigger::HealthAlert => trust >= 0.60,

        // Soulmate (0.85)
        ProactiveTrigger::MemoryInsight => trust >= 0.85,
        ProactiveTrigger::PatternObservation => trust >= 0.85,
        ProactiveTrigger::ProactiveTaskExecution => trust >= 0.85,
    }
}

/// Daily limit scaled by trust
pub fn daily_proactive_limit(trust: f32) -> u32 {
    if trust < 0.15 { 0 }
    else if trust < 0.35 { 3 }   // Explicit reminders only
    else if trust < 0.60 { 5 }   // Gentle scheduled
    else if trust < 0.85 { 15 }  // Pattern-aware
    else { 30 }                    // Full initiative
}

/// Initiative budget regeneration rate scaled by trust
/// Higher trust = faster regeneration = more proactive actions possible
pub fn budget_regen_rate(trust: f32) -> f32 {
    let base = 0.001; // per second
    base * (0.5 + trust) // 0.5x at trust=0, 1.5x at trust=1.0
}
```

### Trust Recovery After Rejection

The existing `threat_score` (exponential decay of rejections) already tracks this. Integration:

```rust
/// If user dismisses/rejects proactive messages frequently,
/// temporarily reduce proactive behavior even within trust allowance
pub fn rejection_throttle(threat_score: f32) -> f32 {
    // threat_score: 0.0 = no issues, 1.0 = constant rejection
    // Returns multiplier on daily limit: 1.0 = normal, 0.0 = fully throttled
    (1.0 - threat_score).max(0.1) // Never fully zero — critical alerts always possible
}
```

### Hysteresis

Same hysteresis as relationship stages: upgrading requires reaching the threshold, downgrading requires dropping 0.05 below. This prevents flickering at boundaries.

---

## Deliverable 6: User Preference Controls

### ProactiveMode (Replacing Simple Consent)

The current `ProactiveConsent` (`Unasked`/`Declined`/`AcceptedAll`) is too coarse. Replace with:

```rust
/// User-facing proactive behavior modes
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum ProactiveMode {
    /// AURA initiates frequently — suggestions, observations, check-ins.
    /// Only for users who want an actively engaged companion.
    Always,

    /// AURA initiates when contextually appropriate.
    /// Respects quiet hours, batches low-priority, uses trust gating.
    /// DEFAULT for AcceptedAll consent.
    Smart,

    /// AURA only initiates for explicit reminders and critical alerts.
    /// No suggestions, no pattern observations, no social nudges.
    Minimal,

    /// AURA never initiates. Purely reactive. Responds only when spoken to.
    /// Equivalent to current `Declined` consent.
    Never,
}

impl Default for ProactiveMode {
    fn default() -> Self {
        ProactiveMode::Smart // Sane default after consent granted
    }
}
```

### Enhanced ProactiveSettings

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactiveSettings {
    /// Current proactive mode
    pub mode: ProactiveMode,

    /// Original consent state (for tracking first-run flow)
    pub consent: ProactiveConsent,

    /// Quiet hours: no proactive messages except Critical
    pub quiet_hours_start: Option<u8>,  // Hour (0-23)
    pub quiet_hours_end: Option<u8>,    // Hour (0-23)

    /// Max proactive messages per hour (user override)
    /// None = use trust-based defaults
    pub max_per_hour: Option<u32>,

    /// Per-category overrides (user can disable specific types)
    pub category_overrides: HashMap<ProactiveCategoryId, bool>,

    /// Channel preference (user can force a specific channel)
    pub preferred_channel: Option<DeliveryChannel>,
}
```

### Mode Behavior Matrix

| Capability | Always | Smart | Minimal | Never |
|-----------|--------|-------|---------|-------|
| User-set reminders | ✅ | ✅ | ✅ | ❌ |
| Medication alerts | ✅ | ✅ | ✅ | ❌ |
| Critical health alerts | ✅ | ✅ | ✅ | ❌ |
| Morning briefing | ✅ | ✅ (if trust ≥ 0.35) | ❌ | ❌ |
| Goal deadline reminders | ✅ | ✅ (if trust ≥ 0.35) | ❌ | ❌ |
| Routine deviation | ✅ | ✅ (if trust ≥ 0.35) | ❌ | ❌ |
| Social gap nudges | ✅ | ✅ (if trust ≥ 0.60) | ❌ | ❌ |
| Suggestions | ✅ | ✅ (if trust ≥ 0.60) | ❌ | ❌ |
| Attention guardian | ✅ | ✅ (if trust ≥ 0.60) | ❌ | ❌ |
| Pattern observations | ✅ | ✅ (if trust ≥ 0.85) | ❌ | ❌ |
| Memory insights | ✅ | ✅ (if trust ≥ 0.85) | ❌ | ❌ |
| Batching | No (all immediate) | Yes | N/A | N/A |
| Budget multiplier | 1.5x regen | 1.0x | 0.5x | N/A |
| Daily limit multiplier | 2x trust default | 1x | 0.3x | 0 |

### First-Run Consent Flow

```
1. First boot → consent = Unasked, mode = Never (purely reactive)
2. After ~5 conversations, AURA asks:
   "I can be proactive — remind you of things, share observations,
    or just stay quiet until you need me. What would you prefer?"
   → Options: Smart / Minimal / Never
   → If Smart/Minimal: consent = AcceptedAll, mode = selected
   → If Never: consent = Declined, mode = Never
3. User can change mode anytime via settings or voice command:
   "AURA, be more proactive" → Smart (if was Minimal/Never)
   "AURA, be less proactive" → Minimal (if was Smart/Always)
   "AURA, stop initiating" → Never
   "AURA, check in more often" → Always (if was Smart)
```

### Per-Category Controls

Users can fine-tune within their mode:

```
"AURA, stop reminding me about social stuff"
→ category_overrides[SocialGap] = false

"AURA, I don't want morning briefings"
→ category_overrides[MorningBriefing] = false

"AURA, turn morning briefings back on"
→ category_overrides[MorningBriefing] = true
```

---

## Deliverable 7: Alpha vs Beta Scope

### Alpha Scope (Ship First)

**Goal:** Reliable, non-annoying proactive behavior with explicit user-set triggers.

| Feature | Implementation | Files |
|---------|---------------|-------|
| **Timer/Reminder delivery** | User sets reminders via voice/text → cron fires → LLM generates reminder text → deliver | `proactive/mod.rs`, `main_loop.rs` |
| **Medication alerts** | Existing `cron_handle_medication` → ProactiveContext IPC → LLM | Already wired |
| **Morning briefing** | Existing `MorningBriefing` sub-engine → enhance with ContextPackage | `proactive/morning.rs` |
| **Goal deadline reminders** | Existing `cron_handle_arc_health_check` + GoalOverdue trigger | Already wired |
| **ProactiveMode** | Replace `ProactiveConsent` enum with `ProactiveMode` (Always/Smart/Minimal/Never) | `proactive_consent.rs` |
| **Trust gating** | Add `trust_allows()` check in `cron_handle_proactive` and `ProactiveDispatcher` | `proactive_dispatcher.rs`, `main_loop.rs` |
| **Channel selector (basic)** | Route proactive messages to correct channel (Telegram vs Voice vs SilentLog) | New: `proactive/channel.rs` |
| **Consent flow** | After ~5 conversations, ask user for proactive preference | `main_loop.rs`, `proactive_consent.rs` |
| **Quiet hours** | Existing quiet hours logic — ensure it gates all non-critical proactive actions | Already exists |
| **Daily limits** | Trust-based daily limits with `rejection_throttle` | `proactive/mod.rs` |

**Alpha acceptance criteria:**
- User can set reminders and receive them reliably
- Morning briefing works when enabled
- Goal deadline warnings fire at appropriate times
- User can switch between Smart/Minimal/Never modes via voice command
- Critical alerts (medication) always get through regardless of mode
- No proactive messages before consent is given
- Quiet hours respected
- No more than trust-appropriate daily limit

### Beta Scope (Second Release)

**Goal:** Intelligent, pattern-aware proactivity that feels like AURA "understands" the user.

| Feature | Implementation | Files |
|---------|---------------|-------|
| **Notification monitoring** | Android `NotificationListenerService` → JNI bridge → daemon processes notification metadata (not content) | New: `android/notification_bridge.rs` |
| **Pattern detector** | Analyze user behavior over time (app usage patterns, routine timing, productivity cycles) using Hebbian learning | New: `arc/proactive/patterns.rs` |
| **Batch queue** | Low-priority triggers batched into digest messages (30-min window or 3+ items) | New: `proactive/batch.rs` |
| **Social gap nudges** | Existing trigger, but Beta adds LLM-generated thoughtful suggestions ("Mom's birthday is next week, maybe call her?") | `proactive_dispatcher.rs` |
| **Attention guardian** | Existing `ForestGuardian`, but Beta adds screen-time pattern analysis and gentle intervention timing | `proactive/attention.rs` |
| **Contextual suggestions** | "I noticed you usually go for a run at 7am but haven't today — want me to set a reminder for 8am?" | `proactive/suggestions.rs` |
| **Memory insights** | "You mentioned wanting to read more — you haven't opened your reading app in 2 weeks" | New trigger type in `proactive_dispatcher.rs` |
| **Full channel matrix** | Android notification channel, driving detection, AURA-foreground detection | `proactive/channel.rs` |
| **Per-category controls** | User can enable/disable specific proactive categories | `proactive_consent.rs` |
| **Digest delivery** | Batch queue flush → LLM generates combined natural-language digest | `proactive/batch.rs` |

**Beta acceptance criteria:**
- All Alpha criteria still met
- Pattern detector identifies ≥3 user patterns within 2 weeks of usage
- Notification bridge captures metadata without reading content (privacy)
- Batch queue correctly accumulates and flushes low-priority items
- Social nudges feel natural (not robotic reminders)
- Attention guardian intervenes at contextually appropriate moments
- Memory insights are accurate and non-intrusive
- User can disable any category individually
- Digest messages are coherent multi-topic summaries

### Future (Post-Beta)

- **Proactive task execution**: AURA takes actions on behalf of user (with confirmation at Soulmate trust)
- **Cross-device coordination**: Proactive messages routed to most appropriate device
- **Predictive scheduling**: AURA suggests optimal times for tasks based on energy/productivity patterns
- **Emotional awareness**: Proactive check-ins based on detected mood changes (VAD system)
- **Learning optimization**: Proactive study/practice reminders based on spaced repetition principles

---

## Implementation Priority Order

### Phase 1: Alpha Foundation (Estimated: 2-3 days)

1. **Refactor `ProactiveConsent` → `ProactiveMode`** — Replace enum, add mode to settings, wire into `cron_handle_proactive`
2. **Add `trust_allows()` gate** — Insert trust check into proactive dispatch pipeline
3. **Add `daily_proactive_limit()`** — Daily counter with trust-based limits
4. **Build basic `ChannelSelector`** — Route to Voice/Telegram/SilentLog based on device state
5. **Wire consent flow** — After 5 conversations, present mode options to user
6. **Tests** — Unit tests for all gates, integration test for full pipeline

### Phase 2: Alpha Polish (Estimated: 1-2 days)

7. **Enhanced morning briefing** — Full ContextPackage in ProactiveContext
8. **Rejection throttle** — Wire threat_score into daily limit calculation
9. **Voice commands for mode switching** — "AURA, be more/less proactive"
10. **End-to-end test** — Full proactive flow: signal → gate → IPC → LLM → channel → delivery

### Phase 3: Beta Foundation (Estimated: 3-5 days)

11. **Batch queue** — BatchQueue struct, flush timer, digest trigger type
12. **Pattern detector scaffold** — PatternDetector struct, basic app-usage pattern recognition
13. **Notification bridge** — Android JNI bridge for NotificationListenerService metadata
14. **Per-category controls** — `category_overrides` in settings, voice commands
15. **Full channel matrix** — Notification channel, driving detection, foreground detection

### Phase 4: Beta Intelligence (Estimated: 3-5 days)

16. **Contextual suggestions** — Pattern-based suggestion generation
17. **Memory insights** — Cross-reference memories with recent behavior
18. **Social gap enhancement** — LLM-generated thoughtful nudges
19. **Attention guardian enhancement** — Screen-time pattern analysis
20. **Digest generation** — LLM produces natural multi-topic summaries

---

## Key Files to Create/Modify

### Create
- `crates/aura-daemon/src/arc/proactive/channel.rs` — ChannelSelector
- `crates/aura-daemon/src/arc/proactive/batch.rs` — BatchQueue (Beta)
- `crates/aura-daemon/src/arc/proactive/patterns.rs` — PatternDetector (Beta)
- `crates/aura-daemon/src/android/notification_bridge.rs` — NotificationListenerService JNI bridge (Beta)

### Modify
- `crates/aura-daemon/src/identity/proactive_consent.rs` — ProactiveMode enum, enhanced settings
- `crates/aura-daemon/src/arc/proactive/mod.rs` — Trust gating, daily limits, channel integration
- `crates/aura-daemon/src/daemon_core/proactive_dispatcher.rs` — Trust gate in dispatch, new trigger types
- `crates/aura-daemon/src/daemon_core/main_loop.rs` — Wire ProactiveMode, consent flow, channel selector
- `crates/aura-types/src/ipc.rs` — New trigger types (Digest, PatternObservation, etc.)
- `crates/aura-daemon/src/arc/proactive/morning.rs` — Enhanced ContextPackage
- `crates/aura-daemon/src/arc/proactive/suggestions.rs` — Pattern-based suggestions (Beta)
- `crates/aura-daemon/src/arc/proactive/attention.rs` — Enhanced ForestGuardian (Beta)
