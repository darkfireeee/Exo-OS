//! Exo-OS Error Types
//!
//! Global error types used throughout the kernel

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Not found (file, thread, etc.)
    NotFound,
    /// Invalid argument
    InvalidArgument,
    /// Out of memory
    OutOfMemory,
    /// Permission denied
    PermissionDenied,
    /// Invalid address/pointer
    InvalidAddress,
    /// Operation not supported
    NotSupported,
    /// Resource busy
    Busy,
    /// I/O error
    IoError,
    /// Already exists
    AlreadyExists,
}

pub type Result<T> = core::result::Result<T, Error>;
