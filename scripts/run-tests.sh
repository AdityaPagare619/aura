#!/usr/bin/env bash
# =============================================================================
# AURA v4 — Test Runner Script
# =============================================================================
# Runs all test suites and reports pass/fail with counts.
# Designed for both host (Linux/macOS/Windows+WSL) and Termux environments.
#
# Usage:
#   ./scripts/run-tests.sh              # Run all tests
#   ./scripts/run-tests.sh unit         # Unit tests only
#   ./scripts/run-tests.sh integration  # Integration tests only
#   ./scripts/run-tests.sh security     # Security tests only
#   ./scripts/run-tests.sh all          # All tests (default)
# =============================================================================

set -euo pipefail

# ─── Colors ──────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# ─── Counters ────────────────────────────────────────────────────────────────
TOTAL_PASS=0
TOTAL_FAIL=0
TOTAL_SKIP=0
TOTAL_SUITE=0

# ─── Helpers ─────────────────────────────────────────────────────────────────
log_info()  { echo -e "${BLUE}[INFO]${NC}  $*"; }
log_pass()  { echo -e "${GREEN}[PASS]${NC}  $*"; }
log_fail()  { echo -e "${RED}[FAIL]${NC}  $*"; }
log_skip()  { echo -e "${YELLOW}[SKIP]${NC}  $*"; }
log_header() { echo -e "\n${BLUE}═══════════════════════════════════════════════════${NC}"; echo -e "${BLUE}  $*${NC}"; echo -e "${BLUE}═══════════════════════════════════════════════════${NC}"; }

run_suite() {
    local name="$1"
    shift
    local cmd="$*"

    TOTAL_SUITE=$((TOTAL_SUITE + 1))
    log_header "$name"
    echo "  Command: $cmd"
    echo "  ---"

    local output
    local exit_code=0
    output=$($cmd 2>&1) || exit_code=$?

    if [ $exit_code -eq 0 ]; then
        # Count passes/fails/skips from cargo test output
        local passes fails ignores
        passes=$(echo "$output" | grep -oP '\d+(?= test(s)? passed)' | tail -1 || echo "0")
        fails=$(echo "$output" | grep -oP '\d+(?= test(s)? failed)' | tail -1 || echo "0")
        ignores=$(echo "$output" | grep -oP '\d+(?= test(s)? ignored)' | tail -1 || echo "0")

        passes=${passes:-0}
        fails=${fails:-0}
        ignores=${ignores:-0}

        TOTAL_PASS=$((TOTAL_PASS + passes))
        TOTAL_FAIL=$((TOTAL_FAIL + fails))
        TOTAL_SKIP=$((TOTAL_SKIP + ignores))

        if [ "$fails" -eq 0 ]; then
            log_pass "$name — ${passes} passed, ${ignores} ignored"
        else
            log_fail "$name — ${passes} passed, ${fails} FAILED, ${ignores} ignored"
            echo "$output" | tail -30
        fi
    else
        TOTAL_FAIL=$((TOTAL_FAIL + 1))
        log_fail "$name — EXIT CODE $exit_code"
        echo "$output" | tail -30
    fi
}

# ─── Pre-flight checks ───────────────────────────────────────────────────────
log_info "AURA v4 Test Runner"
log_info "Platform: $(uname -s 2>/dev/null || echo 'Windows/unknown')"
log_info "Rust: $(rustc --version 2>/dev/null || echo 'not found')"
log_info "Cargo: $(cargo --version 2>/dev/null || echo 'not found')"

# Check we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    log_fail "Cargo.toml not found — run from the AURA project root"
    exit 1
fi

# ─── Test Suites ─────────────────────────────────────────────────────────────
MODE="${1:-all}"

case "$MODE" in
    unit|all)
        run_suite "Unit Tests (all crates)" \
            cargo test --lib --quiet -- --test-threads=4
        ;;
    integration|all)
        run_suite "Integration Tests" \
            cargo test --test '*' --quiet -- --test-threads=1
        ;;
    security|all)
        run_suite "Security Tests (FFI Safety)" \
            cargo test --test test_ffi_safety --quiet -- --test-threads=1
        ;;
    platform|all)
        run_suite "Platform Tests" \
            cargo test --test test_platform --quiet -- --test-threads=1 2>/dev/null || true
        ;;
    telegram|all)
        run_suite "Telegram Tests" \
            cargo test --test test_telegram --quiet -- --test-threads=1 2>/dev/null || true
        ;;
    ipc|all)
        run_suite "IPC Edge Case Tests" \
            cargo test --test test_ipc_edge_cases --quiet -- --test-threads=1 2>/dev/null || true
        ;;
    memory|all)
        run_suite "Memory Management Tests" \
            cargo test --test test_memory_management --quiet -- --test-threads=1 2>/dev/null || true
        ;;
    *)
        echo "Usage: $0 {unit|integration|security|platform|telegram|ipc|memory|all}"
        exit 1
        ;;
esac

# ─── Summary ─────────────────────────────────────────────────────────────────
log_header "TEST SUMMARY"
echo ""
echo -e "  Suites run:    ${TOTAL_SUITE}"
echo -e "  ${GREEN}Passed:${NC}        ${TOTAL_PASS}"
echo -e "  ${RED}Failed:${NC}        ${TOTAL_FAIL}"
echo -e "  ${YELLOW}Skipped:${NC}       ${TOTAL_SKIP}"
echo ""

if [ "$TOTAL_FAIL" -eq 0 ]; then
    echo -e "  ${GREEN}✓ ALL TESTS PASSED${NC}"
    exit 0
else
    echo -e "  ${RED}✗ ${TOTAL_FAIL} TEST(S) FAILED${NC}"
    exit 1
fi
