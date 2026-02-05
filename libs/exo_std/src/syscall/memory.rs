// libs/exo_std/src/syscall/memory.rs
//! Syscalls relatifs à la mémoire

use super::{syscall2, syscall3, syscall4, SyscallNumber, check_syscall_result};
use crate::Result;

/// Protection de page mémoire
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryProtection {
    None = 0,
    Read = 1,
    Write = 2,
    Execute = 4,
    ReadWrite = 3,
    ReadExecute = 5,
    ReadWriteExecute = 7,
}

/// Flags pour mmap
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmapFlags {
    Private = 1,
    Shared = 2,
    Fixed = 4,
    Anonymous = 8,
}

/// Mappe de la mémoire
///
/// # Safety
/// - `addr` doit être aligné sur page ou 0 pour laisser le kernel choisir
/// - `length` doit être > 0
#[inline]
pub unsafe fn mmap(
    addr: usize,
    length: usize,
    prot: MemoryProtection,
    flags: usize,
) -> Result<*mut u8> {
    let ret = syscall4(
        SyscallNumber::Mmap,
        addr,
        length,
        prot as usize,
        flags,
    );
    check_syscall_result(ret).map(|addr| addr as *mut u8)
}

/// Unmap de la mémoire
///
/// # Safety
/// - `addr` doit pointer vers une région mappée
/// - `length` doit correspondre à la taille mappée
#[inline]
pub unsafe fn munmap(addr: *mut u8, length: usize) -> Result<()> {
    let ret = syscall2(SyscallNumber::Munmap, addr as usize, length);
    check_syscall_result(ret).map(|_| ())
}

/// Change la protection d'une région mémoire
///
/// # Safety
/// - `addr` doit pointer vers une région mappée
/// - `length` doit correspondre à la taille de la région
#[inline]
pub unsafe fn mprotect(addr: *mut u8, length: usize, prot: MemoryProtection) -> Result<()> {
    let ret = syscall3(
        SyscallNumber::Mprotect,
        addr as usize,
        length,
        prot as usize,
    );
    check_syscall_result(ret).map(|_| ())
}

/// Modifie le break du segment de données
///
/// # Safety
/// - `addr` doit être une adresse valide pour le nouveau break
#[inline]
pub unsafe fn brk(addr: *mut u8) -> Result<*mut u8> {
    let ret = syscall1(SyscallNumber::Brk, addr as usize);
    check_syscall_result(ret).map(|addr| addr as *mut u8)
}
