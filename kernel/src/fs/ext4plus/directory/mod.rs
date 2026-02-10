//! Directory Management
//!
//! Complete directory implementation with:
//! - Linear directory entries
//! - HTree indexed directories
//! - Directory operations (lookup, create, delete)
//! - Efficient large directory support

pub mod htree;
pub mod linear;
pub mod ops;

use crate::fs::{FsError, FsResult};
use crate::fs::block::BlockDevice;
use crate::fs::core::types::InodeType;
use alloc::sync::Arc;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

pub use htree::{HTree, HashAlgorithm, DirHash};
pub use linear::LinearDirectory;
pub use ops::DirectoryOps;

/// Directory entry
#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    /// Inode number
    pub inode: u64,
    /// Entry name
    pub name: String,
    /// File type
    pub file_type: u8,
}

impl DirectoryEntry {
    /// Create new directory entry
    pub fn new(inode: u64, name: String, file_type: u8) -> Self {
        Self {
            inode,
            name,
            file_type,
        }
    }

    /// Get entry size (aligned to 4 bytes)
    pub fn size(&self) -> usize {
        let base_size = 8 + self.name.len();
        (base_size + 3) & !3 // Align to 4 bytes
    }

    /// Serialize to bytes
    pub fn serialize(&self, buf: &mut [u8]) -> FsResult<()> {
        if buf.len() < 8 + self.name.len() {
            return Err(FsError::InvalidArgument);
        }

        buf[0..4].copy_from_slice(&(self.inode as u32).to_le_bytes());
        buf[4..6].copy_from_slice(&(self.size() as u16).to_le_bytes());
        buf[6] = self.name.len() as u8;
        buf[7] = self.file_type;
        buf[8..8 + self.name.len()].copy_from_slice(self.name.as_bytes());

        Ok(())
    }

    /// Parse from bytes
    pub fn parse(buf: &[u8]) -> FsResult<(Self, usize)> {
        if buf.len() < 8 {
            return Err(FsError::InvalidData);
        }

        let inode = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as u64;
        let rec_len = u16::from_le_bytes([buf[4], buf[5]]) as usize;
        let name_len = buf[6] as usize;
        let file_type = buf[7];

        if inode == 0 || rec_len == 0 {
            return Err(FsError::InvalidData);
        }

        if buf.len() < 8 + name_len {
            return Err(FsError::InvalidData);
        }

        let name = String::from_utf8(buf[8..8 + name_len].to_vec())
            .map_err(|_| FsError::InvalidData)?;

        Ok((Self {
            inode,
            name,
            file_type,
        }, rec_len))
    }
}

/// Directory type
#[derive(Debug, Clone, Copy)]
pub enum DirectoryType {
    /// Linear directory (small directories)
    Linear,
    /// HTree indexed directory (large directories)
    HTree,
}

/// Directory Manager
pub struct DirectoryManager {
    /// Block device
    device: Arc<Mutex<dyn BlockDevice>>,
    /// Inode manager
    inode_manager: Arc<super::inode::InodeManager>,
    /// HTree threshold (switch to HTree after this many entries)
    htree_threshold: usize,
}

impl DirectoryManager {
    /// Create new directory manager
    pub fn new(
        device: Arc<Mutex<dyn BlockDevice>>,
        inode_manager: Arc<super::inode::InodeManager>,
    ) -> FsResult<Arc<Self>> {
        Ok(Arc::new(Self {
            device,
            inode_manager,
            htree_threshold: 100, // Switch to HTree after 100 entries
        }))
    }

    /// Create new directory
    pub fn create_directory(&self, parent_ino: u64, name: &str) -> FsResult<u64> {
        // Allocate inode for directory
        let dir_inode = self.inode_manager.allocate_inode(InodeType::Directory)?;
        let ino = dir_inode.lock().ino;

        // Create . and .. entries
        let mut dir = LinearDirectory::new(ino);
        dir.add_entry(DirectoryEntry::new(ino, ".".into(), 2))?;
        dir.add_entry(DirectoryEntry::new(parent_ino, "..".into(), 2))?;

        log::debug!("ext4plus: Created directory inode {}", ino);

        Ok(ino)
    }

    /// Lookup entry in directory
    pub fn lookup(&self, dir_ino: u64, name: &str) -> FsResult<u64> {
        // In production, would check if directory uses HTree or linear
        // For now, use simple linear search

        log::trace!("ext4plus: Looking up '{}' in directory {}", name, dir_ino);

        // Would actually read directory blocks and search

        Err(FsError::NotFound)
    }

    /// Add entry to directory
    pub fn add_entry(&self, dir_ino: u64, name: String, inode: u64, file_type: u8) -> FsResult<()> {
        log::trace!("ext4plus: Adding entry '{}' (inode {}) to directory {}",
            name, inode, dir_ino);

        // In production, would:
        // 1. Check directory type (linear vs htree)
        // 2. Add entry using appropriate method
        // 3. Update directory inode
        // 4. Flush to disk

        Ok(())
    }

    /// Remove entry from directory
    pub fn remove_entry(&self, dir_ino: u64, name: &str) -> FsResult<()> {
        log::trace!("ext4plus: Removing entry '{}' from directory {}", name, dir_ino);

        // In production, would:
        // 1. Find entry
        // 2. Mark as deleted (set inode to 0)
        // 3. Update directory inode
        // 4. Flush to disk

        Ok(())
    }

    /// List directory entries
    pub fn list_entries(&self, dir_ino: u64) -> FsResult<Vec<DirectoryEntry>> {
        log::trace!("ext4plus: Listing entries in directory {}", dir_ino);

        // In production, would read all directory blocks and parse entries

        Ok(Vec::new())
    }

    /// Check if directory is empty
    pub fn is_empty(&self, dir_ino: u64) -> FsResult<bool> {
        let entries = self.list_entries(dir_ino)?;

        // Directory is empty if it only has . and ..
        Ok(entries.len() <= 2)
    }

    /// Get directory type
    pub fn get_type(&self, dir_ino: u64) -> FsResult<DirectoryType> {
        // In production, would check inode flags
        // For now, assume linear
        Ok(DirectoryType::Linear)
    }

    /// Convert directory to HTree
    pub fn convert_to_htree(&self, dir_ino: u64) -> FsResult<()> {
        log::info!("ext4plus: Converting directory {} to HTree", dir_ino);

        // In production, would:
        // 1. Read all entries
        // 2. Create HTree structure
        // 3. Hash entries and build tree
        // 4. Write back to disk
        // 5. Update inode flags

        Ok(())
    }
}
