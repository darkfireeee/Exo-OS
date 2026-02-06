// libs/exo_std/src/syscall/io.rs
//! Syscalls relatifs aux I/O

use super::{syscall1, syscall3, SyscallNumber, check_syscall_result};
use crate::Result;

/// Descripteur de fichier
pub type Fd = i32;

/// Flags pour open
#[repr(usize)]
pub enum OpenFlags {
    ReadOnly = 0,
    WriteOnly = 1,
    ReadWrite = 2,
    Create = 64,
    Truncate = 512,
    Append = 1024,
}

/// Whence pour seek
#[repr(usize)]
pub enum SeekWhence {
    Start = 0,
    Current = 1,
    End = 2,
}

/// Lit des données depuis un fd
///
/// # Safety
/// - `fd` doit être un descripteur valide
/// - `buf` doit pointer vers un buffer valide de taille `count`
#[inline]
pub unsafe fn read(fd: Fd, buf: *mut u8, count: usize) -> Result<usize> {
    let ret = syscall3(
        SyscallNumber::Read,
        fd as usize,
        buf as usize,
        count,
    );
    check_syscall_result(ret)
}

/// Wrapper sécurisé pour read avec slice
#[inline]
pub fn read_slice(fd: Fd, buf: &mut [u8]) -> Result<usize> {
    unsafe { read(fd, buf.as_mut_ptr(), buf.len()) }
}

/// Écrit des données vers un fd
///
/// # Safety
/// - `fd` doit être un descripteur valide
/// - `buf` doit pointer vers un buffer valide de taille `count`
#[inline]
pub unsafe fn write(fd: Fd, buf: *const u8, count: usize) -> Result<usize> {
    let ret = syscall3(
        SyscallNumber::Write,
        fd as usize,
        buf as usize,
        count,
    );
    check_syscall_result(ret)
}

/// Wrapper sécurisé pour write avec slice
#[inline]
pub fn write_slice(fd: Fd, buf: &[u8]) -> Result<usize> {
    unsafe { write(fd, buf.as_ptr(), buf.len()) }
}

/// Ouvre un fichier
///
/// # Safety
/// - `path` doit être une chaîne C valide
#[inline]
pub unsafe fn open(path: *const u8, flags: usize, mode: usize) -> Result<Fd> {
    let ret = syscall3(SyscallNumber::Open, path as usize, flags, mode);
    check_syscall_result(ret).map(|fd| fd as Fd)
}

/// Ferme un descripteur
///
/// # Safety
/// - `fd` doit être un descripteur valide
#[inline]
pub unsafe fn close(fd: Fd) -> Result<()> {
    let ret = syscall1(SyscallNumber::Close, fd as usize);
    check_syscall_result(ret).map(|_| ())
}

/// Repositionne le curseur dans un fichier
///
/// # Safety
/// - `fd` doit être un descripteur valide
#[inline]
pub unsafe fn seek(fd: Fd, offset: isize, whence: SeekWhence) -> Result<usize> {
    let ret = syscall3(
        SyscallNumber::Seek,
        fd as usize,
        offset as usize,
        whence as usize,
    );
    check_syscall_result(ret)
}

/// Contrôle d'un périphérique
///
/// # Safety
/// - `fd` doit être un descripteur valide
/// - `arg` doit être valide selon la requête
#[inline]
pub unsafe fn ioctl(fd: Fd, request: usize, arg: usize) -> Result<isize> {
    let ret = syscall3(SyscallNumber::Ioctl, fd as usize, request, arg);
    check_syscall_result(ret).map(|v| v as isize)
}

/// Descripteur pour stdin
pub const STDIN_FD: Fd = 0;
/// Descripteur pour stdout
pub const STDOUT_FD: Fd = 1;
/// Descripteur pour stderr
pub const STDERR_FD: Fd = 2;
