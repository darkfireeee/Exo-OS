//! ACPI (Advanced Configuration and Power Interface) Support
//!
//! This module provides ACPI table parsing for detecting hardware configuration,
//! particularly for SMP (Symmetric Multi-Processing) initialization.

pub mod madt;

use core::slice;

/// RSDP (Root System Description Pointer) signature
const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";

/// RSDP structure (ACPI 1.0)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Rsdp {
    pub signature: [u8; 8],
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub revision: u8,
    pub rsdt_address: u32,
}

/// RSDP 2.0 extended structure (ACPI 2.0+)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct RsdpExtended {
    pub rsdp: Rsdp,
    pub length: u32,
    pub xsdt_address: u64,
    pub extended_checksum: u8,
    pub reserved: [u8; 3],
}

/// ACPI SDT (System Description Table) Header
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
    /// Verify SDT checksum
    pub fn verify_checksum(&self) -> bool {
        let ptr = self as *const Self as *const u8;
        let bytes = unsafe { slice::from_raw_parts(ptr, self.length as usize) };
        
        let sum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
        sum == 0
    }
    
    /// Get signature as string slice
    pub fn signature_str(&self) -> &str {
        core::str::from_utf8(&self.signature).unwrap_or("????")
    }
}

/// RSDT (Root System Description Table)
#[repr(C, packed)]
pub struct Rsdt {
    pub header: SdtHeader,
    // Followed by array of u32 pointers to other SDTs
}

impl Rsdt {
    /// Get array of SDT pointers
    pub fn sdt_pointers(&self) -> &[u32] {
        let count = (self.header.length as usize - core::mem::size_of::<SdtHeader>()) / 4;
        let ptr = unsafe { (self as *const Self).add(1) as *const u32 };
        unsafe { slice::from_raw_parts(ptr, count) }
    }
}

/// XSDT (Extended System Description Table) for 64-bit addresses
#[repr(C, packed)]
pub struct Xsdt {
    pub header: SdtHeader,
    // Followed by array of u64 pointers to other SDTs
}

impl Xsdt {
    /// Get array of SDT pointers
    pub fn sdt_pointers(&self) -> &[u64] {
        let count = (self.header.length as usize - core::mem::size_of::<SdtHeader>()) / 8;
        let ptr = unsafe { (self as *const Self).add(1) as *const u64 };
        unsafe { slice::from_raw_parts(ptr, count) }
    }
}

/// Search for RSDP in a memory region
fn search_rsdp(start: usize, end: usize) -> Option<usize> {
    // RSDP is 16-byte aligned
    let mut addr = start;
    while addr < end {
        let ptr = addr as *const [u8; 8];
        let signature = unsafe { &*ptr };
        
        if signature == RSDP_SIGNATURE {
            // Verify checksum
            let rsdp = unsafe { &*(addr as *const Rsdp) };
            let bytes = unsafe { slice::from_raw_parts(addr as *const u8, 20) };
            let sum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
            
            if sum == 0 {
                return Some(addr);
            }
        }
        
        addr += 16;
    }
    
    None
}

/// Find RSDP in EBDA or BIOS area
pub fn find_rsdp() -> Result<usize, &'static str> {
    // 1. Search EBDA (Extended BIOS Data Area)
    // EBDA pointer is at 0x40E (segment:offset format)
    let ebda_ptr = unsafe { *(0x40E as *const u16) } as usize;
    let ebda_start = ebda_ptr << 4; // Convert segment to physical address
    
    if ebda_start != 0 && ebda_start < 0xA0000 {
        if let Some(rsdp_addr) = search_rsdp(ebda_start, ebda_start + 1024) {
            log::info!("RSDP found in EBDA at {:#x}", rsdp_addr);
            return Ok(rsdp_addr);
        }
    }
    
    // 2. Search BIOS area (0xE0000 - 0xFFFFF)
    if let Some(rsdp_addr) = search_rsdp(0xE0000, 0x100000) {
        log::info!("RSDP found in BIOS area at {:#x}", rsdp_addr);
        return Ok(rsdp_addr);
    }
    
    Err("RSDP not found")
}

/// Find a specific ACPI table by signature
pub fn find_table(signature: &[u8; 4]) -> Result<usize, &'static str> {
    let rsdp_addr = find_rsdp()?;
    let rsdp = unsafe { &*(rsdp_addr as *const Rsdp) };
    
    // Check ACPI version
    if rsdp.revision >= 2 {
        // ACPI 2.0+: Use XSDT (64-bit pointers)
        let rsdp_ext = unsafe { &*(rsdp_addr as *const RsdpExtended) };
        let xsdt = unsafe { &*(rsdp_ext.xsdt_address as *const Xsdt) };
        
        if !xsdt.header.verify_checksum() {
            return Err("XSDT checksum verification failed");
        }
        
        for &sdt_addr in xsdt.sdt_pointers() {
            let header = unsafe { &*(sdt_addr as *const SdtHeader) };
            if &header.signature == signature {
                if !header.verify_checksum() {
                    return Err("Table checksum verification failed");
                }
                return Ok(sdt_addr as usize);
            }
        }
    } else {
        // ACPI 1.0: Use RSDT (32-bit pointers)
        let rsdt = unsafe { &*(rsdp.rsdt_address as *const Rsdt) };
        
        if !rsdt.header.verify_checksum() {
            return Err("RSDT checksum verification failed");
        }
        
        for &sdt_addr in rsdt.sdt_pointers() {
            let header = unsafe { &*(sdt_addr as *const SdtHeader) };
            if &header.signature == signature {
                if !header.verify_checksum() {
                    return Err("Table checksum verification failed");
                }
                return Ok(sdt_addr as usize);
            }
        }
    }
    
    Err("Table not found")
}

/// Initialize ACPI subsystem
pub fn init() -> Result<(), &'static str> {
    let rsdp_addr = find_rsdp()?;
    let rsdp = unsafe { &*(rsdp_addr as *const Rsdp) };
    
    log::info!("ACPI initialized, revision {}", rsdp.revision);
    
    Ok(())
}
