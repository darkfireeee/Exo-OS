//! # syscall/abi.rs — Exo-OS x86_64 ABI calling convention
//!
//! Defines types and constants for the Ring3→Ring0 binary interface.
//!
//! ## Register ABI (confirmed by ExoFS_Syscall_Analyse.md §2)
//!
//! | Register | Direction | Role                                            |
//! |----------|-----------|-------------------------------------------------|
//! | `rax`    | input     | Syscall number                                  |
//! | `rdi`    | input     | arg1                                            |
//! | `rsi`    | input     | arg2                                            |
//! | `rdx`    | input     | arg3                                            |
//! | `r10`    | input     | arg4 (not rcx — hw SYSCALL overwrites rcx)      |
//! | `r8`     | input     | arg5                                            |
//! | `r9`     | input     | arg6                                            |
//! | `rax`    | output    | ≥ 0 = success, < 0 = -errno                     |
//! | `rcx`    | hw        | Ring3 return RIP (saved by SYSCALL instruction) |
//! | `r11`    | hw        | Ring3 RFLAGS (saved by SYSCALL instruction)     |
//!
//! ## ABI Security Rules
//!
//! - **ABI-03**: FORBIDDEN to return a kernel pointer or raw enum in rax.
//! - **ABI-04**: FORBIDDEN to modify rdi/rsi/rdx in handler.
//! - **ABI-05**: Kernel stack aligned to 16 bytes on entry to handler.
//! - **ABI-06**: SWAPGS mandatory on trampoline entry AND exit (KPTI).
//! - **ABI-07**: FORBIDDEN to use SYSCALL instruction from Ring 0.
//! - **ABI-08**: Full pt_regs saved — accessible for ptrace.
//! - **BUG-05**: verify_rcx_canonical() mandatory before SYSRETQ (Intel/AMD errata).

// ─────────────────────────────────────────────────────────────────────────────
// SyscallArgs — typed view of the 6 syscall arguments
// ─────────────────────────────────────────────────────────────────────────────

/// Arguments extracted from SyscallFrame (rdi, rsi, rdx, r10, r8, r9).
///
/// Constructed by `dispatch::dispatch()` from the `SyscallFrame` saved by
/// the ASM trampoline. Values are RAW — no validation has been performed.
#[derive(Copy, Clone, Debug, Default)]
#[repr(C)]
pub struct SyscallArgs {
    /// arg1 ← rdi
    pub arg0: u64,
    /// arg2 ← rsi
    pub arg1: u64,
    /// arg3 ← rdx
    pub arg2: u64,
    /// arg4 ← r10 (NOT rcx — SYSCALL overwrote it with return RIP)
    pub arg3: u64,
    /// arg5 ← r8
    pub arg4: u64,
    /// arg6 ← r9
    pub arg5: u64,
}

impl SyscallArgs {
    /// Constructs from the 6 raw parameters of the dispatch table.
    #[inline(always)]
    pub const fn new(a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> Self {
        Self {
            arg0: a0,
            arg1: a1,
            arg2: a2,
            arg3: a3,
            arg4: a4,
            arg5: a5,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SyscallResult — return convention
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a syscall handler.
///
/// - `Ok(v)` → `rax = v` (≥ 0 for success, never a kernel pointer)
/// - `Err(e)` → `rax = e` where `e` is already a negative errno (e.g.: -2 for ENOENT)
///
/// RULE ABI-02: rax ≥ 0 = success, rax < 0 = -errno.
/// RULE ABI-03: FORBIDDEN to place a kernel pointer in Ok(v).
pub type SyscallResult = Result<i64, i64>;

// ─────────────────────────────────────────────────────────────────────────────
// Secure return helpers
// ─────────────────────────────────────────────────────────────────────────────

use crate::syscall::errno::{EACCES, EFAULT, EINVAL, ENOENT, ENOMEM, ENOSYS};

/// EINVAL error return.
#[inline(always)]
pub fn err_inval() -> SyscallResult {
    Err(EINVAL)
}

/// EFAULT error return (invalid userspace pointer).
#[inline(always)]
pub fn err_fault() -> SyscallResult {
    Err(EFAULT)
}

/// ENOMEM error return.
#[inline(always)]
pub fn err_nomem() -> SyscallResult {
    Err(ENOMEM)
}

/// ENOSYS error return (unimplemented syscall).
#[inline(always)]
pub fn err_nosys() -> SyscallResult {
    Err(ENOSYS)
}

/// EACCES error return (capability denied).
#[inline(always)]
pub fn err_acces() -> SyscallResult {
    Err(EACCES)
}

/// ENOENT error return.
#[inline(always)]
pub fn err_noent() -> SyscallResult {
    Err(ENOENT)
}

// ─────────────────────────────────────────────────────────────────────────────
// RCX canonicality validation — FIX BUG-05 (Intel/AMD errata)
// ─────────────────────────────────────────────────────────────────────────────

/// Checks that `rcx` (Ring3 return address) is x86_64 canonical.
///
/// A canonical x86_64 pointer has bits 63:48 identical to bit 47.
/// If non-canonical → SYSRETQ fault in Ring0 → exploitable for privilege escalation.
///
/// The check is performed by the ASM trampoline before `sysretq`.
/// This function is exported for unit tests.
///
/// **BUG-05 FIX**: Missing from ExoFS v3. Mandatory before sysretq.
#[inline]
pub fn is_canonical_address(addr: u64) -> bool {
    // A canonical pointer: bits 63:48 = bit 47 (sign-extended)
    let sign_bit = (addr >> 47) & 1;
    let upper_bits = addr >> 48;
    if sign_bit == 0 {
        upper_bits == 0x0000
    } else {
        upper_bits == 0xFFFF
    }
}

/// Constants for virtual address ranges.
pub mod addr_space {
    /// Upper limit of userspace (canonical hole starts here).
    pub const USER_SPACE_TOP: u64 = 0x0000_8000_0000_0000;
    /// Start of kernel space (canonical high).
    pub const KERNEL_SPACE_BASE: u64 = 0xFFFF_8000_0000_0000;
    /// Standard page size.
    pub const PAGE_SIZE: u64 = 4096;
    /// Page alignment mask.
    pub const PAGE_MASK: u64 = !(PAGE_SIZE - 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// pt_regs layout (reference for ptrace and ASM trampoline)
// ─────────────────────────────────────────────────────────────────────────────

/// Offsets of SyscallFrame saved on the kernel stack by the trampoline.
///
/// Exactly matches the layout documented in `entry_asm.rs`.
pub mod pt_regs_offsets {
    pub const OFF_RAX: usize = 0; // syscall number / return value
    pub const OFF_R9: usize = 8; // arg6
    pub const OFF_R8: usize = 16; // arg5
    pub const OFF_R10: usize = 24; // arg4
    pub const OFF_RDX: usize = 32; // arg3
    pub const OFF_RDI: usize = 40; // arg1
    pub const OFF_RSI: usize = 48; // arg2
    pub const OFF_RSP: usize = 56; // saved userspace RSP
    pub const OFF_R15: usize = 64;
    pub const OFF_R14: usize = 72;
    pub const OFF_R13: usize = 80;
    pub const OFF_R12: usize = 88;
    pub const OFF_RBX: usize = 96;
    pub const OFF_RBP: usize = 104;
    pub const OFF_R11: usize = 112; // Ring3 RFLAGS (saved by hw SYSCALL)
    pub const OFF_RCX: usize = 120; // Ring3 return RIP (saved by hw SYSCALL)
    pub const FRAME_SIZE: usize = 128;
}
