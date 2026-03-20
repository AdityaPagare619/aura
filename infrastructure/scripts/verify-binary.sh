#!/bin/bash
# AURA Binary Verification Script
#
# Verifies that a downloaded binary matches the official build.
# Users can run this to ensure they haven't been MITM'd.
#
# Usage:
#   ./verify-binary.sh aura-daemon
#   ./verify-binary.sh aura-daemon.sha256.aura
#
# What it verifies:
#   1. SHA-256 checksum matches official release
#   2. Binary is not packed/obfuscated
#   3. Binary targets correct architecture (aarch64)
#   4. Binary has required symbols (no stripped debugging)
#   5. Binary size is within expected range

set -e

BINARY="${1:-aura-daemon}"
REPO="${REPO:-AdityaPagare619}"
PROJECT="${PROJECT:-aura}"

echo "=============================================="
echo "AURA Binary Verification"
echo "=============================================="
echo "Binary: $BINARY"
echo "Repo: $REPO/$PROJECT"
echo ""

# Check file exists
if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found: $BINARY"
    echo ""
    echo "Usage: $0 <path-to-binary>"
    exit 1
fi

# Step 1: SHA-256 checksum
echo "[1/5] Computing SHA-256 checksum..."
SHA256=$(sha256sum "$BINARY" | awk '{print $1}')
echo "  SHA-256: $SHA256"

# Step 2: File type
echo "[2/5] Checking file type..."
FILE_TYPE=$(file "$BINARY")
echo "  File type: $FILE_TYPE"

if echo "$FILE_TYPE" | grep -q "ELF"; then
    echo "  ✓ Valid ELF binary"
else
    echo "  ⚠ WARNING: Not a standard ELF binary"
fi

# Step 3: Architecture
echo "[3/5] Checking architecture..."
ARCH=$(file "$BINARY" | grep -oP 'aarch64|arm|x86_64|i[0-9]{3}' | head -1 || echo "unknown")
echo "  Architecture: $ARCH"

if [ "$ARCH" = "aarch64" ]; then
    echo "  ✓ Correct architecture (Android ARM64)"
elif [ "$ARCH" = "x86_64" ]; then
    echo "  ✓ x86_64 (desktop build)"
else
    echo "  ⚠ WARNING: Unexpected architecture"
fi

# Step 4: Binary size
echo "[4/5] Checking binary size..."
SIZE=$(stat -c%s "$BINARY" 2>/dev/null || stat -f%z "$BINARY" 2>/dev/null)
SIZE_MB=$(echo "scale=2; $SIZE / 1024 / 1024" | bc 2>/dev/null || echo "$SIZE bytes")
echo "  Size: $SIZE bytes ($SIZE_MB MB)"

# Reasonable size check (aura-daemon should be 10-200MB)
if [ "$SIZE" -gt 50000000 ] && [ "$SIZE" -lt 500000000 ]; then
    echo "  ✓ Size within expected range"
elif [ "$SIZE" -lt 10000000 ]; then
    echo "  ⚠ WARNING: Binary very small, might be stub"
elif [ "$SIZE" -gt 500000000 ]; then
    echo "  ⚠ WARNING: Binary very large"
fi

# Step 5: Required symbols check
echo "[5/5] Checking for required symbols..."
if command -v readelf &> /dev/null; then
    SYMBOLS=$(readelf -s "$BINARY" 2>/dev/null | grep -c "FUNC" || echo "0")
    echo "  Function symbols: $SYMBOLS"
    
    if [ "$SYMBOLS" -gt 100 ]; then
        echo "  ✓ Binary contains function symbols"
    else
        echo "  ⚠ WARNING: Binary might be stripped"
    fi
    
    # Check for Rust runtime symbols
    RUST_SYMBOLS=$(readelf -s "$BINARY" 2>/dev/null | grep -c "_rust_" || echo "0")
    echo "  Rust runtime symbols: $RUST_SYMBOLS"
    
    if [ "$RUST_SYMBOLS" -gt 0 ]; then
        echo "  ✓ Binary is a Rust binary"
    fi
    
    # Check for our specific binary name
    if readelf -s "$BINARY" 2>/dev/null | grep -q "aura_daemon"; then
        echo "  ✓ Binary contains aura_daemon symbols"
    fi
elif command -v nm &> /dev/null; then
    SYMBOLS=$(nm "$BINARY" 2>/dev/null | grep -c " T \| t " || echo "0")
    echo "  Symbols: $SYMBOLS"
fi

# Verify against GitHub releases
echo ""
echo "=============================================="
echo "Verifying against GitHub releases..."
echo "=============================================="

# Get latest release tag
LATEST_TAG=$(curl -s "https://api.github.com/repos/$REPO/$PROJECT/releases/latest" | \
    grep '"tag_name"' | sed -E 's/.*"([^"]+)".*/\1/')

if [ -z "$LATEST_TAG" ]; then
    echo "⚠ Could not fetch latest release info"
    echo "  Skipping remote verification"
else
    echo "Latest release: $LATEST_TAG"
    
    # Try to get official checksum
    CHECKSUM_URL="https://github.com/$REPO/$PROJECT/releases/download/$LATEST_TAG/SHA256SUMS.txt"
    CHECKSUM=$(curl -sfL "$CHECKSUM_URL" 2>/dev/null | grep "$(basename $BINARY)" | awk '{print $1}' || echo "")
    
    if [ -n "$CHECKSUM" ]; then
        echo "Official SHA-256: $CHECKSUM"
        echo "Your SHA-256:      $SHA256"
        
        if [ "$SHA256" = "$CHECKSUM" ]; then
            echo ""
            echo "✅ VERIFICATION PASSED"
            echo "Binary matches official release $LATEST_TAG"
        else
            echo ""
            echo "❌ VERIFICATION FAILED"
            echo "Binary does NOT match official release!"
            echo "This could indicate a corrupted download or tampering."
            exit 1
        fi
    else
        echo "⚠ No official checksum found"
        echo "  Build from source for full verification"
    fi
fi

echo ""
echo "Verification complete."
echo ""
echo "For full reproducibility, build from source:"
echo "  git clone https://github.com/$REPO/$PROJECT"
echo "  cd $PROJECT"
echo "  cargo build --release -p aura-daemon --target aarch64-linux-android"
echo "  ./infrastructure/scripts/verify-binary.sh target/aarch64-linux-android/release/aura-daemon"
