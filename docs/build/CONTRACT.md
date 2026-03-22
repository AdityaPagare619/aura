# AURA v4.0 Platform Contract Specification

**Document Version:** 1.0  
**Date:** 2026-03-22  
**Status:** ACTIVE — Binding on all engineering decisions  
**Owner:** AURA Founder (Human)  
**Approved:** [Signature Required]

---

## Preamble

This document is the FOUNDATIONAL CONTRACT for AURA v4.0. It defines the boundaries of what AURA supports, what it does NOT support, and what happens when those boundaries are violated.

From `ENTERPRISE-BASIC.txt` Principle P2: *"Infinite environments exist. We never code for all environments. We define contracts that environments must satisfy."*

This contract ENDS the infinite problem space. Everything outside this contract is NOT AURA's responsibility.

---

## Section 1: The Contract Law

### 1.1 Contract is Law

This document is BINDING on all engineering decisions:

- **MUST:** All code changes MUST respect this contract
- **MUST:** All CI/CD pipelines MUST verify contract compliance
- **MUST:** All device tests MUST test contract compliance
- **MUST:** All documentation MUST reference this contract
- **MUST:** All failure reports MUST indicate if contract was violated

### 1.2 Violation is NOT a Bug

When a device violates this contract:

| What Happens | What Does NOT Happen |
|-------------|---------------------|
| Clear error message displayed | Silent crash |
| Specific contract violation identified | Cryptic SIGSEGV |
| User guided to resolution | Java stack trace |
| Incident logged for taxonomy | Data sent to server |

### 1.3 Contract Versioning

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2026-03-22 | Initial contract |

Contract changes require:
1. Document proposed changes
2. Founder approval
3. Full regression test
4. Version bump

---

## Section 2: Supported Platform Contract

### 2.1 Operating System Contract

```
┌─────────────────────────────────────────────────────────────┐
│ REQUIRED: Android API Level                               │
├─────────────────────────────────────────────────────────────┤
│ Minimum:  API 24  (Android 7.0 "Nougat")                  │
│ Tested:   API 30  (Android 11)                            │
│ Latest:  API 35  (Android 15)                          │
│                                                             │
│ NOT SUPPORTED:                                             │
│ • API < 24  (Android 6.0 and below)                     │
│ • Custom ROMs with removed/modified system libraries      │
│ • LineageOS, Resurrection Remix, etc. (verify bionic)   │
│ • iOS, Linux, Windows, macOS (no current support)      │
└─────────────────────────────────────────────────────────────┘
```

**Verification:** Boot stage 2 (`ENV_CHECK`) reads `android.os.Build.VERSION.SDK_INT`

### 2.2 Architecture Contract

```
┌─────────────────────────────────────────────────────────────┐
│ REQUIRED: CPU Architecture                                 │
├─────────────────────────────────────────────────────────────┤
│ Supported:  arm64-v8a  (ARM 64-bit)                      │
│                                                             │
│ NOT SUPPORTED:                                             │
│ • ARMv7, armeabi-v7a  (32-bit ARM)                     │
│ • ARMv6, armeabi        (32-bit ARM legacy)              │
│ • x86, x86_64          (Intel/AMD — emulators only)     │
│ • RISC-V                (experimental, not supported)      │
│ • MIPS                  (deprecated, not supported)        │
└─────────────────────────────────────────────────────────────┘
```

**Verification:** Boot stage 2 reads `ro.product.cpu.abi` and `ro.product.cpu.abilist64`

### 2.3 Termux Contract

```
┌─────────────────────────────────────────────────────────────┐
│ REQUIRED: Termux Package Manager                           │
├─────────────────────────────────────────────────────────────┤
│ Required:  Termux installed from F-Droid or Google Play   │
│ Required:  Termux API >= 0.70                            │
│ Required:  `pkg` command available                         │
│                                                             │
│ NOT SUPPORTED:                                             │
│ • UserLand, Linux Deploy, AnLinux (different environments)│
│ • Chroot/Proot environments (bionic may be missing)      │
│ • Termux from unknown sources (APK mirrors)              │
│ • Termux:Boot (boot service — optional, recommended)     │
└─────────────────────────────────────────────────────────────┘
```

**Verification:** Boot stage 3 (`DEP_CHECK`) runs `pkg version` and verifies output format

### 2.4 System Library Contract

```
┌─────────────────────────────────────────────────────────────┐
│ REQUIRED: bionic libc                                     │
├─────────────────────────────────────────────────────────────┤
│ Required:  bionic (Android's C library)                   │
│ Required:  Android linker (linker64 or ld-android.so)    │
│ Required:  Android dynamic loader                        │
│                                                             │
│ NOT SUPPORTED:                                             │
│ • glibc (Linux C library)                                │
│ • musl (Alpine/musl-based systems)                     │
│ • uClibc (embedded systems)                              │
│ • Custom ROMs with replaced bionic                        │
└─────────────────────────────────────────────────────────────┘
```

**Verification:** Boot stage 2 checks `ro.runtime.first-boot` and library paths

### 2.5 Package Dependencies Contract

```
┌─────────────────────────────────────────────────────────────┐
│ REQUIRED: System Packages                                  │
├─────────────────────────────────────────────────────────────┤
│ curl        >= 7.68.0   (for HTTP backend)              │
│ git         >= 2.30.0    (for updates)                  │
│ bash        >= 5.0      (for scripts)                  │
│ openssl     >= 1.1.1    (for HTTPS via curl)           │
│                                                             │
│ INSTALLED VIA: pkg install curl git bash openssl         │
│                                                             │
│ NOT SUPPORTED:                                             │
│ • wget instead of curl (different API)                    │
│ • curl without OpenSSL (must have TLS support)           │
│ • Git LFS (not used by AURA)                           │
└─────────────────────────────────────────────────────────────┘
```

**Verification:** Boot stage 3 (`DEP_CHECK`) runs `curl --version`, `git --version`, `bash --version`

---

## Section 3: Hardware Contract

### 3.1 Memory Contract

```
┌─────────────────────────────────────────────────────────────┐
│ REQUIRED: RAM                                             │
├─────────────────────────────────────────────────────────────┤
│ Minimum:    3 GB RAM                                      │
│ Recommended: 6 GB RAM                                    │
│ Optimal:    8+ GB RAM                                    │
│                                                             │
│ REASON:                                                    │
│ • AURA daemon: ~100-200 MB resident                     │
│ • llama.cpp model: ~1-4 GB depending on model           │
│ • OS + Termux: ~1-2 GB reserved                       │
│ • Safety margin: ~500 MB                               │
│                                                             │
│ NOT SUPPORTED:                                             │
│ • Devices with < 3 GB RAM (Android reports available)    │
│ • Devices with < 1.5 GB usable (after OS)              │
└─────────────────────────────────────────────────────────────┘
```

**Verification:** Boot stage 2 reads `/proc/meminfo` and calculates `MemAvailable`

**Action on Violation:**
```
ERROR [ENV_CHECK]: Insufficient RAM
  Required:    3 GB minimum
  Available:   X GB
  Recommendation: Close background apps or use smaller model
```

### 3.2 Storage Contract

```
┌─────────────────────────────────────────────────────────────┐
│ REQUIRED: Storage Space                                  │
├─────────────────────────────────────────────────────────────┤
│ Minimum:    2 GB free storage                           │
│ Recommended: 5 GB free storage                          │
│                                                             │
│ BREAKDOWN:                                                 │
│ • AURA binary:       ~15 MB                            │
│ • Model file:         ~1-4 GB (depending on model)     │
│ • Memory DB:         ~100-500 MB                       │
│ • Logs:              ~50-200 MB                          │
│ • Safety margin:     ~500 MB                          │
└─────────────────────────────────────────────────────────────┘
```

**Verification:** Boot stage 2 runs `df -h $PREFIX` and checks `/data` partition

### 3.3 Network Contract

```
┌─────────────────────────────────────────────────────────────┐
│ BEHAVIOR: Network Access                                 │
├─────────────────────────────────────────────────────────────┤
│ Telegram API:    REQUIRED (HTTPS to api.telegram.org)   │
│ Model downloads:  OPTIONAL (user-initiated)             │
│ AURA updates:    OPTIONAL (user-initiated)              │
│ Telemetry:      PROHIBITED (zero telemetry by design)  │
│                                                             │
│ FIREWALL BEHAVIOR:                                       │
│ • Telegram blocked → Daemon exits with clear error    │
│ • No internet → Daemon continues (offline mode)        │
│ • Telemetry attempt → Logged as ethics violation       │
└─────────────────────────────────────────────────────────────┘
```

---

## Section 4: Runtime Behavior Contract

### 4.1 Startup Contract

```
┌─────────────────────────────────────────────────────────────┐
│ STARTUP: Expected Behavior                               │
├─────────────────────────────────────────────────────────────┤
│ Boot Stage 1 (INIT):      < 1 second                  │
│ Boot Stage 2 (ENV_CHECK): < 2 seconds                 │
│ Boot Stage 3 (DEP_CHECK): < 5 seconds                 │
│ Boot Stage 4 (STARTUP):   < 10 seconds                │
│ Boot Stage 5 (READY):    < 20 seconds TOTAL          │
│                                                             │
│ NOT COMPLIANT:                                             │
│ • Startup > 30 seconds → logged as F005-SLOW_STARTUP  │
│ • Any stage > expected time → logged with warning     │
└─────────────────────────────────────────────────────────────┘
```

**Verification:** All boot stages log their start time, end time, and duration

### 4.2 Memory Usage Contract

```
┌─────────────────────────────────────────────────────────────┐
│ RUNTIME: Expected Memory Usage                           │
├─────────────────────────────────────────────────────────────┤
│ AURA daemon RSS:      < 512 MB (normal)                │
│ AURA daemon RSS:      < 1 GB   (with model loaded)    │
│ Memory growth:        < 10 MB/hour (idle leak check)   │
│                                                             │
│ NOT COMPLIANT:                                             │
│ • RSS > 1 GB → logged as F005-MEMORY_EXCEEDED         │
│ • RSS growth > 100 MB/hour → logged as F005-MEMORY_LEAK│
└─────────────────────────────────────────────────────────────┘
```

**Verification:** Runtime monitor polls `/proc/self/status` every 60 seconds

### 4.3 Crash Behavior Contract

```
┌─────────────────────────────────────────────────────────────┐
│ CRASH: Required Behavior                                 │
├─────────────────────────────────────────────────────────────┤
│ • All crashes logged to:  ~/.aura/logs/boot.log        │
│ • All crashes classified to: F001-F015 taxonomy        │
│ • All crashes produce: crash report (JSON, no PII)      │
│ • No crashes produce: silent exit, no logs               │
│ • No crashes produce: Java exceptions, ANRs               │
│                                                             │
│ CRASH REPORT FORMAT:                                     │
│ • version: AURA version                                  │
│ • timestamp: ISO8601 UTC                                 │
│ • failure_class: F001-F015                              │
│ • stack_trace_hash: SHA256 of stack trace              │
│ • boot_stage: INIT|ENV_CHECK|DEP_CHECK|RUNTIME|READY │
│ • os_version, architecture, device_model, ram_mb       │
└─────────────────────────────────────────────────────────────┘
```

---

## Section 5: Out-of-Contract Devices

### 5.1 Known Out-of-Contract Devices

| Device Type | Reason | Behavior |
|-------------|--------|---------|
| Android < API 24 | Old Android versions | Not supported, won't install |
| Custom ROMs (LineageOS, etc.) | May lack bionic or have modified linker | May work, not guaranteed |
| 32-bit ARM devices | No arm64 support | Won't install, clear error |
| Devices < 3GB RAM | Insufficient memory | Clear error at startup |
| iOS | Wrong OS | Won't install, different platform |
| Linux/Windows/macOS | Wrong OS | Won't install, different platform |
| Termux alternatives (UserLand, etc.) | Different environment | Not supported |

### 5.2 Custom ROM Policy

```
┌─────────────────────────────────────────────────────────────┐
│ CUSTOM ROM SUPPORT POLICY                                │
├─────────────────────────────────────────────────────────────┤
│ AURA is designed for STOCK Android + Termux.             │
│                                                             │
│ Custom ROMs are NOT SUPPORTED because:                    │
│ • May remove bionic libc                                  │
│ • May use different dynamic linker                        │
│ • May have different SELinux policies                     │
│ • May have stripped system libraries                      │
│ • Varies by ROM version and device                       │
│                                                             │
│ If you use a custom ROM:                                 │
│ 1. Verify bionic is present: ls -la /system/lib64/libc.so│
│ 2. Verify linker is present: ls -la /system/bin/linker64  │
│ 3. Test AURA on your ROM before relying on it           │
│ 4. Report issues to community, not as bugs               │
└─────────────────────────────────────────────────────────────┘
```

---

## Section 6: Contract Violation Handling

### 6.1 Violation Classification

Every contract violation is classified in the failure taxonomy:

| Violation | Failure Class | Prevention |
|-----------|--------------|-----------|
| API level < 24 | F002-OS_VERSION | Pre-install check |
| RAM < 3GB | F005-MEMORY_CONTRACT | Boot stage 2 |
| Storage < 2GB | F002-STORAGE_CONTRACT | Boot stage 2 |
| curl missing | F002-DEPENDENCY_MISSING | Boot stage 3 |
| Termux missing | F002-ENV_MISSING | Boot stage 2 |
| bionic missing | F003-ABI_CONTRACT | Boot stage 2 |
| SIGSEGV at startup | F005-STARTUP_CRASH | Device testing |
| Slow startup > 30s | F005-SLOW_STARTUP | Boot monitoring |
| Memory > 1GB | F005-MEMORY_EXCEEDED | Runtime monitor |

### 6.2 Error Message Format

All contract violations MUST display:

```
╔═══════════════════════════════════════════════════════════╗
║ AURA CONTRACT VIOLATION                                  ║
╠═══════════════════════════════════════════════════════════╣
║ Violation:  [Specific violation]                         ║
║ Required:    [Contract requirement]                      ║
║ Found:       [Actual device state]                      ║
║                                                             ║
║ What this means:                                         ║
║ [Plain-language explanation]                              ║
║                                                             ║
║ What you can do:                                         ║
║ 1. [Resolution step 1]                                  ║
║ 2. [Resolution step 2]                                  ║
║ 3. [Contact/support info]                               ║
║                                                             ║
║ Contract: aura-contract.v1.0                             ║
║ Document: docs/build/CONTRACT.md                         ║
╚═══════════════════════════════════════════════════════════╝
```

### 6.3 Boot Log Contract

Every boot attempt MUST produce a log entry:

```json
{
  "version": "4.0.0",
  "boot_id": "uuid-v4",
  "timestamp": "2026-03-22T10:30:00.000Z",
  "device": {
    "os_version": "Android 15 (API 35)",
    "architecture": "aarch64",
    "device_model": "Moto G45 5G",
    "ram_mb": 8192,
    "storage_free_gb": 45
  },
  "stages": [
    {
      "name": "INIT",
      "duration_ms": 234,
      "status": "PASS",
      "checks": ["config_loaded", "dirs_created"]
    },
    {
      "name": "ENV_CHECK",
      "duration_ms": 1234,
      "status": "PASS",
      "checks": ["api_level_ok", "arch_ok", "termux_present", "bionic_present", "ram_ok", "storage_ok"]
    },
    {
      "name": "DEP_CHECK",
      "duration_ms": 4567,
      "status": "PASS",
      "checks": ["curl_ok", "git_ok", "bash_ok", "openssl_ok"]
    },
    {
      "name": "RUNTIME_START",
      "duration_ms": 8901,
      "status": "PASS",
      "checks": ["telegram_connected", "daemon_ready"]
    },
    {
      "name": "READY",
      "duration_ms": 0,
      "status": "PASS",
      "checks": ["accepting_requests"]
    }
  ],
  "overall_status": "PASS",
  "contract_version": "1.0"
}
```

---

## Section 7: Test Coverage Contract

### 7.1 Required Test Matrix

```
┌─────────────────────────────────────────────────────────────┐
│ TEST MATRIX: Contract Verification                        │
├─────────────────────────────────────────────────────────────┤
│ Dimension          │ Minimum │ Standard │ Extended        │
│ OS Version         │ API 24  │ API 30   │ API 35         │
│ RAM               │ 3 GB    │ 6 GB     │ 8+ GB          │
│ Vendor Behavior   │ Moto    │ Samsung   │ Xiaomi          │
│                                                             │
│ MINIMUM TEST COVERAGE:                                     │
│ • One device from Minimum column                          │
│ • One device from Standard column                         │
│ • One device from Extended column                         │
│ • Total: 3 device configurations minimum                  │
│                                                             │
│ CURRENT TEST DEVICE: Moto G45 5G (Extended)               │
│ MISSING: Minimum, Standard configurations                 │
└─────────────────────────────────────────────────────────────┘
```

### 7.2 Test Cases Per Configuration

| Test Case | What It Verifies | Expected Result |
|-----------|-----------------|-----------------|
| TC-01 | `./aura-daemon --version` | EXIT 0, version displayed |
| TC-02 | Boot stages complete | All 5 stages PASS |
| TC-03 | Telegram connection | Bot responds to /start |
| TC-04 | Memory stays < 1GB | RSS < 1073741824 bytes |
| TC-05 | No SIGSEGV | No signal 11 in logs |
| TC-06 | Contract violations show clear errors | Error message formatted correctly |
| TC-07 | Crash produces crash report | JSON file in ~/.aura/crash-reports/ |
| TC-08 | Privacy: no telemetry | No network calls except Telegram |

---

## Section 8: Contract Verification Checklist

### 8.1 Pre-Release Checklist

```
BEFORE ANY RELEASE, verify:

□ Boot log format matches Section 6.3
□ Error messages formatted as Section 6.2
□ All 5 boot stages implemented and logging
□ Contract version included in all outputs
□ Crash reports contain no PII
□ Memory monitoring active
□ SIGSEGV produces classified crash report
□ Contract violations display clear messages
□ Test matrix (Section 7.1) has been executed
□ All test cases (Section 7.2) pass on test device
```

### 8.2 Continuous Verification

Every CI run MUST verify:

```
□ cargo check --all-features passes
□ cargo test --workspace passes
□ ELF inspection: Machine: AArch64
□ ELF inspection: NX enabled
□ ELF inspection: PIE enabled
□ SBOM generated
□ Boot log format validated
□ Contract version in binary
```

Every device test MUST verify:

```
□ ./aura-daemon --version returns 0
□ All 5 boot stages complete
□ No SIGSEGV in logs
□ No contract violations (or correctly handled)
□ Memory stays within contract
□ Telegram connects successfully
```

---

## Section 9: Contract Exceptions

### 9.1 Exception Process

Contract exceptions require:

1. Written proposal with rationale
2. Impact analysis (who is affected)
3. Founder approval
4. Regression test plan
5. Exception added to this document with expiry date

### 9.2 Current Exceptions

| Exception | Reason | Expires | Owner |
|-----------|--------|---------|-------|
| None | N/A | N/A | N/A |

---

## Section 10: Document Control

### 10.1 Change Log

| Version | Date | Changes | Author |
|---------|------|---------|--------|
| 1.0 | 2026-03-22 | Initial contract | AI (Architecture) |

### 10.2 Review Schedule

| Review | Frequency | Owner |
|--------|-----------|-------|
| Full contract review | Annually | Founder + AI |
| Exception review | Quarterly | AI (Architecture) |
| Test matrix update | Per release | AI (QA) |
| Violation pattern review | Monthly | AI (Forensics) |

### 10.3 Related Documents

| Document | Relationship |
|----------|-------------|
| docs/failure-db/SIGNATURES.md | Failure taxonomy for violations |
| docs/runtime/BOOT-STAGES.md | Boot stage implementation |
| docs/validation/DEVICE-MATRIX.md | Test coverage |
| docs/architecture/OVERVIEW.md | System architecture |

---

## Appendix A: Quick Reference

### For Users

```
AURA requires:
• Android 7.0 (API 24) or higher
• 64-bit ARM processor (arm64)
• 3 GB RAM minimum
• 2 GB storage free
• Termux installed
• Internet for Telegram (not for operation)
```

### For Developers

```
Contract enforcement:
• Boot stage 2 (ENV_CHECK): Verifies OS, arch, RAM, storage, Termux
• Boot stage 3 (DEP_CHECK): Verifies curl, git, bash, openssl
• Runtime monitor: Verifies memory usage
• Crash handler: Classifies and logs all crashes

All violations → Section 6.2 error format
All logs → Section 6.3 boot log format
All crashes → Section 4.3 crash report format
```

---

**END OF CONTRACT**

*This contract is the FOUNDATION of AURA engineering. All decisions must respect it.*
