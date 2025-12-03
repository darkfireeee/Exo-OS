#!/bin/bash
# Script pour relinker le kernel avec la nouvelle libexo_kernel.a
set -e

echo "=== Relinking Exo-OS Kernel ==="

# Vérifier la présence des fichiers nécessaires
if [ ! -f "target/x86_64-unknown-none/debug/libexo_kernel.a" ]; then
    echo "Erreur: libexo_kernel.a non trouvé (compilez avec 'make build' d'abord)"
    exit 1
fi

if [ ! -f "build/libboot_combined.a" ]; then
    echo "Erreur: libboot_combined.a non trouvé"
    exit 1
fi

if [ ! -f "linker.ld" ]; then
    echo "Erreur: linker.ld non trouvé"
    exit 1
fi

echo "-> Linking with:"
echo "   - build/libboot_combined.a"
echo "   - target/x86_64-unknown-none/debug/libexo_kernel.a"

# Linker le kernel
ld -n -o build/kernel.elf -T linker.ld \
    build/libboot_combined.a \
    target/x86_64-unknown-none/debug/libexo_kernel.a

echo "-> Converting ELF to binary..."
objcopy -O binary build/kernel.elf build/kernel.bin

echo "-> Kernel relinked successfully!"
ls -lh build/kernel.elf build/kernel.bin

echo ""
echo "=== Relinking complete ==="
