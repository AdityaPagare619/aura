# AURA v4 Failure Taxonomy

## Overview

The AURA Failure Taxonomy ensures that every failure is:
1. **Classified** to its root domain
2. **Tracked** with a unique ID
3. **Prevented** by regression tests
4. **Never debugged twice**

The taxonomy is inspired by medical diagnosis systems (ICD-10) and aviation incident reporting. Each failure gets a unique ID (F001, F002, etc.) and is classified into 4 top-level domains.

## The 4 Failure Domains

### 1. NdkCompiler (NDK/Toolchain)
Failures in the Android NDK, Rust compiler, or build toolchain.

### 2. Platform (Environment)
Failures in the Android/Termux runtime environment — permissions, API levels, OS differences.

### 3. Memory (Resource Management)
Failures in memory allocation, leaks, or corruption — especially bionic allocator issues.

### 4. Logic (Code)
Failures in AURA's own code — wrong algorithms, schema mismatches, bypass vulnerabilities.

### 5. Inference (AI/ML)
Failures in the LLM inference pipeline — llama.cpp crashes, model loading, timeouts.

### 6. Network
Failures in network connectivity or API calls.

## The Taxonomy Registry

| ID | Name | Domain | Category | Status | Root Cause |
|----|------|--------|----------|--------|------------|
| F001 | SIGSEGV at startup with NDK r26b | NdkCompiler | SIGSEGV | FIXED | lto=true + panic=abort |
| F002 | Bionic allocator OOM | Memory | OOM | NEEDS_FIX | bionic malloc NULL |
| F003 | Termux permission denied | Platform | PERMISSION_DENIED | UNKNOWN | Investigating |
| F004 | Reflection schema mismatch | Logic | SCHEMA_MISMATCH | FIXED | prompts.rs wrong schema |
| F005 | Semantic similarity stub | Logic | SCHEMA_MISMATCH | FIXED | planner.rs stub 0.0 |
| F006 | Ethics audit bypass | Logic | ETHICS_BYPASS | FIXED | Downgrade at τ>0.6 |
| F007 | GDPR erasure incomplete | Logic | SCHEMA_MISMATCH | FIXED | user_profile only |
| F008 | LLama.cpp crash | Inference | LLAMA_CPP_CRASH | UNKNOWN | Investigating |

## Prevention Checklist

Before any PR merges, CI must verify:
- [ ] F001: Cargo.toml does not have lto=true + panic=abort
- [ ] F002: Memory usage stays under 512MiB in container
- [ ] F003: HOME directory is set, permissions verified
- [ ] F004: Reflection schema validated against grammar
- [ ] F005: Semantic similarity tests pass
- [ ] F006: Ethics bypass tests pass
- [ ] F007: GDPR integration test clears all 5 tiers
- [ ] F008: Model loads successfully in smoke test
