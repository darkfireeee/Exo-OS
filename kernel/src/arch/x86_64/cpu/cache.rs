//! CPU Cache Detection
//!
//! Detect L1, L2, L3 cache sizes and properties using CPUID leaf 4.
//! Provides cache-aware allocation hints for memory subsystem.

use core::arch::x86_64::__cpuid;
use alloc::vec::Vec;

/// Cache type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheType {
    /// No more caches
    Null = 0,
    /// Data cache
    Data = 1,
    /// Instruction cache
    Instruction = 2,
    /// Unified cache (data + instruction)
    Unified = 3,
}

impl From<u32> for CacheType {
    fn from(v: u32) -> Self {
        match v & 0x1F {
            1 => CacheType::Data,
            2 => CacheType::Instruction,
            3 => CacheType::Unified,
            _ => CacheType::Null,
        }
    }
}

/// Cache level information
#[derive(Debug, Clone, Copy)]
pub struct CacheInfo {
    /// Cache level (1, 2, or 3)
    pub level: u8,
    /// Cache type
    pub cache_type: CacheType,
    /// Total cache size in bytes
    pub size: usize,
    /// Cache line size in bytes
    pub line_size: usize,
    /// Number of ways (associativity)
    pub ways: u32,
    /// Number of sets
    pub sets: u32,
    /// Number of cores sharing this cache
    pub shared_cores: u32,
    /// Is self-initializing?
    pub self_init: bool,
    /// Is fully associative?
    pub fully_associative: bool,
    /// Supports WBINVD/INVD?
    pub wbinvd: bool,
    /// Is inclusive of lower levels?
    pub inclusive: bool,
}

/// Complete cache hierarchy
#[derive(Debug)]
pub struct CacheHierarchy {
    /// L1 data cache
    pub l1d: Option<CacheInfo>,
    /// L1 instruction cache
    pub l1i: Option<CacheInfo>,
    /// L2 cache (usually unified)
    pub l2: Option<CacheInfo>,
    /// L3 cache (usually unified, shared)
    pub l3: Option<CacheInfo>,
    /// Cache line size (for memory alignment)
    pub line_size: usize,
    /// All caches
    pub all: Vec<CacheInfo>,
}

impl Default for CacheHierarchy {
    fn default() -> Self {
        Self {
            l1d: None,
            l1i: None,
            l2: None,
            l3: None,
            line_size: 64,
            all: Vec::new(),
        }
    }
}

/// Global cache hierarchy
static mut CACHE_HIERARCHY: Option<CacheHierarchy> = None;

/// Detect cache hierarchy using CPUID leaf 4
pub fn detect() -> CacheHierarchy {
    let mut hierarchy = CacheHierarchy::default();
    
    unsafe {
        // Check if leaf 4 is supported
        let r0 = __cpuid(0);
        if r0.eax < 4 {
            // Fallback to legacy detection
            return detect_legacy();
        }
        
        // Iterate through cache levels (subleaf 0, 1, 2, ...)
        for subleaf in 0..32 {
            let r = cpuid_count(4, subleaf);
            
            let cache_type = CacheType::from(r.eax);
            if cache_type == CacheType::Null {
                break;
            }
            
            let level = ((r.eax >> 5) & 0x7) as u8;
            let self_init = (r.eax & (1 << 8)) != 0;
            let fully_associative = (r.eax & (1 << 9)) != 0;
            let shared_cores = ((r.eax >> 14) & 0xFFF) + 1;
            
            // EBX fields
            let line_size = (r.ebx & 0xFFF) + 1;
            let partitions = ((r.ebx >> 12) & 0x3FF) + 1;
            let ways = ((r.ebx >> 22) & 0x3FF) + 1;
            
            // ECX = number of sets - 1
            let sets = r.ecx + 1;
            
            // EDX flags
            let wbinvd = (r.edx & 1) == 0; // Bit 0 = 0 means WBINVD supported
            let inclusive = (r.edx & 2) != 0;
            
            // Calculate total size
            let size = (line_size * partitions * ways * sets) as usize;
            
            let info = CacheInfo {
                level,
                cache_type,
                size,
                line_size: line_size as usize,
                ways,
                sets,
                shared_cores,
                self_init,
                fully_associative,
                wbinvd,
                inclusive,
            };
            
            // Update line size (should be consistent)
            hierarchy.line_size = line_size as usize;
            
            // Store in appropriate slot
            match (level, cache_type) {
                (1, CacheType::Data) => hierarchy.l1d = Some(info),
                (1, CacheType::Instruction) => hierarchy.l1i = Some(info),
                (2, _) => hierarchy.l2 = Some(info),
                (3, _) => hierarchy.l3 = Some(info),
                _ => {}
            }
            
            hierarchy.all.push(info);
        }
    }
    
    hierarchy
}

/// Legacy cache detection using CPUID leaf 2
unsafe fn detect_legacy() -> CacheHierarchy {
    let mut hierarchy = CacheHierarchy::default();
    
    // Leaf 2 has descriptor bytes, but parsing is complex
    // Use reasonable defaults
    hierarchy.line_size = 64;
    
    // Check for L1 cache via leaf 0x80000005 (AMD) or estimates
    let r_ext = __cpuid(0x80000000);
    if r_ext.eax >= 0x80000005 {
        let r5 = __cpuid(0x80000005);
        
        // L1 data cache (ECX)
        let l1d_size = ((r5.ecx >> 24) & 0xFF) as usize * 1024;
        let l1d_line = (r5.ecx & 0xFF) as usize;
        if l1d_size > 0 {
            hierarchy.l1d = Some(CacheInfo {
                level: 1,
                cache_type: CacheType::Data,
                size: l1d_size,
                line_size: l1d_line,
                ways: ((r5.ecx >> 16) & 0xFF),
                sets: 0,
                shared_cores: 1,
                self_init: true,
                fully_associative: false,
                wbinvd: true,
                inclusive: false,
            });
        }
        
        // L1 instruction cache (EDX)
        let l1i_size = ((r5.edx >> 24) & 0xFF) as usize * 1024;
        let l1i_line = (r5.edx & 0xFF) as usize;
        if l1i_size > 0 {
            hierarchy.l1i = Some(CacheInfo {
                level: 1,
                cache_type: CacheType::Instruction,
                size: l1i_size,
                line_size: l1i_line,
                ways: ((r5.edx >> 16) & 0xFF),
                sets: 0,
                shared_cores: 1,
                self_init: true,
                fully_associative: false,
                wbinvd: true,
                inclusive: false,
            });
        }
    }
    
    // Check for L2/L3 via leaf 0x80000006 (AMD)
    if r_ext.eax >= 0x80000006 {
        let r6 = __cpuid(0x80000006);
        
        // L2 cache (ECX)
        let l2_size = ((r6.ecx >> 16) & 0xFFFF) as usize * 1024;
        let l2_line = (r6.ecx & 0xFF) as usize;
        if l2_size > 0 {
            hierarchy.l2 = Some(CacheInfo {
                level: 2,
                cache_type: CacheType::Unified,
                size: l2_size,
                line_size: l2_line,
                ways: (r6.ecx >> 12) & 0xF,
                sets: 0,
                shared_cores: 1,
                self_init: true,
                fully_associative: false,
                wbinvd: true,
                inclusive: false,
            });
            hierarchy.line_size = l2_line;
        }
        
        // L3 cache (EDX)
        let l3_size = ((r6.edx >> 18) & 0x3FFF) as usize * 512 * 1024;
        let l3_line = (r6.edx & 0xFF) as usize;
        if l3_size > 0 {
            hierarchy.l3 = Some(CacheInfo {
                level: 3,
                cache_type: CacheType::Unified,
                size: l3_size,
                line_size: l3_line,
                ways: (r6.edx >> 12) & 0xF,
                sets: 0,
                shared_cores: 4, // Estimate
                self_init: true,
                fully_associative: false,
                wbinvd: true,
                inclusive: true,
            });
        }
    }
    
    hierarchy
}

/// CPUID with subleaf - RBX safe version (LLVM reserves RBX)
#[inline]
unsafe fn cpuid_count(leaf: u32, subleaf: u32) -> CpuidResult {
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
    CpuidResult { eax, ebx, ecx, edx }
}

struct CpuidResult {
    eax: u32,
    ebx: u32,
    ecx: u32,
    edx: u32,
}

/// Get cached cache hierarchy
pub fn get() -> &'static CacheHierarchy {
    unsafe {
        CACHE_HIERARCHY.as_ref().unwrap_or_else(|| {
            // Shouldn't happen, but return a default
            static DEFAULT: CacheHierarchy = CacheHierarchy {
                l1d: None,
                l1i: None,
                l2: None,
                l3: None,
                line_size: 64,
                all: Vec::new(),
            };
            &DEFAULT
        })
    }
}

/// Initialize cache detection
pub fn init() {
    let hierarchy = detect();
    
    log::info!("Cache Hierarchy detected:");
    
    if let Some(ref l1d) = hierarchy.l1d {
        log::info!("  L1 Data: {}KB, {}-way, {} byte line", 
            l1d.size / 1024, l1d.ways, l1d.line_size);
    }
    
    if let Some(ref l1i) = hierarchy.l1i {
        log::info!("  L1 Inst: {}KB, {}-way, {} byte line",
            l1i.size / 1024, l1i.ways, l1i.line_size);
    }
    
    if let Some(ref l2) = hierarchy.l2 {
        log::info!("  L2: {}KB, {}-way, {} byte line, shared by {} cores",
            l2.size / 1024, l2.ways, l2.line_size, l2.shared_cores);
    }
    
    if let Some(ref l3) = hierarchy.l3 {
        log::info!("  L3: {}MB, {}-way, {} byte line, shared by {} cores",
            l3.size / 1024 / 1024, l3.ways, l3.line_size, l3.shared_cores);
    }
    
    unsafe {
        CACHE_HIERARCHY = Some(hierarchy);
    }
}

/// Get cache line size (for memory alignment)
pub fn line_size() -> usize {
    get().line_size
}

/// Get L1 data cache size
pub fn l1d_size() -> usize {
    get().l1d.map(|c| c.size).unwrap_or(32 * 1024)
}

/// Get L2 cache size
pub fn l2_size() -> usize {
    get().l2.map(|c| c.size).unwrap_or(256 * 1024)
}

/// Get L3 cache size
pub fn l3_size() -> usize {
    get().l3.map(|c| c.size).unwrap_or(0)
}

/// Check if L3 is inclusive
pub fn l3_inclusive() -> bool {
    get().l3.map(|c| c.inclusive).unwrap_or(false)
}

/// Flush cache line containing address
#[inline]
pub fn clflush(addr: usize) {
    unsafe {
        core::arch::asm!(
            "clflush [{}]",
            in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
}

/// Flush cache line and invalidate (CLFLUSHOPT)
#[inline]
pub fn clflushopt(addr: usize) {
    unsafe {
        core::arch::asm!(
            "clflushopt [{}]",
            in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
}

/// Memory fence (mfence)
#[inline]
pub fn mfence() {
    unsafe {
        core::arch::asm!("mfence", options(nostack, preserves_flags));
    }
}

/// Store fence (sfence)
#[inline]
pub fn sfence() {
    unsafe {
        core::arch::asm!("sfence", options(nostack, preserves_flags));
    }
}

/// Load fence (lfence)
#[inline]
pub fn lfence() {
    unsafe {
        core::arch::asm!("lfence", options(nostack, preserves_flags));
    }
}

/// Prefetch data for read
#[inline]
pub fn prefetch_read(addr: usize) {
    unsafe {
        core::arch::asm!(
            "prefetcht0 [{}]",
            in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
}

/// Prefetch data for write
#[inline]
pub fn prefetch_write(addr: usize) {
    unsafe {
        core::arch::asm!(
            "prefetchw [{}]",
            in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
}
