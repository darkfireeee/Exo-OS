// boot_stub.c - Point d'entrée C du kernel avec VGA text mode et traces debug

typedef unsigned long uint64_t;
typedef unsigned int uint32_t;
typedef unsigned short uint16_t;
typedef unsigned char uint8_t;

// Buffer VGA text mode (80x25, 16 couleurs)
#define VGA_BUFFER ((uint16_t*)0xB8000)
#define VGA_WIDTH 80
#define VGA_HEIGHT 25

void vga_clear(void) {
    uint16_t* vga = (uint16_t*)0xB8000;
    for (int i = 0; i < VGA_WIDTH * VGA_HEIGHT; i++) {
        vga[i] = 0x0F20; // Espace blanc sur noir
    }
}

void vga_write(const char* str, int row) {
    uint16_t* vga = (uint16_t*)0xB8000;
    int col = 0;
    while (*str && col < VGA_WIDTH) {
        vga[row * VGA_WIDTH + col] = 0x0F00 | *str;
        str++;
        col++;
    }
}

void kernel_main(uint32_t magic, uint64_t mboot_info) {
    // Initialiser VGA - écrire directement un message
    uint16_t* vga = (uint16_t*)0xB8000;
    
    // Message simple
    const char* msg = "Exo-OS Boot OK";
    for (int i = 0; msg[i] != '\0'; i++) {
        vga[i] = 0x0F00 | msg[i];
    }
    
    // Boucler indéfiniment
    while(1) {
        __asm__ volatile("hlt");
    }
}
