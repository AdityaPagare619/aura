# AURA v4 — COMPLETE CODEBASE TESTING GUIDE
**Date:** March 21, 2026
**Scope:** WHOLE AURA System — All 5 Crates, All Modules
**Device:** Your Android Phone (Termux)

---

## WHAT IS AURA?

AURA has 5 crates:
1. **aura-daemon** — Main daemon, Telegram, memory, ethics, execution
2. **aura-neocortex** — LLM inference, model management
3. **aura-types** — Shared IPC types, actions, DSL
4. **aura-iron-laws** — Compile-time ethics enforcement
5. **aura-llama-sys** — llama.cpp bindings

Each crate has multiple modules. We test EVERYTHING.

---

## PART 1: FRESH SETUP

```bash
# Clean slate
cd ~
rm -rf aura test-results

# Environment check
uname -a
df -h ~/
free -m
cat /proc/cpuinfo | head -3

# Update Termux
pkg update -y

# Install ALL build tools
pkg install rust git clang make openssl-dev -y

# Verify
rustc --version
cargo --version
clang --version
```

---

## PART 2: CLONE AND BUILD

```bash
cd ~
git clone https://github.com/AdityaPagare619/aura.git
cd aura

# Check what we have
git log --oneline -5
cat Cargo.toml | grep "^version"

# BUILD EVERYTHING
cargo build --release 2>&1 | tee build.log

# Check what was built
ls -la target/release/
file target/release/aura-daemon
file target/release/aura-neocortex
```

---

## PART 3: TEST EVERY CRATE

### 3a. aura-iron-laws (Ethics — IMMUTABLE)
```bash
cd ~/aura

# Test that ethics compile-time checks work
cargo test -p aura-iron-laws 2>&1 | tee ethics-test.log

# Check 7 Iron Laws are enforced
grep -r "IRON_LAWS\|IronLaw" crates/aura-iron-laws/src/
cat crates/aura-iron-laws/src/lib.rs | head -100
```

### 3b. aura-types (Shared Types)
```bash
# Test types compile
cargo check -p aura-types 2>&1

# Check IPC types
grep "pub struct\|pub enum" crates/aura-types/src/*.rs | head -20
```

### 3c. aura-llama-sys (LLM Bindings)
```bash
# Test llama bindings
cargo check -p aura-llama-sys 2>&1

# Check for GGUF support
grep -r "gguf\|GGUF" crates/aura-llama-sys/src/ | head -10
```

### 3d. aura-neocortex (LLM Inference)
```bash
# Test neocortex
cargo test -p aura-neocortex --lib 2>&1 | tee neocortex-test.log

# Check inference modules
ls crates/aura-neocortex/src/
grep "pub fn\|pub mod" crates/aura-neocortex/src/*.rs | head -30
```

### 3e. aura-daemon (FULL TEST)
```bash
# Test EVERY module in daemon
cargo test -p aura-daemon 2>&1 | tee daemon-full-test.log

# List all daemon modules
ls crates/aura-daemon/src/
```

---

## PART 4: TEST EVERY MODULE IN AURA-DAEMON

### 4a. identity/ (Ethics, Personality, Trust)
```bash
cd ~/aura

echo "=== Testing identity module ==="
grep -r "pub fn\|pub mod" crates/aura-daemon/src/identity/*.rs | head -30

# Test ethics
grep -n "ethics\|ETHICS\|iron" crates/aura-daemon/src/identity/*.rs | head -20

# Test anti-sycophancy
grep -n "sycophancy\|SYCO" crates/aura-daemon/src/identity/*.rs | head -10

# Test trust tiers
grep -n "TRUST\|tier\|Tier" crates/aura-daemon/src/identity/*.rs | head -20
```

### 4b. memory/ (4-Tier Memory System)
```bash
echo "=== Testing memory module ==="
ls crates/aura-daemon/src/memory/
grep "pub fn\|pub mod" crates/aura-daemon/src/memory/*.rs | head -40

# Check 4 tiers
grep -rn "working\|episodic\|semantic\|archive" crates/aura-daemon/src/memory/*.rs | head -20
```

### 4c. policy/ (Policy Gate)
```bash
echo "=== Testing policy module ==="
ls crates/aura-daemon/src/policy/
grep "pub fn\|pub mod" crates/aura-daemon/src/policy/*.rs | head -30

# Check deny-by-default
grep -n "deny\|DENY\|allow\|ALLOW" crates/aura-daemon/src/policy/*.rs | head -20
```

### 4d. execution/ (ReAct Executor)
```bash
echo "=== Testing execution module ==="
ls crates/aura-daemon/src/execution/
grep "pub fn\|pub mod" crates/aura-daemon/src/execution/*.rs | head -30

# Check planner
grep -n "planner\|PLAN\|react\|ReAct" crates/aura-daemon/src/execution/*.rs | head -20
```

### 4e. telegram/ (Telegram Integration)
```bash
echo "=== Testing telegram module ==="
ls crates/aura-daemon/src/telegram/
grep "pub fn\|pub mod" crates/aura-daemon/src/telegram/*.rs | head -30

# Check bot token handling
grep -n "token\|TOKEN\|TELEGRAM" crates/aura-daemon/src/telegram/*.rs | head -20
```

### 4f. daemon_core/ (Main Loop)
```bash
echo "=== Testing daemon_core ==="
ls crates/aura-daemon/src/daemon_core/
grep "pub fn\|pub mod" crates/aura-daemon/src/daemon_core/*.rs | head -30
```

### 4g. arc/ (Artificial Companion)
```bash
echo "=== Testing arc module ==="
ls crates/aura-daemon/src/arc/
grep "pub fn\|pub mod" crates/aura-daemon/src/arc/*.rs | head -40
```

### 4h. persistence/ (Vault, Journal)
```bash
echo "=== Testing persistence ==="
ls crates/aura-daemon/src/persistence/
grep "pub fn\|pub mod" crates/aura-daemon/src/persistence/*.rs | head -20

# Check vault
grep -n "vault\|VAULT\|encrypt\|ENCRYPT" crates/aura-daemon/src/persistence/*.rs | head -10
```

### 4i. platform/ (Android Bridge)
```bash
echo "=== Testing platform ==="
ls crates/aura-daemon/src/platform/
grep "pub fn\|pub mod" crates/aura-daemon/src/platform/*.rs | head -20
```

### 4j. voice/ (Voice Processing)
```bash
echo "=== Testing voice ==="
ls crates/aura-daemon/src/voice/
grep "pub fn\|pub mod" crates/aura-daemon/src/voice/*.rs | head -30
```

### 4k. screen/ (UI Automation)
```bash
echo "=== Testing screen ==="
ls crates/aura-daemon/src/screen/
grep "pub fn\|pub mod" crates/aura-daemon/src/screen/*.rs | head -30
```

### 4l. goals/ (Goal Management)
```bash
echo "=== Testing goals ==="
ls crates/aura-daemon/src/goals/
grep "pub fn\|pub mod" crates/aura-daemon/src/goals/*.rs | head -30
```

---

## PART 5: DAEMON BINARY TEST

```bash
cd ~/aura

# F001 TEST (Does it start?)
echo "=== F001 TEST ==="
./target/release/aura-daemon --version
echo "EXIT: $?"

./target/release/aura-daemon --help
echo "EXIT: $?"

# Try startup (will need model)
timeout 10 ./target/release/aura-daemon 2>&1 | head -30
echo "EXIT: $?"
```

---

## PART 6: TELEGRAM END-TO-END

```bash
cd ~/aura

# Configure
export AURA_TELEGRAM_TOKEN="8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI"
export AURA_TELEGRAM_CHAT_ID="8407946567"

# Start daemon in background
./target/release/aura-daemon &
DAEMON_PID=$!
echo "Daemon PID: $DAEMON_PID"

# Wait for startup
sleep 10

# Check process
ps -A | grep aura

# Send Telegram message NOW (from your phone):
# "Hey Aura, test message"

# Wait for response
sleep 15

# Kill daemon
kill $DAEMON_PID 2>/dev/null
echo "Daemon stopped"
```

---

## PART 7: INTEGRATION TESTS

```bash
cd ~/aura

# Run ALL integration tests
cargo test --workspace 2>&1 | tee integration-tests.log

# Check results
tail -50 integration-tests.log
grep -E "test result|passed|failed" integration-tests.log
```

---

## PART 8: CAPTURE EVERYTHING

```bash
cd ~/aura

mkdir -p test-results-$(date +%Y%m%d-%H%M%S)
RESULTS=test-results-$(date +%Y%m%d-%H%M%S)

# Save all logs
cp *.log $RESULTS/ 2>/dev/null
cp -r target/release/aura-daemon $RESULTS/ 2>/dev/null
cp -r target/release/aura-neocortex $RESULTS/ 2>/dev/null

# Create summary
cat > $RESULTS/SUMMARY.txt << SUMMARY
=== AURA v4 COMPLETE CODEBASE TEST ===
Date: $(date)
Device: $(uname -a)
Git: $(git log --oneline -1)

=== BUILD STATUS ===
$(tail -10 build.log 2>/dev/null)

=== TEST STATUS ===
$(grep -E "test result|passed|failed" integration-tests.log 2>/dev/null)

=== DAEMON VERSION ===
$(./target/release/aura-daemon --version 2>&1)

=== ISSUES FOUND ===
[List all issues below]
SUMMARY

# List results
ls -la $RESULTS/
```

---

## WHAT TO REPORT BACK

1. **Build:** Did cargo build succeed?
2. **Tests:** How many passed/failed?
3. **Daemon:** Does it start without SIGSEGV?
4. **Telegram:** Did it respond?
5. **Screenshots:** Terminal output
6. **Logs:** All .log files

---

## TELEGRAM CREDENTIALS

```
Bot Token: 8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI
Chat ID: 8407946567
Bot: @AuraTheBegginingBot
```

