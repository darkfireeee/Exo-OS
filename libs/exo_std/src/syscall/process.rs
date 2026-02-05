<<<<<<< Updated upstream
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
=======
//! Appels système de gestion des processus

use super::{SysResult, syscall0, syscall1, syscall2, syscall3, SyscallId};
use crate::error::{SystemError, ProcessError};

/// Quitte le processus actuel
pub unsafe fn exit(code: i32) -> ! {
    syscall1(SyscallId::Exit, code as usize);
    loop {}
}

/// Fork le processus actuel
pub unsafe fn fork() -> Result<u32, ProcessError> {
    let result = syscall0(SyscallId::Fork);

    if result < 0 {
        Err(ProcessError::ForkFailed)
    } else {
        Ok(result as u32)
    }
}

/// Attend qu'un processus se termine
/// Retourne (pid, status)
pub unsafe fn wait(pid: u32) -> Result<(u32, i32), ProcessError> {
    let result = syscall1(SyscallId::Wait, pid as usize);

    if result < 0 {
        Err(ProcessError::WaitFailed)
    } else {
        // Pour l'instant, on retourne le pid donné et le résultat comme status
        Ok((pid, result as i32))
    }
}

/// Tue un processus
pub unsafe fn kill(pid: u32, signal: i32) -> Result<(), ProcessError> {
    let result = syscall2(SyscallId::Kill, pid as usize, signal as usize);

    if result < 0 {
        Err(ProcessError::KillFailed)
    } else {
        Ok(())
    }
}

/// Obtient l'ID du processus actuel
pub fn get_pid() -> u32 {
    #[cfg(feature = "test_mode")]
    {
        1234
>>>>>>> Stashed changes
    }
    
    #[cfg(not(feature = "test_mode"))]
    unsafe {
<<<<<<< Updated upstream
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
=======
        syscall0(SyscallId::GetPid) as u32
    }
}

/// Execute un nouveau programme dans le processus actuel
pub unsafe fn exec(path: &str, args: &[&str]) -> ProcessError {
    #[cfg(feature = "test_mode")]
    {
        let _ = (path, args);
        ProcessError::ExecFailed
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // Préparer les arguments pour le syscall
        let path_ptr = path.as_ptr() as usize;
        let path_len = path.len();
        let args_ptr = args.as_ptr() as usize;
        let args_len = args.len();
        
        // Le syscall exec ne retourne jamais en cas de succès
        let result = syscall3(SyscallId::Exec, path_ptr, path_len, args_ptr);
        
        // Si on arrive ici, l'exec a échoué
        ProcessError::ExecFailed
    }
>>>>>>> Stashed changes
}
