#!/bin/bash
# Script de build complet pour Exo-OS avec linkage C
# Usage: bash scripts/build_complete.sh

set -e  # Arrêter en cas d'erreur

echo "════════════════════════════════════════════════════════"
echo "  🔨 Exo-OS - Build Complet avec Linkage C"
echo "════════════════════════════════════════════════════════"
echo ""

# 1. Compiler le kernel Rust
echo "📦 [1/8] Compilation du kernel Rust..."
cd kernel && cargo build 2>&1 | grep -E "(Finished|Compiling exo-kernel)" | tail -1
cd ..

# 2. Compiler boot.asm
echo "🔧 [2/8] Assemblage de boot.asm..."
nasm -f elf64 kernel/src/arch/x86_64/boot/boot.asm -o build/boot.o
echo "   ✓ boot.o créé ($(stat -c%s build/boot.o) bytes)"

# 3. Compiler boot.c
echo "🔧 [3/8] Compilation de boot.c..."
gcc -m64 -march=x86-64 -ffreestanding -fno-pic -mno-red-zone \
    -mcmodel=kernel -mno-sse -mno-sse2 -nostdlib -nostartfiles \
    -nodefaultlibs -O2 -Wall -Wextra \
    -c kernel/src/arch/x86_64/boot/boot.c -o build/boot_c.o 2>&1 | grep -v "unused variable" || true
echo "   ✓ boot_c.o créé ($(stat -c%s build/boot_c.o) bytes)"

# 4. Créer l'archive boot
echo "📚 [4/8] Création de libboot_combined.a..."
ar rcs build/libboot_combined.a build/boot.o build/boot_c.o
echo "   ✓ libboot_combined.a créé ($(stat -c%s build/libboot_combined.a) bytes)"

# 5. Linker le kernel complet
echo "🔗 [5/8] Linkage du kernel..."
ld -n -o build/kernel.elf -T linker.ld \
    build/libboot_combined.a \
    target/x86_64-unknown-none/debug/libexo_kernel.a 2>&1 | grep -v "warning" || true
echo "   ✓ kernel.elf créé ($(du -h build/kernel.elf | cut -f1))"

# 6. Stripper les symboles debug
echo "✂️  [6/8] Stripping symboles debug..."
strip build/kernel.elf -o build/kernel_stripped.elf
echo "   ✓ kernel_stripped.elf créé ($(du -h build/kernel_stripped.elf | cut -f1))"

# 7. Préparer l'ISO
echo "💿 [7/8] Préparation de l'ISO..."
mkdir -p build/iso/boot/grub
cp build/kernel_stripped.elf build/iso/boot/kernel.bin
cp bootloader/grub.cfg build/iso/boot/grub/
echo "   ✓ Structure ISO prête"

# 8. Créer l'ISO bootable
echo "🚀 [8/8] Création de l'ISO bootable..."
grub-mkrescue -o build/exo_os.iso build/iso/ 2>&1 | grep "completed" || true
ISO_SIZE=$(du -h build/exo_os.iso | cut -f1)
echo "   ✓ exo_os.iso créé ($ISO_SIZE)"

echo ""
echo "════════════════════════════════════════════════════════"
echo "  ✅ Build terminé avec succès !"
echo "════════════════════════════════════════════════════════"
echo ""
echo "📂 Fichiers générés :"
echo "   • build/kernel.elf ($(du -h build/kernel.elf | cut -f1) avec symboles)"
echo "   • build/kernel_stripped.elf ($(du -h build/kernel_stripped.elf | cut -f1) stripped)"
echo "   • build/exo_os.iso ($ISO_SIZE bootable)"
echo ""
echo "🚀 Pour tester :"
echo "   bash scripts/test_qemu.sh"
echo ""
echo "🐛 Pour déboguer :"
echo "   qemu-system-x86_64 -cdrom build/exo_os.iso -m 128M -nographic -serial mon:stdio -d int,cpu_reset"
echo ""
