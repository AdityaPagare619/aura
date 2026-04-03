#!/data/data/com.termux/files/usr/bin/bash
# =============================================================================
# End-to-end test for HTTP backend → llama-server flow
# Tests: Check if llama-server is running → Send prompt → Verify AI response
# =============================================================================

set -e

# Configuration
PORT=8080
BASE_URL="http://localhost:${PORT}"
MODEL_NAME="tinyllama"
TIMEOUT_SECS=30
TEST_PROMPT="Hello, how are you?"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Test counters
PASSED=0
FAILED=0

log_info() {
    echo -e "${YELLOW}[INFO]${NC} $1"
}

log_pass() {
    echo -e "${GREEN}[PASS]${NC} $1"
    ((PASSED++))
}

log_fail() {
    echo -e "${RED}[FAIL]${NC} $1"
    ((FAILED++))
}

echo "=============================================="
echo "  HTTP Backend → llama-server E2E Test"
echo "=============================================="
echo ""

# -----------------------------------------------------------------------------
# Step 1: Check if llama-server is running on port 8080
# -----------------------------------------------------------------------------
log_info "Step 1: Checking if llama-server is running on port ${PORT}..."

if curl -s --connect-timeout 2 "http://localhost:${PORT}/health" > /dev/null 2>&1; then
    log_pass "llama-server is listening on port ${PORT}"
elif curl -s --connect-timeout 2 "http://localhost:${PORT}/v1/models" > /dev/null 2>&1; then
    log_pass "llama-server is listening on port ${PORT}"
else
    log_fail "llama-server is NOT running on port ${PORT}"
    echo ""
    echo "To start llama-server, run:"
    echo "  cd /data/local/tmp/llama"
    echo "  ./llama-server --model /data/local/tmp/aura/models/model.gguf --port ${PORT} --host 0.0.0.0 -c 2048 -ngl 0 &"
    echo ""
    echo "Or use the project's script:"
    echo "  ./run-llama.sh"
    exit 1
fi

# -----------------------------------------------------------------------------
# Step 2: Verify the /v1/models endpoint works
# -----------------------------------------------------------------------------
log_info "Step 2: Checking /v1/models endpoint..."

MODELS_RESPONSE=$(curl -s --max-time "${TIMEOUT_SECS}" "${BASE_URL}/v1/models" 2>&1)
if [ $? -ne 0 ]; then
    log_fail "Failed to connect to /v1/models: ${MODELS_RESPONSE}"
    exit 1
fi

if echo "${MODELS_RESPONSE}" | grep -q "model"; then
    log_pass "/v1/models endpoint is accessible"
else
    log_fail "/v1/models endpoint returned unexpected response: ${MODELS_RESPONSE}"
    exit 1
fi

# -----------------------------------------------------------------------------
# Step 3: Send test prompt to /v1/chat/completions
# -----------------------------------------------------------------------------
log_info "Step 3: Sending test prompt to /v1/chat/completions..."
log_info "Prompt: \"${TEST_PROMPT}\""

# Build the JSON payload
JSON_PAYLOAD=$(cat <<EOF
{
  "model": "${MODEL_NAME}",
  "messages": [
    {"role": "user", "content": "${TEST_PROMPT}"}
  ],
  "max_tokens": 50,
  "temperature": 0.7
}
EOF
)

# Send the request
AI_RESPONSE=$(curl -s --max-time "${TIMEOUT_SECS}" \
    -X POST "${BASE_URL}/v1/chat/completions" \
    -H "Content-Type: application/json" \
    -d "${JSON_PAYLOAD}" 2>&1)

CURL_EXIT_CODE=$?

if [ ${CURL_EXIT_CODE} -ne 0 ]; then
    log_fail "curl failed with exit code ${CURL_EXIT_CODE}: ${AI_RESPONSE}"
    exit 1
fi

# -----------------------------------------------------------------------------
# Step 4: Verify response is a real AI response (not error)
# -----------------------------------------------------------------------------
log_info "Step 4: Verifying AI response..."

# Check for error responses
if echo "${AI_RESPONSE}" | grep -qi "error"; then
    log_fail "Received error response: ${AI_RESPONSE}"
    exit 1
fi

# Check for empty response
if [ -z "${AI_RESPONSE}" ]; then
    log_fail "Empty response received"
    exit 1
fi

# Check for valid JSON with content
if echo "${AI_RESPONSE}" | grep -q '"content"'; then
    # Extract the content for display
    CONTENT=$(echo "${AI_RESPONSE}" | grep -o '"content"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*"content"[[:space:]]*:[[:space:]]*"\(.*\)"/\1/')
    log_pass "Received valid AI response"
    log_info "AI said: ${CONTENT:0:100}..."
else
    # Check if we got a different valid format
    if echo "${AI_RESPONSE}" | grep -q '"choices"'; then
        log_pass "Received valid AI response (choices format)"
    else
        log_fail "Response doesn't contain expected AI content: ${AI_RESPONSE}"
        exit 1
    fi
fi

# -----------------------------------------------------------------------------
# Step 5: Additional validation - check response structure
# -----------------------------------------------------------------------------
log_info "Step 5: Validating response structure..."

# Check for required fields in OpenAI-compatible response
if echo "${AI_RESPONSE}" | grep -q '"id"' && \
   echo "${AI_RESPONSE}" | grep -q '"object"' && \
   echo "${AI_RESPONSE}" | grep -q '"model"'; then
    log_pass "Response has correct OpenAI-compatible structure"
else
    log_fail "Response missing required fields: ${AI_RESPONSE}"
    exit 1
fi

# -----------------------------------------------------------------------------
# Summary
# -----------------------------------------------------------------------------
echo ""
echo "=============================================="
echo "  TEST RESULTS"
echo "=============================================="
echo -e "  ${GREEN}PASSED: ${PASSED}${NC}"
echo -e "  ${RED}FAILED: ${FAILED}${NC}"
echo "=============================================="

if [ ${FAILED} -eq 0 ]; then
    echo -e "${GREEN}All tests passed! HTTP backend is working correctly.${NC}"
    exit 0
else
    echo -e "${RED}Some tests failed. Check the output above.${NC}"
    exit 1
fi
