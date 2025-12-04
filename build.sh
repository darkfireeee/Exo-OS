#!/bin/bash
# Build script for Exo-OS (WSL Ubuntu)
# Compiles boot objects, kernel, and creates bootable ISO

set -e  # Exit on error

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "=== Exo-OS Build Script (Ubuntu/WSL) ==="
echo ""

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Check dependencies
echo -e "${BLUE}[1/6] Checking dependencies...${NC}"
MISSING_DEPS=()

command -v nasm >/dev/null 2>&1 || MISSING_DEPS+=("nasm")
command -v gcc >/dev/null 2>&1 || MISSING_DEPS+=("gcc")
command -v ar >/dev/null 2>&1 || MISSING_DEPS+=("binutils")
command -v grub-mkrescue >/dev/null 2>&1 || MISSING_DEPS+=("grub-pc-bin xorriso")

if [ ${#MISSING_DEPS[@]} -ne 0 ]; then
    echo -e "${RED}Missing dependencies: ${MISSING_DEPS[*]}${NC}"
    echo -e "${YELLOW}Install with: sudo apt-get install ${MISSING_DEPS[*]}${NC}"
    exit 1
fi

# Check for Rust (via WSL or native)
USE_WINDOWS_CARGO=false
if ! command -v cargo >/dev/null 2>&1; then
    # Try Windows cargo via .exe
    if command -v cargo.exe >/dev/null 2>&1; then
        USE_WINDOWS_CARGO=true
        echo -e "${YELLOW}Note: Using Windows cargo.exe (Rust not installed in WSL)${NC}"
    else
        echo -e "${RED}Missing: Rust/Cargo${NC}"
        echo -e "${YELLOW}Install in WSL: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh${NC}"
        exit 1
    fi
fi

echo -e "${GREEN}✓ All dependencies found${NC}"
echo ""

# Build boot objects (C + ASM)
echo -e "${BLUE}[2/6] Compiling boot objects...${NC}"
BOOT_DIR="kernel/src/arch/x86_64/boot"
BUILD_DIR="build"
BOOT_OBJ_DIR="$BUILD_DIR/boot_objs"

mkdir -p "$BOOT_OBJ_DIR"

# Compile boot.asm with NASM
echo "  - Compiling boot.asm..."
nasm -f elf64 -o "$BOOT_OBJ_DIR/boot.o" "$BOOT_DIR/boot.asm"

# Compile boot.c with GCC
echo "  - Compiling boot.c..."
gcc -c "$BOOT_DIR/boot.c" \
    -o "$BOOT_OBJ_DIR/boot_c.o" \
    -m64 \
    -ffreestanding \
    -nostdlib \
    -fno-builtin \
    -fno-stack-protector \
    -fno-pic \
    -fno-pie \
    -mno-red-zone \
    -mcmodel=kernel \
    -mno-mmx \
    -mno-sse \
    -mno-sse2 \
    -O2

# Compile C stubs
echo "  - Compiling C stubs..."
gcc -c "kernel/src/c_compat/stubs.c" \
    -o "$BOOT_OBJ_DIR/stubs.o" \
    -m64 \
    -ffreestanding \
    -nostdlib \
    -fno-builtin \
    -fno-stack-protector \
    -fno-pic \
    -fno-pie \
    -mno-red-zone \
    -mcmodel=kernel \
    -mno-mmx \
    -mno-sse \
    -mno-sse2 \
    -O2

# Create static library
echo "  - Creating libboot_combined.a..."
ar rcs "$BOOT_OBJ_DIR/libboot_combined.a" \
    "$BOOT_OBJ_DIR/boot.o" \
    "$BOOT_OBJ_DIR/boot_c.o" \
    "$BOOT_OBJ_DIR/stubs.o"

echo -e "${GREEN}✓ Boot objects compiled${NC}"
echo ""

# Copy to cargo OUT_DIR (find it first)
echo -e "${BLUE}[3/6] Preparing cargo build...${NC}"
# Build once to create OUT_DIR structure
cargo build --target x86_64-unknown-none.json --manifest-path kernel/Cargo.toml >/dev/null 2>&1 || true

# Find the OUT_DIR
OUT_DIR=$(find target/x86_64-unknown-none/debug/build/exo-kernel-*/out -type d 2>/dev/null | head -1)
if [ -z "$OUT_DIR" ]; then
    echo -e "${YELLOW}Warning: Could not find cargo OUT_DIR, using build/ instead${NC}"
    OUT_DIR="$BUILD_DIR"
fi

# Copy boot library to OUT_DIR
cp "$BOOT_OBJ_DIR/libboot_combined.a" "$OUT_DIR/" 2>/dev/null || true
echo -e "${GREEN}✓ Boot objects ready${NC}"
echo ""

# Build Rust kernel
echo -e "${BLUE}[4/6] Building Rust kernel...${NC}"

if [ "$USE_WINDOWS_CARGO" = true ]; then
    # Use Windows cargo
    echo "  Using Windows cargo..."
    cd kernel
    cargo.exe build --target ../x86_64-unknown-none.json --release 2>&1 | tail -20
    cd ..
else
    # Use WSL cargo
    cd kernel
    cargo build --target ../x86_64-unknown-none.json --release
    cd ..
fi

KERNEL_LIB="target/x86_64-unknown-none/release/libexo_kernel.a"
if [ ! -f "$KERNEL_LIB" ]; then
    echo -e "${RED}Error: Kernel library not found at $KERNEL_LIB${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Kernel compiled successfully${NC}"
echo ""

# Link final kernel binary
echo -e "${BLUE}[5/6] Linking kernel binary...${NC}"
mkdir -p "$BUILD_DIR"

# Check if linker script exists
LINKER_SCRIPT="linker/linker.ld"
if [ ! -f "$LINKER_SCRIPT" ]; then
    LINKER_SCRIPT="linker.ld"
fi

if [ ! -f "$LINKER_SCRIPT" ]; then
    echo -e "${RED}Error: Linker script not found${NC}"
    exit 1
fi

# Use LD to create final executable
# First link as ELF
ld -n -T "$LINKER_SCRIPT" \
    --allow-multiple-definition \
    -o "$BUILD_DIR/kernel.elf" \
    "$BOOT_OBJ_DIR/boot.o" \
    "$BOOT_OBJ_DIR/boot_c.o" \
    "$BOOT_OBJ_DIR/stubs.o" \
    --whole-archive \
    "$KERNEL_LIB" \
    --no-whole-archive \
    --gc-sections 2>/dev/null || \
ld -n -T "$LINKER_SCRIPT" \
    --allow-multiple-definition \
    -o "$BUILD_DIR/kernel.elf" \
    "$BOOT_OBJ_DIR/boot.o" \
    "$BOOT_OBJ_DIR/boot_c.o" \
    "$BOOT_OBJ_DIR/stubs.o" \
    "$KERNEL_LIB"

# GRUB multiboot2 can load ELF files directly, just copy it
cp "$BUILD_DIR/kernel.elf" "$BUILD_DIR/kernel.bin"

echo -e "${GREEN}✓ Kernel binary created: $BUILD_DIR/kernel.bin (ELF multiboot2)${NC}"
echo ""

# Create ISO
echo -e "${BLUE}[6/6] Creating bootable ISO...${NC}"
chmod +x scripts/make_iso.sh
./scripts/make_iso.sh

echo ""
echo -e "${GREEN}=== Build completed successfully! ===${NC}"
echo ""
echo "Output files:"
echo "  - Kernel binary: $BUILD_DIR/kernel.bin"
echo "  - Bootable ISO:  $BUILD_DIR/exo_os.iso"
echo ""
echo "To test:"
echo "  ./scripts/test_qemu.sh"
echo "  or"
echo "  pwsh.exe scripts/test_qemu.ps1"
