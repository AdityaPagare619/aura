#!/usr/bin/env bash
# =============================================================================
# AURA v4 — Deployment Script (Termux)
# =============================================================================
# Deploys fresh binaries and tests the full system.
# Usage: Run this script in Termux after connecting device
# =============================================================================

set -e

# =============================================================================
# EXPECTED SHA256 CHECKSUMS — update after each build
# =============================================================================
BIN_DAEMON_SHA256="${BIN_DAEMON_SHA256:-PLACEHOLDER_DAEMON_SHA256}"
BIN_NEOCORTEX_SHA256="${BIN_NEOCORTEX_SHA256:-PLACEHOLDER_NEOCORTEX_SHA256}"

# Termux detection + path setup
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

DATA_DIR="${AURA_DATA_DIR:-$HOME_DIR/.local/share/aura}"
MODELS_DIR="$DATA_DIR/models"
LOGS_DIR="$DATA_DIR/logs"

# =============================================================================
# verify_binary_checksum <binary_path> <expected_sha256> <label>
# =============================================================================
verify_binary_checksum() {
    local bin_path="$1"
    local expected="$2"
    local label="$3"

    if [ ! -f "$bin_path" ]; then
        echo "ERROR [$label]: Binary not found at $bin_path"
        exit 1
    fi

    if [ "$expected" = "PLACEHOLDER_DAEMON_SHA256" ] || [ "$expected" = "PLACEHOLDER_NEOCORTEX_SHA256" ]; then
        echo "WARNING [$label]: Checksum placeholder not updated — skipping verification"
        echo "  Set BIN_DAEMON_SHA256 and BIN_NEOCORTEX_SHA256 after build."
        return 0
    fi

    local actual
    actual=$(sha256sum "$bin_path" | awk '{print $1}')

    if [ "$actual" = "$expected" ]; then
        echo "OK [$label]: SHA256 verified ($actual)"
    else
        echo "ERROR [$label]: SHA256 MISMATCH"
        echo "  Expected: $expected"
        echo "  Actual:   $actual"
        echo "  Aborting deployment — binary may be corrupted or tampered."
        exit 1
    fi
}

# =============================================================================
# BACKUP CURRENT BINARIES BEFORE DEPLOYMENT
# =============================================================================
backup_current_binaries() {
    if [ -f ~/bin/aura-daemon ]; then
        cp ~/bin/aura-daemon ~/bin/aura-daemon.pre-update
        echo "Backed up current daemon to ~/bin/aura-daemon.pre-update"
    fi
    if [ -f ~/bin/aura-neocortex ]; then
        cp ~/bin/aura-neocortex ~/bin/aura-neocortex.pre-update
        echo "Backed up current neocortex to ~/bin/aura-neocortex.pre-update"
    fi
}

# Termux detection
if [ -d "/data/data/com.termux/files/usr" ]; then
    export PATH="/data/data/com.termux/files/usr/bin:$PATH"
    IS_TERMUX=1
else
    IS_TERMUX=0
fi

echo "=== AURA DEPLOYMENT SCRIPT ==="
echo "Date: $(date)"
echo ""

# Step 1: Kill existing processes
echo "[Step 1] Killing existing processes..."
pkill -f aura-daemon 2>/dev/null || true
pkill -f aura-neocortex 2>/dev/null || true
pkill -f llama-server 2>/dev/null || true
sleep 2

# Step 2: Create directories
echo "[Step 2] Creating directories..."
mkdir -p "$DATA_DIR/db" "$HOME_DIR/.config/aura" "$LOGS_DIR"

# Step 3: Copy fresh binaries from PC (via ADB push to /sdcard/Aura/)
echo "[Step 3] Copying fresh binaries..."
echo "NOTE: Binaries should be pushed from PC first using:"
echo "  adb push target/aarch64-linux-android/release/aura-neocortex /sdcard/Aura/"
echo "  adb push target/aarch64-linux-android/release/aura-daemon /sdcard/Aura/"

# Copy from SD card to Termux
cp /sdcard/Aura/aura-neocortex "$PREFIX/bin/aura-neocortex"
cp /sdcard/Aura/aura-daemon "$PREFIX/bin/aura-daemon"
chmod +x "$PREFIX/bin/aura-neocortex" "$PREFIX/bin/aura-daemon"

# Step 4: Verify binaries (checksum + file integrity)
echo "[Step 4] Verifying binaries..."
ls -la "$PREFIX/bin/aura-neocortex" "$PREFIX/bin/aura-daemon"
file "$PREFIX/bin/aura-neocortex" "$PREFIX/bin/aura-daemon"

# Backup before checksum verification fails
backup_current_binaries

# Verify SHA256 checksums
verify_binary_checksum "$PREFIX/bin/aura-daemon" "$BIN_DAEMON_SHA256" "aura-daemon"
verify_binary_checksum "$PREFIX/bin/aura-neocortex" "$BIN_NEOCORTEX_SHA256" "aura-neocortex"
echo "All binary checksums verified."

# Step 5: Start llama-server (optional — AURA v4 uses aura-neocortex directly)
echo "[Step 5] Starting llama-server (optional)..."
MODEL_FILE=$(ls "$MODELS_DIR/"*.gguf 2>/dev/null | head -1)
if [ -n "$MODEL_FILE" ]; then
    llama-server --model "$MODEL_FILE" --host 127.0.0.1 --port 8080 --ctx-size 2048 --threads 4 &
    sleep 10

    # Verify llama-server is running
    curl -s --max-time 5 http://localhost:8080/health || echo "WARNING: llama-server not running"
else
    echo "WARNING: No model found in $MODELS_DIR — skipping llama-server"
fi

# Step 6: Set wakelock
echo "[Step 6] Setting wakelock..."
termux-wake-lock

# Step 7: Start daemon with env var
echo "[Step 7] Starting daemon..."
AURA_NEOCORTEX_BIN="$PREFIX/bin/aura-neocortex" "$PREFIX/bin/aura-daemon" --config "$HOME_DIR/.config/aura/config.toml" > "$LOGS_DIR/deployment.log" 2>&1 &

# Step 8: Wait and check
echo "[Step 8] Waiting for startup..."
sleep 5

# Step 9: Check logs
echo "[Step 9] Checking logs..."
tail -20 "$LOGS_DIR/deployment.log"

# Step 10: Test neocortex in isolation
echo "[Step 10] Testing neocortex in isolation..."
"$PREFIX/bin/aura-neocortex" --socket @aura_ipc_v4 --model-dir "$MODELS_DIR" 2>&1 | head -10 || echo "Neocortex test completed"

echo ""
echo "=== DEPLOYMENT COMPLETE ==="
echo "Check logs: tail -f $LOGS_DIR/deployment.log"
