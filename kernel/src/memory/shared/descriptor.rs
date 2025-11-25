//! Shared memory descriptor

use crate::memory::{VirtualAddress, PhysicalAddress, PageProtection};
use alloc::vec::Vec;
use alloc::string::String;

/// Shared memory ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShmId(pub u64);

/// Shared memory descriptor
#[derive(Debug)]
pub struct SharedMemoryDescriptor {
    /// Unique ID
    pub id: ShmId,
    /// Name (optional)
    pub name: Option<String>,
    /// Virtual address
    pub virt_addr: VirtualAddress,
    /// Physical frames
    pub frames: Vec<PhysicalAddress>,
    /// Size in bytes
    pub size: usize,
    /// Protection
    pub protection: PageProtection,
    /// Reference count
    pub ref_count: usize,
    /// Owner process ID
    pub owner: u64,
}

impl SharedMemoryDescriptor {
    pub fn new(
        id: ShmId,
        name: Option<String>,
        virt_addr: VirtualAddress,
        frames: Vec<PhysicalAddress>,
        size: usize,
        protection: PageProtection,
        owner: u64,
    ) -> Self {
        Self {
            id,
            name,
            virt_addr,
            frames,
            size,
            protection,
            ref_count: 0,
            owner,
        }
    }
    
    pub fn attach(&mut self) {
        self.ref_count += 1;
    }
    
    pub fn detach(&mut self) {
        if self.ref_count > 0 {
            self.ref_count -= 1;
        }
    }
    
    pub fn is_orphan(&self) -> bool {
        self.ref_count == 0
    }
}
