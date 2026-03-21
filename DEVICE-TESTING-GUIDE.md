# AURA v4 — Device Testing Guide (Fresh Start)
**Date:** March 21, 2026
**Device:** Your Android Phone (Termux)
**Method:** Build native ARM64, test every subsystem

---

## WHY DEVICE TESTING?

CI compiles code. Device RUNS code.
Many bugs only appear when running:
- SIGSEGV at startup (F001)
- Memory corruption
- Android-specific syscalls
- Termux environment issues
- ARM64 ABI mismatches
- Dynamic linking problems

---

## STEP 1: FRESH START (Clean slate)

```bash
# Go to Termux home
cd ~

# Remove old files (fresh start)
rm -rf aura aura-daemon aura-source

# Check Termux environment
uname -a
cat /proc/cpuinfo | head -5
df -h ~/
free -m
```

---

## STEP 2: INSTALL BUILD TOOLS

```bash
# Update package manager
pkg update -y

# Install Rust (native ARM64)
pkg install rust -y

# Verify Rust installed
rustc --version
cargo --version

# Install Git
pkg install git -y

# Install additional tools
pkg install clang make -y

# Check everything
rustc --version
cargo --version
clang --version
git --version
```

---

## STEP 3: CLONE FRESH REPO

```bash
cd ~
git clone https://github.com/AdityaPagare619/aura.git
cd aura

# Check what we're on
git log --oneline -3
git status
cat Cargo.toml | grep "^version"
```

---

## STEP 4: BUILD NATIVE (ARM64, No Cross-Compile!)

```bash
cd ~/aura

# Build for native ARM64 (Termux uses Linux userland on Android)
cargo build --release 2>&1 | tee build.log

# Check if build succeeded
ls -la target/release/aura-daemon
file target/release/aura-daemon
```

---

## STEP 5: F001 SMOKE TEST (Critical)

```bash
cd ~/aura

# Test 1: Version flag
./target/release/aura-daemon --version
echo "EXIT: $?"

# Test 2: Help flag  
./target/release/aura-daemon --help
echo "EXIT: $?"

# Test 3: Try to start (will fail without model, but should NOT SIGSEGV)
timeout 5 ./target/release/aura-daemon 2>&1 | head -20
echo "EXIT: $?"
```

**PASS CRITERIA:**
- `--version` → Exit 0
- `--help` → Exit 0
- Startup → NO SIGSEGV (exit 139)

---

## STEP 6: SUBSYSTEM TESTS

### 6a. Memory System
```bash
cd ~/aura
mkdir -p ~/.local/share/aura
./target/release/aura-daemon memory-test 2>&1 || echo "No memory-test flag"
```

### 6b. Ethics Layer
```bash
# Try to trigger ethics block
echo "Testing ethics layer..."
./target/release/aura-daemon --ethics-test 2>&1 || echo "No ethics-test flag"
```

### 6c. Telegram Config
```bash
export AURA_TELEGRAM_TOKEN="8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI"
export AURA_TELEGRAM_CHAT_ID="8407946567"
./target/release/aura-daemon telegram-status 2>&1 || echo "No telegram-status flag"
```

### 6d. Build Info
```bash
./target/release/aura-daemon build-info 2>&1 || echo "No build-info"
./target/release/aura-daemon --version
sha256sum ./target/release/aura-daemon
```

---

## STEP 7: LOG CAPTURE (Forensic Analysis)

```bash
cd ~/aura

# Run with full logging
RUST_LOG=debug ./target/release/aura-daemon 2>&1 | tee aura-startup.log

# Run for 10 seconds
timeout 10 ./target/release/aura-daemon 2>&1 | tee aura-10sec.log

# Check log files
ls -la *.log
cat aura-startup.log | head -50
```

---

## STEP 8: TELEGRAM END-TO-END TEST

### On device, start daemon with Telegram:
```bash
export AURA_TELEGRAM_TOKEN="8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI"
export AURA_TELEGRAM_CHAT_ID="8407946567"

# Start daemon in background
./target/release/aura-daemon &
DAEMON_PID=$!
echo "Daemon PID: $DAEMON_PID"

# Wait for startup
sleep 5

# Check if running
ps -A | grep aura-daemon

# Kill after test
kill $DAEMON_PID 2>/dev/null
```

### On your phone, send Telegram message:
```
Hey Aura, are you alive?
```

### Check device logs:
```bash
tail -50 ~/.local/share/aura/logs/*.log 2>/dev/null
```

---

## STEP 9: TEST MATRIX

| Test | Command | Expected | F001 Status |
|------|---------|----------|-------------|
| Version | `--version` | Exit 0 | ✓/✗ |
| Help | `--help` | Exit 0 | ✓/✗ |
| Startup | daemon run | No crash | ✓/✗ |
| Memory | `memory-test` | Works | ✓/✗ |
| Ethics | `ethics-test` | Blocked | ✓/✗ |
| Telegram | send msg | Response | ✓/✗ |
| Build info | `build-info` | Shows info | ✓/✗ |

---

## STEP 10: CAPTURE EVIDENCE

```bash
cd ~/aura

# Save everything
mkdir -p test-results
date > test-results/run-date.txt
git log --oneline -1 >> test-results/run-date.txt
uname -a >> test-results/run-date.txt
cp build.log test-results/ 2>/dev/null
ls -la test-results/

# Create summary
cat > test-results/SUMMARY.txt << SUMMARY
=== AURA v4 Device Test Summary ===
Date: $(date)
Device: $(uname -a)
Commit: $(git log --oneline -1)
Version: $(./target/release/aura-daemon --version 2>&1)

=== F001 Status ===
$(./target/release/aura-daemon --version; echo "Exit: $?")

=== Build Status ===
$(tail -5 build.log)

=== Issues Found ===
[List issues here]
SUMMARY

cat test-results/SUMMARY.txt
```

---

## TELEGRAM CREDENTIALS (Ready to use)

```
Bot Token: 8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI
Chat ID: 8407946567
Bot: @AuraTheBegginingBot
```

---

## AFTER TESTING

1. **Screenshot all terminal output**
2. **Save all logs to files**
3. **Note any errors/crashes**
4. **Report findings to CEO**

---

## EXPECTED OUTCOMES

### IF F001 IS FIXED:
```
aura-daemon 4.0.0-alpha.X
EXIT: 0
```
(No SIGSEGV)

### IF ETHICS WORKS:
Ethics blocks harmful requests with clear message.

### IF TELEGRAM WORKS:
AURA responds to "Hey Aura" via Telegram.

### IF SOMETHING BROKE:
Capture the exact error, signal, and crash output.
