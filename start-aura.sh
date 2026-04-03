#!/usr/bin/env bash
# =============================================================================
# AURA v4 — Start Script (Termux)
# =============================================================================
# Starts the AURA daemon and optionally llama-server.
# Uses paths consistent with install.sh:
#   Binaries:   $PREFIX/bin/
#   Config:     $HOME/.config/aura/config.toml
#   Data:       $HOME/.local/share/aura/
#   PID:        $HOME/.local/share/aura/aura-daemon.pid
# =============================================================================

set -e

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

# Paths — match install.sh conventions
DAEMON_BIN="$PREFIX/bin/aura-daemon"
NEOCORTEX_BIN="$PREFIX/bin/aura-neocortex"
CONFIG_FILE="${AURA_CONFIG_FILE:-$HOME_DIR/.config/aura/config.toml}"
DATA_DIR="${AURA_DATA_DIR:-$HOME_DIR/.local/share/aura}"
LOG_FILE="$DATA_DIR/logs/daemon.log"
PID_FILE="$DATA_DIR/aura-daemon.pid"
AURA_IPC_SOCKET="${AURA_IPC_SOCKET:-@aura_ipc_v4}"

# =============================================================================
# HEALTH CHECK — validates daemon and IPC connectivity
# Returns 0 (healthy) or 1 (unhealthy)
# =============================================================================
health_check() {
    local healthy=0

    # Check 1: Daemon process is alive
    if [ -f "$PID_FILE" ]; then
        local pid
        pid=$(cat "$PID_FILE")
        if kill -0 "$pid" 2>/dev/null; then
            echo "[health] Daemon PID $pid is alive"
        else
            echo "[health] FAIL: Daemon PID $pid is dead"
            return 1
        fi
    else
        echo "[health] FAIL: No PID file at $PID_FILE"
        return 1
    fi

    # Check 2: IPC socket (Unix domain socket or abstract socket)
    if [[ "$AURA_IPC_SOCKET" == @* ]]; then
        # Abstract socket — check if daemon process has the socket open
        if ls -l /proc/"$pid"/fd 2>/dev/null | grep -q "socket"; then
            echo "[health] IPC: Daemon has open sockets (abstract: $AURA_IPC_SOCKET)"
        else
            echo "[health] WARNING: No sockets found for PID $pid"
        fi
    elif [ -S "$AURA_IPC_SOCKET" ]; then
        echo "[health] IPC: Unix socket exists ($AURA_IPC_SOCKET)"
    else
        echo "[health] WARNING: IPC socket not found ($AURA_IPC_SOCKET)"
    fi

    echo "[health] OK: System healthy"
    return 0
}

echo "=== Starting AURA ==="

# Check if llama-server is running (optional — AURA v4 uses aura-neocortex directly)
if [ "${AURA_START_LLAMA:-0}" = "1" ]; then
    if ! pgrep -f "llama-server.*--port 8080" > /dev/null 2>&1; then
        echo "llama-server not running, starting..."
        local_model=$(ls "$DATA_DIR/models/"*.gguf 2>/dev/null | head -1)
        if [ -z "$local_model" ]; then
            echo "WARNING: No GGUF model found in $DATA_DIR/models/"
        else
            nohup llama-server \
                --model "$local_model" \
                --port 8080 \
                --host 127.0.0.1 \
                -c 2048 \
                -ngl 0 \
                > "$DATA_DIR/logs/llama-server.log" 2>&1 &
            sleep 3

            if curl -s --max-time 5 http://localhost:8080/health > /dev/null 2>&1; then
                echo "llama-server started successfully on port 8080"
            else
                echo "WARNING: llama-server may not have started properly"
                echo "Check $DATA_DIR/logs/llama-server.log"
            fi
        fi
    else
        echo "llama-server already running"
    fi
fi

# Check if AURA daemon is already running
if [ -f "$PID_FILE" ]; then
    OLD_PID=$(cat "$PID_FILE")
    if kill -0 "$OLD_PID" 2>/dev/null; then
        echo "AURA daemon already running with PID $OLD_PID"
        exit 0
    else
        echo "Stale PID file found, removing..."
        rm -f "$PID_FILE"
    fi
fi

# Check if daemon process is running by name
if pgrep -f "aura-daemon" > /dev/null 2>&1; then
    echo "AURA daemon is already running"
    exit 0
fi

# Ensure config file exists
if [ ! -f "$CONFIG_FILE" ]; then
    echo "Error: Config file not found at $CONFIG_FILE"
    echo "Run install.sh first, or create config manually."
    exit 1
fi

# Ensure log directory exists
mkdir -p "$(dirname "$LOG_FILE")"

# Start aura-daemon
nohup "$DAEMON_BIN" \
    --config "$CONFIG_FILE" \
    > "$LOG_FILE" 2>&1 &

NEW_PID=$!
echo "$NEW_PID" > "$PID_FILE"
echo "AURA daemon started with PID $NEW_PID"

# Wait for startup
sleep 3

# Verify daemon is running
if kill -0 "$NEW_PID" 2>/dev/null; then
    echo "AURA daemon is running"

    # --- Health check ---
    if health_check; then
        echo "=== AURA started successfully ==="
        exit 0
    else
        echo "ERROR: AURA daemon started but health check failed"
        echo "Check log: $LOG_FILE"
        exit 1
    fi
else
    echo "ERROR: AURA daemon failed to start"
    echo "Check log: $LOG_FILE"
    rm -f "$PID_FILE"
    exit 1
fi
