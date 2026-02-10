//! Directory Operations
//!
//! High-level directory operations

use super::{DirectoryEntry, DirectoryManager, DirectoryType};
use crate::fs::{FsError, FsResult};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Directory operations trait
pub trait DirectoryOps {
    /// Create subdirectory
    fn mkdir(&self, name: &str) -> FsResult<u64>;

    /// Remove subdirectory
    fn rmdir(&self, name: &str) -> FsResult<()>;

    /// Add entry
    fn add(&self, name: &str, inode: u64, file_type: u8) -> FsResult<()>;

    /// Remove entry
    fn remove(&self, name: &str) -> FsResult<()>;

    /// Lookup entry
    fn lookup(&self, name: &str) -> FsResult<u64>;

    /// List entries
    fn list(&self) -> FsResult<Vec<DirectoryEntry>>;

    /// Check if empty
    fn is_empty(&self) -> FsResult<bool>;
}

/// Directory operations implementation
pub struct DirOpsImpl {
    /// Directory inode
    dir_ino: u64,
    /// Directory manager
    manager: Arc<DirectoryManager>,
}

impl DirOpsImpl {
    /// Create new directory operations
    pub fn new(dir_ino: u64, manager: Arc<DirectoryManager>) -> Self {
        Self { dir_ino, manager }
    }
}

impl DirectoryOps for DirOpsImpl {
    fn mkdir(&self, name: &str) -> FsResult<u64> {
        // Check if entry already exists
        if self.manager.lookup(self.dir_ino, name).is_ok() {
            return Err(FsError::AlreadyExists);
        }

        // Create subdirectory
        let subdir_ino = self.manager.create_directory(self.dir_ino, name)?;

        // Add entry to this directory
        self.manager.add_entry(self.dir_ino, name.into(), subdir_ino, 2)?;

        log::debug!("ext4plus: Created subdirectory '{}' in {}", name, self.dir_ino);

        Ok(subdir_ino)
    }

    fn rmdir(&self, name: &str) -> FsResult<()> {
        // Lookup directory
        let subdir_ino = self.manager.lookup(self.dir_ino, name)?;

        // Check if empty
        if !self.manager.is_empty(subdir_ino)? {
            return Err(FsError::DirectoryNotEmpty);
        }

        // Remove entry
        self.manager.remove_entry(self.dir_ino, name)?;

        log::debug!("ext4plus: Removed subdirectory '{}' from {}", name, self.dir_ino);

        Ok(())
    }

    fn add(&self, name: &str, inode: u64, file_type: u8) -> FsResult<()> {
        self.manager.add_entry(self.dir_ino, name.into(), inode, file_type)
    }

    fn remove(&self, name: &str) -> FsResult<()> {
        self.manager.remove_entry(self.dir_ino, name)
    }

    fn lookup(&self, name: &str) -> FsResult<u64> {
        self.manager.lookup(self.dir_ino, name)
    }

    fn list(&self) -> FsResult<Vec<DirectoryEntry>> {
        self.manager.list_entries(self.dir_ino)
    }

    fn is_empty(&self) -> FsResult<bool> {
        self.manager.is_empty(self.dir_ino)
    }
}
