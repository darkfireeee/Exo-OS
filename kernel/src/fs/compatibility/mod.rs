//! Filesystem Compatibility Layer
//!
//! Provides legacy filesystem support and userspace filesystem interfaces:
//! - ext4: Read-only ext4 compatibility for existing drives
//! - FAT32: Full read/write FAT32 support for USB drives and compatibility
//! - tmpfs: In-memory filesystem for /tmp and fast storage
//! - FUSE: Userspace filesystem interface for custom filesystems
//!
//! # Design Philosophy
//! - Robust error handling for corrupted filesystems
//! - Backwards compatibility with existing disk formats
//! - Production-ready code with comprehensive validation
//! - Zero-copy I/O where possible
//!
//! # Usage
//! ```ignore
//! // Mount a FAT32 USB drive
//! let fat32_fs = Fat32Fs::mount(device)?;
//!
//! // Create a tmpfs for /tmp
//! let tmpfs = TmpFs::new(1024 * 1024 * 1024); // 1 GB
//!
//! // Read-only mount legacy ext4 drive
//! let ext4_fs = Ext4ReadOnlyFs::mount(device)?;
//! ```

pub mod ext4;
pub mod fat32;
pub mod tmpfs;
pub mod fuse;

pub use ext4::{Ext4ReadOnlyFs, Ext4ReadOnlyInode};
pub use fat32::Fat32Fs;
pub use tmpfs::{TmpFs, TmpfsInode};
pub use fuse::{FuseFs, FuseConnection};

use crate::fs::{FsError, FsResult};

/// Initialize compatibility layer
pub fn init() {
    log::info!("Initializing filesystem compatibility layer...");

    // Register filesystem types
    log::info!("  ext4: Read-only compatibility mode");
    log::info!("  FAT32: Full read/write support");
    log::info!("  tmpfs: In-memory filesystem");
    log::info!("  FUSE: Userspace filesystem interface");

    log::info!("✓ Filesystem compatibility layer initialized");
}

/// Detect filesystem type from boot sector/superblock
pub fn detect_fs_type(device: &dyn crate::fs::block::BlockDevice) -> FsResult<FilesystemType> {
    use crate::fs::utils::endian::*;

    // Read first sector
    let mut buf = [0u8; 512];
    device.read(0, &mut buf)?;

    // Check for FAT32 signature
    if buf[510] == 0x55 && buf[511] == 0xAA {
        let fs_type = &buf[82..90];
        if fs_type == b"FAT32   " {
            return Ok(FilesystemType::Fat32);
        }

        // Check for FAT16/FAT12
        let fs_type_16 = &buf[54..62];
        if fs_type_16 == b"FAT16   " || fs_type_16 == b"FAT12   " {
            return Ok(FilesystemType::Fat16);
        }
    }

    // Check for ext4 superblock
    let mut sb_buf = [0u8; 1024];
    device.read(1024, &mut sb_buf)?;
    let magic = read_le_u32(&sb_buf[56..60]);
    if magic == 0xEF53 {
        return Ok(FilesystemType::Ext4);
    }

    Err(FsError::InvalidData)
}

/// Filesystem type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesystemType {
    Ext4,
    Fat32,
    Fat16,
    Tmpfs,
    Fuse,
    Unknown,
}

impl FilesystemType {
    pub fn as_str(&self) -> &'static str {
        match self {
            FilesystemType::Ext4 => "ext4",
            FilesystemType::Fat32 => "fat32",
            FilesystemType::Fat16 => "fat16",
            FilesystemType::Tmpfs => "tmpfs",
            FilesystemType::Fuse => "fuse",
            FilesystemType::Unknown => "unknown",
        }
    }
}
