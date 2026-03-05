//! # arch/x86_64/spectre/retpoline.rs — Retpoline (Spectre variant 2)
//!
//! La retpoline remplace les indirect calls/jumps par une construction qui
//! empêche la prédiction spéculative de branche indirecte :
//!
//! ```asm
//! ; retpoline sequence pour CALL [rax]
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
//! En pratique, avec Rust/LLVM ≥ 15 : `-Ztarget-feature=+retpoline-indirect-calls`
//! ce module expose uniquement les macros et helpers pour les cas manuels (ASM).

#![allow(dead_code)]

/// Appel indirect via retpoline vers un pointeur de fonction
///
/// Usage : quand on ne peut pas utiliser l'attribut Rust `-Zretpoline`.
/// Nécessaire pour les call gates du kernel.
#[macro_export]
macro_rules! retpoline_call {
    ($target:expr) => {
        // SAFETY: $target = ptr de fonction valide; retpoline empêche la prédiction spéculative.
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

/// Jump indirect via retpoline
#[macro_export]
macro_rules! retpoline_jmp {
    ($target:expr) => {
        // SAFETY: $target est une adresse valide
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

// Stub retpoline dans le code ASM global (appelable depuis l'ASM de stubs)
//
// Expose `__x86_indirect_thunk_rax` et `__x86_indirect_thunk_r11`
// compatibles avec la convention GCC retpoline.
core::arch::global_asm!(
    // Thunk pour RAX
    ".global __x86_indirect_thunk_rax",
    ".type __x86_indirect_thunk_rax, @function",
    "__x86_indirect_thunk_rax:",
    "    call 1f",
    "2:  pause",
    "    lfence",
    "    jmp 2b",
    "1:  mov [rsp], rax",
    "    ret",

    // Thunk pour R11
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
