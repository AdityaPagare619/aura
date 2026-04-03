# AURA COMPREHENSIVE ARCHITECTURE DESIGN DOCUMENT
## Version 1.0 - March 30, 2026

> **⚠️ DEPRECATED DOCUMENTATION (March 30, 2026):** This document was written when AURA was assumed to be APK-based. **AURA v4 is Termux-based**, not APK-based. Many sections of this document are now outdated.
> 
> **Current deployment information:** See `docs/TERMUX-DEPLOYMENT.md` and `docs/architecture/AURA-V4-INSTALLATION-AND-DEPLOYMENT.md` for the current Termux-native deployment model.

---

# EXECUTIVE SUMMARY

This document provides a complete architectural analysis and redesign of AURA (Autonomous Responsive Agent), covering every aspect from installation to runtime operations. The analysis identifies critical gaps between current implementation and production-ready systems, providing actionable recommendations for each component.

---

# PART 1: CURRENT SYSTEM ANALYSIS

## SECTION 1.1: ARCHITECTURE OVERVIEW

### Current Crate Structure

```
aura-neocortex (Workspace)
├── aura-types          → Type definitions and IPC protocols
├── aura-daemon        → Main service daemon (Android service)
├── aura-neocortex     → LLM inference engine
├── aura-llama-sys     → llama.cpp FFI bindings
└── aura-iron-laws     → Safety/ethics enforcement
```

### Component Analysis

| Component | Purpose | Status | Critical Issues |
|-----------|---------|--------|-----------------|
| aura-types | IPC definitions, data types | Stable | None identified |
| aura-daemon | Background service, Telegram bridge | Partial | Complex, untested flows |
| aura-neocortex | LLM inference orchestration | Partial | Backend selection broken |
| aura-llama-sys | llama.cpp FFI | Partial | SIOF fixed, backend incomplete |
| aura-iron-laws | Safety enforcement | Unknown | Not analyzed |

---

## SECTION 1.2: OPERATIONAL FLOW ANALYSIS

### Current Broken Flow (What Happens Now)

```
USER ACTION                              CURRENT SYSTEM BEHAVIOR
─────────────────────────────────────────────────────────────────
1. Clone repository                      → User must read docs
2. Read 50+ pages of instructions       → Too complex
3. Install Termux                        → User must find/install
4. Run commands manually                 → 20+ manual steps
5. Build Rust binary                     → Requires NDK/setup
6. Deploy via ADB                       → Requires developer tools
7. Configure Telegram                    → Manual token handling
8. Start services                       → Multiple commands
9. Hope it works                        → No validation

TOTAL STEPS: 13+ (COMPLETELY UNACCEPTABLE)
```

### What Should Happen (Target Flow)

```
USER ACTION                              SYSTEM BEHAVIOR
─────────────────────────────────────────────────────────────────
1. Install Termux (F-Droid/GitHub)     → Package manager
2. Clone repo OR download scripts      → Source acquisition
3. Run ./install.sh                     → Full setup
4. Enter Telegram token                 → Single input
5. DONE                                → Everything auto-configures

TOTAL STEPS: 3-5 (ACCEPTABLE)

> **NOTE (March 30, 2026):** AURA v4 is Termux-based. The APK flow above is obsolete. 
> See `docs/TERMUX-DEPLOYMENT.md` for current installation steps.
```

### Runtime Flow Analysis

#### Current Runtime Flow (Conceptual)

```
                    ┌─────────────────────────────────────────┐
                    │           AURA SYSTEM                    │
                    │                                          │
                    │  ┌─────────────┐    ┌──────────────┐   │
                    │  │  TELEGRAM   │───▶│   DAEMON     │   │
                    │  │    BOT      │    │  (Service)   │   │
                    │  └─────────────┘    └───────┬──────┘   │
                    │                              │           │
                    │                              ▼           │
                    │                    ┌──────────────────┐   │
                    │                    │   PIPELINE      │   │
                    │                    │ (Amygdala/      │   │
                    │                    │  Contextor)     │   │
                    │                    └────────┬────────┘   │
                    │                             │              │
                    │                             ▼              │
                    │                    ┌──────────────────┐   │
                    │                    │   NEOCORTEX     │   │
                    │                    │  (LLM Engine)   │   │
                    │                    └────────┬────────┘   │
                    │                             │              │
                    │                             ▼              │
                    │                    ┌──────────────────┐   │
                    │                    │   EXECUTION     │   │
                    │                    │    ENGINE       │   │
                    │                    │(Accessibility)  │   │
                    │                    └──────────────────┘   │
                    │                                          │
                    └─────────────────────────────────────────┘
```

---

## SECTION 1.3: CRITICAL GAPS IDENTIFIED

### Gap Category 1: Installation & Deployment

| Gap | Severity | Current State | Target State | Impact |
|-----|----------|---------------|---------------|--------|
| No APK | CRITICAL | Users must build | One-click install | Blocks all users |
| No auto-install | CRITICAL | 13+ manual steps | 3-5 steps | Blocks adoption |
| No device detection | HIGH | Assumes one device | Auto-detect all | Breaks on others |
| No model management | HIGH | Manual download | Auto-download | Breaks setup |

### Gap Category 2: Runtime Operations

| Gap | Severity | Current State | Target State | Impact |
|-----|----------|---------------|---------------|--------|
| No service mgmt | CRITICAL | Manual start/stop | Auto-manage | Breaks reliability |
| No crash recovery | CRITICAL | Dies on error | Auto-recover | Poor UX |
| No health monitoring | HIGH | Silent failures | Health checks | Can't debug |
| No logging | HIGH | No visibility | Structured logs | Can't operate |

### Gap Category 3: LLM Integration

| Gap | Severity | Current State | Target State | Impact |
|-----|----------|---------------|---------------|--------|
| No working backend | CRITICAL | Stub mode only | Real inference | No AI capability |
| Binary incompatibility | CRITICAL | Prebuilt fails | Works everywhere | Breaks deployment |
| No auto-backend | HIGH | Manual selection | Auto-detect | Breaks setup |

---

# PART 2: COMPONENT-DEEP ANALYSIS

## SECTION 2.1: AURA-DAEMON ANALYSIS

### Purpose
Main background service that runs on Android, managing all system interactions.

### Module Breakdown

#### 2.1.1: Telegram Bridge (`bridge/telegram_bridge.rs`)
**Purpose:** Handles communication with Telegram Bot API

**Current Functionality:**
- Receives messages from users
- Sends responses back
- Manages command handling

**Critical Issues:**
- HTTP backend (reqwest) is hard dependency (not feature-gated)
- Contradicts "anti-cloud" requirement
- No local-only mode

**Required Changes:**
- [ ] Feature-gate reqwest (optional)
- [ ] Add local-only mode (no network)
- [ ] Implement offline command handling

#### 2.1.2: Pipeline System (`pipeline/`)
**Purpose:** Message processing pipeline with attention gating

**Modules:**
- `amygdala.rs` - Event scoring/gating
- `contextor.rs` - Memory enrichment
- `slots.rs` - Context slot management
- `parser.rs` - DSL parsing
- `entity.rs` - Entity extraction

**Critical Issues:**
- Complex untested code paths
- Memory retrieval may fail silently
- No fallback when pipeline fails

**Required Changes:**
- [ ] Add comprehensive error handling
- [ ] Add pipeline health metrics
- [ ] Add fallback processing

#### 2.1.3: Identity System (`identity/`)
**Purpose:** User personality, relationship tracking, ethical bounds

**Modules:**
- `personality.rs` - OCEAN personality model
- `ethics.rs` - Safety rules
- `anti_sycophancy.rs` - Response authenticity
- `affective.rs` - Mood tracking

**Critical Issues:**
- Ethics rules hardcoded (not configurable)
- No audit trail for decisions
- Personality not persistent

**Required Changes:**
- [ ] Move ethics to config file
- [ ] Add decision audit logging
- [ ] Persist personality state

#### 2.1.4: Goals System (`goals/`)
**Purpose:** User goal tracking and decomposition

**Modules:**
- `tracker.rs` - Goal state tracking
- `scheduler.rs` - Goal scheduling
- `decomposer.rs` - Breaking goals into steps

**Critical Issues:**
- Goals lost on restart
- No persistence
- Complex state management

**Required Changes:**
- [ ] Add SQLite persistence
- [ ] Add goal recovery on restart
- [ ] Add goal state machine

#### 2.1.5: Platform Integration (`platform/`)
**Purpose:** Android system integration

**Modules:**
- `power.rs` - Battery monitoring
- `thermal.rs` - Temperature monitoring
- `notifications.rs` - Android notifications
- `jni_bridge.rs` - Java/Rust bridge

**Critical Issues:**
- JNI bridge poorly documented
- Power monitoring may fail on some devices
- No graceful degradation

**Required Changes:**
- [ ] Add JNI error handling
- [ ] Add power state fallback
- [ ] Add platform detection

#### 2.1.6: Screen Access (`screen/`)
**Purpose:** AccessibilityService integration for screen reading

**Modules:**
- `reader.rs` - Screen content reading
- `tree.rs` - UI element tree
- `actions.rs` - Tap, scroll, etc.
- `selector.rs` - Element selection

**Critical Issues:**
- Requires AccessibilityService permission
- Permission handling not robust
- Screen state caching issues

**Required Changes:**
- [ ] Add permission request flow
- [ ] Add permission denied handling
- [ ] Add screen state validation

---

## SECTION 2.2: AURA-NEOCORTEX ANALYSIS

### Purpose
LLM inference engine with multi-layer teacher architecture.

### Module Breakdown

#### 2.2.1: Inference Engine (`inference.rs`)
**Purpose:** Orchestrates 6-layer teacher structure

**Layers:**
- Layer 0: GBNF grammar constraints
- Layer 1: Chain-of-thought forcing
- Layer 2: Confidence estimation
- Layer 3: Cascade retry
- Layer 4: Cross-model reflection
- Layer 5: Best-of-N voting

**Critical Issues:**
- Complex pipeline with many failure points
- No fallback when LLM fails
- Response times unmeasured

**Required Changes:**
- [ ] Add inference timeout handling
- [ ] Add cascade failure metrics
- [ ] Add fallback responses

#### 2.2.2: Model Management (`model.rs`)
**Purpose:** Model loading, unloading, tier selection

**Critical Issues:**
- Backend selection broken (HTTP not integrated)
- Model loading can hang indefinitely
- No model health checking

**Required Changes:**
- [ ] Fix HTTP backend integration
- [ ] Add model load timeout
- [ ] Add model health checks

#### 2.2.3: Grammar System (`grammar.rs`)
**Purpose:** GBNF grammar-constrained generation

**Critical Issues:**
- Complex, untested
- Grammar compilation failures not handled

**Required Changes:**
- [ ] Add grammar error handling
- [ ] Add grammar compilation tests
- [ ] Add fallback to unconstrained

---

## SECTION 2.3: AURA-LLAMA-SYS ANALYSIS

### Purpose
FFI bindings to llama.cpp with multiple backend support.

### Backend Architecture

```
┌─────────────────────────────────────────────────────┐
│              aura-llama-sys                         │
├─────────────────────────────────────────────────────┤
│  trait LlamaBackend                                │
│  ├── is_stub() → bool                             │
│  ├── load_model() → pointers                      │
│  ├── tokenize() → Vec<Token>                      │
│  ├── sample_next() → Token                        │
│  └── ...                                          │
├─────────────────────────────────────────────────────┤
│  Implementations:                                  │
│  ├── StubBackend    → Dummy responses             │
│  ├── FfiBackend    → Native llama.cpp            │
│  └── ServerHttpBackend → HTTP to llama-server    │
└─────────────────────────────────────────────────────┘
```

### Critical Issues

| Issue | Severity | Status |
|-------|----------|--------|
| OnceLock SIOF | CRITICAL | FIXED (LazyLock) |
| FFI build broken | CRITICAL | NOT FIXED (NDK issues) |
| HTTP backend | HIGH | IMPLEMENTED but not tested |
| Backend selection | HIGH | Code exists, not integrated |

### Required Changes

1. [ ] Complete HTTP backend integration testing
2. [ ] Fix FFI build system (CMake + proper flags)
3. [ ] Add backend health checking
4. [ ] Add automatic backend selection

---

# PART 3: OPERATIONAL ARCHITECTURE

## SECTION 3.1: INSTALLATION SYSTEM

### Current State: BROKEN

### Target Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                 INSTALLATION LAYER                          │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│   ┌─────────────┐    ┌─────────────┐    ┌─────────────┐  │
│   │   APK       │    │   Script    │    │   Source    │  │
│   │  (Primary)  │    │  (Fallback) │    │  (Dev only) │  │
│   └──────┬──────┘    └──────┬──────┘    └──────┬──────┘  │
│          │                  │                  │          │
│          ▼                  ▼                  ▼          │
│   ┌─────────────────────────────────────────────────────┐ │
│   │              DETECTION LAYER                        │ │
│   │  • Android version detection                         │ │
│   │  • Architecture detection (arm64/armv7/x86_64)      │ │
│   │  • Storage space check                               │ │
│   │  • RAM detection                                    │ │
│   │  • Termux presence detection                        │ │
│   │  • Root detection                                   │ │
│   └──────────────────────┬──────────────────────────────┘ │
│                          │                                │
│                          ▼                                │
│   ┌─────────────────────────────────────────────────────┐ │
│   │              RESOLUTION LAYER                        │ │
│   │  • Select appropriate binary                        │ │
│   │  • Select appropriate model size                   │ │
│   │  • Configure backend priority                       │ │
│   │  • Set memory limits                               │ │
│   └──────────────────────┬──────────────────────────────┘ │
│                          │                                │
│                          ▼                                │
│   ┌─────────────────────────────────────────────────────┐ │
│   │              EXECUTION LAYER                        │ │
│   │  • Install Termux (if needed)                       │ │
│   │  • Install llama.cpp (via apt)                      │ │
│   │  • Download model (auto-select)                      │ │
│   │  • Start llama-server                               │ │
│   │  • Install/start AURA daemon                        │ │
│   │  • Configure Telegram bot                           │ │
│   └──────────────────────┬──────────────────────────────┘ │
│                          │                                │
│                          ▼                                │
│   ┌─────────────────────────────────────────────────────┐ │
│   │              VALIDATION LAYER                       │ │
│   │  • Verify llama-server running                     │ │
│   │  • Verify AURA daemon running                      │ │
│   │  • Test HTTP backend connectivity                   │ │
│   │  • Validate Telegram connection                     │ │
│   └─────────────────────────────────────────────────────┘ │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Installation Flow Specification

```
STEP 1: Environment Detection
├── Detect OS (Android only for now)
├── Detect Android version (10-14)
├── Detect architecture (uname -m)
├── Detect available storage (df)
├── Detect available RAM (cat /proc/meminfo)
├── Detect Termux (check /data/data/com.termux)
└── Detect root (check su existence)

STEP 2: Capability Assessment
├── Calculate capability score
├── Select model tier (TinyLlama/Small/Medium/Large)
├── Determine backend priority
└── Set resource limits

STEP 3: Component Installation
├── Option A: Termux Available
│   ├── apt update
│   ├── apt install llama-cpp
│   ├── download model
│   └── start server
│
├── Option B: No Termux (APK install)
│   ├── Install APK (bundled binaries)
│   ├── Extract bundled model (or download)
│   ├── Start internal server
│   └── Register as Android service
│
└── Option C: Manual (Development)
    ├── Build from source
    ├── Deploy binary
    └── Configure manually

STEP 4: Service Configuration
├── Create config file
├── Set Telegram bot token
├── Configure model path
├── Set backend priority
└── Configure logging

STEP 5: Service Startup
├── Start llama-server (or use bundled)
├── Start AURA daemon
├── Register with Android
└── Set up boot receiver

STEP 6: Validation
├── Health check llama-server
├── Health check daemon
├── Test Telegram bot
├── Send test message
└── Report status
```

---

## SECTION 3.2: RUNTIME MANAGEMENT

### Service Lifecycle

```
┌─────────────────────────────────────────────────────────────┐
│                 SERVICE LIFECYCLE                            │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  STARTUP                                                    │
│  ├── Boot receiver triggers                                │
│  ├── Load configuration                                    │
│  ├── Detect available backends                            │
│  ├── Initialize backend (HTTP → FFI → Stub)               │
│  ├── Start llama-server (if not running)                  │
│  ├── Start daemon service                                 │
│  ├── Register accessibility service                        │
│  └── Report READY                                         │
│                                                             │
│  RUNNING                                                    │
│  ├── Process Telegram messages                            │
│  ├── Execute actions via AccessibilityService              │
│  ├── Monitor health (periodic checks)                      │
│  ├── Log events                                           │
│  ├── Check memory usage                                   │
│  ├── Check battery level                                   │
│  └── Update personality/mood                              │
│                                                             │
│  ERROR RECOVERY                                            │
│  ├── Detect failure (health check failed)                 │
│  ├── Log error details                                    │
│  ├── Attempt restart failed component                    │
│  ├── Fall back to next backend                            │
│  ├── If all fail → stub mode + alert user                │
│  └── Continue with degraded service                       │
│                                                             │
│  SHUTDOWN                                                  │
│  ├── Save state to SQLite                                 │
│  ├── Checkpoint memory                                    │
│  ├── Stop daemon gracefully                               │
│  ├── Stop llama-server (if we started it)                │
│  └── Unregister services                                  │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Health Monitoring

```
┌─────────────────────────────────────────────────────────────┐
│                 HEALTH MONITORING                          │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  METRICS COLLECTED:                                       │
│  ├── Process health (is process running?)                  │
│  ├── Backend health (can we reach llama-server?)           │
│  ├── Memory usage (RSS, heap)                             │
│  ├── Inference latency (time per token)                   │
│  ├── Error rate (errors per minute)                       │
│  ├── Message throughput (messages per minute)              │
│  ├── Queue depth (pending messages)                       │
│  └── Battery level (for power management)                 │
│                                                             │
│  CHECK FREQUENCY:                                          │
│  ├── Health ping: Every 30 seconds                        │
│  ├── Metrics: Every 60 seconds                            │
│  ├── Deep check: Every 5 minutes                          │
│                                                             │
│  ACTIONS ON FAILURE:                                       │
│  ├── 1 failure → Log warning                             │
│  ├── 3 failures → Attempt restart                         │
│  ├── 5 failures → Fall back to backup                     │
│  └── 10 failures → Alert user + degraded mode            │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

---

## SECTION 3.3: DATA MANAGEMENT

### Storage Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                 DATA STORAGE LAYER                        │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌─────────────────┐  ┌─────────────────┐               │
│  │   TRANSIENT     │  │   PERSISTENT    │               │
│  │   (In-Memory)  │  │   (SQLite)      │               │
│  ├─────────────────┤  ├─────────────────┤               │
│  │ • Message queue │  │ • Conversation  │               │
│  │ • Current       │  │   history      │               │
│  │   context       │  │ • User profile │               │
│  │ • Working       │  │ • Personality  │               │
│  │   memory        │  │ • Goals        │               │
│  │ • Runtime       │  │ • Memory       │               │
│  │   state        │  │   (episodic/    │               │
│  └────────┬────────┘  │   semantic)     │               │
│           │          │ • Settings     │               │
│           │          │ • Audit logs   │               │
│           │          └────────┬────────┘               │
│           │                   │                        │
│           ▼                   ▼                        │
│  ┌─────────────────────────────────────────────────────┐ │
│  │              DATA ACCESS LAYER                       │ │
│  │  • SQLite connection pooling                        │ │
│  │  • Transaction management                           │ │
│  │  • Migration system                                 │ │
│  │  • Backup/restore                                   │ │
│  └─────────────────────────────────────────────────────┘ │
│                                                             │
│  STORAGE LOCATIONS:                                        │
│  ├── Config: /data/data/com.aura/config.toml              │
│  ├── Database: /data/data/com.aura/aura.db               │
│  ├── Models: /data/data/com.aura/models/                 │
│  ├── Logs: /data/data/com.aura/logs/                    │
│  └── Cache: /data/data/com.aura/cache/                  │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Memory Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                 MEMORY ARCHITECTURE                        │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  EPISODIC MEMORY (What Happened)                          │
│  ├── Storage: SQLite                                        │
│  ├── Schema: event_id, timestamp, content, importance    │
│  ├── Retention: Configurable (default: 30 days)            │
│  ├── Index: By timestamp, importance                      │
│  └── Query: Time-range, similarity search                  │
│                                                             │
│  SEMANTIC MEMORY (What We Know)                           │
│  ├── Storage: SQLite                                       │
│  ├── Schema: fact_id, content, confidence, source         │
│  ├── Retention: Permanent (with updates)                  │
│  └── Query: Exact match, semantic search                   │
│                                                             │
│  WORKING MEMORY (Current Context)                          │
│  ├── Storage: In-memory                                    │
│  ├── Size: Token limit (configurable)                     │
│  ├── Content: Last N messages + retrieved context       │
│  └── Lifetime: Per-message, not persistent                 │
│                                                             │
│  IDENTITY/PERSONALITY                                      │
│  ├── Storage: SQLite                                       │
│  ├── Content: OCEAN traits, mood, relationship level    │
│  ├── Persistence: Survives restarts                       │
│  └── Updates: Gradual, with hysteresis                   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

---

# PART 4: USER JOURNEY ANALYSIS

## SECTION 4.1: PERSONA ANALYSIS

### Persona 1: Tech-Savvy Developer

**Profile:**
- Has Android development experience
- Can build from source
- Willing to run commands
- Expected: Full control, customization

**Current Journey:**
- Clone repo → 2 hours setup → Success/failure uncertain

**Target Journey:**
- Clone repo → ./build.sh → Done (15 min)

### Persona 2: Regular User (Primary Target)

**Profile:**
- Non-technical
- Just wants it to work
- Willing to install APK
- Expected: One-button install, works out of box

**Current Journey:**
- Impossible - requires CLI, build tools, ADB

**Target Journey:**
- Download APK → Install → Enter token → Works

### Persona 3: Power User

**Profile:**
- Technical enough to use CLI
- Wants customization
- Willing to configure
- Expected: Flexible but not complex

**Target Journey:**
- Install via script → Configure → Done

---

## SECTION 4.2: DEVICE SCENARIOS

### Scenario Matrix

| Scenario | Device | Android | Termux | RAM | Action |
|----------|--------|---------|--------|-----|--------|
| Ideal | Pixel 8 | 14 | Yes | 12GB | Full install |
| Common | Samsung A54 | 13 | Maybe | 6GB | APK or Termux |
| Budget | Moto E13 | 11 | No | 2GB | APK + small model |
| Old | OnePlus 6 | 10 | Yes | 8GB | Termux + small model |
| Custom ROM | Custom | ? | ? | ? | Detect and adapt |

### Device Detection Requirements

```rust
struct DeviceCapabilities {
    // Hardware
    arch: Architecture,        // arm64, armv7, x86_64
    ram_gb: u32,               // Total RAM
    storage_gb: u32,           // Available storage
    
    // Software
    android_version: u32,      // 10-14
    has_termux: bool,          // Termux installed
    is_rooted: bool,           // Root access
    
    // Capabilities
    can_gpu_accel: bool,       // Vulkan support
    inference_tier: Tier,      // Small/Medium/Large
}
```

---

# PART 5: SECURITY ANALYSIS

## SECTION 5.1: THREAT MODEL

### Assets to Protect

| Asset | Sensitivity | Threat |
|-------|-------------|--------|
| Telegram Bot Token | CRITICAL | Stolen → unauthorized access |
| User Conversations | HIGH | Leaked → privacy violation |
| Model Files | MEDIUM | Stolen → IP theft |
| System Access | HIGH | Compromised → phone control |

### Attack Vectors

| Vector | Likelihood | Impact | Mitigation |
|--------|------------|--------|------------|
| Token theft | Medium | Critical | Secure storage, rotation |
| Memory dump | Low | High | Encrypted storage |
| Network interception | Low | Medium | Local-only mode |
| Malicious input | High | Medium | Input sanitization |
| Privilege escalation | Low | Critical | Principle of least privilege |

### Security Controls

```
REQUIRED CONTROLS:
├── Token Storage
│   └── Android Keystore (encrypted)
│
├── Input Validation
│   ├── Length limits
│   ├── Sanitization
│   └── Syntax checking
│
├── Output Filtering
│   ├── No sensitive data in logs
│   ├── No passwords echoed
│   └── Audit trail
│
├── Network Security
│   ├── Localhost only by default
│   ├── Optional encrypted remote
│   └── No cloud callbacks
│
└── Access Control
    ├── Telegram user allowlist (optional)
    ├── Command authorization
    └── Action approval for sensitive
```

---

# PART 6: PERFORMANCE ANALYSIS

## SECTION 6.1: BENCHMARKS

### Target Metrics

| Operation | Target | Acceptable | Unacceptable |
|-----------|--------|------------|--------------|
| Cold start | < 5s | < 15s | > 30s |
| Message receive | < 100ms | < 500ms | > 1s |
| LLM inference | < 3s | < 10s | > 30s |
| Action execution | < 2s | < 5s | > 10s |
| Memory usage | < 500MB | < 1GB | > 2GB |

### Bottleneck Analysis

```
POTENTIAL BOTTLENECKS:
├── LLM Inference
│   ├── Problem: Token generation slow
│   ├── Solution: Use smaller model, GPU acceleration
│   └── Monitoring: Track tokens/second
│
├── Message Processing
│   ├── Problem: Complex pipeline
│   ├── Solution: Parallel processing, caching
│   └── Monitoring: Track queue depth
│
├── Memory
│   ├── Problem: Context accumulation
│   ├── Solution: Aggressive pruning, limits
│   └── Monitoring: Track RSS
│
└── Network
    ├── Problem: Telegram API rate limits
    ├── Solution: Queue, batching
    └── Monitoring: Track 429 errors
```

---

# PART 7: TESTING STRATEGY

## SECTION 7.1: TEST PYRAMID

```
┌─────────────────────────────────────────────────────────────┐
│                    TEST PYRAMID                            │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│                        ▲                                   │
│                       /│\                                  │
│                      / │ \         E2E TESTS               │
│                     /  │  \       (Full user flows)       │
│                    /───┼───\                               │
│                   /    │    \                              │
│                  /     │     \    INTEGRATION TESTS       │
│                 /      │      \   (Component interaction)  │
│                /───────┼───────\                           │
│               /        │        \                         │
│              /         │         \   UNIT TESTS          │
│             /          │          \  (Individual funcs)   │
│            ────────────┴───────────                        │
│                                                             │
│  TARGET COVERAGE:                                           │
│  ├── Unit: 80%                                             │
│  ├── Integration: 60%                                       │
│  └── E2E: Critical paths only                              │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Test Categories

| Category | Purpose | Automation |
|----------|--------|-----------|
| Unit | Test individual functions | CI runs |
| Integration | Test component interaction | CI runs |
| E2E | Test complete flows | Manual + CI |
| Performance | Benchmark operations | CI nightly |
| Security | Vulnerability scanning | CI runs |
| Device | Test on real devices | Manual |

---

# PART 8: DEPLOYMENT ARCHITECTURE

## SECTION 8.1: RELEASE WORKFLOW

```
┌─────────────────────────────────────────────────────────────┐
│                 RELEASE WORKFLOW                           │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌─────────────┐                                           │
│  │   CODE      │                                           │
│  │  (GitHub)   │                                           │
│  └──────┬──────┘                                           │
│         │                                                  │
│         ▼                                                  │
│  ┌─────────────┐    ┌─────────────┐                      │
│  │    BUILD    │───▶│   TEST      │                      │
│  │  (Actions)  │    │  (CI/CD)    │                      │
│  └──────┬──────┘    └──────┬──────┘                      │
│         │                  │                               │
│         ▼                  ▼                              │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐ │
│  │   APK       │    │  ANDROID    │    │   RELEASE   │ │
│  │  (Signed)   │    │   TESTS     │───▶│  (GitHub)   │ │
│  └─────────────┘    └─────────────┘    └─────────────┘ │
│                                                             │
│  BUILD ARTIFACTS:                                          │
│  ├── aura-vX.X.X-arm64.apk                               │
│  ├── aura-vX.X.X-armv7.apk                               │
│  ├── aura-vX.X.X-x86_64.apk                              │
│  ├── aura-daemon (native binary)                          │
│  └── aura-neocortex (native binary)                       │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

---

# PART 9: RISK ANALYSIS

## SECTION 9.1: RISK MATRIX

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Binary incompatible | HIGH | CRITICAL | Test on multiple devices |
| Model download fails | MEDIUM | HIGH | Bundled fallback |
| Telegram API down | LOW | HIGH | Queue + retry |
| Memory exhaustion | MEDIUM | HIGH | Limits + monitoring |
| Accessibility fails | MEDIUM | MEDIUM | Fallback to voice |
| LLM backend fails | HIGH | HIGH | Multi-backend + stub |
| Permission denied | HIGH | HIGH | Clear user instructions |
| Device not supported | MEDIUM | HIGH | Clear requirements |

---

# PART 10: ROADMAP

## SECTION 10.1: IMPLEMENTATION PHASES

### Phase 1: Core Infrastructure (Week 1-2)
- [ ] Fix HTTP backend integration
- [ ] Create installation script
- [ ] Add device detection
- [ ] Set up GitHub Actions

### Phase 2: Reliability (Week 3-4)
- [ ] Add health monitoring
- [ ] Add crash recovery
- [ ] Add comprehensive logging
- [ ] Add error handling

### Phase 3: Testing (Week 5-6)
- [ ] Unit test coverage to 80%
- [ ] Integration tests
- [ ] Device testing matrix
- [ ] Performance benchmarks

### Phase 4: Production Ready (Week 7-8)
- [ ] APK build and signing
- [ ] Release workflow
- [ ] Documentation
- [ ] User onboarding

---

# APPENDICES

## APPENDIX A: GLOSSARY

| Term | Definition |
|------|------------|
| AURA | Autonomous Responsive Agent |
| Neocortex | LLM inference engine |
| Amygdala | Event gating/scoring system |
| Contextor | Memory retrieval system |
| DSL | Domain Specific Language for actions |
| GGUF | llama.cpp model format |

## APPENDIX B: FILE STRUCTURE

```
aura-neocortex/
├── Cargo.toml              # Workspace manifest
├── Makefile               # Build commands
├── .cargo/                # Cargo config
├── .github/               # GitHub Actions
│   └── workflows/
├── crates/
│   ├── aura-types/       # Type definitions
│   ├── aura-daemon/      # Main service
│   ├── aura-neocortex/   # LLM engine
│   ├── aura-llama-sys/   # llama.cpp FFI
│   └── aura-iron-laws/   # Safety rules
├── config/               # Configuration files
├── docs/                 # Documentation
└── tests/                # Integration tests
```

---

**Document Version:** 1.0
**Created:** March 30, 2026
**Status:** INITIAL DRAFT - Requires Review
**Next Step:** Review and prioritize action items
