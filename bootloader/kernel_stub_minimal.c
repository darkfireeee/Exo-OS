// kernel_stub_minimal.c - Point d'entrée C MINIMAL avec traces VGA
typedef unsigned int uint32_t;
typedef unsigned long long uint64_t;
typedef unsigned short uint16_t;

#define VGA ((uint16_t*)0xB8000)

// Couleurs
#define C_BLACK  0x0
#define C_GREEN  0x2
#define C_CYAN   0x3
#define C_RED    0x4
#define C_WHITE  0xF

void vga_write(int row, int col, const char* str, uint16_t color) {
    uint16_t* ptr = VGA + (row * 80 + col);
    while (*str) {
        *ptr++ = (color << 8) | *str++;
    }
}

extern void rust_main(uint32_t magic, uint64_t multiboot_info) __attribute__((noreturn));

__attribute__((noreturn))
void kernel_main(uint32_t magic, uint64_t multiboot_info) {
    // Ligne 2: Message C
    vga_write(2, 0, "[C] Kernel stub entered", C_GREEN << 8);
    
    // Ligne 3: Magic
    vga_write(3, 0, "[C] Magic: ", C_CYAN << 8);
    if (magic == 0x2BADB002) {
        vga_write(3, 15, "OK", C_GREEN << 8);
    } else {
        vga_write(3, 15, "FAIL", C_RED << 8);
        while(1) __asm__("hlt");
    }
    
    // Ligne 4: Avant appel Rust
    vga_write(4, 0, "[C] Calling Rust...", C_CYAN << 8);
    
    // Appeler Rust
    rust_main(magic, multiboot_info);
    
    // Ne devrait jamais arriver ici
    __builtin_unreachable();
}
