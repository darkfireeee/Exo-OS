//! FAT32 File Operations
//!
//! High-level file operations built on top of cluster management.
//! Provides file reading, writing, truncation, and growth.

use alloc::vec::Vec;
use alloc::sync::Arc;
use alloc::string::ToString;

use crate::fs::{FsError, FsResult};

use super::{Fat32Fs, Fat32DirEntry};

impl Fat32Fs {
    /// Read file data
    pub fn read_file(&self, entry: &Fat32DirEntry, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        if entry.is_directory() {
            return Err(FsError::IsDirectory);
        }

        let file_size = entry.file_size as u64;

        if offset >= file_size {
            return Ok(0);
        }

        let to_read = ((file_size - offset) as usize).min(buf.len());

        // Read cluster chain
        let data = self.read_cluster_chain(entry.first_cluster, Some(file_size as usize))?;

        // Copy requested portion
        let start = offset as usize;
        let end = start + to_read;

        if end <= data.len() {
            buf[..to_read].copy_from_slice(&data[start..end]);
            Ok(to_read)
        } else {
            // File shorter than expected
            let actual = data.len().saturating_sub(start);
            if actual > 0 {
                buf[..actual].copy_from_slice(&data[start..start + actual]);
                Ok(actual)
            } else {
                Ok(0)
            }
        }
    }

    /// Write file data
    pub fn write_file(
        &self,
        entry: &mut Fat32DirEntry,
        offset: u64,
        buf: &[u8],
    ) -> FsResult<usize> {
        if entry.is_directory() {
            return Err(FsError::IsDirectory);
        }

        let offset = offset as usize;
        let new_size = offset + buf.len();
        let cluster_size = self.cluster_size();

        // Calculate required clusters
        let required_clusters = (new_size + cluster_size - 1) / cluster_size;

        // Get current cluster chain
        let current_chain = if entry.first_cluster >= 2 {
            self.get_cluster_chain(entry.first_cluster)?
        } else {
            Vec::new()
        };

        let current_clusters = current_chain.len();

        // Allocate more clusters if needed
        if required_clusters > current_clusters {
            let additional = required_clusters - current_clusters;

            if current_clusters == 0 {
                // Allocate first cluster
                let new_chain = self.allocate_cluster_chain(required_clusters)?;
                entry.first_cluster = new_chain[0];
            } else {
                // Extend existing chain
                self.extend_cluster_chain(entry.first_cluster, additional)?;
            }
        }

        // Read existing data
        let mut data = if entry.file_size > 0 {
            self.read_cluster_chain(entry.first_cluster, Some(entry.file_size as usize))?
        } else {
            Vec::new()
        };

        // Extend data if writing beyond current size
        if new_size > data.len() {
            data.resize(new_size, 0);
        }

        // Write new data
        data[offset..offset + buf.len()].copy_from_slice(buf);

        // Update file size
        if new_size > entry.file_size as usize {
            entry.file_size = new_size as u32;
        }

        // Write back to clusters
        self.write_cluster_chain(entry.first_cluster, &data)?;

        Ok(buf.len())
    }

    /// Truncate file to new size
    pub fn truncate_file(&self, entry: &mut Fat32DirEntry, new_size: u64) -> FsResult<()> {
        if entry.is_directory() {
            return Err(FsError::IsDirectory);
        }

        let new_size = new_size as u32;
        let cluster_size = self.cluster_size();

        if new_size == 0 {
            // Free all clusters
            if entry.first_cluster >= 2 {
                self.free_cluster_chain(entry.first_cluster)?;
                entry.first_cluster = 0;
            }
            entry.file_size = 0;
            return Ok(());
        }

        // Calculate required clusters
        let required_clusters = ((new_size as usize) + cluster_size - 1) / cluster_size;

        // Truncate cluster chain
        if entry.first_cluster >= 2 {
            self.truncate_cluster_chain(entry.first_cluster, required_clusters)?;
        }

        entry.file_size = new_size;

        Ok(())
    }

    /// Get file data as vector
    pub fn read_file_all(&self, entry: &Fat32DirEntry) -> FsResult<Vec<u8>> {
        if entry.is_directory() {
            return Err(FsError::IsDirectory);
        }

        if entry.file_size == 0 {
            return Ok(Vec::new());
        }

        self.read_cluster_chain(entry.first_cluster, Some(entry.file_size as usize))
    }

    /// Write entire file
    pub fn write_file_all(&self, entry: &mut Fat32DirEntry, data: &[u8]) -> FsResult<()> {
        // Truncate to 0 first
        self.truncate_file(entry, 0)?;

        if data.is_empty() {
            return Ok(());
        }

        // Write data
        self.write_file(entry, 0, data)?;

        Ok(())
    }

    /// Check if file exists in directory
    pub fn file_exists(&self, dir_cluster: u32, name: &str) -> FsResult<bool> {
        let dir = super::Fat32Directory::new(dir_cluster);
        match self.find_entry(&dir, name) {
            Ok(_) => Ok(true),
            Err(FsError::NotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Create new file in directory
    pub fn create_file(
        &self,
        dir_cluster: u32,
        name: &str,
    ) -> FsResult<Fat32DirEntry> {
        let dir = super::Fat32Directory::new(dir_cluster);

        // Check if already exists
        if self.file_exists(dir_cluster, name)? {
            return Err(FsError::AlreadyExists);
        }

        // Allocate first cluster for file
        let first_cluster = self.allocate_cluster()?;

        // Create directory entry
        self.create_entry(&dir, name, 0, first_cluster)?;

        // Return new entry
        let short_name = {
            let name_upper = name.to_uppercase();
            let parts: alloc::vec::Vec<&str> = name_upper.split('.').collect();
            let (base, ext) = if parts.len() > 1 {
                (parts[0], parts[parts.len() - 1])
            } else {
                (name_upper.as_str(), "")
            };
            let base = if base.len() > 8 { &base[..8] } else { base };
            let ext = if ext.len() > 3 { &ext[..3] } else { ext };
            if ext.is_empty() {
                alloc::format!("{:<8}   ", base)
            } else {
                alloc::format!("{:<8}{:<3}", base, ext)
            }
        };

        Ok(Fat32DirEntry {
            name: name.to_string(),
            short_name,
            attr: 0,
            first_cluster,
            file_size: 0,
            create_time: 0,
            create_date: 0,
            modify_time: 0,
            modify_date: 0,
        })
    }

    /// Create directory
    pub fn create_directory(
        &self,
        parent_cluster: u32,
        name: &str,
    ) -> FsResult<Fat32DirEntry> {
        let dir = super::Fat32Directory::new(parent_cluster);

        // Check if already exists
        if self.file_exists(parent_cluster, name)? {
            return Err(FsError::AlreadyExists);
        }

        // Allocate first cluster for directory
        let first_cluster = self.allocate_cluster()?;

        // Initialize directory with . and .. entries
        let mut dir_data = alloc::vec![0u8; self.cluster_size()];

        // Create "." entry
        let mut dot_entry = super::dir::DirEntry {
            name: [0x20; 11],
            attr: super::dir::ATTR_DIRECTORY,
            nt_reserved: 0,
            create_time_tenth: 0,
            create_time: 0,
            create_date: 0,
            access_date: 0,
            first_cluster_hi: (first_cluster >> 16) as u16,
            modify_time: 0,
            modify_date: 0,
            first_cluster_lo: (first_cluster & 0xFFFF) as u16,
            file_size: 0,
        };
        dot_entry.name[0] = b'.';
        dir_data[0..32].copy_from_slice(&dot_entry.to_bytes());

        // Create ".." entry
        let mut dotdot_entry = dot_entry;
        dotdot_entry.name[1] = b'.';
        dotdot_entry.set_first_cluster(parent_cluster);
        dir_data[32..64].copy_from_slice(&dotdot_entry.to_bytes());

        // Write directory data
        self.write_cluster(first_cluster, &dir_data)?;

        // Create directory entry in parent
        self.create_entry(&dir, name, super::dir::ATTR_DIRECTORY, first_cluster)?;

        // Return new entry
        let short_name = {
            let name_upper = name.to_uppercase();
            let parts: alloc::vec::Vec<&str> = name_upper.split('.').collect();
            let (base, ext) = if parts.len() > 1 {
                (parts[0], parts[parts.len() - 1])
            } else {
                (name_upper.as_str(), "")
            };
            let base = if base.len() > 8 { &base[..8] } else { base };
            let ext = if ext.len() > 3 { &ext[..3] } else { ext };
            if ext.is_empty() {
                alloc::format!("{:<8}   ", base)
            } else {
                alloc::format!("{:<8}{:<3}", base, ext)
            }
        };

        Ok(Fat32DirEntry {
            name: name.to_string(),
            short_name,
            attr: super::dir::ATTR_DIRECTORY,
            first_cluster,
            file_size: 0,
            create_time: 0,
            create_date: 0,
            modify_time: 0,
            modify_date: 0,
        })
    }

    /// Delete file from directory
    pub fn delete_file(&self, dir_cluster: u32, name: &str) -> FsResult<()> {
        let dir = super::Fat32Directory::new(dir_cluster);

        // Find entry
        let entry = self.find_entry(&dir, name)?;

        if entry.is_directory() {
            return Err(FsError::IsDirectory);
        }

        // Free cluster chain
        if entry.first_cluster >= 2 {
            self.free_cluster_chain(entry.first_cluster)?;
        }

        // Delete directory entry
        self.delete_entry(&dir, name)?;

        Ok(())
    }

    /// Delete directory (must be empty)
    pub fn delete_directory(&self, parent_cluster: u32, name: &str) -> FsResult<()> {
        let parent_dir = super::Fat32Directory::new(parent_cluster);

        // Find entry
        let entry = self.find_entry(&parent_dir, name)?;

        if !entry.is_directory() {
            return Err(FsError::NotDirectory);
        }

        // Check if directory is empty (only . and .. allowed)
        let dir = super::Fat32Directory::new(entry.first_cluster);
        let entries = self.read_dir(&dir)?;

        let non_dot_entries: Vec<_> = entries
            .iter()
            .filter(|e| e.name != "." && e.name != "..")
            .collect();

        if !non_dot_entries.is_empty() {
            return Err(FsError::DirectoryNotEmpty);
        }

        // Free cluster chain
        if entry.first_cluster >= 2 {
            self.free_cluster_chain(entry.first_cluster)?;
        }

        // Delete directory entry
        self.delete_entry(&parent_dir, name)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests would require a mock FAT32 filesystem
    // For now, we verify the code compiles and has correct signatures
}
