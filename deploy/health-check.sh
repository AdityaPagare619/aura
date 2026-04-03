#!/usr/bin/env bash
# ============================================================================
# AURA v4 — Deployment Health Check
# ============================================================================
#
# Lightweight health check for monitoring / cron / automated deploys.
# Unlike verify.sh (full post-install verification), this script checks
# runtime health of a running AURA deployment.
#
# Usage:
#   bash deploy/health-check.sh          # Human-readable output
#   bash deploy/health-check.sh --json   # Machine-readable JSON output
#
# Exit codes:
#   0 — All health checks passed
#   1 — One or more checks failed
#
# ============================================================================

set -euo pipefail

# ── Config ───────────────────────────────────────────────────────────────────

if [ -d "/data/data/com.termux/files/usr" ]; then
    IS_TERMUX=1
    HOME_DIR="/data/data/com.termux/files/home"
    PREFIX="/data/data/com.termux/files/usr"
else
    IS_TERMUX=0
    HOME_DIR="${HOME:-/home/user}"
    PREFIX="${PREFIX:-/usr}"
fi

AURA_DATA_DIR="${AURA_DATA_DIR:-$HOME_DIR/.local/share/aura}"
AURA_DB_PATH="${AURA_DB_PATH:-$AURA_DATA_DIR/db/aura.db}"
AURA_MODELS_DIR="${AURA_MODELS_PATH:-$AURA_DATA_DIR/models}"
IPC_SOCKET="${AURA_IPC_SOCKET:-@aura_ipc_v4}"
LOG_DIR="$AURA_DATA_DIR/logs"
CONFIG_FILE="${AURA_CONFIG_FILE:-$HOME_DIR/.config/aura/config.toml}"

JSON_MODE=0
[[ "${1:-}" == "--json" ]] && JSON_MODE=1

# ── State ────────────────────────────────────────────────────────────────────

CHECKS_PASSED=0
CHECKS_FAILED=0
RESULTS=()

# ── Helpers ──────────────────────────────────────────────────────────────────

check_pass() {
    ((CHECKS_PASSED++))
    if [ "$JSON_MODE" -eq 1 ]; then
        RESULTS+=("{\"check\":\"$1\",\"status\":\"pass\",\"detail\":\"$2\"}")
    else
        echo "  ✓ $1: $2"
    fi
}

check_fail() {
    ((CHECKS_FAILED++))
    if [ "$JSON_MODE" -eq 1 ]; then
        RESULTS+=("{\"check\":\"$1\",\"status\":\"fail\",\"detail\":\"$2\"}")
    else
        echo "  ✗ $1: $2"
    fi
}

check_warn() {
    if [ "$JSON_MODE" -eq 1 ]; then
        RESULTS+=("{\"check\":\"$1\",\"status\":\"warn\",\"detail\":\"$2\"}")
    else
        echo "  ⚠ $1: $2"
    fi
}

# ============================================================================
# CHECK 1: Daemon Process
# ============================================================================

check_daemon_process() {
    local daemon_pid
    daemon_pid=$(pgrep -f 'aura-daemon' 2>/dev/null | head -1 || echo "")

    if [ -n "$daemon_pid" ]; then
        # Check if process is actually alive
        if [ -d "/proc/$daemon_pid" ]; then
            local rss_kb rss_mb
            rss_kb=$(awk '/VmRSS/ {print $2}' "/proc/$daemon_pid/status" 2>/dev/null || echo "0")
            rss_mb=$((rss_kb / 1024))
            check_pass "daemon_process" "running (PID $daemon_pid, RSS ${rss_mb}MB)"
        else
            check_fail "daemon_process" "PID $daemon_pid exists but /proc entry missing (zombie?)"
        fi
    else
        check_fail "daemon_process" "aura-daemon not found"
    fi
}

# ============================================================================
# CHECK 2: IPC Socket
# ============================================================================

check_ipc_socket() {
    if [ -S "$IPC_SOCKET" ]; then
        check_pass "ipc_socket" "exists ($IPC_SOCKET)"
    elif [ -e "$IPC_SOCKET" ]; then
        check_fail "ipc_socket" "path exists but is not a socket ($IPC_SOCKET)"
    else
        check_fail "ipc_socket" "not found ($IPC_SOCKET)"
    fi
}

# ============================================================================
# CHECK 3: Database Accessibility
# ============================================================================

check_database() {
    if [ ! -f "$AURA_DB_PATH" ]; then
        check_fail "database" "file not found ($AURA_DB_PATH)"
        return
    fi

    local db_size_kb
    db_size_kb=$(du -k "$AURA_DB_PATH" 2>/dev/null | awk '{print $1}' || echo "0")

    # Check if SQLite can read the header (basic corruption check)
    if command -v sqlite3 &>/dev/null; then
        local pragma_result
        pragma_result=$(sqlite3 "$AURA_DB_PATH" "PRAGMA integrity_check;" 2>/dev/null || echo "error")
        if [ "$pragma_result" = "ok" ]; then
            check_pass "database" "accessible, integrity OK (${db_size_kb}KB)"
        else
            check_fail "database" "integrity check failed: $pragma_result"
        fi
    else
        # Fallback: check file is readable and non-zero
        if [ "$db_size_kb" -gt 0 ] && [ -r "$AURA_DB_PATH" ]; then
            check_pass "database" "file readable (${db_size_kb}KB) (sqlite3 not available for integrity check)"
        else
            check_fail "database" "file missing or unreadable ($AURA_DB_PATH)"
        fi
    fi
}

# ============================================================================
# CHECK 4: Model Files
# ============================================================================

check_model_files() {
    if [ ! -d "$AURA_MODELS_DIR" ]; then
        check_fail "model_files" "models directory not found ($AURA_MODELS_DIR)"
        return
    fi

    local model_count=0
    local model_list=""

    for f in "$AURA_MODELS_DIR"/*.gguf; do
        if [ -f "$f" ]; then
            ((model_count++))
            local size_mb
            size_mb=$(du -m "$f" 2>/dev/null | awk '{print $1}' || echo "0")
            if [ "$size_mb" -gt 500 ]; then
                model_list="${model_list}$(basename "$f")(${size_mb}MB) "
            else
                model_list="${model_list}$(basename "$f")(${size_mb}MB-SMALL) "
            fi
        fi
    done

    if [ "$model_count" -gt 0 ]; then
        check_pass "model_files" "$model_count model(s): $model_list"
    else
        check_fail "model_files" "no .gguf files found in $AURA_MODELS_DIR"
    fi
}

# ============================================================================
# CHECK 6: Telegram Bot Connectivity
# ============================================================================

check_telegram() {
    if [ ! -f "$CONFIG_FILE" ]; then
        check_warn "telegram" "config file not found ($CONFIG_FILE)"
        return
    fi

    local bot_token
    bot_token=$(grep 'bot_token' "$CONFIG_FILE" 2>/dev/null | head -1 | sed 's/.*= "\(.*\)"/\1/' || echo "")

    if [ -z "$bot_token" ] || [ "$bot_token" = "" ]; then
        check_warn "telegram" "bot_token not configured"
        return
    fi

    local api_resp
    api_resp=$(curl --silent --max-time 10 \
        "https://api.telegram.org/bot${bot_token}/getMe" 2>/dev/null || echo "")

    if echo "$api_resp" | grep -q '"ok":true'; then
        local bot_name
        bot_name=$(echo "$api_resp" | grep -o '"username":"[^"]*"' | cut -d'"' -f4 || echo "unknown")
        check_pass "telegram" "bot reachable (@$bot_name)"
    else
        check_fail "telegram" "api.telegram.org unreachable or invalid token"
    fi
}

# ============================================================================
# CHECK 5: Log Health (optional — looks for recent panics)
# ============================================================================

check_logs() {
    if [ ! -d "$LOG_DIR" ]; then
        check_warn "log_health" "log directory not found ($LOG_DIR)"
        return
    fi

    local latest_log
    latest_log=$(ls -t "$LOG_DIR"/current 2>/dev/null || ls -t "$LOG_DIR"/*.log 2>/dev/null | head -1 || echo "")

    if [ -z "$latest_log" ] || [ ! -f "$latest_log" ]; then
        check_warn "log_health" "no log files found"
        return
    fi

    # Check for panics in last 100 lines
    local recent_panic
    recent_panic=$(tail -100 "$latest_log" 2>/dev/null | grep -ci 'panic\|PANIC\|thread.*panicked' || echo "0")

    if [ "$recent_panic" -gt 0 ]; then
        check_fail "log_health" "PANIC detected in recent logs ($latest_log)"
    else
        check_pass "log_health" "no panics in recent logs"
    fi
}

# ============================================================================
# Run All Checks
# ============================================================================

[ "$JSON_MODE" -eq 0 ] && echo ""
[ "$JSON_MODE" -eq 0 ] && echo "═══ AURA Health Check ═══"
[ "$JSON_MODE" -eq 0 ] && echo ""

check_daemon_process
check_ipc_socket
check_database
check_model_files
check_logs
check_telegram

# ============================================================================
# Output
# ============================================================================

if [ "$JSON_MODE" -eq 1 ]; then
    # Build JSON array
    echo "{"
    echo "  \"timestamp\": \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\","
    echo "  \"passed\": $CHECKS_PASSED,"
    echo "  \"failed\": $CHECKS_FAILED,"
    echo "  \"healthy\": $([ "$CHECKS_FAILED" -eq 0 ] && echo "true" || echo "false"),"
    echo "  \"checks\": ["

    _first=1
    for r in "${RESULTS[@]}"; do
        if [ "$_first" -eq 1 ]; then
            _first=0
        else
            echo ","
        fi
        echo "    $r"
    done
    echo ""
    echo "  ]"
    echo "}"
else
    echo ""
    if [ "$CHECKS_FAILED" -eq 0 ]; then
        echo "  ✅ HEALTHY ($CHECKS_PASSED/$((CHECKS_PASSED + CHECKS_FAILED)) checks passed)"
    else
        echo "  ❌ UNHEALTHY ($CHECKS_FAILED failure(s))"
    fi
    echo ""
fi

exit $( [ "$CHECKS_FAILED" -eq 0 ] && echo 0 || echo 1 )
