//! POSIX errno constants owned by the VFS compatibility layer.

use exo_syscall_abi as abi;

pub const EPERM: i64 = abi::EPERM;
pub const ENOENT: i64 = abi::ENOENT;
pub const EIO: i64 = abi::EIO;
pub const EAGAIN: i64 = abi::EAGAIN;
pub const ENOMEM: i64 = abi::ENOMEM;
pub const EACCES: i64 = abi::EACCES;
pub const EFAULT: i64 = abi::EFAULT;
pub const EBUSY: i64 = abi::EBUSY;
pub const EEXIST: i64 = abi::EEXIST;
pub const ENODEV: i64 = abi::ENODEV;
pub const ENOTDIR: i64 = abi::ENOTDIR;
pub const EISDIR: i64 = abi::EISDIR;
pub const EINVAL: i64 = abi::EINVAL;
pub const EMFILE: i64 = abi::EMFILE;
pub const ENOSPC: i64 = abi::ENOSPC;
pub const EPIPE: i64 = abi::EPIPE;
pub const ENOSYS: i64 = abi::ENOSYS;
pub const ETIMEDOUT: i64 = abi::ETIMEDOUT;

pub const fn name(errno: i64) -> &'static str {
    match errno {
        EPERM => "EPERM",
        ENOENT => "ENOENT",
        EIO => "EIO",
        EAGAIN => "EAGAIN",
        ENOMEM => "ENOMEM",
        EACCES => "EACCES",
        EFAULT => "EFAULT",
        EBUSY => "EBUSY",
        EEXIST => "EEXIST",
        ENODEV => "ENODEV",
        ENOTDIR => "ENOTDIR",
        EISDIR => "EISDIR",
        EINVAL => "EINVAL",
        EMFILE => "EMFILE",
        ENOSPC => "ENOSPC",
        EPIPE => "EPIPE",
        ENOSYS => "ENOSYS",
        ETIMEDOUT => "ETIMEDOUT",
        _ => "EUNKNOWN",
    }
}
