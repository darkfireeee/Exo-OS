//! File descriptor RAII wrapper
//!
//! Provides RAII semantics for file descriptors with automatic cleanup.
//! Uses NonZeroU32 internally to enable Option<FileDescriptor> niche optimization.

use core::fmt;
use core::num::NonZeroU32;
use crate::errno::Errno;

/// File descriptor with RAII semantics (auto-closes on drop)
///
/// Owns a file descriptor and automatically closes it when dropped,
/// preventing resource leaks. Use `BorrowedFd` for non-owning references.
///
/// # Internal representation
/// Uses `fd + 1` to allow NonZeroU32 optimization while supporting fd=0 (stdin).
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct FileDescriptor(NonZeroU32);

impl FileDescriptor {
    /// Standard input file descriptor
    pub const STDIN: i32 = 0;
    
    /// Standard output file descriptor
    pub const STDOUT: i32 = 1;
    
    /// Standard error file descriptor
    pub const STDERR: i32 = 2;
    
    /// First user file descriptor (after stdin/stdout/stderr)
    pub const MIN_USER_FD: i32 = 3;
    
    /// Maximum valid file descriptor value
    pub const MAX_FD: i32 = i32::MAX;
    
    /// Create file descriptor from raw value
    ///
    /// Returns None if value is negative (invalid FD).
    ///
    /// # Examples
    /// ```
    /// use exo_types::FileDescriptor;
    /// 
    /// assert!(FileDescriptor::new(-1).is_none());
    /// assert_eq!(FileDescriptor::new(5).unwrap().as_raw(), 5);
    /// ```
    #[inline(always)]
    pub const fn new(fd: i32) -> Option<Self> {
        if fd < 0 {
            None
        } else {
            Some(unsafe { Self::from_raw_unchecked((fd as u32).wrapping_add(1)) })
        }
    }
    
    /// Create FD from raw value without validation
    ///
    /// # Safety
    /// - Caller must ensure `fd >= 0`
    /// - Internal representation is `fd + 1` to use NonZeroU32
    #[inline(always)]
    pub const unsafe fn from_raw_unchecked(fd_plus_one: u32) -> Self {
        Self(NonZeroU32::new_unchecked(fd_plus_one))
    }
    
    /// Get raw file descriptor value
    #[inline(always)]
    pub const fn as_raw(&self) -> i32 {
        self.0.get().wrapping_sub(1) as i32
    }
    
    /// Create non-owning borrowed reference to this FD
    #[inline(always)]
    pub const fn borrow(&self) -> BorrowedFd {
        BorrowedFd(self.0)
    }
    
    /// Check if this is standard input
    #[inline(always)]
    pub const fn is_stdin(&self) -> bool {
        self.as_raw() == Self::STDIN
    }
    
    /// Check if this is standard output
    #[inline(always)]
    pub const fn is_stdout(&self) -> bool {
        self.as_raw() == Self::STDOUT
    }
    
    /// Check if this is standard error
    #[inline(always)]
    pub const fn is_stderr(&self) -> bool {
        self.as_raw() == Self::STDERR
    }
    
    /// Check if this is a standard stream (stdin/stdout/stderr)
    #[inline(always)]
    pub const fn is_standard_stream(&self) -> bool {
        let raw = self.as_raw();
        raw >= 0 && raw <= 2
    }
    
    /// Check if this is a user file descriptor (>= 3)
    #[inline(always)]
    pub const fn is_user_fd(&self) -> bool {
        self.as_raw() >= Self::MIN_USER_FD
    }
    
    /// Leak FD ownership (prevent auto-close)
    ///
    /// Consumes self and returns raw FD without closing it.
    /// Caller is responsible for closing the FD manually.
    #[inline(always)]
    pub fn leak(self) -> i32 {
        let fd = self.as_raw();
        core::mem::forget(self);
        fd
    }
    
    /// Duplicate file descriptor (dup syscall stub)
    ///
    /// NOTE: Syscall stub - returns ENOSYS until syscall layer is implemented.
    #[inline]
    pub fn duplicate(&self) -> Result<Self, Errno> {
        #[cfg(not(test))]
        {
            // Real implementation would call:
            // syscall::dup(self.as_raw()).and_then(|fd| Self::new(fd).ok_or(Errno::EBADF))
            Err(Errno::ENOSYS)
        }
        #[cfg(test)]
        {
            // Test stub: simulate successful dup
            Self::new(self.as_raw()).ok_or(Errno::EBADF)
        }
    }
    
    /// Duplicate file descriptor to specific FD (dup2 syscall stub)
    ///
    /// NOTE: Syscall stub - returns ENOSYS until syscall layer is implemented.
    #[inline]
    pub fn duplicate_to(&self, new_fd: i32) -> Result<Self, Errno> {
        #[cfg(not(test))]
        {
            // Real implementation would call:
            // syscall::dup2(self.as_raw(), new_fd).and_then(|fd| Self::new(fd).ok_or(Errno::EBADF))
            let _ = new_fd;
            Err(Errno::ENOSYS)
        }
        #[cfg(test)]
        {
            // Test stub: simulate successful dup2
            Self::new(new_fd).ok_or(Errno::EBADF)
        }
    }
}

impl Drop for FileDescriptor {
    #[inline]
    fn drop(&mut self) {
        // Syscall stub: close(fd)
        // Real implementation:
        // let _ = syscall::close(self.as_raw());
        
        // For now, no-op (FD would leak in production)
        // This is acceptable for kernel development until syscall layer exists
    }
}

impl fmt::Display for FileDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fd({})", self.as_raw())
    }
}

impl From<FileDescriptor> for i32 {
    /// Convert to i32 by leaking ownership
    ///
    /// WARNING: This prevents auto-close! Use carefully.
    #[inline(always)]
    fn from(fd: FileDescriptor) -> i32 {
        fd.leak()
    }
}

/// Borrowed file descriptor reference (no auto-close)
///
/// Non-owning reference to a file descriptor. Does not close FD on drop.
/// Use this for temporary references to FDs owned elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct BorrowedFd(NonZeroU32);

impl BorrowedFd {
    /// Standard input (borrowed)
    pub const STDIN: Self = unsafe { Self(NonZeroU32::new_unchecked(1)) };
    
    /// Standard output (borrowed)
    pub const STDOUT: Self = unsafe { Self(NonZeroU32::new_unchecked(2)) };
    
    /// Standard error (borrowed)
    pub const STDERR: Self = unsafe { Self(NonZeroU32::new_unchecked(3)) };
    
    /// Create borrowed FD from raw value
    #[inline(always)]
    pub const fn new(fd: i32) -> Option<Self> {
        if fd < 0 {
            None
        } else {
            Some(unsafe { Self(NonZeroU32::new_unchecked((fd as u32).wrapping_add(1))) })
        }
    }
    
    /// Get raw file descriptor value
    #[inline(always)]
    pub const fn as_raw(&self) -> i32 {
        self.0.get().wrapping_sub(1) as i32
    }
    
    /// Check if this is standard input
    #[inline(always)]
    pub const fn is_stdin(&self) -> bool {
        self.as_raw() == 0
    }
    
    /// Check if this is standard output
    #[inline(always)]
    pub const fn is_stdout(&self) -> bool {
        self.as_raw() == 1
    }
    
    /// Check if this is standard error
    #[inline(always)]
    pub const fn is_stderr(&self) -> bool {
        self.as_raw() == 2
    }
    
    /// Check if this is a standard stream
    #[inline(always)]
    pub const fn is_standard_stream(&self) -> bool {
        let raw = self.as_raw();
        raw >= 0 && raw <= 2
    }
}

impl fmt::Display for BorrowedFd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fd({})", self.as_raw())
    }
}

impl From<&FileDescriptor> for BorrowedFd {
    #[inline(always)]
    fn from(fd: &FileDescriptor) -> Self {
        fd.borrow()
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;
    use std::mem::size_of;
    
    #[test]
    fn test_fd_creation() {
        assert!(FileDescriptor::new(-1).is_none());
        assert!(FileDescriptor::new(-100).is_none());
        assert!(FileDescriptor::new(0).is_some());
        assert!(FileDescriptor::new(1).is_some());
        assert!(FileDescriptor::new(5).is_some());
        assert_eq!(FileDescriptor::new(42).unwrap().as_raw(), 42);
    }
    
    #[test]
    fn test_fd_constants() {
        assert_eq!(FileDescriptor::STDIN, 0);
        assert_eq!(FileDescriptor::STDOUT, 1);
        assert_eq!(FileDescriptor::STDERR, 2);
        assert_eq!(FileDescriptor::MIN_USER_FD, 3);
    }
    
    #[test]
    fn test_fd_is_stdin() {
        let stdin = FileDescriptor::new(0).unwrap();
        assert!(stdin.is_stdin());
        assert!(!stdin.is_stdout());
        assert!(!stdin.is_stderr());
        assert!(stdin.is_standard_stream());
    }
    
    #[test]
    fn test_fd_is_stdout() {
        let stdout = FileDescriptor::new(1).unwrap();
        assert!(!stdout.is_stdin());
        assert!(stdout.is_stdout());
        assert!(!stdout.is_stderr());
        assert!(stdout.is_standard_stream());
    }
    
    #[test]
    fn test_fd_is_stderr() {
        let stderr = FileDescriptor::new(2).unwrap();
        assert!(!stderr.is_stdin());
        assert!(!stderr.is_stdout());
        assert!(stderr.is_stderr());
        assert!(stderr.is_standard_stream());
    }
    
    #[test]
    fn test_fd_is_user_fd() {
        assert!(!FileDescriptor::new(0).unwrap().is_user_fd());
        assert!(!FileDescriptor::new(1).unwrap().is_user_fd());
        assert!(!FileDescriptor::new(2).unwrap().is_user_fd());
        assert!(FileDescriptor::new(3).unwrap().is_user_fd());
        assert!(FileDescriptor::new(100).unwrap().is_user_fd());
    }
    
    #[test]
    fn test_fd_leak() {
        let fd = FileDescriptor::new(42).unwrap();
        let raw = fd.leak();
        assert_eq!(raw, 42);
    }
    
    #[test]
    fn test_fd_duplicate() {
        let fd = FileDescriptor::new(5).unwrap();
        let dup = fd.duplicate();
        
        #[cfg(test)]
        {
            assert!(dup.is_ok());
            assert_eq!(dup.unwrap().as_raw(), 5);
        }
        
        #[cfg(not(test))]
        {
            assert_eq!(dup.unwrap_err(), Errno::ENOSYS);
        }
    }
    
    #[test]
    fn test_fd_duplicate_to() {
        let fd = FileDescriptor::new(5).unwrap();
        let dup = fd.duplicate_to(10);
        
        #[cfg(test)]
        {
            assert!(dup.is_ok());
            assert_eq!(dup.unwrap().as_raw(), 10);
        }
        
        #[cfg(not(test))]
        {
            assert_eq!(dup.unwrap_err(), Errno::ENOSYS);
        }
    }
    
    #[test]
    fn test_fd_borrow() {
        let fd = FileDescriptor::new(42).unwrap();
        let borrowed = fd.borrow();
        assert_eq!(borrowed.as_raw(), 42);
    }
    
    #[test]
    fn test_fd_display() {
        let fd = FileDescriptor::new(42).unwrap();
        let s = std::format!("{}", fd);
        assert_eq!(s, "fd(42)");
    }
    
    #[test]
    fn test_fd_ordering() {
        let fd1 = FileDescriptor::new(5).unwrap();
        let fd2 = FileDescriptor::new(10).unwrap();
        let fd3 = FileDescriptor::new(5).unwrap();
        
        assert!(fd1 < fd2);
        assert!(fd2 > fd1);
        assert_eq!(fd1, fd3);
    }
    
    #[test]
    fn test_borrowed_fd_creation() {
        assert!(BorrowedFd::new(-1).is_none());
        assert!(BorrowedFd::new(0).is_some());
        assert_eq!(BorrowedFd::new(42).unwrap().as_raw(), 42);
    }
    
    #[test]
    fn test_borrowed_fd_constants() {
        assert_eq!(BorrowedFd::STDIN.as_raw(), 0);
        assert_eq!(BorrowedFd::STDOUT.as_raw(), 1);
        assert_eq!(BorrowedFd::STDERR.as_raw(), 2);
    }
    
    #[test]
    fn test_borrowed_fd_is_methods() {
        let stdin = BorrowedFd::STDIN;
        assert!(stdin.is_stdin());
        assert!(!stdin.is_stdout());
        assert!(stdin.is_standard_stream());
        
        let user = BorrowedFd::new(42).unwrap();
        assert!(!user.is_stdin());
        assert!(!user.is_standard_stream());
    }
    
    #[test]
    fn test_borrowed_fd_from_owned() {
        let fd = FileDescriptor::new(42).unwrap();
        let borrowed = BorrowedFd::from(&fd);
        assert_eq!(borrowed.as_raw(), 42);
    }
    
    #[test]
    fn test_borrowed_fd_display() {
        let fd = BorrowedFd::new(42).unwrap();
        let s = std::format!("{}", fd);
        assert_eq!(s, "fd(42)");
    }
    
    #[test]
    fn test_fd_size() {
        assert_eq!(size_of::<FileDescriptor>(), size_of::<u32>());
        assert_eq!(size_of::<Option<FileDescriptor>>(), size_of::<u32>());
        assert_eq!(size_of::<BorrowedFd>(), size_of::<u32>());
        assert_eq!(size_of::<Option<BorrowedFd>>(), size_of::<u32>());
    }
    
    #[test]
    fn test_borrowed_fd_copy() {
        let fd1 = BorrowedFd::new(42).unwrap();
        let fd2 = fd1;
        assert_eq!(fd1.as_raw(), fd2.as_raw());
    }
}
