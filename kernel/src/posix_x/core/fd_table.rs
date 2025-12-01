//! File Descriptor Table - Phase 8
//!
//! Manages per-process file descriptor allocations

use crate::posix_x::vfs_posix::VfsHandle;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::RwLock;

/// FD table error type
#[derive(Debug, Clone, Copy)]
pub enum Error {
    BadFileDescriptor,
    TooManyOpenFiles,
}

pub type Result<T> = core::result::Result<T, Error>;

/// Maximum number of file descriptors per process
pub const MAX_FDS: usize = 1024;

/// Standard file descriptors
pub const FD_STDIN: i32 = 0;
pub const FD_STDOUT: i32 = 1;
pub const FD_STDERR: i32 = 2;

/// File descriptor flags
pub const FD_CLOEXEC: u32 = 1;

/// File descriptor entry (Phase 8: VFS-based)
#[derive(Clone)]
pub enum FdEntry {
    Empty,
    Active {
        handle: Arc<RwLock<VfsHandle>>,
        flags: u32,
    },
}

/// Per-process file descriptor table
pub struct FdTable {
    /// FD entries (Empty or Active with VFS handle)
    entries: Vec<FdEntry>,
    /// Next FD to allocate (optimization)
    next_fd: AtomicU32,
}

impl FdTable {
    /// Create a new empty FD table
    pub fn new() -> Self {
        let mut entries = Vec::with_capacity(MAX_FDS);
        entries.resize_with(MAX_FDS, || FdEntry::Empty);

        Self {
            entries,
            next_fd: AtomicU32::new(3), // Start after stdin/stdout/stderr
        }
    }

    /// Create default FD table with stdin/stdout/stderr
    /// TODO: Add actual VFS handles for stdio
    pub fn with_defaults() -> Self {
        Self::new()
    }

    /// Get VFS handle for file descriptor
    pub fn get(&self, fd: i32) -> Option<Arc<RwLock<VfsHandle>>> {
        if fd < 0 || fd >= MAX_FDS as i32 {
            return None;
        }
        match &self.entries[fd as usize] {
            FdEntry::Active { handle, .. } => Some(Arc::clone(handle)),
            FdEntry::Empty => None,
        }
    }

    /// Get FD flags
    pub fn get_flags(&self, fd: i32) -> Result<u32> {
        if fd < 0 || fd >= MAX_FDS as i32 {
            return Err(Error::BadFileDescriptor);
        }
        match &self.entries[fd as usize] {
            FdEntry::Active { flags, .. } => Ok(*flags),
            FdEntry::Empty => Err(Error::BadFileDescriptor),
        }
    }

    /// Set FD flags
    pub fn set_flags(&mut self, fd: i32, new_flags: u32) -> Result<()> {
        if fd < 0 || fd >= MAX_FDS as i32 {
            return Err(Error::BadFileDescriptor);
        }
        match &mut self.entries[fd as usize] {
            FdEntry::Active { flags, .. } => {
                *flags = new_flags;
                Ok(())
            }
            FdEntry::Empty => Err(Error::BadFileDescriptor),
        }
    }

    /// Allocate a new file descriptor with VFS handle
    pub fn allocate(&mut self, handle: VfsHandle) -> Result<i32> {
        let start_fd = self.next_fd.load(Ordering::Relaxed) as usize;

        // Search for free FD starting from next_fd
        for i in start_fd..MAX_FDS {
            if matches!(self.entries[i], FdEntry::Empty) {
                self.entries[i] = FdEntry::Active {
                    handle: Arc::new(RwLock::new(handle)),
                    flags: 0,
                };
                self.next_fd.store((i + 1) as u32, Ordering::Relaxed);
                return Ok(i as i32);
            }
        }

        Err(Error::TooManyOpenFiles)
    }

    /// Allocate a specific file descriptor (dup2 helper)
    pub fn allocate_at(
        &mut self,
        fd: i32,
        handle: Arc<RwLock<VfsHandle>>,
        flags: u32,
    ) -> Result<()> {
        if fd < 0 || fd >= MAX_FDS as i32 {
            return Err(Error::BadFileDescriptor);
        }

        // Close if open
        if !matches!(self.entries[fd as usize], FdEntry::Empty) {
            self.close(fd)?;
        }

        self.entries[fd as usize] = FdEntry::Active { handle, flags };
        Ok(())
    }

    /// Close a file descriptor
    pub fn close(&mut self, fd: i32) -> Result<()> {
        if fd < 0 || fd >= MAX_FDS as i32 {
            return Err(Error::BadFileDescriptor);
        }

        if matches!(self.entries[fd as usize], FdEntry::Empty) {
            return Err(Error::BadFileDescriptor);
        }

        // Drop the VFS handle (Arc will clean up when refcount = 0)
        self.entries[fd as usize] = FdEntry::Empty;
        Ok(())
    }

    /// Duplicate a file descriptor
    pub fn dup(&mut self, old_fd: i32) -> Result<i32> {
        self.dup_with_flags(old_fd, 0)
    }

    /// Duplicate with flags (dup3)
    pub fn dup_with_flags(&mut self, old_fd: i32, flags: u32) -> Result<i32> {
        let handle = self.get(old_fd).ok_or(Error::BadFileDescriptor)?;

        // Find free FD
        let start_fd = self.next_fd.load(Ordering::Relaxed) as usize;
        for i in start_fd..MAX_FDS {
            if matches!(self.entries[i], FdEntry::Empty) {
                self.entries[i] = FdEntry::Active { handle, flags };
                self.next_fd.store((i + 1) as u32, Ordering::Relaxed);
                return Ok(i as i32);
            }
        }

        Err(Error::TooManyOpenFiles)
    }

    /// Duplicate a file descriptor to specific FD (dup2)
    pub fn dup2(&mut self, old_fd: i32, new_fd: i32) -> Result<i32> {
        if old_fd < 0 || old_fd >= MAX_FDS as i32 {
            return Err(Error::BadFileDescriptor);
        }
        if new_fd < 0 || new_fd >= MAX_FDS as i32 {
            return Err(Error::BadFileDescriptor);
        }

        if old_fd == new_fd {
            return Ok(new_fd);
        }

        let handle = self.get(old_fd).ok_or(Error::BadFileDescriptor)?;

        // Close new_fd if it's open
        if !matches!(self.entries[new_fd as usize], FdEntry::Empty) {
            let _ = self.close(new_fd);
        }

        self.entries[new_fd as usize] = FdEntry::Active {
            handle,
            flags: 0, // dup2 clears flags
        };
        Ok(new_fd)
    }

    /// Duplicate to specific FD with flags (dup3)
    pub fn dup3(&mut self, old_fd: i32, new_fd: i32, flags: u32) -> Result<i32> {
        if old_fd < 0 || old_fd >= MAX_FDS as i32 {
            return Err(Error::BadFileDescriptor);
        }
        if new_fd < 0 || new_fd >= MAX_FDS as i32 {
            return Err(Error::BadFileDescriptor);
        }

        if old_fd == new_fd {
            return Err(Error::BadFileDescriptor); // dup3 fails if old == new
        }

        let handle = self.get(old_fd).ok_or(Error::BadFileDescriptor)?;

        // Close new_fd if it's open
        if !matches!(self.entries[new_fd as usize], FdEntry::Empty) {
            let _ = self.close(new_fd);
        }

        self.entries[new_fd as usize] = FdEntry::Active { handle, flags };
        Ok(new_fd)
    }

    /// Close all file descriptors (for process cleanup)
    pub fn close_all(&mut self) {
        for entry in &mut self.entries {
            *entry = FdEntry::Empty;
        }
    }

    /// Get number of open file descriptors
    pub fn count_open(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e, FdEntry::Active { .. }))
            .count()
    }

    /// Clone the FD table (for fork)
    /// This increments the refcount on all active VFS handles
    pub fn clone_table(&self) -> Self {
        let mut new_entries = Vec::with_capacity(MAX_FDS);
        for entry in &self.entries {
            match entry {
                FdEntry::Active { handle, flags } => new_entries.push(FdEntry::Active {
                    handle: Arc::clone(handle),
                    flags: *flags,
                }),
                FdEntry::Empty => new_entries.push(FdEntry::Empty),
            }
        }

        Self {
            entries: new_entries,
            next_fd: AtomicU32::new(self.next_fd.load(Ordering::Relaxed)),
        }
    }

    /// List all open FDs (for debugging)
    pub fn list_fds(&self) -> Vec<i32> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                if matches!(e, FdEntry::Active { .. }) {
                    Some(i as i32)
                } else {
                    None
                }
            })
            .collect()
    }
}

impl Default for FdTable {
    fn default() -> Self {
        Self::new()
    }
}

// TODO: Add tests for VFS-based FD table

/// Global FD table (Phase 8/12 integration)
/// TODO: Move to per-process state in Phase 13
pub static GLOBAL_FD_TABLE: spin::Lazy<RwLock<FdTable>> =
    spin::Lazy::new(|| RwLock::new(FdTable::new()));
