# AURA Multi-Agent Framework Design Document

**Version:** 1.0  
**Status:** Design Specification  
**Date:** 2026-03-21  
**Target:** AURA v4 Engineering Team

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Agent Architecture Overview](#2-agent-architecture-overview)
3. [Agent Specifications](#3-agent-specifications)
   - [AGENT 1: CODE-AUDITOR](#agent-1-code-auditor)
   - [AGENT 2: DEVICE-TESTER](#agent-2-device-tester)
   - [AGENT 3: CI-VALIDATOR](#agent-3-ci-validator)
   - [AGENT 4: INSTALL-AUDITOR](#agent-4-install-auditor)
   - [AGENT 5: RELEASE-MANAGER](#agent-5-release-manager)
   - [AGENT 6: SAFETY-REVIEWER](#agent-6-safety-reviewer)
   - [AGENT 7: SECURITY-REVIEWER](#agent-7-security-reviewer)
   - [AGENT 8: REGRESSION-TRACKER](#agent-8-regression-tracker)
4. [COORDINATOR Agent](#4-coordinator-agent)
5. [Agent Communication Protocol](#5-agent-communication-protocol)
6. [Trigger Conditions](#6-trigger-conditions)
7. [Implementation Guidance](#7-implementation-guidance)
8. [Multi-Agent Workflow Examples](#8-multi-agent-workflow-examples)

---

## 1. Executive Summary

### Purpose

The AURA Multi-Agent Framework is a sophisticated autonomous system designed to enable continuous code quality assurance, device testing, CI/CD validation, and release management for the AURA v4 project. This framework transforms AURA from a reactive development model into a proactive, self-auditing system that continuously monitors its own health, security, and compliance posture.

### Scope

This framework governs the following operational domains:

| Domain | Description |
|--------|-------------|
| **Code Auditing** | Automated review of code changes for quality, regressions, and pattern violations |
| **Device Testing** | Real-device validation on Android hardware via BrowserStack infrastructure |
| **CI Validation** | Pipeline integrity verification including artifact chains, SBOM, and SLSA provenance |
| **Release Management** | End-to-end release orchestration from version consistency to binary verification |
| **Safety Compliance** | Iron Law enforcement, ethics layer auditing, GDPR compliance verification |
| **Security Posture** | Vulnerability scanning, binary hardening verification, supply chain security |
| **Regression Prevention** | Failure taxonomy tracking to prevent recurrence of known issues |

### Strategic Value

The multi-agent architecture delivers the following strategic outcomes:

1. **Continuous Assurance**: Shift from periodic manual audits to continuous autonomous monitoring
2. **Rapid Feedback**: Sub-minute detection of regressions, security vulnerabilities, and policy violations
3. **Consistent Quality**: Standardized auditing across all code paths regardless of time of day or reviewer availability
4. **Reduced Friction**: Automated release processes eliminate manual steps and human error
5. **Knowledge Retention**: Failure taxonomy captures institutional knowledge of past issues and their resolutions

---

## 2. Agent Architecture Overview

### Hub-and-Spoke Model

The AURA Multi-Agent Framework employs a **hub-and-spoke** architecture where a central COORDINATOR agent manages communication with specialized specialist agents. This model provides:

- **Single Point of Contact**: Users interact only with COORDINATOR, not individual agents
- **Context Aggregation**: COORDINATOR synthesizes findings from multiple agents into unified reports
- **Resource Management**: COORDINATOR manages token budgets and stops agents approaching 70% context
- **Decision Authority**: COORDINATOR knows when to escalate to human decision-makers

### Agent Roles Matrix

| Agent | Role | Priority | Trigger Count |
|-------|------|----------|----------------|
| COORDINATOR | Orchestrator | — | All requests |
| CODE-AUDITOR | Code Quality Analyst | HIGH | 4 triggers |
| DEVICE-TESTER | Device Validation | CRITICAL | 3 triggers |
| CI-VALIDATOR | Pipeline Auditor | HIGH | 2 triggers |
| INSTALL-AUDITOR | Script Quality Analyst | HIGH | 3 triggers |
| RELEASE-MANAGER | Release Orchestrator | HIGH | 3 triggers |
| SAFETY-REVIEWER | Compliance Auditor | HIGH | 2 triggers |
| SECURITY-REVIEWER | Security Analyst | HIGH | 4 triggers |
| REGRESSION-TRACKER | Knowledge Base Manager | MEDIUM | 3 triggers |

### Agent Component Structure

Each specialist agent shares a common structure:

```
┌─────────────────────────────────────────┐
│            Specialist Agent            │
├─────────────────────────────────────────┤
│ Purpose: [Agent's mission statement]   │
├─────────────────────────────────────────┤
│ Triggers:                               │
│   - [Trigger condition 1]              │
│   - [Trigger condition 2]              │
│   - [Trigger condition 3]              │
├─────────────────────────────────────────┤
│ Tasks:                                  │
│   1. [Task description]                │
│   2. [Task description]                │
│   3. [Task description]                │
├─────────────────────────────────────────┤
│ Tools Available:                        │
│   - [Tool 1]                            │
│   - [Tool 2]                            │
│   - [Tool 3]                            │
├─────────────────────────────────────────┤
│ Output Format: [JSON schema]           │
├─────────────────────────────────────────┤
│ Priority: [HIGH|MEDIUM|CRITICAL]       │
└─────────────────────────────────────────┘
```

### Communication Flow

```
                    ┌──────────────┐
                    │   USER       │
                    │   REQUEST    │
                    └──────┬───────┘
                           │
                           ▼
                    ┌──────────────┐
                    │ COORDINATOR   │
                    │ (Hub)         │
                    └──────┬───────┘
                           │
            ┌──────────────┼──────────────┐
            │              │              │
            ▼              ▼              ▼
      ┌─────────┐   ┌─────────┐   ┌─────────┐
      │ Agent 1 │   │ Agent 2 │   │ Agent N │
      └────┬────┘   └────┬────┘   └────┬────┘
           │             │             │
           ▼             ▼             │
      ┌─────────────────────────────────┤
      │      AGGREGATED RESPONSE        │
      │      (Unified Report)           │
      └─────────────────────────────────┘
```

---

## 3. Agent Specifications

### AGENT 1: CODE-AUDITOR

**Purpose:** Continuously analyze codebase for issues, patterns, and quality regression

**Priority:** HIGH

#### Triggers

| Trigger | Source | Condition |
|----------|--------|-----------|
| New PR | GitHub webhook | Any PR opened or updated |
| New commit to main | GitHub webhook | Any push to main branch |
| User request | Slash command | `/audit` or `/code-review` |
| Scheduled | Cron | Daily at 02:00 UTC |

#### Tasks

1. **Review code changes for breaking patterns**
   - Analyze git diff between target and base
   - Identify patterns that could cause runtime failures
   - Check for anti-patterns in Rust code (unwrap, expect, unsafe usage)

2. **Check for regressions against failure taxonomy**
   - Cross-reference changes against GAP-P1 through GAP-P7 categories
   - Flag changes that could trigger known failure patterns
   - Query REGRESSION-TRACKER for historical context

3. **Validate JNI type signatures against jni 0.21 API**
   - Parse all JNI function calls
   - Verify signature compatibility with jni 0.21
   - Report deprecated API usage

4. **Check abstract socket imports**
   - Verify `target_os = "android"` is used, not `"linux"`
   - Audit socket-related code for platform-specific issues
   - Check bionic-specific APIs

5. **Verify llama.cpp C/C++ compilation split**
   - Ensure .c files are compiled with clang
   - Ensure .rs files are compiled with rustc
   - Verify build.rs configuration

6. **Check bincode v2 API usage**
   - Verify bincode 2.x API is used (not v1)
   - Check for deprecated serialization methods
   - Validate version compatibility

7. **Audit Rust version compatibility**
   - Check rust-toolchain.toml matches CI
   - Verify MSRV compliance
   - Flag features requiring newer Rust versions

8. **Review dependency changes for known issues**
   - Parse Cargo.toml diff
   - Run `cargo audit` for CVE warnings
   - Check RUSTSEC advisories

#### Tools

| Tool | Purpose |
|------|---------|
| `git diff` | Analyze code changes |
| `grep` | Pattern matching |
| `cargo audit` | Vulnerability scanning |
| `cargo tree` | Dependency tree analysis |
| `rustc --version` | Verify compiler version |

#### Output Format

```json
{
  "agent": "CODE-AUDITOR",
  "timestamp": "2026-03-21T12:00:00Z",
  "trigger": "new_pr",
  "commits_reviewed": ["abc123", "def456"],
  "findings": [
    {
      "severity": "critical|high|medium|low",
      "category": "regression|security|compatibility|style",
      "file": "src/path/file.rs",
      "line": 42,
      "issue": "Description of the issue",
      "fix_recommendation": "Recommended fix"
    }
  ],
  "regression_risk": "low|medium|high",
  "overall_health": "green|yellow|red",
  "taxonomy_matches": ["GAP-P2"],
  "next_steps": ["Action item 1", "Action item 2"]
}
```

---

### AGENT 2: DEVICE-TESTER

**Purpose:** Execute tests on real Android device (Pixel 7, Termux) to validate binary functionality

**Priority:** CRITICAL

#### Triggers

| Trigger | Source | Condition |
|----------|--------|-----------|
| New binary release | GitHub webhook | Release published |
| User request | Slash command | `/test-device` or `/device-test` |
| Post-merge validation | GitHub webhook | PR merged to main |

#### Tasks

1. **Pull latest main branch**
   - Clone or fetch latest AURA repository
   - Verify branch state

2. **Execute build environment setup**
   ```bash
   pkg update && pkg install rust git clang make openssl -y
   ```

3. **Execute release build**
   ```bash
   cargo build --release 2>&1 | tee build.log
   ```
   - Capture exit codes
   - Capture all compilation errors
   - Capture all panic messages

4. **Execute binary validation** (if build succeeds)
   ```bash
   ./target/release/aura-daemon --version
   ```
   - Capture version output
   - Verify binary exists

5. **Execute test suite**
   ```bash
   cargo test --workspace 2>&1 | tee test.log
   ```
   - Capture all test results
   - Parse pass/fail counts

6. **Execute Telegram E2E tests** (if applicable)
   - Test Telegram bot integration
   - Verify message handling

7. **Parse output for pass/fail criteria**
   - Extract exit codes
   - Identify failure patterns
   - Generate structured test report

#### Tools

| Tool | Purpose |
|------|---------|
| BrowserStack MCP | Device session management |
| `takeAppScreenshot` | Visual verification |
| `runAppLiveSession` | Interactive device testing |
| Termux shell commands | Execute build and tests |
| Output parsing | Extract structured results |

#### Output Format

```json
{
  "agent": "DEVICE-TESTER",
  "timestamp": "2026-03-21T12:00:00Z",
  "device": "Pixel 7 (Android 14)",
  "build_result": {
    "status": "success|failure",
    "exit_code": 0,
    "duration_seconds": 1200,
    "errors": ["Error message 1"],
    "warnings": ["Warning message 1"]
  },
  "binary_validation": {
    "version_output": "aura-daemon 4.0.0",
    "path": "/data/data/com.termux/files/home/aura/target/release/aura-daemon",
    "exists": true
  },
  "test_results": {
    "total": 42,
    "passed": 40,
    "failed": 2,
    "skipped": 0,
    "duration_seconds": 300,
    "failures": [
      {
        "test": "test_name",
        "module": "module::path",
        "error": "assertion failed"
      }
    ]
  },
  "telegram_e2e": {
    "status": "not_run|success|partial|failure",
    "messages_sent": 10,
    "messages_received": 10
  },
  "overall_status": "pass|fail",
  "raw_output": "Full command output for debugging"
}
```

#### Partnership Model

The DEVICE-TESTER reports **raw output** to COORDINATOR. COORDINATOR is responsible for:
- Interpreting exit codes
- Determining pass/fail criteria
- Synthesizing findings with other agents' results

---

### AGENT 3: CI-VALIDATOR

**Purpose:** Validate CI/CD pipeline integrity on every run

**Priority:** HIGH

#### Triggers

| Trigger | Source | Condition |
|----------|--------|-----------|
| Every CI run | Workflow dispatch | Any workflow triggered |
| New workflow file | GitHub webhook | .github/workflows/*.yml changed |

#### Tasks

1. **Parse workflow YAML for correctness**
   - Validate YAML syntax
   - Check `on-push` redundancy with `on-push: branches: [main]`
   - Verify job dependencies

2. **Verify artifact chain**
   - Source → Build → Sign → Upload → Release
   - Ensure each stage produces required artifacts
   - Validate artifact naming conventions

3. **Check SBOM generation**
   - Verify sbom-generator runs in release workflow
   - Validate SPDX JSON output
   - Check SBOM includes all dependencies

4. **Validate SLSA provenance attestation**
   - Verify provenance predicate generation
   - Check builder identity
   - Validate attestation format

5. **Verify NDK version in build matrix**
   - Check NDK version matches requirements
   - Verify platform versions (android-24, etc.)
   - Validate toolchain versions

6. **Check cross-compilation targets**
   - Verify aarch64-linux-android target
   - Check arm-linux-androideabi target
   - Validate x86_64-linux-android for emulator

7. **Validate release tag matching Cargo.toml**
   - Parse version from Cargo.toml
   - Compare with git tag
   - Verify semantic versioning

#### Tools

| Tool | Purpose |
|------|---------|
| gh CLI | GitHub API interaction |
| git | Version control operations |
| YAML parsing | Workflow validation |
| jq | JSON/YAML processing |

#### Output Format

```json
{
  "agent": "CI-VALIDATOR",
  "timestamp": "2026-03-21T12:00:00Z",
  "workflow_file": ".github/workflows/release.yml",
  "validation_results": {
    "yaml_syntax": "valid|invalid",
    "on_push_redundancy": "detected|clean",
    "artifact_chain": {
      "source": "present|missing",
      "build": "present|missing",
      "sign": "present|missing",
      "upload": "present|missing",
      "release": "present|missing"
    },
    "sbom_generation": "enabled|disabled|broken",
    "slsa_provenance": "enabled|disabled|broken",
    "ndk_version": "r26b|expected:r26b",
    "cross_compilation_targets": ["aarch64-linux-android"],
    "version_match": true
  },
  "findings": [
    {
      "severity": "critical|high|medium|low",
      "category": "configuration|artifact|security",
      "issue": "Description",
      "location": "workflow file, line N",
      "recommendation": "Fix recommendation"
    }
  ],
  "overall_health": "green|yellow|red"
}
```

---

### AGENT 4: INSTALL-AUDITOR

**Purpose:** Continuously audit installation scripts for correctness

**Priority:** HIGH

#### Triggers

| Trigger | Source | Condition |
|----------|--------|-----------|
| Changes to install.sh | GitHub webhook | install.sh modified |
| Changes to verify.sh | GitHub webhook | verify.sh modified |
| New version release | GitHub webhook | New tag created |

#### Tasks

1. **Compare install.sh version tags against Cargo.toml version**
   - Parse version from Cargo.toml
   - Extract version from install.sh
   - Report mismatches

2. **Detect dropped fixes**
   - Compare old install.sh vs new install.sh
   - Identify removed workarounds or patches
   - Flag potential regressions

3. **Validate package names**
   - Verify `openssl` not `openssl-dev` for Termux
   - Check all package names against Termux package repository
   - Verify package version availability

4. **Check model URLs**
   - Validate GGUF format in URLs
   - Verify URL accessibility
   - Check model size and format specifications

5. **Verify checksum fields**
   - Ensure checksums are populated (not placeholder like `TODO`)
   - Verify SHA256 checksum format
   - Cross-reference with actual file

6. **Audit phase ordering**
   - Verify dependencies install before usage
   - Check error handling in each phase
   - Validate cleanup on failure

7. **Compare verify.sh checks against Rust code config**
   - Parse config field names from Rust
   - Compare with verify.sh field checks
   - Report missing validations

#### Tools

| Tool | Purpose |
|------|---------|
| git diff | Script comparison |
| grep | Pattern matching |
| diff | File difference analysis |
| curl | URL validation |

#### Output Format

```json
{
  "agent": "INSTALL-AUDITOR",
  "timestamp": "2026-03-21T12:00:00Z",
  "files_audited": ["install.sh", "verify.sh"],
  "version_comparison": {
    "cargo_toml": "4.0.0",
    "install_sh": "4.0.0",
    "match": true
  },
  "findings": [
    {
      "severity": "critical|high|medium|low",
      "file": "install.sh",
      "line": 42,
      "issue": "Description",
      "recommendation": "Fix"
    }
  ],
  "dropped_fixes": [
    {
      "description": "Fix description",
      "removed_in": "commit hash",
      "impact": "Low|Medium|High"
    }
  ],
  "overall_health": "green|yellow|red"
}
```

---

### AGENT 5: RELEASE-MANAGER

**Purpose:** Orchestrate the complete release lifecycle

**Priority:** HIGH

#### Triggers

| Trigger | Source | Condition |
|----------|--------|-----------|
| Commit to main with version bump | GitHub webhook | Cargo.toml version changed |
| Manual trigger | Slash command | `/release` or `/trigger-release` |
| Scheduled | Cron | Weekly on Monday 08:00 UTC |

#### Tasks

1. **Check version consistency**
   - Parse Cargo.toml version
   - Compare with install.sh version tag
   - Compare with workflow files
   - Report inconsistencies

2. **Trigger GitHub Actions release workflow**
   - Use gh CLI to dispatch workflow
   - Pass correct version tag
   - Monitor trigger confirmation

3. **Monitor build status**
   - Poll workflow run status
   - Capture build duration
   - Report failures immediately

4. **Verify binary SHA256 after build**
   - Download release artifact
   - Compute SHA256 hash
   - Compare against expected value in release notes

5. **Verify ELF properties**
   - Use readelf to check:
     - NX (non-executable stack)
     - PIE (position independent executable)
     - Static linking (if applicable)
     - Stack canaries
     - RELRO (partial/full)

6. **Create/update GitHub release**
   - Generate release notes from changelog
   - Upload all artifacts
   - Set correct tag
   - Publish release

7. **Trigger DEVICE-TESTER for post-release validation**
   - Invoke DEVICE-TESTER agent
   - Pass release version
   - Wait for results

8. **Update documentation if needed**
   - Check for version-specific docs
   - Update README.md if necessary
   - Regenerate API documentation

#### Tools

| Tool | Purpose |
|------|---------|
| gh CLI | GitHub Actions API |
| GitHub Actions API | Workflow control |
| sha256sum | Binary verification |
| readelf | ELF inspection |

#### Output Format

```json
{
  "agent": "RELEASE-MANAGER",
  "timestamp": "2026-03-21T12:00:00Z",
  "release_version": "4.0.0",
  "version_consistency": {
    "cargo_toml": "4.0.0",
    "install_sh": "4.0.0",
    "workflow": "4.0.0",
    "all_match": true
  },
  "workflow_trigger": {
    "status": "triggered|failed",
    "run_id": "123456789",
    "url": "https://github.com/..."
  },
  "binary_verification": {
    "sha256_expected": "abc123...",
    "sha256_actual": "abc123...",
    "match": true
  },
  "elf_properties": {
    "nx_enabled": true,
    "pie_enabled": true,
    "stack_canaries": true,
    "relro": "full"
  },
  "release_published": true,
  "release_url": "https://github.com/...",
  "device_test_triggered": true,
  "overall_status": "success|failed"
}
```

---

### AGENT 6: SAFETY-REVIEWER

**Purpose:** Audit ethics layer, Iron Laws, policy gates from safety perspective

**Priority:** HIGH

#### Triggers

| Trigger | Source | Condition |
|----------|--------|-----------|
| Changes to aura-iron-laws | GitHub webhook | Iron Laws crate modified |
| Changes to aura-types | GitHub webhook | Policy structs modified |
| Any Iron Law touch | GitHub webhook | Any file in iron-laws/ modified |

#### Tasks

1. **Verify 7 Iron Laws are present and enforced**
   - List all Iron Law files
   - Verify each law has implementation
   - Check enforcement mechanism exists

2. **Check Iron Law #7 (Context Window Integrity)**
   - Search for `TODO` in context-window related code
   - Verify implementation completeness
   - Report any incomplete implementations

3. **Audit non-bypassable Audit verdicts**
   - Identify all audit verdict enforcement points
   - Verify they cannot be bypassed
   - Test bypass attempts

4. **Review GDPR consent granularity**
   - Verify 6 consent categories:
     - Personal data processing
     - Model inference usage
     - Conversation history storage
     - Analytics sharing
     - Third-party data sharing
     - Marketing communications

5. **Check right-to-erasure implementation**
   - Verify data deletion endpoints exist
   - Check cascade deletion behavior
   - Test erasure on all data types

6. **Validate anti-sycophancy measures**
   - Review value alignment code
   - Verify user preference handling
   - Check for reward hacking prevention

7. **Test policy gate enforcement**
   - Verify policy gates block prohibited actions
   - Test edge cases
   - Verify gate logging

#### Tools

| Tool | Purpose |
|------|---------|
| grep | Pattern matching |
| cargo check | Compilation validation |
| code review | Manual review |
| test execution | Policy enforcement testing |

#### Output Format

```json
{
  "agent": "SAFETY-REVIEWER",
  "timestamp": "2026-03-21T12:00:00Z",
  "iron_laws_status": {
    "total_laws": 7,
    "implemented": 7,
    "enforced": 7,
    "incomplete": ["IL-7 if any"]
  },
  "iron_law_audit": [
    {
      "law": "IL-1: Harm Prevention",
      "status": "implemented|enforced|imcomplete",
      "implementation_details": "...",
      "enforcement_mechanism": "..."
    }
  ],
  "gdpr_compliance": {
    "consent_categories": 6,
    "consent_implemented": 6,
    "right_to_erasure": "implemented|incomplete|missing",
    "data_categories_covered": ["personal", "inference", "history"]
  },
  "anti_sycophancy": {
    "status": "implemented|incomplete|missing",
    "value_alignment": "verified|concerns",
    "preference_handling": "verified|concerns"
  },
  "findings": [
    {
      "severity": "critical|high|medium|low",
      "issue": "Description",
      "law_or_policy": "IL-N or GDPR article",
      "recommendation": "Fix"
    }
  ],
  "overall_health": "green|yellow|red"
}
```

---

### AGENT 7: SECURITY-REVIEWER

**Purpose:** Audit security posture of binary, dependencies, and build pipeline

**Priority:** HIGH

#### Triggers

| Trigger | Source | Condition |
|----------|--------|-----------|
| New dependency | GitHub webhook | Cargo.toml changed |
| New release | GitHub webhook | Release published |
| New workflow | GitHub webhook | .github/workflows/ changed |
| User request | Slash command | `/security` or `/security-audit` |

#### Tasks

1. **Run cargo audit for known vulnerabilities**
   - Execute `cargo audit`
   - Parse JSON output
   - Report CVEs and RUSTSEC advisories

2. **Check RUSTSEC advisories against dependencies**
   - Query RUSTSEC database
   - Compare with Cargo.lock
   - Flag affected versions

3. **Verify binary hardening**
   - Use readelf to check:
     - NX (non-executable)
     - PIE (position independent)
     - Stack canaries
     - RELRO (partial/full)

4. **Audit TLS configuration**
   - Check reqwest configuration
   - Verify rustls usage
   - Verify certificate validation

5. **Check for credential leakage**
   - Search for hardcoded secrets
   - Check .env files (should be gitignored)
   - Verify credential handling

6. **Verify rustls-platform-verifier on Android**
   - Check initialization
   - Verify Android certificate store access
   - Test TLS on Android

7. **Review JNI bridge for memory safety**
   - Check for memory leaks
   - Verify JNI reference management
   - Audit native code for vulnerabilities

8. **Validate supply chain**
   - Check NDK provenance
   - Verify download sources
   - Audit build reproducibility

#### Tools

| Tool | Purpose |
|------|---------|
| cargo audit | Vulnerability scanning |
| readelf | Binary inspection |
| grep | Secret scanning |
| security scanning | Pattern matching |

#### Output Format

```json
{
  "agent": "SECURITY-REVIEWER",
  "timestamp": "2026-03-21T12:00:00Z",
  "dependency_vulnerabilities": {
    "cargo_audit_run": true,
    "vulnerabilities_found": 0,
    "advisories": []
  },
  "binary_hardening": {
    "nx_enabled": true,
    "pie_enabled": true,
    "stack_canaries": true,
    "relro": "full",
    "score": "A"
  },
  "tls_configuration": {
    "rustls_used": true,
    "certificate_validation": "enabled",
    "android_verification": "verified"
  },
  "credential_leakage": {
    "scan_completed": true,
    "secrets_found": 0,
    "files_checked": 100
  },
  "jni_safety": {
    "memory_leaks": 0,
    "reference_management": "correct",
    "vulnerabilities": 0
  },
  "supply_chain": {
    "ndk_version": "r26b",
    "provenance_verified": true,
    "build_reproducible": true
  },
  "findings": [
    {
      "severity": "critical|high|medium|low",
      "cve_id": "RUSTSEC-XXXX-XXXX",
      "description": "...",
      "affected_package": "package",
      "recommendation": "Fix"
    }
  ],
  "overall_health": "green|yellow|red"
}
```

---

### AGENT 8: REGRESSION-TRACKER

**Purpose:** Prevent same bugs from appearing twice using failure taxonomy

**Priority:** MEDIUM

#### Triggers

| Trigger | Source | Condition |
|----------|--------|-----------|
| Bug report | GitHub issue | Issue labeled "bug" |
| Any crash | GitHub webhook | Issue with "crash" or "panic" |
| Device panic | DEVICE-TESTER output | Panic in test output |

#### Tasks

1. **Classify failure into taxonomy categories**
   - GAP-P1: SIGSEGV, panic at startup
   - GAP-P2: Linker errors, missing dependencies
   - GAP-P3: Platform-specific (NDK, bionic, Termux)
   - GAP-P4: Logic errors (wrong output, bypassed policy)
   - GAP-P5: Performance (OOM, startup time)
   - GAP-P6: Network/TLS issues
   - GAP-P7: Configuration/user input

2. **Check if failure matches known pattern**
   - Query taxonomy database
   - Search similar symptoms
   - Compare error messages

3. **If known: return previous fix + prevention rules**
   - Return fix commit hash
   - Return prevention rule
   - Return frequency count

4. **If new: add to taxonomy**
   - Create new taxonomy entry
   - Document symptoms
   - Document fix

5. **Track failure frequency per category**
   - Update frequency counters
   - Report trending categories
   - Alert on increasing failures

#### Tools

| Tool | Purpose |
|------|---------|
| git log | Historical search |
| grep | Pattern matching |
| pattern matching | Taxonomy lookup |
| database queries | Taxonomy storage |

#### Output Format

```json
{
  "agent": "REGRESSION-TRACKER",
  "timestamp": "2026-03-21T12:00:00Z",
  "failure_classification": {
    "taxonomy_category": "GAP-P2",
    "subcategory": "linker_missing_symbol",
    "confidence": "high|medium|low"
  },
  "match_found": true,
  "previous_occurrence": {
    "date": "2026-02-15",
    "commit": "abc123",
    "fix_commit": "def456",
    "fix_description": "Added missing symbol export"
  },
  "prevention_rules": [
    "Ensure all symbols are exported in build.rs",
    "Add linker flag validation in CI"
  ],
  "frequency_tracking": {
    "GAP-P1": 0,
    "GAP-P2": 3,
    "GAP-P3": 1,
    "GAP-P4": 0,
    "GAP-P5": 0,
    "GAP-P6": 0,
    "GAP-P7": 0
  },
  "new_entry_created": false,
  "overall_status": "known_pattern|novel_pattern"
}
```

---

## 4. COORDINATOR Agent

The COORDINATOR is the central hub that orchestrates all specialist agents.

### Responsibilities

| Responsibility | Description |
|----------------|-------------|
| **Request Reception** | Receive user requests via slash commands or webhooks |
| **Agent Selection** | Decide which specialist agents to invoke based on triggers |
| **Context Management** | Monitor token usage, stop agents at 70% context |
| **Output Synthesis** | Aggregate findings from multiple agents |
| **Decision Authority** | Know when to stop and ask for human input |

### Agent Selection Logic

```python
def select_agents(trigger_context):
    agents = []
    
    if trigger_context.type == "new_pr":
        agents = ["CODE-AUDITOR", "CI-VALIDATOR"]
    
    elif trigger_context.type == "new_release":
        agents = ["RELEASE-MANAGER", "DEVICE-TESTER", "SAFETY-REVIEWER"]
    
    elif trigger_context.type == "bug_report":
        agents = ["REGRESSION-TRACKER", "CODE-AUDITOR"]
    
    elif trigger_context.type == "install_script_change":
        agents = ["INSTALL-AUDITOR", "CODE-AUDITOR"]
    
    elif trigger_context.type == "manual":
        agents = ["ALL"]
    
    return agents
```

### Context Budget Management

The COORDINATOR implements strict context management:

1. **Monitor**: Track context usage for each agent
2. **Threshold**: At 70% context, instruct agent to wrap up
3. **Fallback**: If agent exceeds 80%, terminate and request human intervention
4. **Checkpoint**: Save partial results before termination

### Output Synthesis

COORDINATOR produces unified reports:

```json
{
  "coordinator_summary": {
    "request_type": "new_pr",
    "agents_invoked": ["CODE-AUDITOR", "CI-VALIDATOR"],
    "timestamp": "2026-03-21T12:00:00Z",
    "duration_seconds": 300
  },
  "findings_aggregation": {
    "total_findings": 5,
    "critical": 1,
    "high": 2,
    "medium": 1,
    "low": 1
  },
  "agent_results": [
    {
      "agent": "CODE-AUDITOR",
      "status": "completed",
      "findings_count": 3,
      "regression_risk": "medium"
    },
    {
      "agent": "CI-VALIDATOR",
      "status": "completed",
      "findings_count": 2,
      "overall_health": "green"
    }
  ],
  "recommendations": [
    "Fix critical issue in src/main.rs before merge",
    "CI pipeline is healthy"
  ],
  "human_intervention_needed": false,
  "human_decision_required": null
}
```

### When to Request Human Decision

The COORDINATOR escalates to human when:

- Agent fails with unresolvable error
- Context limit reached without completion
- Conflicting findings from agents
- Security-critical issues detected
- Policy violation that requires human judgment

---

## 5. Agent Communication Protocol

### Structured JSON Output

All agents MUST return structured JSON with the following required fields:

| Field | Type | Description |
|-------|------|-------------|
| `agent` | string | Agent identifier |
| `timestamp` | ISO8601 | Execution timestamp |
| `status` | enum | `completed`, `failed`, `partial` |

### Success Response

```json
{
  "agent": "AGENT-NAME",
  "timestamp": "2026-03-21T12:00:00Z",
  "status": "completed",
  "findings_count": 5,
  "severity_breakdown": {
    "critical": 1,
    "high": 2,
    "medium": 1,
    "low": 1
  },
  "next_steps": [
    "Fix critical issue in file X",
    "Verify fix with test"
  ],
  "artifacts": ["path/to/output"]
}
```

### Failure Response

```json
{
  "agent": "AGENT-NAME",
  "timestamp": "2026-03-21T12:00:00Z",
  "status": "failed",
  "error": "Error description",
  "diagnostics": "Root cause analysis",
  "attempted_fallback": ["Action 1", "Action 2"],
  "suggested_fix": "Recommended fix"
}
```

### COORDINATOR Aggregation

The COORDINATOR combines agent outputs:

1. **Collect** all agent responses
2. **Merge** findings by severity
3. **Resolve** conflicting recommendations
4. **Synthesize** unified next steps
5. **Present** consolidated report

---

## 6. Trigger Conditions

### Trigger Source Matrix

| Trigger Type | Source | Detection Mechanism |
|--------------|--------|---------------------|
| git hook | GitHub Webhook | `push` event, branch filter |
| workflow dispatch | GitHub Actions | `workflow_dispatch` event |
| scheduled | Cron | Scheduled job |
| manual | Slash command | User invocation |
| issue | GitHub Issue | Issue created/updated |

### Priority Ordering

When multiple agents could run simultaneously:

1. **CRITICAL**: DEVICE-TESTER on new release
2. **HIGH**: CODE-AUDITOR, CI-VALIDATOR, SECURITY-REVIEWER
3. **MEDIUM**: INSTALL-AUDITOR, REGRESSION-TRACKER
4. **LOW**: RELEASE-MANAGER (scheduled)

### Trigger Configuration

```yaml
# Example GitHub webhook configuration
triggers:
  code_auditor:
    - type: pull_request
      action: [opened, synchronize]
    - type: push
      branch: main
    - type: schedule
      cron: "0 2 * * *"  # Daily 02:00 UTC
    - type: command
      slash: /audit

  device_tester:
    - type: release
      action: published
    - type: command
      slash: /test-device
    - type: pull_request
      action: merged

  ci_validator:
    - type: workflow_run
      status: completed
    - type: push
      path: .github/workflows/*.yml
```

---

## 7. Implementation Guidance

### Slash Commands

| Command | Invokes | Description |
|---------|---------|-------------|
| `/audit` | CODE-AUDITOR | Run code audit on current state |
| `/test-device` | DEVICE-TESTER | Trigger device testing |
| `/ci-validate` | CI-VALIDATOR | Validate CI pipeline |
| `/audit-install` | INSTALL-AUDITOR | Audit installation scripts |
| `/release` | RELEASE-MANAGER | Trigger release process |
| `/safety-review` | SAFETY-REVIEWER | Run safety audit |
| `/security` | SECURITY-REVIEWER | Run security audit |
| `/regression-check` | REGRESSION-TRACKER | Check failure taxonomy |
| `/full-audit` | All agents | Run complete system audit |

### Skill Requirements

| Agent | Required Skills |
|-------|-----------------|
| CODE-AUDITOR | `systematic-debugging`, `code-quality-comprehensive-check` |
| DEVICE-TESTER | `test-driven-development`, `verification-before-completion` |
| CI-VALIDATOR | `infrastructure-as-code` |
| INSTALL-AUDITOR | `context-aware-implementation` |
| RELEASE-MANAGER | `executing-plans` |
| SAFETY-REVIEWER | `ethical-reasoning`, `code-quality-comprehensive-check` |
| SECURITY-REVIEWER | `security-audit`, `verification-before-completion` |
| REGRESSION-TRACKER | `complex-problem-decomposition` |

### Token Budget Management

| Phase | Budget | Action |
|-------|--------|--------|
| Start | 0% | Load relevant skills |
| Progress | 50% | Mid-checkpoint if needed |
| Warning | 70% | Begin wrap-up |
| Limit | 80% | Terminate, save state |
| Emergency | >80% | `/compact`, restore from checkpoint |

### Context Handoff Protocol

When COORDINATOR passes context to another agent:

```json
{
  "handoff_summary": {
    "from_agent": "CODE-AUDITOR",
    "to_agent": "DEVICE-TESTER",
    "key_findings": [
      "Build succeeded on main",
      "Found 2 potential regressions",
      "Tests pass locally"
    ],
    "state": "Ready for device validation",
    "next_steps": ["Run on device", "Verify regressions"]
  }
}
```

---

## 8. Multi-Agent Workflow Examples

### Example 1: New PR Merged

**Trigger:** PR merged to main

**Workflow:**
```
PR Merged
    │
    ▼
CODE-AUDITOR ──► CI-VALIDATOR
    │              │
    │    (parallel execution)
    │              │
    ▼              ▼
COORDINATOR ◄──────────────
    │
    ▼
    │
DEVICE-TESTER ──► REGRESSION-TRACKER
    │
    ▼
FINAL REPORT
```

**Agents Invoked:** CODE-AUDITOR, CI-VALIDATOR → DEVICE-TESTER

**Expected Outcome:**
- Code quality verified
- CI pipeline validated
- Binary tested on real device
- Regression check completed

---

### Example 2: New Release

**Trigger:** Version bump in Cargo.toml merged to main

**Workflow:**
```
Version Bump
    │
    ▼
RELEASE-MANAGER
    │
    ├─► Verify version consistency
    ├─► Trigger GitHub Actions
    │
    ▼ (after build)
DEVICE-TESTER
    │
    ├─► Build on device
    ├─► Run tests
    │
    ▼ (after tests)
SAFETY-REVIEWER
    │
    ▼
FINAL REPORT + RELEASE PUBLISHED
```

**Agents Invoked:** RELEASE-MANAGER → DEVICE-TESTER → SAFETY-REVIEWER

**Expected Outcome:**
- Release published
- Binary verified on device
- Safety compliance confirmed

---

### Example 3: Bug Reported

**Trigger:** GitHub issue labeled "bug"

**Workflow:**
```
Bug Report
    │
    ▼
REGRESSION-TRACKER
    │
    ├─► Classify failure
    ├─► Check known patterns
    │
    ▼ (if new pattern)
CODE-AUDITOR
    │
    ├─► Analyze code changes
    ├─► Identify root cause
    │
    ▼ (optional)
DEVICE-TESTER
    │
    ▼
FINAL REPORT + FIX RECOMMENDATION
```

**Agents Invoked:** REGRESSION-TRACKER → (if needed) CODE-AUDITOR → (if needed) DEVICE-TESTER

**Expected Outcome:**
- Failure classified
- Known fix applied or new fix identified

---

### Example 4: Installation Script Changed

**Trigger:** install.sh or verify.sh modified

**Workflow:**
```
Script Changed
    │
    ▼
INSTALL-AUDITOR
    │
    ├─► Version comparison
    ├─► Package validation
    ├─► Checksum verification
    │
    ▼ (if issues found)
CODE-AUDITOR
    │
    ├─► Verify code changes are safe
    │
    ▼
FINAL REPORT
```

**Agents Invoked:** INSTALL-AUDITOR → (if issues) CODE-AUDITOR

**Expected Outcome:**
- Installation script validated
- Code impact assessed
- Release approved or blocked

---

### Example 5: Scheduled Full Audit

**Trigger:** Daily cron (02:00 UTC)

**Workflow:**
```
Scheduled Trigger
    │
    ▼
CODE-AUDITOR ──► CI-VALIDATOR ──► INSTALL-AUDITOR
    │              │                 │
    │         (parallel execution)  │
    │              │                 │
    ▼              ▼                 ▼
          COORDINATOR
    │
    ▼
SECURITY-REVIEWER ──► SAFETY-REVIEWER
    │
    ▼
FULL SYSTEM REPORT
```

**Agents Invoked:** CODE-AUDITOR, CI-VALIDATOR, INSTALL-AUDITOR → SECURITY-REVIEWER, SAFETY-REVIEWER

**Expected Outcome:**
- Complete system health snapshot
- Security posture assessment
- Compliance verification

---

## Appendix A: Agent Summary Table

| Agent | Purpose | Priority | Primary Triggers |
|-------|---------|----------|------------------|
| COORDINATOR | Orchestration | — | All |
| CODE-AUDITOR | Code quality | HIGH | PR, commit, manual, scheduled |
| DEVICE-TESTER | Device validation | CRITICAL | Release, manual, post-merge |
| CI-VALIDATOR | Pipeline integrity | HIGH | CI run, workflow change |
| INSTALL-AUDITOR | Script quality | HIGH | Script change, release |
| RELEASE-MANAGER | Release orchestration | HIGH | Version bump, manual, scheduled |
| SAFETY-REVIEWER | Compliance | HIGH | Iron Laws change, policy change |
| SECURITY-REVIEWER | Security posture | HIGH | Dependency, release, workflow, manual |
| REGRESSION-TRACKER | Knowledge management | MEDIUM | Bug report, crash, panic |

---

## Appendix B: Severity Definitions

| Severity | Definition | Response Time |
|----------|------------|---------------|
| CRITICAL | Immediate impact on functionality or security | < 1 hour |
| HIGH | Significant issue requiring attention | < 24 hours |
| MEDIUM | Issue affecting quality but not critical | < 7 days |
| LOW | Minor issue or improvement suggestion | < 30 days |

---

## Appendix C: Health Status Definitions

| Status | Definition |
|--------|------------|
| GREEN | No critical/high issues, all checks passing |
| YELLOW | Medium issues present, no critical issues |
| RED | Critical issues present, immediate action required |

---

*Document Version: 1.0*  
*Last Updated: 2026-03-21*  
*Maintainer: AURA Engineering Team*