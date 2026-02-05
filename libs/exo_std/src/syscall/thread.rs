<<<<<<< Updated upstream
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
=======
//! Appels système de gestion des threads

use super::{syscall0, syscall1, syscall2, SyscallId};
use crate::error::{SystemError, ThreadError};

/// Crée un nouveau thread
pub unsafe fn thread_create(
    entry: unsafe extern "C" fn(*mut u8) -> *mut u8,
    arg: *mut u8,
) -> Result<u64, ThreadError> {
    #[cfg(feature = "test_mode")]
    {
        let _ = (entry, arg);
        Ok(1)
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        let result = syscall2(
            SyscallId::ThreadCreate,
            entry as usize,
            arg as usize,
        );
        
        if result < 0 {
            Err(ThreadError::CreationFailed)
        } else {
            Ok(result as u64)
        }
    }
}

/// Termine le thread actuel
pub unsafe fn thread_exit() -> ! {
    #[cfg(not(feature = "test_mode"))]
    {
        syscall0(SyscallId::ThreadExit);
        loop {}
    }
    
    #[cfg(feature = "test_mode")]
    {
        panic!("thread_exit called in test mode");
    }
}

/// Yield du CPU
pub fn thread_yield() {
    #[cfg(not(feature = "test_mode"))]
    unsafe {
        syscall0(SyscallId::Yield);
    }
}

/// Dort pendant un nombre de nanosecondes
pub unsafe fn thread_sleep(nanos: u64) {
    syscall1(SyscallId::Sleep, nanos as usize);
}

/// Obtient l'ID du thread actuel
pub fn get_tid() -> u64 {
    #[cfg(feature = "test_mode")]
    {
        1
>>>>>>> Stashed changes
    }
    
    #[cfg(not(feature = "test_mode"))]
    unsafe {
<<<<<<< Updated upstream
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
=======
        syscall0(SyscallId::GetTid) as u64
    }
}

/// Attend qu'un thread se termine
pub unsafe fn thread_join(tid: u64) -> Result<(), ThreadError> {
    #[cfg(feature = "test_mode")]
    {
        let _ = tid;
        Ok(())
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        let result = syscall1(SyscallId::ThreadJoin, tid as usize);
        
        if result < 0 {
            Err(ThreadError::JoinFailed)
        } else {
            Ok(())
        }
>>>>>>> Stashed changes
    }
}
