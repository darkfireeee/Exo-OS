//! Single errno mapping point for the translation layer.

use exo_syscall_abi as abi;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranslationError {
    Invalid,
    Fault,
    NotFound,
    Access,
    Busy,
    NoSpace,
    NoMemory,
    NotImplemented,
}

pub const fn to_errno(err: TranslationError) -> i64 {
    match err {
        TranslationError::Invalid => abi::EINVAL,
        TranslationError::Fault => abi::EFAULT,
        TranslationError::NotFound => abi::ENOENT,
        TranslationError::Access => abi::EACCES,
        TranslationError::Busy => abi::EBUSY,
        TranslationError::NoSpace => abi::ENOSPC,
        TranslationError::NoMemory => abi::ENOMEM,
        TranslationError::NotImplemented => abi::ENOSYS,
    }
}
