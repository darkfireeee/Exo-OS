//! Gestion des processus
//!
//! Ce module fournit des structures et fonctions pour créer et gérer des processus.

pub mod command;
pub mod child;

// Réexportations
pub use command::Command;
pub use child::{Child, ExitStatus};

use crate::error::ProcessError;
use crate::syscall::process as syscall;

/// ID de processus
pub type Pid = u32;

/// Quitte le processus actuel
pub fn exit(code: i32) -> ! {
    #[cfg(feature = "test_mode")]
    loop {
        core::hint::spin_loop();
    }
    
    #[cfg(not(feature = "test_mode"))]
    unsafe {
        syscall::exit(code)
    }
}

/// ID du processus actuel
pub fn id() -> Pid {
    crate::syscall::process::getpid()
}

/// Fork le processus actuel
pub fn fork() -> Result<Pid, ProcessError> {
    #[cfg(feature = "test_mode")]
    {
        Ok(0)
    }

    #[cfg(not(feature = "test_mode"))]
    unsafe {
        syscall::fork().map(|pid| pid as Pid).map_err(|e| e.into())
    }
}

/// Attend qu'un processus se termine
pub fn wait(pid: Pid) -> Result<(Pid, ExitStatus), ProcessError> {
    #[cfg(feature = "test_mode")]
    {
        Ok((pid, ExitStatus::exited(0)))
    }

    #[cfg(not(feature = "test_mode"))]
    unsafe {
        let mut status: i32 = 0;
        let waited_pid = syscall::wait(pid, &mut status as *mut i32)?;
        Ok((waited_pid as Pid, ExitStatus::from_raw(status)))
    }
}

/// Envoie un signal à un processus
pub fn kill(pid: Pid, signal: i32) -> Result<(), ProcessError> {
    #[cfg(feature = "test_mode")]
    {
        let _ = (pid, signal);
        Ok(())
    }

    #[cfg(not(feature = "test_mode"))]
    unsafe {
        syscall::kill(pid, signal).map_err(|e| e.into())
    }
}
