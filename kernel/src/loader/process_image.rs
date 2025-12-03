//! Process Image Management
//! 
//! Structures representing a loaded executable in memory.

use crate::memory::VirtualAddress;
use alloc::string::String;
use alloc::vec::Vec;

/// A loaded ELF executable
#[derive(Debug)]
pub struct LoadedElf {
    /// Entry point address
    pub entry_point: VirtualAddress,
    /// Base address bias (for PIE)
    pub load_bias: VirtualAddress,
    /// Loaded segments
    pub segments: Vec<LoadedSegment>,
    /// TLS template if present
    pub tls_template: Option<TlsTemplate>,
    /// Path to dynamic linker if required
    pub interpreter: Option<String>,
    /// Address of program headers in memory
    pub phdr_addr: VirtualAddress,
    /// Number of program headers
    pub phdr_num: usize,
    /// Size of each program header
    pub phdr_size: usize,
}

impl LoadedElf {
    /// Get the total memory footprint
    pub fn memory_size(&self) -> usize {
        self.segments.iter().map(|s| s.mem_size).sum()
    }
    
    /// Check if this executable needs a dynamic linker
    pub fn needs_interpreter(&self) -> bool {
        self.interpreter.is_some()
    }
    
    /// Get the lowest virtual address
    pub fn base_address(&self) -> VirtualAddress {
        self.segments
            .iter()
            .map(|s| s.vaddr)
            .min()
            .unwrap_or(VirtualAddress::new(0))
    }
    
    /// Get the highest virtual address
    pub fn end_address(&self) -> VirtualAddress {
        self.segments
            .iter()
            .map(|s| VirtualAddress::new(s.vaddr.as_usize() + s.mem_size))
            .max()
            .unwrap_or(VirtualAddress::new(0))
    }
}

/// A loaded segment in memory
#[derive(Debug, Clone)]
pub struct LoadedSegment {
    /// Virtual address (page-aligned)
    pub vaddr: VirtualAddress,
    /// Size in memory (page-aligned)
    pub mem_size: usize,
    /// Offset in file
    pub file_offset: usize,
    /// Size in file
    pub file_size: usize,
    /// Offset within first page
    pub page_offset: usize,
    /// Permission flags
    pub flags: SegmentFlags,
    /// Data offset in ELF file
    pub data_offset: usize,
}

/// Segment permission flags
#[derive(Debug, Clone, Copy, Default)]
pub struct SegmentFlags {
    /// Readable
    pub read: bool,
    /// Writable
    pub write: bool,
    /// Executable
    pub execute: bool,
}

impl SegmentFlags {
    /// Convert to page table flags
    pub fn to_page_flags(&self) -> u64 {
        let mut flags = 0u64;
        
        // Present bit
        flags |= 1 << 0;
        
        // Write bit
        if self.write {
            flags |= 1 << 1;
        }
        
        // User bit (always set for userspace)
        flags |= 1 << 2;
        
        // No-Execute bit (set if NOT executable)
        if !self.execute {
            flags |= 1 << 63;
        }
        
        flags
    }
}

/// TLS (Thread-Local Storage) template
#[derive(Debug, Clone)]
pub struct TlsTemplate {
    /// Address of TLS template in memory
    pub addr: VirtualAddress,
    /// Size of initialized data in file
    pub file_size: usize,
    /// Total size in memory (includes .tbss)
    pub mem_size: usize,
    /// Alignment requirement
    pub align: usize,
}

impl TlsTemplate {
    /// Calculate the TLS block size for a thread
    pub fn block_size(&self) -> usize {
        // Align to template alignment
        (self.mem_size + self.align - 1) & !(self.align - 1)
    }
}

/// Auxiliary vector entry for process startup
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct AuxEntry {
    pub a_type: u64,
    pub a_val: u64,
}

// Auxiliary vector types
pub const AT_NULL: u64 = 0;
pub const AT_IGNORE: u64 = 1;
pub const AT_EXECFD: u64 = 2;
pub const AT_PHDR: u64 = 3;
pub const AT_PHENT: u64 = 4;
pub const AT_PHNUM: u64 = 5;
pub const AT_PAGESZ: u64 = 6;
pub const AT_BASE: u64 = 7;
pub const AT_FLAGS: u64 = 8;
pub const AT_ENTRY: u64 = 9;
pub const AT_NOTELF: u64 = 10;
pub const AT_UID: u64 = 11;
pub const AT_EUID: u64 = 12;
pub const AT_GID: u64 = 13;
pub const AT_EGID: u64 = 14;
pub const AT_PLATFORM: u64 = 15;
pub const AT_HWCAP: u64 = 16;
pub const AT_CLKTCK: u64 = 17;
pub const AT_SECURE: u64 = 23;
pub const AT_BASE_PLATFORM: u64 = 24;
pub const AT_RANDOM: u64 = 25;
pub const AT_HWCAP2: u64 = 26;
pub const AT_EXECFN: u64 = 31;
pub const AT_SYSINFO_EHDR: u64 = 33;

/// Build auxiliary vector for process startup
pub fn build_auxv(
    loaded: &LoadedElf,
    interp_base: Option<VirtualAddress>,
    random_ptr: VirtualAddress,
) -> Vec<AuxEntry> {
    use crate::memory::PAGE_SIZE;
    
    let mut auxv = Vec::with_capacity(16);
    
    // Program headers
    auxv.push(AuxEntry { a_type: AT_PHDR, a_val: loaded.phdr_addr.as_u64() });
    auxv.push(AuxEntry { a_type: AT_PHENT, a_val: loaded.phdr_size as u64 });
    auxv.push(AuxEntry { a_type: AT_PHNUM, a_val: loaded.phdr_num as u64 });
    
    // Page size
    auxv.push(AuxEntry { a_type: AT_PAGESZ, a_val: PAGE_SIZE as u64 });
    
    // Entry point
    auxv.push(AuxEntry { a_type: AT_ENTRY, a_val: loaded.entry_point.as_u64() });
    
    // Interpreter base (if dynamic)
    if let Some(base) = interp_base {
        auxv.push(AuxEntry { a_type: AT_BASE, a_val: base.as_u64() });
    }
    
    // Random bytes pointer (for stack canary)
    auxv.push(AuxEntry { a_type: AT_RANDOM, a_val: random_ptr.as_u64() });
    
    // UID/GID (always 0 for now)
    auxv.push(AuxEntry { a_type: AT_UID, a_val: 0 });
    auxv.push(AuxEntry { a_type: AT_EUID, a_val: 0 });
    auxv.push(AuxEntry { a_type: AT_GID, a_val: 0 });
    auxv.push(AuxEntry { a_type: AT_EGID, a_val: 0 });
    
    // Clock tick (100Hz)
    auxv.push(AuxEntry { a_type: AT_CLKTCK, a_val: 100 });
    
    // Not secure
    auxv.push(AuxEntry { a_type: AT_SECURE, a_val: 0 });
    
    // Null terminator
    auxv.push(AuxEntry { a_type: AT_NULL, a_val: 0 });
    
    auxv
}

/// Stack layout for new process
/// 
/// High addresses:
///   - environment strings
///   - argument strings
///   - auxv (null-terminated)
///   - envp (null-terminated)
///   - argv (null-terminated)
///   - argc
/// Low addresses (stack pointer)
#[derive(Debug)]
pub struct ProcessStack {
    /// Stack pointer after setup
    pub sp: VirtualAddress,
    /// argc location
    pub argc_ptr: VirtualAddress,
    /// argv array location
    pub argv_ptr: VirtualAddress,
    /// envp array location
    pub envp_ptr: VirtualAddress,
    /// auxv array location
    pub auxv_ptr: VirtualAddress,
}

impl ProcessStack {
    /// Setup initial stack for a new process
    pub fn setup(
        stack_top: VirtualAddress,
        args: &[&str],
        env: &[&str],
        auxv: &[AuxEntry],
    ) -> Self {
        // For now, return minimal stack setup
        // Full implementation would write strings and pointers to stack
        
        let sp = VirtualAddress::new(stack_top.as_usize() - 0x100);
        
        ProcessStack {
            sp,
            argc_ptr: sp,
            argv_ptr: VirtualAddress::new(sp.as_usize() + 8),
            envp_ptr: VirtualAddress::new(sp.as_usize() + 8 + (args.len() + 1) * 8),
            auxv_ptr: VirtualAddress::new(sp.as_usize() + 8 + (args.len() + 1) * 8 + (env.len() + 1) * 8),
        }
    }
}
