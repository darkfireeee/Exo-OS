//! Page Attribute Table for cache control

/// PAT memory types
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum PatMemoryType {
    Uncacheable = 0,
    WriteCombining = 1,
    WriteThrough = 4,
    WriteProtected = 5,
    WriteBack = 6,
    UncacheableMinus = 7,
}

/// Configure PAT MSR
pub fn init() {
    // TODO: Configure PAT MSR (0x277)
    // Default: 00=WB, 01=WT, 02=UC-, 03=UC, 04=WB, 05=WT, 06=UC-, 07=UC
}
