# Regression Test Commands

**Document**: `docs/validation/REGRESSION-TESTS.md`  
**Purpose**: Command reference for pre-release regression testing  
**Status**: MANDATORY — Run before any release  
**Created**: 2026-03-22  
**Owner**: QA Validation Charter

---

## Pre-Release Checklist

Before ANY release, execute ALL commands in this document in order.

**CI green is NOT sufficient. Device execution is REQUIRED.**

---

## Build Verification

### B-001: Clean Build

```bash
# Clean previous builds
cargo clean

# Build all crates with all features
cargo build --all-features --target aarch64-linux-android
```

**Expected**: Exit code 0, no warnings

---

### B-002: Workspace Build

```bash
# Build entire workspace
cargo build --workspace --target aarch64-linux-android
```

**Expected**: All crates build successfully

---

## Artifact Inspection

### I-001: Architecture Verification

```bash
# Verify correct architecture
file aura-daemon

# Expected output:
# aura-daemon: ELF 64-bit LSB executable, ARM aarch64, EABI5 version 1 (SYSV), dynamically linked
#
# NOT expected:
# aura-daemon: ELF 64-bit LSB executable, x86-64 (glibc)  ← WRONG
# aura-daemon: ELF 32-bit LSB ARM  ← WRONG
```

**F-Code**: F001, F005

---

### I-002: Dynamic Linker Verification

```bash
# Check dynamic linker (MUST be bionic, NOT glibc)
readelf -d aura-daemon | grep NEEDED

# Expected output (bionic):
# libc.so
# libm.so
# libdl.so
# ld-android.so (or linker64)
#
# NOT expected (glibc):
# libstdc++.so
# libgcc_s.so
# ld-linux-aarch64.so  ← glibc, WRONG
```

**F-Code**: F001, F004

---

### I-003: ELF Header Verification

```bash
# Check ELF class and machine type
readelf -h aura-daemon

# Expected:
# Machine: AArch64
# Class: ELF64
#
# NOT expected:
# Machine: Advanced Micro Devices X86-64  ← WRONG
```

**F-Code**: F005

---

### I-004: PIE Verification

```bash
# Check Position Independent Execution
readelf -h aura-daemon | grep Type

# Expected:
# Type: DYN (Shared object file)  ← PIE enabled
#
# NOT expected:
# Type: EXEC (Executable file)  ← No PIE, less secure
```

**F-Code**: F001 prevention

---

## Unit Testing

### T-001: Workspace Tests

```bash
# Run all workspace tests
cargo test --workspace

# Expected: All tests pass
# If fails: Check F003, F006
```

**F-Code**: F003, F006

---

### T-002: Clippy Lint

```bash
# Run clippy with all features
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Expected: No warnings, no errors
```

**F-Code**: F003, F006

---

### T-003: Security Audit

```bash
# Check for vulnerabilities
cargo audit

# Expected: No vulnerabilities found
```

---

### T-004: Formatting Check

```bash
# Check code formatting
cargo fmt -- --check

# Expected: No diff
```

---

## Device Testing (MANDATORY)

### D-001: Deploy to Device

```bash
# SCP binary to device
scp aura-daemon user@device:/tmp/

# SSH to device and set permissions
ssh user@device 'chmod +x /tmp/aura-daemon'
```

---

### D-002: Execute on Device

```bash
# Execute version check
ssh user@device '/tmp/aura-daemon --version'

# Check exit code
# Expected: 0
# If non-zero: F001, F005, F007
```

**F-Code**: F001, F005, F007

---

### D-003: Full Boot Test

```bash
# Run daemon and check boot stages
ssh user@device '/tmp/aura-daemon 2>&1 &'
sleep 10

# Check boot logs
ssh user@device 'cat /sdcard/AURA/logs/aura-boot-*.log'

# Verify 5 stages logged:
# 1. INIT
# 2. ENV_CHECK
# 3. DEP_CHECK
# 4. RUNTIME_START
# 5. READY
```

**F-Code**: F007, F010, F011

---

### D-004: Memory Test

```bash
# Check RSS memory
ssh user@device 'cat /proc/$(pgrep aura-daemon)/status | grep VmRSS'

# Expected: < 512 MB (normal), < 1 GB (with model)
# If > 1 GB: F005-MEMORY_EXCEEDED
```

**F-Code**: F005-MEMORY

---

### D-005: Telegram Connection Test

```bash
# Check Telegram connection in logs
ssh user@device 'grep -i "telegram\|long.poll" /sdcard/AURA/logs/aura-boot-*.log'

# Expected: Connected to api.telegram.org, long-poll started
```

**F-Code**: F010

---

## Contract Verification

### C-001: Boot Log Format

```bash
# Verify JSON format matches CONTRACT.md Section 6.3
ssh user@device 'cat /sdcard/AURA/logs/aura-boot-*.log | python3 -m json.tool'

# Expected: Valid JSON with stages array, overall_status
```

---

### C-002: Contract Violation Test

```bash
# Create scenario with contract violation
# (e.g., remove Termux dependency)

# Execute and check error format
ssh user@device '/tmp/aura-daemon 2>&1'

# Expected: Error message per CONTRACT.md Section 6.2
```

**F-Code**: F011

---

## Full Regression Suite

### One-Line Full Regression

```bash
# Run complete regression (non-interactive)
cargo clean && \
  cargo build --all-features --target aarch64-linux-android && \
  file aura-daemon | grep -q "ARM aarch64" && \
  readelf -d aura-daemon | grep -q "ld-android" && \
  cargo test --workspace && \
  cargo clippy --workspace --all-targets --all-features -- -D warnings && \
  scp aura-daemon user@device:/tmp/ && \
  ssh user@device 'chmod +x /tmp/aura-daemon && /tmp/aura-daemon --version && echo "EXIT: $?"'

# Expected: All commands succeed, final EXIT: 0
```

---

### Step-by-Step Regression

```bash
# Step 1: Build
echo "=== Step 1: BUILD ==="
cargo build --all-features --target aarch64-linux-android
echo "Exit: $?"

# Step 2: Inspect
echo "=== Step 2: INSPECT ==="
file aura-daemon
readelf -d aura-daemon | head -5

# Step 3: Test
echo "=== Step 3: TEST ==="
cargo test --workspace

# Step 4: Lint
echo "=== Step 4: LINT ==="
cargo clippy --workspace --all-targets -- -D warnings

# Step 5: Audit
echo "=== Step 5: AUDIT ==="
cargo audit || true

# Step 6: Deploy
echo "=== Step 6: DEPLOY ==="
scp aura-daemon user@device:/tmp/

# Step 7: Device Test
echo "=== Step 7: DEVICE ==="
ssh user@device 'chmod +x /tmp/aura-daemon && /tmp/aura-daemon --version'
echo "Exit: $?"

# Step 8: Boot Check
echo "=== Step 8: BOOT ==="
ssh user@device 'ls -la /sdcard/AURA/logs/aura-boot-*.log'

echo "=== REGRESSION COMPLETE ==="
```

---

## Quick Reference Card

| Command | Purpose | F-Codes |
|---------|---------|---------|
| `file aura-daemon` | Architecture check | F005 |
| `readelf -d aura-daemon \| grep NEEDED` | Linker check (bionic vs glibc) | F001, F004 |
| `readelf -h aura-daemon \| grep Machine` | ELF class check | F005 |
| `cargo test --workspace` | Unit tests | F003 |
| `cargo clippy --all-features` | Lint | F006 |
| `ssh device '/tmp/aura-daemon --version'` | Device execution | F001 |
| `ssh device 'cat /sdcard/AURA/logs/aura-boot-*.log'` | Boot stage check | F007, F011 |

---

## Failure Handling

### If Build Fails
```
Action: Check F003 (Dependency), F005 (Linker), F006 (Feature)
Fix: cargo update, verify NDK target, check feature flags
```

### If Inspection Fails
```
Action: Check F001 (ABI), F004 (ABI mismatch), F005 (Wrong ELF)
Fix: Use correct NDK sysroot, set target triple
```

### If Tests Fail
```
Action: Standard test failure
Fix: Fix code, not tests
```

### If Device Test Fails
```
Action: REAL failure - DO NOT IGNORE
Diagnosis: See DEVICE-TEST-PROCEDURE.md
F-Codes: F001, F007, F010
```

---

## CI Gate Integration

These commands map to CI Pipeline v2 stages:

```
STAGE 1: SOURCE     → cargo check
STAGE 2: BUILD      → cargo build --target aarch64-linux-android
STAGE 3: INSPECT    → file, readelf commands
STAGE 4: TEST       → cargo test, cargo clippy, cargo audit
STAGE 5: DEPLOY     → scp to device
STAGE 6: VALIDATE   → ssh device '/tmp/aura-daemon --version'
```

---

## Related Documents

| Document | Relationship |
|----------|-------------|
| `docs/validation/DEVICE-TEST-PROCEDURE.md` | Detailed device test steps |
| `docs/validation/DEVICE-MATRIX.md` | Device coverage tracking |
| `docs/build/CONTRACT.md` | Test requirements |
| `docs/build/FAILURE_TAXONOMY.md` | F-code reference |
| `docs/ci/AURA-CI-PIPELINE-V2-DESIGN.md` | CI pipeline stages |

---

**END OF REGRESSION TESTS**

*Run before every release. CI green is NOT release ready. Device test is the gate.*