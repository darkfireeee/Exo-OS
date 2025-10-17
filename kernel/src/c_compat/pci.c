// src/c_compat/pci.c
// Pilote PCI basique en C (pour la compatibilité)

#include <stddef.h>
#include <stdint.h>

// Adresses des ports de configuration PCI
#define PCI_CONFIG_ADDRESS 0xCF8
#define PCI_CONFIG_DATA    0xCFC

// Déclare la fonction C d'écriture sur le port série pour le debug
extern void serial_write_char(unsigned char c);

// Fonction simple pour écrire une chaîne de caractères (pour le debug)
void serial_print(const char* str) {
    for (int i = 0; str[i] != '\0'; i++) {
        serial_write_char(str[i]);
    }
}

// Fonction pour convertir un nombre en chaîne de caractères hexadécimaux (très basique)
void serial_print_hex(uint32_t val) {
    const char* hex_chars = "0123456789ABCDEF";
    serial_print("0x");
    for (int i = 28; i >= 0; i -= 4) {
        serial_write_char(hex_chars[(val >> i) & 0xF]);
    }
}

// Lit un mot (32 bits) depuis l'espace de configuration PCI
uint32_t pci_config_read_word(uint8_t bus, uint8_t device, uint8_t function, uint8_t offset) {
    uint32_t address;
    uint32_t lbus  = (uint32_t)bus;
    uint32_t ldevice = (uint32_t)device;
    uint32_t lfunction = (uint32_t)function;
    uint16_t tmp = 0;

    // Créer l'adresse de configuration
    address = (uint32_t)((lbus << 16) | (ldevice << 11) |
              (lfunction << 8) | (offset & 0xFC) | ((uint32_t)0x80000000));

    uint32_t result;
    // Écrire l'adresse dans le port d'adresse
    __asm__ volatile("outl %%eax, %%dx" : : "a"(address), "d"(PCI_CONFIG_ADDRESS) : "memory");
    // Lire les données depuis le port de données
    __asm__ volatile("inl %%dx, %%eax" : "=a"(result) : "d"(PCI_CONFIG_DATA) : "memory");
    return result;
}

// Initialise le sous-système PCI (peut rester vide pour cet exemple)
void pci_init() {
    // Pas d'initialisation spécifique nécessaire pour cette simple énumération.
}

// Parcourt tous les bus, devices et fonctions pour trouver les périphériques
void pci_enumerate_buses() {
    for (uint16_t bus = 0; bus < 256; bus++) {
        for (uint8_t device = 0; device < 32; device++) {
            uint16_t vendor_id = (uint16_t)(pci_config_read_word(bus, device, 0, 0) >> 16);
            if (vendor_id == 0xFFFF) continue; // L'appareil n'existe pas

            uint8_t function = 0;
            uint16_t device_id = (uint16_t)(pci_config_read_word(bus, device, function, 0) >> 0);
            
            serial_print("PCI trouvé: Bus=");
            serial_print_hex(bus);
            serial_print(", Device=");
            serial_print_hex(device);
            serial_print(", Vendor=");
            serial_print_hex(vendor_id);
            serial_print(", DeviceID=");
            serial_print_hex(device_id);
            serial_write_char('\n');
        }
    }
}