# AURA Enterprise Team Simulation - Complete Agent Allocation & Execution Plan

## Overview

This document orchestrates a simulated enterprise operation where 10 teams work in parallel on isolated git worktrees, implementing the enterprise solution design from the blueprint. Each team receives detailed prompts, context, and operates independently without interfering with others.

---

# PHASE 0: Infrastructure Setup

## 0.1 Verify Git Worktrees Directory

```bash
# Check if .worktrees exists and is ignored
ls -la /c/Users/Lenovo/aura/.worktrees 2>/dev/null || echo "Directory does not exist"

# Verify it's in .gitignore
grep -q ".worktrees" /c/Users/Lenovo/aura/.gitignore && echo "Already ignored" || echo "Need to add"
```

## 0.2 Create Worktrees for Each Team

| Team | Worktree Path | Branch Name | Purpose |
|------|---------------|-------------|---------|
| Capability Detection | `.worktrees/team-capability` | feature/capability-detection | DeviceCapabilities detection |
| Observability | `.worktrees/team-observability` | feature/observability-layer | Logging, boot stages, classification |
| Health Monitor | `.worktrees/team-health` | feature/health-monitor | HTTP endpoint |
| Degradation Engine | `.worktrees/team-degradation` | feature/degradation-engine | State machine |
| Backend Router | `.worktrees/team-router` | feature/backend-router | Inference routing |
| Platform Integration | `.worktrees/team-platform` | feature/platform-integration | Wire all components |
| QA Testing | `.worktrees/team-qa` | feature/qa-tests | Test suite |
| Documentation | `.worktrees/team-docs` | feature/documentation | Docs |
| Architecture Review | `.worktrees/team-arch` | feature/architecture-review | Contracts review |
| DevOps | `.worktrees/team-devops` | feature/devops-pipeline | CI/CD |

---

# PHASE 1: AGENT ALLOCATION & PROMPTS

## AGENT 1: CAPABILITY DETECTION SPECIALIST

**Worktree**: `.worktrees/team-capability`  
**Branch**: `feature/capability-detection`  
**Files to Modify**: 
- `crates/aura-neocortex/src/aura_config.rs`
- New file: `crates/aura-neocortex/src/capability_detection.rs`

### CONTEXT FOR AGENT

You are implementing the DeviceCapabilities detection system for the AURA daemon. This system runs FIRST at startup and identifies what inference backends are available on the device.

### PRE-READ REQUIREMENTS

Before starting, READ these files to understand the existing codebase:

1. **READ**: `crates/aura-neocortex/src/aura_config.rs` - Understand existing config structures
2. **READ**: `crates/aura-neocortex/src/main.rs` - Understand daemon startup flow
3. **READ**: `docs/ENTERPRISE-SOLUTION-DESIGN-MEETING-50-THOUGHTS.md` - Understand the solution design
4. **READ**: `config/aura.toml` - Understand current configuration format

### YOUR TASK

Implement the DeviceCapabilities detection system that:

1. **Scans for binaries** in `/data/local/tmp/` and `/data/local/tmp/llama/` directories
2. **Tests each binary** by executing with `--version` flag (timeout 5 seconds)
3. **Collects device metrics**:
   - Memory: read `/proc/meminfo`
   - Android API: run `getprop ro.build.version.sdk`
   - CPU ABI: run `getprop ro.product.cpu.abi`
4. **Returns DeviceCapabilities struct** with all fields populated

### SPECIFIC IMPLEMENTATION REQUIREMENTS

#### 1. Add DeviceCapabilities struct to aura_config.rs

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    // Backend availability
    pub neocortex_available: bool,
    pub neocortex_tested: bool,
    pub neocortex_failure_class: Option<FailureClass>,
    
    pub llama_server_available: bool,
    pub llama_server_tested: bool,
    pub llama_server_path: String,
    
    // Device metrics
    pub memory_mb: u64,
    pub android_api: u32,
    pub cpu_abi: String,
    pub cpu_cores: u32,
}
```

#### 2. Add FailureClass enum

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureClass {
    F001, // Artifact Missing
    F002, // Dependency Mismatch
    F003, // ABI Mismatch (OUR CASE - SIGSEGV)
    F004, // Linker Failure
    F005, // Runtime Crash
    F006, // Config Drift
    F007, // Observability Gap
    F008, // Governance Failure
}
```

#### 3. Implement capability_detection.rs

Create new file with:

- `scan_binary_directory(path: &str) -> Vec<BinaryInfo>` - Find potential binaries
- `test_binary_execution(path: &str, timeout_ms: u64) -> BinaryTestResult` - Test with --version
- `detect_device_metrics() -> DeviceMetrics` - Get memory, API, ABI
- `detect_capabilities() -> DeviceCapabilities` - Main detection function

### LOGGING REQUIREMENTS

All detection steps MUST log with structured format:
```
[TIMESTAMP] [LEVEL] [STAGE/CLASS] [COMPONENT] Message
```

Example:
```
[2026-03-27T10:30:45Z] [INFO] [environment_check] [capability_detector] scanning /data/local/tmp/ for binaries
[2026-03-27T10:30:46Z] [INFO] [environment_check] [capability_detector] found 2 potential binaries
[2026-03-27T10:30:47Z] [INFO] [environment_check] [capability_detector] testing neocortex with --version
[2026-03-27T10:30:47Z] [ERROR] [dependency_check] [capability_detector] neocortex test failed failure_class=F003 exit_code=139 signal=SIGSEGV
[2026-03-27T10:30:48Z] [INFO] [environment_check] [capability_detector] testing llama-server with --version
[2026-03-27T10:30:48Z] [INFO] [environment_check] [capability_detector] llama-server test passed
[2026-03-27T10:30:49Z] [INFO] [environment_check] [capability_detector] detected memory_mb=4096 android_api=35 cpu_abi=arm64-v8a
```

### ERROR HANDLING

- Detection failures should NOT crash the daemon
- Return partial results if some detection fails
- Log all failures with appropriate F001-F008 classification

### TEST REQUIREMENTS

Write unit tests for:
- Binary scanning (mock directory)
- Execution testing (mock binary that succeeds)
- Execution testing (mock binary that fails with SIGSEGV)
- Device metrics parsing

### OUTPUT FORMAT

When complete, your summary should include:
1. What you implemented (structs, functions)
2. How to test it works
3. Expected log output when running
4. Any issues encountered

---

## AGENT 2: OBSERVABILITY LAYER SPECIALIST

**Worktree**: `.worktrees/team-observability`  
**Branch**: `feature/observability-layer`  
**Files to Modify**:
- New file: `crates/aura-neocortex/src/observability.rs`
- Modify: `crates/aura-neocortex/src/main.rs` (add boot stage logging)

### CONTEXT FOR AGENT

You are implementing the observability layer that provides structured logging, boot stage tracking, and failure classification. This system makes the daemon "honest about what it's doing."

### PRE-READ REQUIREMENTS

1. **READ**: `crates/aura-neocortex/src/main.rs` - Understand daemon entry point
2. **READ**: `docs/ENTERPRISE-SOLUTION-DESIGN-MEETING-50-THOUGHTS.md` - Understand observability design
3. **READ**: `crates/aura-neocortex/src/aura_config.rs` - Understand config (for FailureClass)

### YOUR TASK

Implement the observability system that:

1. **Structured Logger** - All logs follow format: `[TIMESTAMP] [LEVEL] [STAGE/CLASS] [COMPONENT] Message`
2. **Boot Stage Tracker** - Log each startup phase: init → environment_check → dependency_check → runtime_start → ready
3. **Failure Classifier** - Map errors to F001-F008 taxonomy

### SPECIFIC IMPLEMENTATION REQUIREMENTS

#### 1. Create observability.rs with:

```rust
// Log levels
#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

// Boot stages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootStage {
    Init,
    EnvironmentCheck,
    DependencyCheck,
    RuntimeStart,
    Ready,
}

// Structured log entry
#[derive(Debug)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub stage: Option<BootStage>,
    pub failure_class: Option<FailureClass>,
    pub component: String,
    pub message: String,
}

// Observability system
pub struct ObservabilitySystem {
    log_writer: Box<dyn Write>,  // stdout or file
    current_stage: BootStage,
    boot_start_time: DateTime<Utc>,
}

impl ObservabilitySystem {
    pub fn new() -> Self;
    pub fn log(&self, level: LogLevel, stage: Option<BootStage>, failure_class: Option<FailureClass>, component: &str, message: &str);
    pub fn set_stage(&mut self, stage: BootStage);
    pub fn classify_error(&self, error: &str) -> FailureClass;
    pub fn boot_complete(&self) -> bool;
}
```

#### 2. Log format specification

```
[TIMESTAMP] [LEVEL] [STAGE/CLASS] [COMPONENT] Message

Examples:
[2026-03-27T10:30:45Z] [INFO] [init] [daemon] daemon starting version 1.0.0
[2026-03-27T10:30:46Z] [INFO] [environment_check] [capability_detector] detected memory_mb=4096 android_api=35
[2026-03-27T10:30:47Z] [ERROR] [dependency_check] [capability_detector] neocortex test failed failure_class=F003 exit_code=139
[2026-03-27T10:30:50Z] [INFO] [ready] [daemon] daemon operational mode=degraded active_backend=llama_server
```

#### 3. Failure classification logic

```rust
impl ObservabilitySystem {
    pub fn classify_error(&self, error: &str) -> FailureClass {
        // F001: "file not found", "no such file"
        if error.contains("not found") || error.contains("No such file") {
            return FailureClass::F001;
        }
        // F002: "library not found", "undefined symbol"
        if error.contains("library not found") || error.contains("undefined symbol") {
            return FailureClass::F002;
        }
        // F003: SIGSEGV, SIGABRT, signal
        if error.contains("SIGSEGV") || error.contains("signal") || error.contains("exit_code=139") {
            return FailureClass::F003;
        }
        // F004: linker errors
        if error.contains("linker") || error.contains("ld.so") {
            return FailureClass::F004;
        }
        // F005: panic during operation
        if error.contains("panic") || error.contains("thread ") {
            return FailureClass::F005;
        }
        // F006: config parse errors
        if error.contains("parse") || error.contains("toml") {
            return FailureClass::F006;
        }
        // F007: logging failures
        if error.contains("log") && error.contains("fail") {
            return FailureClass::F007;
        }
        // F008: governance (release without validation)
        FailureClass::F008
    }
}
```

#### 4. Integrate into main.rs

Add boot stage logging at startup:

```rust
fn main() {
    let obs = ObservabilitySystem::new();
    
    obs.log(LogLevel::Info, Some(BootStage::Init), None, "daemon", "daemon starting version 1.0.0");
    
    // ... capability detection ...
    obs.log(LogLevel::Info, Some(BootStage::EnvironmentCheck), None, "capability_detector", "detecting device capabilities");
    
    // ... binary verification ...
    obs.log(LogLevel::Info, Some(BootStage::DependencyCheck), None, "capability_detector", "verifying binaries");
    
    // ... backend start ...
    obs.log(LogLevel::Info, Some(BootStage::RuntimeStart), None, "backend", "starting inference backend");
    
    // ... ready ...
    obs.log(LogLevel::Info, Some(BootStage::Ready), None, "daemon", "daemon operational");
}
```

### TEST REQUIREMENTS

Write tests for:
- Log format correctness
- Boot stage sequence tracking
- Error classification for each F001-F008 type

### OUTPUT FORMAT

When complete:
1. What you implemented (structs, functions, integration)
2. Expected log output at each boot stage
3. How failure classification works
4. Test results

---

## AGENT 3: HEALTH MONITOR SPECIALIST

**Worktree**: `.worktrees/team-health`  
**Branch**: `feature/health-monitor`  
**Files to Modify**:
- New file: `crates/aura-neocortex/src/health_monitor.rs`
- Modify: `crates/aura-neocortex/src/main.rs` (add HTTP server)

### CONTEXT FOR AGENT

You are implementing the health monitoring endpoint that exposes system state via HTTP. This enables external monitoring, load balancer health checks, and diagnostics.

### PRE-READ REQUIREMENTS

1. **READ**: `crates/aura-neocortex/src/main.rs` - Current daemon structure
2. **READ**: `crates/aura-neocortex/src/aura_config.rs` - For DeviceCapabilities

### YOUR TASK

Implement a minimal HTTP server that exposes `/health` endpoint returning JSON with daemon state.

### SPECIFIC IMPLEMENTATION REQUIREMENTS

#### 1. Create health_monitor.rs

```rust
use std::net::TcpListener;
use std::io::{Read, Write};
use serde::{Serialize, Deserialize};
use crate::aura_config::{DeviceCapabilities, DegradationState};

// Health response structure
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub version: String,
    pub uptime_seconds: u64,
    pub status: String,  // "full", "degraded", "minimal", "broken"
    pub boot_stages: BootStages,
    pub backends: Backends,
    pub active_backend: String,
    pub degradation_level: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_inference_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_requests: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BootStages {
    pub init: String,
    pub environment_check: String,
    pub dependency_check: String,
    pub runtime_start: String,
    pub ready: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Backends {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub neocortex: Option<BackendStatus>,
    pub llama_server: BackendStatus,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BackendStatus {
    pub available: bool,
    pub tested: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

// Health monitor system
pub struct HealthMonitor {
    start_time: DateTime<Utc>,
    state: HealthState,
    // ... internal state ...
}

impl HealthMonitor {
    pub fn new() -> Self;
    pub fn get_health(&self) -> HealthResponse;
    pub fn start_server(&self, port: u16) -> Result<(), std::io::Error>;
    pub fn update_state(&mut self, capabilities: &DeviceCapabilities, degradation_state: &DegradationState);
}
```

#### 2. HTTP Server Requirements

- Bind to configurable port (default 8080 or from config)
- Handle GET /health
- Return JSON with Content-Type: application/json
- Request timeout: 5 seconds
- If server fails to bind: log warning, continue WITHOUT crashing

#### 3. Example Response

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
      "last_error": "SIGSEGV exit 139"
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

### TEST REQUIREMENTS

- Test JSON serialization
- Test HTTP response parsing
- Test endpoint availability

### OUTPUT FORMAT

When complete:
1. Implementation overview
2. How to query health endpoint
3. Example responses for different states

---

## AGENT 4: DEGRADATION ENGINE SPECIALIST

**Worktree**: `.worktrees/team-degradation`  
**Branch**: `feature/degradation-engine`  
**Files to Modify**:
- New file: `crates/aura-neocortex/src/degradation_engine.rs`

### CONTEXT FOR AGENT

You are implementing the graceful degradation state machine. This system handles failures WITHOUT crashing - it automatically falls back to secondary backends when primary fails.

### PRE-READ REQUIREMENTS

1. **READ**: `crates/aura-neocortex/src/aura_config.rs` - For FailureClass definition
2. **READ**: `docs/ENTERPRISE-SOLUTION-DESIGN-MEETING-50-THOUGHTS.md` - Understand state machine design

### YOUR TASK

Implement the state machine that handles degradation levels and transitions.

### SPECIFIC IMPLEMENTATION REQUIREMENTS

#### 1. Create degradation_engine.rs

```rust
use crate::aura_config::{FailureClass, DeviceCapabilities};

// Degradation states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DegradationState {
    Full,       // Primary backend works, all features
    Degraded,   // Primary fails, secondary works
    Minimal,    // All backends fail, daemon runs, offline mode
    Broken,     // Daemon fails, logs why
}

// Events that trigger state transitions
#[derive(Debug)]
pub enum DegradationEvent {
    InferenceSuccess { backend: String },
    InferenceFailure { backend: String, failure_class: FailureClass, error: String },
    BackendBecameUnavailable { backend: String },
    RecoveryPossible { backend: String },
}

// State machine
pub struct DegradationEngine {
    current_state: DegradationState,
    primary_backend: Option<String>,
    secondary_backend: Option<String>,
    active_backend: Option<String>,
    transition_history: Vec<StateTransition>,
}

#[derive(Debug)]
pub struct StateTransition {
    pub from_state: DegradationState,
    pub to_state: DegradationState,
    pub trigger: String,
    pub timestamp: DateTime<Utc>,
}

impl DegradationEngine {
    pub fn new() -> Self;
    
    // Initialize based on capabilities
    pub fn initialize(&mut self, capabilities: &DeviceCapabilities);
    
    // Handle events and potentially transition
    pub fn handle_event(&mut self, event: DegradationEvent) -> DegradationState;
    
    // Get current state
    pub fn get_state(&self) -> DegradationState;
    
    // Get active backend
    pub fn get_active_backend(&self) -> Option<&String>;
    
    // Check if can handle inference
    pub fn can_handle_inference(&self) -> bool;
    
    // Get transition history for logging
    pub fn get_transition_log(&self) -> &[StateTransition];
}
```

#### 2. State Transition Logic

```rust
impl DegradationEngine {
    pub fn handle_event(&mut self, event: DegradationEvent) -> DegradationState {
        match event {
            DegradationEvent::InferenceFailure { backend, failure_class, error } => {
                // If primary fails, try secondary
                if Some(&backend) == self.primary_backend.as_ref() {
                    if self.secondary_backend.is_some() {
                        let old_state = self.current_state;
                        self.current_state = DegradationState::Degraded;
                        self.active_backend = self.secondary_backend.clone();
                        self.log_transition(old_state, DegradationState::Degraded, 
                            &format!("primary_failed_falling_back_to_secondary"));
                    } else {
                        let old_state = self.current_state;
                        self.current_state = DegradationState::Minimal;
                        self.log_transition(old_state, DegradationState::Minimal,
                            &format!("all_backends_failed_operating_in_minimal_mode"));
                    }
                }
            }
            DegradationEvent::RecoveryPossible { backend } => {
                // If failed backend becomes available again, can promote
                if backend == *self.primary_backend.as_ref().unwrap() 
                    && self.current_state == DegradationState::Degraded 
                {
                    let old_state = self.current_state;
                    self.current_state = DegradationState::Full;
                    self.active_backend = self.primary_backend.clone();
                    self.log_transition(old_state, DegradationState::Full,
                        &format!("backend_recovered_upgrading_to_full"));
                }
            }
            _ => {}
        }
        self.current_state
    }
}
```

#### 3. Logging Requirements

Log all state transitions:
```
[2026-03-27T10:30:47Z] [INFO] [runtime] [degradation_engine] primary_failed_falling_back_to_secondary
[2026-03-27T10:30:48Z] [WARN] [runtime] [degradation_engine] all_backends_failed_operating_in_minimal_mode
```

### TEST REQUIREMENTS

- Test state transitions (Full → Degraded → Minimal)
- Test fallback when primary fails
- Test recovery when backend becomes available

### OUTPUT FORMAT

When complete:
1. State machine implementation
2. How transitions work
3. Test scenarios and results

---

## AGENT 5: BACKEND ROUTER SPECIALIST

**Worktree**: `.worktrees/team-router`  
**Branch**: `feature/backend-router`  
**Files to Modify**:
- New file: `crates/aura-neocortex/src/backend_router.rs`
- Modify: `crates/aura-neocortex/src/model.rs` (integrate routing)

### CONTEXT FOR AGENT

You are implementing the inference routing system that selects which backend to use based on availability, implements retry logic, and coordinates with the degradation engine.

### PRE-READ REQUIREMENTS

1. **READ**: `crates/aura-neocortex/src/model.rs` - Current model/inference implementation
2. **READ**: `crates/aura-neocortex/src/aura_config.rs` - For config structures
3. **READ**: `docs/config/aura.toml` - Current config format

### YOUR TASK

Implement the BackendRouter that routes inference requests to appropriate backends with fallback and retry logic.

### SPECIFIC IMPLEMENTATION REQUIREMENTS

#### 1. Create backend_router.rs

```rust
use crate::aura_config::{DeviceCapabilities, InferenceOptions, InferenceResponse};
use crate::degradation_engine::DegradationState;

// Priority vector - ordered list of backends to try
#[derive(Debug)]
pub struct BackendRouter {
    priority_vector: Vec<BackendInfo>,
    current_index: usize,
    retry_count: usize,
    max_retries: u8,
    backoff_ms: u64,
}

#[derive(Debug, Clone)]
pub struct BackendInfo {
    pub name: String,
    pub path: Option<String>,
    pub available: bool,
    pub tested: bool,
}

impl BackendRouter {
    pub fn new() -> Self;
    
    // Build priority vector from capabilities
    pub fn build_priority_vector(&mut self, capabilities: &DeviceCapabilities);
    
    // Get next available backend
    pub fn get_next_backend(&mut self) -> Option<&BackendInfo>;
    
    // Handle failure - try next backend or return error
    pub fn handle_failure(&mut self, error: &str) -> Option<&BackendInfo>;
    
    // Reset for new request
    pub fn reset(&mut self);
    
    // Execute inference with routing
    pub fn route_inference(&mut self, prompt: &str, options: &InferenceOptions) -> Result<InferenceResponse, RouterError>;
}

#[derive(Debug)]
pub struct RouterError {
    pub failure_class: FailureClass,
    pub attempted_backends: Vec<String>,
    pub error_message: String,
}
```

#### 2. Priority Vector Building

```rust
impl BackendRouter {
    pub fn build_priority_vector(&mut self, capabilities: &DeviceCapabilities) {
        self.priority_vector.clear();
        
        // If neocortex is available and tested, it's primary
        if capabilities.neocortex_available && capabilities.neocortex_tested {
            self.priority_vector.push(BackendInfo {
                name: "neocortex".to_string(),
                path: None,  // Use default path
                available: true,
                tested: true,
            });
        }
        
        // Always include llama-server as fallback
        if capabilities.llama_server_available && capabilities.llama_server_tested {
            self.priority_vector.push(BackendInfo {
                name: "llama_server".to_string(),
                path: Some(capabilities.llama_server_path.clone()),
                available: true,
                tested: true,
            });
        }
        
        self.current_index = 0;
    }
}
```

#### 3. Retry Logic with Exponential Backoff

```rust
impl BackendRouter {
    pub fn handle_failure(&mut self, error: &str) -> Option<&BackendInfo> {
        self.retry_count += 1;
        
        if self.retry_count >= self.max_retries {
            // Exhausted retries
            return None;
        }
        
        // Move to next backend in priority
        self.current_index += 1;
        
        if self.current_index < self.priority_vector.len() {
            // Exponential backoff before retry
            let backoff = self.backoff_ms * (2_u64.pow(self.retry_count as u32));
            std::thread::sleep(std::time::Duration::from_millis(backoff));
            
            Some(&self.priority_vector[self.current_index])
        } else {
            None
        }
    }
}
```

#### 4. Integration with model.rs

Replace direct backend calls with router:

```rust
// In model.rs - instead of:
let response = self.server_backend.infer(prompt, options).await?;

// Use:
let response = self.router.route_inference(prompt, options).await?;
```

### TEST REQUIREMENTS

- Test priority vector building
- Test fallback when primary fails
- Test retry with backoff timing

### OUTPUT FORMAT

When complete:
1. Router implementation
2. How fallback works
3. Test results

---

## AGENT 6: PLATFORM INTEGRATION SPECIALIST

**Worktree**: `.worktrees/team-platform`  
**Branch**: `feature/platform-integration`  
**Files to Modify**:
- `crates/aura-neocortex/src/main.rs` - Wire all components together
- Existing config files

### CONTEXT FOR AGENT

You are the integration specialist who wires all the components together - capability detection, observability, health monitor, degradation engine, and backend router.

### PRE-READ REQUIREMENTS

1. **READ**: `crates/aura-neocortex/src/main.rs` - Current daemon entry
2. **READ**: All files created by other agents (capability_detection.rs, observability.rs, health_monitor.rs, degradation_engine.rs, backend_router.rs)

### YOUR TASK

Wire all components together in main.rs to create the complete enterprise system.

### SPECIFIC IMPLEMENTATION REQUIREMENTS

#### 1. Main.rs Integration

```rust
fn main() {
    // Initialize observability first
    let obs = ObservabilitySystem::new();
    obs.log(LogLevel::Info, Some(BootStage::Init), None, "daemon", "daemon starting version 1.0.0");
    
    // Phase 1: Capability Detection
    obs.log(LogLevel::Info, Some(BootStage::EnvironmentCheck), None, "capability_detector", "detecting device capabilities");
    let capabilities = detect_capabilities();
    obs.log(LogLevel::Info, Some(BootStage::EnvironmentCheck), None, "capability_detector", 
        &format!("detected: neocortex={}, llama_server={}", 
            capabilities.neocortex_available, capabilities.llama_server_available));
    
    // Phase 2: Initialize Degradation Engine
    let mut degradation_engine = DegradationEngine::new();
    degradation_engine.initialize(&capabilities);
    let state = degradation_engine.get_state();
    
    // Phase 3: Initialize Router
    let mut router = BackendRouter::new();
    router.build_priority_vector(&capabilities);
    
    // Phase 4: Initialize Health Monitor
    let health_monitor = HealthMonitor::new();
    health_monitor.update_state(&capabilities, &state);
    
    // Phase 5: Start HTTP server in background
    std::thread::spawn(|| {
        if let Err(e) = health_monitor.start_server(8080) {
            eprintln!("Health server failed to start: {}", e);
        }
    });
    
    // Boot stages complete
    obs.log(LogLevel::Info, Some(BootStage::DependencyCheck), None, "capability_detector", "binary verification complete");
    obs.log(LogLevel::Info, Some(BootStage::RuntimeStart), None, "backend", "inference backend initialized");
    obs.log(LogLevel::Info, Some(BootStage::Ready), None, "daemon", 
        &format!("daemon operational mode={:?} active_backend={:?}", 
            state, router.get_active_backend()));
    
    // Main loop - handle inference requests
    loop {
        // Handle client connections
        // Route through router
        // Update health state
        // Emit events to degradation engine
    }
}
```

#### 2. Error Handling Integration

When inference fails:

```rust
// In inference loop
match router.route_inference(prompt, &options) {
    Ok(response) => {
        // Success - update metrics
    }
    Err(e) => {
        // Failure - emit event to degradation engine
        degradation_engine.handle_event(DegradationEvent::InferenceFailure {
            backend: router.get_active_backend().unwrap().clone(),
            failure_class: e.failure_class,
            error: e.error_message,
        });
        
        // Check if we need to degrade
        let new_state = degradation_engine.get_state();
        health_monitor.update_state(&capabilities, &new_state);
    }
}
```

### OUTPUT FORMAT

When complete:
1. Complete main.rs with all integrations
2. How all components work together
3. Full startup sequence

---

# PHASE 2: EXECUTION INSTRUCTIONS

## How to Execute This Plan

### Step 1: Create Worktrees (Sequential)

For each team, create an isolated worktree:

```bash
cd /c/Users/Lenovo/aura

# Create worktree for Capability Detection
git worktree add .worktrees/team-capability -b feature/capability-detection

# Create worktree for Observability  
git worktree add .worktrees/team-observability -b feature/observability-layer

# ... repeat for each team
```

### Step 2: Dispatch Agents (Parallel)

Dispatch each agent to their respective worktree:

1. **Agent 1 (Capability)**: Work on `.worktrees/team-capability`
2. **Agent 2 (Observability)**: Work on `.worktrees/team-observability`
3. **Agent 3 (Health)**: Work on `.worktrees/team-health`
4. **Agent 4 (Degradation)**: Work on `.worktrees/team-degradation`
5. **Agent 5 (Router)**: Work on `.worktrees/team-router`
6. **Agent 6 (Integration)**: Work on `.worktrees/team-platform`

### Step 3: Independent Testing

Each agent tests in their isolated environment:
- Run `cargo build` in their worktree
- Run `cargo test` in their worktree
- Verify their component works

### Step 4: Integration Testing

After individual work is complete:
- Merge branches to staging
- Run full integration tests
- Verify all components work together

### Step 5: Main Branch Update

Only after all tests pass:
- Merge to main
- Main branch contains complete enterprise system

---

# PROOF REQUIREMENTS

## Positive Proof (System Works)

- [ ] Boot logs show all 5 stages
- [ ] Capability detection populates DeviceCapabilities
- [ ] Health endpoint returns JSON
- [ ] Inference works via llama-server
- [ ] Degradation works (primary fails → secondary)

## Negative Proof (System Fails with Evidence)

- [ ] Logs show F003 classification for neocortex crash
- [ ] Logs show fallback transition
- [ ] Health shows degraded status
- [ ] All failures classified properly

---

This document provides the complete enterprise simulation. Each agent receives specific, detailed instructions with pre-read requirements, implementation specs, and test requirements.