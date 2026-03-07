# AURA v4 Telegram Redesign & Proactive Behavior Fix - Comprehensive Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.
> **CRITICAL**: Use ALL loaded superpowers: algorithm-design-implementation, polymath-expert-synthesis, system-architecture-patterns, complex-problem-decomposition, strategic-system-fit-analysis, system-design-philosophy, multi-expert-coordination-framework, system-temporal-evolution, precise-system-modeling, scientific-rigor-methodology

**Goal:** Transform AURA's Telegram integration from stub implementation to production-ready control interface, while fixing proactive behavior to be contextually intelligent rather than random.

**Architecture:** 
- Telegram as primary control interface (AURA's "remote control")
- Real reqwest HTTP backend for Bot API communication
- Smart proactive engine with user consent and context awareness
- Hybrid communication: chat-to-chat, voice-to-voice, triggered by user intent NOT geolocation

**Tech Stack:** Rust (tokio, reqwest), Telegram Bot API, SQLite for queue, Cron scheduling

---

## Problem Analysis (Using polymath-expert-synthesis + strategic-system-fit-analysis)

### Current State Assessment

| Component | Current State | Issues |
|-----------|--------------|--------|
| Telegram HTTP Backend | StubHttpBackend | Returns network errors - NOT production |
| Telegram Commands | 43 commands | Missing /start, /stop from v3 |
| Control Interface | Partial | No daemon start/stop/restart control |
| User Chat | Via bridge | Works but not first-class |
| Voice Handling | Partial | No smart hybrid mode |
| Proactive Behavior | Time-based | Morning briefings, not context-aware |
| Geolocation | Not implemented | User says NOT wanted |

### Root Cause Analysis (Using first-principles-reasoning)

1. **Telegram Stub Issue**: The HttpBackend trait exists but only StubHttpBackend implemented - no reqwest
2. **Missing Commands**: v3 had /start, /stop - these are NOT in v4 commands.rs
3. **Proactive Yapping**: The proactive engine triggers on time (morning, welcome home) without user consent context
4. **No Smart Hybrid**: Voice vs chat selection is NOT based on user intent analysis

### Strategic Fit (Using strategic-system-fit-analysis)

| Dimension | Current | Target | Gap |
|-----------|---------|--------|-----|
| Market Fit | C+ | A | Telegram as primary control is right for power users |
| Scale Fit | B- | A | Need production HTTP, not stub |
| Timing Fit | C | A | Need to fix before release |
| Competitive | B | A | OpenClaw has full Telegram, AURA needs parity |

---

## Workstream Decomposition (Using complex-problem-decomposition)

### Phase 1: Foundation (Critical Path)
- **Task 1.1**: Implement real reqwest HttpBackend
- **Task 1.2**: Add /start, /stop, /restart commands (from v3)
- **Task 1.3**: Wire TelegramEngine to daemon control (start/stop/restart)

### Phase 2: Intelligence
- **Task 2.1**: User consent system for proactive behavior
- **Task 2.2**: Context-aware proactive trigger (not time-only)
- **Task 2.3**: Smart communication mode selection (chat vs voice)

### Phase 3: Polish
- **Task 3.1**: Remove geolocation-based triggers (if any exist)
- **Task 3.2**: Fix voice/chat hybrid mode
- **Task 3.3**: End-to-end integration testing

---

## Detailed Implementation Tasks

### Task 1.1: Implement Real reqwest HTTP Backend

**Files:**
- Create: `crates/aura-daemon/src/telegram/reqwest_backend.rs`
- Modify: `crates/aura-daemon/src/telegram/mod.rs`
- Modify: `crates/aura-daemon/src/telegram/polling.rs:73-97`

**Step 1: Create failing test for reqwest backend**

```rust
// crates/aura-daemon/src/telegram/reqwest_backend.rs

use aura_types::errors::AuraError;
use async_trait::async_trait;

pub struct ReqwestHttpBackend {
    client: reqwest::Client,
    base_url: String,
}

impl ReqwestHttpBackend {
    pub fn new(bot_token: &str) -> Self {
        let base_url = format!("https://api.telegram.org/bot{}", bot_token);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("reqwest client build");
        Self { client, base_url }
    }
}

#[async_trait]
impl super::polling::HttpBackend for ReqwestHttpBackend {
    async fn get(&self, url: &str) -> Result<Vec<u8>, AuraError> {
        let full_url = if url.starts_with("http") {
            url.to_string()
        } else {
            format!("{}/{}", self.base_url, url.trim_start_matches('/'))
        };
        
        let response = self.client
            .get(&full_url)
            .send()
            .await
            .map_err(|e| AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed))?;
        
        let bytes = response
            .bytes()
            .await
            .map_err(|e| AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed))?;
        
        Ok(bytes.to_vec())
    }

    async fn post_json(&self, url: &str, body: &[u8]) -> Result<Vec<u8>, AuraError> {
        let full_url = if url.starts_with("http") {
            url.to_string()
        } else {
            format!("{}/{}", self.base_url, url.trim_start_matches('/'))
        };
        
        let response = self.client
            .post(&full_url)
            .header("Content-Type", "application/json")
            .body(body.to_vec())
            .send()
            .await
            .map_err(|e| AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed))?;
        
        let bytes = response
            .bytes()
            .await
            .map_err(|e| AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed))?;
        
        Ok(bytes.to_vec())
    }

    async fn post_multipart(
        &self,
        url: &str,
        fields: Vec<(&str, String)>,
        file_field: Option<(&str, Vec<u8>, &str)>,
    ) -> Result<Vec<u8>, AuraError> {
        let full_url = if url.starts_with("http") {
            url.to_string()
        } else {
            format!("{}/{}", self.base_url, url.trim_start_matches('/'))
        };
        
        let mut form = reqwest::multipart::Form::new();
        
        for (key, value) in fields {
            form = form.text(key, value);
        }
        
        if let Some((field_name, file_data, mime_type)) = file_field {
            let part = reqwest::multipart::Part::bytes(file_data)
                .mime_str(mime_type)
                .map_err(|e| AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed))?;
            form = form.part(field_name, part);
        }
        
        let response = self.client
            .post(&full_url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed))?;
        
        let bytes = response
            .bytes()
            .await
            .map_err(|e| AuraError::Ipc(aura_types::errors::IpcError::ConnectionFailed))?;
        
        Ok(bytes.to_vec())
    }
}
```

**Step 2: Run test to verify it fails (need to add reqwest to Cargo.toml)**

Expected: FAIL - "cannot find attribute reqwest"

**Step 3: Add reqwest to Cargo.toml dependencies**

```toml
# In aura-daemon/Cargo.toml, add:
reqwest = { version = "0.12", features = ["multipart"] }
```

**Step 4: Run test again**

Expected: PASS after adding dependency

**Step 5: Commit**

```bash
git add crates/aura-daemon/src/telegram/reqwest_backend.rs
git add crates/aura-daemon/Cargo.toml
git commit -m "feat: implement reqwest HTTP backend for Telegram Bot API"
```

---

### Task 1.2: Add /start, /stop, /restart Commands (from v3)

**Files:**
- Modify: `crates/aura-daemon/src/telegram/commands.rs`
- Modify: `crates/aura-daemon/src/telegram/handlers/mod.rs`

**Step 1: Add Start/Stop/Restart commands to TelegramCommand enum**

```rust
// Add to commands.rs - in the System category (around line 26-43):

// ── Control (3) ──────────────────────────────────────────────────────
/// `/start` — Start AURA daemon (control interface).
Start,
/// `/stop` — Stop AURA daemon (control interface).
Stop,
/// `/reboot` — Restart AURA daemon (control interface).
Reboot,
```

**Step 2: Add permission requirements**

```rust
// In required_permission() method, add:
TelegramCommand::Start => PermissionLevel::Admin,
TelegramCommand::Stop => PermissionLevel::Admin,
TelegramCommand::Reboot => PermissionLevel::Admin,
```

**Step 3: Add parser support**

```rust
// In the parse() method, add case handling:
"start" => TelegramCommand::Start,
"stop" => TelegramCommand::Stop,
"reboot" => TelegramCommand::Reboot,
```

**Step 4: Add handler implementations**

```rust
// In handlers/mod.rs - create control_handler:

pub async fn control_handler(
    cmd: &TelegramCommand,
    ctx: &mut HandlerContext,
) -> Result<HandlerResponse, AuraError> {
    match cmd {
        TelegramCommand::Start => {
            // Check if already running
            // TODO: Wire to daemon startup
            Ok(HandlerResponse::text("AURA is already running."))
        }
        TelegramCommand::Stop => {
            // Signal daemon shutdown
            // TODO: Wire to graceful shutdown
            Ok(HandlerResponse::text("AURA shutting down..."))
        }
        TelegramCommand::Reboot => {
            // Signal daemon restart
            // TODO: Wire to restart
            Ok(HandlerResponse::text("AURA restarting..."))
        }
        _ => Err(AuraError::Internal("Not a control command".into())),
    }
}
```

**Step 5: Run tests**

Run: `cargo test -p aura-daemon telegram::commands`
Expected: PASS

**Step 6: Commit**

```bash
git add crates/aura-daemon/src/telegram/commands.rs
git add crates/aura-daemon/src/telegram/handlers/mod.rs
git commit -m "feat: add /start, /stop, /reboot control commands"
```

---

### Task 1.3: Wire Telegram Control to Daemon

**Files:**
- Modify: `crates/aura-daemon/src/telegram/mod.rs`
- Modify: `crates/aura-daemon/src/daemon_core/shutdown.rs`

**Step 1: Add control callback to TelegramEngine**

```rust
// In TelegramEngine struct, add:
pub struct TelegramEngine {
    // ... existing fields
    /// Callback for daemon control (start/stop/reboot)
    control_callback: Option<Box<dyn DaemonControl>>,
}

// Add trait:
pub trait DaemonControl: Send + Sync {
    fn stop(&self);
    fn restart(&self);
}
```

**Step 2: Wire stop to graceful shutdown**

```rust
// In shutdown.rs - expose the stop signal
pub fn request_shutdown() {
    // Signal shutdown via the cancel flag
}
```

**Step 3: Test the wiring**

Run: `cargo test -p aura-daemon telegram`
Expected: PASS

**Step 4: Commit**

---

### Task 2.1: User Consent System for Proactive Behavior

**Files:**
- Create: `crates/aura-daemon/src/identity/proactive_consent.rs`
- Modify: `crates/aura-daemon/src/telegram/security.rs`
- Modify: `crates/aura-daemon/src/arc/proactive/mod.rs`

**Step 1: Define consent states**

```rust
// crates/aura-daemon/src/identity/proactive_consent.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProactiveConsent {
    /// User has not been asked
    Unasked,
    /// User explicitly declined
    Declined,
    /// User accepted all proactive suggestions
    AcceptedAll,
    /// User accepted only specific categories
    AcceptedCategories(Vec<ProactiveCategory>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProactiveCategory {
    Reminders,
    Health,
    Social,
    MorningBriefing,
    WelcomeHome,
    Suggestions,
}

impl Default for ProactiveConsent {
    fn default() -> Self {
        Self::Unasked
    }
}

impl ProactiveConsent {
    pub fn can_produce(&self, category: ProactiveCategory) -> bool {
        match self {
            Self::Unasked => false, // Must ask first!
            Self::Declined => false,
            Self::AcceptedAll => true,
            Self::AcceptedCategories(cats) => cats.contains(&category),
        }
    }
}
```

**Step 2: Add consent to user profile**

```rust
// In identity/user_profile.rs, add:
pub struct UserProfile {
    // ... existing fields
    pub proactive_consent: ProactiveConsentConsent,
}
```

**Step 3: Add consent commands to Telegram**

```rust
// Add to commands.rs:
/// `/proactive` — Show/set proactive preferences.
Proactive { action: ProactiveAction },

/// `/quiet` — Disable all proactive suggestions.
Quiet,

/// `/wake` — Re-enable proactive suggestions.
Wake,

// Add enum:
pub enum ProactiveAction {
    Show,
    Enable,
    Disable,
    EnableCategory(String),
}
```

**Step 4: Test**

Run: `cargo test -p aura-daemon proactive`
Expected: PASS

**Step 5: Commit**

---

### Task 2.2: Context-Aware Proactive Trigger

**Files:**
- Modify: `crates/aura-daemon/src/arc/proactive/mod.rs`
- Modify: `crates/aura-daemon/src/arc/proactive/suggestions.rs`

**Step 1: Add context check to proactive tick**

```rust
// In ProactiveEngine::tick(), add context validation:

pub fn tick(&mut self, now_ms: u64, power: PowerTier, context: &Context) -> Vec<ProactiveAction> {
    // FIRST: Check user consent
    if !self.user_consent_check(context) {
        tracing::debug!("proactive disabled - user consent required");
        return vec![];
    }
    
    // SECOND: Check context quality (not just time)
    if !self.has_good_context(context) {
        tracing::debug!("proactive skipped - poor context quality");
        return vec![];
    }
    
    // Then proceed with existing logic...
}

fn user_consent_check(&self, context: &Context) -> bool {
    context.user_profile
        .as_ref()
        .map(|p| p.proactive_consent.can_produce(ProactiveCategory::Suggestions))
        .unwrap_or(false)
}

fn has_good_context(&self, context: &Context) -> bool {
    // Context quality signals:
    // - User is active (not sleeping)
    // - Battery sufficient (>20%)
    // - Not in meeting mode
    // - Has recent interaction history
    // - NOT based on geolocation!
    
    context.battery_level > 20
        && !context.is_quiet_hours
        && context.last_interaction_ms.map(|t| {
            (context.now_ms - t) < 3600_000 // Active within last hour
        }).unwrap_or(false)
}
```

**Step 2: Remove geolocation-based triggers**

Search for any geolocation-based triggers and remove them:

```bash
# Check for geolocation in proactive code
grep -r "geo" crates/aura-daemon/src/arc/proactive/
```

**Step 3: Test**

Run: `cargo test -p aura-daemon arc::proactive`
Expected: PASS

**Step 4: Commit**

---

### Task 2.3: Smart Communication Mode Selection

**Files:**
- Modify: `crates/aura-daemon/src/bridge/telegram_bridge.rs`
- Modify: `crates/aura-daemon/src/telegram/handlers/mod.rs`
- Create: `crates/aura-daemon/src/telegram/voice_handler.rs`

**Step 1: Add mode detection**

```rust
// In telegram_bridge.rs, add intelligent mode selection:

enum CommunicationMode {
    /// User sent text - respond with text
    Text,
    /// User sent voice - respond with voice (if enabled)
    Voice,
    /// User wants chat mode - text only
    ChatMode,
    /// User wants voice mode - speak responses
    VoiceMode,
}

fn detect_communication_mode(update: &TelegramUpdate) -> CommunicationMode {
    // Priority:
    // 1. Explicit mode command (/voice, /chat)
    // 2. If user has voice preference enabled
    // 3. If message is voice note
    // 4. Default to text
    
    if update.text.starts_with("/voice") {
        return CommunicationMode::VoiceMode;
    }
    if update.text.starts_with("/chat") {
        return CommunicationMode::ChatMode;
    }
    if update.voice.is_some() {
        return CommunicationMode::Voice;
    }
    
    // Check user preference from profile
    // Default to text
    CommunicationMode::Text
}
```

**Step 2: Implement voice response handler**

```rust
// Create voice_handler.rs for smart voice/chat selection:

pub struct VoiceHandler {
    tts: TtsEngine,
    voice_preferences: VoicePreferences,
}

impl VoiceHandler {
    pub fn should_speak(&self, context: &UserContext, response: &str) -> bool {
        // Smart conditions:
        // 1. User explicitly in voice mode
        // 2. User sent voice message (conversational)
        // 3. Response is short (< 50 words) AND user prefers voice
        // 4. NOT: long responses (TTS can't handle well)
        // 5. NOT: code/technical content
        
        let user_mode = context.voice_mode;
        let is_short = response.split_whitespace().count() < 50;
        let is_technical = contains_code_or_technical(response);
        
        match user_mode {
            VoiceMode::Always => !is_technical,
            VoiceMode::Smart => is_short && !is_technical && context.last_message_was_voice,
            VoiceMode::Never => false,
        }
    }
}
```

**Step 3: Wire into telegram bridge**

**Step 4: Test**

Run: `cargo test -p aura-daemon telegram`
Expected: PASS

**Step 5: Commit**

---

### Task 3.1: Remove Any Geolocation Triggers

**Step 1: Search and destroy**

```bash
# Find any geolocation references in proactive
grep -r "geo" crates/aura-daemon/src/arc/proactive/
grep -r "location" crates/aura-daemon/src/arc/proactive/
grep -r "gps" crates/aura-daemon/src/arc/proactive/
```

**Step 2: Remove any found references**

**Step 3: Commit**

---

### Task 3.2: Fix Voice/Chat Hybrid Mode

**Step 1: Comprehensive test**

Run: `cargo test -p aura-daemon --test integration_tests`
Expected: PASS

**Step 2: Fix any failures**

**Step 3: Commit**

---

### Task 3.3: End-to-End Integration Testing

**Step 1: Create E2E test**

```rust
#[tokio::test]
async fn test_telegram_control_flow() {
    // 1. Start with stub backend
    // 2. Send /start command
    // 3. Verify response
    // 4. Send /status command  
    // 5. Verify status response
    // 6. Send /stop command
    // 7. Verify shutdown initiated
}
```

**Step 2: Run full test suite**

Run: `cargo test -p aura-daemon`
Expected: ALL PASS (2585+ tests)

**Step 3: Final commit**

---

## Dependencies & Execution Order

```
Task 1.1 (Foundation) ──┬──► Must complete before anything else
Task 1.2               ──┤
Task 1.3               ──┘
                        │
Task 2.1 ───────────────┼──► Requires Phase 1
Task 2.2 ───────────────┤
Task 2.3 ───────────────┘
                        │
Task 3.1 ───────────────┼──► Requires Phase 2
Task 3.2 ───────────────┤
Task 3.3 ───────────────┘
```

---

## Success Criteria

| Criterion | Target | Verification |
|-----------|--------|--------------|
| Telegram HTTP | Real reqwest backend compiles | Integration test with real API |
| Control Commands | /start, /stop, /reboot work | Unit tests |
| Proactive Consent | User must opt-in first | Consent state machine |
| No Geolocation | Zero location-based triggers | Code audit |
| Hybrid Mode | Smart voice/chat selection | Integration tests |
| Test Coverage | 2600+ tests passing | cargo test |

---

## Risk Analysis (Using scientific-rigor-methodology)

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| reqwest fails on Android | Medium | High | Fallback to ureq |
| Proactive breaks existing behavior | Low | Medium | Feature flag default off |
| Voice mode causes loops | Medium | High | Strict conditions |

---

## Open Questions

1. Should /start automatically enable proactive?
2. How to handle multi-user Telegram chats?
3. Voice mode - what TTS engine to use?

---

*Plan created using: polymath-expert-synthesis, strategic-system-fit-analysis, complex-problem-decomposition, algorithm-design-implementation, precise-system-modeling, scientific-rigor-methodology*
