//! Raw system call wrappers
//!
//! Low-level assembly syscall invocation for x86-64.
//! These functions use inline assembly to invoke system calls
//! with proper register conventions.

/// System call result type
pub type SyscallResult = isize;

/// Invoke system call with 0 arguments
///
/// # Safety
/// This is unsafe because it directly invokes kernel code.
/// Caller must ensure syscall number and contract are valid.
#[inline(always)]
pub unsafe fn syscall0(n: usize) -> SyscallResult {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") n,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
    }
    ret
}

/// Invoke system call with 1 argument
///
/// # Safety
/// This is unsafe because it directly invokes kernel code.
/// Caller must ensure syscall number and contract are valid.
#[inline(always)]
pub unsafe fn syscall1(n: usize, arg1: usize) -> SyscallResult {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") n,
            in("rdi") arg1,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
    }
    ret
}

/// Invoke system call with 2 arguments
///
/// # Safety
/// This is unsafe because it directly invokes kernel code.
/// Caller must ensure syscall number and contract are valid.
#[inline(always)]
pub unsafe fn syscall2(n: usize, arg1: usize, arg2: usize) -> SyscallResult {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") n,
            in("rdi") arg1,
            in("rsi") arg2,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
    }
    ret
}

/// Invoke system call with 3 arguments
///
/// # Safety
/// This is unsafe because it directly invokes kernel code.
/// Caller must ensure syscall number and contract are valid.
#[inline(always)]
pub unsafe fn syscall3(n: usize, arg1: usize, arg2: usize, arg3: usize) -> SyscallResult {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") n,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
    }
    ret
}

/// Invoke system call with 4 arguments
///
/// # Safety
/// This is unsafe because it directly invokes kernel code.
/// Caller must ensure syscall number and contract are valid.
#[inline(always)]
pub unsafe fn syscall4(
    n: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
) -> SyscallResult {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") n,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
    }
    ret
}

/// Invoke system call with 5 arguments
///
/// # Safety
/// This is unsafe because it directly invokes kernel code.
/// Caller must ensure syscall number and contract are valid.
#[inline(always)]
pub unsafe fn syscall5(
    n: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> SyscallResult {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") n,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            in("r8") arg5,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
    }
    ret
}

/// Invoke system call with 6 arguments
///
/// # Safety
/// This is unsafe because it directly invokes kernel code.
/// Caller must ensure syscall number and contract are valid.
#[inline(always)]
pub unsafe fn syscall6(
    n: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
    arg6: usize,
) -> SyscallResult {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") n,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            in("r8") arg5,
            in("r9") arg6,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
    }
    ret
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn test_syscall_signatures() {
        // Just test that the functions compile and have correct signatures
        // We can't actually invoke syscalls in tests without a proper kernel

        let _f0: unsafe fn(usize) -> SyscallResult = syscall0;
        let _f1: unsafe fn(usize, usize) -> SyscallResult = syscall1;
        let _f2: unsafe fn(usize, usize, usize) -> SyscallResult = syscall2;
        let _f3: unsafe fn(usize, usize, usize, usize) -> SyscallResult = syscall3;
        let _f4: unsafe fn(usize, usize, usize, usize, usize) -> SyscallResult = syscall4;
        let _f5: unsafe fn(usize, usize, usize, usize, usize, usize) -> SyscallResult = syscall5;
        let _f6: unsafe fn(usize, usize, usize, usize, usize, usize, usize) -> SyscallResult =
            syscall6;
    }
}
