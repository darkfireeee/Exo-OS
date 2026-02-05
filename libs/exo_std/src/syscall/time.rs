<<<<<<< Updated upstream
// libs/exo_std/src/syscall/time.rs
//! Syscalls relatifs au temps

use super::{syscall0, syscall2, SyscallNumber, check_syscall_result};
use crate::Result;

/// Type de clock
#[repr(usize)]
pub enum ClockType {
    /// Horloge monotone (ne recule jamais)
    Monotonic = 0,
    /// Horloge temps réel (peut être ajustée)
    Realtime = 1,
}

/// Obtient le temps actuel en nanosecondes
#[inline]
pub fn get_time(clock: ClockType) -> u64 {
    #[cfg(feature = "test_mode")]
    {
        let _ = clock;
        0 // Temps simulé en mode test
    }
    
    #[cfg(not(feature = "test_mode"))]
    unsafe {
        syscall1(SyscallNumber::GetTime, clock as usize) as u64
    }
}

/// Défini le temps système (nécessite privilèges)
///
/// # Safety
/// L'appelant doit avoir les permissions appropriées
#[inline]
pub unsafe fn set_time(clock: ClockType, nanos: u64) -> Result<()> {
    let ret = syscall2(SyscallNumber::SetTime, clock as usize, nanos as usize);
    check_syscall_result(ret).map(|_| ())
=======
//! Appels système de gestion du temps

use super::{SysResult, syscall1, SyscallId};
use crate::error::{SystemError, TimeError};

/// Type d'horloge
#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum ClockType {
    /// Horloge système (wall time)
    Realtime = 0,
    /// Horloge monotone (ne recule jamais)
    Monotonic = 1,
    /// Temps CPU du processus
    ProcessCpu = 2,
    /// Temps CPU du thread
    ThreadCpu = 3,
}

/// Obtient le temps actuel en nanosecondes
pub unsafe fn get_time(clock: ClockType) -> Result<u64, TimeError> {
    let result = syscall1(SyscallId::GetTime, clock as usize);

    if result < 0 {
        Err(TimeError::Other)
    } else {
        Ok(result as u64)
    }
}

/// Dort pendant un nombre de nanosecondes
pub unsafe fn sleep_nanos(nanos: u64) {
    syscall1(SyscallId::Sleep, nanos as usize);
>>>>>>> Stashed changes
}
