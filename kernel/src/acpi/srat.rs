//! SRAT (System Resource Affinity Table) Parser
//!
//! Parses NUMA topology from ACPI SRAT table

use super::tables::AcpiSdtHeader;
use alloc::vec::Vec;

/// SRAT structure types
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SratStructureType {
    /// Processor Local APIC/SAPIC Affinity
    ProcessorLocalApic = 0,
    /// Memory Affinity
    MemoryAffinity = 1,
    /// Processor Local x2APIC Affinity
    ProcessorLocalX2Apic = 2,
    /// GICC Affinity (ARM)
    GiccAffinity = 3,
    /// GIC ITS Affinity (ARM)
    GicItsAffinity = 4,
    /// Generic Initiator Affinity
    GenericInitiator = 5,
}

/// SRAT Memory Affinity Structure
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct SratMemoryAffinity {
    /// Type (1 = Memory Affinity)
    pub typ: u8,
    /// Length (40 bytes)
    pub length: u8,
    /// Proximity domain (low 8 bits for ACPI 1.0)
    pub proximity_domain_lo: u8,
    /// Reserved
    pub reserved1: u8,
    /// Base address low 32 bits
    pub base_addr_lo: u32,
    /// Base address high 32 bits
    pub base_addr_hi: u32,
    /// Length low 32 bits
    pub length_lo: u32,
    /// Length high 32 bits
    pub length_hi: u32,
    /// Reserved
    pub reserved2: u32,
    /// Flags
    pub flags: u32,
    /// Reserved
    pub reserved3: u64,
}

impl SratMemoryAffinity {
    /// Get full 64-bit base address
    pub fn base_address(&self) -> u64 {
        (self.base_addr_hi as u64) << 32 | self.base_addr_lo as u64
    }
    
    /// Get full 64-bit length
    pub fn length(&self) -> u64 {
        (self.length_hi as u64) << 32 | self.length_lo as u64
    }
    
    /// Check if entry is enabled
    pub fn is_enabled(&self) -> bool {
        self.flags & 1 != 0
    }
    
    /// Check if memory is hot-pluggable
    pub fn is_hotpluggable(&self) -> bool {
        self.flags & 2 != 0
    }
    
    /// Check if memory is non-volatile
    pub fn is_nonvolatile(&self) -> bool {
        self.flags & 4 != 0
    }
}

/// SRAT Processor Local APIC Affinity Structure
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct SratProcessorApicAffinity {
    /// Type (0 = Processor APIC Affinity)
    pub typ: u8,
    /// Length (16 bytes)
    pub length: u8,
    /// Proximity domain (low 8 bits)
    pub proximity_domain_lo: u8,
    /// Local APIC ID
    pub apic_id: u8,
    /// Flags
    pub flags: u32,
    /// Local SAPIC EID
    pub sapic_eid: u8,
    /// Proximity domain high 24 bits
    pub proximity_domain_hi: [u8; 3],
    /// Clock domain
    pub clock_domain: u32,
}

impl SratProcessorApicAffinity {
    /// Get full proximity domain
    pub fn proximity_domain(&self) -> u32 {
        self.proximity_domain_lo as u32
            | (self.proximity_domain_hi[0] as u32) << 8
            | (self.proximity_domain_hi[1] as u32) << 16
            | (self.proximity_domain_hi[2] as u32) << 24
    }
    
    /// Check if entry is enabled
    pub fn is_enabled(&self) -> bool {
        self.flags & 1 != 0
    }
}

/// SRAT Processor Local x2APIC Affinity Structure
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct SratProcessorX2ApicAffinity {
    /// Type (2 = Processor x2APIC Affinity)
    pub typ: u8,
    /// Length (24 bytes)
    pub length: u8,
    /// Reserved
    pub reserved1: [u8; 2],
    /// Proximity domain
    pub proximity_domain: u32,
    /// x2APIC ID
    pub x2apic_id: u32,
    /// Flags
    pub flags: u32,
    /// Clock domain
    pub clock_domain: u32,
    /// Reserved
    pub reserved2: [u8; 4],
}

impl SratProcessorX2ApicAffinity {
    /// Check if entry is enabled
    pub fn is_enabled(&self) -> bool {
        self.flags & 1 != 0
    }
}

/// Parsed NUMA memory region
#[derive(Debug, Clone)]
pub struct NumaMemoryAffinity {
    /// NUMA node (proximity domain)
    pub node: u32,
    /// Base physical address
    pub base_addr: u64,
    /// Region size in bytes
    pub size: u64,
    /// Hot-pluggable memory
    pub hotpluggable: bool,
    /// Non-volatile memory (persistent)
    pub nonvolatile: bool,
}

/// Parsed NUMA processor affinity
#[derive(Debug, Clone)]
pub struct NumaProcessorAffinity {
    /// NUMA node (proximity domain)
    pub node: u32,
    /// APIC ID (local or x2APIC)
    pub apic_id: u32,
    /// Is x2APIC format
    pub is_x2apic: bool,
}

/// Parsed SRAT information
#[derive(Debug, Default)]
pub struct SratInfo {
    /// Memory affinity entries
    pub memory_affinities: Vec<NumaMemoryAffinity>,
    /// Processor affinity entries
    pub processor_affinities: Vec<NumaProcessorAffinity>,
    /// Number of NUMA nodes detected
    pub node_count: u32,
}

/// Parse SRAT table
pub unsafe fn parse_srat(srat_addr: u64) -> Option<SratInfo> {
    if srat_addr == 0 {
        return None;
    }
    
    let header = &*(srat_addr as *const AcpiSdtHeader);
    
    // Verify signature
    if &header.signature != b"SRAT" {
        log::error!("SRAT: Invalid signature");
        return None;
    }
    
    let mut info = SratInfo::default();
    let mut max_node = 0u32;
    
    // Parse structures after header (36 bytes) + reserved (12 bytes) = 48 bytes
    let mut offset = 48usize;
    let table_end = header.length as usize;
    
    while offset + 2 <= table_end {
        let struct_addr = srat_addr + offset as u64;
        let struct_type = *(struct_addr as *const u8);
        let struct_len = *((struct_addr + 1) as *const u8) as usize;
        
        if struct_len == 0 {
            break;
        }
        
        match struct_type {
            0 => {
                // Processor Local APIC Affinity
                if struct_len >= 16 && offset + struct_len <= table_end {
                    let entry = &*(struct_addr as *const SratProcessorApicAffinity);
                    if entry.is_enabled() {
                        let node = entry.proximity_domain();
                        info.processor_affinities.push(NumaProcessorAffinity {
                            node,
                            apic_id: entry.apic_id as u32,
                            is_x2apic: false,
                        });
                        if node > max_node {
                            max_node = node;
                        }
                    }
                }
            }
            1 => {
                // Memory Affinity
                if struct_len >= 40 && offset + struct_len <= table_end {
                    let entry = &*(struct_addr as *const SratMemoryAffinity);
                    if entry.is_enabled() && entry.length() > 0 {
                        let node = entry.proximity_domain_lo as u32;
                        info.memory_affinities.push(NumaMemoryAffinity {
                            node,
                            base_addr: entry.base_address(),
                            size: entry.length(),
                            hotpluggable: entry.is_hotpluggable(),
                            nonvolatile: entry.is_nonvolatile(),
                        });
                        if node > max_node {
                            max_node = node;
                        }
                        
                        log::debug!(
                            "SRAT: Memory node {} @ {:#x} size {} MB",
                            node,
                            entry.base_address(),
                            entry.length() / (1024 * 1024)
                        );
                    }
                }
            }
            2 => {
                // Processor Local x2APIC Affinity
                if struct_len >= 24 && offset + struct_len <= table_end {
                    let entry = &*(struct_addr as *const SratProcessorX2ApicAffinity);
                    if entry.is_enabled() {
                        info.processor_affinities.push(NumaProcessorAffinity {
                            node: entry.proximity_domain,
                            apic_id: entry.x2apic_id,
                            is_x2apic: true,
                        });
                        if entry.proximity_domain > max_node {
                            max_node = entry.proximity_domain;
                        }
                    }
                }
            }
            _ => {
                // Unknown structure type, skip
            }
        }
        
        offset += struct_len;
    }
    
    info.node_count = max_node + 1;
    
    log::info!(
        "SRAT: Parsed {} memory regions, {} processors, {} NUMA nodes",
        info.memory_affinities.len(),
        info.processor_affinities.len(),
        info.node_count
    );
    
    Some(info)
}
