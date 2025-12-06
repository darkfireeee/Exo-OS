//! Filesystem subsystem for hybrid architecture
//!
//! Organized structure:
//! - `vfs/`: Virtual File System core
//! - `real_fs/`: Real filesystems (FAT32, ext4)
//! - `pseudo_fs/`: Pseudo filesystems (devfs, procfs, sysfs, tmpfs)
//! - `ipc_fs/`: IPC filesystems (pipes, sockets, symlinks)
//! - `operations/`: Core operations (buffer, locks, fdtable, cache)
//! - `advanced/`: Advanced features (io_uring, zero-copy, aio, mmap, quota, namespace, acl, notify)

use crate::memory::MemoryError;
use alloc::string::String;
use alloc::vec::Vec;

/// Virtual File System core
pub mod vfs;

/// Real filesystems (FAT32, ext4)
pub mod real_fs;

/// Pseudo filesystems (devfs, procfs, sysfs, tmpfs)
pub mod pseudo_fs;

/// IPC filesystems (pipes, sockets, symlinks)
pub mod ipc_fs;

/// Core operations (buffer, locks, fdtable, cache)
pub mod operations;

/// Advanced features (io_uring, zero-copy, aio, mmap, quota, namespace, acl, notify)
pub mod advanced;

/// Page cache for filesystem I/O
pub mod page_cache;

/// Core filesystem types and traits
pub mod core;

/// Descriptor management
pub mod descriptor;

/// Filesystem initialization
pub fn init() {
    log::info!("Filesystem subsystem initialized (24 modules)");
    log::info!("  Real FS: FAT32, ext4");
    log::info!("  Pseudo FS: devfs, procfs, sysfs, tmpfs");
    log::info!("  IPC FS: pipes, sockets, symlinks");
    log::info!("  Advanced: io_uring, zero-copy, AIO, mmap, quota, ACL, inotify");
}

/// Filesystem errors
#[derive(Debug)]
pub enum FsError {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    FileExists,
    NotDirectory,
    IsDirectory,
    DirectoryNotEmpty,
    InvalidPath,
    InvalidArgument,
    TooManySymlinks,
    InvalidData,
    IoError,
    NotSupported,
    TooManyFiles,
    TooManyOpenFiles,
    InvalidFd,
    ConnectionRefused,
    Again,
    QuotaExceeded,
    Memory(MemoryError),
}

impl FsError {
    pub fn to_errno(&self) -> i32 {
        match self {
            FsError::NotFound => 2,            // ENOENT
            FsError::PermissionDenied => 13,   // EACCES
            FsError::AlreadyExists => 17,      // EEXIST
            FsError::FileExists => 17,         // EEXIST
            FsError::NotDirectory => 20,       // ENOTDIR
            FsError::IsDirectory => 21,        // EISDIR
            FsError::DirectoryNotEmpty => 39,  // ENOTEMPTY
            FsError::InvalidPath => 22,        // EINVAL
            FsError::InvalidArgument => 22,    // EINVAL
            FsError::TooManySymlinks => 40,    // ELOOP
            FsError::InvalidData => 22,        // EINVAL
            FsError::IoError => 5,             // EIO
            FsError::NotSupported => 95,       // EOPNOTSUPP
            FsError::TooManyFiles => 24,       // EMFILE
            FsError::TooManyOpenFiles => 24,   // EMFILE
            FsError::InvalidFd => 9,           // EBADF
            FsError::ConnectionRefused => 111, // ECONNREFUSED
            FsError::Again => 11,              // EAGAIN
            FsError::QuotaExceeded => 122,     // EDQUOT
            FsError::Memory(_) => 12,          // ENOMEM (simplified)
        }
    }
}

impl From<MemoryError> for FsError {
    fn from(e: MemoryError) -> Self {
        FsError::Memory(e)
    }
}

impl From<FsError> for MemoryError {
    fn from(e: FsError) -> Self {
        match e {
            FsError::NotFound => MemoryError::NotFound,
            FsError::PermissionDenied => MemoryError::PermissionDenied,
            FsError::AlreadyExists | FsError::FileExists => MemoryError::AlreadyMapped,
            FsError::NotDirectory => MemoryError::InvalidParameter,
            FsError::IsDirectory => MemoryError::InvalidParameter,
            FsError::DirectoryNotEmpty => MemoryError::InvalidParameter,
            FsError::InvalidPath => MemoryError::InvalidAddress,
            FsError::InvalidArgument => MemoryError::InvalidParameter,
            FsError::TooManySymlinks => MemoryError::InvalidParameter,
            FsError::InvalidData => MemoryError::InvalidParameter,
            FsError::IoError => MemoryError::InternalError("IO Error"),
            FsError::NotSupported => MemoryError::InternalError("Not supported"),
            FsError::TooManyFiles => MemoryError::OutOfMemory,
            FsError::TooManyOpenFiles => MemoryError::OutOfMemory,
            FsError::InvalidFd => MemoryError::InvalidParameter,
            FsError::ConnectionRefused => MemoryError::InternalError("Connection refused"),
            FsError::Again => MemoryError::InternalError("Try again"),
            FsError::QuotaExceeded => MemoryError::InternalError("Quota exceeded"),
            FsError::Memory(e) => e,
        }
    }
}

pub type FsResult<T> = Result<T, FsError>;

/// File metadata
#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub size: u64,
    pub is_dir: bool,
    pub read_only: bool,
}

/// File handle trait
pub trait File {
    fn read(&mut self, buf: &mut [u8]) -> FsResult<usize>;
    fn write(&mut self, buf: &[u8]) -> FsResult<usize>;
    fn seek(&mut self, pos: u64) -> FsResult<u64>;
    fn metadata(&self) -> FsResult<FileMetadata>;
}


