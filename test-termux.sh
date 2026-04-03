#!/bin/bash
# =============================================================================
# AURA v4 — Quick Test Script (Run inside Termux)
# =============================================================================
# This script tests that everything is working after install.sh
#
# Usage:
#   bash test-termux.sh
# =============================================================================

set -e

echo "========================================"
echo "  AURA v4 — Quick Test"
echo "========================================"
echo ""

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Test 1: Check llama-server
echo "[Test 1] Checking llama-server..."
if command -v llama-server &> /dev/null; then
    echo -e "${GREEN}✓ llama-server found${NC}"
    llama-server --version | head -1
else
    echo -e "${RED}✗ llama-server NOT found${NC}"
    echo "Install with: pkg install llama-cpp"
    exit 1
fi

# Test 2: Check model
echo ""
echo "[Test 2] Checking model..."
MODEL_PATH="$HOME/.local/share/aura/models"
if [ -d "$MODEL_PATH" ]; then
    echo -e "${GREEN}✓ Models directory exists${NC}"
    ls -la "$MODEL_PATH" | head -5
else
    echo -e "${RED}✗ Models directory NOT found${NC}"
    exit 1
fi

# Test 3: Start llama-server
echo ""
echo "[Test 3] Starting llama-server..."
cd "$MODEL_PATH"
MODEL_FILE=$(ls *.gguf 2>/dev/null | head -1)

if [ -z "$MODEL_FILE" ]; then
    echo -e "${RED}✗ No GGUF model found${NC}"
    exit 1
fi

echo "Using model: $MODEL_FILE"

# Start server in background
llama-server --model "$MODEL_FILE" --host 127.0.0.1 --port 8080 --ctx-size 2048 --threads 4 &
SERVER_PID=$!
sleep 5

# Test 4: Health check
echo ""
echo "[Test 4] Testing health endpoint..."
if curl -s http://localhost:8080/health > /dev/null 2>&1; then
    echo -e "${GREEN}✓ Health check passed${NC}"
else
    echo -e "${RED}✗ Health check failed${NC}"
    kill $SERVER_PID 2>/dev/null
    exit 1
fi

# Test 5: Real inference
echo ""
echo "[Test 5] Testing real AI inference..."
RESPONSE=$(curl -s -X POST http://localhost:8080/v1/chat/completions \
    -H "Content-Type: application/json" \
    -d '{
        "model": "tinyllama",
        "messages": [{"role": "user", "content": "Say hello in 3 words!"}],
        "max_tokens": 20
    }')

if echo "$RESPONSE" | grep -q "content"; then
    echo -e "${GREEN}✓ AI Inference working!${NC}"
    echo "Response:"
    echo "$RESPONSE" | grep -o '"content":"[^"]*"' | cut -d'"' -f4 | head -c 200
    echo ""
else
    echo -e "${RED}✗ Inference failed${NC}"
    echo "Response: $RESPONSE"
    kill $SERVER_PID 2>/dev/null
    exit 1
fi

# Test 6: Check aura-neocortex
echo ""
echo "[Test 6] Checking aura-neocortex..."
if command -v aura-neocortex &> /dev/null; then
    echo -e "${GREEN}✓ aura-neocortex found${NC}"
    aura-neocortex --version 2>/dev/null || aura-neocortex --help 2>/dev/null | head -1
else
    echo -e "${YELLOW}⚠ aura-neocortex NOT found (run install.sh)${NC}"
fi

# Cleanup
echo ""
echo "[Cleanup] Stopping llama-server..."
kill $SERVER_PID 2>/dev/null || true
sleep 1

echo ""
echo "========================================"
echo -e "${GREEN}  All Tests Passed!${NC}"
echo "========================================"
