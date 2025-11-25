// PCI Driver
// Basic PCI device enumeration and configuration

#include <stdint.h>

#define PCI_CONFIG_ADDRESS 0xCF8
#define PCI_CONFIG_DATA    0xCFC

static inline void outl(uint16_t port, uint32_t value) {
    __asm__ volatile("outl %0, %1" : : "a"(value), "Nd"(port));
}

static inline uint32_t inl(uint16_t port) {
    uint32_t value;
    __asm__ volatile("inl %1, %0" : "=a"(value) : "Nd"(port));
    return value;
}

// Read PCI configuration register
uint32_t pci_read_config(uint8_t bus, uint8_t device, uint8_t function, uint8_t offset) {
    uint32_t address = (uint32_t)(
        ((uint32_t)bus << 16) |
        ((uint32_t)device << 11) |
        ((uint32_t)function << 8) |
        (offset & 0xFC) |
        0x80000000
    );
    
    outl(PCI_CONFIG_ADDRESS, address);
    return inl(PCI_CONFIG_DATA);
}

// Write PCI configuration register
void pci_write_config(uint8_t bus, uint8_t device, uint8_t function, uint8_t offset, uint32_t value) {
    uint32_t address = (uint32_t)(
        ((uint32_t)bus << 16) |
        ((uint32_t)device << 11) |
        ((uint32_t)function << 8) |
        (offset & 0xFC) |
        0x80000000
    );
    
    outl(PCI_CONFIG_ADDRESS, address);
    outl(PCI_CONFIG_DATA, value);
}

// Get vendor ID
uint16_t pci_get_vendor(uint8_t bus, uint8_t device, uint8_t function) {
    return (uint16_t)(pci_read_config(bus, device, function, 0) & 0xFFFF);
}

// Get device ID
uint16_t pci_get_device(uint8_t bus, uint8_t device, uint8_t function) {
    return (uint16_t)((pci_read_config(bus, device, function, 0) >> 16) & 0xFFFF);
}

// Enumerate PCI devices
void pci_enumerate(void) {
    for (uint16_t bus = 0; bus < 256; bus++) {
        for (uint8_t device = 0; device < 32; device++) {
            uint16_t vendor = pci_get_vendor(bus, device, 0);
            if (vendor != 0xFFFF) {
                uint16_t device_id = pci_get_device(bus, device, 0);
                // Device found: vendor:device_id
                // TODO: Call Rust callback
            }
        }
    }
}
