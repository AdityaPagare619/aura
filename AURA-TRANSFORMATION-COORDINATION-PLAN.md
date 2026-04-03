# AURA ENTERPRISE TRANSFORMATION - MASTER COORDINATION PLAN

**Document Version:** 1.0
**Created:** 2026-04-02
**Status:** ACTIVE
**Orchestrator:** 🎯 General (Enterprise Coordinator)
**Scope:** Transform AURA from "works on my device" to "production-grade for ALL Android devices"

---

## TABLE OF CONTENTS

1. [Executive Summary](#1-executive-summary)
2. [Department Coordination Matrix](#2-department-coordination-matrix)
3. [Phase-by-Phase Plan](#3-phase-by-phase-plan)
4. [Detailed Department Plans](#4-detailed-department-plans)
5. [Success Metrics](#5-success-metrics)
6. [Risk Mitigation](#6-risk-mitigation)
7. [Immediate Next Steps](#7-immediate-next-steps)

---

## 1. EXECUTIVE SUMMARY

### Mission Statement
Transform AURA from a device-specific prototype into a production-grade, device-agnostic Android AI assistant capable of running on ALL Android devices.

### Current State
- **127+ issues** identified across 6 categories
- **34 critical issues** blocking production deployment
- **150+ file paths** referenced with specific line numbers
- **200+ recommendations** for transformation
- **213+ Rust files** across 5 crates requiring attention

### Target State
- Zero critical security vulnerabilities
- Device-agnostic architecture with no hardcoded paths
- Automated CI/CD pipeline for all Android devices
- 80%+ test coverage
- One-click installation process
- Production-grade code quality

### Transformation Principles
1. **Limit the problem space with contracts** — Don't fix everything, focus on architecture
2. **Trust validated runtime behavior** — Not just builds
3. **Eliminate classes of failure** — Not debug repeatedly
4. **Code for guarantees (ABI, OS contracts)** — Not for devices
5. **Depend on systems + pipelines** — Not individuals

### Lessons Learned Integration
| Past Issue | Root Cause | Solution Applied |
|------------|------------|------------------|
| SIGSEGV crashes | LTO=true + panic=abort | LTO=thin, panic=unwind |
| libloading crashes | Direct C library loading on Android | HTTP backend architecture |
| Device-specific failures | Hardcoded paths | Environment variable contracts |
| CI failures | NDK r27d vs r26b mismatch | Standardize on r26b |
| Security breach | Exposed secrets in config.toml | Environment variable contracts |

---

## 2. DEPARTMENT COORDINATION MATRIX

### 2.1 Department Overview

| # | Department | Role | Agent Type | Priority | Dependencies |
|---|------------|------|------------|----------|--------------|
| 1 | 🔒 Security | Fix 5 critical vulnerabilities | Security + Debugger | CRITICAL | None |
| 2 | 🏗️ Architecture | Transform to device-agnostic | Architect | CRITICAL | None |
| 3 | 🔧 Build System | Fix for all platforms | Tool Engineer | CRITICAL | None |
| 4 | 💎 Code Quality | Production-grade code | Reviewer | HIGH | Security, Architecture |
| 5 | 🚀 Deployment | One-click installation | Builder | HIGH | Build System, Architecture |
| 6 | 🧪 Testing | Full coverage | TDD Guide | HIGH | Security, Architecture, Code Quality |
| 7 | 📚 Documentation | Complete docs | General | MEDIUM | All departments |
| 8 | ⚙️ DevOps | CI/CD pipeline | MCP Engineer | HIGH | Build System, Testing |
| 9 | 🏢 Infrastructure | Scaling and monitoring | Architect | MEDIUM | DevOps, Deployment |
| 10 | 🔬 Research | Best practices | Researcher | LOW | None |
| 11 | 🎨 Design | User experience | General | LOW | Architecture |
| 12 | ✅ Review | Code inspection | Reviewer | HIGH | All departments |

### 2.2 Dependency Flow

```
PHASE 0 (24 hours) — CRITICAL FIXES
┌─────────────────────────────────────────────────────────┐
│                                                         │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐              │
│  │ Security │  │Architect │  │  Build   │              │
│  │ 5 vulns  │  │ 4 issues │  │ 4 issues │              │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘              │
│       │             │             │                     │
│       └─────────────┼─────────────┘                     │
│                     ▼                                   │
│              PHASE 1 (Week 1)                           │
│  ┌──────────────────────────────────────────┐           │
│  │ Code Quality (8 issues) ← Dep: Sec+Arch │           │
│  └───────────────────┬──────────────────────┘           │
│                      ▼                                  │
│              PHASE 2 (Week 2)                           │
│  ┌──────────────────────────────────────────┐           │
│  │ Deployment (7 issues) ← Dep: Build+Arch │           │
│  └───────────────────┬──────────────────────┘           │
│                      ▼                                  │
│              PHASE 3 (Week 3)                           │
│  ┌──────────────────────────────────────────┐           │
│  │ Testing (8 gaps) ← Dep: Sec+Arch+Qual   │           │
│  └───────────────────┬──────────────────────┘           │
│                      ▼                                  │
│              PHASE 4 (Week 4)                           │
│  ┌──────────────────────────────────────────┐           │
│  │ Docs + Review ← Dep: All departments     │           │
│  └──────────────────────────────────────────┘           │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

### 2.3 Parallel Execution Rules

**Can run in parallel:**
- Security + Architecture + Build System (Phase 0)
- Code Quality departments within same phase
- Testing across different modules

**Must be sequential:**
- Security fixes before Code Quality review
- Architecture before Deployment
- Build System before DevOps pipeline
- Testing after all implementation complete

---

## 3. PHASE-BY-PHASE PLAN

### PHASE 0: CRITICAL FIXES (24 Hours)

**Objective:** Eliminate all 34 critical issues blocking production
**Agents:** Security, Architect, Tool Engineer (3 agents parallel)

#### 0.1 Security Critical Fixes (8 hours)

| Issue | File:Line | CWE | Fix Strategy | Agent |
|-------|-----------|-----|--------------|-------|
| JNI Use-After-Free | lib.rs:179 | CWE-416 | Add null check before Box::from_raw | Security |
| unsafe impl Send (7 types) | voice/*.rs | CWE-362 | Add Send trait bounds to wrapper types | Security |
| from_raw_parts validation | lib.rs:1465,1494,1674 | CWE-119 | Add length validation + null checks | Security |
| Path Traversal | lib.rs:1331-1364 | CWE-22 | Sanitize paths, use PathBuf | Security |
| URL Injection | jni_bridge.rs:666-680 | CWE-939 | Validate URL scheme + domain whitelist | Security |

**Contract: Security**
```rust
// SAFETY CONTRACT: All FFI boundaries must validate:
// 1. Pointers are non-null
// 2. Pointers are aligned
// 3. Lengths are within bounds
// 4. No use-after-free (check Box::from_raw safety)
// 5. All URLs validated against whitelist
// 6. All paths sanitized against traversal
```

#### 0.2 Architecture Critical Fixes (8 hours)

| Issue | File:Line | Fix Strategy | Agent |
|-------|-----------|--------------|-------|
| Hardcoded neocortex path | spawn.rs:34 | Use AURA_NEOCORTEX_PATH env var | Architect |
| Hardcoded Termux fallback | main.rs:88 | Use AURA_HOME env var | Architect |
| Hardcoded Android DB path | config.rs:280 | Use AURA_DB_PATH env var | Architect |
| Hardcoded model directory | config.rs:178 | Use AURA_MODELS_PATH env var | Architect |

**Contract: Architecture**
```rust
// DEVICE-AGNOSTIC CONTRACT:
// 1. ALL paths must come from environment variables
// 2. ALL environment variables must have fallback defaults
// 3. NO hardcoded device-specific paths
// 4. Path resolution order: Env Var → User Config → Default
// 5. All paths must be absolute and cross-platform
```

#### 0.3 Build System Critical Fixes (8 hours)

| Issue | File:Line | Fix Strategy | Agent |
|-------|-----------|--------------|-------|
| NDK r27d vs r26b | .cargo/config.toml:10 | Standardize on r26b | Tool Engineer |
| Hardcoded Windows path | .cargo/config.toml:10 | Use $ANDROID_NDK_HOME | Tool Engineer |
| API Level mismatch | config.toml | Standardize on API 26 | Tool Engineer |
| curl-backend confusion | Cargo.toml | Remove curl, use reqwest | Tool Engineer |

**Contract: Build System**
```rust
// BUILD CONTRACT:
// 1. NDK version: r26b (not r27d)
// 2. API level: 26 (not 21)
// 3. HTTP client: reqwest (not curl-backend)
// 4. NDK path: $ANDROID_NDK_HOME (not hardcoded)
// 5. Target: aarch64-linux-android (primary)
// 6. LTO: thin (not true)
// 7. panic: unwind (not abort)
```

---

### PHASE 1: ARCHITECTURE TRANSFORMATION (Week 1)

**Objective:** Transform architecture to be truly device-agnostic
**Agents:** Architect, Builder, Reviewer

#### 1.1 Environment Variable Contract System

**Create:** `aura-daemon/src/platform/env_contract.rs`

```rust
// ENVIRONMENT VARIABLE CONTRACT
// All device-specific configuration MUST come from these variables:
//
// AURA_HOME          — Base directory for all AURA data
// AURA_NEOCORTEX_PATH — Path to neocortex binary
// AURA_DB_PATH       — Path to SQLite database
// AURA_MODELS_PATH   — Path to model files
// AURA_LOG_PATH      — Path to log files
// AURA_CONFIG_PATH   — Path to configuration
// AURA_TEMP_PATH     — Path to temporary files
//
// Fallback resolution order:
// 1. Environment variable
// 2. User configuration file
// 3. Platform-specific default
// 4. Hardcoded default (last resort)
```

**Tasks:**
1. Create `platform/env_contract.rs` with all env var definitions
2. Create `platform/path_resolver.rs` with fallback chain
3. Update `config.rs:178,280` to use env contract
4. Update `spawn.rs:34` to use env contract
5. Update `main.rs:88` to use env contract
6. Add env var validation at startup
7. Add env var documentation to README

#### 1.2 Device Abstraction Layer

**Create:** `aura-daemon/src/platform/device.rs`

```rust
// DEVICE ABSTRACTION CONTRACT
// Abstract all device-specific behavior behind trait boundaries:
//
// trait DevicePlatform {
//     fn home_dir(&self) -> PathBuf;
//     fn data_dir(&self) -> PathBuf;
//     fn cache_dir(&self) -> PathBuf;
//     fn temp_dir(&self) -> PathBuf;
//     fn is_rooted(&self) -> bool;
//     fn api_level(&self) -> u32;
// }
```

**Tasks:**
1. Create `platform/device.rs` with DevicePlatform trait
2. Create `platform/android.rs` with Android implementation
3. Create `platform/linux.rs` with Linux implementation
4. Create `platform/windows.rs` with Windows implementation
5. Update all hardcoded paths to use DevicePlatform
6. Add device capability detection
7. Add graceful degradation for unsupported features

#### 1.3 Configuration Abstraction

**Create:** `aura-daemon/src/config/contract.rs`

```rust
// CONFIGURATION CONTRACT
// All configuration must be:
// 1. Serializable (serde)
// 2. Validatable (custom validators)
// 3. Documentable (comments)
// 4. Testable (unit tests)
// 5. Defaultable (sensible defaults)
```

**Tasks:**
1. Create `config/contract.rs` with all config structs
2. Add serde derive for all config types
3. Add custom validators for all config fields
4. Add documentation for all config options
5. Add default implementations
6. Create config migration system
7. Add config validation at startup

---

### PHASE 2: BUILD SYSTEM & DEPLOYMENT (Week 2)

**Objective:** Automated build and one-click deployment
**Agents:** Tool Engineer, Builder, MCP Engineer

#### 2.1 Build System Standardization

**Tasks:**
1. Standardize NDK to r26b across all platforms
2. Create `.cargo/config.toml` template with env vars
3. Add NDK version detection and validation
4. Create build scripts for all target platforms
5. Add build artifact checksums
6. Create build verification tests
7. Add build caching for CI

#### 2.2 CI/CD Pipeline

**Create:** `.github/workflows/aura-ci.yml`

```yaml
# CI/CD CONTRACT
# Pipeline stages:
# 1. Lint (clippy, rustfmt)
# 2. Build (all targets)
# 3. Test (unit + integration)
# 4. Security (cargo-audit)
# 5. Package (APK generation)
# 6. Deploy (if main branch)
```

**Tasks:**
1. Create GitHub Actions workflow
2. Add clippy + rustfmt checks
3. Add cargo-audit security scan
4. Add multi-target build matrix
5. Add APK generation
6. Add deployment automation
7. Add rollback mechanism

#### 2.3 Deployment System

**Create:** `deploy/` directory with:

**Tasks:**
1. Create `deploy/install.sh` with checksum verification
2. Create `deploy/uninstall.sh` with cleanup
3. Create `deploy/rollback.sh` with version management
4. Create `deploy/health-check.sh` with post-deploy validation
5. Add deployment logging
6. Add deployment notifications
7. Create deployment documentation

**Contract: Deployment**
```bash
# DEPLOYMENT CONTRACT:
# 1. Checksum verification before install
# 2. Rollback mechanism before overwriting
# 3. Health check after deployment
# 4. Graceful shutdown before update
# 5. Deployment logging for debugging
# 6. Version consistency across all artifacts
```

---

### PHASE 3: TESTING & CODE QUALITY (Week 3)

**Objective:** Full test coverage and production-grade code
**Agents:** TDD Guide, Reviewer, Debugger

#### 3.1 Test Coverage Strategy

**Target:** 80%+ code coverage

| Module | Current Coverage | Target | Tests Needed |
|--------|------------------|--------|--------------|
| Security (FFI) | 0% | 90% | FFI boundary tests |
| Architecture | 0% | 85% | Env var contract tests |
| Build System | 0% | 80% | Build verification tests |
| IPC | 20% | 90% | Roundtrip tests |
| Voice | 0% | 75% | Subsystem tests |
| Memory | 10% | 85% | Memory subsystem tests |
| HTTP Backend | 0% | 80% | Mock server tests |

#### 3.2 Test Implementation Plan

**Security Tests (TDD):**
```rust
#[cfg(test)]
mod security_tests {
    use super::*;
    
    #[test]
    fn test_ffi_null_pointer_rejection() {
        // Test that null pointers are rejected
    }
    
    #[test]
    fn test_path_traversal_prevention() {
        // Test that ../.. is rejected
    }
    
    #[test]
    fn test_url_injection_prevention() {
        // Test that malicious URLs are rejected
    }
    
    #[test]
    fn test_send_trait_safety() {
        // Test that Send implementations are safe
    }
}
```

**Architecture Tests:**
```rust
#[cfg(test)]
mod architecture_tests {
    #[test]
    fn test_env_var_resolution() {
        // Test env var → fallback chain
    }
    
    #[test]
    fn test_device_abstraction() {
        // Test DevicePlatform trait
    }
    
    #[test]
    fn test_no_hardcoded_paths() {
        // Scan for hardcoded paths
    }
}
```

**Integration Tests:**
```rust
#[cfg(test)]
mod integration_tests {
    #[test]
    fn test_daemon_neocortex_communication() {
        // Test HTTP backend communication
    }
    
    #[test]
    fn test_ipc_roundtrip() {
        // Test authenticated IPC
    }
    
    #[test]
    fn test_voice_subsystem() {
        // Test voice pipeline
    }
}
```

#### 3.3 Code Quality Improvements

**Tasks:**
1. Remove duplicate code in `telegram/mod.rs` (200+ lines)
2. Fix null pointer returns in `server_http_backend.rs:196`
3. Implement tokenize/detokenize in `server_http_backend.rs:205-216`
4. Fix token handling bugs in `server_http_backend.rs:228-253`
5. Replace magic numbers with named constants
6. Break down large functions (250-500+ lines)
7. Standardize error types
8. Fix compiler warnings (unused fields/imports)

**Contract: Code Quality**
```rust
// CODE QUALITY CONTRACT:
// 1. No duplicate code (DRY)
// 2. No null pointer returns
// 3. All functions implemented (no stubs)
// 4. All magic numbers named
// 5. Functions < 100 lines
// 6. Consistent error types
// 7. Zero compiler warnings
// 8. All public APIs documented
```

---

### PHASE 4: DOCUMENTATION & REVIEW (Week 4)

**Objective:** Complete documentation and final review
**Agents:** General, Reviewer, Researcher

#### 4.1 Documentation Strategy

**Documents to create:**
1. `README.md` — Project overview and quick start
2. `ARCHITECTURE.md` — System architecture and design decisions
3. `API.md` — API documentation
4. `DEPLOYMENT.md` — Deployment guide
5. `SECURITY.md` — Security considerations
6. `TESTING.md` — Testing guide
7. `CONTRIBUTING.md` — Contribution guidelines
8. `CHANGELOG.md` — Version history

#### 4.2 Final Review Process

**Review Checklist:**
- [ ] All 34 critical issues resolved
- [ ] All hardcoded paths eliminated
- [ ] All security vulnerabilities fixed
- [ ] All tests passing
- [ ] All documentation complete
- [ ] CI/CD pipeline working
- [ ] Deployment tested on real devices
- [ ] Performance benchmarks documented
- [ ] Code review completed
- [ ] Security audit passed

#### 4.3 Production Readiness Assessment

**Readiness Criteria:**
1. **Security:** Zero critical vulnerabilities
2. **Architecture:** Device-agnostic with env contracts
3. **Build:** Automated CI/CD for all targets
4. **Testing:** 80%+ coverage
5. **Code Quality:** Zero compiler warnings
6. **Deployment:** One-click install with rollback
7. **Documentation:** Complete and up-to-date
8. **Performance:** Benchmarks documented

---

## 4. DETAILED DEPARTMENT PLANS

### 4.1 🔒 SECURITY DEPARTMENT

**Role:** Fix all 5 critical security vulnerabilities
**Agent:** Security + Debugger
**Priority:** CRITICAL
**Dependencies:** None

#### Task 1.1: Fix JNI Use-After-Free (CWE-416)
**File:** `lib.rs:179`
**Problem:** `Box::from_raw` called on potentially freed pointer
**Solution:**
```rust
// BEFORE (UNSAFE):
let data = Box::from_raw(ptr);

// AFTER (SAFE):
if ptr.is_null() {
    return Err(Error::NullPointer);
}
let data = Box::from_raw(ptr);
// Ensure pointer is not used after this point
```
**Tests:** Null pointer rejection, double-free prevention
**Time:** 2 hours

#### Task 1.2: Fix unsafe impl Send (7 types) (CWE-362)
**Files:** `voice/wake_word.rs:109`, `voice/vad.rs:125`, `voice/tts.rs:122,275`, `voice/stt.rs:104,280`, `voice/signal_processing.rs:83`
**Problem:** Raw C pointers unconditionally marked Send
**Solution:**
```rust
// BEFORE (UNSAFE):
unsafe impl Send for WakeWordEngine {}

// AFTER (SAFE):
// Remove unsafe impl Send
// Use Arc<Mutex<T>> for thread-safe sharing
// Or use crossbeam channels for communication
```
**Tests:** Thread safety tests
**Time:** 3 hours

#### Task 1.3: Validate from_raw_parts (CWE-119)
**Files:** `lib.rs:1465,1494,1674`, `tts.rs:171`
**Problem:** Slices created from unvalidated pointers
**Solution:**
```rust
// BEFORE (UNSAFE):
let slice = std::slice::from_raw_parts(ptr, len);

// AFTER (SAFE):
if ptr.is_null() {
    return Err(Error::NullPointer);
}
if len > MAX_SLICE_LEN {
    return Err(Error::LengthTooLarge);
}
if ptr as usize % std::mem::align_of::<u8>() != 0 {
    return Err(Error::UnalignedPointer);
}
let slice = std::slice::from_raw_parts(ptr, len);
```
**Tests:** Alignment tests, length validation tests
**Time:** 2 hours

#### Task 1.4: Fix Path Traversal (CWE-22)
**File:** `lib.rs:1331-1364`
**Problem:** Path passed directly to C's fopen() without sanitization
**Solution:**
```rust
// BEFORE (UNSAFE):
let path = CString::new(path_str)?;
let file = unsafe { libc::fopen(path.as_ptr(), mode.as_ptr()) };

// AFTER (SAFE):
use std::path::Path;
let path = Path::new(path_str);
if path.components().any(|c| c == std::path::Component::ParentDir) {
    return Err(Error::PathTraversal);
}
if !path.is_absolute() {
    return Err(Error::RelativePath);
}
// Canonicalize and validate
let canonical = path.canonicalize()?;
if !canonical.starts_with(ALLOWED_BASE_DIR) {
    return Err(Error::PathOutsideAllowedDir);
}
let path = CString::new(canonical.to_str().unwrap())?;
let file = unsafe { libc::fopen(path.as_ptr(), mode.as_ptr()) };
```
**Tests:** Path traversal rejection, canonicalization tests
**Time:** 2 hours

#### Task 1.5: Fix URL Injection (CWE-939)
**File:** `jni_bridge.rs:666-680`
**Problem:** URL passed directly to Intent without validation
**Solution:**
```rust
// BEFORE (UNSAFE):
let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[
    JValue::Object(&url_string)
])?;

// AFTER (SAFE):
// Validate URL scheme
if !url.starts_with("https://") {
    return Err(Error::InvalidUrlScheme);
}
// Validate against whitelist
let allowed_domains = ["api.aura.ai", "models.aura.ai"];
let domain = extract_domain(&url)?;
if !allowed_domains.contains(&domain.as_str()) {
    return Err(Error::DomainNotAllowed);
}
// Create intent with validated URL
let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[
    JValue::Object(&url_string)
])?;
```
**Tests:** URL scheme validation, domain whitelist tests
**Time:** 2 hours

**Total Security Time:** 11 hours

---

### 4.2 🏗️ ARCHITECTURE DEPARTMENT

**Role:** Transform to device-agnostic architecture
**Agent:** Architect
**Priority:** CRITICAL
**Dependencies:** None

#### Task 2.1: Create Environment Variable Contract System
**Files:** Create `platform/env_contract.rs`, `platform/path_resolver.rs`
**Time:** 4 hours

#### Task 2.2: Fix Hardcoded neocortex Path
**File:** `spawn.rs:34`
**Before:** `/data/local/tmp/aura-neocortex`
**After:** `std::env::var("AURA_NEOCORTEX_PATH").unwrap_or_else(|_| "/data/local/tmp/aura-neocortex".to_string())`
**Time:** 1 hour

#### Task 2.3: Fix Hardcoded Termux Fallback
**File:** `main.rs:88`
**Before:** `/data/data/com.termux/files/home`
**After:** `std::env::var("AURA_HOME").unwrap_or_else(|_| default_home())`
**Time:** 1 hour

#### Task 2.4: Fix Hardcoded Android DB Path
**File:** `config.rs:280`
**Before:** `/data/data/com.aura/databases/aura.db`
**After:** `std::env::var("AURA_DB_PATH").unwrap_or_else(|_| default_db_path())`
**Time:** 1 hour

#### Task 2.5: Fix Hardcoded Model Directory
**File:** `config.rs:178`
**Before:** `/data/local/tmp/aura/models`
**After:** `std::env::var("AURA_MODELS_PATH").unwrap_or_else(|_| default_models_path())`
**Time:** 1 hour

#### Task 2.6: Create Device Abstraction Layer
**Files:** Create `platform/device.rs`, `platform/android.rs`, `platform/linux.rs`
**Time:** 4 hours

#### Task 2.7: Add Configuration Abstraction
**Files:** Create `config/contract.rs`, update all config usage
**Time:** 3 hours

**Total Architecture Time:** 15 hours

---

### 4.3 🔧 BUILD SYSTEM DEPARTMENT

**Role:** Fix build system for all platforms
**Agent:** Tool Engineer
**Priority:** CRITICAL
**Dependencies:** None

#### Task 3.1: Standardize NDK Version
**File:** `.cargo/config.toml:10`
**Change:** `r27d` → `r26b`
**Time:** 30 minutes

#### Task 3.2: Remove Hardcoded Windows Path
**File:** `.cargo/config.toml:10`
**Change:** `C:/Android/ndk/android-ndk-r27d/...` → `$ANDROID_NDK_HOME/...`
**Time:** 30 minutes

#### Task 3.3: Standardize API Level
**File:** `config.toml`
**Change:** API 21 → API 26
**Time:** 30 minutes

#### Task 3.4: Fix curl-backend Confusion
**File:** `Cargo.toml`
**Change:** Remove `curl-backend`, use `reqwest` only
**Time:** 30 minutes

#### Task 3.5: Create Build Scripts
**Files:** Create `build.sh`, `build-all.sh`, `build-android.sh`
**Time:** 2 hours

#### Task 3.6: Add Build Verification
**Files:** Create `scripts/verify-build.sh`
**Time:** 1 hour

**Total Build System Time:** 5 hours

---

### 4.4 💎 CODE QUALITY DEPARTMENT

**Role:** Achieve production-grade code quality
**Agent:** Reviewer
**Priority:** HIGH
**Dependencies:** Security, Architecture

#### Task 4.1: Remove Duplicate Code
**File:** `telegram/mod.rs`
**Problem:** 200+ lines of duplicate code
**Solution:** Extract common functions, use traits
**Time:** 3 hours

#### Task 4.2: Fix Null Pointer Returns
**File:** `server_http_backend.rs:196`
**Problem:** Returns null pointer
**Solution:** Return `Result<T, Error>` instead
**Time:** 1 hour

#### Task 4.3: Implement Missing Functions
**File:** `server_http_backend.rs:205-216`
**Problem:** Empty tokenize/detokenize
**Solution:** Implement using neocortex client
**Time:** 2 hours

#### Task 4.4: Fix Token Handling Bugs
**File:** `server_http_backend.rs:228-253`
**Problem:** Token count doesn't match token list
**Solution:** Fix token counting logic
**Time:** 1 hour

#### Task 4.5: Replace Magic Numbers
**Files:** Multiple
**Problem:** Hardcoded numbers without explanation
**Solution:** Define named constants
**Time:** 2 hours

#### Task 4.6: Break Down Large Functions
**Files:** Multiple (250-500+ line functions)
**Problem:** Functions too large
**Solution:** Extract helper functions
**Time:** 3 hours

#### Task 4.7: Standardize Error Types
**Files:** Multiple
**Problem:** Inconsistent error handling
**Solution:** Create unified error types
**Time:** 2 hours

#### Task 4.8: Fix Compiler Warnings
**Files:** Multiple
**Problem:** 3 unused fields/imports
**Solution:** Remove or use
**Time:** 30 minutes

**Total Code Quality Time:** 14.5 hours

---

### 4.5 🚀 DEPLOYMENT DEPARTMENT

**Role:** One-click installation process
**Agent:** Builder
**Priority:** HIGH
**Dependencies:** Build System, Architecture

#### Task 5.1: Remove Exposed Bot Token
**File:** `config.toml:45`
**Before:** `8764736044:AAEuSHrnfzvrEbp9txWFrgSeC6R_daT6304`
**After:** `$TELEGRAM_BOT_TOKEN`
**Time:** 30 minutes

#### Task 5.2: Add Checksum Verification
**Files:** Create `deploy/checksum.sh`
**Solution:** SHA256 verification before install
**Time:** 1 hour

#### Task 5.3: Create Rollback Mechanism
**Files:** Create `deploy/rollback.sh`
**Solution:** Version management with backup/restore
**Time:** 2 hours

#### Task 5.4: Standardize Versioning
**Files:** `install.sh`, `config.toml`
**Solution:** Single source of truth for versions
**Time:** 1 hour

#### Task 5.5: Add Deployment Logging
**Files:** Create `deploy/log.sh`
**Solution:** Structured logging for all deployment operations
**Time:** 1 hour

#### Task 5.6: Add Health Checks
**Files:** Create `deploy/health-check.sh`
**Solution:** Post-deployment validation
**Time:** 1 hour

#### Task 5.7: Add Graceful Shutdown
**Files:** Update `daemon_core.rs`
**Solution:** Signal handling for clean shutdown
**Time:** 2 hours

**Total Deployment Time:** 8.5 hours

---

### 4.6 🧪 TESTING DEPARTMENT

**Role:** Full test coverage
**Agent:** TDD Guide
**Priority:** HIGH
**Dependencies:** Security, Architecture, Code Quality

#### Task 6.1: Security FFI Tests
**Files:** Create `tests/security/ffi_tests.rs`
**Tests:** Null pointer rejection, double-free prevention, alignment validation
**Time:** 3 hours

#### Task 6.2: Architecture Contract Tests
**Files:** Create `tests/architecture/env_contract_tests.rs`
**Tests:** Env var resolution, fallback chain, device abstraction
**Time:** 3 hours

#### Task 6.3: Build System Tests
**Files:** Create `tests/build/verification_tests.rs`
**Tests:** Build verification, artifact checksums
**Time:** 2 hours

#### Task 6.4: IPC Roundtrip Tests
**Files:** Create `tests/ipc/roundtrip_tests.rs`
**Tests:** Authenticated IPC, protocol versioning
**Time:** 2 hours

#### Task 6.5: HTTP Backend Tests
**Files:** Create `tests/http/backend_tests.rs`
**Tests:** Mock server, request/response validation
**Time:** 3 hours

#### Task 6.6: Voice Subsystem Tests
**Files:** Create `tests/voice/subsystem_tests.rs`
**Tests:** Wake word, VAD, TTS, STT
**Time:** 3 hours

#### Task 6.7: Memory Subsystem Tests
**Files:** Create `tests/memory/subsystem_tests.rs`
**Tests:** Vector search, WAL-mode SQLite
**Time:** 2 hours

#### Task 6.8: Integration Tests
**Files:** Create `tests/integration/daemon_tests.rs`
**Tests:** Daemon ↔ neocortex communication
**Time:** 3 hours

**Total Testing Time:** 21 hours

---

### 4.7 📚 DOCUMENTATION DEPARTMENT

**Role:** Complete documentation
**Agent:** General
**Priority:** MEDIUM
**Dependencies:** All departments

#### Task 7.1: README.md
**Content:** Project overview, quick start, prerequisites
**Time:** 2 hours

#### Task 7.2: ARCHITECTURE.md
**Content:** System design, module relationships, design decisions
**Time:** 3 hours

#### Task 7.3: API.md
**Content:** All public APIs, examples, error codes
**Time:** 3 hours

#### Task 7.4: DEPLOYMENT.md
**Content:** Installation guide, configuration, troubleshooting
**Time:** 2 hours

#### Task 7.5: SECURITY.md
**Content:** Security model, threat analysis, best practices
**Time:** 2 hours

#### Task 7.6: TESTING.md
**Content:** Test strategy, running tests, coverage reports
**Time:** 1 hour

#### Task 7.7: CONTRIBUTING.md
**Content:** Contribution guidelines, code style, PR process
**Time:** 1 hour

#### Task 7.8: CHANGELOG.md
**Content:** Version history, breaking changes
**Time:** 1 hour

**Total Documentation Time:** 15 hours

---

### 4.8 ⚙️ DEVOPS DEPARTMENT

**Role:** CI/CD pipeline
**Agent:** MCP Engineer
**Priority:** HIGH
**Dependencies:** Build System, Testing

#### Task 8.1: Create GitHub Actions Workflow
**File:** `.github/workflows/aura-ci.yml`
**Content:** Lint, build, test, security, package, deploy
**Time:** 3 hours

#### Task 8.2: Add Clippy + Rustfmt Checks
**File:** `.github/workflows/lint.yml`
**Content:** Automated code style enforcement
**Time:** 1 hour

#### Task 8.3: Add Cargo Audit
**File:** `.github/workflows/security.yml`
**Content:** Dependency vulnerability scanning
**Time:** 1 hour

#### Task 8.4: Add Multi-Target Build Matrix
**File:** `.github/workflows/build.yml`
**Content:** aarch64, armv7, x86_64 targets
**Time:** 2 hours

#### Task 8.5: Add APK Generation
**File:** `.github/workflows/package.yml`
**Content:** Automated APK creation
**Time:** 2 hours

#### Task 8.6: Add Deployment Automation
**File:** `.github/workflows/deploy.yml`
**Content:** Automated deployment to devices
**Time:** 2 hours

#### Task 8.7: Add Rollback Mechanism
**File:** `.github/workflows/rollback.yml`
**Content:** Automated rollback on failure
**Time:** 1 hour

**Total DevOps Time:** 12 hours

---

### 4.9 🏢 INFRASTRUCTURE DEPARTMENT

**Role:** Scaling and monitoring
**Agent:** Architect
**Priority:** MEDIUM
**Dependencies:** DevOps, Deployment

#### Task 9.1: Health Monitoring System
**Files:** Create `health/monitor.rs`
**Content:** CPU, memory, battery, temperature monitoring
**Time:** 3 hours

#### Task 9.2: Performance Benchmarking
**Files:** Create `benchmarks/`
**Content:** Latency, throughput, resource usage benchmarks
**Time:** 3 hours

#### Task 9.3: Resource Scaling
**Files:** Create `platform/scaler.rs`
**Content:** Adaptive resource allocation based on device capabilities
**Time:** 3 hours

#### Task 9.4: Telemetry System
**Files:** Update `telemetry/`
**Content:** Usage metrics, error tracking, performance data
**Time:** 2 hours

**Total Infrastructure Time:** 11 hours

---

### 4.10 🔬 RESEARCH DEPARTMENT

**Role:** Best practices and continuous improvement
**Agent:** Researcher
**Priority:** LOW
**Dependencies:** None

#### Task 10.1: Android Best Practices Research
**Content:** Android 14+ compatibility, new APIs, security updates
**Time:** 4 hours

#### Task 10.2: Rust on Android Research
**Content:** Latest rustc support, NDK integration, optimization
**Time:** 4 hours

#### Task 10.3: Security Best Practices
**Content:** Latest CVEs, security patterns, threat models
**Time:** 4 hours

#### Task 10.4: Performance Optimization
**Content:** Rust optimization techniques, Android profiling
**Time:** 4 hours

**Total Research Time:** 16 hours

---

### 4.11 🎨 DESIGN DEPARTMENT

**Role:** User experience
**Agent:** General
**Priority:** LOW
**Dependencies:** Architecture

#### Task 11.1: Configuration UX
**Files:** Update config interface
**Content:** User-friendly configuration management
**Time:** 3 hours

#### Task 11.2: Error Messages
**Files:** Update error handling
**Content:** Clear, actionable error messages
**Time:** 2 hours

#### Task 11.3: Logging UX
**Files:** Update logging system
**Content:** Structured, searchable logs
**Time:** 2 hours

**Total Design Time:** 7 hours

---

### 4.12 ✅ REVIEW DEPARTMENT

**Role:** Code inspection and quality gates
**Agent:** Reviewer
**Priority:** HIGH
**Dependencies:** All departments

#### Task 12.1: Security Code Review
**Files:** All security fixes
**Content:** Verify all vulnerabilities fixed
**Time:** 3 hours

#### Task 12.2: Architecture Review
**Files:** All architecture changes
**Content:** Verify device-agnostic design
**Time:** 3 hours

#### Task 12.3: Build System Review
**Files:** All build changes
**Content:** Verify cross-platform compatibility
**Time:** 2 hours

#### Task 12.4: Code Quality Review
**Files:** All code quality improvements
**Content:** Verify production-grade code
**Time:** 3 hours

#### Task 12.5: Testing Review
**Files:** All tests
**Content:** Verify test coverage and quality
**Time:** 2 hours

#### Task 12.6: Final Integration Review
**Files:** Complete codebase
**Content:** End-to-end verification
**Time:** 4 hours

**Total Review Time:** 17 hours

---

## 5. SUCCESS METRICS

### 5.1 Quantitative Metrics

| Metric | Current | Target | Measurement |
|--------|---------|--------|-------------|
| Critical vulnerabilities | 5 | 0 | cargo-audit |
| Hardcoded paths | 12+ | 0 | grep scan |
| Test coverage | ~10% | 80%+ | cargo-tarpaulin |
| Compiler warnings | 3 | 0 | cargo-clippy |
| Build time | Unknown | <10 min | CI metrics |
| Deployment time | Manual | <5 min | Deploy script |
| Documentation | Partial | Complete | Doc coverage |

### 5.2 Qualitative Metrics

| Aspect | Current State | Target State |
|--------|---------------|--------------|
| Security | 5 critical CVEs | Zero vulnerabilities |
| Architecture | Device-specific | Device-agnostic |
| Build System | Manual, broken | Automated, reliable |
| Code Quality | Prototype-grade | Production-grade |
| Deployment | Manual, risky | One-click, safe |
| Testing | Minimal | Comprehensive |
| Documentation | Sparse | Complete |

### 5.3 Production Readiness Checklist

**Security:**
- [ ] All FFI boundaries validated
- [ ] All paths sanitized against traversal
- [ ] All URLs validated against whitelist
- [ ] No use-after-free vulnerabilities
- [ ] No unsafe Send implementations

**Architecture:**
- [ ] No hardcoded device paths
- [ ] Environment variable contracts implemented
- [ ] Device abstraction layer complete
- [ ] Configuration system abstracted

**Build System:**
- [ ] NDK standardized to r26b
- [ ] API level standardized to 26
- [ ] No hardcoded paths in config
- [ ] Build scripts for all targets

**Code Quality:**
- [ ] No duplicate code
- [ ] No null pointer returns
- [ ] All functions implemented
- [ ] No magic numbers
- [ ] Functions < 100 lines
- [ ] Consistent error types
- [ ] Zero compiler warnings

**Deployment:**
- [ ] No exposed secrets
- [ ] Checksum verification
- [ ] Rollback mechanism
- [ ] Health checks
- [ ] Deployment logging

**Testing:**
- [ ] Security tests pass
- [ ] Architecture tests pass
- [ ] Integration tests pass
- [ ] 80%+ coverage

**Documentation:**
- [ ] README complete
- [ ] Architecture documented
- [ ] API documented
- [ ] Deployment guide complete

---

## 6. RISK MITIGATION

### 6.1 Technical Risks

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Breaking changes in security fixes | Medium | High | TDD approach, extensive testing |
| NDK compatibility issues | Low | Medium | Standardized on r26b, tested in CI |
| Test coverage targets not met | Medium | Medium | Focus on critical paths first |
| Performance regression | Low | Medium | Benchmark before/after changes |
| Android version incompatibility | Low | High | Test on multiple API levels |

### 6.2 Process Risks

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Scope creep | Medium | High | Strict phase boundaries |
| Resource constraints | Low | Medium | Parallel execution where possible |
| Dependency delays | Low | Medium | Clear dependency mapping |
| Quality gate failures | Medium | Medium | Incremental verification |

### 6.3 Contingency Plans

**If Phase 0 extends beyond 24 hours:**
- Focus only on security fixes (highest priority)
- Defer architecture and build system to Phase 1

**If test coverage target not met:**
- Prioritize security and critical path tests
- Accept 60% coverage as minimum viable

**If CI/CD pipeline fails:**
- Manual deployment process as fallback
- Document all manual steps

**If device testing impossible:**
- Use Android emulator for testing
- Document known emulator limitations

---

## 7. IMMEDIATE NEXT STEPS

### Hour 0-1: Setup and Kickoff
1. ✅ This coordination plan is complete
2. Initialize all 12 department worktrees
3. Create memory entities for tracking progress
4. Assign agents to departments

### Hour 1-8: Phase 0 Security Fixes (Parallel)
1. Security agent starts JNI Use-After-Free fix
2. Architecture agent starts env contract system
3. Build system agent starts NDK standardization

### Hour 8-16: Phase 0 Continued
1. Security agent fixes unsafe impl Send
2. Architecture agent fixes hardcoded paths
3. Build system agent creates build scripts

### Hour 16-24: Phase 0 Completion
1. Security agent completes all 5 fixes
2. Architecture agent completes all 4 fixes
3. Build system agent completes all 4 fixes
4. Review agent verifies all Phase 0 work

### Day 2-7: Phase 1 Architecture Transformation
1. Create device abstraction layer
2. Create configuration abstraction
3. Update all hardcoded paths
4. Add comprehensive tests

### Week 2: Phase 2 Build System & Deployment
1. Standardize build system
2. Create CI/CD pipeline
3. Create deployment system
4. Test deployment process

### Week 3: Phase 3 Testing & Code Quality
1. Implement all test suites
2. Fix all code quality issues
3. Achieve 80%+ coverage
4. Pass all quality gates

### Week 4: Phase 4 Documentation & Review
1. Complete all documentation
2. Final code review
3. Security audit
4. Production readiness assessment

---

## APPENDIX A: AGENT ASSIGNMENTS

| Agent | Department | Phase | Tasks |
|-------|------------|-------|-------|
| 🛡️ Security | Security | 0 | 5 critical vulnerabilities |
| 🏗️ Architect | Architecture | 0, 1 | Device-agnostic transformation |
| 🔧 Tool Engineer | Build System | 0, 2 | Build standardization |
| ✅ Reviewer | Code Quality, Review | 3, 4 | Code inspection |
| 🔨 Builder | Deployment | 2 | One-click installation |
| 🧪 TDD Guide | Testing | 3 | Full test coverage |
| 🤖 General | Documentation | 4 | Complete docs |
| 🔗 MCP Engineer | DevOps | 2 | CI/CD pipeline |
| 🔬 Researcher | Research | All | Best practices |

---

## APPENDIX B: FILE REFERENCE INDEX

### Security Files
- `lib.rs:179` — JNI Use-After-Free
- `voice/wake_word.rs:109` — unsafe impl Send
- `voice/vad.rs:125` — unsafe impl Send
- `voice/tts.rs:122,275` — unsafe impl Send
- `voice/stt.rs:104,280` — unsafe impl Send
- `voice/signal_processing.rs:83` — unsafe impl Send
- `lib.rs:1465,1494,1674` — from_raw_parts validation
- `tts.rs:171` — from_raw_parts validation
- `lib.rs:1331-1364` — Path Traversal
- `jni_bridge.rs:666-680` — URL Injection

### Architecture Files
- `spawn.rs:34` — Hardcoded neocortex path
- `main.rs:88` — Hardcoded Termux fallback
- `config.rs:280` — Hardcoded Android DB path
- `config.rs:178` — Hardcoded model directory

### Build System Files
- `.cargo/config.toml:10` — NDK path and version
- `Cargo.toml` — curl-backend confusion

### Deployment Files
- `config.toml:45` — Exposed bot token

### Code Quality Files
- `telegram/mod.rs` — 200+ lines duplicate code
- `server_http_backend.rs:196` — Null pointer returns
- `server_http_backend.rs:205-216` — Empty functions
- `server_http_backend.rs:228-253` — Token handling bugs

---

## APPENDIX C: CONTRACT DEFINITIONS

### Security Contract
```rust
// All FFI boundaries must:
// 1. Validate pointers non-null
// 2. Validate alignment
// 3. Validate length bounds
// 4. Prevent use-after-free
// 5. Prevent double-free
```

### Architecture Contract
```rust
// All configuration must come from:
// 1. Environment variables (primary)
// 2. User config file (secondary)
// 3. Platform defaults (tertiary)
// 4. Hardcoded defaults (last resort)
```

### Build Contract
```rust
// Build system must:
// 1. Use NDK r26b
// 2. Target API 26
// 3. Use reqwest for HTTP
// 4. Use env vars for paths
// 5. Support all target architectures
```

### Testing Contract
```rust
// All code must have:
// 1. Unit tests for logic
// 2. Integration tests for APIs
// 3. Security tests for FFI
// 4. 80%+ coverage
// 5. All tests passing
```

---

**Document Status:** ✅ COMPLETE
**Next Action:** Begin Phase 0 execution
**Orchestrator:** 🎯 General (Enterprise Coordinator)
**Last Updated:** 2026-04-02
