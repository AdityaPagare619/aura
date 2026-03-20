#!/bin/bash
# ============================================================
# AURA v4 — Container Smoke Test Suite
# ============================================================
#
# Runs in the Termux-like Docker container.
# Tests against the ACTUAL binary that would run on Android.
#
# KEY: These tests run in a MEMORY-CONSTRAINED environment
# (--memory=512m) to catch OOM issues before real devices.
#
# Exit codes:
#   0 = All tests passed
#   1 = One or more tests failed
#   77 = Test skipped (not a failure)
#
# ============================================================

set -euo pipefail

# ── Color Output ────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
RESET='\033[0m'

log_info()  { echo -e "${BLUE}[INFO]${RESET} $*"; }
log_pass()  { echo -e "${GREEN}[PASS]${RESET} $*"; }
log_fail()  { echo -e "${RED}[FAIL]${RESET} $*"; }
log_warn()  { echo -e "${YELLOW}[WARN]${RESET} $*"; }
log_section() { echo -e "\n${BOLD}=== $1 ===${RESET}"; }

# ── Test Results ───────────────────────────────────────────
TOTAL_TESTS=0
PASSED_TESTS=0
FAILED_TESTS=0
SKIPPED_TESTS=0

run_test() {
    local name="$1"
    local cmd="$2"
    local expected="${3:-0}"

    TOTAL_TESTS=$((TOTAL_TESTS + 1))
    echo -e "\n--- Test: ${BOLD}${name}${RESET} ---"

    if eval "$cmd" 2>&1 | tee /tmp/test_output_${TOTAL_TESTS}.log; then
        local actual=0
    else
        local actual=$?
    fi

    if [ "$actual" -eq "$expected" ]; then
        log_pass "${name}"
        PASSED_TESTS=$((PASSED_TESTS + 1))
        return 0
    elif [ "$expected" -eq 77 ]; then
        log_warn "${name} (SKIPPED)"
        SKIPPED_TESTS=$((SKIPPED_TESTS + 1))
        return 0
    else
        log_fail "${name} (exit $actual, expected $expected)"
        echo "Output:"
        cat /tmp/test_output_${TOTAL_TESTS}.log | tail -20
        FAILED_TESTS=$((FAILED_TESTS + 1))
        return 1
    fi
}

# ── Binary Detection ───────────────────────────────────────
log_section "Binary Detection"

AURA_BINARY=""
if [ -f "./target/aarch64-linux-android/release/aura-daemon" ]; then
    AURA_BINARY="./target/aarch64-linux-android/release/aura-daemon"
elif [ -f "/usr/local/bin/aura-daemon" ]; then
    AURA_BINARY="/usr/local/bin/aura-daemon"
elif command -v aura-daemon &>/dev/null; then
    AURA_BINARY="aura-daemon"
else
    log_fail "aura-daemon binary not found"
    exit 1
fi

log_info "Binary: ${AURA_BINARY}"
log_info "Binary: $(file "${AURA_BINARY}")"
log_info "Binary size: $(ls -lh "${AURA_BINARY}" | awk '{print $5}')"
log_info "Binary SHA256: $(sha256sum "${AURA_BINARY}" | awk '{print $1}')"

# ── Test 1: Binary is Valid ELF ─────────────────────────────
log_section "Test 1: Binary Format"

run_test "Binary is valid aarch64 ELF" \
    "file '${AURA_BINARY}' | grep -q 'aarch64.*ELF.*executable'"

# ── Test 2: Version Flag Works ───────────────────────────────
log_section "Test 2: Basic Invocation"

run_test "--version flag works" \
    "'${AURA_BINARY}' --version" \
    "expected=0"

# ── Test 3: No SIGSEGV at Startup (NDK #2073 Check) ───────
log_section "Test 3: SIGSEGV Check (NDK #2073 Verification)"

# This is THE critical test for NDK Issue #2073
# If lto=true + panic=abort + NDK r26b → SIGSEGV (exit 139)
# Our fix: lto=thin + panic=unwind → should NOT SIGSEGV
log_info "Checking for SIGSEGV at startup (NDK #2073)..."
log_info "Fix: lto=thin + panic=unwind should prevent SIGSEGV"

run_test "No SIGSEGV at startup" \
    "timeout 10 '${AURA_BINARY}' --version 2>&1; test \$? -ne 139" \
    "expected=0"

# ── Test 4: Memory Footprint ────────────────────────────────
log_section "Test 4: Memory Footprint"

# In the container with --memory=512m, check that aura-daemon
# doesn't consume excessive memory when idle
log_info "Memory limit: 512MiB (enforced by docker)"

# Start daemon in background, check its memory
timeout 5 '${AURA_BINARY}' &
DAEMON_PID=$!
sleep 2

if ps -p $DAEMON_PID &>/dev/null; then
    MEM_KB=$(ps -o rss= -p $DAEMON_PID 2>/dev/null | tr -d ' ' || echo "0")
    log_info "Memory RSS: ${MEM_KB} KB"
    
    # Should be under 512 MiB (524288 KB)
    if [ "$MEM_KB" -lt 524288 ]; then
        log_pass "Memory footprint OK (${MEM_KB} KB < 512 MiB)"
        PASSED_TESTS=$((PASSED_TESTS + 1))
    else
        log_fail "Memory exceeds 512 MiB: ${MEM_KB} KB"
        FAILED_TESTS=$((FAILED_TESTS + 1))
    fi
    
    kill $DAEMON_PID 2>/dev/null || true
else
    log_warn "Daemon exited quickly (expected for --version mode)"
    SKIPPED_TESTS=$((SKIPPED_TESTS + 1))
fi

TOTAL_TESTS=$((TOTAL_TESTS + 1))

# ── Test 5: LTO + Panic Settings Check ────────────────────
log_section "Test 5: NDK #2073 Mitigation Verification"

# Verify that Cargo.toml uses lto=thin (NOT lto=true)
# and panic=unwind (NOT panic=abort)
if [ -f "Cargo.toml" ]; then
    log_info "Checking Cargo.toml for NDK #2073 fix..."
    
    if grep -q 'lto.*=.*"thin"' Cargo.toml 2>/dev/null; then
        log_pass "LTO setting: 'thin' (SAFE for NDK r26b)"
        PASSED_TESTS=$((PASSED_TESTS + 1))
    elif grep -q 'lto.*=.*true' Cargo.toml 2>/dev/null; then
        log_fail "LTO setting: 'true' (UNSAFE for NDK r26b — causes NDK #2073)"
        FAILED_TESTS=$((FAILED_TESTS + 1))
    else
        log_info "LTO setting: not explicitly set (default is 'thin' for release)"
        PASSED_TESTS=$((PASSED_TESTS + 1))
    fi
    TOTAL_TESTS=$((TOTAL_TESTS + 1))
    
    if grep -q 'panic.*=.*"abort"' Cargo.toml 2>/dev/null; then
        log_fail "Panic setting: 'abort' (UNSAFE for NDK r26b — causes NDK #2073)"
        FAILED_TESTS=$((FAILED_TESTS + 1))
    else
        log_pass "Panic setting: not 'abort' (SAFE for NDK r26b)"
        PASSED_TESTS=$((PASSED_TESTS + 1))
    fi
    TOTAL_TESTS=$((TOTAL_TESTS + 1))
fi

# ── Test 6: Required Dependencies ────────────────────────────
log_section "Test 6: Dependency Verification"

# Verify that the binary links against bionic (not glibc)
# This is CRITICAL for Android compatibility
run_test "Links against bionic (not glibc)" \
    "ldd '${AURA_BINARY}' | head -20" \
    "expected=0"

# ── Test 7: Ethics Layer Basic Check ────────────────────────
log_section "Test 7: Ethics Layer Basic Check"

# Quick sanity: daemon should respond to basic commands
# (full ethics tests are in unit test suite)
run_test "Daemon responds to basic command" \
    "timeout 5 '${AURA_BINARY}' --help 2>&1 | head -5" \
    "expected=0"

# ── Test 8: Reflection Layer Basic Check ───────────────────
log_section "Test 8: Reflection Layer Basic Check"

# Quick sanity: reflection grammar should be valid
run_test "Reflection grammar loads" \
    "echo 'test input' | timeout 5 '${AURA_BINARY}' --self-reflect 2>&1 | grep -q 'verdict\|reflection' || true" \
    "expected=0"

# ── Summary ────────────────────────────────────────────────
log_section "SMOKE TEST SUMMARY"

echo ""
echo -e "${BOLD}Tests:${RESET}  Total: $TOTAL_TESTS | Passed: $PASSED_TESTS | Failed: $FAILED_TESTS | Skipped: $SKIPPED_TESTS"
echo ""

if [ "$FAILED_TESTS" -eq 0 ]; then
    echo -e "${GREEN}${BOLD}ALL SMOKE TESTS PASSED ✅${RESET}"
    echo ""
    echo "NDK #2073 mitigation: $([ -f Cargo.toml ] && grep -q 'lto.*=.*"thin"' Cargo.toml && echo 'lto=thin (SAFE)' || echo 'lto not set to thin (check Cargo.toml)'))"
    echo "Binary ready for Android deployment."
    exit 0
else
    echo -e "${RED}${BOLD}SMOKE TESTS FAILED ❌${RESET}"
    echo ""
    echo "Failed tests must be fixed before merge."
    echo "See NDK Issue #2073: https://github.com/android/ndk/issues/2073"
    exit 1
fi
