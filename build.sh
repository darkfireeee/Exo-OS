#!/bin/bash
# Build script for Exo-OS (Ubuntu/WSL/Alpine/Codespaces)
# Compiles boot objects, kernel, and creates bootable ISO
# Auto-installs dependencies if missing

set -e  # Exit on error

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "=== Exo-OS Build Script with Auto-Install ==="
echo ""

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Detect OS/distro
if [ -f /etc/os-release ]; then
    . /etc/os-release
    OS_ID="$ID"
else
    OS_ID="unknown"
fi

echo -e "${BLUE}Detected OS: $OS_ID${NC}"
echo ""

# Function to install packages based on distro
install_package() {
    local pkg=$1
    echo -e "${YELLOW}Installing $pkg...${NC}"
    
    case "$OS_ID" in
        alpine)
            apk add --no-cache $pkg
            ;;
        ubuntu|debian)
            apt-get update -qq && apt-get install -y $pkg
            ;;
        fedora|rhel|centos)
            dnf install -y $pkg
            ;;
        arch)
            pacman -S --noconfirm $pkg
            ;;
        *)
            echo -e "${RED}Unknown distro, cannot auto-install $pkg${NC}"
            return 1
            ;;
    esac
}

# Check and install dependencies
echo -e "${BLUE}[1/7] Checking and installing dependencies...${NC}"

# Check NASM
if ! command -v nasm >/dev/null 2>&1; then
    echo -e "${YELLOW}NASM not found, installing...${NC}"
    install_package nasm
fi

# Check GCC/Clang
if ! command -v gcc >/dev/null 2>&1 && ! command -v clang >/dev/null 2>&1; then
    echo -e "${YELLOW}Compiler not found, installing GCC...${NC}"
    case "$OS_ID" in
        alpine)
            install_package "gcc g++ musl-dev"
            ;;
        *)
            install_package "gcc g++"
            ;;
    esac
fi

# Check binutils (ld, ar)
if ! command -v ar >/dev/null 2>&1; then
    echo -e "${YELLOW}Binutils not found, installing...${NC}"
    install_package binutils
fi

# Check GRUB tools
if ! command -v grub-mkrescue >/dev/null 2>&1; then
    echo -e "${YELLOW}GRUB tools not found, installing...${NC}"
    case "$OS_ID" in
        alpine)
            install_package "grub grub-bios xorriso"
            ;;
        ubuntu|debian)
            install_package "grub-pc-bin xorriso"
            ;;
        *)
            install_package "grub2-tools xorriso"
            ;;
    esac
fi

# Check QEMU (for testing)
if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
    echo -e "${YELLOW}QEMU not found, installing...${NC}"
    case "$OS_ID" in
        alpine)
            install_package "qemu-system-x86_64"
            ;;
        ubuntu|debian)
            install_package "qemu-system-x86"
            ;;
        *)
            install_package "qemu-system-x86"
            ;;
    esac
fi

# Check for Rust and install if missing
echo -e "${BLUE}[2/7] Checking Rust installation...${NC}"

USE_WINDOWS_CARGO=false
if ! command -v cargo >/dev/null 2>&1; then
    # Try Windows cargo via .exe
    if command -v cargo.exe >/dev/null 2>&1; then
        USE_WINDOWS_CARGO=true
        echo -e "${YELLOW}Note: Using Windows cargo.exe${NC}"
    else
        echo -e "${YELLOW}Rust not found, installing...${NC}"
        
        # Install rustup
        export CARGO_HOME="${CARGO_HOME:-/tmp/rust-codespace/.cargo}"
        export RUSTUP_HOME="${RUSTUP_HOME:-/tmp/rust-codespace/.rustup}"
        export PATH="$CARGO_HOME/bin:$PATH"
        
        # Create directories
        mkdir -p "$CARGO_HOME" "$RUSTUP_HOME"
        
        # Install Rust
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
        
        # Source cargo env
        source "$CARGO_HOME/env"
        
        # Install nightly (required for Exo-OS)
        rustup toolchain install nightly
        rustup default nightly
        
        # Add rust-src component (required for no_std)
        rustup component add rust-src
        
        echo -e "${GREEN}✓ Rust installed successfully${NC}"
    fi
else
    # Rust already installed, check nightly
    export PATH="$CARGO_HOME/bin:$PATH"
    
    if ! rustup toolchain list | grep -q nightly; then
        echo -e "${YELLOW}Installing Rust nightly...${NC}"
        rustup toolchain install nightly
    fi
    
    # Set nightly as default
    rustup default nightly
    
    # Ensure rust-src is installed
    if ! rustup component list | grep -q "rust-src.*installed"; then
        echo -e "${YELLOW}Installing rust-src component...${NC}"
        rustup component add rust-src
    fi
fi

echo -e "${GREEN}✓ All dependencies installed${NC}"
echo ""

# Build boot objects (C + ASM)
echo -e "${BLUE}[3/7] Compiling boot objects...${NC}"
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
echo -e "${BLUE}[4/7] Preparing cargo build...${NC}"
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
echo -e "${BLUE}[5/7] Building Rust kernel...${NC}"

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
echo -e "${BLUE}[6/7] Linking kernel binary...${NC}"
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
echo -e "${BLUE}[7/7] Creating bootable ISO...${NC}"

# Prepare ISO directory structure
ISO_DIR="$BUILD_DIR/iso"
mkdir -p "$ISO_DIR/boot/grub"

# Copy kernel (both .bin for grub.cfg and .elf as backup)
cp "$BUILD_DIR/kernel.bin" "$ISO_DIR/boot/"
cp "$BUILD_DIR/kernel.elf" "$ISO_DIR/boot/"

# Copy GRUB configuration
cp bootloader/grub.cfg "$ISO_DIR/boot/grub/"

# Create ISO with grub-mkrescue
if command -v grub-mkrescue >/dev/null 2>&1; then
    grub-mkrescue -o "$BUILD_DIR/exo_os.iso" "$ISO_DIR" 2>&1 | grep -E "(Writing|completed)" || true
    echo -e "${GREEN}✓ ISO created: $BUILD_DIR/exo_os.iso${NC}"
else
    echo -e "${YELLOW}Warning: grub-mkrescue not found, ISO not created${NC}"
fi

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
