// libs/exo_types/src/fd.rs
//! File descriptor RAII wrapper

use core::fmt;
use core::num::NonZeroU32;
use crate::errno::Errno;

/// File descriptor (RAII wrapper with auto-close)
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct FileDescriptor(NonZeroU32);

impl FileDescriptor {
    /// Standard input
    pub const STDIN: i32 = 0;
    
    /// Standard output
    pub const STDOUT: i32 = 1;
    
    /// Standard error
    pub const STDERR: i32 = 2;
    
    /// Minimum user FD
    pub const MIN_USER_FD: i32 = 3;
    
    /// Maximum FD value
    pub const MAX_FD: i32 = i32::MAX;
    
    /// Create FD from raw value
    ///
    /// Returns None if value is negative
    #[inline]
    pub const fn new(fd: i32) -> Option<Self> {
        if fd < 0 {
            None
        } else {
            // Safety: checked non-negative
            Some(unsafe { Self::from_raw_unchecked((fd as u32) + 1) })
        }
    }
    
    /// Create FD from raw value without validation
    ///
    /// # Safety
    /// Caller must ensure fd >= 0, internal representation is fd + 1
    #[inline]
    pub const unsafe fn from_raw_unchecked(fd_plus_one: u32) -> Self {
        Self(NonZeroU32::new_unchecked(fd_plus_one))
    }
    
    /// Get raw FD value
    #[inline]
    pub const fn as_raw(&self) -> i32 {
        (self.0.get() - 1) as i32
    }
    
    /// Create non-owning reference to FD
    #[inline]
    pub const fn borrow(&self) -> BorrowedFd {
        BorrowedFd(self.0)
    }
    
    /// Leak FD (prevent auto-close)
    #[inline]
    pub fn leak(self) -> i32 {
        let fd = self.as_raw();
        core::mem::forget(self);
        fd
    }
    
    /// Duplicate file descriptor (dup syscall)
    pub fn duplicate(&self) -> Result<Self, Errno> {
        // TODO: Real dup() syscall  
        Err(Errno::ENOSYS)
    }
}

impl Drop for FileDescriptor {
    fn drop(&mut self) {
        // TODO: Real close() syscall
        // For now, no-op (would leak FD in real implementation)
        let _fd = self.as_raw();
        // syscall::close(fd);
    }
}

impl fmt::Display for FileDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_raw())
    }
}

impl From<FileDescriptor> for i32 {
    #[inline]
    fn from(fd: FileDescriptor) -> i32 {
        fd.leak()
    }
}

/// Borrowed file descriptor reference (no auto-close)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct BorrowedFd(NonZeroU32);

impl BorrowedFd {
    /// Get raw FD value
    #[inline]
    pub const fn as_raw(&self) -> i32 {
        (self.0.get() - 1) as i32
    }
    
    /// Create from raw FD
    #[inline]
    pub const fn new(fd: i32) -> Option<Self> {
        if fd < 0 {
            None
        } else {
            Some(unsafe { Self(NonZeroU32::new_unchecked((fd as u32) + 1)) })
        }
    }
}

impl fmt::Display for BorrowedFd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_raw())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_fd_creation() {
        assert!(FileDescriptor::new(-1).is_none());
        assert!(FileDescriptor::new(0).is_some());
        assert_eq!(FileDescriptor::new(5).unwrap().as_raw(), 5);
    }
    
    #[test]
    fn test_fd_constants() {
        assert_eq!(FileDescriptor::STDIN, 0);
        assert_eq!(FileDescriptor::STDOUT, 1);
        assert_eq!(FileDescriptor::STDERR, 2);
    }
    
    #[test]
    fn test_fd_leak() {
        let fd = FileDescriptor::new(42).unwrap();
        let raw = fd.leak();
        assert_eq!(raw, 42);
        // fd is not dropped, no close() called
    }
}
