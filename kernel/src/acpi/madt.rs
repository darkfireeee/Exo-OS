//! MADT (Multiple APIC Description Table) Parser
//!
//! Parses the MADT/APIC table to discover:
//! - Local APICs (CPUs)
//! - I/O APICs
//! - Interrupt Source Overrides
//! - NMI Sources

use alloc::vec::Vec;
use super::tables::AcpiSdtHeader;

/// MADT Entry Types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MadtEntryType {
    LocalApic = 0,
    IoApic = 1,
    InterruptSourceOverride = 2,
    NmiSource = 3,
    LocalApicNmi = 4,
    LocalApicAddressOverride = 5,
    IoSapic = 6,
    LocalSapic = 7,
    PlatformInterruptSources = 8,
    LocalX2Apic = 9,
    LocalX2ApicNmi = 10,
    GicCpu = 11,
    GicDistributor = 12,
    GicMsiFrame = 13,
    GicRedistributor = 14,
    GicIts = 15,
}

/// MADT Header
#[repr(C, packed)]
pub struct MadtHeader {
    pub header: AcpiSdtHeader,
    /// Physical address of Local APIC
    pub local_apic_address: u32,
    /// Flags (bit 0 = PCAT_COMPAT - dual 8259 setup)
    pub flags: u32,
}

/// Local APIC Entry (Type 0)
#[repr(C, packed)]
pub struct MadtLocalApic {
    pub entry_type: u8,
    pub length: u8,
    /// ACPI Processor UID
    pub acpi_processor_id: u8,
    /// Processor's Local APIC ID
    pub apic_id: u8,
    /// Flags (bit 0 = enabled, bit 1 = online capable)
    pub flags: u32,
}

impl MadtLocalApic {
    pub fn is_enabled(&self) -> bool {
        (self.flags & 0x01) != 0
    }
    
    pub fn is_online_capable(&self) -> bool {
        (self.flags & 0x02) != 0
    }
}

/// I/O APIC Entry (Type 1)
#[repr(C, packed)]
pub struct MadtIoApic {
    pub entry_type: u8,
    pub length: u8,
    /// I/O APIC ID
    pub io_apic_id: u8,
    pub reserved: u8,
    /// Physical address of I/O APIC
    pub io_apic_address: u32,
    /// Global System Interrupt Base
    pub global_system_interrupt_base: u32,
}

/// Interrupt Source Override Entry (Type 2)
#[repr(C, packed)]
pub struct MadtInterruptSourceOverride {
    pub entry_type: u8,
    pub length: u8,
    /// Bus (0 = ISA)
    pub bus: u8,
    /// Bus-relative IRQ
    pub source: u8,
    /// Global System Interrupt this source maps to
    pub global_system_interrupt: u32,
    /// Flags (polarity, trigger mode)
    pub flags: u16,
}

/// Local x2APIC Entry (Type 9)
#[repr(C, packed)]
pub struct MadtLocalX2Apic {
    pub entry_type: u8,
    pub length: u8,
    pub reserved: u16,
    /// Processor's local x2APIC ID
    pub x2apic_id: u32,
    /// Flags (same as Local APIC)
    pub flags: u32,
    /// ACPI Processor UID
    pub acpi_processor_uid: u32,
}

impl MadtLocalX2Apic {
    pub fn is_enabled(&self) -> bool {
        (self.flags & 0x01) != 0
    }
    
    pub fn is_online_capable(&self) -> bool {
        (self.flags & 0x02) != 0
    }
}

/// Parsed CPU information from MADT
#[derive(Debug, Clone, Copy)]
pub struct CpuInfo {
    /// Local APIC ID
    pub apic_id: u32,
    /// ACPI Processor ID
    pub acpi_id: u32,
    /// Is this the BSP?
    pub is_bsp: bool,
    /// Is this CPU enabled?
    pub enabled: bool,
    /// Can this CPU be brought online?
    pub online_capable: bool,
}

/// Parsed I/O APIC information
#[derive(Debug, Clone, Copy)]
pub struct IoApicInfo {
    pub id: u8,
    pub address: u32,
    pub gsi_base: u32,
}

/// Parsed interrupt override information
#[derive(Debug, Clone, Copy)]
pub struct InterruptOverride {
    pub source_irq: u8,
    pub global_irq: u32,
    pub polarity: u8,
    pub trigger_mode: u8,
}

/// Parsed MADT information
#[derive(Debug)]
pub struct MadtInfo {
    /// Local APIC base address
    pub local_apic_address: u32,
    /// Legacy dual 8259 present
    pub has_8259: bool,
    /// List of CPUs
    pub cpus: Vec<CpuInfo>,
    /// List of I/O APICs
    pub io_apics: Vec<IoApicInfo>,
    /// Interrupt source overrides
    pub overrides: Vec<InterruptOverride>,
    /// BSP's APIC ID
    pub bsp_apic_id: u32,
}

/// Parse MADT table
/// 
/// # Safety
/// madt_addr must point to a valid MADT table
pub unsafe fn parse_madt(madt_addr: u64) -> Option<MadtInfo> {
    let header = &*(madt_addr as *const MadtHeader);
    
    // Verify signature
    if &header.header.signature != b"APIC" {
        log::error!("MADT: Invalid signature");
        return None;
    }
    
    // Get BSP's APIC ID (current CPU running this code is BSP)
    let bsp_apic_id = get_current_apic_id();
    
    let mut info = MadtInfo {
        local_apic_address: header.local_apic_address,
        has_8259: (header.flags & 0x01) != 0,
        cpus: Vec::new(),
        io_apics: Vec::new(),
        overrides: Vec::new(),
        bsp_apic_id,
    };
    
    // Parse entries
    let table_end = madt_addr + header.header.length as u64;
    let mut entry_addr = madt_addr + core::mem::size_of::<MadtHeader>() as u64;
    
    while entry_addr < table_end {
        let entry_type = *(entry_addr as *const u8);
        let entry_length = *((entry_addr + 1) as *const u8);
        
        if entry_length == 0 {
            break; // Invalid entry
        }
        
        match entry_type {
            0 => {
                // Local APIC
                let entry = &*(entry_addr as *const MadtLocalApic);
                if entry.is_enabled() || entry.is_online_capable() {
                    info.cpus.push(CpuInfo {
                        apic_id: entry.apic_id as u32,
                        acpi_id: entry.acpi_processor_id as u32,
                        is_bsp: entry.apic_id as u32 == bsp_apic_id,
                        enabled: entry.is_enabled(),
                        online_capable: entry.is_online_capable(),
                    });
                }
            }
            1 => {
                // I/O APIC
                let entry = &*(entry_addr as *const MadtIoApic);
                info.io_apics.push(IoApicInfo {
                    id: entry.io_apic_id,
                    address: entry.io_apic_address,
                    gsi_base: entry.global_system_interrupt_base,
                });
            }
            2 => {
                // Interrupt Source Override
                let entry = &*(entry_addr as *const MadtInterruptSourceOverride);
                info.overrides.push(InterruptOverride {
                    source_irq: entry.source,
                    global_irq: entry.global_system_interrupt,
                    polarity: (entry.flags & 0x03) as u8,
                    trigger_mode: ((entry.flags >> 2) & 0x03) as u8,
                });
            }
            9 => {
                // Local x2APIC (for systems with > 255 CPUs)
                let entry = &*(entry_addr as *const MadtLocalX2Apic);
                if entry.is_enabled() || entry.is_online_capable() {
                    info.cpus.push(CpuInfo {
                        apic_id: entry.x2apic_id,
                        acpi_id: entry.acpi_processor_uid,
                        is_bsp: entry.x2apic_id == bsp_apic_id,
                        enabled: entry.is_enabled(),
                        online_capable: entry.is_online_capable(),
                    });
                }
            }
            _ => {
                // Skip unknown entry types
            }
        }
        
        entry_addr += entry_length as u64;
    }
    
    log::info!("MADT: Found {} CPUs, {} I/O APICs, {} overrides",
        info.cpus.len(), info.io_apics.len(), info.overrides.len());
    
    Some(info)
}

/// Get current CPU's APIC ID
fn get_current_apic_id() -> u32 {
    unsafe {
        // Try x2APIC first
        let apic_base = rdmsr(0x1B);
        if (apic_base & (1 << 10)) != 0 {
            // x2APIC mode
            rdmsr(0x802) as u32
        } else {
            // xAPIC mode - read from MMIO
            // Fallback: use CPUID
            let cpuid = core::arch::x86_64::__cpuid(1);
            (cpuid.ebx >> 24) as u32
        }
    }
}

#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nomem, nostack, preserves_flags)
    );
    ((high as u64) << 32) | (low as u64)
}

/// Get MADT info if available
pub fn get_madt_info() -> Option<MadtInfo> {
    if let Some(madt_addr) = crate::acpi::get_madt_addr() {
        unsafe { parse_madt(madt_addr) }
    } else {
        None
    }
}
