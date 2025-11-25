use super::inode::Inode;
use crate::fs::{FsError, FsResult};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use hashbrown::HashMap;
use spin::RwLock;

// VFS Mount point management.
//
// Handles filesystem mounting and mount point resolution with:
// - Efficient mount point lookup
// - Support for multiple filesystem types
// - Path-based mount resolution

/// Filesystem type identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FsType {
    Tmpfs,
    Ext2,
    Fat32,
    Procfs,
    Devfs,
}

/// Mount flags
#[derive(Debug, Clone, Copy)]
pub struct MountFlags {
    pub read_only: bool,
    pub no_exec: bool,
    pub no_suid: bool,
}

impl MountFlags {
    pub const fn new() -> Self {
        Self {
            read_only: false,
            no_exec: false,
            no_suid: false,
        }
    }

    pub const fn read_only() -> Self {
        Self {
            read_only: true,
            no_exec: false,
            no_suid: false,
        }
    }
}

/// A mount point in the VFS
pub struct Mount {
    /// Mount point path (e.g., "/", "/dev", "/proc")
    pub path: String,
    /// Filesystem type
    pub fs_type: FsType,
    /// Root inode of the mounted filesystem
    pub root: Arc<RwLock<dyn Inode>>,
    /// Mount flags
    pub flags: MountFlags,
}

impl Mount {
    pub fn new(
        path: String,
        fs_type: FsType,
        root: Arc<RwLock<dyn Inode>>,
        flags: MountFlags,
    ) -> Self {
        Self {
            path,
            fs_type,
            root,
            flags,
        }
    }
}

/// Mount table managing all mount points
pub struct MountTable {
    /// All active mounts, sorted by path length (longest first for proper resolution)
    mounts: RwLock<Vec<Mount>>,
}

impl MountTable {
    pub const fn new() -> Self {
        Self {
            mounts: RwLock::new(Vec::new()),
        }
    }

    /// Mount a filesystem at the given path
    pub fn mount(
        &self,
        path: String,
        fs_type: FsType,
        root: Arc<RwLock<dyn Inode>>,
        flags: MountFlags,
    ) -> FsResult<()> {
        let mut mounts = self.mounts.write();

        // Check if already mounted
        if mounts.iter().any(|m| m.path == path) {
            return Err(FsError::AlreadyExists);
        }

        // Add mount
        mounts.push(Mount::new(path, fs_type, root, flags));

        // Sort by path length (descending) for proper mount point resolution
        mounts.sort_by(|a, b| b.path.len().cmp(&a.path.len()));

        Ok(())
    }

    /// Unmount a filesystem at the given path
    pub fn unmount(&self, path: &str) -> FsResult<()> {
        let mut mounts = self.mounts.write();

        let index = mounts
            .iter()
            .position(|m| m.path == path)
            .ok_or(FsError::NotFound)?;

        mounts.remove(index);
        Ok(())
    }

    /// Find the mount point for a given path
    ///
    /// Returns the mount and the remaining path relative to the mount point
    pub fn resolve_mount(&self, path: &str) -> FsResult<(Arc<RwLock<dyn Inode>>, String)> {
        let mounts = self.mounts.read();

        // Find the longest matching mount point
        for mount in mounts.iter() {
            if path.starts_with(&mount.path) {
                let remaining = if mount.path == "/" {
                    path.to_string()
                } else {
                    path.strip_prefix(&mount.path)
                        .unwrap_or("")
                        .to_string()
                };

                return Ok((Arc::clone(&mount.root), remaining));
            }
        }

        Err(FsError::NotFound)
    }

    /// Get all mount points
    pub fn list_mounts(&self) -> Vec<String> {
        let mounts = self.mounts.read();
        mounts.iter().map(|m| m.path.clone()).collect()
    }

    /// Check if a path is a mount point
    pub fn is_mount_point(&self, path: &str) -> bool {
        let mounts = self.mounts.read();
        mounts.iter().any(|m| m.path == path)
    }
}

lazy_static::lazy_static! {
    /// Global mount table
    pub static ref MOUNT_TABLE: MountTable = MountTable::new();
}

/// Initialize the mount system with a root filesystem
pub fn init_root(root_inode: Arc<RwLock<dyn Inode>>) -> FsResult<()> {
    MOUNT_TABLE.mount(
        String::from("/"),
        FsType::Tmpfs,
        root_inode,
        MountFlags::new(),
    )?;
    log::debug!("Root filesystem mounted at /");
    Ok(())
}