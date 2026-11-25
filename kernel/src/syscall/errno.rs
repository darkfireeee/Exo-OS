//! # syscall/errno.rs — Mapping `KernelError` / `ExofsError` → canonical `exo_abi::errno`

use crate::fs::exofs::core::ExofsError;
pub use exo_abi::errno::*;

/// Generic kernel errors — covers all variants required by `ERRNO-03`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelError {
    NotFound,
    NoMemory,
    InvalidArg,
    NoSpace,
    AccessDenied,
    Corrupt,
    NotExecutable,
    TimedOut,
    WouldBlock,
    TooBig,
    Interrupted,
    NotSupported,
    QuotaExceeded,
    VersionMismatch,
    AlreadyExists,
    BadFd,
    IoError,
    Busy,
}

#[inline]
pub fn kernel_err_to_errno(e: KernelError) -> i64 {
    match e {
        KernelError::NotFound => ENOENT,
        KernelError::NoMemory => ENOMEM,
        KernelError::InvalidArg => EINVAL,
        KernelError::NoSpace => ENOSPC,
        KernelError::AccessDenied => EACCES,
        KernelError::Corrupt => EIO,
        KernelError::NotExecutable => ENOEXEC,
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

#[inline]
pub fn result_to_retval(r: Result<i64, KernelError>) -> i64 {
    match r {
        Ok(v) => v,
        Err(e) => kernel_err_to_errno(e),
    }
}

#[inline]
pub fn exofs_result_to_retval(r: Result<i64, ExofsError>) -> i64 {
    match r {
        Ok(v) => v,
        Err(e) => exofs_err_to_errno(e),
    }
}
