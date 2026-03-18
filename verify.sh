#!/data/data/com.termux/files/usr/bin/bash
# ============================================================================
# AURA v4 — Operational Verification Script
# ============================================================================
#
# Run this after install.sh to verify EVERY component is functional.
#
# Usage:
#   bash verify.sh              # Full verification
#   bash verify.sh --quick      # Skip slow checks (model load, Telegram E2E)
#
# Exit codes:
#   0 — All checks passed
#   1 — One or more checks failed
#
# ============================================================================

set -euo pipefail

# ── Colors ──────────────────────────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
NC='\033[0m'

# ── State ───────────────────────────────────────────────────────────────────

PASS=0
FAIL=0
WARN=0
SKIP=0
QUICK=0

[[ "${1:-}" == "--quick" ]] && QUICK=1

# ── Paths ───────────────────────────────────────────────────────────────────

PREFIX="${PREFIX:-/data/data/com.termux/files/usr}"
HOME_DIR="${HOME:-/data/data/com.termux/files/home}"
AURA_CONFIG_DIR="$HOME_DIR/.config/aura"
AURA_CONFIG_FILE="$AURA_CONFIG_DIR/config.toml"
AURA_DATA_DIR="$HOME_DIR/.local/share/aura"
AURA_MODELS_DIR="$AURA_DATA_DIR/models"
AURA_LOGS_DIR="$AURA_DATA_DIR/logs"
AURA_DB_DIR="$AURA_DATA_DIR/db"
AURA_BIN="$PREFIX/bin/aura-daemon"
AURA_NEOCORTEX_BIN="$PREFIX/bin/aura-neocortex"
AURA_SV_DIR="$PREFIX/var/service/aura-daemon"

# ── Helpers ─────────────────────────────────────────────────────────────────

pass() { ((PASS++)); echo -e "  ${GREEN}✓${NC} $1"; }
fail() { ((FAIL++)); echo -e "  ${RED}✗${NC} $1"; }
warn() { ((WARN++)); echo -e "  ${YELLOW}⚠${NC} $1"; }
skip() { ((SKIP++)); echo -e "  ${BLUE}⏭${NC} $1 (skipped)"; }
header() { echo -e "\n${BOLD}═══ $1 ═══${NC}"; }

# ── Check helpers ───────────────────────────────────────────────────────────

check_file_exists() {
    local path="$1" label="$2"
    if [ -f "$path" ]; then
        pass "$label exists: $path"
        return 0
    else
        fail "$label missing: $path"
        return 1
    fi
}

check_file_executable() {
    local path="$1" label="$2"
    if [ -x "$path" ]; then
        pass "$label is executable"
        return 0
    else
        fail "$label is NOT executable: $path"
        return 1
    fi
}

check_dir_exists() {
    local path="$1" label="$2"
    if [ -d "$path" ]; then
        pass "$label directory exists: $path"
        return 0
    else
        fail "$label directory missing: $path"
        return 1
    fi
}

# ============================================================================
# SECTION 1: PRE-FLIGHT CHECKS
# ============================================================================

header "1. Pre-Flight Checks"

# 1a. Binary
if check_file_exists "$AURA_BIN" "aura-daemon binary"; then
    check_file_executable "$AURA_BIN" "aura-daemon binary"

    # Verify ARM64
    binary_arch=$(file "$AURA_BIN" 2>/dev/null || true)
    if echo "$binary_arch" | grep -qi 'aarch64\|ARM aarch64'; then
        pass "Binary architecture: ARM64 (aarch64)"
    elif echo "$binary_arch" | grep -qi 'ELF'; then
        warn "Binary is ELF but could not confirm ARM64: $binary_arch"
    else
        fail "Binary architecture unexpected: $binary_arch"
    fi

    # Version check
    bin_version=$("$AURA_BIN" --version 2>/dev/null || echo "unknown")
    if echo "$bin_version" | grep -q '4\.0\.0'; then
        pass "Binary version: $bin_version"
    else
        warn "Binary version unexpected: $bin_version"
    fi
fi

if check_file_exists "$AURA_NEOCORTEX_BIN" "aura-neocortex binary"; then
    check_file_executable "$AURA_NEOCORTEX_BIN" "aura-neocortex binary"

    # Link/runtime check catches missing shared libs (e.g. libc++_shared.so).
    neocortex_probe=$("$AURA_NEOCORTEX_BIN" --help 2>&1 || true)
    if echo "$neocortex_probe" | grep -qi 'CANNOT LINK EXECUTABLE'; then
        fail "aura-neocortex runtime link failed: $(echo "$neocortex_probe" | head -1)"
    elif echo "$neocortex_probe" | grep -qi 'aura-neocortex'; then
        pass "aura-neocortex responds to --help"
    else
        warn "aura-neocortex probe output unexpected (check manually with --help)"
    fi
fi

# 1b. Config file
if check_file_exists "$AURA_CONFIG_FILE" "Config file"; then
    # Check permissions (should be 600)
    config_perms=$(stat -c '%a' "$AURA_CONFIG_FILE" 2>/dev/null || stat -f '%Lp' "$AURA_CONFIG_FILE" 2>/dev/null || echo "unknown")
    if [ "$config_perms" = "600" ]; then
        pass "Config file permissions: 600 (restricted)"
    else
        warn "Config file permissions: $config_perms (should be 600 — contains bot token)"
    fi

    # Validate TOML syntax
    if command -v python3 &>/dev/null; then
        if python3 -c "
import sys
try:
    # Python 3.11+ has tomllib
    import tomllib
    with open('$AURA_CONFIG_FILE', 'rb') as f:
        tomllib.load(f)
    sys.exit(0)
except ImportError:
    # Fallback: basic check that it's not obviously broken
    with open('$AURA_CONFIG_FILE') as f:
        content = f.read()
    if '[daemon]' in content and '[neocortex]' in content:
        sys.exit(0)
    sys.exit(1)
except Exception as e:
    print(str(e), file=sys.stderr)
    sys.exit(1)
" 2>/dev/null; then
            pass "Config TOML syntax: valid"
        else
            fail "Config TOML syntax: INVALID"
        fi
    else
        skip "TOML syntax check (python3 not available)"
    fi

    # Check critical config fields
    if grep -q 'enabled.*=.*true' "$AURA_CONFIG_FILE" 2>/dev/null; then
        pass "Telegram enabled = true in config"
    else
        fail "Telegram NOT enabled in config (missing 'enabled = true')"
    fi

    if grep -q 'bot_token.*=.*"[^"]\+"' "$AURA_CONFIG_FILE" 2>/dev/null; then
        pass "Telegram bot_token is set"
    else
        fail "Telegram bot_token is empty or missing"
    fi

    if grep -q 'allowed_chat_ids.*=.*\[' "$AURA_CONFIG_FILE" 2>/dev/null; then
        pass "Telegram allowed_chat_ids field present"
    else
        fail "Telegram allowed_chat_ids MISSING (Rust expects this, NOT allowed_user_ids)"
    fi

    if grep -q 'default_model_path' "$AURA_CONFIG_FILE" 2>/dev/null; then
        pass "Neocortex default_model_path field present"
    else
        fail "Neocortex default_model_path MISSING (Rust expects this, NOT model_file)"
    fi

    if grep -q 'data_dir' "$AURA_CONFIG_FILE" 2>/dev/null; then
        pass "Daemon data_dir field present"
    else
        warn "Daemon data_dir not in config — will use compiled default (/data/data/com.aura/files)"
    fi

    if grep -q 'db_path' "$AURA_CONFIG_FILE" 2>/dev/null; then
        pass "SQLite db_path field present"
    else
        fail "SQLite db_path MISSING"
    fi
fi

# 1c. Directories
check_dir_exists "$AURA_DATA_DIR" "Data"
check_dir_exists "$AURA_MODELS_DIR" "Models"
check_dir_exists "$AURA_LOGS_DIR" "Logs"
check_dir_exists "$AURA_DB_DIR" "Database"

# ============================================================================
# SECTION 2: MODEL VERIFICATION
# ============================================================================

header "2. Model Verification"

# Find GGUF model file
model_found=0
model_file=""
model_size=0

if [ -d "$AURA_MODELS_DIR" ]; then
    for f in "$AURA_MODELS_DIR"/*.gguf; do
        if [ -f "$f" ]; then
            model_found=1
            model_file="$f"
            model_size=$(stat -c '%s' "$f" 2>/dev/null || stat -f '%z' "$f" 2>/dev/null || echo 0)
            break
        fi
    done
fi

if [ "$model_found" -eq 1 ]; then
    pass "GGUF model found: $(basename "$model_file")"

    # Size check — a real GGUF model should be > 500 MB
    model_size_mb=$((model_size / 1048576))
    if [ "$model_size_mb" -gt 500 ]; then
        pass "Model size: ${model_size_mb} MB (looks real)"
    elif [ "$model_size_mb" -gt 0 ]; then
        warn "Model size: ${model_size_mb} MB (suspiciously small for a GGUF model)"
    else
        fail "Model file is empty or unreadable"
    fi

    # GGUF magic bytes check: first 4 bytes should be "GGUF" (0x47 0x47 0x55 0x46)
    if command -v xxd &>/dev/null; then
        magic=$(xxd -l 4 -p "$model_file" 2>/dev/null || echo "")
        if [ "$magic" = "47475546" ]; then
            pass "GGUF magic bytes: valid"
        else
            fail "GGUF magic bytes: INVALID (got: $magic, expected: 47475546)"
        fi
    elif command -v od &>/dev/null; then
        magic=$(od -A n -t x1 -N 4 "$model_file" 2>/dev/null | tr -d ' \n' || echo "")
        if [ "$magic" = "47475546" ]; then
            pass "GGUF magic bytes: valid"
        else
            fail "GGUF magic bytes: INVALID (got: $magic, expected: 47475546)"
        fi
    else
        skip "GGUF magic bytes check (xxd/od not available)"
    fi

    # Verify config points to this model
    if grep -q "$(basename "$model_file")" "$AURA_CONFIG_FILE" 2>/dev/null; then
        pass "Config references this model file"
    else
        warn "Config may not reference $(basename "$model_file") — check default_model_path"
    fi
else
    fail "No GGUF model found in $AURA_MODELS_DIR"
fi

# ============================================================================
# SECTION 3: NETWORK & TELEGRAM API
# ============================================================================

header "3. Network & Telegram API"

# 3a. Basic network
if ping -c 1 -W 3 api.telegram.org &>/dev/null 2>&1; then
    pass "Network: api.telegram.org reachable"
else
    if curl -s --max-time 5 https://api.telegram.org/ &>/dev/null 2>&1; then
        pass "Network: api.telegram.org reachable (via HTTPS)"
    else
        fail "Network: api.telegram.org UNREACHABLE"
    fi
fi

# 3b. Telegram API /getMe
bot_token=""
if [ -f "$AURA_CONFIG_FILE" ]; then
    bot_token=$(grep 'bot_token' "$AURA_CONFIG_FILE" 2>/dev/null | head -1 | sed 's/.*=\s*"\(.*\)".*/\1/' || echo "")
fi

if [ -n "$bot_token" ] && [ "$bot_token" != "" ]; then
    getme_response=$(curl -s --max-time 10 "https://api.telegram.org/bot${bot_token}/getMe" 2>/dev/null || echo "")
    if echo "$getme_response" | grep -q '"ok":true'; then
        bot_username=$(echo "$getme_response" | grep -o '"username":"[^"]*"' | head -1 | cut -d'"' -f4)
        pass "Telegram API /getMe: OK (bot: @$bot_username)"
    elif echo "$getme_response" | grep -q '"ok":false'; then
        error_desc=$(echo "$getme_response" | grep -o '"description":"[^"]*"' | head -1 | cut -d'"' -f4)
        fail "Telegram API /getMe: FAILED ($error_desc)"
    else
        fail "Telegram API /getMe: no response (network issue?)"
    fi
else
    fail "Cannot test Telegram API — bot_token not found in config"
fi

# ============================================================================
# SECTION 4: SERVICE SETUP
# ============================================================================

header "4. Service Setup"

if [ -d "$AURA_SV_DIR" ]; then
    pass "Service directory exists: $AURA_SV_DIR"

    if [ -f "$AURA_SV_DIR/run" ] && [ -x "$AURA_SV_DIR/run" ]; then
        pass "Service run script exists and is executable"
    else
        fail "Service run script missing or not executable"
    fi

    if [ -f "$AURA_SV_DIR/log/run" ] && [ -x "$AURA_SV_DIR/log/run" ]; then
        pass "Log service run script exists and is executable"
    else
        warn "Log service run script missing (logs may not rotate)"
    fi

    # Check if service is running
    if command -v sv &>/dev/null; then
        sv_status=$(sv status aura-daemon 2>/dev/null || echo "unknown")
        if echo "$sv_status" | grep -q 'run:'; then
            pass "Service status: RUNNING ($sv_status)"
        elif echo "$sv_status" | grep -q 'down:'; then
            warn "Service status: DOWN ($sv_status) — start with: sv up aura-daemon"
        else
            warn "Service status: $sv_status"
        fi
    else
        warn "sv command not available — cannot check service status"
    fi
else
    warn "Service directory not found — service not configured (manual start OK)"
fi

# ============================================================================
# SECTION 5: DAEMON STARTUP TEST
# ============================================================================

header "5. Daemon Startup Test"

# Check if daemon is already running
daemon_pid=$(pgrep -f 'aura-daemon' 2>/dev/null | head -1 || echo "")

if [ -n "$daemon_pid" ]; then
    pass "Daemon is running (PID: $daemon_pid)"

    # Check how long it's been running
    if [ -d "/proc/$daemon_pid" ]; then
        uptime_ticks=$(cut -d' ' -f22 "/proc/$daemon_pid/stat" 2>/dev/null || echo "0")
        clk_tck=$(getconf CLK_TCK 2>/dev/null || echo "100")
        sys_uptime=$(cut -d' ' -f1 /proc/uptime 2>/dev/null || echo "0")
        if [ "$clk_tck" -gt 0 ] && [ "$uptime_ticks" -gt 0 ]; then
            start_secs=$((uptime_ticks / clk_tck))
            running_secs=$(echo "$sys_uptime - $start_secs" | bc 2>/dev/null || echo "unknown")
            pass "Daemon uptime: ~${running_secs}s"
        fi

        # RSS check
        rss_kb=$(awk '/VmRSS/ {print $2}' "/proc/$daemon_pid/status" 2>/dev/null || echo "0")
        rss_mb=$((rss_kb / 1024))
        if [ "$rss_mb" -lt 50 ]; then
            pass "Daemon RSS: ${rss_mb} MB (healthy)"
        elif [ "$rss_mb" -lt 100 ]; then
            warn "Daemon RSS: ${rss_mb} MB (elevated)"
        else
            fail "Daemon RSS: ${rss_mb} MB (too high — check for leaks)"
        fi
    fi

    # Check logs for panics
    if [ -d "$AURA_LOGS_DIR" ]; then
        recent_log=$(ls -t "$AURA_LOGS_DIR"/current 2>/dev/null || ls -t "$AURA_LOGS_DIR"/*.log 2>/dev/null | head -1 || echo "")
        if [ -n "$recent_log" ] && [ -f "$recent_log" ]; then
            if grep -qi 'panic\|PANIC\|thread.*panicked' "$recent_log" 2>/dev/null; then
                fail "PANIC detected in logs: $recent_log"
                grep -i 'panic' "$recent_log" | tail -3
            else
                pass "No panics in daemon logs"
            fi

            if grep -qi 'startup complete' "$recent_log" 2>/dev/null; then
                pass "Daemon completed all startup phases"
            fi

            # Check for health events (cron ticks indicate daemon is alive and scheduling)
            if grep -qi 'cron\|tick\|heartbeat\|health' "$recent_log" 2>/dev/null; then
                pass "Health/cron events present in logs"
            else
                warn "No health/cron events in logs yet (daemon may need more time)"
            fi

            # Check for telegram bridge spawn
            if grep -qi 'telegram.*bridge.*spawned\|telegram.*spawned' "$recent_log" 2>/dev/null; then
                pass "Telegram bridge spawned"
            else
                warn "Telegram bridge spawn not found in logs"
            fi
        else
            warn "No log files found in $AURA_LOGS_DIR"
        fi
    fi
else
    warn "Daemon is NOT running"

    if [ "$QUICK" -eq 0 ]; then
        echo -e "  ${BLUE}→${NC} Attempting test start (5 seconds)..."

        # Start daemon in background
        "$AURA_BIN" &>/tmp/aura-verify-startup.log &
        test_pid=$!

        sleep 5

        if kill -0 "$test_pid" 2>/dev/null; then
            pass "Daemon started and survived 5 seconds (PID: $test_pid)"

            # Check for panics in output
            if grep -qi 'panic\|PANIC' /tmp/aura-verify-startup.log 2>/dev/null; then
                fail "PANIC during startup — check /tmp/aura-verify-startup.log"
            else
                pass "No panics during test startup"
            fi

            # Check startup log
            if grep -qi 'startup complete' /tmp/aura-verify-startup.log 2>/dev/null; then
                pass "All startup phases completed"
            fi

            # Kill test instance
            kill "$test_pid" 2>/dev/null || true
            wait "$test_pid" 2>/dev/null || true
        else
            fail "Daemon exited within 5 seconds — check /tmp/aura-verify-startup.log"
            echo -e "  ${RED}Last 10 lines:${NC}"
            tail -10 /tmp/aura-verify-startup.log 2>/dev/null | sed 's/^/    /'
        fi
    else
        skip "Daemon startup test (--quick mode)"
    fi
fi

# ============================================================================
# SECTION 6: END-TO-END TELEGRAM TEST
# ============================================================================

header "6. End-to-End Telegram Test"

if [ "$QUICK" -eq 1 ]; then
    skip "Telegram E2E test (--quick mode)"
elif [ -z "$bot_token" ]; then
    skip "Telegram E2E test (no bot token)"
else
    # Extract owner chat ID from config
    owner_id=$(grep 'allowed_chat_ids' "$AURA_CONFIG_FILE" 2>/dev/null | grep -o '[0-9]\+' | head -1 || echo "")

    if [ -n "$owner_id" ]; then
        # Send a test message via Telegram API
        send_result=$(curl -s --max-time 10 \
            "https://api.telegram.org/bot${bot_token}/sendMessage" \
            -d "chat_id=${owner_id}" \
            -d "text=🔍 AURA verify.sh — connectivity test. If you see this, Telegram bridge can reach you." \
            2>/dev/null || echo "")

        if echo "$send_result" | grep -q '"ok":true'; then
            pass "Telegram sendMessage: OK (message sent to chat $owner_id)"
        else
            error_desc=$(echo "$send_result" | grep -o '"description":"[^"]*"' | head -1 | cut -d'"' -f4)
            if echo "$error_desc" | grep -qi 'chat not found\|bot was blocked'; then
                fail "Telegram sendMessage: $error_desc — user must /start the bot first"
            else
                fail "Telegram sendMessage: ${error_desc:-unknown error}"
            fi
        fi
    else
        warn "Cannot determine owner chat ID from config — skipping send test"
    fi
fi

# ============================================================================
# SECTION 7: RESOURCE CHECK
# ============================================================================

header "7. Resource Check"

# Available storage
avail_mb=$(df -m "$HOME_DIR" 2>/dev/null | awk 'NR==2 {print $4}' || echo "0")
if [ "$avail_mb" -gt 2000 ]; then
    pass "Available storage: ${avail_mb} MB"
elif [ "$avail_mb" -gt 500 ]; then
    warn "Available storage: ${avail_mb} MB (low — model downloads need space)"
else
    fail "Available storage: ${avail_mb} MB (critically low)"
fi

# Available RAM
if [ -f /proc/meminfo ]; then
    avail_ram_kb=$(awk '/MemAvailable/ {print $2}' /proc/meminfo 2>/dev/null || echo "0")
    avail_ram_mb=$((avail_ram_kb / 1024))
    if [ "$avail_ram_mb" -gt 2000 ]; then
        pass "Available RAM: ${avail_ram_mb} MB"
    elif [ "$avail_ram_mb" -gt 1000 ]; then
        warn "Available RAM: ${avail_ram_mb} MB (tight for 8B model)"
    else
        warn "Available RAM: ${avail_ram_mb} MB (may need smaller model)"
    fi
fi

# CPU info
if [ -f /proc/cpuinfo ]; then
    cpu_cores=$(grep -c '^processor' /proc/cpuinfo 2>/dev/null || echo "unknown")
    pass "CPU cores: $cpu_cores"
fi

# ============================================================================
# SUMMARY
# ============================================================================

header "VERIFICATION SUMMARY"

TOTAL=$((PASS + FAIL + WARN + SKIP))
echo ""
echo -e "  ${GREEN}Passed:${NC}  $PASS"
echo -e "  ${RED}Failed:${NC}  $FAIL"
echo -e "  ${YELLOW}Warnings:${NC} $WARN"
echo -e "  ${BLUE}Skipped:${NC} $SKIP"
echo -e "  ${BOLD}Total:${NC}   $TOTAL"
echo ""

if [ "$FAIL" -eq 0 ]; then
    echo -e "  ${GREEN}${BOLD}✓ ALL CHECKS PASSED${NC}"
    if [ "$WARN" -gt 0 ]; then
        echo -e "  ${YELLOW}(${WARN} warnings — review above)${NC}"
    fi
    echo ""
    exit 0
else
    echo -e "  ${RED}${BOLD}✗ ${FAIL} CHECK(S) FAILED${NC}"
    echo -e "  Review the failures above and fix before running AURA."
    echo ""
    exit 1
fi
