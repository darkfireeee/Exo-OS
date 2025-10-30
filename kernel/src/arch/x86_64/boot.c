// kernel/src/arch/x86_64/boot.c
// Amorçage x86_64 avec interface Multiboot2

#include <stdint.h>

// Constantes Multiboot2
#define MULTIBOOT2_MAGIC 0x36d76289

// Structures Multiboot2
struct multiboot_tag {
    uint32_t type;
    uint32_t size;
};

// Pile pour le kernel (16 KiB)
static uint8_t boot_stack[16 * 1024];

// Fonction pour obtenir le pointeur de la pile
void* get_boot_stack_top() {
    return (void*)(boot_stack + sizeof(boot_stack));
}

// Déclaration externe de la fonction Rust
extern void rust_kernel_main(uintptr_t mb_info, uint32_t mb_magic);

// Fonction d'initialisation du port série
extern void serial_init(void);

// Point d'entrée principal du kernel
void boot_entry(void* mb_info, uint32_t mb_magic) {
    // Vérifier le magic Multiboot2
    if (mb_magic != MULTIBOOT2_MAGIC) {
        // Erreur: pas un bootloader Multiboot2
        while (1) {
            __asm__ volatile ("hlt");
        }
    }

    // Initialiser le port série pour le debug
    serial_init();

    // Appeler le kernel Rust
    rust_kernel_main((uintptr_t)mb_info, mb_magic);
}

// Fonction de panic en C (fallback)
void c_panic(const char* msg) {
    // Fonction externe pour écrire un caractère série
    extern void serial_write_char(unsigned char c);
    
    // Utiliser la fonction du module serial.c
    if (msg) {
        const char* p = msg;
        while (*p) {
            serial_write_char(*p);
            p++;
        }
        serial_write_char('\n');
    }
    
    // Boucle infinie
    while (1) {
        __asm__ volatile ("hlt");
    }
}

// Fonction pour lire un octet du port
uint8_t inb(uint16_t port) {
    uint8_t result;
    __asm__ volatile ("inb %1, %0" : "=a"(result) : "Nd"(port));
    return result;
}

// Fonction pour écrire un octet sur le port
void outb(uint16_t port, uint8_t value) {
    __asm__ volatile ("outb %b0, %w1" : : "a"(value), "Nd"(port));
}

// Fonction pour lire un mot (16 bits) du port
uint16_t inw(uint16_t port) {
    uint16_t result;
    __asm__ volatile ("inw %1, %0" : "=a"(result) : "Nd"(port));
    return result;
}

// Fonction pour écrire un mot (16 bits) sur le port
void outw(uint16_t port, uint16_t value) {
    __asm__ volatile ("outw %b0, %w1" : : "a"(value), "Nd"(port));
}