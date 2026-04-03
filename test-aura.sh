#!/usr/bin/env bash
# =============================================================================
# AURA v4 — Complete Test Script (Run in Termux)
# =============================================================================
# This script tests the FULL AURA system:
# 1. llama-server connection
# 2. HTTP backend (our code)
# 3. aura-neocortex
# 4. Full pipeline readiness
#
# Usage: bash test-aura.sh
# All output saved to /sdcard/Aura/test-results.log
# =============================================================================

set -e

# Termux detection
if [ -d "/data/data/com.termux/files/usr" ]; then
    export PATH="/data/data/com.termux/files/usr/bin:$PATH"
    IS_TERMUX=1
else
    IS_TERMUX=0
fi

LOG="/sdcard/Aura/test-results.log"
echo "=== AURA TEST START ===" > "$LOG"
date >> "$LOG"
echo "" >> "$LOG"

# --- Test 1: Environment ---
echo "[TEST 1] Environment Check" | tee -a "$LOG"
echo "  Architecture: $(uname -m)" | tee -a "$LOG"
echo "  Termux prefix: $PREFIX" | tee -a "$LOG"
echo "" >> "$LOG"

# --- Test 2: llama-server ---
echo "[TEST 2] llama-server" | tee -a "$LOG"
if command -v llama-server &> /dev/null; then
    echo "  ✅ llama-server found: $PREFIX/bin/llama-server" | tee -a "$LOG"
    llama-server --version | head -1 | tee -a "$LOG"
else
    echo "  ❌ llama-server NOT found" | tee -a "$LOG"
    exit 1
fi
echo "" >> "$LOG"

# --- Test 3: Model file ---
echo "[TEST 3] Model File" | tee -a "$LOG"
MODEL_PATH="$HOME/.local/share/aura/models"
MODEL_FILE=$(ls "$MODEL_PATH/"*.gguf 2>/dev/null | head -1)
if [ -n "$MODEL_FILE" ] && [ -f "$MODEL_FILE" ]; then
    MODEL_SIZE=$(du -h "$MODEL_FILE" | cut -f1)
    echo "  ✅ Model found: $MODEL_FILE ($MODEL_SIZE)" | tee -a "$LOG"
else
    echo "  ❌ No GGUF model found in $MODEL_PATH" | tee -a "$LOG"
    echo "  Download a model or run install.sh" | tee -a "$LOG"
    exit 1
fi
echo "" >> "$LOG"

# --- Test 4: Start llama-server (if not running) ---
echo "[TEST 4] llama-server Status" | tee -a "$LOG"
if curl -s http://localhost:8080/health > /dev/null 2>&1; then
    echo "  ✅ llama-server already running on port 8080" | tee -a "$LOG"
else
    echo "  ⚠️ llama-server not running, starting..." | tee -a "$LOG"
    llama-server --model "$MODEL_FILE" --host 127.0.0.1 --port 8080 --ctx-size 2048 --threads 4 &
    sleep 10
    if curl -s http://localhost:8080/health > /dev/null 2>&1; then
        echo "  ✅ llama-server started successfully" | tee -a "$LOG"
    else
        echo "  ❌ llama-server failed to start" | tee -a "$LOG"
        exit 1
    fi
fi
echo "" >> "$LOG"

# --- Test 5: Real AI Inference ---
echo "[TEST 5] Real AI Inference" | tee -a "$LOG"
RESPONSE=$(curl -s -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "tinyllama",
    "messages": [{"role": "user", "content": "Say hello in 3 words!"}],
    "max_tokens": 20
  }')
if echo "$RESPONSE" | grep -q "content"; then
    echo "  ✅ REAL AI RESPONSE!" | tee -a "$LOG"
    echo "  Response: $RESPONSE" | head -c 300 | tee -a "$LOG"
else
    echo "  ❌ Inference failed" | tee -a "$LOG"
    echo "  Response: $RESPONSE" | tee -a "$LOG"
    exit 1
fi
echo "" >> "$LOG"

# --- Test 6: aura-neocortex binary ---
echo "[TEST 6] aura-neocortex Binary" | tee -a "$LOG"
if command -v aura-neocortex &>/dev/null; then
    echo "  ✅ aura-neocortex found: $(command -v aura-neocortex)" | tee -a "$LOG"
    aura-neocortex --version 2>&1 | head -1 | tee -a "$LOG" || echo "  (version check returned: $?)" | tee -a "$LOG"
else
    echo "  ❌ aura-neocortex NOT found in PATH" | tee -a "$LOG"
    echo "  Run install.sh to build and install" | tee -a "$LOG"
fi
echo "" >> "$LOG"

# --- Test 7: Configuration ---
echo "[TEST 7] Configuration" | tee -a "$LOG"
CONFIG_PATH="$HOME/.config/aura/config.toml"
if [ -f "$CONFIG_PATH" ]; then
    echo "  ✅ Config found: $CONFIG_PATH" | tee -a "$LOG"
    echo "  Content:" | tee -a "$LOG"
    cat "$CONFIG_PATH" | tee -a "$LOG"
else
    echo "  ❌ Config NOT found" | tee -a "$LOG"
    echo "  Create with:" | tee -a "$LOG"
    echo '  mkdir -p ~/.config/aura' | tee -a "$LOG"
    echo '  cat > ~/.config/aura/config.toml << "EOF"' | tee -a "$LOG"
    echo '  [neocortex]' | tee -a "$LOG"
    echo '  backend_priority = ["http", "ffi", "stub"]' | tee -a "$LOG"
    echo '  ' | tee -a "$LOG"
    echo '  [neocortex.backend.http]' | tee -a "$LOG"
    echo '  base_url = "http://localhost:8080"' | tee -a "$LOG"
    echo '  model_name = "tinyllama"' | tee -a "$LOG"
    echo '  timeout_secs = 60' | tee -a "$LOG"
    echo '  health_check = true' | tee -a "$LOG"
    echo '  EOF' | tee -a "$LOG"
fi
echo "" >> "$LOG"

# --- Test 8: aura-daemon binary ---
echo "[TEST 8] aura-daemon Binary" | tee -a "$LOG"
if command -v aura-daemon &>/dev/null; then
    echo "  ✅ aura-daemon found: $(command -v aura-daemon)" | tee -a "$LOG"
    aura-daemon --version 2>&1 | head -1 | tee -a "$LOG" || echo "  (version check returned: $?)" | tee -a "$LOG"
else
    echo "  ❌ aura-daemon NOT found in PATH" | tee -a "$LOG"
    echo "  Run install.sh to build and install" | tee -a "$LOG"
fi
echo "" >> "$LOG"

# --- Summary ---
echo "=== TEST SUMMARY ===" | tee -a "$LOG"
echo "✅ llama-server: $(curl -s http://localhost:8080/health > /dev/null 2>&1 && echo 'RUNNING' || echo 'NOT RUNNING')" | tee -a "$LOG"
echo "✅ aura-neocortex: $(command -v aura-neocortex &>/dev/null && echo 'PRESENT' || echo 'MISSING')" | tee -a "$LOG"
echo "✅ aura-daemon: $(command -v aura-daemon &>/dev/null && echo 'PRESENT' || echo 'MISSING')" | tee -a "$LOG"
echo "✅ Config: $([ -f ~/.config/aura/config.toml ] && echo 'PRESENT' || echo 'MISSING')" | tee -a "$LOG"
echo "✅ Model: $(ls ~/.local/share/aura/models/*.gguf &>/dev/null && echo 'PRESENT' || echo 'MISSING')" | tee -a "$LOG"
echo "" >> "$LOG"
echo "Full log saved to: $LOG" | tee -a "$LOG"
