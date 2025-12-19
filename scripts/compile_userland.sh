#!/bin/bash
# Compile test binaries for Exo-OS
# Creates minimal statically-linked ELF64 binaries with no libc

set -e

echo "🔨 Building Exo-OS Test Binaries..."
echo ""

# Create bin directory
mkdir -p userland/bin

# Compilation flags for minimal ELF
CFLAGS="-nostdlib -static -fno-pic -fno-stack-protector -mno-red-zone"
LDFLAGS="-nostdlib -static -Wl,--build-id=none"

# Function to compile and strip
compile_test() {
    local name=$1
    local src="userland/${name}.c"
    local elf="userland/bin/${name}.elf"
    
    echo "📝 Compiling ${name}..."
    
    # Compile with GCC
    gcc $CFLAGS $LDFLAGS -o "$elf" "$src"
    
    # Strip debug symbols to reduce size
    strip -s "$elf"
    
    # Show file info
    local size=$(stat -c%s "$elf" 2>/dev/null || stat -f%z "$elf" 2>/dev/null || echo "unknown")
    echo "   ✅ Created $elf ($size bytes)"
    
    # Verify it's ELF64 (check magic bytes)
    if head -c 4 "$elf" | grep -q "ELF"; then
        echo "   ✅ Valid ELF binary"
    else
        echo "   ⚠️  Warning: May not be ELF format"
    fi
    
    echo ""
}

# Compile all test binaries
compile_test "test_hello"
compile_test "test_fork_exec"
compile_test "test_pipe"

# Also compile the original hello.c if it exists
if [ -f "userland/hello.c" ]; then
    echo "📝 Compiling hello (legacy)..."
    gcc $CFLAGS $LDFLAGS -o "userland/bin/hello.elf" "userland/hello.c"
    strip -s "userland/bin/hello.elf"
    size=$(stat -c%s "userland/bin/hello.elf" 2>/dev/null || stat -f%z "userland/bin/hello.elf" 2>/dev/null || echo "unknown")
    echo "   ✅ Created userland/bin/hello.elf ($size bytes)"
    echo ""
fi

echo "🎉 All test binaries compiled successfully!"
echo ""
echo "Created binaries:"
ls -lh userland/bin/*.elf
echo ""
echo "Next steps:"
echo "  1. These ELF files will be embedded in the kernel"
echo "  2. VFS will load them into tmpfs at boot"
echo "  3. exec() can run them from /tmp/"
