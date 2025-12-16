//! Real filesystems module
//!
//! Contains implementations of real on-disk filesystems:
//! - FAT32: Microsoft FAT32 filesystem
//! - ext4: Linux ext4 filesystem with extents and journaling

// FAT32 filesystem implementation
pub mod fat32;

// ext4 filesystem implementation (Phase 2+)
// pub mod ext4;  // ⏸️ Requires ExtentTree implementation
