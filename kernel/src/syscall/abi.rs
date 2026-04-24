//! # syscall/abi.rs — Convention d'appel ABI x86_64 Exo-OS
//!
//! Définit les types et constantes de l'interface binaire Ring3→Ring0.
//!
//! ## ABI Registres (confirmé par ExoFS_Syscall_Analyse.md §2)
//!
//! | Registre | Sens   | Rôle                                            |
//! |----------|--------|-------------------------------------------------|
//! | `rax`    | entrée | Numéro syscall                                  |
//! | `rdi`    | entrée | arg1                                            |
//! | `rsi`    | entrée | arg2                                            |
//! | `rdx`    | entrée | arg3                                            |
//! | `r10`    | entrée | arg4 (pas rcx — SYSCALL hw écrase rcx)          |
//! | `r8`     | entrée | arg5                                            |
//! | `r9`     | entrée | arg6                                            |
//! | `rax`    | sortie | ≥ 0 = succès, < 0 = -errno                     |
//! | `rcx`    | hw     | RIP retour Ring3 (sauvé par SYSCALL instruction)|
//! | `r11`    | hw     | RFLAGS Ring3 (sauvé par SYSCALL instruction)    |
//!
//! ## Règles de sécurité ABI
//!
//! - **ABI-03** : INTERDIT de retourner un pointeur kernel ou enum brute dans rax.
//! - **ABI-04** : INTERDIT de modifier rdi/rsi/rdx dans le handler.
//! - **ABI-05** : Stack kernel alignée 16 bytes à l'entrée du handler.
//! - **ABI-06** : SWAPGS obligatoire entrée ET sortie du trampoline (KPTI).
//! - **ABI-07** : INTERDIT d'utiliser l'instruction SYSCALL depuis Ring 0.
//! - **ABI-08** : pt_regs complet sauvegardé — accessible pour ptrace.
//! - **BUG-05** : verify_rcx_canonical() obligatoire avant SYSRETQ (errata Intel/AMD).

// ─────────────────────────────────────────────────────────────────────────────
// SyscallArgs — vue typée des 6 arguments syscall
// ─────────────────────────────────────────────────────────────────────────────

/// Arguments extraits du SyscallFrame (rdi, rsi, rdx, r10, r8, r9).
///
/// Construit par `dispatch::dispatch()` depuis le `SyscallFrame` sauvegardé par
/// le trampoline ASM. Les valeurs sont RAW — aucune validation n'a été faite.
#[derive(Copy, Clone, Debug, Default)]
#[repr(C)]
pub struct SyscallArgs {
    /// arg1 ← rdi
    pub arg0: u64,
    /// arg2 ← rsi
    pub arg1: u64,
    /// arg3 ← rdx
    pub arg2: u64,
    /// arg4 ← r10 (PAS rcx — SYSCALL l'a écrasé avec RIP retour)
    pub arg3: u64,
    /// arg5 ← r8
    pub arg4: u64,
    /// arg6 ← r9
    pub arg5: u64,
}

impl SyscallArgs {
    /// Construit depuis les 6 paramètres bruts de la table de dispatch.
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
// SyscallResult — convention de retour
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un handler syscall.
///
/// - `Ok(v)` → `rax = v` (≥ 0 pour un succès, jamais un pointeur kernel)
/// - `Err(e)` → `rax = e` où `e` est déjà un errno négatif (ex: -2 pour ENOENT)
///
/// RÈGLE ABI-02 : rax ≥ 0 = succès, rax < 0 = -errno.
/// RÈGLE ABI-03 : INTERDIT de mettre un pointeur kernel dans Ok(v).
pub type SyscallResult = Result<i64, i64>;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers de retour sécurisés
// ─────────────────────────────────────────────────────────────────────────────

use crate::syscall::errno::{EACCES, EFAULT, EINVAL, ENOENT, ENOMEM, ENOSYS};

/// Retour d'erreur EINVAL.
#[inline(always)]
pub fn err_inval() -> SyscallResult {
    Err(EINVAL)
}

/// Retour d'erreur EFAULT (pointeur userspace invalide).
#[inline(always)]
pub fn err_fault() -> SyscallResult {
    Err(EFAULT)
}

/// Retour d'erreur ENOMEM.
#[inline(always)]
pub fn err_nomem() -> SyscallResult {
    Err(ENOMEM)
}

/// Retour d'erreur ENOSYS (syscall non implémenté).
#[inline(always)]
pub fn err_nosys() -> SyscallResult {
    Err(ENOSYS)
}

/// Retour d'erreur EACCES (capability refusée).
#[inline(always)]
pub fn err_acces() -> SyscallResult {
    Err(EACCES)
}

/// Retour d'erreur ENOENT.
#[inline(always)]
pub fn err_noent() -> SyscallResult {
    Err(ENOENT)
}

// ─────────────────────────────────────────────────────────────────────────────
// Validation canonicité RCX — FIX BUG-05 (errata Intel/AMD)
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie que `rcx` (adresse de retour Ring3) est canonique x86_64.
///
/// Un pointeur canonique x86_64 a les bits 63:48 identiques au bit 47.
/// Si non-canonique → SYSRETQ fault en Ring0 → exploitable pour une escalade.
///
/// La vérification est faite par le trampoline ASM avant `sysretq`.
/// Cette fonction est exportée pour les tests unitaires.
///
/// **BUG-05 FIX** : Absent d'ExoFS v3. Obligatoire avant sysretq.
#[inline]
pub fn is_canonical_address(addr: u64) -> bool {
    // Un pointeur canonique : bits 63:48 = bit 47 (sign-extended)
    let sign_bit = (addr >> 47) & 1;
    let upper_bits = addr >> 48;
    if sign_bit == 0 {
        upper_bits == 0x0000
    } else {
        upper_bits == 0xFFFF
    }
}

/// Constantes pour les plages d'adresses virtuelles.
pub mod addr_space {
    /// Limite haute de l'espace userspace (canonical hole commence ici).
    pub const USER_SPACE_TOP: u64 = 0x0000_8000_0000_0000;
    /// Début de l'espace noyau (canonical high).
    pub const KERNEL_SPACE_BASE: u64 = 0xFFFF_8000_0000_0000;
    /// Taille d'une page standard.
    pub const PAGE_SIZE: u64 = 4096;
    /// Masque d'alignement page.
    pub const PAGE_MASK: u64 = !(PAGE_SIZE - 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// Layout pt_regs (référence pour ptrace et le trampoline ASM)
// ─────────────────────────────────────────────────────────────────────────────

/// Offsets du SyscallFrame sauvegardé sur la kernel stack par le trampoline.
///
/// Correspond exactement au layout documenté dans `entry_asm.rs`.
pub mod pt_regs_offsets {
    pub const OFF_RAX: usize = 0; // numéro syscall / valeur retour
    pub const OFF_R9: usize = 8; // arg6
    pub const OFF_R8: usize = 16; // arg5
    pub const OFF_R10: usize = 24; // arg4
    pub const OFF_RDX: usize = 32; // arg3
    pub const OFF_RDI: usize = 40; // arg1
    pub const OFF_RSI: usize = 48; // arg2
    pub const OFF_RSP: usize = 56; // RSP userspace sauvegardé
    pub const OFF_R15: usize = 64;
    pub const OFF_R14: usize = 72;
    pub const OFF_R13: usize = 80;
    pub const OFF_R12: usize = 88;
    pub const OFF_RBX: usize = 96;
    pub const OFF_RBP: usize = 104;
    pub const OFF_R11: usize = 112; // RFLAGS Ring3 (sauvé par SYSCALL hw)
    pub const OFF_RCX: usize = 120; // RIP retour Ring3 (sauvé par SYSCALL hw)
    pub const FRAME_SIZE: usize = 128;
}
