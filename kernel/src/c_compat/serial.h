/**
 * serial.h - Header pour le pilote port série
 */

#ifndef SERIAL_H
#define SERIAL_H

#include <stdbool.h>

/**
 * Initialise le port série COM1 (0x3F8)
 * - 38400 bauds
 * - 8N1 (8 bits, no parity, 1 stop bit)
 * - FIFO activé
 */
void serial_init(void);

/**
 * Écrit un caractère sur le port série
 */
void serial_write_char(char c);

/**
 * Écrit une chaîne sur le port série
 */
void serial_write_string(const char* str);

/**
 * Lit un caractère du port série (bloquant)
 */
char serial_read_char(void);

/**
 * Vérifie si des données sont disponibles
 */
bool serial_available(void);

#endif // SERIAL_H
