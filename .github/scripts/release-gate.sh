#!/bin/bash
# AURA v4 Release Gate Checklist
# ALL checks must pass before release

set -e

echo "=== AURA v4 Release Gate ==="
echo ""

# Check 1: Build
echo "[1/6] Build..."
cargo build --all-features --target aarch64-linux-android
echo "✅ Build passed"

# Check 2: Inspect
echo "[2/6] Inspect..."
file aura-daemon | grep -q "ARM aarch64" || exit 1
echo "✅ Architecture verified"

# Check 3: Test
echo "[3/6] Test..."
cargo test --workspace
echo "✅ Tests passed"

# Check 4: Lint
echo "[4/6] Lint..."
cargo clippy --workspace -- -D warnings
echo "✅ Lint passed"

# Check 5: Device test (manual)
echo "[5/6] Device test..."
echo "⚠️  MANUAL STEP: Test on Moto G45 5G"
read -p "Did device test pass? (y/n) " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then exit 1; fi
echo "✅ Device test passed"

# Check 6: Docs
echo "[6/6] Documentation..."
test -f docs/build/CONTRACT.md || exit 1
test -f docs/build/FAILURE_TAXONOMY.md || exit 1
echo "✅ Documentation complete"

echo ""
echo "=== ALL GATES PASSED ==="
echo "Release approved."
