#!/usr/bin/env bash
# ============================================================
# AURA v4 — Build Verification Script
# Checks toolchain, NDK, compiles, and runs tests.
# Usage: bash scripts/verify-build.sh [--target TARGET]
# ============================================================
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

PASS=0
FAIL=0
WARN=0
TARGET="${1:-}"

# ── Helpers ──────────────────────────────────────────────────
pass() { ((PASS++)); echo -e "  ${GREEN}✓${RESET} $1"; }
fail() { ((FAIL++)); echo -e "  ${RED}✗${RESET} $1"; }
warn() { ((WARN++)); echo -e "  ${YELLOW}!${RESET} $1"; }
section() { echo -e "\n${CYAN}${BOLD}── $1 ──${RESET}"; }

# ── 1. Rust Toolchain ───────────────────────────────────────
section "Rust Toolchain"

if command -v rustc &>/dev/null; then
  RUST_VERSION=$(rustc --version | head -1)
  pass "rustc: $RUST_VERSION"
else
  fail "rustc not found — install via https://rustup.rs"
fi

if command -v cargo &>/dev/null; then
  CARGO_VERSION=$(cargo --version | head -1)
  pass "cargo: $CARGO_VERSION"
else
  fail "cargo not found"
fi

if command -v rustfmt &>/dev/null; then
  pass "rustfmt: installed"
else
  warn "rustfmt not found — install with: rustup component add rustfmt"
fi

if command -v clippy-driver &>/dev/null || cargo clippy --version &>/dev/null 2>&1; then
  pass "clippy: installed"
else
  warn "clippy not found — install with: rustup component add clippy"
fi

# Check if target is installed (if specified)
if [ -n "$TARGET" ]; then
  if rustup target list --installed | grep -q "$TARGET"; then
    pass "Target $TARGET: installed"
  else
    fail "Target $TARGET: not installed — install with: rustup target add $TARGET"
  fi
fi

# ── 2. Android NDK ──────────────────────────────────────────
section "Android NDK"

NDK_HOME="${ANDROID_NDK_HOME:-${NDK_HOME:-}}"

if [ -z "$NDK_HOME" ]; then
  warn "ANDROID_NDK_HOME / NDK_HOME not set — skipping NDK checks"
else
  if [ -d "$NDK_HOME" ]; then
    pass "NDK directory exists: $NDK_HOME"

    # Try to detect NDK version
    if [ -f "$NDK_HOME/source.properties" ]; then
      NDK_VER=$(grep "Pkg.Revision" "$NDK_HOME/source.properties" | cut -d'=' -f2 | xargs)
      pass "NDK version: $NDK_VER"
    else
      warn "Cannot read NDK version (source.properties missing)"
    fi

    # Check toolchain binaries
    TOOLCHAIN_DIR="$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"
    if [ -d "$TOOLCHAIN_DIR" ]; then
      pass "Toolchain directory exists"

      CLANG="$TOOLCHAIN_DIR/aarch64-linux-android26-clang"
      if [ -f "$CLANG" ]; then
        pass "aarch64-linux-android26-clang: found"
      else
        warn "aarch64-linux-android26-clang not found at expected path"
      fi

      STRIP="$TOOLCHAIN_DIR/llvm-strip"
      if [ -f "$STRIP" ]; then
        pass "llvm-strip: found"
      else
        warn "llvm-strip not found"
      fi
    else
      warn "Toolchain directory not found: $TOOLCHAIN_DIR"
    fi
  else
    fail "NDK directory does not exist: $NDK_HOME"
  fi
fi

# ── 3. Cargo Check ──────────────────────────────────────────
section "cargo check"

if cargo check --workspace --features "aura-llama-sys/stub" 2>&1; then
  pass "cargo check passed"
else
  fail "cargo check failed"
fi

# ── 4. Cargo Test ───────────────────────────────────────────
section "cargo test"

if RUST_LOG=error cargo test --workspace --features "aura-llama-sys/stub" 2>&1; then
  pass "All tests passed"
else
  fail "Some tests failed"
fi

# ── 5. Summary ──────────────────────────────────────────────
section "Summary"

echo -e "  ${GREEN}Passed:${RESET}  $PASS"
echo -e "  ${RED}Failed:${RESET}  $FAIL"
echo -e "  ${YELLOW}Warned:${RESET}  $WARN"

if [ "$FAIL" -gt 0 ]; then
  echo -e "\n${RED}${BOLD}VERIFICATION FAILED${RESET}"
  exit 1
else
  echo -e "\n${GREEN}${BOLD}VERIFICATION PASSED${RESET}"
  exit 0
fi
