//! CPU Topology Detection
//! 
//! Detect cores, threads, cache hierarchy, NUMA nodes.

use super::cpuid::cpuid;

/// CPU topology information
#[derive(Debug, Clone, Copy)]
pub struct CpuTopology {
    pub physical_cores: u32,
    pub logical_cores: u32,
    pub l1_cache_size: u32,   // KB
    pub l2_cache_size: u32,   // KB
    pub l3_cache_size: u32,   // KB
    pub cache_line_size: u32, // bytes
}

static mut TOPOLOGY: CpuTopology = CpuTopology {
    physical_cores: 1,
    logical_cores: 1,
    l1_cache_size: 0,
    l2_cache_size: 0,
    l3_cache_size: 0,
    cache_line_size: 64,
};

/// Detect CPU topology
pub fn detect() -> CpuTopology {
    let r1 = cpuid(1);
    let logical_cores = ((r1.ebx >> 16) & 0xFF) as u32;
    
    // Detect cache info (simplified)
    let r4 = cpuid(4);
    let l1_cache = if r4.eax & 0x1F == 1 { // Data cache
        ((r4.ebx >> 22) + 1) * ((r4.ebx >> 12) & 0x3FF) * (r4.ebx & 0xFFF) * (r4.ecx + 1) / 1024
    } else { 32 };
    
    CpuTopology {
        physical_cores: logical_cores, // TODO: Detect HT
        logical_cores,
        l1_cache_size: l1_cache,
        l2_cache_size: 256,  // TODO: Detect from CPUID
        l3_cache_size: 8192, // TODO: Detect from CPUID
        cache_line_size: 64,
    }
}

/// Get cached topology
pub fn get() -> CpuTopology {
    unsafe { TOPOLOGY }
}

pub fn init() {
    let topo = detect();
    unsafe { TOPOLOGY = topo; }
    
    log::info!("CPU Topology:");
    log::info!("  Cores: {} physical, {} logical", topo.physical_cores, topo.logical_cores);
    log::info!("  Cache: L1={}KB, L2={}KB, L3={}KB", 
        topo.l1_cache_size, topo.l2_cache_size, topo.l3_cache_size);
}
