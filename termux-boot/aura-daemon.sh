#!/data/data/com.termux/files/usr/bin/bash
# ─────────────────────────────────────────────────────────────────────────────
# AURA Daemon — Termux Boot Script
# ─────────────────────────────────────────────────────────────────────────────
# This script starts the AURA daemon when Termux boots.
#
# Installation:
#   1. Install termux-boot: pkg install termux-boot
#   2. Copy this script to:
#      ~/.termux/boot/aura-daemon.sh
#   3. Make it executable: chmod +x ~/.termux/boot/aura-daemon.sh
#   4. Reboot or restart termux-boot service
#
# Usage:
#   Manual start:  ~/.termux/boot/aura-daemon.sh
#   Check status:  aura status
#   Stop daemon:   aura stop
#   View logs:     tail -f /data/local/tmp/aura/logs/aura.log
#
# This replaces the systemd service file (aura-daemon.service) which is
# NOT compatible with Termux/Android.
# ─────────────────────────────────────────────────────────────────────────────

# Wait for storage to be mounted (Termux boot runs early)
sleep 5

# ── Configuration ───────────────────────────────────────────────────────────
AURA_DATA_DIR="${AURA_DATA_DIR:-/data/local/tmp/aura}"
AURA_BIN="${PREFIX:-/data/data/com.termux/files/usr}/bin/aura-daemon"
LOG_DIR="${AURA_DATA_DIR}/logs"
LOG_FILE="${LOG_DIR}/aura.log"
PID_FILE="${AURA_DATA_DIR}/aura-daemon.pid"

# ── Ensure directories exist ───────────────────────────────────────────────
mkdir -p "$LOG_DIR"
mkdir -p "$AURA_DATA_DIR/models"

# ── Check if already running ───────────────────────────────────────────────
if [ -f "$PID_FILE" ]; then
    OLD_PID=$(cat "$PID_FILE" 2>/dev/null)
    if [ -n "$OLD_PID" ] && kill -0 "$OLD_PID" 2>/dev/null; then
        echo "[$(date)] AURA daemon already running (PID: $OLD_PID)"
        exit 0
    fi
    # Stale PID file — clean up
    rm -f "$PID_FILE"
fi

# ── Check binary exists ────────────────────────────────────────────────────
if [ ! -x "$AURA_BIN" ]; then
    echo "[$(date)] ERROR: AURA daemon binary not found at $AURA_BIN"
    echo "[$(date)] Install with: cargo install --path crates/aura-daemon"
    exit 1
fi

# ── Start daemon ───────────────────────────────────────────────────────────
echo "[$(date)] Starting AURA daemon..."

export AURA_DATA_DIR
export RUST_LOG="${RUST_LOG:-info}"

# Run in background, redirect output to log file
nohup "$AURA_BIN" --config "${AURA_DATA_DIR}/config.toml" \
    >> "$LOG_FILE" 2>&1 &

DAEMON_PID=$!
echo "$DAEMON_PID" > "$PID_FILE"

# ── Verify startup ─────────────────────────────────────────────────────────
sleep 2
if kill -0 "$DAEMON_PID" 2>/dev/null; then
    echo "[$(date)] AURA daemon started successfully (PID: $DAEMON_PID)"
    echo "[$(date)] Logs: $LOG_FILE"
    echo "[$(date)] Config: ${AURA_DATA_DIR}/config.toml"
else
    echo "[$(date)] ERROR: AURA daemon failed to start"
    echo "[$(date)] Check logs: tail -50 $LOG_FILE"
    rm -f "$PID_FILE"
    exit 1
fi
