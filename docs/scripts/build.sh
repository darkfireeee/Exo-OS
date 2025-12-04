#!/bin/bash
# Script de compilation et test du kernel Exo-OS

set -e  # Arrêter en cas d'erreur

echo "=== Compilation du kernel Exo-OS ==="

# Vérifier que nous sommes dans le bon répertoire
if [ ! -f "Cargo.toml" ]; then
    echo "Erreur: Cargo.toml introuvable. Exécutez ce script depuis la racine du projet."
    exit 1
fi

# Vérifier que la compilation Rust existe déjà (compilée sous Windows)
if [ ! -f "target/x86_64-unknown-none/release/libexo_kernel.a" ]; then
    echo "Erreur: libexo_kernel.a non trouvé"
    echo "Compilez d'abord avec: cargo build --release --lib (sous Windows)"
    exit 1
fi

echo "-> Kernel Rust trouvé: $(ls -lh target/x86_64-unknown-none/release/libexo_kernel.a | awk '{print $5}')"

# Créer le répertoire build
mkdir -p build

# Compiler le kernel stub C
echo "-> Compilation du kernel stub C..."
gcc -m64 -ffreestanding -fno-pic -mno-red-zone -mcmodel=kernel \
    -nostdlib -nostartfiles -nodefaultlibs -O0 -Wall -Wextra \
    -c bootloader/kernel_stub.c -o build/kernel_stub.o

# Assembler le bootloader
echo "-> Assemblage du bootloader (boot.asm)..."
nasm -f elf64 bootloader/boot.asm -o build/boot.o

# Linker le tout pour créer l'image du kernel
echo "-> Linkage final..."
ld -n -o build/exo_kernel.elf -T linker/linker.ld \
    build/boot.o \
    build/kernel_stub.o \
    target/x86_64-unknown-none/release/libexo_kernel.a

echo "-> Image kernel créée: build/exo_kernel.elf"
ls -lh build/exo_kernel.elf

echo ""
echo "=== Compilation terminée avec succès ==="
echo ""
