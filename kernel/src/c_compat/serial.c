// Serial Driver (COM1)
// Basic serial port driver for early debug output

#include <stdint.h>

#define COM1_PORT 0x3F8

static inline void outb(uint16_t port, uint8_t value) {
    __asm__ volatile("outb %0, %1" : : "a"(value), "Nd"(port));
}

static inline uint8_t inb(uint16_t port) {
    uint8_t value;
    __asm__ volatile("inb %1, %0" : "=a"(value) : "Nd"(port));
    return value;
}

// Initialize COM1
void serial_init(void) {
    outb(COM1_PORT + 1, 0x00);    // Disable interrupts
    outb(COM1_PORT + 3, 0x80);    // Enable DLAB (set baud rate divisor)
    outb(COM1_PORT + 0, 0x03);    // Set divisor to 3 (38400 baud)
    outb(COM1_PORT + 1, 0x00);    //
    outb(COM1_PORT + 3, 0x03);    // 8 bits, no parity, one stop bit
    outb(COM1_PORT + 2, 0xC7);    // Enable FIFO, clear them, 14-byte threshold
    outb(COM1_PORT + 4, 0x0B);    // IRQs enabled, RTS/DSR set
}

// Check if transmit is empty
static int serial_transmit_empty(void) {
    return inb(COM1_PORT + 5) & 0x20;
}

// Write character
void serial_putc(char c) {
    while (!serial_transmit_empty());
    outb(COM1_PORT, c);
}

// Write string
void serial_puts(const char* str) {
    while (*str) {
        if (*str == '\n') {
            serial_putc('\r');
        }
        serial_putc(*str++);
    }
}

// Check if data is available
int serial_received(void) {
    return inb(COM1_PORT + 5) & 1;
}

// Read character
char serial_getc(void) {
    while (!serial_received());
    return inb(COM1_PORT);
}
