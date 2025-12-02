//! POSIX VFS Adapter Layer
//!
//! ⚠️ **This is NOT a VFS implementation!**
//!
//! This module is an **adapter/wrapper** that bridges POSIX syscalls to the kernel's VFS.
//!
//! ## Architecture
//! ```
//! POSIX Syscalls (open/read/write)
//!        ↓
//!   vfs_posix (this module) ← Adapter layer
//!        ↓
//!   kernel/src/fs/vfs/ ← Real VFS implementation
//! ```
//!
//! ## Responsibilities
//! - Convert POSIX file descriptors → VFS handles
//! - Parse POSIX flags (O_RDONLY, O_CREAT, etc.)
//! - Maintain per-file offsets
//! - Resolve string paths → inodes
//! - Provide high-level file operations
//!
//! ## NOT Responsible For
//! - Inode implementation (done by kernel VFS)
//! - Filesystem drivers (tmpfs, ext4, etc.)
//! - Block device I/O
//! - Page cache management

pub mod file_ops;
pub mod inode_cache;
pub mod path_resolver;
// pub mod pipe; // Moved to kernel_interface
pub use crate::posix_x::kernel_interface::ipc_bridge as pipe;
// pub mod fd_manager; // Moved to kernel_interface
pub use crate::posix_x::core::fd_table as fd_manager;

use crate::fs::vfs::inode::{Inode, InodeType};
use crate::fs::{FsError, FsResult};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

/// VFS Handle for POSIX operations
pub struct VfsHandle {
    /// Inode reference
    inode: Arc<RwLock<dyn Inode>>,

    /// Current file offset
    offset: u64,

    /// Open flags
    flags: OpenFlags,

    /// Path (for debugging)
    path: String,
}

/// Open flags compatible with POSIX
#[derive(Debug, Clone, Copy)]
pub struct OpenFlags {
    pub read: bool,
    pub write: bool,
    pub append: bool,
    pub create: bool,
    pub truncate: bool,
    pub excl: bool,
    pub nonblock: bool,
    pub cloexec: bool,
}

impl OpenFlags {
    /// Parse from POSIX O_* flags
    pub fn from_posix(flags: i32) -> Self {
        const O_RDONLY: i32 = 0x0000;
        const O_WRONLY: i32 = 0x0001;
        const O_RDWR: i32 = 0x0002;
        const O_APPEND: i32 = 0x0400;
        const O_CREAT: i32 = 0x0040;
        const O_TRUNC: i32 = 0x0200;
        const O_EXCL: i32 = 0x0080;
        const O_NONBLOCK: i32 = 0x0800;
        const O_CLOEXEC: i32 = 0x80000;

        let access = flags & 0x03;
        Self {
            read: access == O_RDONLY || access == O_RDWR,
            write: access == O_WRONLY || access == O_RDWR,
            append: (flags & O_APPEND) != 0,
            create: (flags & O_CREAT) != 0,
            truncate: (flags & O_TRUNC) != 0,
            excl: (flags & O_EXCL) != 0,
            nonblock: (flags & O_NONBLOCK) != 0,
            cloexec: (flags & O_CLOEXEC) != 0,
        }
    }

    /// Convert to POSIX flags
    pub fn to_posix(&self) -> i32 {
        let mut flags = 0;

        if self.read && self.write {
            flags |= 0x0002; // O_RDWR
        } else if self.write {
            flags |= 0x0001; // O_WRONLY
        }

        if self.append {
            flags |= 0x0400;
        }
        if self.create {
            flags |= 0x0040;
        }
        if self.truncate {
            flags |= 0x0200;
        }
        if self.excl {
            flags |= 0x0080;
        }
        if self.nonblock {
            flags |= 0x0800;
        }
        if self.cloexec {
            flags |= 0x80000;
        }

        flags
    }
}

impl VfsHandle {
    /// Create new VFS handle
    pub fn new(inode: Arc<RwLock<dyn Inode>>, flags: OpenFlags, path: String) -> Self {
        let offset = if flags.append { inode.read().size() } else { 0 };

        Self {
            inode,
            offset,
            flags,
            path,
        }
    }

    /// Read from file at current offset
    #[inline]
    pub fn read(&mut self, buf: &mut [u8]) -> FsResult<usize> {
        if !self.flags.read {
            return Err(FsError::PermissionDenied);
        }

        let inode = self.inode.read();
        let n = inode.read_at(self.offset, buf)?;
        self.offset += n as u64;
        Ok(n)
    }

    /// Write to file at current offset
    #[inline]
    pub fn write(&mut self, buf: &[u8]) -> FsResult<usize> {
        if !self.flags.write {
            return Err(FsError::PermissionDenied);
        }

        let mut inode = self.inode.write();

        // Handle append mode
        if self.flags.append {
            self.offset = inode.size();
        }

        let n = inode.write_at(self.offset, buf)?;
        self.offset += n as u64;
        Ok(n)
    }

    /// Seek to new offset
    pub fn seek(&mut self, whence: SeekWhence, offset: i64) -> FsResult<u64> {
        let new_offset = match whence {
            SeekWhence::Set => {
                if offset < 0 {
                    return Err(FsError::InvalidArgument);
                }
                offset as u64
            }
            SeekWhence::Cur => {
                let result = (self.offset as i64).checked_add(offset);
                match result {
                    Some(o) if o >= 0 => o as u64,
                    _ => return Err(FsError::InvalidArgument),
                }
            }
            SeekWhence::End => {
                let size = self.inode.read().size() as i64;
                let result = size.checked_add(offset);
                match result {
                    Some(o) if o >= 0 => o as u64,
                    _ => return Err(FsError::InvalidArgument),
                }
            }
        };

        self.offset = new_offset;
        Ok(new_offset)
    }

    /// Get file metadata
    pub fn stat(&self) -> FsResult<FileStat> {
        let inode = self.inode.read();
        Ok(FileStat {
            ino: inode.ino(),
            size: inode.size(),
            inode_type: inode.inode_type(),
            permissions: inode.permissions(),
        })
    }

    /// Truncate file
    pub fn truncate(&mut self, size: u64) -> FsResult<()> {
        if !self.flags.write {
            return Err(FsError::PermissionDenied);
        }
        self.inode.write().truncate(size)
    }

    /// Get inode reference (for advanced operations)
    pub fn inode(&self) -> Arc<RwLock<dyn Inode>> {
        Arc::clone(&self.inode)
    }

    /// Get current offset
    #[inline(always)]
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Get flags
    #[inline(always)]
    pub fn flags(&self) -> OpenFlags {
        self.flags
    }

    /// Get path
    pub fn path(&self) -> &str {
        &self.path
    }
}

/// Seek whence values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SeekWhence {
    Set = 0, // SEEK_SET
    Cur = 1, // SEEK_CUR
    End = 2, // SEEK_END
}

impl SeekWhence {
    pub fn from_i32(whence: i32) -> Option<Self> {
        match whence {
            0 => Some(Self::Set),
            1 => Some(Self::Cur),
            2 => Some(Self::End),
            _ => None,
        }
    }
}

/// File statistics (compatible with POSIX stat)
#[derive(Debug, Clone)]
pub struct FileStat {
    pub ino: u64,
    pub size: u64,
    pub inode_type: InodeType,
    pub permissions: crate::fs::vfs::inode::InodePermissions,
}

impl FileStat {
    /// Convert to POSIX stat structure
    pub fn to_posix_stat(&self) -> PosixStat {
        let mode = match self.inode_type {
            InodeType::File => 0o100000,
            InodeType::Directory => 0o040000,
            InodeType::Symlink => 0o120000,
            InodeType::CharDevice => 0o020000,
            InodeType::BlockDevice => 0o060000,
            InodeType::Fifo => 0o010000,
            InodeType::Socket => 0o140000,
        };

        PosixStat {
            st_dev: 0,
            st_ino: self.ino,
            st_mode: mode | 0o644, // TODO: use real permissions
            st_nlink: 1,
            st_uid: 0,
            st_gid: 0,
            st_rdev: 0,
            st_size: self.size as i64,
            st_blksize: 4096,
            st_blocks: ((self.size + 511) / 512) as i64,
            st_atime: 0,
            st_mtime: 0,
            st_ctime: 0,
        }
    }
}

/// POSIX stat structure (compatible with libc)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PosixStat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_mtime: i64,
    pub st_ctime: i64,
}

/// Initialize VFS integration
pub fn init() {
    log::info!("[POSIX-VFS] Initializing VFS integration layer");
    path_resolver::init();
    inode_cache::init();
    log::info!("[POSIX-VFS] VFS integration ready");
}
