#!/bin/bash
# AURA v4 Rollback Procedure for Termux

BACKUP_FILE="${AURA_DAEMON_BACKUP:-aura-daemon.backup}"
CURRENT_FILE="${HOME}/bin/aura-daemon"
BROKEN_FILE="${HOME}/bin/aura-daemon.broken"

echo "=== AURA Rollback ==="
echo "Current: $CURRENT_FILE"
echo "Backup: $BACKUP_FILE"

# Stop daemon
echo "Stopping daemon..."
pkill aura-daemon || true

# Backup broken binary
if [ -f "$CURRENT_FILE" ]; then
    echo "Backing up broken binary..."
    cp "$CURRENT_FILE" "$BROKEN_FILE"
fi

# Restore backup
if [ -f "$BACKUP_FILE" ]; then
    echo "Restoring backup..."
    cp "$BACKUP_FILE" "$CURRENT_FILE"
    chmod +x "$CURRENT_FILE"
else
    echo "ERROR: No backup found at $BACKUP_FILE"
    exit 1
fi

# Verify
echo "Verifying..."
$CURRENT_FILE --version

echo ""
echo "=== Rollback Complete ==="
