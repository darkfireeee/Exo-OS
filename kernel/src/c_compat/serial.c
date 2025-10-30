// src/c_compat/serial.c
// Pilote série simple en C (pour le debug)

// Configuration pour le code kernel freestanding
#ifdef __clang__
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wunused-function"
#pragma clang diagnostic ignored "-Wattributes"
#endif
#ifdef __GNUC__
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-function"
#pragma GCC diagnostic ignored "-Wattributes"
#endif

#include <stddef.h>

#ifdef __clang__
#pragma clang diagnostic pop
#endif
#ifdef __GNUC__
#pragma GCC diagnostic pop
#endif

// Adresse de base du port série COM1
#define SERIAL_COM1_BASE 0x3F8

// Offsets des registres du port série
#define SERIAL_DATA_REG(base) (base)
#define SERIAL_FIFO_CMD_REG(base) (base + 2)
#define SERIAL_LINE_CMD_REG(base) (base + 3)
#define SERIAL_MODEM_CMD_REG(base) (base + 4)
#define SERIAL_LINE_STATUS_REG(base) (base + 5)

// Fonctions d'entrée/sortie sur les ports (inline assembly)
static inline void outb(unsigned short port, unsigned char val) {
    __asm__ volatile("outb %b0, %w1" : : "a"(val), "Nd"(port) : "memory");
}

static inline unsigned char inb(unsigned short port) {
    unsigned char ret;
    __asm__ volatile("inb %w1, %b0" : "=a"(ret) : "Nd"(port) : "memory");
    return ret;
}

// Configure le baud rate du port série
void serial_configure_baud_rate(unsigned short com, unsigned short divisor) {
    outb(SERIAL_LINE_CMD_REG(com), 0x80);
    outb(SERIAL_DATA_REG(com), (divisor >> 8) & 0x00FF);
    outb(SERIAL_DATA_REG(com), divisor & 0x00FF);
}

// Configure la ligne du port série : 8 bits, pas de parité, 1 bit de stop
void serial_configure_line(unsigned short com) {
    outb(SERIAL_LINE_CMD_REG(com), 0x03);
}

// Initialise le port série
void serial_init(void) {
    // Désactiver les interruptions
    outb(SERIAL_MODEM_CMD_REG(SERIAL_COM1_BASE), 0x00);
    // Configurer le baud rate à 38400
    serial_configure_baud_rate(SERIAL_COM1_BASE, 3);
    // Configurer la ligne
    serial_configure_line(SERIAL_COM1_BASE);
}

// Écrit un caractère sur le port série
void serial_write_char(unsigned char c) {
    // Attendre que le transmit buffer soit vide
    while ((inb(SERIAL_LINE_STATUS_REG(SERIAL_COM1_BASE)) & 0x20) == 0) {
        // Attendre
    }
    // Écrire le caractère
    outb(SERIAL_DATA_REG(SERIAL_COM1_BASE), c);
}

// Fonction de panic pour le code C
void c_panic(const char* msg) {
    // Écrire le message de panic sur le port série
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
        __asm__ volatile("hlt");
    }
}

#ifdef __clang__
#pragma clang diagnostic pop
#endif
#ifdef __GNUC__
#pragma GCC diagnostic pop
#endif