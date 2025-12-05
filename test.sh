#!/bin/bash
# Quick test script for Exo-OS
# Runs QEMU and captures output to file for analysis

set -e

cd "$(dirname "$0")"

echo "=== Exo-OS Quick Test Script ==="
echo ""

# Check if ISO exists
if [ ! -f "build/exo_os.iso" ]; then
    echo "Error: build/exo_os.iso not found"
    echo "Run 'bash build.sh build' first"
    exit 1
fi

ISO_SIZE=$(stat -c%s "build/exo_os.iso" 2>/dev/null || stat -f%z "build/exo_os.iso")
ISO_SIZE_MB=$((ISO_SIZE / 1024 / 1024))
echo "ISO size: ${ISO_SIZE_MB}MB"

if [ "$ISO_SIZE_MB" -lt 15 ]; then
    echo "⚠️  Warning: ISO seems small (< 15MB), may not boot correctly"
fi
echo ""

echo "[1/3] Starting QEMU (20 seconds)..."
LOG_FILE="/tmp/exo_os_test_$(date +%s).log"

# Run QEMU with 20 second timeout
timeout 20 qemu-system-x86_64 \
    -cdrom build/exo_os.iso \
    -serial file:"$LOG_FILE" \
    -no-reboot \
    -display none \
    > /dev/null 2>&1 || true

echo "[2/3] QEMU finished, analyzing output..."
echo "       Log saved to: $LOG_FILE"
echo ""

# Check log size
LOG_SIZE=$(wc -c < "$LOG_FILE" 2>/dev/null || echo "0")
if [ "$LOG_SIZE" -eq 0 ]; then
    echo "❌ ERROR: Log file is empty (QEMU may have failed to start)"
    exit 1
fi

echo "[3/3] Test Results:"
echo "==================="
echo ""

# Extract test results
echo ">>> Boot Status:"
grep -i "kernel ready\|initialized" "$LOG_FILE" 2>/dev/null | head -5 || echo "  (boot messages not found)"
echo ""

echo ">>> VFS Status:"
grep -i "VFS.*hello.elf\|loaded.*hello" "$LOG_FILE" 2>/dev/null || echo "  (hello.elf load status not found)"
echo ""

echo ">>> Test Results:"
grep "\[TEST\].*PASSED\|\[TEST\].*FAILED\|\[TEST\].*✅\|\[TEST\].*❌" "$LOG_FILE" 2>/dev/null || echo "  (no test results found)"
echo ""

echo ">>> hello.elf Output:"
grep -i "hello from execve" "$LOG_FILE" 2>/dev/null || echo "  (hello.elf output not found)"
echo ""

echo ">>> Fork+Exec+Wait Test:"
grep -A 10 "test_fork_exec_wait" "$LOG_FILE" 2>/dev/null | head -15 || echo "  (test_fork_exec_wait not found)"
echo ""

echo "==================="
echo "Full log available at: $LOG_FILE"
echo ""
echo "To view full log: cat $LOG_FILE"
echo "To search log: grep -i <keyword> $LOG_FILE"
