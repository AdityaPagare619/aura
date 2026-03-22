# AURA v4 Device Matrix

**Document**: `docs/validation/DEVICE-MATRIX.md`  
**Version**: 4.0.0-stable  
**Date**: 2026-03-22  
**Status**: ACTIVE  
**Owner**: QA Validation Charter

---

## Overview

This document defines the required device matrix for AURA v4 validation. Per CONTRACT.md Section 7, minimum test coverage requires **3 device configurations**: Minimum, Standard, and Extended.

---

## Device Configuration Matrix

### Required Test Configurations

| Config | OS Version | RAM | Storage | Vendor | Purpose |
|--------|-----------|-----|---------|--------|---------|
| **MINIMUM** | API 24 (Android 7.0) | 3 GB | 16 GB | Budget OEM | Contract floor |
| **STANDARD** | API 30 (Android 11) | 6 GB | 64 GB | Mid-range | Typical user |
| **EXTENDED** | API 35 (Android 15) | 8+ GB | 128+ GB | Flagship | Latest platform |

### Currently Validated Devices

| Device | Config | OS | RAM | Status | Last Tested |
|--------|--------|-----|-----|--------|-------------|
| Moto G45 5G | EXTENDED | API 35 | 8 GB | PASS | 2026-03-22 |
| - | MINIMUM | API 24 | 3 GB | MISSING | - |
| - | STANDARD | API 30 | 6 GB | MISSING | - |

---

## Test Case Specification

### TC-01: Version Command

```
Command:  ./aura-daemon --version
Expected: Exit 0, version string displayed
Validates: Binary loads correctly
```

### TC-02: Boot Stages Complete

```
Command:  ./aura-daemon (background)
Expected: All 5 stages PASS in boot.log
Validates: Platform contract satisfied
```

### TC-03: Telegram Connection

```
Command:  Send /start to bot via Telegram
Expected: Bot responds with greeting
Validates: MTProto session functional
```

### TC-04: Memory Contract

```
Command:  Monitor RSS during idle
Expected: RSS < 512 MB (normal), < 1 GB (with model)
Validates: F005-MEMORY_EXCEEDED not triggered
```

### TC-05: No SIGSEGV

```
Command:  Check logs after 1 hour operation
Expected: No signal 11, no F001
Validates: ABI contract satisfied (no glibc/bionic mismatch)
```

### TC-06: Contract Violation Display

```
Test:      Run on unsupported device (< API 24)
Expected:  Clear error message in CONTRACT format
Validates: F002-OS_VERSION handled correctly
```

### TC-07: Crash Report Generation

```
Test:      Simulate crash (kill -11)
Expected:  JSON report in ~/.aura/crash-reports/
Validates: F011-OBSERVABILITY requirements met
```

### TC-08: Privacy Verification

```
Command:  Monitor network connections
Expected: Only api.telegram.org (HTTPS)
Validates: Zero telemetry, F003 privacy contract
```

---

## Test Execution Matrix

| Device | TC-01 | TC-02 | TC-03 | TC-04 | TC-05 | TC-06 | TC-07 | TC-08 |
|--------|-------|-------|-------|-------|-------|-------|-------|-------|
| Moto G45 5G (EXTENDED) | PASS | PASS | PASS | PASS | PASS | N/A | PASS | PASS |
| MINIMUM Config | - | - | - | - | - | - | - | - |
| STANDARD Config | - | - | - | - | - | - | - | - |

---

## Coverage Gaps

### Critical Gaps

1. **MINIMUM Configuration** - No device available with API 24 + 3 GB RAM
   - Most devices with API 24 have been updated
   - Recommendation: Use Android emulator with API 24 + 3 GB

2. **STANDARD Configuration** - No mid-range device validated
   - Needed for typical user experience verification
   - Recommendation: Acquire or rent test device (e.g., Pixel 5a)

---

## CI/CD Integration

Every release must execute device tests before claiming validation:

```bash
# Required before release
./verify.sh --device-matrix

# Must pass all 8 TCs on at least 1 device
# MINIMUM and STANDARD gaps must be documented
```

---

## Related Documents

| Document | Purpose |
|----------|---------|
| `docs/build/CONTRACT.md` | Platform contract specification |
| `docs/DEVICE-TESTING-METHODOLOGY.md` | Testing procedures |
| `docs/build/FAILURE_TAXONOMY.md` | Failure classification |

---

**END OF DOCUMENT**
