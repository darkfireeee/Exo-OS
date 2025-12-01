#!/bin/bash
# link_boot.sh - Script pour lier les fichiers objets boot avec le kernel
# Ce script crée une archive statique des fichiers boot.o pour rust-lld

set -e

OUT_DIR=${1:-"target/x86_64-unknown-none/debug"}
BOOT_ASM="kernel/src/arch/x86_64/boot/boot.asm"
BOOT_C="kernel/src/arch/x86_64/boot/boot.c"

echo "=== Exo-OS Boot Linker Script ==="
echo "OUT_DIR: $OUT_DIR"

# Créer le répertoire de sortie si nécessaire
mkdir -p "$OUT_DIR/boot_objs"

# Compiler boot.asm avec NASM
echo "[1/4] Compiling boot.asm with NASM..."
nasm -f elf64 -o "$OUT_DIR/boot_objs/boot.o" "$BOOT_ASM"

# Compiler boot.c avec GCC
echo "[2/4] Compiling boot.c with GCC..."
gcc -c "$BOOT_C" \
    -o "$OUT_DIR/boot_objs/boot_c.o" \
    -ffreestanding \
    -nostdlib \
    -fno-builtin \
    -fno-stack-protector \
    -mno-red-zone \
    -fno-pic \
    -fno-pie \
    -m64

# Créer une archive statique
echo "[3/4] Creating static library libboot_combined.a..."
ar rcs "$OUT_DIR/boot_objs/libboot_combined.a" \
    "$OUT_DIR/boot_objs/boot.o" \
    "$OUT_DIR/boot_objs/boot_c.o"

# Copier dans le répertoire de recherche de cargo
echo "[4/4] Copying to cargo output directory..."
cp "$OUT_DIR/boot_objs/libboot_combined.a" "$OUT_DIR/"

echo "✅ Boot objects linked successfully!"
echo "Library: $OUT_DIR/libboot_combined.a"
