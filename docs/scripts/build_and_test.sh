#!/bin/bash
set -e
cd /tmp/exo-os

echo '=== Building Exo-OS Kernel ==='

# Assembler le bootloader
echo 'Assembling bootloader...'
nasm -f elf64 boot.asm -o boot.o

# Créer un stub Rust minimal pour tester
echo 'Creating kernel stub...'
cat > kernel_stub.rs << 'EOF'
#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[no_mangle]
pub extern "C" fn rust_kernel_main(_multiboot_info: u32) -> ! {
    // Écrire sur le serial port (COM1)
    unsafe {
        let port = 0x3F8u16;
        let msg = b"Exo-OS Kernel Started!\n";
        for &byte in msg {
            core::arch::asm!(
                "out dx, al",
                in("dx") port,
                in("al") byte,
                options(nomem, nostack, preserves_flags)
            );
        }
    }
    loop {
        unsafe { core::arch::asm!("hlt"); }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe { core::arch::asm!("hlt"); }
    }
}
EOF

echo 'Compiling kernel stub with rustc...'
rustc --edition 2021 --target x86_64-unknown-none --crate-type staticlib -C opt-level=2 kernel_stub.rs -o kernel_stub.a 2>&1 | head -20

if [ ! -f kernel_stub.a ]; then
    echo "ERROR: Failed to compile kernel stub"
    exit 1
fi

# Linker script
cat > linker.ld << 'EOF'
ENTRY(_start)

SECTIONS {
    . = 1M;
    
    .boot : {
        *(.multiboot)
        *(.bootstrap)
    }
    
    .text : {
        *(.text*)
    }
    
    .rodata : {
        *(.rodata*)
    }
    
    .data : {
        *(.data*)
    }
    
    .bss : {
        *(.bss*)
        *(COMMON)
    }
}
EOF

echo 'Linking kernel...'
ld -n -o exo-kernel.elf -T linker.ld boot.o kernel_stub.a

echo 'Creating bootable binary...'
objcopy -O binary exo-kernel.elf exo-kernel.bin

echo 'Kernel built successfully!'
ls -lh exo-kernel.*

echo ''
echo '=== Launching in QEMU ==='
echo 'Press Ctrl+A then X to exit QEMU'
echo ''
sleep 2

qemu-system-x86_64 -serial stdio -nographic exo-kernel.bin
