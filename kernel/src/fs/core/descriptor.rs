//! File Descriptor Management
//!
//! Implements high-performance file descriptor table with lock-free operations.
//!
//! Migrated from fs/operations/fdtable/ and fs/core.rs to core/descriptor.rs

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use spin::RwLock;

use crate::fs::{FsError, FsResult};

/// Maximum file descriptors per process
pub const MAX_FDS: usize = 1024;

/// Standard file descriptors
pub const STDIN_FD: i32 = 0;
pub const STDOUT_FD: i32 = 1;
pub const STDERR_FD: i32 = 2;

// ═══════════════════════════════════════════════════════════════════════════
// FILE DESCRIPTOR FLAGS
// ═══════════════════════════════════════════════════════════════════════════

/// File descriptor flags (fcntl F_GETFD/F_SETFD)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FdFlags {
    flags: u8,
}

impl FdFlags {
    /// Create new FD flags
    pub const fn new() -> Self {
        Self { flags: 0 }
    }

    /// Check close-on-exec flag
    #[inline(always)]
    pub fn close_on_exec(&self) -> bool {
        (self.flags & 0x1) != 0
    }

    /// Set close-on-exec flag
    #[inline]
    pub fn set_close_on_exec(&mut self, value: bool) {
        if value {
            self.flags |= 0x1;
        } else {
            self.flags &= !0x1;
        }
    }

    /// Get raw flags
    #[inline(always)]
    pub fn raw(&self) -> u8 {
        self.flags
    }

    /// Set raw flags
    #[inline]
    pub fn set_raw(&mut self, flags: u8) {
        self.flags = flags;
    }
}

impl Default for FdFlags {
    fn default() -> Self {
        Self::new()
    }
}

/// File status flags (fcntl F_GETFL/F_SETFL)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileFlags {
    flags: u32,
}

impl FileFlags {
    /// Create new file flags
    pub const fn new() -> Self {
        Self { flags: 0 }
    }

    /// Check non-blocking flag (O_NONBLOCK)
    #[inline(always)]
    pub fn nonblock(&self) -> bool {
        (self.flags & 0x800) != 0
    }

    /// Set non-blocking flag
    #[inline]
    pub fn set_nonblock(&mut self, value: bool) {
        if value {
            self.flags |= 0x800;
        } else {
            self.flags &= !0x800;
        }
    }

    /// Check append flag (O_APPEND)
    #[inline(always)]
    pub fn append(&self) -> bool {
        (self.flags & 0x400) != 0
    }

    /// Set append flag
    #[inline]
    pub fn set_append(&mut self, value: bool) {
        if value {
            self.flags |= 0x400;
        } else {
            self.flags &= !0x400;
        }
    }

    /// Check direct I/O flag (O_DIRECT)
    #[inline(always)]
    pub fn direct(&self) -> bool {
        (self.flags & 0x4000) != 0
    }

    /// Get raw flags
    #[inline(always)]
    pub fn raw(&self) -> u32 {
        self.flags
    }

    /// Set raw flags
    #[inline]
    pub fn set_raw(&mut self, flags: u32) {
        self.flags = flags;
    }
}

impl Default for FileFlags {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// FILE DESCRIPTOR TABLE
// ═══════════════════════════════════════════════════════════════════════════

/// File Descriptor Table - Per-process FD management
///
/// ## Performance Targets
/// - Lookup: O(1) via BTreeMap
/// - Allocate: O(log n) for finding free slot
/// - Thread-safe with RwLock
pub struct FileDescriptorTable<T = super::FileHandle> {
    /// Map fd → FileHandle
    handles: RwLock<BTreeMap<u32, Arc<T>>>,
    /// FD flags (close-on-exec, etc.)
    fd_flags: RwLock<BTreeMap<u32, FdFlags>>,
    /// Next fd to allocate
    next_fd: AtomicU32,
}

impl<T> FileDescriptorTable<T> {
    /// Create new FD table
    pub fn new() -> Self {
        Self {
            handles: RwLock::new(BTreeMap::new()),
            fd_flags: RwLock::new(BTreeMap::new()),
            next_fd: AtomicU32::new(3), // 0=stdin, 1=stdout, 2=stderr
        }
    }

    /// Allocate a new file descriptor
    ///
    /// # Performance
    /// - O(log n) insertion into BTreeMap
    /// - Thread-safe via RwLock
    pub fn allocate_fd(&self, handle: T) -> FsResult<u32> {
        let fd = self.next_fd.fetch_add(1, Ordering::Relaxed);

        if fd >= MAX_FDS as u32 {
            return Err(FsError::TooManyOpenFiles);
        }

        let mut handles = self.handles.write();
        handles.insert(fd, Arc::new(handle));

        let mut fd_flags = self.fd_flags.write();
        fd_flags.insert(fd, FdFlags::new());

        Ok(fd)
    }

    /// Allocate FD with specific number (for dup2)
    pub fn allocate_fd_at(&self, fd: u32, handle: T) -> FsResult<()> {
        if fd >= MAX_FDS as u32 {
            return Err(FsError::InvalidFd);
        }

        let mut handles = self.handles.write();
        handles.insert(fd, Arc::new(handle));

        let mut fd_flags = self.fd_flags.write();
        fd_flags.insert(fd, FdFlags::new());

        // Update next_fd if needed
        let current_next = self.next_fd.load(Ordering::Relaxed);
        if fd >= current_next {
            self.next_fd.store(fd + 1, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Get file handle
    ///
    /// # Performance
    /// - O(log n) lookup in BTreeMap
    /// - Read lock held briefly
    #[inline(always)]
    pub fn get(&self, fd: u32) -> FsResult<Arc<T>> {
        let handles = self.handles.read();
        handles.get(&fd)
            .cloned()
            .ok_or(FsError::InvalidFd)
    }

    /// Close file descriptor
    pub fn close(&self, fd: u32) -> FsResult<()> {
        let mut handles = self.handles.write();
        handles.remove(&fd)
            .ok_or(FsError::InvalidFd)?;

        let mut fd_flags = self.fd_flags.write();
        fd_flags.remove(&fd);

        Ok(())
    }

    /// Duplicate file descriptor (dup)
    pub fn duplicate(&self, old_fd: u32) -> FsResult<u32> {
        let handle = self.get(old_fd)?;

        // Get old flags
        let old_flags = {
            let flags_map = self.fd_flags.read();
            flags_map.get(&old_fd).copied().unwrap_or_default()
        };

        // Allocate new FD
        let new_fd = self.next_fd.fetch_add(1, Ordering::Relaxed);
        if new_fd >= MAX_FDS as u32 {
            return Err(FsError::TooManyOpenFiles);
        }

        let mut handles = self.handles.write();
        handles.insert(new_fd, handle);

        let mut fd_flags = self.fd_flags.write();
        fd_flags.insert(new_fd, old_flags);

        Ok(new_fd)
    }

    /// Duplicate to specific FD (dup2)
    pub fn duplicate_to(&self, old_fd: u32, new_fd: u32) -> FsResult<u32> {
        if old_fd == new_fd {
            // Check that old_fd is valid
            let _ = self.get(old_fd)?;
            return Ok(new_fd);
        }

        if new_fd >= MAX_FDS as u32 {
            return Err(FsError::InvalidFd);
        }

        let handle = self.get(old_fd)?;

        // Close new_fd if it exists
        let _ = self.close(new_fd);

        // Get old flags
        let old_flags = {
            let flags_map = self.fd_flags.read();
            flags_map.get(&old_fd).copied().unwrap_or_default()
        };

        let mut handles = self.handles.write();
        handles.insert(new_fd, handle);

        let mut fd_flags = self.fd_flags.write();
        fd_flags.insert(new_fd, old_flags);

        // Update next_fd if needed
        let current_next = self.next_fd.load(Ordering::Relaxed);
        if new_fd >= current_next {
            self.next_fd.store(new_fd + 1, Ordering::Relaxed);
        }

        Ok(new_fd)
    }

    /// Get FD flags (fcntl F_GETFD)
    pub fn get_fd_flags(&self, fd: u32) -> FsResult<FdFlags> {
        let flags_map = self.fd_flags.read();
        flags_map.get(&fd).copied().ok_or(FsError::InvalidFd)
    }

    /// Set FD flags (fcntl F_SETFD)
    pub fn set_fd_flags(&self, fd: u32, flags: FdFlags) -> FsResult<()> {
        // Verify FD exists
        let _ = self.get(fd)?;

        let mut flags_map = self.fd_flags.write();
        flags_map.insert(fd, flags);

        Ok(())
    }

    /// Set close-on-exec flag
    pub fn set_cloexec(&self, fd: u32, cloexec: bool) -> FsResult<()> {
        // Verify FD exists
        let _ = self.get(fd)?;

        let mut flags_map = self.fd_flags.write();
        let flags = flags_map.entry(fd).or_insert(FdFlags::new());
        flags.set_close_on_exec(cloexec);

        Ok(())
    }

    /// Get close-on-exec flag
    pub fn is_cloexec(&self, fd: u32) -> FsResult<bool> {
        let flags = self.get_fd_flags(fd)?;
        Ok(flags.close_on_exec())
    }

    /// Close all FDs marked as close-on-exec
    ///
    /// Called during exec() to close FDs with FD_CLOEXEC set
    pub fn close_on_exec(&self) -> FsResult<()> {
        let flags_map = self.fd_flags.read();
        let to_close: Vec<u32> = flags_map
            .iter()
            .filter(|(_, flags)| flags.close_on_exec())
            .map(|(fd, _)| *fd)
            .collect();
        drop(flags_map);

        for fd in to_close {
            let _ = self.close(fd);
        }

        Ok(())
    }

    /// Get number of open FDs
    pub fn count(&self) -> usize {
        let handles = self.handles.read();
        handles.len()
    }

    /// Check if FD is valid
    pub fn is_valid(&self, fd: u32) -> bool {
        let handles = self.handles.read();
        handles.contains_key(&fd)
    }

    /// Get all open FDs (for debugging)
    pub fn list_fds(&self) -> Vec<u32> {
        let handles = self.handles.read();
        handles.keys().copied().collect()
    }
}

impl<T> Default for FileDescriptorTable<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// SEEK WHENCE
// ═══════════════════════════════════════════════════════════════════════════

/// Seek whence for lseek()
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SeekWhence {
    /// Seek from start of file
    Start = 0,
    /// Seek from current position
    Current = 1,
    /// Seek from end of file
    End = 2,
}

impl SeekWhence {
    /// Convert from i32 (for syscall compatibility)
    pub fn from_i32(whence: i32) -> Option<Self> {
        match whence {
            0 => Some(SeekWhence::Start),
            1 => Some(SeekWhence::Current),
            2 => Some(SeekWhence::End),
            _ => None,
        }
    }
}
