// libs/exo_std/src/syscall/time.rs
//! Syscalls relatifs au temps

use super::{syscall1, SyscallNumber};
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
}
