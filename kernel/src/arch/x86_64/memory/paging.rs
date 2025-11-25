//! 4-level paging support

pub const PAGE_SIZE: usize = 4096;
pub const ENTRIES_PER_TABLE: usize = 512;

/// Page table entry
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    pub fn new() -> Self {
        Self(0)
    }
    
    pub fn is_present(&self) -> bool {
        self.0 & 1 != 0
    }
    
    pub fn set_present(&mut self, present: bool) {
        if present {
            self.0 |= 1;
        } else {
            self.0 &= !1;
        }
    }
    
    pub fn address(&self) -> u64 {
        self.0 & 0x000F_FFFF_FFFF_F000
    }
    
    pub fn set_address(&mut self, addr: u64) {
        self.0 = (self.0 & !0x000F_FFFF_FFFF_F000) | (addr & 0x000F_FFFF_FFFF_F000);
    }
}

/// Page table (512 entries)
#[repr(align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; ENTRIES_PER_TABLE],
}

impl PageTable {
    pub fn new() -> Self {
        Self {
            entries: [PageTableEntry::new(); ENTRIES_PER_TABLE],
        }
    }
}
