// kernel_stub.c - Point d'entrée C du kernel Exo-OS
// Code minimal pour valider le boot en mode 64-bit

// Types de base
typedef unsigned long long uint64_t;
typedef unsigned int uint32_t;
typedef unsigned short uint16_t;
typedef unsigned char uint8_t;

// Buffer VGA text mode (0xB8000, 80x25, format: [attribut][caractère])
#define VGA_BUFFER ((uint16_t*)0xB8000)
#define VGA_WIDTH 80
#define VGA_HEIGHT 25
#define VGA_COLOR(fg, bg) ((bg << 4) | fg)

// Couleurs VGA standard
enum vga_color {
    VGA_BLACK = 0,
    VGA_BLUE = 1,
    VGA_GREEN = 2,
    VGA_CYAN = 3,
    VGA_RED = 4,
    VGA_MAGENTA = 5,
    VGA_BROWN = 6,
    VGA_LIGHT_GREY = 7,
    VGA_DARK_GREY = 8,
    VGA_LIGHT_BLUE = 9,
    VGA_LIGHT_GREEN = 10,
    VGA_LIGHT_CYAN = 11,
    VGA_LIGHT_RED = 12,
    VGA_LIGHT_MAGENTA = 13,
    VGA_YELLOW = 14,
    VGA_WHITE = 15,
};

// Curseur VGA global
static uint32_t vga_row = 0;
static uint32_t vga_col = 0;
static uint8_t vga_color = VGA_COLOR(VGA_LIGHT_GREY, VGA_BLACK);

// Effacer l'écran VGA
void vga_clear(void) {
    for (uint32_t i = 0; i < VGA_WIDTH * VGA_HEIGHT; i++) {
        VGA_BUFFER[i] = (vga_color << 8) | ' ';
    }
    vga_row = 0;
    vga_col = 0;
}

// Définir la couleur VGA
void vga_set_color(uint8_t fg, uint8_t bg) {
    vga_color = VGA_COLOR(fg, bg);
}

// Afficher un caractère à la position actuelle
void vga_putchar(char c) {
    if (c == '\n') {
        vga_col = 0;
        vga_row++;
    } else {
        const uint32_t index = vga_row * VGA_WIDTH + vga_col;
        VGA_BUFFER[index] = (vga_color << 8) | c;
        vga_col++;
    }
    
    // Retour à la ligne automatique
    if (vga_col >= VGA_WIDTH) {
        vga_col = 0;
        vga_row++;
    }
    
    // Scroll si nécessaire (simpliste: retour au début)
    if (vga_row >= VGA_HEIGHT) {
        vga_row = 0;
    }
}

// Afficher une chaîne de caractères
void vga_print(const char* str) {
    while (*str) {
        vga_putchar(*str);
        str++;
    }
}

// Afficher un nombre en hexadécimal
void vga_print_hex(uint32_t value) {
    const char hex_chars[] = "0123456789ABCDEF";
    vga_print("0x");
    
    for (int i = 7; i >= 0; i--) {
        uint8_t nibble = (value >> (i * 4)) & 0xF;
        vga_putchar(hex_chars[nibble]);
    }
}

// Afficher un nombre en hexadécimal 64-bit
void vga_print_hex64(uint64_t value) {
    const char hex_chars[] = "0123456789ABCDEF";
    vga_print("0x");
    
    for (int i = 15; i >= 0; i--) {
        uint8_t nibble = (value >> (i * 4)) & 0xF;
        vga_putchar(hex_chars[nibble]);
    }
}

// Structure Multiboot info (version simplifiée)
struct multiboot_info {
    uint32_t flags;
    uint32_t mem_lower;
    uint32_t mem_upper;
    uint32_t boot_device;
    uint32_t cmdline;
    uint32_t mods_count;
    uint32_t mods_addr;
    // ... d'autres champs existent mais on n'en a pas besoin pour l'instant
};

// Déclaration du point d'entrée Rust
extern void rust_main(uint32_t magic, uint64_t multiboot_info_addr) __attribute__((noreturn));

// Point d'entrée C du kernel (appelé depuis boot.asm en mode 64-bit)
// Initialise l'environnement de base puis passe le contrôle au kernel Rust
__attribute__((noreturn))
void kernel_main(uint32_t magic, uint64_t multiboot_info_addr) {
    // Effacer l'écran
    vga_clear();
    
    // Titre avec couleurs
    vga_set_color(VGA_YELLOW, VGA_BLACK);
    vga_print("========================================\n");
    vga_print("         EXO-OS KERNEL v0.1.0          \n");
    vga_print("========================================\n\n");
    
    // Informations de boot
    vga_set_color(VGA_LIGHT_GREY, VGA_BLACK);
    vga_print("Boot Mode: 64-bit Long Mode\n");
    vga_print("Bootloader: GRUB (Multiboot1)\n\n");
    
    // Vérifier magic Multiboot
    vga_print("Multiboot Magic: ");
    vga_print_hex(magic);
    
    if (magic == 0x2BADB002) {
        vga_set_color(VGA_LIGHT_GREEN, VGA_BLACK);
        vga_print(" [OK]\n");
    } else {
        vga_set_color(VGA_LIGHT_RED, VGA_BLACK);
        vga_print(" [FAIL]\n");
        vga_set_color(VGA_LIGHT_GREY, VGA_BLACK);
        goto halt;
    }
    
    vga_set_color(VGA_LIGHT_GREY, VGA_BLACK);
    vga_print("Multiboot Info: ");
    vga_print_hex64(multiboot_info_addr);
    vga_print("\n\n");
    
    // Message de succès
    vga_set_color(VGA_LIGHT_GREEN, VGA_BLACK);
    vga_print("[SUCCESS] ");
    vga_set_color(VGA_WHITE, VGA_BLACK);
    vga_print("Kernel initialized successfully!\n\n");
    
    vga_set_color(VGA_LIGHT_CYAN, VGA_BLACK);
    vga_print("System ready. Entering idle loop...\n");
    
    vga_set_color(VGA_DARK_GREY, VGA_BLACK);
    vga_print("Press Ctrl+Alt+2 for QEMU monitor, type 'quit' to exit\n");
    
halt:
    vga_set_color(VGA_LIGHT_CYAN, VGA_BLACK);
    vga_print("\n>>> Passing control to Rust kernel...\n");
    
    // Passer le contrôle au kernel Rust
    // Cette fonction ne retourne jamais
    rust_main(magic, multiboot_info_addr);
    
    // Ne devrait JAMAIS arriver ici
    __builtin_unreachable();
}
