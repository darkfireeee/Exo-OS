// Stub implementations for missing C functions
// These are temporary stubs until proper implementations are added

#include <stdint.h>

// VGA functions
void vga_init(void) {
    // TODO: Initialize VGA text mode
}

void vga_clear(void) {
    volatile uint16_t *vga = (uint16_t*)0xB8000;
    for (int i = 0; i < 80*25; i++) {
        vga[i] = 0x0F20; // White on black, space
    }
}

void vga_putc(char c) {
    static int x = 0, y = 0;
    volatile uint16_t *vga = (uint16_t*)0xB8000;
    
    if (c == '\n') {
        x = 0;
        y++;
    } else {
        vga[y * 80 + x] = 0x0F00 | c;
        x++;
        if (x >= 80) {
            x = 0;
            y++;
        }
    }
    
    if (y >= 25) {
        y = 0;
    }
}

void vga_puts(const char *s) {
    while (*s) {
        vga_putc(*s++);
    }
}

void vga_set_color(uint8_t fg, uint8_t bg) {
    // TODO: Set VGA color
}

// Keyboard functions
void keyboard_init(void) {
    // TODO: Initialize keyboard driver
}

char keyboard_getc(void) {
    // TODO: Read from keyboard buffer
    return 0;
}

int keyboard_has_input(void) {
    // TODO: Check if keyboard input is available
    return 0;
}

// PCI functions
void pci_init(void) {
    // TODO: Initialize PCI bus
}

// ACPI functions
void acpi_init(uint64_t rsdp_addr) {
    // TODO: Initialize ACPI
    (void)rsdp_addr;
}

void acpi_shutdown(void) {
    // TODO: ACPI shutdown
    while(1) __asm__ volatile ("hlt");
}

void acpi_reboot(void) {
    // TODO: ACPI reboot
    // Use keyboard controller for now
    __asm__ volatile (
        "mov $0xFE, %al\n"
        "out %al, $0x64\n"
    );
    while(1) __asm__ volatile ("hlt");
}

// Serial port functions (COM1 0x3F8)
#define COM1_PORT 0x3F8

static inline void outb(uint16_t port, uint8_t val) {
    __asm__ volatile ("outb %0, %1" : : "a"(val), "Nd"(port));
}

static inline uint8_t inb(uint16_t port) {
    uint8_t ret;
    __asm__ volatile ("inb %1, %0" : "=a"(ret) : "Nd"(port));
    return ret;
}

void serial_init(void) {
    outb(COM1_PORT + 1, 0x00);    // Disable interrupts
    outb(COM1_PORT + 3, 0x80);    // Enable DLAB
    outb(COM1_PORT + 0, 0x03);    // Divisor low (38400 baud)
    outb(COM1_PORT + 1, 0x00);    // Divisor high
    outb(COM1_PORT + 3, 0x03);    // 8N1
    outb(COM1_PORT + 2, 0xC7);    // Enable FIFO
    outb(COM1_PORT + 4, 0x0B);    // IRQs enabled
}

static int serial_is_transmit_empty(void) {
    return inb(COM1_PORT + 5) & 0x20;
}

void serial_putc(uint8_t c) {
    while (!serial_is_transmit_empty());
    outb(COM1_PORT, c);
}

void serial_puts(const uint8_t *str) {
    while (*str) {
        if (*str == '\n') {
            serial_putc('\r');
        }
        serial_putc(*str++);
    }
}

uint8_t serial_getc(void) {
    while (!(inb(COM1_PORT + 5) & 0x01));
    return inb(COM1_PORT);
}
