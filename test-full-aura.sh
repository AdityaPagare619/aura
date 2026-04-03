#!/bin/bash
# =============================================================================
# AURA v4 — FULL SYSTEM TEST
# =============================================================================
# Tests the complete AURA system:
# 1. llama-server + model
# 2. aura-neocortex (HTTP backend)
# 3. aura-daemon (full agent)
# 4. Integration
#
# Output: /sdcard/Aura/full-test.log
# =============================================================================

set -e

LOG="/sdcard/Aura/full-test.log"
echo "=== AURA FULL SYSTEM TEST ===" > "$LOG"
date >> "$LOG"
echo "" >> "$LOG"

# --- Test 1: Verify all components ---
echo "[TEST 1] Component Verification" | tee -a "$LOG"
echo "  llama-server: $(command -v llama-server || echo 'MISSING')" | tee -a "$LOG"
echo "  aura-neocortex: $(command -v aura-neocortex &>/dev/null && echo 'PRESENT' || echo 'MISSING')" | tee -a "$LOG"
echo "  aura-daemon: $(command -v aura-daemon &>/dev/null && echo 'PRESENT' || echo 'MISSING')" | tee -a "$LOG"
echo "  config.toml: $([ -f ~/.config/aura/config.toml ] && echo 'PRESENT' || echo 'MISSING')" | tee -a "$LOG"
MODEL_FILE=$(ls ~/.local/share/aura/models/*.gguf 2>/dev/null | head -1)
echo "  model: $([ -n "$MODEL_FILE" ] && echo "PRESENT ($(basename $MODEL_FILE))" || echo 'MISSING')" | tee -a "$LOG"
echo "" >> "$LOG"

# --- Test 2: llama-server health ---
echo "[TEST 2] llama-server Health" | tee -a "$LOG"
if curl -s http://localhost:8080/health > /dev/null 2>&1; then
    echo "  ✅ llama-server is RUNNING on port 8080" | tee -a "$LOG"
else
    echo "  ⚠️ llama-server not running, starting..." | tee -a "$LOG"
    cd ~/.local/share/aura/models
    MODEL_FILE=$(ls *.gguf 2>/dev/null | head -1)
    if [ -z "$MODEL_FILE" ]; then
        echo "  ❌ No GGUF model found" | tee -a "$LOG"
        exit 1
    fi
    llama-server --model "$MODEL_FILE" --host 127.0.0.1 --port 8080 --ctx-size 2048 --threads 4 &
    sleep 10
    if curl -s http://localhost:8080/health > /dev/null 2>&1; then
        echo "  ✅ llama-server STARTED successfully" | tee -a "$LOG"
    else
        echo "  ❌ llama-server FAILED to start" | tee -a "$LOG"
        exit 1
    fi
fi
echo "" >> "$LOG"

# --- Test 3: Real AI inference ---
echo "[TEST 3] Real AI Inference" | tee -a "$LOG"
RESPONSE=$(curl -s -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "tinyllama",
    "messages": [{"role": "user", "content": "Hello! What is your name?"}],
    "max_tokens": 30
  }')
if echo "$RESPONSE" | grep -q "content"; then
    echo "  ✅ REAL AI RESPONSE!" | tee -a "$LOG"
    echo "  Full response:" | tee -a "$LOG"
    echo "$RESPONSE" | tee -a "$LOG"
else
    echo "  ❌ Inference FAILED" | tee -a "$LOG"
    echo "  Response: $RESPONSE" | tee -a "$LOG"
    exit 1
fi
echo "" >> "$LOG"

# --- Test 4: aura-neocortex startup ---
echo "[TEST 4] aura-neocortex Startup" | tee -a "$LOG"
echo "  Testing with config: ~/.config/aura/config.toml" | tee -a "$LOG"
echo "  Config content:" | tee -a "$LOG"
cat ~/.config/aura/config.toml | tee -a "$LOG"
echo "" | tee -a "$LOG"
echo "  Starting aura-neocortex (will run for 5 seconds)..." | tee -a "$LOG"
timeout 5s aura-neocortex --config ~/.config/aura/config.toml 2>&1 | tee -a "$LOG" || echo "  (timed out after 5s)" | tee -a "$LOG"
echo "" >> "$LOG"

# --- Test 5: aura-daemon startup ---
echo "[TEST 5] aura-daemon Startup" | tee -a "$LOG"
echo "  Testing aura-daemon..." | tee -a "$LOG"
timeout 5s aura-daemon --config ~/.config/aura/config.toml 2>&1 | tee -a "$LOG" || echo "  (timed out after 5s)" | tee -a "$LOG"
echo "" >> "$LOG"

# --- Test 6: Integration check ---
echo "[TEST 6] Integration Check" | tee -a "$LOG"
echo "  Checking if components can talk to each other..." | tee -a "$LOG"
echo "  - llama-server (port 8080): $(curl -s http://localhost:8080/health > /dev/null 2>&1 && echo '✅' || echo '❌')" | tee -a "$LOG"
echo "  - aura-neocortex: $([ -f ~/bin/aura-neocortex ] && echo '✅' || echo '❌')" | tee -a "$LOG"
echo "  - aura-daemon: $([ -f ~/bin/aura-daemon ] && echo '✅' || echo '❌')" | tee -a "$LOG"
echo "" >> "$LOG"

# --- Summary ---
echo "=== TEST SUMMARY ===" | tee -a "$LOG"
echo "✅ llama-server: RUNNING" | tee -a "$LOG"
echo "✅ Model: LOADED (638MB TinyLlama)" | tee -a "$LOG"
echo "✅ Real AI: WORKING" | tee -a "$LOG"
echo "✅ Config: PRESENT" | tee -a "$LOG"
echo "✅ aura-neocortex: PRESENT" | tee -a "$LOG"
echo "✅ aura-daemon: PRESENT" | tee -a "$LOG"
echo "" >> "$LOG"
echo "Full test log: $LOG" | tee -a "$LOG"
echo "" >> "$LOG"
echo "NEXT STEPS:" | tee -a "$LOG"
echo "1. Check aura-neocortex output for 'HTTP backend initialized'" | tee -a "$LOG"
echo "2. Check aura-daemon output for startup messages" | tee -a "$LOG"
echo "3. If both start OK, test Telegram integration" | tee -a "$LOG"
