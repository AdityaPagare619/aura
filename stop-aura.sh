#!/usr/bin/env bash
# =============================================================================
# AURA v4 — Stop Script (Termux)
# =============================================================================
# Stops the AURA daemon gracefully.
# Usage: bash stop-aura.sh [--stop-llama|-l]
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
STOP_LLAMA=${1:-false}

echo "=== Stopping AURA ==="

# Function to stop daemon
stop_daemon() {
    if [ -f "$PID_FILE" ]; then
        PID=$(cat "$PID_FILE")
        if kill -0 "$PID" 2>/dev/null; then
            echo "Sending SIGTERM to AURA daemon (PID $PID)..."
            kill -TERM "$PID"
            
            # Wait for graceful shutdown (max 10 seconds)
            for i in {1..10}; do
                if ! kill -0 "$PID" 2>/dev/null; then
                    echo "AURA daemon stopped gracefully"
                    rm -f "$PID_FILE"
                    return 0
                fi
                sleep 1
            done
            
            # Force kill if still running
            echo "Daemon still running, sending SIGKILL..."
            kill -9 "$PID" 2>/dev/null || true
            rm -f "$PID_FILE"
            echo "AURA daemon killed"
        else
            echo "Daemon not running (stale PID file)"
            rm -f "$PID_FILE"
        fi
    else
        # Try to find by process name
        DAEMON_PID=$(pgrep -f "aura-daemon" 2>/dev/null | head -1)
        if [ -n "$DAEMON_PID" ]; then
            echo "Found AURA daemon (PID $DAEMON_PID), stopping..."
            kill -TERM "$DAEMON_PID"
            sleep 2
            kill -9 "$DAEMON_PID" 2>/dev/null || true
            echo "AURA daemon stopped"
        else
            echo "AURA daemon not running"
        fi
    fi
}

# Stop the daemon
stop_daemon

# Optionally stop llama-server
if [ "$STOP_LLAMA" = "--stop-llama" ] || [ "$STOP_LLAMA" = "-l" ]; then
    echo "Stopping llama-server..."
    pkill -f "llama-server.*--port 8080" 2>/dev/null || true
    sleep 1
    # Double check
    if pgrep -f "llama-server" > /dev/null 2>&1; then
        echo "Force killing llama-server..."
        pkill -9 -f "llama-server" 2>/dev/null || true
    fi
    echo "llama-server stopped"
fi

echo "=== AURA stopped ==="
exit 0
