# AURA User Journey — Design Specification

**Department:** DESIGN (@design)
**Version:** 1.0
**Status:** DRAFT
**Last Updated:** 2026-04-02

---

## 1. Design Philosophy

AURA is "the world's first private, local AGI." Every design decision must reinforce three pillars:

| Pillar | Design Implication |
|--------|-------------------|
| **Private** | No cloud calls, no data exfiltration, privacy indicators always visible |
| **Local** | Offline-first UX, graceful degradation when models aren't loaded |
| **Personal** | Learns and adapts — the user shapes AURA, not the other way around |

### Core UX Principles

1. **Progressive Disclosure** — Show only what's needed; reveal complexity on demand
2. **Dual Modality** — Every interaction works via Telegram text OR voice
3. **Transparent Operation** — User always knows what AURA is doing and why
4. **Graceful Recovery** — Every error state has a clear path forward
5. **Zero Lock-in** — User can export, delete, or abandon at any time

---

## 2. User Personas

### Persona 1: Privacy-Conscious Power User ("Alex")
- Wants full control, detailed configuration
- Uses Telegram commands extensively
- Values transparency and auditability
- Will read `/help` and explore debug commands

### Persona 2: Casual User ("Jordan")
- Wants AURA to "just work"
- Prefers natural language over commands
- Uses voice primarily
- Gets overwhelmed by too many options

### Persona 3: Developer/Tester ("Sam")
- Building on AURA or testing capabilities
- Needs debug tools, performance metrics
- Wants to understand internal state
- Uses `/dump`, `/trace`, `/perf` regularly

---

## 3. Complete User Journey Map

### Phase 0: Discovery → Download

```
User hears about AURA
    ↓
Visits GitHub / website
    ↓
Downloads: APK (Android) or install.sh (Termux/Linux)
    ↓
Sees README: "The world's first private, local AGI"
```

**UX Considerations:**
- Landing page must immediately communicate "private + local + AGI"
- Download page shows device requirements upfront (RAM, Android version)
- No account creation — ever

### Phase 1: Installation (1-5 minutes)

See [INSTALLATION.md](./INSTALLATION.md) for full details.

**UX Touchpoints:**
- Progress indicator during model download
- Clear "what's happening" at each step
- Estimated time remaining for model download
- "Skip and download later" option for models

### Phase 2: First Launch → Onboarding (3-7 minutes)

The 7-phase onboarding engine (`onboarding.rs`) drives this:

```
┌─────────────────────────────────────────────────────────┐
│                    ONBOARDING FLOW                       │
├─────────────────────────────────────────────────────────┤
│                                                         │
│  ┌──────────────┐                                       │
│  │ 1. Welcome   │  "Hey there! I'm AURA..."             │
│  │ Introduction │  Explain: private, local, learns you  │
│  └──────┬───────┘                                       │
│         ↓                                               │
│  ┌──────────────┐                                       │
│  │ 2. Perms     │  Request: mic, accessibility, notif   │
│  │ Setup        │  "You can change these anytime"       │
│  └──────┬───────┘                                       │
│         ↓                                               │
│  ┌──────────────┐                                       │
│  │ 3. About You │  Name, interests (tags)               │
│  │ Introduction │  "Nice to meet you, {name}!"          │
│  └──────┬───────┘                                       │
│         ↓                                               │
│  ┌──────────────┐                                       │
│  │ 4. Telegram  │  Optional: connect Telegram bot       │
│  │ Setup        │  "You can set this up later"          │
│  └──────┬───────┘                                       │
│         ↓                                               │
│  ┌──────────────┐                                       │
│  │ 5. Personality│  7-question OCEAN calibration quiz    │
│  │ Calibration  │  "How chatty should I be?"            │
│  └──────┬───────┘                                       │
│         ↓                                               │
│  ┌──────────────┐                                       │
│  │ 6. First     │  Device calibration + mini tutorial   │
│  │ Actions      │  "Running in {tier} mode"             │
│  └──────┬───────┘                                       │
│         ↓                                               │
│  ┌──────────────┐                                       │
│  │ 7. Done!     │  Summary, first briefing scheduled    │
│  │ Completion   │  "Welcome aboard, {name}!"            │
│  └──────────────┘                                       │
│                                                         │
│  [Skip All] available at any point                      │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

**Phase-by-Phase UX Design:**

#### Phase 1: Introduction
- **UI:** Full-screen card with AURA avatar
- **Copy:** "Hey there! I'm AURA — your personal AI companion. I live right here on your phone, learning how to help you better every day. Everything stays private and on-device."
- **CTA:** "Let's go" button
- **Escape:** "Skip" link (bottom, muted)

#### Phase 2: Permission Setup
- **UI:** System permission dialogs, one at a time
- **Order:** Microphone → Accessibility → Notifications → Storage
- **Copy per permission:**
  - Mic: "I need this to listen when you talk to me"
  - Accessibility: "I need this to help you navigate your phone"
  - Notifications: "I need this to alert you when something matters"
  - Storage: "I need this to remember things between sessions"
- **Key UX:** Each permission has "Why?" expandable explanation
- **Skip:** Each permission individually skippable
- **Recovery:** Settings link if user denies and later wants to grant

#### Phase 3: User Introduction
- **UI:** Conversational form (not a spreadsheet)
- **Fields:**
  - Name (text input, optional — defaults to "friend")
  - Interests (tag chips — tap to select, free-form addition)
- **Copy:** "What should I call you?"
- **Validation:** None required — everything is optional
- **Personality:** AURA responds to input with warmth matching calibration (yet to come)

#### Phase 4: Telegram Setup
- **UI:** QR code or bot link + step-by-step instructions
- **Copy:** "Want to chat with me on Telegram too? I'll set up a private bot just for you."
- **Steps shown:**
  1. Open Telegram
  2. Search for @YourAURABot (or scan QR)
  3. Send /start
  4. Paste the pairing code shown here
- **Skip:** "Not now" — can set up later via `/config telegram`

#### Phase 5: Personality Calibration
- **UI:** Slider-based quiz (1-5 Likert scale)
- **7 questions, ~30 seconds total:**
  1. Creative vs. tried-and-true
  2. Proactive reminders vs. user-led
  3. Chatty vs. direct
  4. Challenge vs. supportive
  5. Cautious vs. confident
  6. Suggest new things vs. stick to known
  7. Humor vs. professional
- **UX:** Each question has two labeled poles with a slider
- **Copy:** Friendly, not clinical — "How chatty should I be?"
- **Feedback:** Subtle animation when answer is recorded

#### Phase 6: First Actions
- **UI:** Device calibration runs automatically with progress bar
- **What happens:**
  - Device benchmark (CPU, RAM, model tier detection)
  - Mini tutorial: 3 quick demonstrations
    1. "Try asking me something" → `/ask what can you do?`
    2. "Try voice" → wake word prompt
    3. "Try a command" → `/status`
- **Copy:** "I've checked out your device — running in {tier} mode. Let me show you a few things!"

#### Phase 7: Completion
- **UI:** Summary card with personalized next steps
- **Shows:**
  - "Nice to meet you, {name}!"
  - Configured interests
  - Personality summary (friendly version of OCEAN)
  - Tomorrow's briefing time
  - "Just talk to me anytime — no special commands needed"
- **CTA:** "Start using AURA"

### Phase 3: Daily Use — Interaction Patterns

#### 3A. Telegram Text Interaction

```
User opens Telegram
    ↓
Sees AURA chat (pinned if set up)
    ↓
Types naturally: "What's on my schedule today?"
    ↓
AURA responds (or asks clarifying question)
    ↓
User can follow up: "Remind me about the dentist at 3pm"
    ↓
AURA confirms: "Got it — reminder set for 3:00 PM"
```

**Interaction Modes:**

| Mode | Trigger | Example |
|------|---------|---------|
| **Natural Language** | Any text without `/` | "What's the weather?" |
| **Command** | `/` prefix | `/status`, `/ask question` |
| **Dialogue** | Multi-step flows | `/forget *` → 3-step confirm |
| **Callback** | Inline buttons | Dashboard actions |

**Command Discovery UX:**

```
/help → Categories overview
/help ask → Detailed /ask help
/unknown → "I don't know that command. Did you mean /ask?"
/bare text → Treated as /ask (seamless fallback)
```

#### 3B. Voice Interaction

```
User says: "Hey AURA"
    ↓
┌─────────────────────────────────────────────┐
│  State: WakeWordListening                   │
│  Indicator: Subtle pulse animation (if app) │
│  Privacy: Mic indicator ON                  │
└─────────────────────────────────────────────┘
    ↓ (wake word detected)
┌─────────────────────────────────────────────┐
│  State: ActiveListening                     │
│  Indicator: "Listening..." shown            │
│  Privacy: Mic indicator BLINKING            │
│  Timeout: 15 seconds                        │
└─────────────────────────────────────────────┘
    ↓ (user speaks, VAD detects speech end)
┌─────────────────────────────────────────────┐
│  State: Processing                          │
│  Indicator: "Thinking..." with dots         │
│  Privacy: Mic indicator OFF                 │
└─────────────────────────────────────────────┘
    ↓ (STT complete → LLM processes)
┌─────────────────────────────────────────────┐
│  State: Speaking                            │
│  Indicator: Sound wave animation            │
│  TTS plays response                         │
│  Barge-in: User can interrupt with wake word│
└─────────────────────────────────────────────┘
    ↓ (speaking complete)
Back to WakeWordListening
```

**Voice UX Details:**

| Aspect | Design Decision |
|--------|----------------|
| Wake word | "Hey AURA" — 2 syllables, distinct from "OK Google" / "Hey Siri" |
| Listen timeout | 15s default, configurable |
| Barge-in | Say "Hey AURA" while AURA is speaking to interrupt |
| Feedback | Visual + audio confirmation of wake word detection |
| Error recovery | If STT fails: "I didn't catch that — could you repeat?" |
| Privacy indicator | System mic dot + in-app visual indicator |
| Offline behavior | STT/TTS work offline (on-device models) |

#### 3C. Android App Interaction

```
User opens AURA app
    ↓
┌─────────────────────────────────────────┐
│           AURA HOME SCREEN              │
│                                         │
│  ┌─────────────────────────────────┐    │
│  │  Status: Active ●               │    │
│  │  Voice: Listening for "Hey AURA"│    │
│  │  Model: Zipformer (streaming)   │    │
│  │  Battery: Optimized mode        │    │
│  └─────────────────────────────────┘    │
│                                         │
│  [ Talk to AURA ]  [ Open Telegram ]    │
│                                         │
│  Recent Activity                        │
│  ├─ Morning briefing delivered (8:02am) │
│  ├─ Reminder: Dentist at 3pm           │
│  └─ Memory stored: favorite restaurant  │
│                                         │
│  ┌────┐ ┌────┐ ┌────┐ ┌────┐          │
│  │Talk│ │Chat│ │ Mem│ │More│          │
│  └────┘ └────┘ └────┘ └────┘          │
└─────────────────────────────────────────┘
```

### Phase 4: Ongoing Relationship — Adaptive Behavior

```
Day 1-3: AURA observes patterns
    ↓
Day 3-7: First proactive suggestions
    "You usually call Mom around this time — want me to dial?"
    ↓
Week 2+: Deeply personalized
    "Based on your sleep pattern, you might want an early night.
     Your 9am meeting prep is ready if you need it."
    ↓
Ongoing: AURA's personality evolves
    - OCEAN traits drift based on interaction patterns
    - Trust level increases with successful actions
    - Communication style adapts to user preferences
```

### Phase 5: Error & Recovery Flows

#### 5A. Model Not Loaded
```
User asks a question
    ↓
LLM model not available
    ↓
AURA: "I'm still getting set up — my thinking model isn't loaded yet.
       This usually takes about 30 seconds. Want me to check the status?"
    ↓
/status → Shows model loading progress
```

#### 5B. Permission Revoked
```
User revokes mic permission
    ↓
Next voice interaction
    ↓
AURA: "I can't hear you anymore — looks like microphone access was
       removed. You can re-enable it in Settings, or just chat with
       me on Telegram instead."
    ↓
[Open Settings] [Switch to Text]
```

#### 5C. Storage Full
```
Device storage low
    ↓
AURA: "I'm running low on space — I've paused memory consolidation
       to free up some room. Want me to clean up old logs?"
    ↓
[Clean Up] [Review What to Keep] [Ignore]
```

#### 5D. Network Required (for Telegram)
```
Telegram bot can't reach API
    ↓
Queues message locally
    ↓
When network returns: "I had a few messages queued — sending them now."
    ↓
Delivers queued messages with context
```

### Phase 6: Advanced Usage — Power User Flows

#### Multi-Step Workflows
```
User: "/automate morning routine"
    ↓
Dialogue flow starts:
    Step 1/3: "What time should this run?" → "7:30am"
    Step 2/3: "What should I do?" → "Read weather, calendar, news"
    Step 3/3: "Confirm routine?" → "yes"
    ↓
Routine created: "Morning routine scheduled for 7:30am daily"
```

#### Memory Management
```
User: "/forget old meeting notes"
    ↓
AURA searches memories → Shows matches
    ↓
User: "Delete the ones from March"
    ↓
Confirmation: "This will permanently delete 12 memories. Type DELETE to confirm."
    ↓
User: "DELETE"
    ↓
"Done — 12 memories removed"
```

---

## 4. Accessibility Considerations

### Visual
- All UI elements meet WCAG AA contrast ratios
- Screen reader compatible (Android TalkBack)
- Font scaling respects system settings
- Color is never the sole indicator of state

### Motor
- Voice-first alternative for every touch interaction
- Large tap targets (minimum 48dp)
- Gesture alternatives for swipe actions

### Cognitive
- Plain language (no jargon in user-facing text)
- Consistent navigation patterns
- Undo for destructive actions (where possible)
- Progressive complexity — simple first, advanced on demand

### Privacy-Specific
- Privacy indicators are always visible, not hidden in settings
- Data collection is opt-in, not opt-out
- Clear "what AURA can see" explanations at every permission request

---

## 5. Notification & Proactive Behavior UX

### Proactive Suggestion Types

| Type | Example | UX Treatment |
|------|---------|-------------|
| **Briefing** | "Good morning! Today: 2 meetings, dentist at 3" | Full card |
| **Reminder** | "Your dentist appointment is in 30 minutes" | Push notification |
| **Insight** | "You've been coding for 3 hours — take a break?" | Gentle card |
| **Suggestion** | "Based on your interests, you might like..." | Collapsible card |
| **Alert** | "Battery critically low — switched to power-save" | Urgent banner |

### Proactive Behavior Rules
- **Quiet hours:** Respect user's sleep schedule (configurable)
- **Frequency cap:** Max 3 proactive messages per hour
- **Relevance filter:** Only suggest if confidence > 70%
- **Opt-out:** `/quiet` disables all proactive messages
- **Re-enable:** `/wake` re-enables proactive messages

---

## 6. Trust & Privacy UX

### Trust Levels (from `commands.rs`)

| Level | Name | What User Can Do |
|-------|------|-----------------|
| 0.0-0.3 | Stranger | Read-only status, help |
| 0.3-0.6 | Acquaintance | Ask questions, search memories |
| 0.6-0.8 | Trusted | Execute actions, send messages |
| 0.8-1.0 | Intimate | Full access, including config changes |

### Privacy Dashboard (proposed)

```
/privacy — Show privacy dashboard

┌────────────────────────────────────┐
│         PRIVACY STATUS             │
│                                    │
│  Data Location: On-device only     │
│  Cloud Calls: NONE                 │
│  Last External Connection: Never   │
│                                    │
│  Stored Data:                      │
│  ├─ Memories: 147 entries (2.3 MB) │
│  ├─ Profile: 1 entry (0.1 KB)     │
│  └─ Logs: 890 entries (1.2 MB)    │
│                                    │
│  Permissions Active:               │
│  ├─ Microphone: Granted            │
│  ├─ Accessibility: Granted         │
│  └─ Storage: Granted               │
│                                    │
│  [Export My Data] [Delete Everything]│
└────────────────────────────────────┘
```

---

## 7. Onboarding Interruption & Resume

The onboarding engine persists state to SQLite. If the user kills the app mid-onboarding:

```
App restarts
    ↓
Check: onboarding_state table
    ↓
Found: phase = UserIntroduction, completed = false
    ↓
Resume from Phase 3: "Welcome back! We were getting to know you."
    ↓
Continue from where we left off
```

**UX for resume:**
- Brief "welcome back" message (not full re-intro)
- Visual progress indicator shows where they left off
- Option to restart from beginning
- No data loss — all previous answers preserved

---

## 8. Cross-Platform Consistency

| Feature | Telegram | Voice | Android App |
|---------|----------|-------|-------------|
| Ask question | `/ask` or bare text | "Hey AURA, what's..." | Talk button |
| Check status | `/status` | "Hey AURA, how are you?" | Dashboard |
| Set reminder | `/schedule` | "Remind me to..." | Calendar tab |
| View memories | `/memories` | "What do you remember about..." | Memory tab |
| Configure | `/set key value` | "Change my name to..." | Settings |
| Lock/Unlock | `/lock` / `/unlock` | N/A (security) | Biometric |

---

## 9. Metrics for UX Success

| Metric | Target | Measurement |
|--------|--------|-------------|
| Onboarding completion rate | >85% | `completed` flag in SQLite |
| Time to first value | <5 min | Time from install to first useful response |
| Daily active usage | >60% of installs | Telegram message frequency |
| Command discovery | >40% use non-/ask commands | Command distribution |
| Voice adoption | >30% use voice weekly | Wake word detection count |
| Error recovery rate | >90% self-serve | Error → resolution without human help |
| Trust level growth | Reach 0.6+ within 2 weeks | Trust metric tracking |

---

## 10. Future UX Considerations

- **Multi-device:** One AURA across phone + tablet + desktop
- **Shared spaces:** Family/group AURA instances
- **Plugin UX:** How third-party capabilities surface to users
- **AR overlay:** Visual AURA indicators in AR glasses
- **Emotional intelligence:** Biomarker-driven response adaptation

---

---

## 11. Quick Start Guide

### First-Time Setup (5 minutes)

1. **Install Termux** from [F-Droid](https://f-droid.org/en/packages/com.termux/)
2. **Run installer:**
   ```bash
   termux-setup-storage
   curl -fsSL https://raw.githubusercontent.com/AdityaPagare619/aura/main/install.sh -o install.sh
   bash install.sh
   ```
3. **Follow prompts:** Model selection → Telegram bot → Vault PIN
4. **Wait:** 10-30 minutes (mostly unattended)
5. **Chat:** Open Telegram → find your bot → send `/start`

### Daily Usage

**Just talk naturally:**
```
You: "What's on my schedule today?"
AURA: "You have a meeting at 2pm and dentist at 4pm."

You: "Remind me to buy groceries at 6"
AURA: "Got it! Reminder set for 6pm."
```

**No special commands needed.** AURA understands natural language.

### Privacy Guarantee

| What Stays Private | What Connects Out |
|---|---|
| ✅ All conversations | ❌ Nothing (unless you enable Telegram) |
| ✅ All memories | |
| ✅ Your personal data | |
| ✅ Usage patterns | |
| ✅ AI processing | |

**Your data never leaves your phone.** Period.

### Getting Help

- Ask AURA: "How do I use you?"
- Check status: `/status` in Telegram
- View logs: `tail ~/.local/share/aura/logs/current`
- Documentation: [README.md](../README.md)

---

*This document defines the complete user experience from first discovery through daily use. It should be read alongside [DEPLOYMENT-GUIDE.md](../DEPLOYMENT-GUIDE.md) and [API-REFERENCE.md](API-REFERENCE.md).*
