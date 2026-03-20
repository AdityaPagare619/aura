#!/bin/bash
# AURA F001 ROOT CAUSE DIAGNOSTIC SCRIPT
# Run this in Termux (F-Droid) on your Android device
# This tests WHY the binary crashes at startup
# COPY EVERYTHING BELOW AND PASTE INTO TERMUX

set -e

STAMP=$(date +%Y%m%d-%H%M%S)
OUTDIR="$HOME/storage/downloads/AURA-F001-DIAG-$STAMP"
mkdir -p "$OUTDIR"

echo "=========================================="
echo "AURA F001 ROOT CAUSE DIAGNOSTIC"
echo "Started: $(date)"
echo "Output: $OUTDIR"
echo "=========================================="

# Step 1: Find the daemon binary
echo ""
echo "=== FINDING DAEMON BINARY ==="

DAEMON=""
DAEMON_DIR="$HOME/storage/downloads"

for dir in \
    "$HOME/storage/downloads" \
    "$PREFIX/bin" \
    "/data/data/com.termux/files/usr/bin" \
    "$HOME" \
    "$HOME/../home"; do
    for name in "aura-daemon" "aura-daemon-v4.0.0-alpha.8-aarch64-linux-android"; do
        if [ -f "$dir/$name" ]; then
            DAEMON="$dir/$name"
            break 2
        fi
    done
done

if [ -z "$DAEMON" ]; then
    echo "Downloading alpha.8 daemon binary..."
    cd "$DAEMON_DIR"
    curl -L -o aura-daemon-alpha8 https://github.com/AdityaPagare619/aura/releases/download/v4.0.0-alpha.8/aura-daemon-v4.0.0-alpha.8-aarch64-linux-android
    chmod +x aura-daemon-alpha8
    DAEMON="$DAEMON_DIR/aura-daemon-alpha8"
fi

echo "Using daemon: $DAEMON"
ls -la "$DAEMON"
file "$DAEMON"

echo ""
echo "=== SAVING BINARY INFO ==="

# Save binary details
file "$DAEMON" > "$OUTDIR/00_binary_file.txt"
ls -la "$DAEMON" > "$OUTDIR/01_binary_ls.txt"

# Try readelf if available
if command -v readelf &>/dev/null; then
    readelf -h "$DAEMON" > "$OUTDIR/02_elf_header.txt" 2>&1
    readelf -d "$DAEMON" > "$OUTDIR/03_elf_dynamic.txt" 2>&1
    readelf -l "$DAEMON" > "$OUTDIR/04_elf_program_headers.txt" 2>&1
    readelf -S "$DAEMON" > "$OUTDIR/05_elf_sections.txt" 2>&1
else
    echo "readelf not available in termux"
fi

echo ""
echo "=== TEST 1: WITH LD_PRELOAD (normal Termux) ==="
echo "This is the default Termux environment"

$DAEMON --version > "$OUTDIR/10_test1_with_preload.txt" 2>&1
EXIT1=$?
echo "Exit code: $EXIT1" >> "$OUTDIR/10_test1_with_preload.txt"
echo "Result: Exit code $EXIT1"
cat "$OUTDIR/10_test1_with_preload.txt"

echo ""
echo "=== TEST 2: WITHOUT LD_PRELOAD ==="
echo "Testing with LD_PRELOAD completely cleared"

env -i HOME="$HOME" PATH="$PATH" PREFIX="$PREFIX" LD_PRELOAD= "$DAEMON" --version > "$OUTDIR/11_test2_no_preload.txt" 2>&1
EXIT2=$?
echo "Exit code: $EXIT2" >> "$OUTDIR/11_test2_no_preload.txt"
echo "Result: Exit code $EXIT2"
cat "$OUTDIR/11_test2_no_preload.txt"

echo ""
echo "=== TEST 3: WITH MINIMAL ENVIRONMENT ==="
echo "Testing with only the most essential variables"

env -i \
    HOME="$HOME" \
    PREFIX="$PREFIX" \
    LD_PRELOAD= \
    LD_LIBRARY_PATH= \
    "$DAEMON" --version > "$OUTDIR/12_test3_min_env.txt" 2>&1
EXIT3=$?
echo "Exit code: $EXIT3" >> "$OUTDIR/12_test3_min_env.txt"
echo "Result: Exit code $EXIT3"
cat "$OUTDIR/12_test3_min_env.txt"

echo ""
echo "=== TEST 4: STRACE (if available) ==="
echo "System call trace"

if command -v strace &>/dev/null; then
    echo "Running strace..."
    strace -f -e trace=open,openat,read,execve "$DAEMON" --version 2>&1 | head -100 > "$OUTDIR/13_test4_strace.txt"
    echo "Saved strace output"
else
    echo "strace not available: pkg install strace"
    echo "strace not available" > "$OUTDIR/13_test4_strace.txt"
fi

echo ""
echo "=== TEST 5: COPY TO /tmp AND RUN ==="
echo "Testing if location matters"

cp "$DAEMON" /tmp/aura-daemon-test
chmod +x /tmp/aura-daemon-test
LD_PRELOAD= /tmp/aura-daemon-test --version > "$OUTDIR/14_test5_tmp.txt" 2>&1
EXIT5=$?
echo "Exit code: $EXIT5" >> "$OUTDIR/14_test5_tmp.txt"
echo "Result: Exit code $EXIT5"
cat "$OUTDIR/14_test5_tmp.txt"

echo ""
echo "=== TEST 6: CHECK LD_PRELOAD VALUE ==="
echo "What is Termux setting?"

echo "Current LD_PRELOAD: $LD_PRELOAD" > "$OUTDIR/15_ld_preload_value.txt"
echo "All LD_* vars:" >> "$OUTDIR/15_ld_preload_value.txt"
env | grep '^LD_' >> "$OUTDIR/15_ld_preload_value.txt" || true
cat "$OUTDIR/15_ld_preload_value.txt"

# If LD_PRELOAD is set, try each component
if [ -n "$LD_PRELOAD" ]; then
    echo ""
    echo "=== TEST 7: ISOLATE EACH LD_PRELOAD ==="
    
    echo "$LD_PRELOAD" | tr ':' '\n' | while read -r lib; do
        if [ -f "$lib" ]; then
            echo "Testing with ONLY: $lib"
            LD_PRELOAD="$lib" "$DAEMON" --version > "$OUTDIR/16_test_${lib##*/}.txt" 2>&1
            echo "Exit: $?" >> "$OUTDIR/16_test_${lib##*/}.txt"
        fi
    done
fi

echo ""
echo "=== TEST 8: NDCORTEX BINARY ==="
echo "Same tests for neocortex"

NEOCORTEX=""
for dir in \
    "$HOME/storage/downloads" \
    "$PREFIX/bin" \
    "/data/data/com.termux/files/usr/bin" \
    "$HOME"; do
    for name in "aura-neocortex" "aura-neocortex-v4.0.0-alpha.8-aarch64-linux-android"; do
        if [ -f "$dir/$name" ]; then
            NEOCORTEX="$dir/$name"
            break 2
        fi
    done
done

if [ -z "$NEOCORTEX" ]; then
    echo "Downloading alpha.8 neocortex binary..."
    cd "$DAEMON_DIR"
    curl -L -o aura-neocortex-alpha8 https://github.com/AdityaPagare619/aura/releases/download/v4.0.0-alpha.8/aura-neocortex-v4.0.0-alpha.8-aarch64-linux-android
    chmod +x aura-neocortex-alpha8
    NEOCORTEX="$DAEMON_DIR/aura-neocortex-alpha8"
fi

echo "Using neocortex: $NEOCORTEX"
file "$NEOCORTEX"

echo "Test 1 (with preload):"
LD_PRELOAD= "$NEOCORTEX" --help > "$OUTDIR/20_neo_with_preload.txt" 2>&1
echo "Exit: $?" >> "$OUTDIR/20_neo_with_preload.txt"
cat "$OUTDIR/20_neo_with_preload.txt"

echo ""
echo "Test 2 (without preload):"
env -i HOME="$HOME" PATH="$PATH" PREFIX="$PREFIX" LD_PRELOAD= "$NEOCORTEX" --help > "$OUTDIR/21_neo_no_preload.txt" 2>&1
echo "Exit: $?" >> "$OUTDIR/21_neo_no_preload.txt"
cat "$OUTDIR/21_neo_no_preload.txt"

echo ""
echo "=========================================="
echo "F001 DIAGNOSTIC COMPLETE"
echo "Finished: $(date)"
echo ""
echo "RESULTS SUMMARY:"
echo "Test 1 (with preload): Exit $EXIT1"
echo "Test 2 (no preload):   Exit $EXIT2"
echo "Test 3 (min env):       Exit $EXIT3"
echo "Test 5 (/tmp copy):     Exit $EXIT5"
echo ""
if [ "$EXIT2" = "0" ] && [ "$EXIT1" != "0" ]; then
    echo "*** FOUND ROOT CAUSE: LD_PRELOAD causes the crash ***"
    echo "The Termux preload library breaks the binary."
    echo "Fix: Remove LD_PRELOAD or rebuild with different flags."
elif [ "$EXIT2" != "0" ] && [ "$EXIT3" != "0" ]; then
    echo "*** NO DIFFERENCE: Problem is NOT LD_PRELOAD ***"
    echo "Crash happens even without preload. Check toolchain/build."
else
    echo "*** INCONCLUSIVE: More analysis needed ***"
fi
echo ""
echo "Output directory: $OUTDIR"
echo "=========================================="
echo ""
echo "ZIP AND SHARE:"
echo "  cd ~/storage/downloads"
echo "  zip -r AURA-F001-DIAG-$STAMP.zip AURA-F001-DIAG-$STAMP/"
