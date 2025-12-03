//! CPU Topology Detection
//! 
//! Detect cores, threads (HT/SMT), cache hierarchy using CPUID.
//! Uses modern CPUID leaf 0x1F (Intel) / 0x8000001E (AMD) for accurate topology.

use core::arch::x86_64::__cpuid;

/// CPU topology information
#[derive(Debug, Clone, Copy)]
pub struct CpuTopology {
    /// Number of physical CPU cores
    pub physical_cores: u32,
    /// Number of logical CPUs (threads)
    pub logical_cores: u32,
    /// Threads per core (1 = no SMT, 2 = HT enabled)
    pub threads_per_core: u32,
    /// Number of CPU packages/sockets
    pub packages: u32,
    /// Is Hyper-Threading / SMT enabled?
    pub smt_enabled: bool,
    /// CPU vendor
    pub vendor: CpuVendor,
    /// APIC ID bits for SMT
    pub smt_mask_width: u32,
    /// APIC ID bits for core
    pub core_mask_width: u32,
}

/// CPU vendor
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuVendor {
    Intel,
    Amd,
    Unknown,
}

static mut TOPOLOGY: CpuTopology = CpuTopology {
    physical_cores: 1,
    logical_cores: 1,
    threads_per_core: 1,
    packages: 1,
    smt_enabled: false,
    vendor: CpuVendor::Unknown,
    smt_mask_width: 0,
    core_mask_width: 0,
};

/// Detect CPU vendor
fn detect_vendor() -> CpuVendor {
    unsafe {
        let r = __cpuid(0);
        let vendor_bytes: [u8; 12] = [
            (r.ebx & 0xFF) as u8, ((r.ebx >> 8) & 0xFF) as u8,
            ((r.ebx >> 16) & 0xFF) as u8, ((r.ebx >> 24) & 0xFF) as u8,
            (r.edx & 0xFF) as u8, ((r.edx >> 8) & 0xFF) as u8,
            ((r.edx >> 16) & 0xFF) as u8, ((r.edx >> 24) & 0xFF) as u8,
            (r.ecx & 0xFF) as u8, ((r.ecx >> 8) & 0xFF) as u8,
            ((r.ecx >> 16) & 0xFF) as u8, ((r.ecx >> 24) & 0xFF) as u8,
        ];
        
        if &vendor_bytes == b"GenuineIntel" {
            CpuVendor::Intel
        } else if &vendor_bytes == b"AuthenticAMD" {
            CpuVendor::Amd
        } else {
            CpuVendor::Unknown
        }
    }
}

/// Detect CPU topology using CPUID
pub fn detect() -> CpuTopology {
    let vendor = detect_vendor();
    
    unsafe {
        // Basic info from leaf 1
        let r1 = __cpuid(1);
        let logical_per_package = ((r1.ebx >> 16) & 0xFF) as u32;
        let has_htt = (r1.edx & (1 << 28)) != 0;
        
        // Try modern topology enumeration (leaf 0x1F or 0x0B)
        if let Some(topo) = detect_topology_modern(vendor) {
            return topo;
        }
        
        // Fallback: Use legacy detection
        detect_topology_legacy(vendor, logical_per_package, has_htt)
    }
}

/// Modern topology detection using CPUID leaf 0x1F (Intel) or extended AMD leaves
unsafe fn detect_topology_modern(vendor: CpuVendor) -> Option<CpuTopology> {
    // Check max CPUID leaf
    let r0 = __cpuid(0);
    let max_leaf = r0.eax;
    
    // Intel: Try leaf 0x1F first (V2 Extended Topology), then 0x0B
    if max_leaf >= 0x1F {
        let r = __cpuid(0x1F);
        if r.ebx != 0 {
            return Some(parse_intel_topology(0x1F, vendor));
        }
    }
    
    if max_leaf >= 0x0B {
        let r = __cpuid(0x0B);
        if r.ebx != 0 {
            return Some(parse_intel_topology(0x0B, vendor));
        }
    }
    
    // AMD: Use leaf 0x8000001E
    let r_ext = __cpuid(0x80000000);
    if r_ext.eax >= 0x8000001E && vendor == CpuVendor::Amd {
        return Some(parse_amd_topology());
    }
    
    None
}

/// Parse Intel topology from CPUID leaf 0x0B or 0x1F
unsafe fn parse_intel_topology(leaf: u32, vendor: CpuVendor) -> CpuTopology {
    let mut smt_mask_width = 0u32;
    let mut core_mask_width = 0u32;
    let mut logical_cores = 1u32;
    let mut threads_per_core = 1u32;
    let mut cores_per_package = 1u32;
    
    // Iterate through topology levels
    for subleaf in 0..8 {
        let r = __cpuid_count(leaf, subleaf);
        
        let level_type = (r.ecx >> 8) & 0xFF;
        let shift = r.eax & 0x1F;
        let count = r.ebx & 0xFFFF;
        
        if level_type == 0 {
            break; // Invalid level
        }
        
        match level_type {
            1 => {
                // SMT level
                smt_mask_width = shift;
                threads_per_core = count;
            }
            2 => {
                // Core level
                core_mask_width = shift;
                cores_per_package = count / threads_per_core.max(1);
            }
            _ => {}
        }
        
        logical_cores = count;
    }
    
    let physical_cores = logical_cores / threads_per_core.max(1);
    let smt_enabled = threads_per_core > 1;
    
    CpuTopology {
        physical_cores,
        logical_cores,
        threads_per_core,
        packages: 1, // Assume single socket
        smt_enabled,
        vendor,
        smt_mask_width,
        core_mask_width,
    }
}

/// Parse AMD topology from extended CPUID leaves
unsafe fn parse_amd_topology() -> CpuTopology {
    let r1 = __cpuid(1);
    let logical_per_package = ((r1.ebx >> 16) & 0xFF) as u32;
    
    // Leaf 0x8000001E: Extended APIC ID
    let r_1e = __cpuid(0x8000001E);
    let threads_per_core = ((r_1e.ebx >> 8) & 0xFF) as u32 + 1;
    
    // Leaf 0x80000008: Size identifiers
    let r_08 = __cpuid(0x80000008);
    let core_count = ((r_08.ecx) & 0xFF) as u32 + 1;
    
    let logical_cores = logical_per_package.max(core_count * threads_per_core);
    let physical_cores = logical_cores / threads_per_core;
    
    CpuTopology {
        physical_cores,
        logical_cores,
        threads_per_core,
        packages: 1,
        smt_enabled: threads_per_core > 1,
        vendor: CpuVendor::Amd,
        smt_mask_width: log2(threads_per_core),
        core_mask_width: log2(physical_cores),
    }
}

/// Legacy topology detection fallback
unsafe fn detect_topology_legacy(vendor: CpuVendor, logical_per_package: u32, has_htt: bool) -> CpuTopology {
    let mut physical_cores = logical_per_package;
    let mut threads_per_core = 1;
    
    if has_htt {
        // Check for actual core count
        let r0 = __cpuid(0);
        if r0.eax >= 4 {
            let r4 = __cpuid_count(4, 0);
            if (r4.eax & 0x1F) != 0 {
                // Valid cache info
                physical_cores = ((r4.eax >> 26) & 0x3F) + 1;
                threads_per_core = logical_per_package / physical_cores.max(1);
            }
        }
    }
    
    CpuTopology {
        physical_cores: physical_cores.max(1),
        logical_cores: logical_per_package.max(1),
        threads_per_core: threads_per_core.max(1),
        packages: 1,
        smt_enabled: threads_per_core > 1,
        vendor,
        smt_mask_width: log2(threads_per_core),
        core_mask_width: log2(physical_cores),
    }
}

/// Integer log2
fn log2(mut n: u32) -> u32 {
    if n == 0 { return 0; }
    let mut r = 0;
    while n > 1 {
        n >>= 1;
        r += 1;
    }
    r
}

/// CPUID with subleaf - RBX safe version (LLVM reserves RBX)
#[inline]
unsafe fn __cpuid_count(leaf: u32, subleaf: u32) -> core::arch::x86_64::CpuidResult {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "mov {ebx_out:e}, ebx",
        "pop rbx",
        inout("eax") leaf => eax,
        inout("ecx") subleaf => ecx,
        ebx_out = out(reg) ebx,
        out("edx") edx,
        options(nostack, preserves_flags)
    );
    core::arch::x86_64::CpuidResult { eax, ebx, ecx, edx }
}

/// Get cached topology
pub fn get() -> CpuTopology {
    unsafe { TOPOLOGY }
}

/// Initialize topology detection
pub fn init() {
    let topo = detect();
    unsafe { TOPOLOGY = topo; }
    
    log::info!("CPU Topology detected:");
    log::info!("  Vendor: {:?}", topo.vendor);
    log::info!("  Physical cores: {}", topo.physical_cores);
    log::info!("  Logical cores: {}", topo.logical_cores);
    log::info!("  Threads/core: {} (SMT: {})", 
        topo.threads_per_core, 
        if topo.smt_enabled { "enabled" } else { "disabled" });
}

/// Get APIC ID masks for topology decomposition
pub fn get_apic_masks() -> (u32, u32, u32) {
    let topo = get();
    let smt_mask = (1 << topo.smt_mask_width) - 1;
    let core_mask = ((1 << topo.core_mask_width) - 1) ^ smt_mask;
    let pkg_mask = !((1 << topo.core_mask_width) - 1);
    (smt_mask, core_mask, pkg_mask)
}

/// Extract SMT ID from APIC ID
pub fn get_smt_id(apic_id: u32) -> u32 {
    let (smt_mask, _, _) = get_apic_masks();
    apic_id & smt_mask
}

/// Extract Core ID from APIC ID  
pub fn get_core_id(apic_id: u32) -> u32 {
    let topo = get();
    let (_, core_mask, _) = get_apic_masks();
    (apic_id & core_mask) >> topo.smt_mask_width
}

/// Extract Package ID from APIC ID
pub fn get_package_id(apic_id: u32) -> u32 {
    let topo = get();
    apic_id >> topo.core_mask_width
}
