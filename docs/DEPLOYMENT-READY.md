# AURA Enterprise Deployment Checklist - READY TO EXECUTE

## Device Target
- **Device**: Moto G45 5G
- **Android API**: 35
- **Architecture**: arm64-v8a
- **Location**: Termux at /data/data/com.termux/files/home/

## Pre-Flight Before Deployment

### Step 0: Verify Device Connection
```bash
adb devices
# Should show device serial number
```

### Step 1: Quarantine Old Binaries (Prevent Conflicts, Reversible)
```bash
adb shell 'TS=$(date +%Y%m%d_%H%M%S); mkdir -p /data/local/tmp/aura_backup_$TS; \
for f in aura-daemon aura-daemon-new aura-daemon-old aura-neocortex aura-neocortex-v4 aura-neocortex-v5; do \
  [ -e /data/local/tmp/$f ] && mv /data/local/tmp/$f /data/local/tmp/aura_backup_$TS/$f; \
done'
```

### Step 2: Push Correct Android Artifact (NOT Windows .exe)
```bash
# REQUIRED: use Android ELF artifact built for aarch64-linux-android.
# Option A: Download from GitHub Actions artifact: aura-binaries-aarch64-linux-android
# Option B: Build locally with Android NDK toolchain and --target aarch64-linux-android

# Example (artifact already downloaded locally):
adb push artifacts/android-main-<run-id>/aura-neocortex /data/local/tmp/aura-neocortex

# Verify binary format BEFORE execution:
adb shell file /data/local/tmp/aura-neocortex
# Expected: ELF ... arm64 ... /system/bin/linker64
```

### Step 3: Set Permissions
```bash
adb shell chmod +x /data/local/tmp/aura-neocortex
```

### Step 4: First Test - Version Check (No Crash!)
```bash
adb shell /data/local/tmp/aura-neocortex --version
echo "Exit code: $?"
# Expected: 0 and version string printed
```

### Step 4b: Verify Artifact Provenance (SHA)
```bash
# Compare host and device sha256 to ensure exact artifact copied
sha256sum artifacts/android-main-<run-id>/aura-neocortex
adb shell sha256sum /data/local/tmp/aura-neocortex
```

### Step 5: Start Daemon with Full Logging
```bash
adb shell /data/local/tmp/aura-neocortex --socket @aura_ipc_v4 --model-dir /sdcard/Aura/models &
sleep 3
```

### Step 6: Check Structured Logs
```bash
adb logcat -d | grep -E "\[.*\] \[.*\] \[.*\] \[.*\]"
# Should see boot stages: init, environment_check, dependency_check, runtime_start, ready
```

### Step 7: Verify Health Endpoint
```bash
adb forward tcp:19401 tcp:19401
adb shell curl http://localhost:19401/health
# Expected JSON with daemon_state, active_backend, capabilities
```

### Step 8: Test Graceful Degradation
```bash
# First check what backends are available
adb logcat -d | grep -i "available"
# If neocortex fails, should see F003 and switch to llama_server
```

## What to Look For

### SUCCESS Indicators
- ✅ Exit code 0 on --version (no SIGSEGV!)
- ✅ Structured logs show boot stages in order
- ✅ /health returns JSON with "running_full" or "running_degraded"
- ✅ Memory metrics in logs (memory_mb, android_api, cpu_abi)
- ✅ Circuit breaker initialized in logs

### FAILURE Indicators (and what they mean)
- ❌ exit_code=139 → F003 (ABI mismatch) - wrong binary architecture
- ❌ "not found" → F001 (Artifact missing) - binary not pushed
- ❌ "library not found" → F002 (Dependency) - missing .so files
- ❌ "linker" → F004 (Linker failure) - .so loading issue

### Critical Note
- `target/release/aura-neocortex.exe` from Windows host is **not** deployable to Android.
- Always deploy an Android ELF artifact (`aarch64-linux-android`) and verify with `file`.

## Emergency Rollback
If something goes wrong:
```bash
# Kill any running aura processes
adb shell pkill -9 aura-neocortex
adb shell pkill -9 aura-daemon
# Remove binary
adb shell rm -f /data/local/tmp/aura-neocortex
```

## Documentation Location
- Android Artifact (example): `C:\Users\Lenovo\aura\artifacts\android-main-<run-id>\aura-neocortex`
- Device Intel: `C:\Users\Lenovo\aura\device-intel\DEVICE-INTELLIGENCE.md`
- Test Procedure: `C:\Users\Lenovo\aura\docs\validation\DEVICE-TEST-PROCEDURE.md`
