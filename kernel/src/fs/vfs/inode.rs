//! VFS Inode (Index Node) representation.
//!
//! High-performance inode implementation with:
//! - Cache-aligned structures (64 bytes)
//! - Zero-copy operations where possible
//! - Inline hints for hot paths

use crate::fs::{FileMetadata, FsError, FsResult};
use alloc::string::String;
use alloc::vec::Vec;

/// Inode types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum InodeType {
    File = 0,
    Directory = 1,
    Symlink = 2,
    CharDevice = 3,
    BlockDevice = 4,
    Fifo = 5,
    Socket = 6,
}

/// Inode permissions (packed into u16 for efficiency)
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct InodePermissions(u16);

impl InodePermissions {
    const USER_READ: u16 = 1 << 0;
    const USER_WRITE: u16 = 1 << 1;
    const USER_EXEC: u16 = 1 << 2;
    const GROUP_READ: u16 = 1 << 3;
    const GROUP_WRITE: u16 = 1 << 4;
    const GROUP_EXEC: u16 = 1 << 5;
    const OTHER_READ: u16 = 1 << 6;
    const OTHER_WRITE: u16 = 1 << 7;
    const OTHER_EXEC: u16 = 1 << 8;

    #[inline(always)]
    pub const fn new() -> Self {
        Self(Self::USER_READ | Self::USER_WRITE | Self::GROUP_READ | Self::OTHER_READ)
    }

    #[inline(always)]
    pub const fn user_read(&self) -> bool {
        (self.0 & Self::USER_READ) != 0
    }

    #[inline(always)]
    pub const fn user_write(&self) -> bool {
        (self.0 & Self::USER_WRITE) != 0
    }

    #[inline(always)]
    pub const fn set_permissions(&mut self, perms: u16) {
        self.0 = perms;
    }
}

/// VFS Inode trait
///
/// High-performance trait. Inline hints applied in implementations.
pub trait Inode: Send + Sync {
    /// Gets the inode number.
    fn ino(&self) -> u64;

    /// Gets the inode type.
    fn inode_type(&self) -> InodeType;

    /// Gets file size in bytes.
    fn size(&self) -> u64;

    /// Gets inode permissions.
    fn permissions(&self) -> InodePermissions;

    /// Reads data from the inode (zero-copy where possible).
    ///
    /// # Performance
    /// Target: < 200 cycles for small reads (cache hit)
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize>;

    /// Writes data to the inode (zero-copy where possible).
    ///
    /// # Performance
    /// Target: < 300 cycles for small writes (cache hit)
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> FsResult<usize>;

    /// Truncates the file to the given size.
    fn truncate(&mut self, size: u64) -> FsResult<()>;

    /// Lists directory entries (only for directories).
    fn list(&self) -> FsResult<Vec<String>>;

    /// Looks up a child entry (only for directories).
    ///
    /// # Performance
    /// Target: < 100 cycles for hash lookup
    fn lookup(&self, name: &str) -> FsResult<u64>;

    /// Creates a new child entry (only for directories).
    fn create(&mut self, name: &str, inode_type: InodeType) -> FsResult<u64>;

    /// Removes a child entry (only for directories).
    fn remove(&mut self, name: &str) -> FsResult<()>;
}

/// Gets file metadata from an inode.
///
/// # Performance
/// Inline for zero-cost abstraction.
#[inline(always)]
pub fn inode_to_metadata(inode: &dyn Inode) -> FileMetadata {
    FileMetadata {
        size: inode.size(),
        is_dir: inode.inode_type() == InodeType::Directory,
        read_only: !inode.permissions().user_write(),
    }
}
