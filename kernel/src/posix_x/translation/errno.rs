//! Error Code Translation (POSIX errno ↔ Exo-OS)
//!
//! Complete mapping of POSIX error codes to Exo-OS error types

use crate::fs::FsError;
use crate::memory::MemoryError;

/// POSIX errno codes
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Errno {
    /// Success (not actually an error)
    ESUCCESS = 0,
    /// Operation not permitted
    EPERM = 1,
    /// No such file or directory
    ENOENT = 2,
    /// No such process
    ESRCH = 3,
    /// Interrupted system call
    EINTR = 4,
    /// I/O error
    EIO = 5,
    /// No such device or address
    ENXIO = 6,
    /// Argument list too long
    E2BIG = 7,
    /// Exec format error
    ENOEXEC = 8,
    /// Bad file descriptor
    EBADF = 9,
    /// No child processes
    ECHILD = 10,
    /// Resource temporarily unavailable
    EAGAIN = 11,
    /// Cannot allocate memory
    ENOMEM = 12,
    /// Permission denied
    EACCES = 13,
    /// Bad address
    EFAULT = 14,
    /// Block device required
    ENOTBLK = 15,
    /// Device or resource busy
    EBUSY = 16,
    /// File exists
    EEXIST = 17,
    /// Invalid cross-device link
    EXDEV = 18,
    /// No such device
    ENODEV = 19,
    /// Not a directory
    ENOTDIR = 20,
    /// Is a directory
    EISDIR = 21,
    /// Invalid argument
    EINVAL = 22,
    /// Too many open files in system
    ENFILE = 23,
    /// Too many open files
    EMFILE = 24,
    /// Inappropriate ioctl for device
    ENOTTY = 25,
    /// Text file busy
    ETXTBSY = 26,
    /// File too large
    EFBIG = 27,
    /// No space left on device
    ENOSPC = 28,
    /// Illegal seek
    ESPIPE = 29,
    /// Read-only file system
    EROFS = 30,
    /// Too many links
    EMLINK = 31,
    /// Broken pipe
    EPIPE = 32,
    /// Numerical argument out of domain
    EDOM = 33,
    /// Numerical result out of range
    ERANGE = 34,
    /// Resource deadlock avoided
    EDEADLK = 35,
    /// File name too long
    ENAMETOOLONG = 36,
    /// No locks available
    ENOLCK = 37,
    /// Function not implemented
    ENOSYS = 38,
    /// Directory not empty
    ENOTEMPTY = 39,
    /// Too many levels of symbolic links
    ELOOP = 40,
}

impl Errno {
    /// Convert to i32 (for syscall return)
    pub fn as_i32(self) -> i32 {
        self as i32
    }

    /// Convert from i32
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::ESUCCESS),
            1 => Some(Self::EPERM),
            2 => Some(Self::ENOENT),
            9 => Some(Self::EBADF),
            12 => Some(Self::ENOMEM),
            13 => Some(Self::EACCES),
            14 => Some(Self::EFAULT),
            22 => Some(Self::EINVAL),
            _ => None,
        }
    }
}

/// Convert MemoryError to errno
pub fn memory_error_to_errno(err: MemoryError) -> Errno {
    match err {
        MemoryError::OutOfMemory => Errno::ENOMEM,
        MemoryError::InvalidAddress => Errno::EFAULT,
        MemoryError::PermissionDenied => Errno::EACCES,
        MemoryError::AlreadyMapped => Errno::EEXIST,
        MemoryError::NotMapped => Errno::EINVAL,
        _ => Errno::EINVAL,
    }
}

/// Convert FsError to errno
pub fn fs_error_to_errno(err: FsError) -> Errno {
    match err {
        FsError::NotFound => Errno::ENOENT,
        FsError::PermissionDenied => Errno::EACCES,
        FsError::AlreadyExists => Errno::EEXIST,
        FsError::InvalidFd => Errno::EBADF,
        FsError::TooManyFiles => Errno::EMFILE,
        // FsError::NameTooLong => Errno::ENAMETOOLONG,
        // FsError::ReadOnlyFs => Errno::EROFS,
        // FsError::NoSpace => Errno::ENOSPC,
        _ => Errno::EIO,
    }
}

/// Get error string description
pub fn strerror(errno: Errno) -> &'static str {
    match errno {
        Errno::ESUCCESS => "Success",
        Errno::EPERM => "Operation not permitted",
        Errno::ENOENT => "No such file or directory",
        Errno::EBADF => "Bad file descriptor",
        Errno::ENOMEM => "Cannot allocate memory",
        Errno::EACCES => "Permission denied",
        Errno::EFAULT => "Bad address",
        Errno::EINVAL => "Invalid argument",
        Errno::EMFILE => "Too many open files",
        Errno::ENOENT => "No such file or directory",
        _ => "Unknown error",
    }
}
