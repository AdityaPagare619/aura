# AURA Telegram Command Interface — UX Specification

**Department:** DESIGN (@design)
**Version:** 1.0
**Status:** DRAFT
**Last Updated:** 2026-04-02

---

## 1. Overview

AURA's Telegram interface is the primary remote control for the AURA daemon. It provides 43 commands across 7 categories, natural language processing, multi-step dialogue flows, and voice message support. The bot lives entirely on the user's device — Telegram is just the transport layer.

### Design Constraints

| Constraint | Implication |
|------------|-------------|
| **No cloud** | Bot runs on user's phone, polling Telegram API directly |
| **Single user** | Bot is private — one admin chat ID, optionally whitelisted family |
| **Offline-capable** | Messages queue locally when network is unavailable |
| **Security-first** | 5-layer security gate before any command executes |
| **Dual input** | Text commands AND voice messages (OGG/Opus) |

---

## 2. Interaction Model

### 2.1 Command vs. Natural Language

AURA supports two input modes that work seamlessly together:

```
┌─────────────────────────────────────────────────────────────┐
│                    INPUT ROUTING                             │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  User types in Telegram                                     │
│         ↓                                                   │
│  ┌─────────────┐                                            │
│  │ Starts with │──YES──→ Parse as command                   │
│  │    "/" ?     │         (43 known commands)                │
│  └──────┬──────┘         Unknown → "Did you mean /ask?"     │
│         │ NO                                                  │
│         ↓                                                   │
│  ┌─────────────┐                                            │
│  │ Has active  │──YES──→ Feed to dialogue FSM               │
│  │  dialogue?  │         (multi-step flow)                  │
│  └──────┬──────┘                                            │
│         │ NO                                                  │
│         ↓                                                   │
│  ┌─────────────┐                                            │
│  │ Route as    │                                            │
│  │ /ask <text> │→ Natural language → LLM reasoning          │
│  └─────────────┘                                            │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

**Key UX decisions:**

1. **Bare text is always `/ask`** — "What's the weather?" works without `/ask`
2. **Commands have aliases** — `/s` = `/status`, `/a` = `/ask`, `/t` = `/think`
3. **Unknown commands get suggestions** — "/statu" → "Did you mean /status?"
4. **Bot suffix is stripped** — `/status@aura_bot` → `/status`

### 2.2 Interaction Patterns

| Pattern | Trigger | Flow | Example |
|---------|---------|------|---------|
| **Quick Command** | `/command` | Single request → single response | `/status` → dashboard |
| **Natural Language** | Free text | Question → LLM answer | "What's my schedule?" → answer |
| **Command with Args** | `/command args` | Parsed args → targeted response | `/ask what time is it` → answer |
| **Multi-Step Dialogue** | Destructive/action command | FSM flow → confirmations | `/forget *` → 3-step confirm |
| **Voice Message** | OGG/Opus audio | STT → route as text → optional TTS reply | Voice → text response |
| **Inline Buttons** | Dashboard messages | Tap button → callback → action | Status dashboard buttons |
| **Photo Response** | Screenshot commands | Command → image + caption | `/screenshot` → screen capture |

---

## 3. Command Reference

### 3.1 System Commands

These provide visibility into AURA's health and status.

| Command | Alias | Args | Permission | Description |
|---------|-------|------|------------|-------------|
| `/status` | `/s` | — | ReadOnly | Full dashboard (CPU, RAM, battery, model, goals, memory) |
| `/health` | `/h` | — | ReadOnly | Quick health check (ok/degraded/down) |
| `/restart` | — | — | Admin | Restart the daemon process |
| `/logs` | `/log` | `[service] [lines]` | Action | Show recent logs (default: 20 lines) |
| `/uptime` | `/up` | — | ReadOnly | Daemon uptime |
| `/version` | `/v` | — | ReadOnly | Build version and git hash |
| `/power` | `/battery`, `/bat` | — | ReadOnly | Battery level and power mode |

**UX Details for `/status`:**

```
┌─────────────────────────────────────────┐
│  AURA System Dashboard                  │
│                                         │
│  Status: ● Online                       │
│  Uptime: 3h 42m                         │
│  Version: v4.2.1 (abc1234)             │
│                                         │
│  CPU: 12% │ RAM: 1.2/3.8 GB │ Bat: 78%│
│  Thermal: Normal │ Network: WiFi        │
│                                         │
│  Model: Zipformer (streaming)           │
│  LLM: Loaded (2.1 GB)                  │
│  TTS: Piper v2.1                       │
│                                         │
│  Active Goals: 2                        │
│  ├─ Morning briefing (daily 8:00)       │
│  └─ Code review reminder (weekly Mon)   │
│                                         │
│  Memory: 147 entries (2.3 MB)           │
│  Telegram: Connected (2 min ago)        │
│  Voice: Idle (wake word active)         │
└─────────────────────────────────────────┘
```

### 3.2 AI Commands

These invoke AURA's reasoning capabilities.

| Command | Alias | Args | Permission | Description |
|---------|-------|------|------------|-------------|
| `/ask` | `/a`, `/?`, bare text | `<question>` | Query | Ask a question (default for bare text) |
| `/think` | `/t` | `<problem>` | Query | Deep reasoning on a problem (slower, more thorough) |
| `/plan` | `/p` | `<goal>` | Action | Generate an actionable plan |
| `/explain` | `/e` | `<topic>` | Query | Explain a topic in depth |
| `/summarize` | `/sum` | `<text>` | Query | Summarize provided text |
| `/translate` | `/tr` | `<text> <lang>` | Query | Translate text to target language |

**UX Differences between `/ask` and `/think`:**

| Aspect | `/ask` | `/think` |
|--------|--------|----------|
| Speed | Fast (streaming) | Slow (full reasoning) |
| Depth | Quick answer | Multi-step analysis |
| Use case | Factual questions | Complex problems |
| Indicators | Typing... | "Thinking deeply..." |
| Token budget | Standard | Higher |

**Natural Language Fallback:**

```
User: "Hey AURA, what's the capital of France?"
Routing: → /ask "Hey AURA, what's the capital of France?"

Note: The "Hey AURA" prefix in text is passed through.
In voice, "Hey AURA" is the wake word and stripped.
```

### 3.3 Memory Commands

AURA's persistent memory system.

| Command | Alias | Args | Permission | Description |
|---------|-------|------|------------|-------------|
| `/remember` | `/rem` | `<text>` | Action | Store a memory |
| `/recall` | `/rec` | `<query>` | Query | Search memories |
| `/forget` | — | `<query>` | Modify | Delete matching memories |
| `/memories` | `/mem` | `[filter]` | Query | List memories |
| `/consolidate` | — | — | Modify | Trigger memory consolidation |
| `/memorystats` | `/memstats` | — | Query | Memory statistics |

**Destructive Action UX — `/forget *`:**

When deleting memories, AURA uses a 3-step confirmation dialogue:

```
User: /forget *
Bot:  Step 1/3: This will permanently delete ALL memories. Are you sure?
      [Type "yes" or "no"]

User: yes
Bot:  Step 2/3: Type DELETE ALL to confirm.

User: DELETE ALL
Bot:  Step 3/3: Final confirmation: type yes to proceed with deletion.

User: yes
Bot:  Done — 147 memories permanently deleted.

User: /cancel (at any step)
Bot:  Dialogue cancelled. No memories were deleted.
```

**Why 3 steps?** Prevents accidental data loss. The "DELETE ALL" exact-phrase match ensures the user understands the severity.

### 3.4 Agency Commands

These let AURA interact with the device and external services.

| Command | Alias | Args | Permission | Description |
|---------|-------|------|------------|-------------|
| `/do` | `/d` | `<instruction>` | Action | Execute an instruction |
| `/open` | `/o` | `<app>` | Action | Open an application |
| `/send` | — | `<app> <contact> <msg>` | Action | Send a message via an app |
| `/call` | — | `<contact>` | Action | Make a phone call |
| `/schedule` | `/sched` | `<event> at <time>` | Action | Schedule an event |
| `/screenshot` | `/ss` | — | Action | Capture the screen |
| `/navigate` | `/nav` | `<destination>` | Action | Open navigation |
| `/automate` | `/auto` | `<routine>` | Action | Run an automation routine |

**Permission gating for agency commands:**

Agency commands require:
1. Chat ID whitelisting ✓
2. PIN unlocked (if PIN set) ✓
3. Permission level: Action or higher ✓
4. Rate limit check ✓
5. Audit log entry ✓

Some agency commands additionally require user approval via PolicyGate:

```
User: /call Mom
Bot:  ⚠️ Approval Required
      AURA wants to: Call "Mom"
      This will use your phone's calling app.

      [Approve] [Deny] (expires in 5 minutes)
```

### 3.5 Configuration Commands

| Command | Alias | Args | Permission | Description |
|---------|-------|------|------------|-------------|
| `/set` | — | `<key> <value>` | Modify | Set a config value |
| `/get` | — | `<key>` | Query | Get a config value |
| `/personality` | — | `[trait value]` | Query/Modify | View/set personality traits |
| `/trust` | — | `[level]` | Query/Modify | View/set trust level |
| `/voice` | — | — | Modify | Enable voice mode |
| `/chat` | — | — | Modify | Enable chat mode |
| `/quiet` | — | — | Query | Disable proactive suggestions |
| `/wake` | — | — | Query | Enable proactive suggestions |

**Personality display UX:**

```
User: /personality
Bot:  🧠 AURA Personality Profile

      Openness: ████████░░ 0.72
      "Creative, curious, open to new ideas"

      Conscientiousness: ██████░░░░ 0.55
      "Moderately organized, balances structure with flexibility"

      Extraversion: ████░░░░░░ 0.38
      "Introverted, prefers deep conversations over small talk"

      Agreeableness: █████████░ 0.85
      "Very cooperative, supportive, trusting"

      Neuroticism: ███░░░░░░░ 0.25
      "Emotionally stable, calm under pressure"

      To change: /personality openness 0.8
```

### 3.6 Security Commands

| Command | Alias | Args | Permission | Description |
|---------|-------|------|------------|-------------|
| `/pin` | — | `<set\|clear\|status>` | Admin | PIN management |
| `/lock` | — | — | Admin | Lock the bot |
| `/unlock` | — | `<pin>` | Admin | Unlock the bot |
| `/audit` | — | `[lines]` | Query | Show audit log |
| `/permissions` | `/perms` | — | Query | Show permission table |

**Lock/Unlock UX:**

```
User: /lock
Bot:  🔒 AURA is now locked.
      All commands will be rejected until you /unlock with your PIN.

User: /status (while locked)
Bot:  🔒 AURA is locked. Use /unlock <pin> to continue.

User: /unlock 1234
Bot:  🔓 AURA unlocked. Welcome back!
```

**PIN security:**
- Stored as Argon2id hash (never plaintext)
- Constant-time verification (prevents timing attacks)
- Failed attempts are rate-limited
- PIN is redacted in audit logs: `/pin set ***`

### 3.7 Debug Commands

| Command | Alias | Args | Permission | Description |
|---------|-------|------|------------|-------------|
| `/trace` | — | `<module>` | Query | Enable tracing for a module |
| `/dump` | — | `<component>` | Query | Dump component state |
| `/perf` | — | — | Query | Performance metrics |
| `/etg` | — | `[app]` | Query | Element tree graph (Android) |
| `/goals` | `/g` | — | Query | Show active goals |

**Debug commands are hidden from `/help` by default** — they appear only if the user is in "developer mode" (detected by usage patterns or explicit opt-in).

### 3.8 Meta Commands

| Command | Alias | Args | Permission | Description |
|---------|-------|------|------------|-------------|
| `/help` | — | `[command]` | ReadOnly | Show help (full or per-command) |

**Help System UX:**

```
User: /help
Bot:  [Full command list organized by category — see commands.rs full_help_text()]

User: /help ask
Bot:  <b>/ask &lt;question&gt;</b>
      Ask AURA a question. Bare text (without /) also works as /ask.
      Aliases: /a /?

User: /help nonexistent
Bot:  No detailed help for '/nonexistent'. Use /help for the full list.
```

---

## 4. Security UX

### 5-Layer Security Pipeline

Every message passes through 5 layers before any handler executes:

```
┌─────────────────────────────────────────────────────────────┐
│                 SECURITY PIPELINE                            │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Incoming message                                           │
│         ↓                                                   │
│  Layer 1: CHAT ID WHITELIST                                 │
│  ┌─────────────────────────────────────────────────────┐    │
│  │ Is chat_id in allowed_chat_ids?                      │    │
│  │ NO → Silent reject (no response to unknown sender)   │    │
│  │ YES → Continue                                       │    │
│  └─────────────────────────────────────────────────────┘    │
│         ↓                                                   │
│  Layer 2: PIN VERIFICATION                                  │
│  ┌─────────────────────────────────────────────────────┐    │
│  │ Is bot locked?                                       │    │
│  │ YES + is /unlock → Verify PIN (Argon2id)            │    │
│  │ YES + not /unlock → "🔒 Locked. Use /unlock <pin>"  │    │
│  │ NO → Continue                                        │    │
│  └─────────────────────────────────────────────────────┘    │
│         ↓                                                   │
│  Layer 3: PERMISSION CHECK                                  │
│  ┌─────────────────────────────────────────────────────┐    │
│  │ Does user have required permission level?            │    │
│  │ ReadOnly < Query < Action < Modify < Admin           │    │
│  │ NO → "Insufficient permissions for /{command}"      │    │
│  │ YES → Continue                                       │    │
│  └─────────────────────────────────────────────────────┘    │
│         ↓                                                   │
│  Layer 4: RATE LIMITING                                     │
│  ┌─────────────────────────────────────────────────────┐    │
│  │ Sliding window: 30/min, 300/hour per chat ID        │    │
│  │ EXCEEDED → "Rate limit exceeded. Try again in Xs"   │    │
│  │ OK → Continue                                        │    │
│  └─────────────────────────────────────────────────────┘    │
│         ↓                                                   │
│  Layer 5: AUDIT LOG                                         │
│  ┌─────────────────────────────────────────────────────┐    │
│  │ Log: timestamp, chat_id, command, outcome            │    │
│  │ Sensitive data redacted (PIN values, etc.)           │    │
│  └─────────────────────────────────────────────────────┘    │
│         ↓                                                   │
│  Command handler executes                                   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Security Error Messages

| Error | Message Shown | UX Treatment |
|-------|---------------|--------------|
| Unknown sender | *(no response)* | Silent — don't confirm bot exists |
| Bot locked | "🔒 AURA is locked. Use /unlock <pin>." | Clear CTA |
| Wrong PIN | "Incorrect PIN. X attempts remaining." | Progressive warning |
| Insufficient permission | "You don't have permission for /{command}." | Suggest upgrade path |
| Rate limited | "Slow down! Try again in {seconds}s." | Friendly, not accusatory |

---

## 5. Voice Message Support

### Voice Input Pipeline

```
User sends voice message (OGG/Opus)
    ↓
Telegram delivers as voice_data
    ↓
voice_pipeline::process_voice_input()
    ├─ Decode OGG/Opus → PCM 16kHz
    ├─ Denoise (RNNoise)
    └─ Return duration + PCM samples
    ↓
Currently: Route description text to command handler
Future: STT transcribes PCM → route transcript
    ↓
Handler processes → Response generated
    ↓
If input was voice: Try TTS response
    ├─ Piper synthesis succeeds → Send voice reply
    └─ TTS fails → Fall back to text reply
```

**UX for voice messages:**

| Scenario | User Experience |
|----------|----------------|
| Send voice, TTS works | User gets voice reply (seamless conversation) |
| Send voice, TTS fails | User gets text reply (graceful degradation) |
| Send voice, STT fails | "I didn't catch that — could you type it or try again?" |
| Voice during TTS | Barge-in: wake word interrupts AURA's speaking |

---

## 6. Offline & Queue UX

### Message Queue

When the device is offline (no network to Telegram API), messages are queued locally:

```
Network down
    ↓
User sends command
    ↓
SQLite queue stores: chat_id, content, priority, TTL, retry_count
    ↓
When network returns:
    ├─ Messages sent in priority order (voice-tagged first)
    ├─ Coalesced: Multiple /status requests → single response
    └─ Expired: Messages older than TTL are discarded
    ↓
"📡 Back online — delivering 3 queued messages"
```

**Queue UX:**
- User doesn't know messages are queued (transparent)
- When reconnecting, batch delivery with header
- Priority: voice responses > normal commands > system notifications
- TTL: 1 hour default (configurable)

---

## 7. Dialogue System UX

### Multi-Step Flows

For commands that need multiple inputs or confirmations, AURA uses an FSM-based dialogue system.

**Supported flows:**

| Flow | Trigger | Steps | UX |
|------|---------|-------|----|
| **ForgetAll** | `/forget *` | 3 (yes/no → DELETE ALL → yes) | Triple confirmation |
| **PinChange** | `/pin set` when PIN exists | 2 (old PIN → new PIN) | Security verification |
| **AutomateSetup** | `/automate` | 3+ (time → actions → confirm) | Guided setup |
| **GenericConfirm** | Destructive actions | 1 (yes/no) | Simple confirmation |

**Dialogue UX Rules:**
1. Each step shows "Step N/M: {prompt}" for progress
2. User can type `/cancel` at any point to abort
3. Invalid input shows helpful hint, not error
4. Dialogues timeout after 2 minutes of inactivity
5. Only one active dialogue per chat
6. Timeout message: "Dialogue timed out."

---

## 8. Response Formatting

### Telegram HTML Support

AURA uses Telegram's HTML parse mode for formatting:

| Element | Usage | Example |
|---------|-------|---------|
| `<b>` | Bold for labels | "<b>Status:</b> Online" |
| `<code>` | Code/values | "<code>CPU: 12%</code>" |
| `<pre>` | Code blocks | Log output |
| `<i>` | Italic for notes | "<i>Tip: use /s for quick status</i>" |

### Response Types

| Type | Handler | Example |
|------|---------|---------|
| Text | `HandlerResponse::Text` | Plain text answer |
| HTML | `HandlerResponse::Html` | Formatted dashboard |
| Photo | `HandlerResponse::Photo { data, caption }` | Screenshot |
| Voice | `HandlerResponse::Voice { text }` | TTS output |

### Message Length

- Telegram limit: 4096 characters
- For long responses: Split into multiple messages (sequential delivery)
- Dashboard: Compact format — essential info first, details on scroll

---

## 9. Command Discovery & Onboarding UX

### First-Time User Flow

```
User sends /start to bot
    ↓
Bot: "Hey {name}! I'm AURA — your personal AI.
      Type /help to see what I can do, or just ask me anything."

User: "What can you do?"
Bot: [Natural language response about capabilities]

User: /help
Bot: [Full command list by category]

User: /help ask
Bot: [Detailed /ask help]
```

### Progressive Command Introduction

Instead of dumping all 43 commands at once, AURA introduces commands contextually:

```
User: "What time is it?"
Bot:  "It's 3:42 PM. 💡 Tip: You can also use /uptime to see how long I've been running."

User: asks about memory
Bot:  "I remember that! 💡 Try /memories to see everything I've stored."

User: asks a complex question
Bot:  [answer] "💡 For deeper analysis, try /think <problem> — I'll take more time to reason through it."
```

### Command Suggestions for Unknown Input

```
User: /statu
Bot:  I don't know /statu. Did you mean /status?

User: /weather
Bot:  I don't have a /weather command. But you can ask me directly:
      "What's the weather?" (just type it without the /)
```

---

## 10. Error Handling UX

### Error Categories & Responses

| Error | User Message | Recovery Action |
|-------|--------------|-----------------|
| Unknown command | "I don't know that command. Did you mean /{suggestion}?" | Suggest closest match |
| Missing args | "/{command} requires: {args}. Example: /{example}" | Show usage |
| Invalid args | "Invalid input for /{command}: {reason}" | Show expected format |
| LLM unavailable | "My thinking model isn't loaded yet. Checking status..." | Auto-retry or suggest /status |
| STT failed | "I couldn't understand that. Could you try again or type it?" | Offer text fallback |
| TTS failed | *(falls back to text response silently)* | Transparent degradation |
| Permission denied | "I can't do that — {permission} is needed." | Suggest settings |
| Network error | "Can't reach Telegram right now. I'll queue your message." | Queue + notify on reconnect |
| Internal error | "Something went wrong internally. The error has been logged." | Log + suggest /debug dump |

### Error UX Principles

1. **Never show stack traces** to users
2. **Always suggest next action** — don't leave user stuck
3. **Log everything internally** — `/audit` for admin review
4. **Be human** — "Something went wrong" not "Error: 0x80004005"
5. **Offer alternatives** — if voice fails, suggest text; if command fails, suggest natural language

---

## 11. Accessibility

### Telegram-Specific Accessibility

| Feature | Implementation |
|---------|---------------|
| **Screen readers** | All responses are text (works with TalkBack/VoiceOver) |
| **Voice input** | Telegram voice messages are natively accessible |
| **Command aliases** | Short aliases (`/s`, `/h`) reduce typing |
| **Bare text mode** | No `/` required — just type naturally |
| **Error clarity** | Errors explain what went wrong AND what to do |
| **Progress indicators** | Step N/M in dialogues |
| **No color-only signals** | Status uses text + emoji, not just color |

---

## 12. Bot Setup Flow (First-Time Telegram Configuration)

### During Onboarding (Phase 4)

```
┌─────────────────────────────────────────────────────────────┐
│              TELEGRAM SETUP FLOW                             │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  AURA App: "Want to chat on Telegram too?"                  │
│                                                             │
│  [Yes, set it up]  [Not now]                                │
│         │                                                   │
│         ↓ (Yes)                                             │
│                                                             │
│  AURA App: "Here's your pairing code: AURA-X7K9-M2P4"      │
│            "1. Open Telegram"                               │
│            "2. Search for your AURA bot"                    │
│            "3. Send: /start AURA-X7K9-M2P4"                │
│                                                             │
│  ┌──────────────────────────────┐                           │
│  │  [QR Code for bot link]      │                           │
│  └──────────────────────────────┘                           │
│                                                             │
│  Telegram: /start AURA-X7K9-M2P4                           │
│  Bot: "✅ Paired successfully! You're connected to AURA     │
│        on your device. Type /help to get started."          │
│                                                             │
│  AURA App: "Telegram is all set! You can chat with me       │
│             there too."                                     │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Manual Setup (Post-Onboarding)

```
User configures in config.toml:
  telegram_bot_token = "123456:ABC-DEF..."
  telegram_allowed_chat_ids = [987654321]

Restart daemon → Bot is live
```

---

## 13. Metrics & Analytics

### Command Usage Analytics (Internal Only)

Track command distribution to improve UX:

| Metric | Purpose |
|--------|---------|
| Command frequency | Which commands are most used |
| Natural language vs. commands | Adoption of bare-text mode |
| Error rate per command | Which commands confuse users |
| Dialogue completion rate | Are multi-step flows too complex? |
| Time to first value | How quickly users get useful output |
| Voice vs. text ratio | Voice adoption |

**Privacy:** All analytics are on-device only. No telemetry sent anywhere.

---

*This specification covers the complete Telegram command interface. For the broader user experience, see [USER-JOURNEY.md](./USER-JOURNEY.md). For installation, see [INSTALLATION.md](./INSTALLATION.md).*
