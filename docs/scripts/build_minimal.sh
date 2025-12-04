#!/bin/bash
# build_minimal.sh - Build MINIMAL pour debug

set -e

echo "=== BUILD MINIMAL ==="

# 1. Compiler Rust
echo "-> Rust lib..."
if [ ! -f "target/x86_64-unknown-none/release/libexo_kernel.a" ]; then
    echo "ERROR: Compilez d'abord avec: cargo build --release --lib --target x86_64-unknown-none.json"
    exit 1
fi

# 2. Créer dossier build
mkdir -p build

# 3. Compiler C stub
echo "-> C stub..."
gcc -m64 -ffreestanding -fno-pie -fno-stack-protector -mno-red-zone \
    -c bootloader/kernel_stub_minimal.c -o build/kernel_stub.o

# 4. Assembler bootloader
echo "-> ASM bootloader..."
nasm -f elf64 bootloader/boot_minimal.asm -o build/boot.o

# 5. Linker
echo "-> Linking..."
ld -n -o build/exo_kernel.elf -T linker/linker_minimal.ld \
    build/boot.o \
    build/kernel_stub.o \
    target/x86_64-unknown-none/release/libexo_kernel.a

echo "-> ELF créé: build/exo_kernel.elf"
ls -lh build/exo_kernel.elf

echo "=== BUILD OK ==="
