// libs/exo_std/src/syscall/process.rs
//! Syscalls relatifs aux processus

use super::{syscall0, syscall1, syscall2, syscall3, SyscallNumber, check_syscall_result};
use crate::Result;

/// ID de processus
pub type Pid = u32;

/// Quitte le processus avec le code donné
///
/// # Safety
/// Cette fonction ne retourne jamais
#[inline]
pub unsafe fn exit(code: i32) -> ! {
    syscall1(SyscallNumber::Exit, code as usize);
    
    // Si le syscall retourne (ne devrait pas arriver), boucle infinie
    #[allow(clippy::empty_loop)]
    loop {
        core::hint::spin_loop();
    }
}

/// Fork le processus actuel
///
/// Retourne le PID de l'enfant dans le parent, 0 dans l'enfant
#[inline]
pub unsafe fn fork() -> Result<Pid> {
    let ret = syscall0(SyscallNumber::Fork);
    check_syscall_result(ret).map(|pid| pid as Pid)
}

/// Remplace le processus actuel par un nouveau programme
///
/// # Safety
/// - `path` doit être une chaîne C valide
/// - `argv` et `envp` doivent être des tableaux de pointeurs valides terminés par NULL
#[inline]
pub unsafe fn exec(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> Result<()> {
    let ret = syscall3(
        SyscallNumber::Exec,
        path as usize,
        argv as usize,
        envp as usize,
    );
    check_syscall_result(ret).map(|_| ())
}

/// Attend la fin d'un processus enfant
///
/// Retourne le PID et le statut de sortie
#[inline]
pub unsafe fn wait(pid: Pid, status: *mut i32) -> Result<Pid> {
    let ret = syscall2(SyscallNumber::Wait, pid as usize, status as usize);
    check_syscall_result(ret).map(|pid| pid as Pid)
}

/// Obtient le PID du processus actuel
#[inline]
pub fn getpid() -> Pid {
    #[cfg(feature = "test_mode")]
    {
        1 // PID simulé en mode test
    }
    
    #[cfg(not(feature = "test_mode"))]
    unsafe {
        syscall0(SyscallNumber::GetPid) as Pid
    }
}

/// Envoie un signal à un processus
///
/// # Safety
/// L'appelant doit avoir les permissions pour envoyer le signal
#[inline]
pub unsafe fn kill(pid: Pid, signal: i32) -> Result<()> {
    let ret = syscall2(SyscallNumber::Kill, pid as usize, signal as usize);
    check_syscall_result(ret).map(|_| ())
}
