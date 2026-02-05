<<<<<<< Updated upstream
// libs/exo_std/src/syscall/mod.rs
//! Couche d'abstraction pour les appels système
//!
//! Ce module centralise tous les appels système vers le kernel Exo-OS,
//! fournissant une interface type-safe et documentée.

pub mod process;
pub mod thread;
pub mod memory;
pub mod io;
pub mod time;

/// Numéros d'appels système
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallNumber {
    // Process
    Exit = 0,
    Fork = 1,
    Exec = 2,
    Wait = 3,
    GetPid = 4,
    Kill = 5,
    
    // Thread
    ThreadCreate = 10,
    ThreadExit = 11,
    ThreadJoin = 12,
    GetTid = 13,
    ThreadYield = 14,
    ThreadSleep = 15,
    
    // Memory
    Mmap = 20,
    Munmap = 21,
    Mprotect = 22,
    Brk = 23,
    
    // I/O
    Read = 30,
    Write = 31,
    Open = 32,
    Close = 33,
    Seek = 34,
    Ioctl = 35,
    
    // Time
    GetTime = 40,
    SetTime = 41,
    
    // IPC
    IpcSend = 50,
    IpcRecv = 51,
    IpcCreate = 52,
    
    // Sync
    Futex = 60,
}

/// Code de retour d'un syscall
pub type SyscallReturn = isize;

/// Exécute un syscall avec 0 argument
#[inline]
pub unsafe fn syscall0(num: SyscallNumber) -> SyscallReturn {
    #[cfg(feature = "test_mode")]
    {
        // Mode test: retourne succès simulé
        let _ = num;
=======
//! Couche d'abstraction des appels système
//!
//! Ce module fournit une interface sûre pour tous les appels système.

pub mod io;
pub mod process;
pub mod thread;
pub mod time;
pub mod memory;
pub mod ipc;

// Réexportations
pub use io::*;
pub use process::*;
pub use thread::*;
pub use time::*;
pub use memory::*;
pub use ipc::*;

/// ID d'appel système
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum SyscallId {
    Read = 0,
    Write = 1,
    Open = 2,
    Close = 3,
    Fork = 4,
    Exit = 5,
    Wait = 6,
    Kill = 7,
    Sleep = 8,
    GetTime = 9,
    Mmap = 10,
    Munmap = 11,
    Send = 12,
    Recv = 13,
    GetPid = 14,
    GetTid = 15,
    ThreadCreate = 16,
    ThreadExit = 17,
    Yield = 18,
    Exec = 19,
    ThreadJoin = 20,
    CapabilityVerify = 21,
    CapabilityRequest = 22,
    CapabilityRevoke = 23,
    CapabilityDelegate = 24,
}

/// Résultat d'un appel système
pub type SysResult<T> = Result<T, crate::error::SystemError>;

/// Effectue un appel système brut
#[inline]
pub unsafe fn syscall0(id: SyscallId) -> isize {
    #[cfg(feature = "test_mode")]
    {
        let _ = id;
>>>>>>> Stashed changes
        0
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
<<<<<<< Updated upstream
        let ret: isize;
        core::arch::asm!(
            "syscall",
            in("rax") num as usize,
            lateout("rax") ret,
            options(nostack, preserves_flags)
        );
        ret
    }
}

/// Exécute un syscall avec 1 argument
#[inline]
pub unsafe fn syscall1(num: SyscallNumber, arg1: usize) -> SyscallReturn {
    #[cfg(feature = "test_mode")]
    {
        let _ = (num, arg1);
=======
        let result: isize;
        core::arch::asm!(
            "syscall",
            in("rax") id as usize,
            lateout("rax") result,
            options(nostack)
        );
        result
    }
}

/// Appel système avec 1 argument
#[inline]
pub unsafe fn syscall1(id: SyscallId, arg1: usize) -> isize {
    #[cfg(feature = "test_mode")]
    {
        let _ = (id, arg1);
>>>>>>> Stashed changes
        0
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
<<<<<<< Updated upstream
        let ret: isize;
        core::arch::asm!(
            "syscall",
            in("rax") num as usize,
            in("rdi") arg1,
            lateout("rax") ret,
            options(nostack, preserves_flags)
        );
        ret
    }
}

/// Exécute un syscall avec 2 arguments
#[inline]
pub unsafe fn syscall2(num: SyscallNumber, arg1: usize, arg2: usize) -> SyscallReturn {
    #[cfg(feature = "test_mode")]
    {
        let _ = (num, arg1, arg2);
=======
        let result: isize;
        core::arch::asm!(
            "syscall",
            in("rax") id as usize,
            in("rdi") arg1,
            lateout("rax") result,
            options(nostack)
        );
        result
    }
}

/// Appel système avec 2 arguments
#[inline]
pub unsafe fn syscall2(id: SyscallId, arg1: usize, arg2: usize) -> isize {
    #[cfg(feature = "test_mode")]
    {
        let _ = (id, arg1, arg2);
>>>>>>> Stashed changes
        0
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
<<<<<<< Updated upstream
        let ret: isize;
        core::arch::asm!(
            "syscall",
            in("rax") num as usize,
            in("rdi") arg1,
            in("rsi") arg2,
            lateout("rax") ret,
            options(nostack, preserves_flags)
        );
        ret
    }
}

/// Exécute un syscall avec 3 arguments
#[inline]
pub unsafe fn syscall3(num: SyscallNumber, arg1: usize, arg2: usize, arg3: usize) -> SyscallReturn {
    #[cfg(feature = "test_mode")]
    {
        let _ = (num, arg1, arg2, arg3);
=======
        let result: isize;
        core::arch::asm!(
            "syscall",
            in("rax") id as usize,
            in("rdi") arg1,
            in("rsi") arg2,
            lateout("rax") result,
            options(nostack)
        );
        result
    }
}

/// Appel système avec 3 arguments
#[inline]
pub unsafe fn syscall3(id: SyscallId, arg1: usize, arg2: usize, arg3: usize) -> isize {
    #[cfg(feature = "test_mode")]
    {
        let _ = (id, arg1, arg2, arg3);
>>>>>>> Stashed changes
        0
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
<<<<<<< Updated upstream
        let ret: isize;
        core::arch::asm!(
            "syscall",
            in("rax") num as usize,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            lateout("rax") ret,
            options(nostack, preserves_flags)
        );
        ret
    }
}

/// Exécute un syscall avec 4 arguments
#[inline]
pub unsafe fn syscall4(
    num: SyscallNumber,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
) -> SyscallReturn {
    #[cfg(feature = "test_mode")]
    {
        let _ = (num, arg1, arg2, arg3, arg4);
        0
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        let ret: isize;
        core::arch::asm!(
            "syscall",
            in("rax") num as usize,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            lateout("rax") ret,
            options(nostack, preserves_flags)
        );
        ret
    }
}

/// Exécute un syscall avec 5 arguments
#[inline]
pub unsafe fn syscall5(
    num: SyscallNumber,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> SyscallReturn {
    #[cfg(feature = "test_mode")]
    {
        let _ = (num, arg1, arg2, arg3, arg4, arg5);
        0
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        let ret: isize;
        core::arch::asm!(
            "syscall",
            in("rax") num as usize,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            in("r8") arg5,
            lateout("rax") ret,
            options(nostack, preserves_flags)
        );
        ret
    }
}

/// Vérifie le code de retour d'un syscall et convertit en Result
#[inline]
pub fn check_syscall_result(ret: SyscallReturn) -> crate::Result<usize> {
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        // Convertit le code d'erreur négatif en erreur appropriée
        match ret {
            -1 => Err(crate::error::SystemError::InvalidArgument.into()),
            -2 => Err(crate::error::IoErrorKind::NotFound.into()),
            -3 => Err(crate::error::IoErrorKind::PermissionDenied.into()),
            -4 => Err(crate::error::SystemError::ResourceExhausted.into()),
            -5 => Err(crate::error::IoErrorKind::WouldBlock.into()),
            _ => Err(crate::error::SystemError::Other.into()),
=======
        let result: isize;
        core::arch::asm!(
            "syscall",
            in("rax") id as usize,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            lateout("rax") result,
            options(nostack)
        );
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_syscall() {
        unsafe {
            let _ = syscall0(SyscallId::GetTime);
            let _ = syscall1(SyscallId::Close, 0);
>>>>>>> Stashed changes
        }
    }
}
