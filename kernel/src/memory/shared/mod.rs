//! Shared memory management

pub mod descriptor;
pub mod permissions;
pub mod pool;

pub use descriptor::{SharedMemoryDescriptor, ShmId};
pub use permissions::ShmPermissions;
pub use pool::SharedMemoryPool;
