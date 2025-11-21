//! Filesystem subsystem for hybrid architecture
//!
//! Supports multiple filesystems: VFS, FAT32, ext4, tmpfs, devfs, procfs, sysfs

use alloc::vec::Vec;
use alloc::string::String;
use crate::memory::MemoryError;

/// Virtual File System
pub mod vfs;

/// FAT32 filesystem
pub mod fat32;

/// ext4 filesystem  
pub mod ext4;

/// Filesystem errors
#[derive(Debug)]
pub enum FsError {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    NotDirectory,
    IsDirectory,
    InvalidPath,
    IoError,
    NotSupported,
    Memory(MemoryError),
}

impl From<MemoryError> for FsError {
    fn from(e: MemoryError) -> Self {
        FsError::Memory(e)
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
