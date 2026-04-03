#!/usr/bin/env bash
# =============================================================================
# AURA v4 — Status Script (Termux)
# =============================================================================
# Shows running status of AURA daemon and llama-server.
# =============================================================================

# Termux detection + path setup
if [ -d "/data/data/com.termux/files/usr" ]; then
    IS_TERMUX=1
    PREFIX="/data/data/com.termux/files/usr"
    HOME_DIR="/data/data/com.termux/files/home"
else
    IS_TERMUX=0
    PREFIX="${PREFIX:-/usr}"
    HOME_DIR="${HOME:-/home/user}"
fi
export PATH="$PREFIX/bin:$PATH"

DATA_DIR="${AURA_DATA_DIR:-$HOME_DIR/.local/share/aura}"
PID_FILE="$DATA_DIR/aura-daemon.pid"
LOG_FILE="$DATA_DIR/logs/daemon.log"

echo "=== AURA Status ==="
echo ""

# Check llama-server
echo "--- llama-server (port 8080) ---"
if pgrep -f "llama-server.*--port 8080" > /dev/null 2>&1; then
    LLAMA_PID=$(pgrep -f "llama-server.*--port 8080" | head -1)
    echo "Status: RUNNING"
    echo "PID: $LLAMA_PID"
    
    # Health check
    if curl -s --max-time 3 http://localhost:8080/health > /dev/null 2>&1; then
        echo "Health: OK (responding to requests)"
    else
        echo "Health: UNRESPONSIVE (curl failed)"
    fi
else
    echo "Status: NOT RUNNING"
fi

echo ""

# Check AURA daemon
echo "--- aura-daemon ---"
DAEMON_RUNNING=false

if [ -f "$PID_FILE" ]; then
    PID=$(cat "$PID_FILE")
    if kill -0 "$PID" 2>/dev/null; then
        echo "Status: RUNNING"
        echo "PID: $PID"
        DAEMON_RUNNING=true
    else
        echo "Status: NOT RUNNING (stale PID file)"
    fi
fi

if [ "$DAEMON_RUNNING" = "false" ]; then
    # Try finding by process name
    DAEMON_PID=$(pgrep -f "aura-daemon" 2>/dev/null | head -1)
    if [ -n "$DAEMON_PID" ]; then
        echo "Status: RUNNING (found by name)"
        echo "PID: $DAEMON_PID"
        DAEMON_RUNNING=true
    else
        echo "Status: NOT RUNNING"
    fi
fi

echo ""

# Show last log lines
echo "--- Last 10 log lines ---"
if [ -f "$LOG_FILE" ]; then
    tail -n 10 "$LOG_FILE"
else
    echo "Log file not found: $LOG_FILE"
fi

echo ""
echo "=== End Status ==="
