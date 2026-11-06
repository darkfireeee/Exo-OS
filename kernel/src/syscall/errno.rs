//! # syscall/errno.rs — Mapping KernelError / ExofsError → POSIX errno
//!
//! ## Rules (ERRNO-01 to ERRNO-03)
//!
//! - **ERRNO-01**: `rax = -(errno as i64) as u64`. Never a positive errno in rax.
//! - **ERRNO-02**: FORBIDDEN to return -1 with errno=0.
//! - **ERRNO-03**: This module MUST cover ALL variants of KernelError and ExofsError.
//!
//! ## BUG ERRNO-MISSING — fixed here
//! The old code only listed 3 cases (NoSpace, NotFound, Denied).
//! This module covers the 15+ required variants.
//!
//! ## Usage
//! ```rust
//! use crate::syscall::errno::kernel_err_to_errno;
//! let retval: i64 = kernel_err_to_errno(KernelError::NotFound);  // → -2 (ENOENT)
//! ```

// ─────────────────────────────────────────────────────────────────────────────
// Import of kernel error types
// ─────────────────────────────────────────────────────────────────────────────

use crate::fs::exofs::core::ExofsError;

// ─────────────────────────────────────────────────────────────────────────────
// POSIX errno codes — NEGATIVE values (rax = -errno)
// ─────────────────────────────────────────────────────────────────────────────

/// Operation not permitted (EPERM)
pub const EPERM: i64 = -1;
/// No such file or directory (ENOENT)
pub const ENOENT: i64 = -2;
/// No such process (ESRCH)
pub const ESRCH: i64 = -3;
/// Interrupted system call during sleep (EINTR)
pub const EINTR: i64 = -4;
/// No child processes (ECHILD)
pub const ECHILD: i64 = -10;
/// Physical I/O error (EIO)
pub const EIO: i64 = -5;
/// Argument list too long — len > MAX (E2BIG)
pub const E2BIG: i64 = -7;
/// Message too long for IPC buffer/protocol (EMSGSIZE)
pub const EMSGSIZE: i64 = -90;
/// Bad file descriptor (EBADF)
pub const EBADF: i64 = -9;
/// Resource temporarily unavailable / non-blocking (EAGAIN)
pub const EAGAIN: i64 = -11;
/// Out of memory (ENOMEM)
pub const ENOMEM: i64 = -12;
/// Permission denied — capability Denied (EACCES)
pub const EACCES: i64 = -13;
/// Bad userspace address (EFAULT)
pub const EFAULT: i64 = -14;
/// Device or resource busy (EBUSY)
pub const EBUSY: i64 = -16;
/// File/object already exists (EEXIST)
pub const EEXIST: i64 = -17;
/// Not a directory (ENOTDIR)
pub const ENOTDIR: i64 = -20;
/// Is a directory (EISDIR)
pub const EISDIR: i64 = -21;
/// Invalid argument — len=0, null ptr, invalid flags (EINVAL)
pub const EINVAL: i64 = -22;
/// Too many open files (EMFILE)
pub const EMFILE: i64 = -24;
/// No space left on device (ENOSPC)
pub const ENOSPC: i64 = -28;
/// Illegal seek (pipe, socket, FIFO) (ESPIPE)
pub const ESPIPE: i64 = -29;
/// Result out of range — overflow (ERANGE)
pub const ERANGE: i64 = -34;
/// Function not implemented (ENOSYS)
pub const ENOSYS: i64 = -38;
/// Invalid data format (EBADMSG)
pub const EBADMSG: i64 = -74;
/// Arithmetic overflow (EOVERFLOW)
pub const EOVERFLOW: i64 = -75;
/// Operation not supported (ENOTSUP / EOPNOTSUPP)
pub const ENOTSUP: i64 = -95;
/// Directory not empty (ENOTEMPTY)
pub const ENOTEMPTY: i64 = -39;
/// Connection timed out (ETIMEDOUT)
pub const ETIMEDOUT: i64 = -110;
/// Capability quota exceeded (EDQUOT)
pub const EDQUOT: i64 = -122;
/// Key has been revoked (EKEYREV — custom ExoOS)
pub const EKEYREV: i64 = -126;
/// Incompatible disk format (EPROTO)
pub const EPROTO: i64 = -71;
/// Non-existent epoch (ENOEPOCH — custom ExoOS)
pub const ENOEPOCH: i64 = -130;
/// GC table full (EGCFULL — custom ExoOS)
pub const EGCFULL: i64 = -131;
/// Epoch table full (EEPOCHFULL — custom ExoOS)
pub const EEPOCHFULL: i64 = -133;
/// NVMe commit error (ECOMMIT — custom ExoOS)
pub const ECOMMIT: i64 = -134;

// ─────────────────────────────────────────────────────────────────────────────
// KernelError — defined locally (mirror of kernel/process/error enum)
// ─────────────────────────────────────────────────────────────────────────────

/// Generic kernel errors — covers the 15 variants required by ERRNO-03.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelError {
    /// Object or path not found.
    NotFound,
    /// Out of memory — never panic, return ENOMEM.
    NoMemory,
    /// Invalid argument (len=0, null ptr, invalid flags).
    InvalidArg,
    /// Disk space exhausted.
    NoSpace,
    /// Permission denied (capability Denied).
    AccessDenied,
    /// Corruption detected (magic, checksum).
    Corrupt,
    /// Non-executable object (ObjectKind != Code).
    NotExecutable,
    /// Operation timed out.
    TimedOut,
    /// Non-blocking + no data (O_NONBLOCK).
    WouldBlock,
    /// len > MAX (E2BIG).
    TooBig,
    /// Signal received during sleep.
    Interrupted,
    /// Syscall not implemented.
    NotSupported,
    /// Capability quota exceeded.
    QuotaExceeded,
    /// Incompatible version format.
    VersionMismatch,
    /// Resource already exists.
    AlreadyExists,
    /// Bad file descriptor.
    BadFd,
    /// Physical I/O error.
    IoError,
    /// Resource busy.
    Busy,
}

// ─────────────────────────────────────────────────────────────────────────────
// Mapping KernelError → POSIX errno (ERRNO-03: COMPLETE coverage)
// ─────────────────────────────────────────────────────────────────────────────

/// Converts a `KernelError` into a negative POSIX errno code.
///
/// RULE ERRNO-01: the returned value is ALWAYS negative.
/// RULE ERRNO-03: ALL variants are covered.
#[inline]
pub fn kernel_err_to_errno(e: KernelError) -> i64 {
    match e {
        KernelError::NotFound => ENOENT,
        KernelError::NoMemory => ENOMEM,
        KernelError::InvalidArg => EINVAL,
        KernelError::NoSpace => ENOSPC,
        KernelError::AccessDenied => EACCES,
        KernelError::Corrupt => EIO,
        KernelError::NotExecutable => -8, // ENOEXEC = 8
        KernelError::TimedOut => ETIMEDOUT,
        KernelError::WouldBlock => EAGAIN,
        KernelError::TooBig => E2BIG,
        KernelError::Interrupted => EINTR,
        KernelError::NotSupported => ENOSYS,
        KernelError::QuotaExceeded => EDQUOT,
        KernelError::VersionMismatch => EPROTO,
        KernelError::AlreadyExists => EEXIST,
        KernelError::BadFd => EBADF,
        KernelError::IoError => EIO,
        KernelError::Busy => EBUSY,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Mapping ExofsError → POSIX errno (ERRNO-03: COMPLETE coverage)
// ─────────────────────────────────────────────────────────────────────────────

/// Converts an `ExofsError` into a negative POSIX errno code.
///
/// RULE ERRNO-01: the returned value is ALWAYS negative.
/// RULE ERRNO-03: ALL ExofsError variants are mapped.
#[inline]
pub fn exofs_err_to_errno(e: ExofsError) -> i64 {
    match e {
        ExofsError::NoMemory => ENOMEM,
        ExofsError::NoSpace => ENOSPC,
        ExofsError::IoError => EIO,
        ExofsError::PartialWrite => EIO,
        ExofsError::OffsetOverflow => EOVERFLOW,
        ExofsError::InvalidMagic => EBADMSG,
        ExofsError::ChecksumMismatch => EBADMSG,
        ExofsError::IncompatibleVersion => EPROTO,
        ExofsError::CorruptedStructure => EBADMSG,
        ExofsError::CorruptedChain => EBADMSG,
        ExofsError::ObjectNotFound => ENOENT,
        ExofsError::BlobNotFound => ENOENT,
        ExofsError::ObjectAlreadyExists => EEXIST,
        ExofsError::WrongObjectKind => EINVAL,
        ExofsError::WrongObjectClass => EINVAL,
        ExofsError::InvalidArgument => EINVAL,
        // FIX: old names replaced by correct variants of ExofsError
        ExofsError::PermissionDenied => EACCES,
        ExofsError::SecretBlobIdLeakPrevented => EACCES,
        ExofsError::QuotaExceeded => EDQUOT,
        ExofsError::NoValidEpoch => ENOEPOCH,
        ExofsError::EpochFull => EEPOCHFULL,
        ExofsError::GcQueueFull => EGCFULL,
        ExofsError::CommitInProgress => ECOMMIT,
        ExofsError::Concurrency => EBUSY,
        ExofsError::NotSupported => ENOTSUP,
        ExofsError::PathTooLong => ERANGE,
        ExofsError::TooManySymlinks => ERANGE,
        ExofsError::NotADirectory => ENOTDIR,
        ExofsError::DirectoryNotEmpty => ENOTEMPTY,
        ExofsError::InvalidPathComponent => EINVAL,
        ExofsError::InvalidObjectKind => EINVAL,
        ExofsError::InvalidObjectClass => EINVAL,
        ExofsError::ObjectTooLarge => EOVERFLOW,
        ExofsError::InternalError => EIO,
        ExofsError::AlreadyMounted => EEXIST,
        ExofsError::RecoveryFailed => EIO,
        ExofsError::Corrupt => EBADMSG,
        ExofsError::CorruptFilesystem => EBADMSG,
        ExofsError::BadMagic => EBADMSG,
        ExofsError::MagicMismatch => EBADMSG,
        ExofsError::BlobIdMismatch => EBADMSG,
        ExofsError::DataHashMismatch => EBADMSG,
        ExofsError::Overflow => EOVERFLOW,
        ExofsError::Underflow => EOVERFLOW,
        ExofsError::EndOfFile => EIO,
        ExofsError::UnexpectedEof => EIO,
        ExofsError::InlineTooLarge => EOVERFLOW,
        ExofsError::InvalidSize => EINVAL,
        ExofsError::ShortWrite => EIO,
        ExofsError::AlreadyExists => EEXIST,
        ExofsError::InvalidState => EINVAL,
        ExofsError::AlreadyInitialized => EBUSY,
        ExofsError::Resource => ENOMEM,
        ExofsError::NotFound => ENOENT,
        ExofsError::BufferFull => ENOSPC,
        ExofsError::IoFailed => EIO,
        ExofsError::DecompressError => EIO,
        ExofsError::Logic => EIO,
        ExofsError::Shutdown => EBUSY,
        ExofsError::TooManyPins => ERANGE,
        ExofsError::InvalidPin => EINVAL,
        ExofsError::InvalidEpochId => EINVAL,
        ExofsError::EpochOverflow => EOVERFLOW,
        ExofsError::EpochSequenceViolation => EINVAL,
        ExofsError::FutureEpoch => EINVAL,
        ExofsError::NvmeFlushFailed => EIO,
        ExofsError::RefCountUnderflow => EIO,
        ExofsError::DirtyDataLoss(_) => EIO,
        ExofsError::DiskTooSmall { .. } => ENOSPC,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Generic helper for handlers — returns i64 from Result<i64, E>
// ─────────────────────────────────────────────────────────────────────────────

/// Utility — converts `Result<i64, KernelError>` to `i64` (0/errno).
#[inline]
pub fn result_to_retval(r: Result<i64, KernelError>) -> i64 {
    match r {
        Ok(v) => v,
        Err(e) => kernel_err_to_errno(e),
    }
}

/// Utility — converts `Result<i64, ExofsError>` to `i64` (0/errno).
#[inline]
pub fn exofs_result_to_retval(r: Result<i64, ExofsError>) -> i64 {
    match r {
        Ok(v) => v,
        Err(e) => exofs_err_to_errno(e),
    }
}
