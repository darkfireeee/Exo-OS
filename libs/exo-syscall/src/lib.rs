//! `exo-syscall` — Couche d'appels système typée et `#![no_std]` pour Exo-OS.
//!
//! Fournit des primitives `Result<usize, SysError>` au-dessus de `exo_syscall_abi`,
//! utilisées par `exo-alloc`, `exo-rt`, `exo-libc` et `std::sys::exo_os`.

#![no_std]
#![forbid(unsafe_op_in_unsafe_fn)]

pub use exo_syscall_abi as abi;
pub use exo_types::epoll::EpollEventAbi as EpollEvent;

pub mod exofs;
pub mod memory;
pub mod net;
pub mod phoenix;
pub mod process;

/// Code d'erreur système POSIX / Exo-OS positif (`1..=4095`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SysError(pub i32);

impl SysError {
    pub const EPERM: Self = Self(1);
    pub const ENOENT: Self = Self(2);
    pub const ESRCH: Self = Self(3);
    pub const EINTR: Self = Self(4);
    pub const EIO: Self = Self(5);
    pub const ENXIO: Self = Self(6);
    pub const E2BIG: Self = Self(7);
    pub const ENOEXEC: Self = Self(8);
    pub const EBADF: Self = Self(9);
    pub const ECHILD: Self = Self(10);
    pub const EAGAIN: Self = Self(11);
    pub const ENOMEM: Self = Self(12);
    pub const EACCES: Self = Self(13);
    pub const EFAULT: Self = Self(14);
    pub const EBUSY: Self = Self(16);
    pub const EEXIST: Self = Self(17);
    pub const EXDEV: Self = Self(18);
    pub const ENODEV: Self = Self(19);
    pub const ENOTDIR: Self = Self(20);
    pub const EISDIR: Self = Self(21);
    pub const EINVAL: Self = Self(22);
    pub const ENFILE: Self = Self(23);
    pub const EMFILE: Self = Self(24);
    pub const ENOTTY: Self = Self(25);
    pub const EFBIG: Self = Self(27);
    pub const ENOSPC: Self = Self(28);
    pub const ESPIPE: Self = Self(29);
    pub const EROFS: Self = Self(30);
    pub const EPIPE: Self = Self(32);
    pub const ERANGE: Self = Self(34);
    pub const ENAMETOOLONG: Self = Self(36);
    pub const ENOSYS: Self = Self(38);
    pub const ENOTEMPTY: Self = Self(39);
    pub const ELOOP: Self = Self(40);
    pub const ENOTSOCK: Self = Self(88);
    pub const EAFNOSUPPORT: Self = Self(97);
    pub const EADDRINUSE: Self = Self(98);
    pub const EADDRNOTAVAIL: Self = Self(99);
    pub const ENETUNREACH: Self = Self(101);
    pub const ECONNABORTED: Self = Self(103);
    pub const ECONNRESET: Self = Self(104);
    pub const ENOBUFS: Self = Self(105);
    pub const EISCONN: Self = Self(106);
    pub const ENOTCONN: Self = Self(107);
    pub const ETIMEDOUT: Self = Self(110);
    pub const ECONNREFUSED: Self = Self(111);
    pub const EINPROGRESS: Self = Self(115);

    #[inline(always)]
    pub const fn errno(self) -> i32 {
        self.0
    }
}

pub type SysResult<T = usize> = Result<T, SysError>;

/// Convertit un retour brut de syscall (`i64`) en `SysResult<usize>`.
#[inline(always)]
pub const fn decode_ret(ret: i64) -> SysResult<usize> {
    if ret < 0 && ret >= -4095 {
        Err(SysError((-ret) as i32))
    } else {
        Ok(ret as usize)
    }
}
