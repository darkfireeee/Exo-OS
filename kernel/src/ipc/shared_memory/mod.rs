//! Shared Memory Module
//!
//! Zero-copy shared memory for IPC

pub mod pool;
pub mod page;
pub mod mapping;

pub use pool::{ShmId, ShmPermissions, ShmRegion, SharedMemoryPool};
pub use page::{SharedPage, PageFlags, PAGE_SIZE};
pub use mapping::{SharedMapping, MappingFlags, map_shared, unmap_shared};

/// Initialize shared memory subsystem
pub fn init() {
    pool::init();
    log::info!("Shared memory subsystem initialized");
}
