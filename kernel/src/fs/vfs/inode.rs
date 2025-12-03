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
/// Uses standard POSIX octal representation
#[derive(Debug, Clone, Copy, Default)]
#[repr(transparent)]
pub struct InodePermissions(u16);

impl InodePermissions {
    // Standard POSIX permission bits
    pub const USER_READ: u16 = 0o400;
    pub const USER_WRITE: u16 = 0o200;
    pub const USER_EXEC: u16 = 0o100;
    pub const GROUP_READ: u16 = 0o040;
    pub const GROUP_WRITE: u16 = 0o020;
    pub const GROUP_EXEC: u16 = 0o010;
    pub const OTHER_READ: u16 = 0o004;
    pub const OTHER_WRITE: u16 = 0o002;
    pub const OTHER_EXEC: u16 = 0o001;
    
    // Special bits
    pub const SETUID: u16 = 0o4000;
    pub const SETGID: u16 = 0o2000;
    pub const STICKY: u16 = 0o1000;

    /// Default: rw-r--r-- (0o644)
    #[inline(always)]
    pub const fn new() -> Self {
        Self(Self::USER_READ | Self::USER_WRITE | Self::GROUP_READ | Self::OTHER_READ)
    }
    
    /// From octal mode (e.g., 0o755)
    #[inline(always)]
    pub const fn from_mode(mode: u16) -> Self {
        Self(mode & 0o7777)
    }
    
    /// To octal mode
    #[inline(always)]
    pub const fn to_mode(&self) -> u16 {
        self.0
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
    pub const fn user_exec(&self) -> bool {
        (self.0 & Self::USER_EXEC) != 0
    }
    
    #[inline(always)]
    pub const fn group_read(&self) -> bool {
        (self.0 & Self::GROUP_READ) != 0
    }
    
    #[inline(always)]
    pub const fn group_write(&self) -> bool {
        (self.0 & Self::GROUP_WRITE) != 0
    }
    
    #[inline(always)]
    pub const fn group_exec(&self) -> bool {
        (self.0 & Self::GROUP_EXEC) != 0
    }
    
    #[inline(always)]
    pub const fn other_read(&self) -> bool {
        (self.0 & Self::OTHER_READ) != 0
    }
    
    #[inline(always)]
    pub const fn other_write(&self) -> bool {
        (self.0 & Self::OTHER_WRITE) != 0
    }
    
    #[inline(always)]
    pub const fn other_exec(&self) -> bool {
        (self.0 & Self::OTHER_EXEC) != 0
    }
    
    #[inline(always)]
    pub const fn is_setuid(&self) -> bool {
        (self.0 & Self::SETUID) != 0
    }
    
    #[inline(always)]
    pub const fn is_setgid(&self) -> bool {
        (self.0 & Self::SETGID) != 0
    }
    
    #[inline(always)]
    pub const fn is_sticky(&self) -> bool {
        (self.0 & Self::STICKY) != 0
    }

    #[inline(always)]
    pub fn set_permissions(&mut self, perms: u16) {
        self.0 = perms & 0o7777;
    }
}

/// Timestamp for inode (nanoseconds since epoch)
#[derive(Debug, Clone, Copy, Default)]
pub struct Timestamp {
    pub secs: i64,
    pub nsecs: u32,
}

impl Timestamp {
    pub const fn new(secs: i64, nsecs: u32) -> Self {
        Self { secs, nsecs }
    }
    
    pub fn now() -> Self {
        // TODO: Get actual time from RTC/HPET
        Self { secs: 0, nsecs: 0 }
    }
}

/// Inode metadata (for stat syscall)
#[derive(Debug, Clone, Copy)]
pub struct InodeStat {
    pub ino: u64,
    pub mode: u16,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub blksize: u32,
    pub blocks: u64,
    pub atime: Timestamp,
    pub mtime: Timestamp,
    pub ctime: Timestamp,
    pub inode_type: InodeType,
}

/// VFS Inode trait
///
/// High-performance trait with full POSIX semantics.
/// Supports timestamps, ownership, extended operations.
/// Inline hints applied in implementations.
pub trait Inode: Send + Sync {
    // ========== Basic Attributes ==========
    
    /// Gets the inode number.
    fn ino(&self) -> u64;

    /// Gets the inode type.
    fn inode_type(&self) -> InodeType;

    /// Gets file size in bytes.
    fn size(&self) -> u64;

    /// Gets inode permissions.
    fn permissions(&self) -> InodePermissions;
    
    /// Sets inode permissions (chmod)
    fn set_permissions(&mut self, _perms: InodePermissions) -> FsResult<()> {
        Err(FsError::NotSupported) // Default impl
    }
    
    // ========== Ownership (UID/GID) ==========
    
    /// Gets owner user ID
    fn uid(&self) -> u32 { 0 } // Default: root
    
    /// Gets owner group ID
    fn gid(&self) -> u32 { 0 } // Default: root
    
    /// Sets owner (chown)
    fn set_owner(&mut self, _uid: u32, _gid: u32) -> FsResult<()> {
        Err(FsError::NotSupported)
    }
    
    // ========== Link Count ==========
    
    /// Gets hard link count
    fn nlink(&self) -> u32 { 1 }
    
    /// Increments link count (for hard links)
    fn inc_nlink(&mut self) -> FsResult<()> {
        Err(FsError::NotSupported)
    }
    
    /// Decrements link count
    fn dec_nlink(&mut self) -> FsResult<()> {
        Err(FsError::NotSupported)
    }
    
    // ========== Timestamps ==========
    
    /// Gets access time
    fn atime(&self) -> Timestamp { Timestamp::default() }
    
    /// Gets modification time
    fn mtime(&self) -> Timestamp { Timestamp::default() }
    
    /// Gets status change time
    fn ctime(&self) -> Timestamp { Timestamp::default() }
    
    /// Updates timestamps (utimes/utimensat)
    fn set_times(&mut self, _atime: Option<Timestamp>, _mtime: Option<Timestamp>) -> FsResult<()> {
        Err(FsError::NotSupported)
    }
    
    // ========== Full stat ==========
    
    /// Gets full inode statistics
    fn stat(&self) -> InodeStat {
        InodeStat {
            ino: self.ino(),
            mode: self.permissions().to_mode() | ((self.inode_type() as u16) << 12),
            nlink: self.nlink(),
            uid: self.uid(),
            gid: self.gid(),
            size: self.size(),
            blksize: 4096,
            blocks: (self.size() + 511) / 512,
            atime: self.atime(),
            mtime: self.mtime(),
            ctime: self.ctime(),
            inode_type: self.inode_type(),
        }
    }

    // ========== Data Operations ==========

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
    
    /// Sync data to storage (fsync)
    fn sync(&mut self) -> FsResult<()> {
        Ok(()) // No-op for tmpfs
    }
    
    /// Sync data only, not metadata (fdatasync)
    fn datasync(&mut self) -> FsResult<()> {
        self.sync()
    }

    // ========== Directory Operations ==========

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
    
    /// Links an existing inode into this directory (hard link)
    fn link(&mut self, _name: &str, _ino: u64) -> FsResult<()> {
        Err(FsError::NotSupported)
    }
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
