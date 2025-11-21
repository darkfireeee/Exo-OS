//! Virtual File System layer

use super::{FsResult, FsError};

/// VFS inode
pub mod inode;

/// Directory entries
pub mod dentry;

/// Mount points
pub mod mount;

/// VFS cache
pub mod cache;

/// Initialize VFS
pub fn init() -> FsResult<()> {
    log::debug!("VFS initialized");
    Ok(())
}
