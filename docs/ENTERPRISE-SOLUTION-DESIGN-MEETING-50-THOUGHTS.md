# AURA Enterprise Solution Design - Complete Implementation Plan

## Meeting Overview

**Purpose**: Transform AURA from a "college project that crashes when something fails" to an "enterprise platform that handles variations gracefully"

**Input**: Analysis findings from 50+ deep thoughts and 30+ web research sources

**Output**: Complete solution architecture with 5 core systems, implementation phases, team assignments, and proof requirements

---

## Problem Recap

The AURA daemon crashes on device (SIGSEGV exit 139) when neocortex binary receives any argument. Root cause is BUILD INFRASTRUCTURE failure (F003/F004), not a code bug. The system has:
- No runtime capability detection
- No graceful degradation when binaries fail
- No boot stage logging
- No failure classification
- No device matrix testing
- No validation pipeline

This is why ONE failure crashes EVERYTHING.

---

## Solution Architecture - 5 Core Systems

### System 1: Runtime Capability Detector

**Purpose**: Run FIRST at daemon startup, identify what's available on the device

**Algorithm**:
1. Scan /data/local/tmp/ for known binaries (neocortex, llama-server)
2. Execute each with --version (timeout 5 seconds)
3. Capture stdout, check expected output patterns
4. If execution fails (timeout, signal, non-zero), record as "tested but failed"
5. Query device metrics in parallel:
   - Memory: /proc/meminfo
   - API: getprop ro.build.version.sdk
   - ABI: getprop ro.product.cpu.abi

**Output**: DeviceCapabilities struct
```rust
pub struct DeviceCapabilities {
    pub neocortex_available: bool,
    pub neocortex_tested: bool,
    pub neocortex_failure_class: Option<FailureClass>,
    pub llama_server_available: bool,
    pub llama_server_tested: bool,
    pub llama_server_path: String,
    pub memory_mb: u64,
    pub android_api: u32,
    pub cpu_abi: String,
    pub cpu_cores: u32,
}
```

---

### System 2: Backend Router

**Purpose**: Route inference requests to appropriate backend based on availability

**Priority Logic**:
- If neocortex_works AND llama_server_works → vector = [neocortex, llama_server]
- If ONLY llama_server_works → vector = [llama_server]
- If NEITHER works → vector = []

**Interface**:
```rust
fn route_request(prompt: &str, options: InferenceOptions) -> Result<Response, FailureContext>

pub struct FailureContext {
    pub failure_class: FailureClass,
    pub attempted_backends: Vec<String>,
    pub error_message: String,
    pub timestamp: DateTime<Utc>,
}
```

---

### System 3: Graceful Degradation Engine

**Purpose**: Handle failures without crashing - state machine with four levels

**State Levels**:
| Level | Name | Description |
|-------|------|-------------|
| 1 | FULL | Primary backend works, all features |
| 2 | DEGRADED | Primary fails but secondary works |
| 3 | MINIMAL | All backends fail, daemon runs, offline mode |
| 4 | BROKEN | Daemon fails, but logs WHY |

**State Machine Events**:
- InferenceSuccess(backend)
- InferenceFailure(backend, error)
- BackendBecameUnavailable(backend)
- RecoveryPossible(backend)

**Transitions**:
- FULL → DEGRADED: Primary returns non-recoverable error
- DEGRADED → MINIMAL: All backends exhausted
- MINIMAL → BROKEN: Daemon cannot maintain basic operation

---

### System 4: Observability Layer

**Purpose**: Provide visibility through structured logging, boot stages, failure classification

**Boot Stages** (in order):
1. init - daemon starting
2. environment_check - device capabilities
3. dependency_check - binary verification
4. runtime_start - inference backend init
5. ready - operational

**Log Format**:
```
[TIMESTAMP] [LEVEL] [STAGE/CLASS] [COMPONENT] Message
[2026-03-27T10:30:45Z] [INFO] [init] [daemon] daemon starting version 1.0.0
[2026-03-27T10:30:46Z] [INFO] [environment_check] [capability_detector] detected memory_mb=4096 android_api=35
[2026-03-27T10:30:47Z] [ERROR] [dependency_check] [capability_detector] neocortex test failed failure_class=F003 exit_code=139
```

**Failure Classification (F001-F008)**:
| Code | Name | Description |
|------|------|-------------|
| F001 | Artifact Missing | file not found |
| F002 | Dependency Mismatch | library not found |
| F003 | ABI Mismatch | signal during initialization (OUR CASE) |
| F004 | Linker Failure | dynamic library error |
| F005 | Runtime Crash | panic during operation |
| F006 | Config Drift | parse error |
| F007 | Observability Gap | log failed |
| F008 | Governance Failure | release process violation |

---

### System 5: Health Monitor

**Purpose**: Expose system state via HTTP endpoint

**Endpoint**: GET /health

**Response**:
```json
{
  "version": "1.0.0",
  "uptime_seconds": 3600,
  "status": "degraded",
  "boot_stages": {
    "init": "complete",
    "environment_check": "complete",
    "dependency_check": "complete",
    "runtime_start": "complete",
    "ready": "complete"
  },
  "backends": {
    "neocortex": {
      "available": false,
      "tested": true,
      "failure_class": "F003",
      "last_error": "SIGSEGV 139"
    },
    "llama_server": {
      "available": true,
      "tested": true,
      "path": "/data/local/tmp/llama/llama-server"
    }
  },
  "active_backend": "llama_server",
  "degradation_level": 2,
  "last_inference_ms": 1500,
  "total_requests": 42
}
```

---

## Integration Flow

```
┌─────────────────────────────────────────┐
│           DAEMON START                   │
└─────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────┐
│  OBSERVABILITY: log "init"               │
└─────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────┐
│  CAPABILITY DETECTOR runs                │
│  - Scan binaries                         │
│  - Test execution                        │
│  - Query device metrics                  │
│  → returns DeviceCapabilities            │
└─────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────┐
│  BACKEND ROUTER initializes               │
│  - Build priority vector from caps       │
└─────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────┐
│  GRACEFUL DEGRADATION enters state       │
│  - FULL or DEGRADED or MINIMAL          │
└─────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────┐
│  OBSERVABILITY: log "ready" with state   │
└─────────────────────────────────────────┘
```

---

## Implementation Phases

### Phase 1: Core Infrastructure (Hours 1-8)

| Task | Description |
|------|-------------|
| 1.1 | Add structured logging with boot stages to main.rs |
| 1.2 | Implement DeviceCapabilities struct |
| 1.3 | Implement capability detection function |
| 1.4 | Add failure classification function |
| 1.5 | Implement /health endpoint |

**Success Criteria**: Daemon starts, logs each stage, runs capability detection, exposes /health

### Phase 2: Degradation Engine (Hours 8-16)

| Task | Description |
|------|-------------|
| 2.1 | Define DegradationState enum (Full, Degraded, Minimal, Broken) |
| 2.2 | Implement state machine with event-driven transitions |
| 2.3 | Implement BackendRouter with priority vector logic |
| 2.4 | Add automatic retry with exponential backoff |
| 2.5 | Implement recovery detection |

**Success Criteria**: System falls back to llama-server when neocortex fails, logs transition

### Phase 3: Inference Integration (Hours 16-24)

| Task | Description |
|------|-------------|
| 3.1 | Modify ServerBackend initialization to receive DeviceCapabilities |
| 3.2 | Add inference routing via BackendRouter |
| 3.3 | Wire up failure events |
| 3.4 | Add response transformation |
| 3.5 | Add metrics collection |

**Success Criteria**: Queries work via llama-server, failures trigger degradation

### Phase 4: Testing and Validation (Hours 24-32)

| Task | Description |
|------|-------------|
| 4.1 | Unit tests for each component |
| 4.2 | Integration test with mock backends |
| 4.3 | Degradation scenario tests |
| 4.4 | End-to-end test with llama-server |
| 4.5 | Regression test suite |

**Success Criteria**: All tests pass, degradation scenarios work

### Phase 5: Documentation (Hours 32-40)

| Document | Description |
|----------|-------------|
| architecture/overview.md | System architecture diagram |
| build/contract.md | Toolchain versions, reproducibility |
| runtime/boot-stages.md | Startup sequence, expected logs |
| validation/device-matrix.md | Tested configurations |
| release/rollback.md | Deployment, rollback procedures |
| failure-db/signatures.md | Known failure patterns |
| incident/postmortems.md | Incident analysis format |

**Success Criteria**: All 7 documents created

---

## Agent Task Assignments

| Agent | Component | Files to Modify |
|-------|-----------|------------------|
| Agent A | Capability Detector | aura_config.rs - DeviceCapabilities, detection |
| Agent B | Observability Layer | new observability.rs - logging, classification |
| Agent C | Health Monitor | main.rs - HTTP endpoint |
| Agent D | Degradation Engine | new degradation.rs - state machine |
| Agent E | Backend Router | new router.rs - priority vector, routing |
| Agent F | Integration Testing | new tests/ - test harness |

---

## Team Structure (Per Operating Guide)

| Team | Responsibility | Deliverable |
|------|----------------|--------------|
| Product | Define what "working" means per degradation level | User journey, success criteria |
| Architecture | Define contracts, failure taxonomy | DeviceCapabilities contract, F001-F008 |
| Build/Infra | Reproducible artifacts, locked toolchain | Build scripts, provenance |
| Runtime Platform | Boot logging, failure classification | Observability Layer, Health Monitor |
| QA/Validation | Prove on target environment | Test suite, validation evidence |
| DevOps/Release | Rollout control, governance | Release procedure, rollback |
| Forensics | Convert failure to knowledge | Post-mortems, failure DB |

---

## Validation Gates (Decision Tree D1-D6)

| Gate | Question | Current Status | Implementation |
|------|----------|----------------|----------------|
| D1 | Did artifact build? | ✅ cargo build passes | - |
| D2 | Does artifact validate structurally? | ❌ Not done | Add ELF validation |
| D3 | Does it run on target device class? | ❌ Not done | Device test script |
| D4 | Did boot stages complete? | ❌ Not done | Parse boot logs |
| D5 | Is failure class known? | ❌ Not done | Trigger test failures |
| D6 | Should release be blocked? | ❌ Cannot answer | Based on D1-D5 |

---

## Proof Requirements - Bidirectional Validation

### Positive Proof (System Works)
- ✅ Boot logs show all stages in order
- ✅ Capability detection shows llama_server_available: true
- ✅ Health endpoint shows status: "degraded" with active_backend: "llama_server"
- ✅ Inference requests return responses from llama-server

### Negative Proof (System Fails with Evidence)
- ✅ Logs show "neocortex test failed failure_class=F003 exit_code=139 signal=SIGSEGV"
- ✅ Logs show "primary_failed_falling_back_to_secondary" when neocortex unavailable
- ✅ If both fail, logs show "all_backends_failed_operating_in_minimal_mode"

We can DEMONSTRATE both paths - this is enterprise operations.

---

## Neocortex Binary Problem - Resolution

**The Problem**: Build infrastructure failure (F003/F004) - binary crashes on any argument

**Not a Solution**: Rebuild for different Android version (wrong approach)

**Recommended Path**: 
1. **Immediate**: Use llama-server exclusively - works now
2. **Long-term**: Build on device via Termux (guaranteed compatibility)

**Graceful Degradation Handles This**:
- When neocortex fails detection (F003)
- System automatically uses llama-server
- User sees DEGRADED mode but system WORKS

---

## Key Interface Contracts

### DeviceCapabilities
```rust
pub struct DeviceCapabilities {
    pub neocortex_available: bool,
    pub neocortex_tested: bool,
    pub neocortex_failure_class: Option<FailureClass>,
    pub llama_server_available: bool,
    pub llama_server_tested: bool,
    pub llama_server_path: String,
    pub memory_mb: u64,
    pub android_api: u32,
    pub cpu_abi: String,
    pub cpu_cores: u32,
}
```

### InferenceRequest
```rust
pub struct InferenceRequest {
    pub prompt: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub system_prompt: Option<String>,
}
```

### InferenceResponse
```rust
pub struct InferenceResponse {
    pub text: String,
    pub tokens: u32,
    pub inference_ms: u64,
    pub backend_used: String,
}
```

---

## Configuration Schema (Enterprise Patterns)

```toml
[daemon]
port = 8080
log_level = "debug"

[server]
binary_path = "/data/local/tmp/llama/llama-server"
host = "127.0.0.1"
port = 8080
timeout_ms = 120000

[model]
name = "llama3"
context_size = 2048

[degradation]
enable_auto_fallback = true
max_retries = 3
backoff_ms = 1000
```

---

## Security Requirements

- Data at rest: encrypt model cache, use Android Keystore
- Data in transit: TLS for HTTP communication
- Access control: authenticate clients before inference
- Audit logging: log all sensitive operations
- Secrets: never commit to version control

---

## Timeline Summary

| Phase | Hours | Milestone |
|-------|-------|-----------|
| Phase 1 | 1-8 | Core infrastructure working |
| Phase 2 | 8-16 | Degradation engine working |
| Phase 3 | 16-24 | Inference integration working |
| Phase 4 | 24-32 | All tests passing |
| Phase 5 | 32-40 | All documents created |

**Total**: ~40 hours (approximately 1 week)

---

## Final Summary

This document provides a complete enterprise solution design that transforms AURA from a "college project that crashes" to an "enterprise platform that handles failures gracefully."

The key transformation:
- **Before**: One failure → crash → manual debugging
- **After**: One failure → classification → automatic fallback → user sees degraded but working system

The user controls the next steps. Implementation can begin immediately.

---

*Document generated from 50 sequential thinking thoughts during solution design meeting. Contains complete implementation specification, team assignments, and proof requirements.*