#!/bin/bash
# force_rebuild.sh - Force un rebuild complet avec vérifications

set -e

echo "========================================"
echo "  EXO-OS FORCE REBUILD"
echo "========================================"
echo ""

# Couleurs
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

# Demander confirmation
echo "This will:"
echo "  1. Delete all build artifacts"
echo "  2. Clean Cargo cache"
echo "  3. Rebuild kernel from scratch"
echo "  4. Regenerate ISO"
echo ""
read -p "Continue? (y/N) " -n 1 -r
echo ""

if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "Aborted."
    exit 1
fi

echo ""
info "Step 1/5: Cleaning Cargo build artifacts..."
cd kernel
cargo clean
cd ..
info "✓ Cargo cache cleaned"

echo ""
info "Step 2/5: Removing target directory..."
rm -rf target/
info "✓ Target directory removed"

echo ""
info "Step 3/5: Removing old ISO..."
rm -f build/os.iso
rm -f build/*.iso
info "✓ ISO removed"

echo ""
info "Step 4/5: Removing intermediate files..."
rm -f kernel/kernel.bin
rm -f kernel/kernel.elf
rm -rf build/isofiles/
info "✓ Intermediate files removed"

echo ""
info "Step 5/5: Rebuilding kernel..."
cd kernel

# Build avec verbose pour voir ce qui se compile
cargo build --release --target x86_64-unknown-none -vv 2>&1 | tee ../build.log

# Vérifier le succès
if [ $? -eq 0 ]; then
    info "✓ Kernel compiled successfully"
else
    echo ""
    echo -e "${RED}ERROR: Compilation failed!${NC}"
    echo "Check build.log for details"
    exit 1
fi

cd ..

# Vérifier que le nouveau string est présent
echo ""
info "Verifying new code is compiled..."

KERNEL_BIN="target/x86_64-unknown-none/release/kernel"

if [ -f "$KERNEL_BIN" ]; then
    if strings "$KERNEL_BIN" | grep -q "Magic: 0x"; then
        info "✓ New code FOUND in binary!"
        echo "  Strings found:"
        strings "$KERNEL_BIN" | grep -E "(Magic|RUST)"
    else
        warn "New string 'Magic: 0x' NOT FOUND!"
        echo ""
        echo "All strings in binary:"
        strings "$KERNEL_BIN"
    fi
else
    echo -e "${RED}ERROR: Kernel binary not found!${NC}"
    exit 1
fi

# Copier le kernel
echo ""
info "Copying kernel binary..."
mkdir -p build/isofiles/boot/grub
cp "$KERNEL_BIN" build/isofiles/boot/kernel.bin
info "✓ Kernel copied"

# Vérifier la copie
COPIED_SIZE=$(stat -c %s build/isofiles/boot/kernel.bin 2>/dev/null || stat -f "%z" build/isofiles/boot/kernel.bin)
ORIGINAL_SIZE=$(stat -c %s "$KERNEL_BIN" 2>/dev/null || stat -f "%z" "$KERNEL_BIN")

if [ "$COPIED_SIZE" != "$ORIGINAL_SIZE" ]; then
    echo -e "${RED}ERROR: Copied kernel has different size!${NC}"
    exit 1
fi

info "✓ Copy verified (size: $COPIED_SIZE bytes)"

# Créer grub.cfg
echo ""
info "Creating GRUB config..."
cat > build/isofiles/boot/grub/grub.cfg << 'EOF'
set timeout=0
set default=0

menuentry "Exo-OS" {
    multiboot2 /boot/kernel.bin
    boot
}
EOF
info "✓ GRUB config created"

# Générer ISO
echo ""
info "Generating ISO..."

if command -v grub-mkrescue &> /dev/null; then
    grub-mkrescue -o build/os.iso build/isofiles/ 2>&1 | tee -a build.log
    
    if [ $? -eq 0 ]; then
        info "✓ ISO generated successfully"
    else
        echo -e "${RED}ERROR: ISO generation failed!${NC}"
        exit 1
    fi
else
    echo -e "${RED}ERROR: grub-mkrescue not found!${NC}"
    echo "Install with: sudo apt install grub-pc-bin xorriso"
    exit 1
fi

# Vérifications finales
echo ""
echo "========================================"
echo "  VERIFICATION"
echo "========================================"
echo ""

info "Final checks..."

# Check 1: ISO existe
if [ -f "build/os.iso" ]; then
    ISO_SIZE=$(stat -c %s build/os.iso 2>/dev/null || stat -f "%z" build/os.iso)
    info "✓ ISO exists (size: $ISO_SIZE bytes)"
else
    echo -e "${RED}ERROR: ISO not found!${NC}"
    exit 1
fi

# Check 2: Kernel dans ISO
mkdir -p /tmp/exo-verify
if sudo mount -o loop build/os.iso /tmp/exo-verify 2>/dev/null; then
    if [ -f "/tmp/exo-verify/boot/kernel.bin" ]; then
        ISO_KERNEL_SIZE=$(stat -c %s /tmp/exo-verify/boot/kernel.bin 2>/dev/null || stat -f "%z" /tmp/exo-verify/boot/kernel.bin)
        info "✓ Kernel in ISO (size: $ISO_KERNEL_SIZE bytes)"
        
        if [ "$ISO_KERNEL_SIZE" != "$ORIGINAL_SIZE" ]; then
            warn "Size mismatch! Something went wrong."
        else
            info "✓ Sizes match perfectly"
        fi
    else
        warn "Kernel not found in ISO"
    fi
    sudo umount /tmp/exo-verify
else
    warn "Could not verify ISO contents (need sudo)"
fi
rmdir /tmp/exo-verify 2>/dev/null || true

# Résumé final
echo ""
echo "========================================"
echo "  REBUILD COMPLETE!"
echo "========================================"
echo ""
echo "Summary:"
echo "  - Kernel binary: $KERNEL_BIN"
echo "  - Kernel size:   $ORIGINAL_SIZE bytes"
echo "  - ISO file:      build/os.iso"
echo "  - ISO size:      $ISO_SIZE bytes"
echo ""
echo "Next steps:"
echo "  1. Test with: qemu-system-x86_64 -cdrom build/os.iso -serial file:serial.log"
echo "  2. Check logs: cat serial.log"
echo ""

# Afficher les premiers strings du kernel
echo "First 20 strings in kernel:"
strings "$KERNEL_BIN" | head -20
echo ""
