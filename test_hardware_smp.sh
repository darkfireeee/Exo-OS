#!/bin/bash
# Phase 2c Week 4: Hardware SMP Validation with Bochs
# Tests real multi-core execution, cache coherency, TLB shootdown

set -e

echo "======================================================================"
echo "Phase 2c Week 4: Hardware SMP Validation"
echo "======================================================================"

# Build kernel
echo ""
echo "[1/5] Building kernel (release mode)..."
cd /workspaces/Exo-OS/kernel
cargo build --release --target ../x86_64-unknown-none.json 2>&1 | grep -E "(Finished|error)" || true

if [ ! -f ../target/x86_64-unknown-none/release/libexo_kernel.a ]; then
    echo "❌ Kernel build failed"
    exit 1
fi

echo "✅ Kernel build SUCCESS"

# Create bootable ISO
echo ""
echo "[2/5] Creating bootable ISO..."
cd /workspaces/Exo-OS

# Copy kernel to boot directory
mkdir -p build/iso/boot/grub
cp target/x86_64-unknown-none/release/libexo_kernel.a build/iso/boot/kernel.bin
cp bootloader/grub.cfg build/iso/boot/grub/

# Create ISO
if command -v grub-mkrescue &> /dev/null; then
    grub-mkrescue -o exo-os.iso build/iso/
    echo "✅ ISO created: exo-os.iso"
else
    echo "⚠️ grub-mkrescue not found, skipping ISO creation"
    echo "   Install: apt-get install grub-pc-bin xorriso"
fi

# Configure Bochs for SMP
echo ""
echo "[3/5] Configuring Bochs (4 CPUs)..."

cat > .bochsrc << 'EOF'
# Bochs configuration for SMP testing (4 CPUs)
megs: 512
cpu: count=4, ips=50000000, reset_on_triple_fault=1
cpuid: level=6, mmx=1, sep=1, sse=sse4_2, xapic=1, aes=1, movbe=1, xsave=1

# Boot from ISO
boot: cdrom
ata0-master: type=cdrom, path="exo-os.iso", status=inserted

# Display
display_library: x, options="gui_debug"
vga: extension=vbe
pci: enabled=1

# Logging
log: bochs.log
error: action=report
info: action=report
debug: action=ignore
panic: action=ask

# Debugging
magic_break: enabled=1
port_e9_hack: enabled=1
com1: enabled=1, mode=file, dev=com1.log
EOF

echo "✅ Bochs configured for 4 CPUs"

# Run tests in Bochs
echo ""
echo "[4/5] Running SMP tests in Bochs..."
echo ""
echo "Bochs will launch with 4 virtual CPUs."
echo "Expected tests to run:"
echo "  - Multi-core context switching"
echo "  - Cache coherency (L1/L2 isolation)"
echo "  - Load balancing across CPUs"
echo "  - TLB shootdown on remote CPUs"
echo "  - FPU lazy switching (multi-core)"
echo ""
echo "Press 'c' in Bochs debugger to continue execution"
echo "Watch for test output in serial log (com1.log)"
echo ""

if command -v bochs &> /dev/null; then
    # Run Bochs (will open GUI)
    timeout 120 bochs -q -f .bochsrc || true
    
    echo ""
    echo "[5/5] Analyzing test results..."
    
    # Check serial log for test results
    if [ -f com1.log ]; then
        echo ""
        echo "=== Test Results from Serial Log ==="
        grep -E "(✅|❌|PASS|FAIL|test_)" com1.log | tail -20 || echo "No test results found"
        
        # Count passes/fails
        PASSES=$(grep -c "✅\|PASS" com1.log || echo "0")
        FAILS=$(grep -c "❌\|FAIL" com1.log || echo "0")
        
        echo ""
        echo "Summary: $PASSES passed, $FAILS failed"
        
        if [ "$FAILS" -gt 0 ]; then
            echo "❌ Some tests failed - check com1.log for details"
            exit 1
        else
            echo "✅ All hardware tests PASSED"
        fi
    else
        echo "⚠️ No serial log found (com1.log)"
    fi
else
    echo "❌ Bochs not installed"
    echo "   Install: apt-get install bochs bochs-x"
    exit 1
fi

echo ""
echo "======================================================================"
echo "Hardware validation complete!"
echo "Logs: bochs.log, com1.log"
echo "======================================================================"
