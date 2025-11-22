/**
 * Boot Stub for Exo-OS Kernel
 * 
 * Minimal C entry point that:
 * 1. Validates multiboot2 magic
 * 2. Sets up basic stack
 * 3. Calls Rust kernel_main
 */

#include <stdint.h>
#include <stddef.h>

// Multiboot2 constants
#define MULTIBOOT2_MAGIC 0x36d76289

// External symbols from linker script
extern void _stack_top;

// Rust kernel entry point (from lib.rs)
extern void _start(uint32_t magic, void* multiboot_info) __attribute__((noreturn));

// Multiboot2 header - must be in first 32KB
__attribute__((section(".multiboot")))
__attribute__((aligned(8)))
static const uint32_t multiboot_header[] = {
    0xe85250d6,  // Magic
    0,           // Architecture (i386)
    32,          // Header length
    0x17adaf12,  // Checksum (-(magic + arch + length))
    
    // End tag
    0, 0,
    8
};

// Boot entry point called by GRUB
__attribute__((noreturn))
void boot_main(uint32_t magic, void* multiboot_info) {
    // Validate multiboot2 magic
    if (magic != MULTIBOOT2_MAGIC) {
        // Halt if invalid - no serial output yet
        while(1) {
            __asm__ volatile("hlt");
        }
    }
    
    // Call Rust kernel
    _start(magic, multiboot_info);
    
    // Should never reach here
    while(1) {
        __asm__ volatile("hlt");
    }
}
