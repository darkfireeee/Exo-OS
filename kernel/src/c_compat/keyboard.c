// PS/2 Keyboard Driver
// Basic keyboard input support

#include <stdint.h>

#define KBD_DATA_PORT   0x60
#define KBD_STATUS_PORT 0x64
#define KBD_CMD_PORT    0x64

// Keyboard status bits
#define KBD_STATUS_OUTPUT_FULL  0x01
#define KBD_STATUS_INPUT_FULL   0x02

// Inline assembly for port I/O
static inline uint8_t inb(uint16_t port) {
    uint8_t value;
    __asm__ volatile("inb %1, %0" : "=a"(value) : "Nd"(port));
    return value;
}

static inline void outb(uint16_t port, uint8_t value) {
    __asm__ volatile("outb %0, %1" : : "a"(value), "Nd"(port));
}

// Scancode to ASCII table (US layout)
static const char scancode_to_ascii[128] = {
    0, 27, '1', '2', '3', '4', '5', '6', '7', '8', '9', '0', '-', '=', '\b',
    '\t', 'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p', '[', ']', '\n',
    0, 'a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', ';', '\'', '`', 0,
    '\\', 'z', 'x', 'c', 'v', 'b', 'n', 'm', ',', '.', '/', 0, '*', 0, ' '
};

static uint8_t shift_pressed = 0;
static uint8_t ctrl_pressed = 0;
static uint8_t alt_pressed = 0;

// Wait for keyboard controller
static void kbd_wait_input(void) {
    while (inb(KBD_STATUS_PORT) & KBD_STATUS_INPUT_FULL);
}

static void kbd_wait_output(void) {
    while (!(inb(KBD_STATUS_PORT) & KBD_STATUS_OUTPUT_FULL));
}

// Read scancode
uint8_t kbd_read_scancode(void) {
    kbd_wait_output();
    return inb(KBD_DATA_PORT);
}

// Convert scancode to ASCII
char kbd_scancode_to_ascii(uint8_t scancode) {
    // Handle make/break codes
    if (scancode & 0x80) {
        // Key released
        scancode &= 0x7F;
        if (scancode == 0x2A || scancode == 0x36) shift_pressed = 0;
        if (scancode == 0x1D) ctrl_pressed = 0;
        if (scancode == 0x38) alt_pressed = 0;
        return 0;
    }
    
    // Key pressed
    if (scancode == 0x2A || scancode == 0x36) {
        shift_pressed = 1;
        return 0;
    }
    if (scancode == 0x1D) {
        ctrl_pressed = 1;
        return 0;
    }
    if (scancode == 0x38) {
        alt_pressed = 1;
        return 0;
    }
    
    if (scancode >= 128) return 0;
    
    char c = scancode_to_ascii[scancode];
    if (c >= 'a' && c <= 'z' && shift_pressed) {
        c -= 32; // Convert to uppercase
    }
    
    return c;
}

// Initialize keyboard
void kbd_init(void) {
    // Disable first PS/2 port
    kbd_wait_input();
    outb(KBD_CMD_PORT, 0xAD);
    
    // Read configuration byte
    kbd_wait_input();
    outb(KBD_CMD_PORT, 0x20);
    kbd_wait_output();
    uint8_t config = inb(KBD_DATA_PORT);
    
    // Enable interrupts and translation
    config |= 0x01;  // Enable first port interrupt
    config &= ~0x10; // Enable first port clock
    
    // Write configuration byte
    kbd_wait_input();
    outb(KBD_CMD_PORT, 0x60);
    kbd_wait_input();
    outb(KBD_DATA_PORT, config);
    
    // Enable first PS/2 port
    kbd_wait_input();
    outb(KBD_CMD_PORT, 0xAE);
}
