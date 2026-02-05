<<<<<<< Updated upstream
// libs/exo_std/src/syscall/io.rs
//! Syscalls relatifs aux I/O

use super::{syscall3, syscall4, SyscallNumber, check_syscall_result};
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
=======
//! Appels système I/O

use super::{SysResult, syscall2, syscall3, SyscallId};
use crate::error::{SystemError, IoError};

/// Lit depuis un descripteur de fichier
pub unsafe fn read(fd: u32, buffer: &mut [u8]) -> Result<usize, IoError> {
    let result = syscall3(
        SyscallId::Read,
        fd as usize,
        buffer.as_mut_ptr() as usize,
        buffer.len(),
    );

    if result < 0 {
        Err(IoError::Other)
    } else {
        Ok(result as usize)
    }
}

/// Écrit vers un descripteur de fichier
pub unsafe fn write(fd: u32, buffer: &[u8]) -> Result<usize, IoError> {
    let result = syscall3(
        SyscallId::Write,
        fd as usize,
        buffer.as_ptr() as usize,
        buffer.len(),
    );

    if result < 0 {
        Err(IoError::Other)
    } else {
        Ok(result as usize)
    }
}

/// Ouvre un fichier
pub unsafe fn open(path: &str, flags: u32) -> Result<u32, IoError> {
    let result = syscall2(
        SyscallId::Open,
        path.as_ptr() as usize,
        flags as usize,
    );

    if result < 0 {
        Err(IoError::NotFound)
    } else {
        Ok(result as u32)
    }
}

/// Ferme un descripteur de fichier
pub unsafe fn close(fd: u32) -> Result<(), IoError> {
    let result = syscall2(SyscallId::Close, fd as usize, 0);

    if result < 0 {
        Err(IoError::Other)
    } else {
        Ok(())
    }
}
>>>>>>> Stashed changes
