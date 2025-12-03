//! ACPI Table Structures
//!
//! Raw structures for ACPI tables

/// RSDP (Root System Description Pointer) - ACPI 2.0+
#[repr(C, packed)]
pub struct RsdpDescriptor {
    /// "RSD PTR " signature
    pub signature: [u8; 8],
    /// Checksum for first 20 bytes
    pub checksum: u8,
    /// OEM ID string
    pub oem_id: [u8; 6],
    /// ACPI revision (0 = 1.0, 2 = 2.0+)
    pub revision: u8,
    /// Physical address of RSDT (32-bit)
    pub rsdt_address: u32,
    
    // Extended fields (ACPI 2.0+)
    /// Length of table including header
    pub length: u32,
    /// Physical address of XSDT (64-bit)
    pub xsdt_address: u64,
    /// Extended checksum
    pub extended_checksum: u8,
    /// Reserved
    pub reserved: [u8; 3],
}

/// Common ACPI SDT (System Description Table) Header
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct AcpiSdtHeader {
    /// 4-byte signature (e.g., "SRAT", "APIC", "FACP")
    pub signature: [u8; 4],
    /// Length of entire table including header
    pub length: u32,
    /// ACPI specification revision
    pub revision: u8,
    /// Checksum (entire table must sum to 0)
    pub checksum: u8,
    /// OEM ID
    pub oem_id: [u8; 6],
    /// OEM table ID
    pub oem_table_id: [u8; 8],
    /// OEM revision
    pub oem_revision: u32,
    /// Creator ID
    pub creator_id: u32,
    /// Creator revision
    pub creator_revision: u32,
}

impl AcpiSdtHeader {
    /// Get signature as string
    pub fn signature_str(&self) -> &str {
        core::str::from_utf8(&self.signature).unwrap_or("????")
    }
}
