//! MADT (Multiple APIC Description Table) Parsing
//!
//! The MADT contains information about:
//! - Local APICs (one per CPU core)
//! - I/O APICs (for external interrupts)
//! - Interrupt source overrides
//! - Non-maskable interrupts (NMI)

use super::SdtHeader;
use alloc::vec::Vec;

/// MADT signature
pub const MADT_SIGNATURE: &[u8; 4] = b"APIC";

/// MADT table structure
#[repr(C, packed)]
pub struct Madt {
    pub header: SdtHeader,
    pub local_apic_address: u32,
    pub flags: u32,
    // Followed by variable-length entries
}

impl Madt {
    /// Get slice of entry bytes
    pub fn entries_bytes(&self) -> &[u8] {
        let entries_start = unsafe { (self as *const Self).add(1) as *const u8 };
        let entries_len = self.header.length as usize - core::mem::size_of::<Self>();
        unsafe { core::slice::from_raw_parts(entries_start, entries_len) }
    }
}

/// MADT entry header (common to all entry types)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MadtEntryHeader {
    pub entry_type: u8,
    pub length: u8,
}

/// MADT Entry Type 0: Processor Local APIC
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MadtLocalApic {
    pub header: MadtEntryHeader,
    pub acpi_processor_id: u8,
    pub apic_id: u8,
    pub flags: u32, // Bit 0: Processor Enabled, Bit 1: Online Capable
}

impl MadtLocalApic {
    pub fn is_enabled(&self) -> bool {
        (self.flags & 0x1) != 0
    }
    
    pub fn is_online_capable(&self) -> bool {
        (self.flags & 0x2) != 0
    }
}

/// MADT Entry Type 1: I/O APIC
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MadtIoApic {
    pub header: MadtEntryHeader,
    pub ioapic_id: u8,
    pub reserved: u8,
    pub ioapic_address: u32,
    pub global_system_interrupt_base: u32,
}

/// MADT Entry Type 2: Interrupt Source Override
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MadtInterruptOverride {
    pub header: MadtEntryHeader,
    pub bus: u8,
    pub source: u8,
    pub global_system_interrupt: u32,
    pub flags: u16,
}

/// MADT Entry Type 4: NMI (Non-Maskable Interrupt)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MadtNmi {
    pub header: MadtEntryHeader,
    pub acpi_processor_id: u8,
    pub flags: u16,
    pub lint: u8, // Local APIC LINT# (0 or 1)
}

/// MADT Entry Type 9: Processor Local x2APIC
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MadtLocalX2Apic {
    pub header: MadtEntryHeader,
    pub reserved: u16,
    pub x2apic_id: u32,
    pub flags: u32,
    pub acpi_processor_uid: u32,
}

impl MadtLocalX2Apic {
    pub fn is_enabled(&self) -> bool {
        (self.flags & 0x1) != 0
    }
}

/// Parsed MADT information
#[derive(Debug, Clone)]
pub struct MadtInfo {
    pub local_apic_address: u64,
    pub cpu_count: usize,
    pub apic_ids: Vec<u32>,
    pub ioapic_address: u64,
    pub ioapic_id: u8,
    pub ioapic_gsi_base: u32,
}

/// Parse MADT table
pub fn parse_madt() -> Result<MadtInfo, &'static str> {
    // Find MADT table
    let madt_addr = super::find_table(MADT_SIGNATURE)?;
    let madt = unsafe { &*(madt_addr as *const Madt) };
    
    let local_apic_addr = madt.local_apic_address;
    log::info!("MADT found at {:#x}", madt_addr);
    log::info!("Local APIC address: {:#x}", local_apic_addr);
    
    let mut info = MadtInfo {
        local_apic_address: local_apic_addr as u64,
        cpu_count: 0,
        apic_ids: Vec::new(),
        ioapic_address: 0,
        ioapic_id: 0,
        ioapic_gsi_base: 0,
    };
    
    // Parse entries
    let entries = madt.entries_bytes();
    let mut offset = 0;
    
    while offset < entries.len() {
        let header = unsafe { &*(entries.as_ptr().add(offset) as *const MadtEntryHeader) };
        
        match header.entry_type {
            0 => {
                // Processor Local APIC
                let entry = unsafe { &*(entries.as_ptr().add(offset) as *const MadtLocalApic) };
                
                if entry.is_enabled() || entry.is_online_capable() {
                    let apic_id = entry.apic_id;
                    let flags = entry.flags;
                    info.cpu_count += 1;
                    info.apic_ids.push(apic_id as u32);
                    log::debug!("CPU {}: APIC ID {}, flags {:#x}", 
                        info.cpu_count - 1, apic_id, flags);
                }
            }
            1 => {
                // I/O APIC
                let entry = unsafe { &*(entries.as_ptr().add(offset) as *const MadtIoApic) };
                
                if info.ioapic_address == 0 {
                    let ioapic_addr = entry.ioapic_address;
                    let ioapic_id = entry.ioapic_id;
                    let gsi_base = entry.global_system_interrupt_base;
                    info.ioapic_address = ioapic_addr as u64;
                    info.ioapic_id = ioapic_id;
                    info.ioapic_gsi_base = gsi_base;
                    log::info!("I/O APIC: ID {}, address {:#x}, GSI base {}", 
                        ioapic_id, ioapic_addr, gsi_base);
                }
            }
            2 => {
                // Interrupt Source Override
                let entry = unsafe { &*(entries.as_ptr().add(offset) as *const MadtInterruptOverride) };
                let bus = entry.bus;
                let source = entry.source;
                let gsi = entry.global_system_interrupt;
                log::debug!("IRQ Override: bus {} source {} -> GSI {}", 
                    bus, source, gsi);
            }
            4 => {
                // NMI
                let entry = unsafe { &*(entries.as_ptr().add(offset) as *const MadtNmi) };
                log::debug!("NMI: processor {} LINT{}", 
                    entry.acpi_processor_id, entry.lint);
            }
            9 => {
                // Processor Local x2APIC
                let entry = unsafe { &*(entries.as_ptr().add(offset) as *const MadtLocalX2Apic) };
                
                if entry.is_enabled() {
                    let x2apic_id = entry.x2apic_id;
                    info.cpu_count += 1;
                    info.apic_ids.push(x2apic_id);
                    log::debug!("CPU {}: x2APIC ID {}", info.cpu_count - 1, x2apic_id);
                }
            }
            _ => {
                log::debug!("Unknown MADT entry type {}", header.entry_type);
            }
        }
        
        offset += header.length as usize;
    }
    
    log::info!("Detected {} CPUs", info.cpu_count);
    
    if info.cpu_count == 0 {
        return Err("No CPUs detected in MADT");
    }
    
    if info.ioapic_address == 0 {
        log::warn!("No I/O APIC found in MADT");
    }
    
    Ok(info)
}
