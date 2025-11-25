//! Shared Memory Pool Management
//!
//! Global pool for shared memory regions

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::memory::{MemoryResult, MemoryError};
use crate::memory::address::PhysicalAddress;

/// Shared memory ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShmId(pub u64);

/// Shared memory permissions
#[derive(Debug, Clone, Copy)]
pub struct ShmPermissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl ShmPermissions {
    pub const READ_ONLY: Self = Self { read: true, write: false, execute: false };
    pub const READ_WRITE: Self = Self { read: true, write: true, execute: false };
    pub const READ_EXEC: Self = Self { read: true, write: false, execute: true };
}

/// Shared memory region descriptor
pub struct ShmRegion {
    /// Unique ID
    pub id: ShmId,
    
    /// Physical address
    pub phys_addr: PhysicalAddress,
    
    /// Size in bytes
    pub size: usize,
    
    /// Permissions
    pub perms: ShmPermissions,
    
    /// Owner process ID
    pub owner_pid: usize,
    
    /// Reference count
    pub ref_count: usize,
    
    /// Optional name
    pub name: Option<String>,
}

impl ShmRegion {
    pub fn new(id: ShmId, phys_addr: PhysicalAddress, size: usize, perms: ShmPermissions, owner: usize) -> Self {
        Self {
            id,
            phys_addr,
            size,
            perms,
            owner_pid: owner,
            ref_count: 1,
            name: None,
        }
    }
    
    pub fn with_name(mut self, name: String) -> Self {
        self.name = Some(name);
        self
    }
    
    pub fn inc_ref(&mut self) {
        self.ref_count += 1;
    }
    
    pub fn dec_ref(&mut self) -> bool {
        self.ref_count = self.ref_count.saturating_sub(1);
        self.ref_count == 0
    }
}

/// Global shared memory pool
pub struct SharedMemoryPool {
    /// All regions indexed by ID
    regions: BTreeMap<ShmId, ShmRegion>,
    
    /// Named regions
    named: BTreeMap<String, ShmId>,
    
    /// Next available ID
    next_id: u64,
}

impl SharedMemoryPool {
    pub const fn new() -> Self {
        Self {
            regions: BTreeMap::new(),
            named: BTreeMap::new(),
            next_id: 1,
        }
    }
    
    /// Allocate new shared memory region
    pub fn allocate(&mut self, size: usize, perms: ShmPermissions, owner: usize) -> MemoryResult<ShmId> {
        // Allocate physical memory
        let phys_addr = self.alloc_physical(size)?;
        
        let id = ShmId(self.next_id);
        self.next_id += 1;
        
        let region = ShmRegion::new(id, phys_addr, size, perms, owner);
        self.regions.insert(id, region);
        
        Ok(id)
    }
    
    /// Create named shared memory
    pub fn create_named(&mut self, name: String, size: usize, perms: ShmPermissions, owner: usize) -> MemoryResult<ShmId> {
        if self.named.contains_key(&name) {
            return Err(MemoryError::AlreadyMapped);
        }
        
        let id = self.allocate(size, perms, owner)?;
        
        if let Some(region) = self.regions.get_mut(&id) {
            region.name = Some(name.clone());
        }
        
        self.named.insert(name, id);
        Ok(id)
    }
    
    /// Open named shared memory
    pub fn open_named(&mut self, name: &str) -> MemoryResult<ShmId> {
        self.named.get(name)
            .copied()
            .ok_or(MemoryError::NotFound)
    }
    
    /// Get region by ID
    pub fn get(&self, id: ShmId) -> Option<&ShmRegion> {
        self.regions.get(&id)
    }
    
    /// Get mutable region
    pub fn get_mut(&mut self, id: ShmId) -> Option<&mut ShmRegion> {
        self.regions.get_mut(&id)
    }
    
    /// Attach to shared memory (increment ref count)
    pub fn attach(&mut self, id: ShmId) -> MemoryResult<PhysicalAddress> {
        let region = self.regions.get_mut(&id)
            .ok_or(MemoryError::NotFound)?;
        
        region.inc_ref();
        Ok(region.phys_addr)
    }
    
    /// Detach from shared memory (decrement ref count)
    pub fn detach(&mut self, id: ShmId) -> MemoryResult<bool> {
        let should_free = {
            let region = self.regions.get_mut(&id)
                .ok_or(MemoryError::NotFound)?;
            region.dec_ref()
        };
        
        if should_free {
            if let Some(region) = self.regions.remove(&id) {
                if let Some(name) = &region.name {
                    self.named.remove(name);
                }
                
                // TODO: Free physical memory when deallocator API complete
                let frame_count = (region.size + 4095) / 4096;
                log::debug!("Would free {} frame(s) from shared memory", frame_count);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }
    
    /// Allocate physical memory for shared region
    fn alloc_physical(&self, size: usize) -> MemoryResult<PhysicalAddress> {
        // TODO: Use proper physical allocator when API complete
        // Calculate number of frames needed
        let frame_count = (size + 4095) / 4096;
        
        // For now, return dummy address (will be replaced with real allocation)
        log::debug!("Would allocate {} frame(s) for shared memory", frame_count);
        
        // Dummy address in high memory
        Ok(PhysicalAddress::new(0x1000_0000))
    }
}

/// Global pool instance
static GLOBAL_POOL: Mutex<SharedMemoryPool> = Mutex::new(SharedMemoryPool::new());

/// Initialize shared memory subsystem
pub fn init() {
    log::debug!("Shared memory pool initialized");
}

/// Allocate shared memory
pub fn allocate(size: usize, perms: ShmPermissions, owner: usize) -> MemoryResult<ShmId> {
    GLOBAL_POOL.lock().allocate(size, perms, owner)
}

/// Create named shared memory
pub fn create_named(name: String, size: usize, perms: ShmPermissions, owner: usize) -> MemoryResult<ShmId> {
    GLOBAL_POOL.lock().create_named(name, size, perms, owner)
}

/// Open named shared memory
pub fn open_named(name: &str) -> MemoryResult<ShmId> {
    GLOBAL_POOL.lock().open_named(name)
}

/// Attach to shared memory
pub fn attach(id: ShmId) -> MemoryResult<PhysicalAddress> {
    GLOBAL_POOL.lock().attach(id)
}

/// Detach from shared memory
pub fn detach(id: ShmId) -> MemoryResult<bool> {
    GLOBAL_POOL.lock().detach(id)
}
