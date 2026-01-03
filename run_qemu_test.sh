#!/bin/bash
# Script pour lancer QEMU avec le kernel et afficher la sortie

echo "=== Reconstruction du kernel avec CoW ==="
cd kernel
cargo build --target ../x86_64-unknown-none.json -Z build-std=core,alloc,compiler_builtins --release 2>&1 | tail -20

if [ $? -ne 0 ]; then
    echo "ERREUR: Compilation échouée"
    exit 1
fi

echo ""
echo "=== Copie du kernel vers build/ ==="
cd ..
cp -v kernel/target/x86_64-unknown-none/release/libexo_kernel.a build/kernel.bin 2>&1 || echo "Pas de libexo_kernel.a"
cp -v kernel/target/x86_64-unknown-none/release/exo-kernel build/kernel.elf 2>&1 || echo "Pas de exo-kernel"

echo ""
echo "=== Lancement de QEMU ==="
echo "Appuyez Ctrl+A puis X pour quitter"
echo ""

# Lancer QEMU avec serial output
qemu-system-x86_64 \
    -cdrom build/exo_os.iso \
    -m 512M \
    -serial stdio \
    -no-reboot \
    -no-shutdown \
    -d cpu_reset,guest_errors \
    -D qemu.log \
    -boot d

echo ""
echo "=== Test terminé ==="
echo "Logs QEMU dans: qemu.log"
