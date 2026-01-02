//! ACPI (Advanced Configuration and Power Interface) Support
//!
//! Basic ACPI table parsing for hardware discovery:
//! - RSDP (Root System Description Pointer) location
//! - RSDT/XSDT (Root/Extended System Description Table)
//! - MADT (Multiple APIC Description Table) - for SMP
//! - FADT (Fixed ACPI Description Table) - for power management
//!
//! ## ACPI Tables Hierarchy
//!
//! ```text
//! RSDP (BIOS data area or EFI)
//!   └── RSDT/XSDT
//!       ├── MADT (APIC info)
//!       ├── FADT (PM info)
//!       ├── HPET
//!       ├── MCFG (PCIe config)
//!       └── ...
//! ```

use alloc::vec::Vec;
use alloc::string::String;
use core::slice;
use crate::memory::{PhysAddr, VirtAddr};

/// RSDP (Root System Description Pointer) signature
const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";

/// RSDP structure (ACPI 1.0)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct Rsdp {
    signature: [u8; 8],
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_address: u32,
}

/// RSDP 2.0 Extended structure (ACPI 2.0+)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct Rsdp2 {
    rsdp: Rsdp,
    length: u32,
    xsdt_address: u64,
    extended_checksum: u8,
    reserved: [u8; 3],
}

/// SDT (System Description Table) Header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: u32,
    pub creator_revision: u32,
}

impl SdtHeader {
    /// Get signature as string
    pub fn signature_str(&self) -> String {
        String::from_utf8_lossy(&self.signature).to_string()
    }
    
    /// Validate checksum
    pub fn validate_checksum(&self) -> bool {
        unsafe {
            let bytes = slice::from_raw_parts(
                self as *const _ as *const u8,
                self.length as usize,
            );
            
            let sum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
            sum == 0
        }
    }
    
    /// Get table data (excluding header)
    pub fn data(&self) -> &[u8] {
        unsafe {
            let data_ptr = (self as *const _ as *const u8)
                .add(core::mem::size_of::<SdtHeader>());
            
            let data_len = self.length as usize - core::mem::size_of::<SdtHeader>();
            
            slice::from_raw_parts(data_ptr, data_len)
        }
    }
}

/// MADT (Multiple APIC Description Table) Entry types
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MadtEntryType {
    LocalApic = 0,
    IoApic = 1,
    InterruptOverride = 2,
    NmiSource = 3,
    LocalApicNmi = 4,
    LocalApicAddressOverride = 5,
    IoSapic = 6,
    LocalSapic = 7,
    PlatformInterruptSources = 8,
    ProcessorLocalX2Apic = 9,
    LocalX2ApicNmi = 10,
}

/// MADT Entry header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MadtEntryHeader {
    pub entry_type: u8,
    pub length: u8,
}

/// Local APIC Entry
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct LocalApicEntry {
    pub header: MadtEntryHeader,
    pub acpi_processor_id: u8,
    pub apic_id: u8,
    pub flags: u32,
}

/// I/O APIC Entry
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IoApicEntry {
    pub header: MadtEntryHeader,
    pub io_apic_id: u8,
    pub reserved: u8,
    pub io_apic_address: u32,
    pub global_system_interrupt_base: u32,
}

/// Interrupt Source Override Entry
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct InterruptOverrideEntry {
    pub header: MadtEntryHeader,
    pub bus: u8,
    pub source: u8,
    pub global_system_interrupt: u32,
    pub flags: u16,
}

/// Parsed ACPI information
pub struct AcpiInfo {
    /// ACPI revision
    pub revision: u8,
    
    /// RSDT/XSDT physical address
    pub root_sdt: PhysAddr,
    
    /// All discovered SDTs
    pub tables: Vec<(String, PhysAddr)>,
    
    /// Local APICs (CPU cores)
    pub local_apics: Vec<LocalApicEntry>,
    
    /// I/O APICs
    pub io_apics: Vec<IoApicEntry>,
    
    /// Interrupt overrides
    pub interrupt_overrides: Vec<InterruptOverrideEntry>,
}

impl AcpiInfo {
    /// Find table by signature
    pub fn find_table(&self, signature: &str) -> Option<PhysAddr> {
        self.tables
            .iter()
            .find(|(sig, _)| sig == signature)
            .map(|(_, addr)| *addr)
    }
}

/// Find RSDP in memory
fn find_rsdp() -> Option<PhysAddr> {
    // Search in EBDA (Extended BIOS Data Area)
    // EBDA pointer at 0x040E (segment address)
    let ebda_seg = unsafe {
        let ptr = 0x040E as *const u16;
        *ptr
    };
    
    if ebda_seg != 0 {
        let ebda_start = (ebda_seg as usize) << 4;
        if let Some(rsdp) = search_rsdp(ebda_start, 1024) {
            return Some(rsdp);
        }
    }
    
    // Search in main BIOS area (0xE0000 - 0xFFFFF)
    search_rsdp(0xE0000, 0x20000)
}

/// Search for RSDP in memory range
fn search_rsdp(start: usize, size: usize) -> Option<PhysAddr> {
    for addr in (start..start + size).step_by(16) {
        unsafe {
            let ptr = addr as *const Rsdp;
            let rsdp = &*ptr;
            
            if &rsdp.signature == RSDP_SIGNATURE {
                // Validate checksum
                let bytes = slice::from_raw_parts(ptr as *const u8, 20);
                let sum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
                
                if sum == 0 {
                    return Some(PhysAddr::new(addr as u64));
                }
            }
        }
    }
    
    None
}

/// Parse RSDT/XSDT entries
fn parse_root_sdt(sdt_phys: PhysAddr, use_xsdt: bool) -> Vec<PhysAddr> {
    let mut tables = Vec::new();
    
    // Identity map (assume identity mapping for now)
    let sdt_virt = sdt_phys.as_u64() as *const SdtHeader;
    
    unsafe {
        let header = &*sdt_virt;
        
        if !header.validate_checksum() {
            crate::logger::warn("[ACPI] Invalid SDT checksum");
            return tables;
        }
        
        let data = header.data();
        
        if use_xsdt {
            // XSDT uses 64-bit addresses
            let entry_count = data.len() / 8;
            let entries = slice::from_raw_parts(data.as_ptr() as *const u64, entry_count);
            
            for &entry in entries {
                tables.push(PhysAddr::new(entry));
            }
        } else {
            // RSDT uses 32-bit addresses
            let entry_count = data.len() / 4;
            let entries = slice::from_raw_parts(data.as_ptr() as *const u32, entry_count);
            
            for &entry in entries {
                tables.push(PhysAddr::new(entry as u64));
            }
        }
    }
    
    tables
}

/// Parse MADT (Multiple APIC Description Table)
fn parse_madt(madt_phys: PhysAddr) -> (Vec<LocalApicEntry>, Vec<IoApicEntry>, Vec<InterruptOverrideEntry>) {
    let mut local_apics = Vec::new();
    let mut io_apics = Vec::new();
    let mut overrides = Vec::new();
    
    let madt_virt = madt_phys.as_u64() as *const u8;
    
    unsafe {
        let header = &*(madt_virt as *const SdtHeader);
        
        if !header.validate_checksum() {
            crate::logger::warn("[ACPI] Invalid MADT checksum");
            return (local_apics, io_apics, overrides);
        }
        
        // Skip header (36 bytes) and LAPIC address/flags (8 bytes)
        let mut offset = core::mem::size_of::<SdtHeader>() + 8;
        let end = header.length as usize;
        
        while offset < end {
            let entry_ptr = madt_virt.add(offset);
            let entry_header = &*(entry_ptr as *const MadtEntryHeader);
            
            match entry_header.entry_type {
                0 => {
                    // Local APIC
                    let entry = &*(entry_ptr as *const LocalApicEntry);
                    if (entry.flags & 0x1) != 0 {
                        // Enabled
                        local_apics.push(*entry);
                    }
                }
                1 => {
                    // I/O APIC
                    let entry = &*(entry_ptr as *const IoApicEntry);
                    io_apics.push(*entry);
                }
                2 => {
                    // Interrupt Override
                    let entry = &*(entry_ptr as *const InterruptOverrideEntry);
                    overrides.push(*entry);
                }
                _ => {}
            }
            
            offset += entry_header.length as usize;
        }
    }
    
    (local_apics, io_apics, overrides)
}

/// Initialize ACPI and parse tables
pub fn init() -> Result<AcpiInfo, &'static str> {
    crate::logger::info("[ACPI] Initializing ACPI subsystem");
    
    // Find RSDP
    let rsdp_phys = find_rsdp().ok_or("RSDP not found")?;
    
    crate::logger::info(&alloc::format!(
        "[ACPI] RSDP found at {:#x}",
        rsdp_phys.as_u64()
    ));
    
    let rsdp_virt = rsdp_phys.as_u64() as *const Rsdp;
    
    let (revision, root_sdt, use_xsdt) = unsafe {
        let rsdp = &*rsdp_virt;
        
        if rsdp.revision >= 2 {
            // ACPI 2.0+: use XSDT
            let rsdp2 = &*(rsdp_virt as *const Rsdp2);
            (rsdp.revision, PhysAddr::new(rsdp2.xsdt_address), true)
        } else {
            // ACPI 1.0: use RSDT
            (rsdp.revision, PhysAddr::new(rsdp.rsdt_address as u64), false)
        }
    };
    
    crate::logger::info(&alloc::format!(
        "[ACPI] ACPI revision {}, {} at {:#x}",
        revision,
        if use_xsdt { "XSDT" } else { "RSDT" },
        root_sdt.as_u64()
    ));
    
    // Parse root SDT
    let table_addresses = parse_root_sdt(root_sdt, use_xsdt);
    
    crate::logger::info(&alloc::format!(
        "[ACPI] Found {} ACPI tables",
        table_addresses.len()
    ));
    
    // Enumerate tables
    let mut tables = Vec::new();
    let mut local_apics = Vec::new();
    let mut io_apics = Vec::new();
    let mut interrupt_overrides = Vec::new();
    
    for &table_phys in &table_addresses {
        let table_virt = table_phys.as_u64() as *const SdtHeader;
        
        unsafe {
            let header = &*table_virt;
            let signature = header.signature_str();
            
            crate::logger::info(&alloc::format!(
                "[ACPI]   {}: {} bytes",
                signature,
                header.length
            ));
            
            tables.push((signature.clone(), table_phys));
            
            // Parse MADT
            if signature == "APIC" {
                let (lapics, ioapics, ovr) = parse_madt(table_phys);
                local_apics = lapics;
                io_apics = ioapics;
                interrupt_overrides = ovr;
                
                crate::logger::info(&alloc::format!(
                    "[ACPI]   MADT: {} CPUs, {} I/O APICs, {} overrides",
                    local_apics.len(),
                    io_apics.len(),
                    interrupt_overrides.len()
                ));
            }
        }
    }
    
    Ok(AcpiInfo {
        revision,
        root_sdt,
        tables,
        local_apics,
        io_apics,
        interrupt_overrides,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_sdt_header_size() {
        assert_eq!(core::mem::size_of::<SdtHeader>(), 36);
    }
    
    #[test]
    fn test_rsdp_signature() {
        assert_eq!(RSDP_SIGNATURE, b"RSD PTR ");
    }
}
