//! SysV x86-64 entry (`_exo_ring1_entry`) and signal restorer (`__exo_sigreturn_trampoline`)
//! assembly stubs for Ring 1 `#![no_std]` servers and drivers (`x86_64-exo-userspace.json`).

#[cfg(all(target_arch = "x86_64", not(test)))]
core::arch::global_asm!(
    ".global __exo_sigreturn_trampoline",
    ".type __exo_sigreturn_trampoline, @function",
    "__exo_sigreturn_trampoline:",
    "mov rax, 15", // SYS_RT_SIGRETURN
    "syscall",
    "ud2",
);

#[cfg(all(target_arch = "x86_64", target_os = "none"))]
core::arch::global_asm!(
    ".section .ltext._exo_ring1_entry,\"ax\",@progbits",
    ".global _exo_ring1_entry",
    ".type _exo_ring1_entry, @function",
    "_exo_ring1_entry:",
    "xor rbp, rbp",
    "and rsp, -16",
    "movabs rax, offset _start",
    "call rax",
    "ud2",
);

#[cfg(all(target_arch = "x86_64", not(test)))]
extern "C" {
    fn __exo_sigreturn_trampoline();
}

/// Address of the signal-return trampoline to store in `sigaction.sa_restorer`.
#[inline]
pub fn sigreturn_trampoline() -> u64 {
    #[cfg(all(target_arch = "x86_64", not(test)))]
    {
        __exo_sigreturn_trampoline as *const () as u64
    }
    #[cfg(not(all(target_arch = "x86_64", not(test))))]
    {
        0
    }
}
