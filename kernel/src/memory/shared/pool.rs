//! Shared memory pool for efficient allocation

use super::descriptor::{SharedMemoryDescriptor, ShmId};
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Global shared memory pool
pub struct SharedMemoryPool {
    /// Descriptors indexed by ID
    descriptors: BTreeMap<ShmId, SharedMemoryDescriptor>,
    /// Next available ID
    next_id: u64,
}

impl SharedMemoryPool {
    pub fn new() -> Self {
        Self {
            descriptors: BTreeMap::new(),
            next_id: 1,
        }
    }
    
    /// Allocate new ID
    pub fn alloc_id(&mut self) -> ShmId {
        let id = ShmId(self.next_id);
        self.next_id += 1;
        id
    }
    
    /// Add descriptor to pool
    pub fn add(&mut self, desc: SharedMemoryDescriptor) {
        self.descriptors.insert(desc.id, desc);
    }
    
    /// Remove descriptor from pool
    pub fn remove(&mut self, id: ShmId) -> Option<SharedMemoryDescriptor> {
        self.descriptors.remove(&id)
    }
    
    /// Get descriptor
    pub fn get(&self, id: ShmId) -> Option<&SharedMemoryDescriptor> {
        self.descriptors.get(&id)
    }
    
    /// Get mutable descriptor
    pub fn get_mut(&mut self, id: ShmId) -> Option<&mut SharedMemoryDescriptor> {
        self.descriptors.get_mut(&id)
    }
    
    /// Find by name
    pub fn find_by_name(&self, name: &str) -> Option<&SharedMemoryDescriptor> {
        self.descriptors.values().find(|desc| {
            desc.name.as_ref().map(|n| n.as_str()) == Some(name)
        })
    }
    
    /// Clean up orphan descriptors
    pub fn cleanup_orphans(&mut self) {
        self.descriptors.retain(|_, desc| !desc.is_orphan());
    }
}

impl Default for SharedMemoryPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Global pool instance
static GLOBAL_POOL: Mutex<Option<SharedMemoryPool>> = Mutex::new(None);

/// Initialize shared memory pool
pub fn init() {
    let mut pool = GLOBAL_POOL.lock();
    *pool = Some(SharedMemoryPool::new());
}

/// Get access to global pool
pub fn with_pool<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut SharedMemoryPool) -> R,
{
    let mut pool = GLOBAL_POOL.lock();
    pool.as_mut().map(f)
}
