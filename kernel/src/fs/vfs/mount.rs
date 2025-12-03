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
#[derive(Debug, Clone, Copy, Default)]
pub struct MountFlags {
    pub read_only: bool,
    pub no_exec: bool,
    pub no_suid: bool,
    pub no_dev: bool,
    pub synchronous: bool,
    pub no_atime: bool,
    pub bind: bool,           // Bind mount
    pub move_mount: bool,     // Move mount
    pub remount: bool,        // Remount with new flags
}

impl MountFlags {
    pub const fn new() -> Self {
        Self {
            read_only: false,
            no_exec: false,
            no_suid: false,
            no_dev: false,
            synchronous: false,
            no_atime: false,
            bind: false,
            move_mount: false,
            remount: false,
        }
    }

    pub const fn read_only() -> Self {
        Self {
            read_only: true,
            no_exec: false,
            no_suid: false,
            no_dev: false,
            synchronous: false,
            no_atime: false,
            bind: false,
            move_mount: false,
            remount: false,
        }
    }
    
    /// Parse from mount options string (e.g., "ro,noexec,nosuid")
    pub fn from_options(options: &str) -> Self {
        let mut flags = Self::new();
        for opt in options.split(',') {
            match opt.trim() {
                "ro" => flags.read_only = true,
                "rw" => flags.read_only = false,
                "noexec" => flags.no_exec = true,
                "exec" => flags.no_exec = false,
                "nosuid" => flags.no_suid = true,
                "suid" => flags.no_suid = false,
                "nodev" => flags.no_dev = true,
                "dev" => flags.no_dev = false,
                "sync" => flags.synchronous = true,
                "async" => flags.synchronous = false,
                "noatime" => flags.no_atime = true,
                "atime" => flags.no_atime = false,
                "bind" => flags.bind = true,
                "move" => flags.move_mount = true,
                "remount" => flags.remount = true,
                _ => {}
            }
        }
        flags
    }
}

/// Mount propagation type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MountPropagation {
    /// Private mount (default)
    #[default]
    Private,
    /// Shared mount (events propagate to peers)
    Shared,
    /// Slave mount (receives events from master)
    Slave,
    /// Unbindable mount
    Unbindable,
}

/// Unique mount ID
static NEXT_MOUNT_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

/// A mount point in the VFS
pub struct Mount {
    /// Unique mount ID
    pub id: u64,
    /// Mount point path (e.g., "/", "/dev", "/proc")
    pub path: String,
    /// Filesystem type
    pub fs_type: FsType,
    /// Root inode of the mounted filesystem
    pub root: Arc<RwLock<dyn Inode>>,
    /// Mount flags
    pub flags: MountFlags,
    /// Mount propagation
    pub propagation: MountPropagation,
    /// Source path (for bind mounts)
    pub source: Option<String>,
    /// Device name or identifier
    pub device: Option<String>,
}

impl Mount {
    pub fn new(
        path: String,
        fs_type: FsType,
        root: Arc<RwLock<dyn Inode>>,
        flags: MountFlags,
    ) -> Self {
        Self {
            id: NEXT_MOUNT_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
            path,
            fs_type,
            root,
            flags,
            propagation: MountPropagation::Private,
            source: None,
            device: None,
        }
    }
    
    /// Create a bind mount
    pub fn bind(source_path: String, target_path: String, root: Arc<RwLock<dyn Inode>>, flags: MountFlags) -> Self {
        let mut mount = Self::new(target_path, FsType::Tmpfs, root, flags);
        mount.source = Some(source_path);
        mount
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