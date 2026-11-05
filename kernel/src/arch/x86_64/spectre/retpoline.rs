//! # arch/x86_64/spectre/retpoline.rs — Retpoline (Spectre variant 2)
//!
//! Retpoline replaces indirect calls/jumps with a construct that
//! prevents speculative branch prediction on indirect branches:
//!
//! ```asm
//! ; retpoline sequence for CALL [rax]
//! call set_up_target
//! capture_spec:
//!     pause
//!     lfence
//!     jmp capture_spec
//! set_up_target:
//!     mov [rsp], rax    ; override return addr with real target
//!     ret
//! ```
//!
//! In practice, with Rust/LLVM ≥ 15: `-Ztarget-feature=+retpoline-indirect-calls`
//! this module only exposes macros and helpers for manual cases (ASM).

/// Indirect call via retpoline to a function pointer
///
/// Usage: when the Rust `-Zretpoline` attribute cannot be used.
/// Required for kernel call gates.
#[macro_export]
macro_rules! retpoline_call {
    ($target:expr) => {
        // SAFETY: $target = valid function ptr; retpoline prevents speculative prediction.
        unsafe {
            core::arch::asm!(
                "call 2f",
                "3:",
                "pause",
                "lfence",
                "jmp 3b",
                "2:",
                "mov [rsp], {target}",
                "ret",
                target = in(reg) $target,
                options(nostack),
            );
        }
    }
}

/// Indirect jump via retpoline
#[macro_export]
macro_rules! retpoline_jmp {
    ($target:expr) => {
        // SAFETY: $target is a valid address
        unsafe {
            core::arch::asm!(
                "call 2f",
                "3:",
                "pause",
                "lfence",
                "jmp 3b",
                "2:",
                "mov [rsp], {target}",
                "ret",
                target = in(reg) $target,
                options(noreturn),
            );
        }
    }
}

// Retpoline stub in global ASM code (callable from stub ASM)
//
// Exposes `__x86_indirect_thunk_rax` and `__x86_indirect_thunk_r11`
// compatible with the GCC retpoline convention.
core::arch::global_asm!(
    // Thunk for RAX
    ".global __x86_indirect_thunk_rax",
    ".type __x86_indirect_thunk_rax, @function",
    "__x86_indirect_thunk_rax:",
    "    call 1f",
    "2:  pause",
    "    lfence",
    "    jmp 2b",
    "1:  mov [rsp], rax",
    "    ret",
    // Thunk for R11
    ".global __x86_indirect_thunk_r11",
    ".type __x86_indirect_thunk_r11, @function",
    "__x86_indirect_thunk_r11:",
    "    call 1f",
    "2:  pause",
    "    lfence",
    "    jmp 2b",
    "1:  mov [rsp], r11",
    "    ret",
);
