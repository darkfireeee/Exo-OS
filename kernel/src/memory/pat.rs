//! PAT (Page Attribute Table) Configuration
//!
//! Configures memory caching types for different memory regions.
//! PAT allows fine-grained control over memory type (WB, UC, WC, etc.)
//! on a per-page basis via page table PCD/PWT/PAT bits.
//!
//! ## Memory Types
//! - UC (Uncacheable): Device memory, MMIO
//! - WC (Write-Combining): Frame buffers, graphics memory
//! - WT (Write-Through): Some device memory
//! - WP (Write-Protected): ROM, read-only mappings
//! - WB (Write-Back): Normal RAM (default)
//! - UC- (Uncacheable Minus): Like UC but can be overridden by MTRRs

use core::arch::asm;

/// MSR address for PAT
const IA32_PAT: u32 = 0x277;

/// Memory types for PAT
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MemoryType {
    /// Uncacheable - all accesses go to memory
    Uncacheable = 0x00,
    /// Write-Combining - writes are combined, good for frame buffers
    WriteCombining = 0x01,
    /// Reserved (do not use)
    Reserved2 = 0x02,
    /// Reserved (do not use)
    Reserved3 = 0x03,
    /// Write-Through - reads cached, writes go through
    WriteThrough = 0x04,
    /// Write-Protected - reads cached, writes uncached
    WriteProtected = 0x05,
    /// Write-Back - normal caching (default for RAM)
    WriteBack = 0x06,
    /// Uncacheable Minus - like UC but MTRR can override
    UncacheableMinus = 0x07,
}

impl From<u8> for MemoryType {
    fn from(v: u8) -> Self {
        match v {
            0x00 => MemoryType::Uncacheable,
            0x01 => MemoryType::WriteCombining,
            0x04 => MemoryType::WriteThrough,
            0x05 => MemoryType::WriteProtected,
            0x06 => MemoryType::WriteBack,
            0x07 => MemoryType::UncacheableMinus,
            _ => MemoryType::Uncacheable,
        }
    }
}

/// PAT entry index (0-7)
#[derive(Debug, Clone, Copy)]
pub struct PatIndex(pub u8);

impl PatIndex {
    /// Create new PAT index (must be 0-7)
    pub const fn new(index: u8) -> Self {
        Self(index & 0x07)
    }
    
    /// Get the page table flags needed to select this PAT entry
    /// Returns (PWT, PCD, PAT) bits for page table entry
    pub fn page_flags(&self) -> (bool, bool, bool) {
        let pwt = (self.0 & 0x01) != 0;
        let pcd = (self.0 & 0x02) != 0;
        let pat = (self.0 & 0x04) != 0;
        (pwt, pcd, pat)
    }
}

/// Default PAT configuration
/// This is the standard x86 configuration after reset
pub const DEFAULT_PAT: [MemoryType; 8] = [
    MemoryType::WriteBack,          // PAT0: WB (default for normal pages)
    MemoryType::WriteThrough,       // PAT1: WT
    MemoryType::UncacheableMinus,   // PAT2: UC-
    MemoryType::Uncacheable,        // PAT3: UC
    MemoryType::WriteBack,          // PAT4: WB
    MemoryType::WriteThrough,       // PAT5: WT
    MemoryType::UncacheableMinus,   // PAT6: UC-
    MemoryType::Uncacheable,        // PAT7: UC
];

/// Optimized PAT configuration for OS use
/// Provides easy access to WC for frame buffers
pub const OPTIMIZED_PAT: [MemoryType; 8] = [
    MemoryType::WriteBack,          // PAT0: WB - Normal RAM
    MemoryType::WriteCombining,     // PAT1: WC - Frame buffers
    MemoryType::UncacheableMinus,   // PAT2: UC- - Device memory (MTRR can override)
    MemoryType::Uncacheable,        // PAT3: UC - Strong uncached (MMIO)
    MemoryType::WriteThrough,       // PAT4: WT - Some device memory
    MemoryType::WriteProtected,     // PAT5: WP - Read-only data
    MemoryType::WriteCombining,     // PAT6: WC - Alternative WC index
    MemoryType::Uncacheable,        // PAT7: UC - Strong uncached
];

/// Current PAT configuration
static mut CURRENT_PAT: [MemoryType; 8] = DEFAULT_PAT;

/// Read current PAT MSR value
pub fn read_pat() -> u64 {
    unsafe {
        let (low, high): (u32, u32);
        asm!(
            "rdmsr",
            in("ecx") IA32_PAT,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags)
        );
        ((high as u64) << 32) | (low as u64)
    }
}

/// Write PAT MSR value
unsafe fn write_pat(value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    asm!(
        "wrmsr",
        in("ecx") IA32_PAT,
        in("eax") low,
        in("edx") high,
        options(nomem, nostack, preserves_flags)
    );
}

/// Encode PAT array into MSR value
fn encode_pat(entries: &[MemoryType; 8]) -> u64 {
    let mut value = 0u64;
    for (i, &mtype) in entries.iter().enumerate() {
        value |= (mtype as u64) << (i * 8);
    }
    value
}

/// Decode PAT MSR value into array
fn decode_pat(value: u64) -> [MemoryType; 8] {
    let mut entries = [MemoryType::Uncacheable; 8];
    for i in 0..8 {
        entries[i] = MemoryType::from(((value >> (i * 8)) & 0xFF) as u8);
    }
    entries
}

/// Configure PAT with custom entries
pub fn configure(entries: &[MemoryType; 8]) {
    let value = encode_pat(entries);
    
    unsafe {
        // Disable interrupts while modifying PAT
        asm!("cli", options(nostack, preserves_flags));
        
        // Flush caches
        asm!("wbinvd", options(nostack));
        
        // Write new PAT value
        write_pat(value);
        
        // Update cached configuration
        CURRENT_PAT = *entries;
        
        // Re-enable interrupts
        asm!("sti", options(nostack, preserves_flags));
    }
    
    log::info!("PAT configured: {:?}", entries);
}

/// Initialize PAT with optimized configuration
pub fn init() {
    log::info!("Initializing PAT (Page Attribute Table)...");
    
    // Read current PAT to log it
    let current = read_pat();
    let decoded = decode_pat(current);
    log::debug!("Current PAT: {:?}", decoded);
    
    // Configure optimized PAT
    configure(&OPTIMIZED_PAT);
    
    log::info!("PAT initialized with optimized configuration");
}

/// Get current PAT configuration
pub fn get_config() -> [MemoryType; 8] {
    unsafe { CURRENT_PAT }
}

/// Find PAT index for a memory type
pub fn find_index(mtype: MemoryType) -> Option<PatIndex> {
    let config = get_config();
    for (i, &entry) in config.iter().enumerate() {
        if entry == mtype {
            return Some(PatIndex::new(i as u8));
        }
    }
    None
}

/// Get PAT index for Write-Back (normal RAM)
pub fn index_wb() -> PatIndex {
    find_index(MemoryType::WriteBack).unwrap_or(PatIndex::new(0))
}

/// Get PAT index for Write-Combining (frame buffers)
pub fn index_wc() -> PatIndex {
    find_index(MemoryType::WriteCombining).unwrap_or(PatIndex::new(1))
}

/// Get PAT index for Uncacheable (MMIO)
pub fn index_uc() -> PatIndex {
    find_index(MemoryType::Uncacheable).unwrap_or(PatIndex::new(3))
}

/// Get PAT index for Write-Through
pub fn index_wt() -> PatIndex {
    find_index(MemoryType::WriteThrough).unwrap_or(PatIndex::new(4))
}

/// Calculate page table entry flags for a memory type
/// Returns flags to OR into the page table entry
pub fn pte_flags_for_type(mtype: MemoryType) -> u64 {
    let index = find_index(mtype).unwrap_or(PatIndex::new(0));
    let (pwt, pcd, pat) = index.page_flags();
    
    let mut flags = 0u64;
    if pwt { flags |= 1 << 3; }  // PWT bit
    if pcd { flags |= 1 << 4; }  // PCD bit
    if pat { flags |= 1 << 7; }  // PAT bit (for 4KB pages, bit 12 for large pages)
    
    flags
}

/// Page table entry helpers for common memory types
pub mod pte {
    use super::*;
    
    /// Normal RAM (Write-Back)
    pub fn normal() -> u64 {
        pte_flags_for_type(MemoryType::WriteBack)
    }
    
    /// Device MMIO (Uncacheable)
    pub fn mmio() -> u64 {
        pte_flags_for_type(MemoryType::Uncacheable)
    }
    
    /// Frame buffer (Write-Combining)
    pub fn framebuffer() -> u64 {
        pte_flags_for_type(MemoryType::WriteCombining)
    }
    
    /// ROM/Read-only (Write-Protected)
    pub fn readonly() -> u64 {
        pte_flags_for_type(MemoryType::WriteProtected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_encode_decode() {
        let encoded = encode_pat(&OPTIMIZED_PAT);
        let decoded = decode_pat(encoded);
        assert_eq!(decoded, OPTIMIZED_PAT);
    }
    
    #[test]
    fn test_pat_index() {
        let idx = PatIndex::new(5);
        let (pwt, pcd, pat) = idx.page_flags();
        assert!(pwt);   // bit 0
        assert!(!pcd);  // bit 1
        assert!(pat);   // bit 2
    }
}
