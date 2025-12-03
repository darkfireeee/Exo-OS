//! ACPI (Advanced Configuration and Power Interface) Parser
//!
//! Native Rust implementation for parsing ACPI tables.
//! Supports RSDP, RSDT, XSDT, SRAT for NUMA topology, MADT for SMP.

pub mod tables;
pub mod srat;
pub mod madt;

pub use tables::{RsdpDescriptor, AcpiSdtHeader};
pub use srat::{parse_srat, NumaMemoryAffinity, NumaProcessorAffinity};
pub use madt::{parse_madt, MadtInfo, CpuInfo, IoApicInfo, get_madt_info};

use spin::Once;

/// ACPI tables location
static ACPI_TABLES: Once<AcpiTables> = Once::new();

/// Parsed ACPI tables
pub struct AcpiTables {
    pub rsdp_addr: u64,
    pub rsdt_addr: Option<u64>,
    pub xsdt_addr: Option<u64>,
    pub srat_addr: Option<u64>,
    pub madt_addr: Option<u64>,
}

/// Initialize ACPI by finding RSDP
pub fn init(rsdp_addr: u64) {
    if rsdp_addr == 0 {
        log::warn!("ACPI: No RSDP address provided, using fallback detection");
        if let Some(addr) = find_rsdp_in_memory() {
            init_from_rsdp(addr);
        } else {
            log::warn!("ACPI: RSDP not found, NUMA disabled");
        }
        return;
    }
    
    init_from_rsdp(rsdp_addr);
}

fn init_from_rsdp(rsdp_addr: u64) {
    log::info!("ACPI: RSDP at {:#x}", rsdp_addr);
    
    let tables = unsafe { parse_rsdp(rsdp_addr) };
    
    if let Some(ref t) = tables {
        if t.srat_addr.is_some() {
            log::info!("ACPI: SRAT table found - NUMA topology available");
        }
        if t.madt_addr.is_some() {
            log::info!("ACPI: MADT table found - APIC info available");
        }
    }
    
    ACPI_TABLES.call_once(|| tables.unwrap_or(AcpiTables {
        rsdp_addr,
        rsdt_addr: None,
        xsdt_addr: None,
        srat_addr: None,
        madt_addr: None,
    }));
}

/// Find RSDP by scanning memory regions
fn find_rsdp_in_memory() -> Option<u64> {
    // RSDP can be in:
    // 1. First 1KB of EBDA (Extended BIOS Data Area)
    // 2. BIOS ROM area: 0xE0000 - 0xFFFFF
    
    unsafe {
        // Search BIOS ROM area
        let mut addr = 0xE0000u64;
        while addr < 0x100000 {
            if check_rsdp_signature(addr) {
                return Some(addr);
            }
            addr += 16; // RSDP is 16-byte aligned
        }
    }
    
    None
}

/// Check if address contains valid RSDP signature
unsafe fn check_rsdp_signature(addr: u64) -> bool {
    let ptr = addr as *const [u8; 8];
    let signature = &*ptr;
    signature == b"RSD PTR "
}

/// Parse RSDP and find other tables
unsafe fn parse_rsdp(rsdp_addr: u64) -> Option<AcpiTables> {
    let rsdp = &*(rsdp_addr as *const RsdpDescriptor);
    
    // Verify signature
    if &rsdp.signature != b"RSD PTR " {
        log::error!("ACPI: Invalid RSDP signature");
        return None;
    }
    
    // Verify checksum
    if !verify_checksum(rsdp_addr, 20) {
        log::error!("ACPI: RSDP checksum failed");
        return None;
    }
    
    let mut tables = AcpiTables {
        rsdp_addr,
        rsdt_addr: None,
        xsdt_addr: None,
        srat_addr: None,
        madt_addr: None,
    };
    
    // Check ACPI version
    if rsdp.revision >= 2 {
        // ACPI 2.0+ - use XSDT (64-bit addresses)
        let xsdt_addr = rsdp.xsdt_address;
        if xsdt_addr != 0 {
            tables.xsdt_addr = Some(xsdt_addr);
            parse_xsdt(&mut tables, xsdt_addr);
        }
    } else {
        // ACPI 1.0 - use RSDT (32-bit addresses)
        let rsdt_addr = rsdp.rsdt_address as u64;
        if rsdt_addr != 0 {
            tables.rsdt_addr = Some(rsdt_addr);
            parse_rsdt(&mut tables, rsdt_addr);
        }
    }
    
    Some(tables)
}

/// Parse XSDT (Extended System Descriptor Table) - 64-bit addresses
unsafe fn parse_xsdt(tables: &mut AcpiTables, xsdt_addr: u64) {
    let header = &*(xsdt_addr as *const AcpiSdtHeader);
    
    if &header.signature != b"XSDT" {
        log::error!("ACPI: Invalid XSDT signature");
        return;
    }
    
    // Number of table pointers
    let entries = (header.length as usize - core::mem::size_of::<AcpiSdtHeader>()) / 8;
    let ptrs = core::slice::from_raw_parts(
        (xsdt_addr + 36) as *const u64,
        entries
    );
    
    for &ptr in ptrs {
        if ptr == 0 {
            continue;
        }
        
        let table_header = &*(ptr as *const AcpiSdtHeader);
        match &table_header.signature {
            b"SRAT" => tables.srat_addr = Some(ptr),
            b"APIC" => tables.madt_addr = Some(ptr),
            _ => {}
        }
    }
}

/// Parse RSDT (Root System Descriptor Table) - 32-bit addresses
unsafe fn parse_rsdt(tables: &mut AcpiTables, rsdt_addr: u64) {
    let header = &*(rsdt_addr as *const AcpiSdtHeader);
    
    if &header.signature != b"RSDT" {
        log::error!("ACPI: Invalid RSDT signature");
        return;
    }
    
    // Number of table pointers
    let entries = (header.length as usize - core::mem::size_of::<AcpiSdtHeader>()) / 4;
    let ptrs = core::slice::from_raw_parts(
        (rsdt_addr + 36) as *const u32,
        entries
    );
    
    for &ptr in ptrs {
        if ptr == 0 {
            continue;
        }
        
        let table_header = &*(ptr as u64 as *const AcpiSdtHeader);
        match &table_header.signature {
            b"SRAT" => tables.srat_addr = Some(ptr as u64),
            b"APIC" => tables.madt_addr = Some(ptr as u64),
            _ => {}
        }
    }
}

/// Verify ACPI table checksum
fn verify_checksum(addr: u64, len: usize) -> bool {
    let bytes = unsafe { core::slice::from_raw_parts(addr as *const u8, len) };
    let sum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
    sum == 0
}

/// Get parsed ACPI tables
pub fn get_tables() -> Option<&'static AcpiTables> {
    ACPI_TABLES.get()
}

/// Get SRAT address if available
pub fn get_srat_addr() -> Option<u64> {
    ACPI_TABLES.get().and_then(|t| t.srat_addr)
}

/// Get MADT address if available
pub fn get_madt_addr() -> Option<u64> {
    ACPI_TABLES.get().and_then(|t| t.madt_addr)
}
