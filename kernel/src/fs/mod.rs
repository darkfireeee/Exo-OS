//! Filesystem subsystem for hybrid architecture
//!
//! Supports multiple filesystems: VFS, FAT32, ext4, tmpfs, devfs, procfs, sysfs

use crate::memory::MemoryError;
use alloc::string::String;
use alloc::vec::Vec;

/// Virtual File System
pub mod vfs;

/// FAT32 filesystem
pub mod fat32;

/// ext4 filesystem  
pub mod ext4;

/// TmpFS - Temporary filesystem (RAM)
pub mod tmpfs;

/// DevFS - Device filesystem
pub mod devfs;

/// ProcFS - Process information
pub mod procfs;

/// SysFS - System information
pub mod sysfs;

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
    InvalidFd,
    ConnectionRefused,
    Again,
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
            FsError::InvalidFd => 9,           // EBADF
            FsError::ConnectionRefused => 111, // ECONNREFUSED
            FsError::Again => 11,              // EAGAIN
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
            FsError::InvalidFd => MemoryError::InvalidParameter,
            FsError::ConnectionRefused => MemoryError::InternalError("Connection refused"),
            FsError::Again => MemoryError::InternalError("Try again"),
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

/// Initialize filesystem subsystem
pub fn init() -> FsResult<()> {
    log::info!("Initializing filesystem subsystem (hybrid architecture)");
    vfs::init()?;
    Ok(())
}
