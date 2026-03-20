#!/bin/bash
# AURA Binary Audit Script
#
# Performs security audit of AURA binaries.
# Checks for common vulnerability patterns.
#
# Usage:
#   ./audit-binary.sh aura-daemon
#   ./audit-binary.sh --full aura-daemon

set -e

BINARY="${1:-aura-daemon}"
FULL_AUDIT=false

if [ "$1" = "--full" ]; then
    FULL_AUDIT=true
    BINARY="${2:-aura-daemon}"
fi

echo "=============================================="
echo "AURA Binary Security Audit"
echo "=============================================="
echo "Binary: $BINARY"
echo "Full audit: $FULL_AUDIT"
echo ""

if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found: $BINARY"
    exit 1
fi

# Check 1: No hardcoded credentials
echo "[1/8] Checking for hardcoded credentials..."
CREDENTIALS=$(strings "$BINARY" 2>/dev/null | grep -iE \
    "(password|passwd|pwd|secret|api_key|apikey|token|auth)" | \
    grep -vE "(/password|PASSWORD|SOURCE_DATE_EPOCH)" | \
    head -5 || true)

if [ -n "$CREDENTIALS" ]; then
    echo "⚠ WARNING: Potential credentials found:"
    echo "$CREDENTIALS" | while read line; do
        echo "  - $line"
    done
else
    echo "  ✓ No hardcoded credentials found"
fi

# Check 2: No dangerous syscalls
echo "[2/8] Checking for dangerous syscalls..."
if command -v readelf &> /dev/null; then
    DANGEROUS_SYSCALLS=$(readelf -s "$BINARY" 2>/dev/null | \
        grep -iE "(exec|system|popen|fork)" | \
        grep -v "GLIBC" | head -3 || true)
    
    if [ -n "$DANGEROUS_SYSCALLS" ]; then
        echo "  Found function calls:"
        echo "$DANGEROUS_SYSCALLS" | while read line; do
            echo "  - $line"
        done
    else
        echo "  ✓ No obviously dangerous syscalls found"
    fi
fi

# Check 3: Proper binary hardening
echo "[3/8] Checking binary hardening..."
if command -v readelf &> /dev/null; then
    # Check for NX (non-executable stack)
    NX=$(readelf -l "$BINARY" 2>/dev/null | grep -c "GNU_STACK" || echo "0")
    if [ "$NX" -gt 0 ]; then
        NX_FLAGS=$(readelf -l "$BINARY" 2>/dev/null | grep "GNU_STACK")
        if echo "$NX_FLAGS" | grep -q "E"; then
            echo "  ⚠ WARNING: Stack is executable!"
        else
            echo "  ✓ NX (non-executable stack) enabled"
        fi
    fi
    
    # Check for RELRO
    RELRO=$(readelf -d "$BINARY" 2>/dev/null | grep -c "RELRO" || echo "0")
    if [ "$RELRO" -gt 0 ]; then
        echo "  ✓ RELRO enabled"
    else
        echo "  ⚠ RELRO not found"
    fi
    
    # Check for PIE
    PIE=$(readelf -h "$BINARY" 2>/dev/null | grep "Class:" || true)
    if echo "$PIE" | grep -q "ELF32"; then
        echo "  ⚠ 32-bit binary (consider 64-bit)"
    else
        echo "  ✓ 64-bit binary"
    fi
fi

# Check 4: Binary size sanity
echo "[4/8] Checking binary size..."
SIZE=$(stat -c%s "$BINARY" 2>/dev/null || stat -f%z "$BINARY" 2>/dev/null)
SIZE_MB=$(echo "scale=2; $SIZE / 1024 / 1024" | bc 2>/dev/null || echo "$SIZE bytes")
echo "  Size: $SIZE bytes ($SIZE_MB MB)"

# Too small could be a stub, too large might include debug info
if [ "$SIZE" -lt 1000000 ]; then
    echo "  ⚠ WARNING: Binary very small (< 1MB)"
elif [ "$SIZE" -gt 500000000 ]; then
    echo "  ⚠ WARNING: Binary very large (> 500MB)"
else
    echo "  ✓ Size within expected range"
fi

# Check 5: Rust-specific checks
echo "[5/8] Checking Rust-specific attributes..."
if command -v rust-objdump &> /dev/null; then
    # Check for Rust panic handler
    RUST_PANIC=$(rust-objdump -t "$BINARY" 2>/dev/null | grep -c "rust_panic" || echo "0")
    echo "  Rust panic symbols: $RUST_PANIC"
    
    # Check for LLVM symbols
    LLVM_SYMBOLS=$(rust-objdump -t "$BINARY" 2>/dev/null | grep -c "llvm." || echo "0")
    echo "  LLVM symbols: $LLVM_SYMBOLS"
elif command -v nm &> /dev/null; then
    RUST_SYMBOLS=$(nm "$BINARY" 2>/dev/null | grep -c "rust_" || echo "0")
    echo "  Rust symbols: $RUST_SYMBOLS"
fi

# Check 6: License compliance
echo "[6/8] Checking license information..."
if command -v strings &> /dev/null; then
    LICENSES=$(strings "$BINARY" 2>/dev/null | \
        grep -iE "(MIT License|Apache|GPL|LGPL|BSD)" | \
        sort -u | head -5 || true)
    
    if [ -n "$LICENSES" ]; then
        echo "  Found licenses:"
        echo "$LICENSES" | while read line; do
            echo "  - $line"
        done
    else
        echo "  No standard license strings found"
    fi
fi

# Check 7: Stripped binary check
echo "[7/8] Checking if binary is stripped..."
if command -v readelf &> /dev/null; then
    SYMBOLS=$(readelf --dyn-syms "$BINARY" 2>/dev/null | grep -c "FUNC" || echo "0")
    echo "  Dynamic function symbols: $SYMBOLS"
    
    if [ "$SYMBOLS" -lt 5 ]; then
        echo "  ⚠ WARNING: Binary appears fully stripped"
    else
        echo "  ✓ Binary has debug symbols (or partial strip)"
    fi
fi

# Check 8: Dynamic library dependencies
echo "[8/8] Checking dynamic library dependencies..."
if command -v ldd &> /dev/null; then
    LIBS=$(ldd "$BINARY" 2>/dev/null | grep -v "not a dynamic" | head -10 || true)
    
    if [ -n "$LIBS" ]; then
        echo "  Dependencies:"
        echo "$LIBS" | while read line; do
            echo "  - $line"
        done
    else
        echo "  ✓ Statically linked (no dynamic dependencies)"
    fi
elif command -v readelf &> /dev/null; then
    DEPS=$(readelf -d "$BINARY" 2>/dev/null | grep "NEEDED" | head -5 || true)
    if [ -n "$DEPS" ]; then
        echo "  Dynamic dependencies:"
        echo "$DEPS" | while read line; do
            echo "  - $line"
        done
    fi
fi

echo ""
echo "=============================================="
echo "Audit complete."
echo ""
echo "For production deployment, also run:"
echo "  1. ./verify-binary.sh $BINARY"
echo "  2. cargo audit (from project root)"
echo "  3. RUSTFLAGS='-C target-feature=+crt-static' cargo build"
echo "=============================================="
