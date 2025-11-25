// ACPI (Advanced Configuration and Power Interface) Driver
// Basic ACPI table parsing

#include <stdint.h>

// ACPI table signatures
#define RSDP_SIGNATURE "RSD PTR "
#define RSDT_SIGNATURE "RSDT"
#define XSDT_SIGNATURE "XSDT"
#define FADT_SIGNATURE "FACP"

// RSDP structure
typedef struct {
    char signature[8];
    uint8_t checksum;
    char oem_id[6];
    uint8_t revision;
    uint32_t rsdt_address;
} __attribute__((packed)) RSDP;

// RSDP 2.0 extended structure
typedef struct {
    RSDP v1;
    uint32_t length;
    uint64_t xsdt_address;
    uint8_t extended_checksum;
    uint8_t reserved[3];
} __attribute__((packed)) RSDPv2;

// ACPI table header
typedef struct {
    char signature[4];
    uint32_t length;
    uint8_t revision;
    uint8_t checksum;
    char oem_id[6];
    char oem_table_id[8];
    uint32_t oem_revision;
    uint32_t creator_id;
    uint32_t creator_revision;
} __attribute__((packed)) ACPITableHeader;

// RSDT structure
typedef struct {
    ACPITableHeader header;
    uint32_t entries[];
} __attribute__((packed)) RSDT;

// XSDT structure
typedef struct {
    ACPITableHeader header;
    uint64_t entries[];
} __attribute__((packed)) XSDT;

static RSDP* rsdp = 0;
static RSDT* rsdt = 0;
static XSDT* xsdt = 0;

// Verify checksum
static uint8_t acpi_checksum(void* ptr, uint32_t length) {
    uint8_t sum = 0;
    uint8_t* bytes = (uint8_t*)ptr;
    for (uint32_t i = 0; i < length; i++) {
        sum += bytes[i];
    }
    return sum;
}

// Find RSDP in memory range
static RSDP* find_rsdp_range(uintptr_t start, uintptr_t end) {
    for (uintptr_t addr = start; addr < end; addr += 16) {
        RSDP* candidate = (RSDP*)addr;
        if (*(uint64_t*)candidate->signature == *(uint64_t*)RSDP_SIGNATURE) {
            if (acpi_checksum(candidate, sizeof(RSDP)) == 0) {
                return candidate;
            }
        }
    }
    return 0;
}

// Find RSDP in BIOS memory areas
RSDP* acpi_find_rsdp(void) {
    // Search EBDA (Extended BIOS Data Area)
    uint16_t ebda_base = *(uint16_t*)0x40E;
    if (ebda_base) {
        RSDP* found = find_rsdp_range(ebda_base << 4, (ebda_base << 4) + 0x400);
        if (found) return found;
    }
    
    // Search main BIOS area (0xE0000 - 0xFFFFF)
    return find_rsdp_range(0xE0000, 0x100000);
}

// Find ACPI table by signature
void* acpi_find_table(const char* signature) {
    if (!rsdt && !xsdt) return 0;
    
    if (xsdt) {
        // Use XSDT (64-bit addresses)
        uint32_t entries = (xsdt->header.length - sizeof(ACPITableHeader)) / 8;
        for (uint32_t i = 0; i < entries; i++) {
            ACPITableHeader* header = (ACPITableHeader*)(uintptr_t)xsdt->entries[i];
            if (*(uint32_t*)header->signature == *(uint32_t*)signature) {
                if (acpi_checksum(header, header->length) == 0) {
                    return header;
                }
            }
        }
    } else if (rsdt) {
        // Use RSDT (32-bit addresses)
        uint32_t entries = (rsdt->header.length - sizeof(ACPITableHeader)) / 4;
        for (uint32_t i = 0; i < entries; i++) {
            ACPITableHeader* header = (ACPITableHeader*)(uintptr_t)rsdt->entries[i];
            if (*(uint32_t*)header->signature == *(uint32_t*)signature) {
                if (acpi_checksum(header, header->length) == 0) {
                    return header;
                }
            }
        }
    }
    
    return 0;
}

// Initialize ACPI
int acpi_init(void) {
    rsdp = acpi_find_rsdp();
    if (!rsdp) return -1;
    
    // Check ACPI version
    if (rsdp->revision >= 2) {
        RSDPv2* rsdp2 = (RSDPv2*)rsdp;
        if (acpi_checksum(rsdp2, rsdp2->length) == 0) {
            xsdt = (XSDT*)(uintptr_t)rsdp2->xsdt_address;
        }
    }
    
    // Fallback to RSDT
    if (!xsdt && rsdp->rsdt_address) {
        rsdt = (RSDT*)(uintptr_t)rsdp->rsdt_address;
    }
    
    return (rsdt || xsdt) ? 0 : -1;
}
