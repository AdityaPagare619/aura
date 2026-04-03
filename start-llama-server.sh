#!/data/data/com.termux/files/usr/bin/bash
# =============================================================================
# AURA v4 — Start llama-server (Termux)
# =============================================================================
# Starts llama-server for external HTTP backend testing.
# AURA v4 uses aura-neocortex directly — this is for fallback/testing only.
# =============================================================================

set -e

PREFIX="/data/data/com.termux/files/usr"
HOME_DIR="/data/data/com.termux/files/home"
DATA_DIR="${AURA_DATA_DIR:-$HOME_DIR/.local/share/aura}"
MODELS_DIR="$DATA_DIR/models"
LOG_FILE="$DATA_DIR/logs/llama-server.log"

# Find first GGUF model
MODEL_FILE=$(ls "$MODELS_DIR/"*.gguf 2>/dev/null | head -1)
if [ -z "$MODEL_FILE" ]; then
    echo "ERROR: No GGUF model found in $MODELS_DIR"
    echo "Download a model or run install.sh"
    exit 1
fi

echo "Using model: $(basename "$MODEL_FILE")"

# Kill existing llama-server if running
pkill -f llama-server 2>/dev/null || true
sleep 1

mkdir -p "$(dirname "$LOG_FILE")"

# Start llama-server in background
nohup llama-server \
    --model "$MODEL_FILE" \
    --port 8080 \
    --host 127.0.0.1 \
    -c 2048 \
    -ngl 0 \
    > "$LOG_FILE" 2>&1 &

echo "llama-server starting..."
sleep 3

# Check if it's running
if curl -s --max-time 5 http://localhost:8080/health > /dev/null 2>&1; then
    echo "llama-server is running on port 8080"
else
    echo "llama-server failed to start. Check log:"
    tail -20 "$LOG_FILE"
    exit 1
fi
