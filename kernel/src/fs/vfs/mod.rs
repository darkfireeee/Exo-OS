//! Virtual File System layer

use super::{FsError, FsResult};

/// VFS inode
pub mod inode;

/// Directory entries
pub mod dentry;

/// Mount points
pub mod mount;

/// VFS cache
pub mod cache;

/// tmpfs - Temporary RAM filesystem
pub mod tmpfs;

/// Initialize VFS
pub fn init() -> FsResult<()> {
    cache::init();
    log::info!("VFS initialized with cache and tmpfs");
    Ok(())
}
