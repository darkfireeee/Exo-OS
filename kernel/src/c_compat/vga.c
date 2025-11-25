// VGA Text Mode Driver
// 80x25 text mode with color support

#include <stdint.h>

#define VGA_BUFFER 0xB8000
#define VGA_WIDTH  80
#define VGA_HEIGHT 25

static uint16_t* vga_buffer = (uint16_t*)VGA_BUFFER;
static uint8_t vga_color = 0x0F; // White on black
static uint8_t vga_row = 0;
static uint8_t vga_col = 0;

// Set text color
void vga_set_color(uint8_t fg, uint8_t bg) {
    vga_color = (bg << 4) | (fg & 0x0F);
}

// Clear screen
void vga_clear(void) {
    uint16_t blank = (vga_color << 8) | ' ';
    for (int i = 0; i < VGA_WIDTH * VGA_HEIGHT; i++) {
        vga_buffer[i] = blank;
    }
    vga_row = 0;
    vga_col = 0;
}

// Scroll screen
static void vga_scroll(void) {
    // Move all lines up
    for (int y = 0; y < VGA_HEIGHT - 1; y++) {
        for (int x = 0; x < VGA_WIDTH; x++) {
            vga_buffer[y * VGA_WIDTH + x] = vga_buffer[(y + 1) * VGA_WIDTH + x];
        }
    }
    
    // Clear last line
    uint16_t blank = (vga_color << 8) | ' ';
    for (int x = 0; x < VGA_WIDTH; x++) {
        vga_buffer[(VGA_HEIGHT - 1) * VGA_WIDTH + x] = blank;
    }
    
    vga_row = VGA_HEIGHT - 1;
}

// Put character at position
void vga_putc_at(char c, uint8_t x, uint8_t y) {
    if (x >= VGA_WIDTH || y >= VGA_HEIGHT) return;
    vga_buffer[y * VGA_WIDTH + x] = (vga_color << 8) | c;
}

// Put character
void vga_putc(char c) {
    if (c == '\n') {
        vga_col = 0;
        vga_row++;
    } else if (c == '\r') {
        vga_col = 0;
    } else if (c == '\t') {
        vga_col = (vga_col + 8) & ~7;
    } else {
        vga_putc_at(c, vga_col, vga_row);
        vga_col++;
    }
    
    if (vga_col >= VGA_WIDTH) {
        vga_col = 0;
        vga_row++;
    }
    
    if (vga_row >= VGA_HEIGHT) {
        vga_scroll();
    }
}

// Put string
void vga_puts(const char* str) {
    while (*str) {
        vga_putc(*str++);
    }
}

// Initialize VGA
void vga_init(void) {
    vga_clear();
}
