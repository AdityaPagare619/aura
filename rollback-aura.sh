#!/usr/bin/env bash
# =============================================================================
# AURA v4 — Rollback Script (Termux)
# =============================================================================
# Restores previous binary after a failed deployment.
# Preserves user data: memory, config, database.
#
# Usage: bash rollback-aura.sh [--force]
# =============================================================================

set -e

# Termux detection
if [ -d "/data/data/com.termux/files/usr" ]; then
    export PATH="/data/data/com.termux/files/usr/bin:$PATH"
    IS_TERMUX=1
    PREFIX="/data/data/com.termux/files/usr"
    HOME_DIR="/data/data/com.termux/files/home"
else
    IS_TERMUX=0
    PREFIX="${PREFIX:-/usr}"
    HOME_DIR="${HOME:-/home/user}"
fi

FORCE="${1:-}"
DAEMON_BIN="$PREFIX/bin/aura-daemon"
BACKUP_BIN="${DAEMON_BIN}.pre-update"
NEOCORTEX_BIN="$PREFIX/bin/aura-neocortex"
BACKUP_NEOCORTEX="${NEOCORTEX_BIN}.pre-update"
DATA_DIR="${AURA_DATA_DIR:-$HOME_DIR/.local/share/aura}"
LOG_FILE="$DATA_DIR/logs/rollback.log"

log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" | tee -a "$LOG_FILE"
}

log "=== AURA ROLLBACK STARTED ==="

# --- Validate backups exist ---
if [ ! -f "$BACKUP_BIN" ]; then
    log "ERROR: No backup found at $BACKUP_BIN"
    log "Cannot rollback — no pre-update backup exists."
    exit 1
fi

if [ ! -f "$BACKUP_NEOCORTEX" ]; then
    log "WARNING: No neocortex backup at $BACKUP_NEOCORTEX"
    log "Will only rollback daemon binary."
fi

# --- Stop running daemon ---
log "Stopping AURA daemon..."
PID_FILE="$DATA_DIR/aura-daemon.pid"
if [ -f "$PID_FILE" ]; then
    OLD_PID=$(cat "$PID_FILE")
    kill "$OLD_PID" 2>/dev/null || true
    sleep 2
fi
pkill -f "aura-daemon" 2>/dev/null || true
pkill -f "aura-neocortex" 2>/dev/null || true
sleep 1

# --- Restore daemon backup ---
log "Restoring daemon backup..."
cp "$BACKUP_BIN" "$DAEMON_BIN"
chmod +x "$DAEMON_BIN"
log "Daemon restored: $(ls -la "$DAEMON_BIN")"

# --- Restore neocortex backup if exists ---
if [ -f "$BACKUP_NEOCORTEX" ]; then
    log "Restoring neocortex backup..."
    cp "$BACKUP_NEOCORTEX" "$NEOCORTEX_BIN"
    chmod +x "$NEOCORTEX_BIN"
    log "Neocortex restored: $(ls -la "$NEOCORTEX_BIN")"
fi

# --- Restart service ---
log "Restarting AURA daemon..."
if command -v sv &>/dev/null; then
    sv up aura-daemon 2>/dev/null || true
    sleep 2
    sv status aura-daemon 2>/dev/null || true
else
    # Fallback: start directly
    mkdir -p "$DATA_DIR/logs"
    AURA_NEOCORTEX_BIN="$NEOCORTEX_BIN" nohup "$DAEMON_BIN" \
        --config "$HOME_DIR/.config/aura/config.toml" \
        > "$DATA_DIR/logs/rollback-startup.log" 2>&1 &
    echo "$!" > "$PID_FILE"
    sleep 3
fi

# --- Verify ---
if pgrep -f "aura-daemon" > /dev/null 2>&1; then
    log "=== ROLLBACK COMPLETE — daemon running ==="
else
    log "WARNING: Daemon may not be running after rollback"
    log "Check logs: tail -f $DATA_DIR/logs/rollback-startup.log"
    exit 1
fi
