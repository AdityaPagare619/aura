#!/usr/bin/env bash
# =============================================================================
# AURA v4 — Restart Script (Termux)
# =============================================================================
# Stops then starts AURA daemon.
# Usage: bash restart-aura.sh [--stop-llama|-l]
# =============================================================================

# Termux detection
if [ -d "/data/data/com.termux/files/usr" ]; then
    export PATH="/data/data/com.termux/files/usr/bin:$PATH"
    IS_TERMUX=1
else
    IS_TERMUX=0
fi

# Determine script directory
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

STOP_LLAMA=""
if [ "$1" = "--stop-llama" ] || [ "$1" = "-l" ]; then
    STOP_LLAMA="--stop-llama"
fi

echo "=== Restarting AURA ==="

# Stop AURA
bash "$SCRIPT_DIR/stop-aura.sh" $STOP_LLAMA

# Small delay before starting
sleep 2

# Start AURA
bash "$SCRIPT_DIR/start-aura.sh"

echo "=== Restart complete ==="
