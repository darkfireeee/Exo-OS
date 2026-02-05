// libs/exo_std/src/syscall/thread.rs
//! Syscalls relatifs aux threads

use super::{syscall0, syscall1, syscall2, syscall3, SyscallNumber, check_syscall_result};
use crate::Result;

/// ID de thread
pub type Tid = u64;

/// Crée un nouveau thread
///
/// # Safety
/// - `entry` doit être une fonction valide
/// - `arg` doit être un pointeur valide ou null
/// - `stack` doit pointer vers une pile valide de taille `stack_size`
#[inline]
pub unsafe fn thread_create(
    entry: extern "C" fn(*mut u8) -> *mut u8,
    arg: *mut u8,
    stack: *mut u8,
    stack_size: usize,
) -> Result<Tid> {
    let ret = syscall4(
        SyscallNumber::ThreadCreate,
        entry as usize,
        arg as usize,
        stack as usize,
        stack_size,
    );
    check_syscall_result(ret).map(|tid| tid as Tid)
}

/// Termine le thread actuel
///
/// # Safety
/// Cette fonction ne retourne jamais
#[inline]
pub unsafe fn thread_exit(retval: *mut u8) -> ! {
    syscall1(SyscallNumber::ThreadExit, retval as usize);
    
    #[allow(clippy::empty_loop)]
    loop {
        core::hint::spin_loop();
    }
}

/// Attend la fin d'un thread
///
/// # Safety
/// - `tid` doit être un ID de thread valide
/// - `retval` peut être null si le retour n'est pas nécessaire
#[inline]
pub unsafe fn thread_join(tid: Tid, retval: *mut *mut u8) -> Result<()> {
    let ret = syscall2(SyscallNumber::ThreadJoin, tid as usize, retval as usize);
    check_syscall_result(ret).map(|_| ())
}

/// Obtient le TID du thread actuel
#[inline]
pub fn gettid() -> Tid {
    #[cfg(feature = "test_mode")]
    {
        1 // TID simulé en mode test
    }
    
    #[cfg(not(feature = "test_mode"))]
    unsafe {
        syscall0(SyscallNumber::GetTid) as Tid
    }
}

/// Cède le CPU au scheduler
#[inline]
pub fn yield_now() {
    #[cfg(not(feature = "test_mode"))]
    unsafe {
        syscall0(SyscallNumber::ThreadYield);
    }
}

/// Endort le thread pour un nombre de nanosecondes
#[inline]
pub fn sleep_nanos(nanos: u64) {
    #[cfg(not(feature = "test_mode"))]
    unsafe {
        syscall1(SyscallNumber::ThreadSleep, nanos as usize);
    }
}
