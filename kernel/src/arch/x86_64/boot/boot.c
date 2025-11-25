/**
 * ═══════════════════════════════════════════════════════════════════════════
 * Exo-OS Boot Bridge (C → Rust)
 * ═══════════════════════════════════════════════════════════════════════════
 * 
 * Ce fichier fait le pont entre le bootstrap assembleur (boot.asm)
 * et le kernel Rust (rust_main).
 * 
 * Responsabilités :
 * 1. Initialisation matérielle de base (serial port pour debug)
 * 2. Parsing des informations Multiboot2
 * 3. Configuration précoce de la mémoire
 * 4. Appel de rust_main() avec les bonnes informations
 * 
 * ═══════════════════════════════════════════════════════════════════════════
 */

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

// ────────────────────────────────────────────────────────────────────────────
// Multiboot2 Structures
// ────────────────────────────────────────────────────────────────────────────

#define MULTIBOOT2_MAGIC 0x36D76289

// Tag types
#define MULTIBOOT_TAG_TYPE_END              0
#define MULTIBOOT_TAG_TYPE_CMDLINE          1
#define MULTIBOOT_TAG_TYPE_BOOT_LOADER_NAME 2
#define MULTIBOOT_TAG_TYPE_MODULE           3
#define MULTIBOOT_TAG_TYPE_BASIC_MEMINFO    4
#define MULTIBOOT_TAG_TYPE_BOOTDEV          5
#define MULTIBOOT_TAG_TYPE_MMAP             6
#define MULTIBOOT_TAG_TYPE_VBE              7
#define MULTIBOOT_TAG_TYPE_FRAMEBUFFER      8
#define MULTIBOOT_TAG_TYPE_ELF_SECTIONS     9
#define MULTIBOOT_TAG_TYPE_APM              10
#define MULTIBOOT_TAG_TYPE_EFI32            11
#define MULTIBOOT_TAG_TYPE_EFI64            12
#define MULTIBOOT_TAG_TYPE_SMBIOS           13
#define MULTIBOOT_TAG_TYPE_ACPI_OLD         14
#define MULTIBOOT_TAG_TYPE_ACPI_NEW         15
#define MULTIBOOT_TAG_TYPE_NETWORK          16
#define MULTIBOOT_TAG_TYPE_EFI_MMAP         17
#define MULTIBOOT_TAG_TYPE_EFI_BS           18
#define MULTIBOOT_TAG_TYPE_EFI32_IH         19
#define MULTIBOOT_TAG_TYPE_EFI64_IH         20
#define MULTIBOOT_TAG_TYPE_LOAD_BASE_ADDR   21

struct multiboot_tag {
    uint32_t type;
    uint32_t size;
} __attribute__((packed));

struct multiboot_tag_string {
    uint32_t type;
    uint32_t size;
    char string[0];
} __attribute__((packed));

struct multiboot_tag_basic_meminfo {
    uint32_t type;
    uint32_t size;
    uint32_t mem_lower;
    uint32_t mem_upper;
} __attribute__((packed));

struct multiboot_mmap_entry {
    uint64_t addr;
    uint64_t len;
    uint32_t type;
    uint32_t zero;
} __attribute__((packed));

struct multiboot_tag_mmap {
    uint32_t type;
    uint32_t size;
    uint32_t entry_size;
    uint32_t entry_version;
    struct multiboot_mmap_entry entries[0];
} __attribute__((packed));

// ────────────────────────────────────────────────────────────────────────────
// Serial Port (COM1) - Debug output précoce
// ────────────────────────────────────────────────────────────────────────────

#define COM1_PORT 0x3F8

static inline void outb(uint16_t port, uint8_t val) {
    __asm__ volatile ("outb %0, %1" : : "a"(val), "Nd"(port));
}

static inline uint8_t inb(uint16_t port) {
    uint8_t ret;
    __asm__ volatile ("inb %1, %0" : "=a"(ret) : "Nd"(port));
    return ret;
}

static void serial_init(void) {
    outb(COM1_PORT + 1, 0x00);    // Disable interrupts
    outb(COM1_PORT + 3, 0x80);    // Enable DLAB (set baud rate divisor)
    outb(COM1_PORT + 0, 0x03);    // Divisor low byte (38400 baud)
    outb(COM1_PORT + 1, 0x00);    // Divisor high byte
    outb(COM1_PORT + 3, 0x03);    // 8 bits, no parity, one stop bit
    outb(COM1_PORT + 2, 0xC7);    // Enable FIFO, clear, 14-byte threshold
    outb(COM1_PORT + 4, 0x0B);    // IRQs enabled, RTS/DSR set
}

static int serial_is_transmit_empty(void) {
    return inb(COM1_PORT + 5) & 0x20;
}

static void serial_write_char(char c) {
    while (!serial_is_transmit_empty());
    outb(COM1_PORT, c);
}

static void serial_write_string(const char* str) {
    while (*str) {
        if (*str == '\n') {
            serial_write_char('\r');
        }
        serial_write_char(*str++);
    }
}

// ────────────────────────────────────────────────────────────────────────────
// VGA Text Mode - Fallback display
// ────────────────────────────────────────────────────────────────────────────

#define VGA_WIDTH  80
#define VGA_HEIGHT 25
#define VGA_BUFFER 0xB8000

static uint16_t* const vga_buffer = (uint16_t*)VGA_BUFFER;
static size_t vga_row = 0;
static size_t vga_col = 0;
static uint8_t vga_color = 0x07; // Light grey on black

static void vga_clear(void) {
    for (size_t y = 0; y < VGA_HEIGHT; y++) {
        for (size_t x = 0; x < VGA_WIDTH; x++) {
            vga_buffer[y * VGA_WIDTH + x] = (vga_color << 8) | ' ';
        }
    }
    vga_row = 0;
    vga_col = 0;
}

static void vga_putchar(char c) {
    if (c == '\n') {
        vga_col = 0;
        vga_row++;
        if (vga_row >= VGA_HEIGHT) {
            vga_row = 0;
        }
        return;
    }
    
    size_t index = vga_row * VGA_WIDTH + vga_col;
    vga_buffer[index] = (vga_color << 8) | c;
    
    vga_col++;
    if (vga_col >= VGA_WIDTH) {
        vga_col = 0;
        vga_row++;
        if (vga_row >= VGA_HEIGHT) {
            vga_row = 0;
        }
    }
}

static void vga_write_string(const char* str) {
    while (*str) {
        vga_putchar(*str++);
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Debug print (serial + vga)
// ────────────────────────────────────────────────────────────────────────────

static void debug_print(const char* str) {
    serial_write_string(str);
    vga_write_string(str);
}

// ────────────────────────────────────────────────────────────────────────────
// Multiboot2 parsing
// ────────────────────────────────────────────────────────────────────────────

static void parse_multiboot2(uint64_t mbi_addr) {
    struct multiboot_tag* tag;
    uint32_t total_size = *(uint32_t*)mbi_addr;
    
    debug_print("[BOOT] Multiboot2 info detected\n");
    
    // Skip size (4 bytes) and reserved (4 bytes)
    tag = (struct multiboot_tag*)(mbi_addr + 8);
    
    while (tag->type != MULTIBOOT_TAG_TYPE_END) {
        switch (tag->type) {
            case MULTIBOOT_TAG_TYPE_CMDLINE: {
                struct multiboot_tag_string* cmdline = (struct multiboot_tag_string*)tag;
                debug_print("[BOOT] Command line: ");
                debug_print(cmdline->string);
                debug_print("\n");
                break;
            }
            
            case MULTIBOOT_TAG_TYPE_BOOT_LOADER_NAME: {
                struct multiboot_tag_string* bootloader = (struct multiboot_tag_string*)tag;
                debug_print("[BOOT] Bootloader: ");
                debug_print(bootloader->string);
                debug_print("\n");
                break;
            }
            
            case MULTIBOOT_TAG_TYPE_BASIC_MEMINFO: {
                struct multiboot_tag_basic_meminfo* meminfo = (struct multiboot_tag_basic_meminfo*)tag;
                debug_print("[BOOT] Basic memory info detected\n");
                break;
            }
            
            case MULTIBOOT_TAG_TYPE_MMAP: {
                debug_print("[BOOT] Memory map detected\n");
                break;
            }
            
            default:
                // Ignore other tags
                break;
        }
        
        // Move to next tag (aligned to 8 bytes)
        tag = (struct multiboot_tag*)((uint8_t*)tag + ((tag->size + 7) & ~7));
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Rust interface
// ────────────────────────────────────────────────────────────────────────────

// Déclaré dans kernel Rust (lib.rs)
extern void rust_kernel_entry(uint32_t magic, uint64_t multiboot_info) __attribute__((noreturn));

// ────────────────────────────────────────────────────────────────────────────
// boot_main - Point d'entrée depuis boot.asm
// ────────────────────────────────────────────────────────────────────────────

void __attribute__((noreturn)) boot_main(uint32_t magic, uint64_t multiboot_info) {
    // Initialize serial port for early debug
    serial_init();
    
    // Clear VGA screen
    vga_clear();
    
    // Print boot message
    debug_print("═══════════════════════════════════════════════════════\n");
    debug_print("  Exo-OS Kernel v0.4.0 - Booting...\n");
    debug_print("═══════════════════════════════════════════════════════\n");
    debug_print("\n");
    
    // Verify Multiboot2 magic
    if (magic != MULTIBOOT2_MAGIC) {
        debug_print("[ERROR] Invalid Multiboot2 magic number!\n");
        debug_print("[ERROR] Expected: 0x36D76289\n");
        debug_print("[ERROR] System halted.\n");
        while (1) {
            __asm__ volatile ("hlt");
        }
    }
    
    debug_print("[BOOT] Multiboot2 magic verified\n");
    
    // Parse Multiboot2 information
    if (multiboot_info != 0) {
        parse_multiboot2(multiboot_info);
    } else {
        debug_print("[WARN] No Multiboot2 info provided\n");
    }
    
    debug_print("[BOOT] Jumping to Rust kernel...\n");
    debug_print("\n");
    
    // Jump to Rust kernel
    rust_kernel_entry(magic, multiboot_info);
    
    // Should never reach here
    debug_print("[ERROR] Rust kernel returned!\n");
    while (1) {
        __asm__ volatile ("hlt");
    }
}
