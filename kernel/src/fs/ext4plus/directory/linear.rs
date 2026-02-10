//! Linear Directory
//!
//! Simple linear directory implementation for small directories.
//! Entries are stored sequentially in directory blocks.

use super::DirectoryEntry;
use crate::fs::{FsError, FsResult};
use alloc::vec::Vec;

/// Linear directory
pub struct LinearDirectory {
    /// Directory inode number
    inode: u64,
    /// Directory entries
    entries: Vec<DirectoryEntry>,
}

impl LinearDirectory {
    /// Create new linear directory
    pub fn new(inode: u64) -> Self {
        Self {
            inode,
            entries: Vec::new(),
        }
    }

    /// Add entry
    pub fn add_entry(&mut self, entry: DirectoryEntry) -> FsResult<()> {
        // Check for duplicate names
        if self.entries.iter().any(|e| e.name == entry.name) {
            return Err(FsError::AlreadyExists);
        }

        self.entries.push(entry);
        log::trace!("ext4plus: Added entry to linear directory {}", self.inode);

        Ok(())
    }

    /// Remove entry
    pub fn remove_entry(&mut self, name: &str) -> FsResult<()> {
        if let Some(pos) = self.entries.iter().position(|e| e.name == name) {
            self.entries.remove(pos);
            log::trace!("ext4plus: Removed entry from linear directory {}", self.inode);
            Ok(())
        } else {
            Err(FsError::NotFound)
        }
    }

    /// Lookup entry
    pub fn lookup(&self, name: &str) -> Option<&DirectoryEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Get all entries
    pub fn entries(&self) -> &[DirectoryEntry] {
        &self.entries
    }

    /// Get entry count
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Serialize to bytes
    pub fn serialize(&self, block_size: usize) -> FsResult<Vec<u8>> {
        let mut buffer = alloc::vec![0u8; block_size];
        let mut offset = 0;

        for entry in &self.entries {
            let entry_size = entry.size();
            if offset + entry_size > buffer.len() {
                return Err(FsError::NoSpace);
            }

            entry.serialize(&mut buffer[offset..offset + entry_size])?;
            offset += entry_size;
        }

        Ok(buffer)
    }

    /// Parse from bytes
    pub fn parse(inode: u64, data: &[u8]) -> FsResult<Self> {
        let mut entries = Vec::new();
        let mut offset = 0;

        while offset + 8 <= data.len() {
            match DirectoryEntry::parse(&data[offset..]) {
                Ok((entry, rec_len)) => {
                    if entry.inode != 0 {
                        entries.push(entry);
                    }
                    offset += rec_len;
                }
                Err(_) => break,
            }
        }

        Ok(Self { inode, entries })
    }

    /// Check if directory is empty (only . and ..)
    pub fn is_empty(&self) -> bool {
        self.entries.len() <= 2
    }
}
