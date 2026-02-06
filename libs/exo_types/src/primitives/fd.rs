//! File Descriptor type
//!
//! Type-safe file descriptor with RAII support and validation.

use core::fmt;

/// Standard input file descriptor
pub const STDIN: Fd = Fd(0);

/// Standard output file descriptor
pub const STDOUT: Fd = Fd(1);

/// Standard error file descriptor
pub const STDERR: Fd = Fd(2);

/// Invalid/closed file descriptor sentinel
pub const INVALID_FD: Fd = Fd(-1);

/// File Descriptor
///
/// Type-safe wrapper around file descriptors.
/// On Unix-like systems, FDs are non-negative integers (except -1 for invalid).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Fd(i32);

impl Fd {
    /// Create a new file descriptor with validation
    ///
    /// # Panics
    /// Panics in debug mode if FD is invalid (< -1)
    #[inline(always)]
    pub const fn new(fd: i32) -> Self {
        debug_assert!(fd >= -1, "File descriptor must be >= -1");
        Fd(fd)
    }

    /// Create FD without validation (unsafe)
    ///
    /// # Safety
    /// Caller must ensure FD is valid
    #[inline(always)]
    pub const unsafe fn new_unchecked(fd: i32) -> Self {
        Fd(fd)
    }

    /// Try to create FD, returning None if invalid
    #[inline(always)]
    pub const fn try_new(fd: i32) -> Option<Self> {
        if fd >= -1 {
            Some(Fd(fd))
        } else {
            None
        }
    }

    /// Get raw FD value
    #[inline(always)]
    pub const fn as_i32(self) -> i32 {
        self.0
    }

    /// Get as usize (for array indexing)
    ///
    /// # Panics
    /// Panics if FD is negative (invalid)
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        debug_assert!(self.0 >= 0, "Cannot convert negative FD to usize");
        self.0 as usize
    }

    /// Check if FD is valid (>= 0)
    #[inline(always)]
    pub const fn is_valid(self) -> bool {
        self.0 >= 0
    }

    /// Check if FD is invalid (-1)
    #[inline(always)]
    pub const fn is_invalid(self) -> bool {
        self.0 == -1
    }

    /// Check if this is stdin (0)
    #[inline(always)]
    pub const fn is_stdin(self) -> bool {
        self.0 == 0
    }

    /// Check if this is stdout (1)
    #[inline(always)]
    pub const fn is_stdout(self) -> bool {
        self.0 == 1
    }

    /// Check if this is stderr (2)
    #[inline(always)]
    pub const fn is_stderr(self) -> bool {
        self.0 == 2
    }

    /// Check if this is a standard stream (0, 1, or 2)
    #[inline(always)]
    pub const fn is_standard(self) -> bool {
        self.0 >= 0 && self.0 <= 2
    }

    /// Create an invalid FD marker
    #[inline(always)]
    pub const fn invalid() -> Self {
        INVALID_FD
    }
}

impl fmt::Debug for Fd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            0 => f.write_str("Fd(STDIN)"),
            1 => f.write_str("Fd(STDOUT)"),
            2 => f.write_str("Fd(STDERR)"),
            -1 => f.write_str("Fd(INVALID)"),
            n => f.debug_tuple("Fd").field(&n).finish(),
        }
    }
}

impl fmt::Display for Fd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<i32> for Fd {
    type Error = ();

    #[inline(always)]
    fn try_from(fd: i32) -> Result<Self, Self::Error> {
        Self::try_new(fd).ok_or(())
    }
}

impl From<Fd> for i32 {
    #[inline(always)]
    fn from(fd: Fd) -> Self {
        fd.0
    }
}

impl Default for Fd {
    #[inline]
    fn default() -> Self {
        INVALID_FD
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;
    use std::mem::size_of;

    #[test]
    fn test_fd_creation() {
        let fd = Fd::new(3);
        assert_eq!(fd.as_i32(), 3);

        assert_eq!(Fd::new(0), STDIN);
        assert_eq!(Fd::new(1), STDOUT);
        assert_eq!(Fd::new(2), STDERR);
        assert_eq!(Fd::new(-1), INVALID_FD);
    }

    #[test]
    fn test_fd_try_new() {
        assert!(Fd::try_new(0).is_some());
        assert!(Fd::try_new(10).is_some());
        assert!(Fd::try_new(-1).is_some());
        assert!(Fd::try_new(-2).is_none());
        assert!(Fd::try_new(-100).is_none());
    }

    #[test]
    fn test_fd_validity() {
        assert!(STDIN.is_valid());
        assert!(STDOUT.is_valid());
        assert!(STDERR.is_valid());
        assert!(Fd::new(10).is_valid());
        assert!(!INVALID_FD.is_valid());
        assert!(INVALID_FD.is_invalid());
    }

    #[test]
    fn test_fd_standard_checks() {
        assert!(STDIN.is_stdin());
        assert!(!STDIN.is_stdout());
        assert!(!STDIN.is_stderr());
        assert!(STDIN.is_standard());

        assert!(!STDOUT.is_stdin());
        assert!(STDOUT.is_stdout());
        assert!(!STDOUT.is_stderr());
        assert!(STDOUT.is_standard());

        assert!(!STDERR.is_stdin());
        assert!(!STDERR.is_stdout());
        assert!(STDERR.is_stderr());
        assert!(STDERR.is_standard());

        let fd = Fd::new(3);
        assert!(!fd.is_stdin());
        assert!(!fd.is_stdout());
        assert!(!fd.is_stderr());
        assert!(!fd.is_standard());
    }

    #[test]
    fn test_fd_conversions() {
        let fd = Fd::new(5);

        assert_eq!(fd.as_i32(), 5);
        assert_eq!(fd.as_usize(), 5);

        let i32_val: i32 = fd.into();
        assert_eq!(i32_val, 5);

        assert_eq!(Fd::try_from(5).unwrap(), fd);
        assert!(Fd::try_from(-2).is_err());
    }

    #[test]
    fn test_fd_display() {
        assert_eq!(std::format!("{}", Fd::new(10)), "10");

        let debug = std::format!("{:?}", STDIN);
        assert!(debug.contains("STDIN"));

        let debug = std::format!("{:?}", STDOUT);
        assert!(debug.contains("STDOUT"));

        let debug = std::format!("{:?}", STDERR);
        assert!(debug.contains("STDERR"));

        let debug = std::format!("{:?}", INVALID_FD);
        assert!(debug.contains("INVALID"));
    }

    #[test]
    fn test_fd_default() {
        let fd: Fd = Default::default();
        assert_eq!(fd, INVALID_FD);
        assert!(!fd.is_valid());
    }

    #[test]
    fn test_fd_ordering() {
        let fd1 = Fd::new(3);
        let fd2 = Fd::new(5);
        let fd3 = Fd::new(3);

        assert!(fd1 < fd2);
        assert!(fd2 > fd1);
        assert_eq!(fd1, fd3);
        assert_ne!(fd1, fd2);
    }

    #[test]
    fn test_fd_size() {
        assert_eq!(size_of::<Fd>(), size_of::<i32>());
    }

    #[test]
    fn test_fd_invalid() {
        let invalid = Fd::invalid();
        assert_eq!(invalid, INVALID_FD);
        assert!(!invalid.is_valid());
        assert!(invalid.is_invalid());
    }
}
