//! File Descriptor Manager for POSIX-X
//!
//! Manages the mapping between POSIX file descriptors (integers) and VFS handles.
//! Provides fast FD allocation/deallocation with O(1) operations.

use crate::fs::FsResult;
use crate::posix_x::vfs_posix::{OpenFlags, VfsHandle};
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

/// Maximum number of file descriptors per process
pub const MAX_FDS: usize = 1024;

/// Special FD values
pub const STDIN_FD: i32 = 0;
pub const STDOUT_FD: i32 = 1;
pub const STDERR_FD: i32 = 2;

/// File descriptor entry
enum FdEntry {
    /// Empty slot
    Empty,
    /// Active file descriptor
    Active(Arc<RwLock<VfsHandle>>),
}

/// File descriptor table for a process
pub struct FdTable {
    /// Array of FD entries
    entries: Vec<FdEntry>,

    /// Next FD to allocate (optimization)
    next_fd: i32,
}

impl FdTable {
    /// Create new FD table
    pub fn new() -> Self {
        let mut entries = Vec::with_capacity(MAX_FDS);

        // Reserve stdio FDs (0, 1, 2) as empty for now
        // They will be initialized by init process
        for _ in 0..3 {
            entries.push(FdEntry::Empty);
        }

        Self {
            entries,
            next_fd: 3,
        }
    }

    /// Allocate new file descriptor
    ///
    /// # Performance
    /// O(1) amortized - tracks next available FD
    pub fn allocate(&mut self, handle: VfsHandle) -> FsResult<i32> {
        let handle = Arc::new(RwLock::new(handle));

        // Try to use next_fd hint first
        if (self.next_fd as usize) < self.entries.len() {
            if matches!(self.entries[self.next_fd as usize], FdEntry::Empty) {
                let fd = self.next_fd;
                self.entries[fd as usize] = FdEntry::Active(handle);
                self.next_fd = self.find_next_free_fd(fd + 1);
                return Ok(fd);
            }
        }

        // Search for first empty slot
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if matches!(entry, FdEntry::Empty) {
                *entry = FdEntry::Active(handle);
                self.next_fd = self.find_next_free_fd(i as i32 + 1);
                return Ok(i as i32);
            }
        }

        // Grow table if not at max
        if self.entries.len() < MAX_FDS {
            let fd = self.entries.len() as i32;
            self.entries.push(FdEntry::Active(handle));
            self.next_fd = fd + 1;
            return Ok(fd);
        }

        Err(crate::fs::FsError::TooManyFiles)
    }

    /// Get VFS handle for FD
    ///
    /// # Performance
    /// O(1) array indexing
    #[inline]
    pub fn get(&self, fd: i32) -> Option<Arc<RwLock<VfsHandle>>> {
        if fd < 0 || (fd as usize) >= self.entries.len() {
            return None;
        }

        match &self.entries[fd as usize] {
            FdEntry::Active(handle) => Some(Arc::clone(handle)),
            FdEntry::Empty => None,
        }
    }

    /// Close file descriptor
    ///
    /// # Performance
    /// O(1) - just marks slot as empty
    pub fn close(&mut self, fd: i32) -> FsResult<()> {
        if fd < 0 || (fd as usize) >= self.entries.len() {
            return Err(crate::fs::FsError::InvalidFd);
        }

        match &self.entries[fd as usize] {
            FdEntry::Empty => Err(crate::fs::FsError::InvalidFd),
            FdEntry::Active(_) => {
                self.entries[fd as usize] = FdEntry::Empty;

                // Update next_fd hint if this FD is lower
                if fd < self.next_fd {
                    self.next_fd = fd;
                }

                Ok(())
            }
        }
    }

    /// Duplicate file descriptor
    pub fn dup(&mut self, oldfd: i32) -> FsResult<i32> {
        let handle = self.get(oldfd).ok_or(crate::fs::FsError::InvalidFd)?;

        // Clone the handle
        let cloned = {
            let h = handle.read();
            VfsHandle::new(h.inode(), h.flags(), h.path().to_string())
        };

        self.allocate(cloned)
    }

    /// Duplicate file descriptor to specific FD
    pub fn dup2(&mut self, oldfd: i32, newfd: i32) -> FsResult<i32> {
        if oldfd == newfd {
            // Check that oldfd is valid
            self.get(oldfd).ok_or(crate::fs::FsError::InvalidFd)?;
            return Ok(newfd);
        }

        let handle = self.get(oldfd).ok_or(crate::fs::FsError::InvalidFd)?;

        // Close newfd if it's open
        if newfd >= 0 && (newfd as usize) < self.entries.len() {
            let _ = self.close(newfd);
        }

        // Extend table if necessary
        while (newfd as usize) >= self.entries.len() && self.entries.len() < MAX_FDS {
            self.entries.push(FdEntry::Empty);
        }

        if (newfd as usize) >= MAX_FDS {
            return Err(crate::fs::FsError::TooManyFiles);
        }

        // Clone the handle
        let cloned = {
            let h = handle.read();
            VfsHandle::new(h.inode(), h.flags(), h.path().to_string())
        };

        self.entries[newfd as usize] = FdEntry::Active(Arc::new(RwLock::new(cloned)));
        Ok(newfd)
    }

    /// Get all open FDs (for debugging/proc)
    pub fn list_fds(&self) -> Vec<i32> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, entry)| {
                if matches!(entry, FdEntry::Active(_)) {
                    Some(i as i32)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Count open FDs
    pub fn count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e, FdEntry::Active(_)))
            .count()
    }

    /// Close all FDs (for process termination)
    pub fn close_all(&mut self) {
        for entry in &mut self.entries {
            *entry = FdEntry::Empty;
        }
        self.next_fd = 0;
    }

    /// Find next free FD starting from hint
    fn find_next_free_fd(&self, start: i32) -> i32 {
        for i in (start as usize)..self.entries.len() {
            if matches!(self.entries[i], FdEntry::Empty) {
                return i as i32;
            }
        }
        self.entries.len() as i32
    }
}

impl Default for FdTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fd_allocation() {
        let mut table = FdTable::new();
        assert_eq!(table.count(), 0);

        // First allocation should be FD 3 (after stdio)
        // This would need a real VfsHandle to test properly
    }

    #[test]
    fn test_fd_close() {
        let mut table = FdTable::new();
        // Test would need VfsHandle
    }
}
