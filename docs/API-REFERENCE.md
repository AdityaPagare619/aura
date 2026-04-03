# AURA v4 — API Reference

Complete reference for all public interfaces, IPC protocol, JNI bridge, HTTP endpoints, and configuration.

---

## Table of Contents

- [Crate Public Interfaces](#crate-public-interfaces)
- [IPC Protocol](#ipc-protocol)
- [JNI Bridge](#jni-bridge)
- [HTTP Backend](#http-backend)
- [Configuration Reference](#configuration-reference)
- [Environment Variables](#environment-variables)

---

## Crate Public Interfaces

### aura-types

Shared types used across all crates.

#### Core Types

```rust
// From aura-types/src/ipc.rs

/// Authentication envelope wrapping all IPC messages
pub struct AuthenticatedEnvelope<T> {
    pub protocol_version: u32,   // Current: 3
    pub session_token: String,   // 32-byte hex (64 chars), CSPRNG
    pub seq: u64,                // Monotonic sequence number
    pub payload: T,              // The actual message
}

/// IPC protocol constants
pub const MAX_MESSAGE_SIZE: usize = 256 * 1024;     // 256 KB
pub const LENGTH_PREFIX_SIZE: usize = 4;             // 4 bytes
pub const FRAME_HEADER_SIZE: usize = 4;              // Alias
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const PROTOCOL_VERSION: u32 = 3;
```

#### Message Types

```rust
// Daemon → Neocortex
pub enum DaemonToNeocortex {
    Load {
        path: String,
        context_size: u32,
        thread_count: u32,
    },
    Infer {
        context: ContextPackage,
        mode: InferenceMode,
    },
    Unload,
    HealthCheck,
}

// Neocortex → Daemon
pub enum NeocortexToDaemon {
    InferResult {
        text: String,
        tokens_prompt: u32,
        tokens_generated: u32,
        confidence: f32,
    },
    ModelError {
        error: String,
    },
    HealthResponse {
        status: HealthStatus,
    },
}
```

#### Inference Modes

```rust
pub enum InferenceMode {
    Planner,        // Temperature: 0.1
    Strategist,     // Temperature: 0.4
    Composer,       // Temperature: 0.2
    Conversational, // Temperature: 0.7
}
```

---

### aura-daemon

Main cognitive core crate.

#### Key Modules

| Module | Purpose | Public API |
|---|---|---|
| `daemon_core` | Main loop, event processing | `DaemonCore::new()`, `run()` |
| `memory` | 4-tier memory system | `MemorySystem::store()`, `retrieve()`, `consolidate()` |
| `identity` | OCEAN personality, trust | `Identity::update_mood()`, `get_trust_level()` |
| `execution` | Action execution engine | `Executor::execute_plan()` |
| `ipc` | IPC client to neocortex | `IpcClient::send()`, `receive()` |
| `telegram` | Telegram bot integration | `TelegramBot::start()`, `handle_message()` |
| `voice` | STT/TTS pipeline (optional) | `VoiceSystem::listen()`, `speak()` |
| `platform` | Platform abstraction | `path_resolver::data_dir()`, `model_dir()` |
| `arc` | Adaptive reasoning context | `Arc::get_initiative()`, `update_domain()` |
| `health` | Battery/thermal monitoring | `HealthMonitor::get_battery()`, `get_thermal()` |

#### Memory System API

```rust
/// Store a new memory entry
pub fn store(&self, entry: MemoryEntry) -> Result<MemoryId>;

/// Retrieve memories by query
pub fn retrieve(
    &self,
    query: &str,
    limit: usize,
    tier: MemoryTier,
) -> Result<Vec<ScoredMemory>>;

/// Consolidate episodic → semantic
pub fn consolidate(&self) -> Result<ConsolidationStats>;

/// Cross-tier query
pub fn cross_tier_query(
    &self,
    query: &str,
    max_per_tier: usize,
) -> Result<Vec<ScoredMemory>>;
```

#### Identity API

```rust
/// Get current mood state (VAD model)
pub fn get_mood(&self) -> MoodState;

/// Update mood based on interaction
pub fn update_mood(&self, delta: MoodDelta) -> Result<()>;

/// Get trust level for a user
pub fn get_trust_level(&self, user_id: u64) -> TrustTier;

/// Trust tier hierarchy
pub enum TrustTier {
    Stranger,      // 0.0 - 0.15
    Acquaintance,  // 0.15 - 0.35
    Friend,        // 0.35 - 0.60
    CloseFriend,   // 0.60 - 0.85
    Soulmate,      // 0.85 - 1.0
}
```

---

### aura-neocortex

LLM inference engine.

#### Key Modules

| Module | Purpose | Public API |
|---|---|---|
| `inference` | Core inference loop | `InferenceEngine::infer()` |
| `model` | Model loading/management | `ModelManager::load()`, `unload()` |
| `context` | Context window management | `ContextManager::build_context()` |
| `grammar` | GBNF grammar constraints | `GrammarEngine::apply()` |
| `prompts` | Teacher stack assembly | `PromptBuilder::build_system_prompt()` |
| `ipc_handler` | IPC message handler | `IpcHandler::handle_message()` |

#### Inference API

```rust
/// Run inference with context
pub fn infer(
    &self,
    context: &ContextPackage,
    mode: InferenceMode,
) -> Result<InferenceResult>;

/// Result structure
pub struct InferenceResult {
    pub text: String,
    pub tokens_prompt: u32,
    pub tokens_generated: u32,
    pub confidence: f32,
    pub logprobs: Option<Vec<f32>>,
}
```

---

### aura-iron-laws

Immutable ethics layer.

#### API

```rust
/// Create a new ethics gate
pub fn new() -> EthicsGate;

/// Evaluate an action against Iron Laws
pub fn evaluate(&self, action: &Action) -> EthicsResult;

/// Action builder
pub struct Action { ... }
impl Action {
    pub fn new(description: &str) -> Self;
    pub fn learning(self) -> Self;
    pub fn with_consent(self) -> Self;
}

/// Ethics evaluation result
pub enum EthicsResult {
    Permitted,
    Denied(IronLaw),
    RequiresConsent {
        law: IronLaw,
        reason: String,
    },
}
```

---

### aura-llama-sys

FFI bindings to llama.cpp.

#### API

```rust
/// Initialize llama backend
pub fn llama_backend_init();

/// Load a model from path
pub fn llama_load_model(
    path: *const c_char,
    params: *const llama_model_params,
) -> *mut llama_model;

/// Create context for inference
pub fn llama_new_context_with_model(
    model: *mut llama_model,
    params: *const llama_context_params,
) -> *mut llama_context;

/// Tokenize input text
pub fn llama_tokenize(
    model: *mut llama_model,
    text: *const c_char,
    text_len: i32,
    tokens: *mut llama_token,
    n_max_tokens: i32,
    add_bos: bool,
) -> i32;

/// Run inference step
pub fn llama_decode(
    ctx: *mut llama_context,
    batch: llama_batch,
) -> i32;

/// Get logits after inference
pub fn llama_get_logits(
    ctx: *mut llama_context,
) -> *mut f32;

/// Free resources
pub fn llama_free(ctx: *mut llama_context);
pub fn llama_free_model(model: *mut llama_model);
```

---

## IPC Protocol

### Transport Layer

| Platform | Transport | Address |
|---|---|---|
| Android | Abstract Unix domain socket | `@aura_ipc_v4` |
| Desktop | TCP loopback | `127.0.0.1:19400` |

### Wire Format

```
+----------------+------------------------------------------+
| 4 bytes (LE)   | Variable length (up to 256 KB)           |
| Length prefix   | Bincode-serialized payload               |
+----------------+------------------------------------------+
```

### Message Flow

```
Daemon                                    Neocortex
  │                                          │
  │──── Load { path, ctx_size, threads } ───▶│
  │                                          │
  │◀──── ModelError / HealthResponse ────────│
  │                                          │
  │──── Infer { context, mode } ────────────▶│
  │                                          │
  │◀──── InferResult { text, tokens } ──────│
  │                                          │
  │──── HealthCheck ────────────────────────▶│
  │◀──── HealthResponse ────────────────────│
  │                                          │
  │──── Unload ────────────────────────────▶│
  │                                          │
```

### Authentication

1. **Token Generation:** Daemon generates 32-byte CSPRNG token at startup
2. **Token Sharing:** Passed via inherited file descriptor (not CLI/env)
3. **Envelope:** Every message wrapped in `AuthenticatedEnvelope<T>`
4. **Replay Protection:** Monotonic sequence numbers
5. **Invalid Messages:** Silently dropped (no error response)

---

## JNI Bridge

### Core Functions

```kotlin
// From Android app perspective

// Initialize daemon, returns opaque state pointer
external fun nativeInit(): Long

// Start main loop (blocks until shutdown)
external fun nativeRun(statePtr: Long)

// Clean shutdown (frees state)
external fun nativeShutdown(statePtr: Long)
```

### Android Intents

The JNI bridge can invoke these Android intents:

| Intent | Purpose | Security |
|---|---|---|
| `ACTION_VIEW` | Open URL | Whitelist: `https://`, `http://` only |
| Notification | Show notification | Rate limited |
| Sensor access | Battery, thermal, screen | Read-only |
| Accessibility | UI automation | Requires user grant |

### Thread Safety Warning

> **Important:** The JNI bridge uses raw pointers. Calling `nativeRun()` twice on the same pointer or calling `nativeShutdown()` while `nativeRun()` is executing can cause use-after-free. The enterprise audit recommends using an `AtomicBool` sentinel to track pointer ownership.

---

## HTTP Backend

### Telegram Bot API

AURA uses the Telegram Bot API for user interaction:

| Endpoint | Method | Purpose |
|---|---|---|
| `getUpdates` | Poll | Receive messages (long-polling) |
| `sendMessage` | POST | Send text responses |
| `sendPhoto` | POST | Send images |
| `sendDocument` | POST | Send files |
| `setWebhook` | POST | Configure webhook (alternative) |

### Backend Selection

| Feature | Backend | TLS | Platform |
|---|---|---|---|
| `curl-backend` | System libcurl | OpenSSL | Termux/Android |
| `reqwest` | reqwest crate | rustls | CI/Linux |

> **Mutually exclusive:** Build fails if both or neither feature is selected.

---

## Configuration Reference

### File Location

```
~/.config/aura/config.toml
```

### Load Order

1. Compiled-in Rust defaults
2. `~/.config/aura/config.toml`
3. `AURA_*` environment variables (highest priority)

### Configuration Sections

#### [daemon]

```toml
[daemon]
data_dir = "/data/local/tmp/aura"       # Root data directory
log_level = "info"                       # trace|debug|info|warn|error
checkpoint_interval_s = 300              # State save interval
rss_warning_mb = 28                      # Memory warning threshold
rss_ceiling_mb = 30                      # Memory hard limit
version = "4.0.0"                        # Config version
```

#### [neocortex]

```toml
[neocortex]
model_dir = "/data/local/tmp/aura/models"
default_model_name = "Qwen3-8B-Q4_K_M"
default_model_path = "models/test.gguf"
default_model_context_size = 32768
default_n_ctx = 4096
n_threads = 4
max_memory_mb = 2048
inference_timeout_ms = 60000
```

#### [llama]

```toml
[llama.model]
n_gpu_layers = 0
use_mmap = true
use_mlock = false

[llama.context]
n_ctx = 4096
n_batch = 512
n_threads = 4
seed = 41146

[llama.sampling]
temperature = 0.6
top_p = 0.9
top_k = 40
repeat_penalty = 1.1
max_tokens = 512
```

#### [telegram]

```toml
[telegram]
bot_token = "YOUR_TOKEN"                # Or use AURA_TELEGRAM_TOKEN env var
allowed_chat_ids = [8407946567]         # Whitelist of allowed users
trust_level = 0.5
poll_interval_ms = 2000
```

#### [identity]

```toml
[identity]
user_name = "User"
assistant_name = "AURA"
mood_cooldown_ms = 60000
max_mood_delta = 0.2
trust_hysteresis = 0.05

[identity.ocean]
openness = 0.85
conscientiousness = 0.75
extraversion = 0.50
agreeableness = 0.70
neuroticism = 0.25

[identity.mood_neutral]
valence = 0.0
arousal = 0.0
dominance = 0.5

[identity.relationship_thresholds]
stranger_max = 0.15
acquaintance_max = 0.35
friend_max = 0.60
close_friend_max = 0.85
```

#### [sqlite]

```toml
[sqlite]
db_path = "/data/local/tmp/aura/db/aura.db"
wal_size_limit = 4194304                # 4 MB WAL limit
max_episodes = 10000
max_semantic_entries = 5000
```

#### [vault]

```toml
[vault]
pin_hash = "sha256:salt:hash"           # Set during install
auto_lock_seconds = 0                    # 0 = manual lock only
```

#### [amygdala]

```toml
[amygdala]
instant_threshold = 0.65
weight_lex = 0.40
weight_src = 0.25
weight_time = 0.20
weight_anom = 0.15
storm_dedup_size = 50
storm_rate_limit_ms = 30000
cold_start_events = 200
cold_start_hours = 72
```

#### [execution]

```toml
[execution]
max_steps_normal = 200
max_steps_safety = 50
max_steps_power = 500
rate_limit_actions_per_min = 60
delay_min_ms = 150
delay_max_ms = 500
```

#### [power]

```toml
[power]
daily_token_budget = 50000
conservative_threshold = 50
low_power_threshold = 30
critical_threshold = 15
emergency_threshold = 5
```

#### [power_tiers]

```toml
[power_tiers.charging]
max_inference_calls_per_hour = 120
model_tier = "Full8B"
background_scan_interval_s = 30
proactive_enabled = true
max_concurrent_goals = 8

[power_tiers.normal]
max_inference_calls_per_hour = 60
model_tier = "Standard4B"
background_scan_interval_s = 120
proactive_enabled = true
max_concurrent_goals = 5

[power_tiers.conserve]
max_inference_calls_per_hour = 20
model_tier = "Brainstem1_5B"
background_scan_interval_s = 600
proactive_enabled = false
max_concurrent_goals = 2

[power_tiers.critical]
max_inference_calls_per_hour = 5
model_tier = "Brainstem1_5B"
background_scan_interval_s = 1800
proactive_enabled = false
max_concurrent_goals = 1

[power_tiers.emergency]
max_inference_calls_per_hour = 0
model_tier = "Brainstem1_5B"
background_scan_interval_s = 3600
proactive_enabled = false
max_concurrent_goals = 0
```

#### [thermal]

```toml
[thermal]
warm_c = 40.0
hot_c = 45.0
critical_c = 50.0
hysteresis_c = 2.0
min_transition_interval_s = 10
```

#### [retry]

```toml
[retry]
max_retries = 3
base_delay_ms = 200
backoff_factor = 2
max_delay_ms = 10000
jitter_ms = 50
```

#### [features]

```toml
[features]
voice_enabled = false
proactive_triggers_enabled = true
learning_enabled = true
sentiment_analysis_enabled = true
multi_language_enabled = false
debug_mode = false
```

---

## Environment Variables

### Runtime Variables

| Variable | Purpose | Android Default | Desktop Default |
|---|---|---|---|
| `AURA_HOME` | Home directory | Termux `$HOME` | `dirs::home_dir()` |
| `AURA_DATA_DIR` | Root data directory | `/data/local/tmp/aura` | `~/.local/share/aura` |
| `AURA_MODELS_PATH` | Model directory | `/data/local/tmp/aura/models` | `<data_dir>/models` |
| `AURA_DB_PATH` | SQLite database path | `/data/data/com.aura/databases/aura.db` | `<data_dir>/aura.db` |
| `AURA_NEOCORTEX_BIN` | Neocortex binary path | `/data/local/tmp/aura-neocortex` | `aura-neocortex` (PATH) |
| `AURA_TELEGRAM_TOKEN` | Telegram bot token | (from config) | (from config) |
| `AURA_LOG_PATH` | Log directory | `<data_dir>/logs/` | `<data_dir>/logs/` |
| `AURA_CONFIG_PATH` | Config file path | `~/.config/aura/config.toml` | `~/.config/aura/config.toml` |
| `AURA_STARTED` | Guard flag (prevents duplicate) | Unset | Unset |

### Build Variables

| Variable | Purpose | Default |
|---|---|---|
| `AURA_COMPILE_LLAMA` | Compile real llama.cpp | Unset (stub mode) |
| `ANDROID_NDK_HOME` | NDK installation path | Required for cross-compile |
| `ANDROID_NDK_HOST_TAG` | NDK host platform | `linux-x86_64` |

### Installer Variables

| Variable | Purpose | Default |
|---|---|---|
| `AURA_REPO` | Git repository URL | `https://github.com/AdityaPagare619/aura.git` |
| `AURA_ALLOW_UNVERIFIED_ARTIFACTS` | Skip SHA256 verification | `0` (verification required) |
| `HF_TOKEN` | HuggingFace auth token | Unset (anonymous) |

### Quick Reference

```bash
# Minimal runtime setup (Termux)
export AURA_DATA_DIR="/data/local/tmp/aura"
export AURA_MODELS_PATH="$AURA_DATA_DIR/models"
export AURA_DB_PATH="$AURA_DATA_DIR/db/aura.db"
export AURA_NEOCORTEX_BIN="/data/data/com.termux/files/usr/bin/aura-neocortex"
export AURA_CONFIG_PATH="$HOME/.config/aura/config.toml"

# Security-sensitive (prefer env over config)
export AURA_TELEGRAM_TOKEN="123456789:ABCdef..."
```

---

## Error Codes

### Daemon Errors

| Code | Meaning | Resolution |
|---|---|---|
| `E001` | Config parse error | Check config.toml syntax |
| `E002` | Model not found | Verify model path |
| `E003` | IPC connection failed | Check neocortex binary |
| `E004` | Database locked | Stop daemon, check WAL files |
| `E005` | Memory limit exceeded | Reduce n_ctx or use smaller model |
| `E006` | Telegram auth failed | Verify bot token |
| `E007` | Thermal emergency | Let device cool down |
| `E008` | Ethics violation | Action blocked by Iron Laws |

### Neocortex Errors

| Code | Meaning | Resolution |
|---|---|---|
| `N001` | Model load failed | Verify GGUF file integrity |
| `N002` | Context overflow | Reduce context size |
| `N003` | Inference timeout | Increase timeout_ms |
| `N004` | Grammar violation | Check GBNF grammar file |
| `N005` | Token limit exceeded | Reduce max_tokens |

---

## See Also

| Document | Description |
|---|---|
| [ARCHITECTURE.md](../ARCHITECTURE.md) | System architecture overview |
| [DEPLOYMENT-GUIDE.md](../DEPLOYMENT-GUIDE.md) | Installation and deployment |
| [USER-JOURNEY.md](USER-JOURNEY.md) | User experience guide |
| [ENVIRONMENT-VARIABLES.md](../ENVIRONMENT-VARIABLES.md) | Detailed env var reference |
| [config.toml](../config.toml) | Active configuration |
| [aura-config.example.toml](../aura-config.example.toml) | All options with comments |