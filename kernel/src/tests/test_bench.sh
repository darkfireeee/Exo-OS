#!/bin/bash
# Test script for context switch benchmark

cd /workspaces/Exo-OS

echo "=== Starting QEMU Test (15 seconds) ==="
echo ""

timeout 15 qemu-system-x86_64 \
    -cdrom build/exo_os.iso \
    -m 512M \
    -serial stdio \
    -display none \
    2>&1 | tee /tmp/qemu_output.log

echo ""
echo "=== Extracting Benchmark Results ==="
echo ""

# Extract last benchmark report
grep -A 12 "CONTEXT SWITCH BENCHMARK" /tmp/qemu_output.log | tail -15

echo ""
echo "=== Performance Summary ==="
AVG=$(grep "Average:" /tmp/qemu_output.log | tail -1 | awk '{print $3}')
MIN=$(grep "Min:" /tmp/qemu_output.log | tail -1 | awk '{print $3}')
echo "  Current Average: $AVG cycles"
echo "  Current Min:     $MIN cycles"
echo "  Target:          304 cycles"
echo "  Phase 0 Limit:   500 cycles"
echo ""
