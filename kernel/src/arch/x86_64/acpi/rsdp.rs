//! RSDP (Root System Description Pointer) Parsing
//!
//! The RSDP is the entry point to ACPI tables. It contains a pointer
//! to the RSDT (32-bit) or XSDT (64-bit).
//!
//! RSDP is located in one of these memory regions:
//! - First 1KB of EBDA (Extended BIOS Data Area)
//! - 0xE0000 to 0xFFFFF (BIOS ROM area)

use core::ptr;

/// RSDP structure (ACPI 1.0)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Rsdp {
    pub signature: [u8; 8],  // "RSD PTR "
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub revision: u8,
    pub rsdt_address: u32,   // 32-bit physical address of RSDT
}

/// RSDP structure (ACPI 2.0+)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct RsdpExtended {
    pub rsdp: Rsdp,
    pub length: u32,
    pub xsdt_address: u64,   // 64-bit physical address of XSDT
    pub extended_checksum: u8,
    pub reserved: [u8; 3],
}

impl Rsdp {
    /// Validate RSDP checksum (first 20 bytes)
    pub fn validate_checksum(&self) -> bool {
        let ptr = self as *const Self as *const u8;
        let mut sum: u8 = 0;
        
        unsafe {
            for i in 0..20 {
                sum = sum.wrapping_add(*ptr.add(i));
            }
        }
        
        sum == 0
    }
    
    /// Check if signature is valid
    pub fn is_valid(&self) -> bool {
        &self.signature == b"RSD PTR " && self.validate_checksum()
    }
}

impl RsdpExtended {
    /// Validate extended checksum (entire structure)
    pub fn validate_extended_checksum(&self) -> bool {
        let ptr = self as *const Self as *const u8;
        let mut sum: u8 = 0;
        
        unsafe {
            for i in 0..self.length as usize {
                sum = sum.wrapping_add(*ptr.add(i));
            }
        }
        
        sum == 0
    }
    
    /// Check if ACPI 2.0+ (has XSDT)
    pub fn is_extended(&self) -> bool {
        self.rsdp.revision >= 2 && self.validate_extended_checksum()
    }
}

/// Find RSDP in BIOS memory regions
pub fn find_rsdp() -> Option<&'static Rsdp> {
    // Search in EBDA (Extended BIOS Data Area)
    // EBDA pointer is at 0x40E (segment address)
    let ebda_ptr = unsafe {
        let ebda_seg = ptr::read_volatile(0x40E as *const u16);
        (ebda_seg as u64) << 4  // Convert segment to physical address
    };
    
    if ebda_ptr != 0 && ebda_ptr < 0x100000 {
        if let Some(rsdp) = search_rsdp_in_range(ebda_ptr, ebda_ptr + 1024) {
            return Some(rsdp);
        }
    }
    
    // Search in BIOS ROM area (0xE0000 - 0xFFFFF)
    search_rsdp_in_range(0xE0000, 0x100000)
}

/// Search for RSDP in a memory range
fn search_rsdp_in_range(start: u64, end: u64) -> Option<&'static Rsdp> {
    // RSDP is aligned on 16-byte boundary
    let mut addr = start & !0xF;
    
    while addr < end {
        unsafe {
            let rsdp = &*(addr as *const Rsdp);
            
            if rsdp.is_valid() {
                return Some(rsdp);
            }
        }
        
        addr += 16;  // Next 16-byte boundary
    }
    
    None
}

/// Get RSDT (Root System Description Table) address
pub fn get_rsdt_address(rsdp: &Rsdp) -> u32 {
    rsdp.rsdt_address
}

/// Get XSDT (Extended System Description Table) address if available
pub fn get_xsdt_address(rsdp: &Rsdp) -> Option<u64> {
    if rsdp.revision >= 2 {
        let extended = unsafe { &*(rsdp as *const Rsdp as *const RsdpExtended) };
        if extended.is_extended() {
            return Some(extended.xsdt_address);
        }
    }
    None
}
