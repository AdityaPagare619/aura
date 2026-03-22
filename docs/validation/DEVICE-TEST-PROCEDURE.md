# Device Test Procedure: Moto G45 5G

**Document**: `docs/validation/DEVICE-TEST-PROCEDURE.md`  
**Purpose**: Device-specific test procedures for Moto G45 5G validation  
**Status**: MANDATORY — All device tests MUST follow this procedure  
**Created**: 2026-03-22  
**Owner**: QA Validation Charter

---

## Pre-Test Checklist

Before running any device tests, verify:

- [ ] Device connected to power (battery > 50%)
- [ ] Termux installed (version >= 0.70)
- [ ] Termux updated: `pkg update && pkg upgrade`
- [ ] Binary downloaded from CI artifact
- [ ] Device has SSH access configured (for automated testing)
- [ ] Network connectivity available for Telegram API

---

## Test Steps

### T-001: Binary Executes

**Purpose**: Verify the binary runs without SIGSEGV (tests F001 prevention)

**Steps**:
1. Deploy binary:
   ```bash
   cp aura-daemon ~/bin/
   ```
2. Make executable:
   ```bash
   chmod +x ~/bin/aura-daemon
   ```
3. Execute version check:
   ```bash
   ~/bin/aura-daemon --version
   ```
4. Check exit code:
   ```bash
   echo $?
   ```

**Expected Result**:
- Exit code: 0
- Output: AURA version string printed
- No SIGSEGV signal
- No "Exec format error"

**Failure Classification**:
| If... | Then... | F-Code |
|-------|---------|--------|
| Exit code != 0 | Check boot logs | F007 |
| SIGSEGV signal | ABI mismatch - verify NDK sysroot | F001 |
| "Exec format error" | Wrong ELF class - verify target triple | F005 |

---

### T-002: Boot Stages Complete

**Purpose**: Verify all 5 boot stages complete successfully

**Steps**:
1. Execute daemon in background:
   ```bash
   ~/bin/aura-daemon &
   PID=$!
   sleep 10
   kill $PID 2>/dev/null
   ```
2. Check boot logs:
   ```bash
   cat /sdcard/AURA/logs/aura-boot-*.log
   ```
3. Verify JSON boot log format (per CONTRACT.md Section 6.3)

**Expected Result**:
- 5 boot stages logged:
  1. INIT (config loaded, dirs created)
  2. ENV_CHECK (API level, arch, RAM, storage, Termux, bionic)
  3. DEP_CHECK (curl, git, bash, openssl)
  4. RUNTIME_START (Telegram connected)
  5. READY (accepting requests)
- All stages show status: PASS
- No warnings in any stage

**Failure Classification**:
| If... | Then... | F-Code |
|-------|---------|--------|
| Stage 1 fails | Config/dirs issue | F007 |
| Stage 2 fails | Environment contract violation | F010 |
| Stage 3 fails | Missing dependencies | F002 |
| Stage 4 fails | Telegram connection issue | F007 |
| Stage 5 not reached | Boot did not complete | F007 |

---

### T-003: curl_backend Works (reqwest)

**Purpose**: Verify HTTP backend (reqwest) connects to Telegram API

**Steps**:
1. Execute daemon:
   ```bash
   ~/bin/aura-daemon
   ```
2. Check Telegram connection in logs:
   ```bash
   grep -i "telegram\|long.poll\|api" /sdcard/AURA/logs/aura-boot-*.log
   ```
3. Verify long-poll started:
   ```bash
   # Wait 5 seconds then check logs for poll activity
   sleep 5
   tail -50 /sdcard/AURA/logs/aura-boot-*.log | grep -i "poll\|update"
   ```

**Expected Result**:
- Telegram bot connected to api.telegram.org
- Long-poll loop started
- No HTTPS/TLS errors in logs

**Failure Classification**:
| If... | Then... | F-Code |
|-------|---------|--------|
| Connection refused | Network or firewall issue | F010 |
| TLS error | OpenSSL issue | F002 |
| Bot auth failed | Check BOT_TOKEN | F010 |

---

### T-004: Memory Usage Within Contract

**Purpose**: Verify RSS stays within CONTRACT.md limits

**Steps**:
1. Run daemon:
   ```bash
   ~/bin/aura-daemon &
   PID=$!
   sleep 30
   ```
2. Check memory:
   ```bash
   cat /proc/$PID/status | grep VmRSS
   ```
3. Wait and recheck (monitor for leaks):
   ```bash
   sleep 60
   cat /proc/$PID/status | grep VmRSS
   ```

**Expected Result**:
- RSS < 512 MB (normal operation)
- RSS < 1 GB (with model loaded)
- Growth < 100 MB/hour

**Failure Classification**:
| If... | Then... | F-Code |
|-------|---------|--------|
| RSS > 1 GB | Memory limit exceeded | F005-MEMORY_EXCEEDED |
| Growth > 100 MB/hour | Memory leak | F005-MEMORY_LEAK |

---

### T-005: Contract Violation Handling

**Purpose**: Verify error messages formatted correctly when contract violated

**Steps**:
1. Create test with contract violation (e.g., insufficient storage)
2. Execute binary
3. Check error message format

**Expected Result**:
- Error message matches CONTRACT.md Section 6.2 format
- Shows: Violation, Required, Found, What this means, What you can do

**Failure Classification**:
| If... | Then... | F-Code |
|-------|---------|--------|
| No error message | F011 - Observability gap |
| Wrong format | Contract not implemented |

---

## Device Test Queue

Run tests in order:

| ID | Test Name | Purpose | F-Codes Tested |
|----|-----------|---------|----------------|
| DT-001 | T-001 Binary Executes | Verify no SIGSEGV | F001, F005 |
| DT-002 | T-002 Boot Stages | Verify all 5 stages pass | F007, F010 |
| DT-003 | T-003 curl_backend | Verify Telegram connects | F010 |
| DT-004 | T-004 Memory Usage | Verify contract limits | F005-MEMORY |
| DT-005 | T-005 Contract Violation | Verify error format | F011 |

---

## Failure Decision Path (D5 Integration)

When failure occurs, follow D5 from OPERATING-PROCEDURES.md:

```
FAILURE ENCOUNTERED IN DEVICE TEST
         │
         ▼
Is failure in FAILURE_TAXONOMY.md?
         │
    ┌────┴────┐
    │         │
   YES        NO
    │         │
    ▼         ▼
Apply     Create new
F-code    F-code (F016+)
    │         │
    ▼         ▼
Check     Document in
Prevention│ ISSUE-LOG.md
    │         │
    ▼         ▼
Apply     Add to
Resolution│ TAXONOMY.md
    │         │
    ▼         ▼
Add test  Add test
that      that
catches   catches it
```

---

## Device-Specific Notes (Moto G45 5G)

### Known Behaviors
- Android 15 (API 35) with bionic libc
- Termux: 0.70+ from F-Droid
- RAM: 8 GB (well above 3 GB minimum)
- Storage: 128 GB (well above 2 GB minimum)

### Test Environment
- Network: WiFi connected
- Power: Battery > 50%
- SSH: Configured for automation

---

## CI Integration

Device tests are:
1. STAGE 6 in CI Pipeline v2 (docs/ci/AURA-CI-PIPELINE-V2-DESIGN.md)
2. MANDATORY release gate (D6 in OPERATING-PROCEDURES.md)
3. Not optional - device testing IS the validation

---

**END OF PROCEDURE**

*This procedure is MANDATORY for all device testing. CI passing is BUILD validation, not device validation.*