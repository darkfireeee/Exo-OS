// kernel/src/memory/protection/smep.rs
//
// SMEP — Supervisor Mode Execution Prevention (Intel + AMD).
//
// La CPU refuse l'exécution de code en espace user (adresses canoniques basses)
// lorsque la CPL est 0 et CR4.SMEP = 1.
//
// Références :
//   Intel SDM Vol.3A § 4.6.2 — "Supervisor-Mode Execution Prevention"
//   AMD APM Vol.2 § 5.16 — "Supervisor Mode Execution Prevention"
//
// Couche 0 : pas de dépendance vers scheduler/process/ipc/fs.

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Bits CR4
// ─────────────────────────────────────────────────────────────────────────────

/// CR4 bit 20 — SMEP.
pub const CR4_SMEP_BIT: u64 = 1 << 20;

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques SMEP
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct SmepStats {
    /// Nombre d'activations de SMEP.
    pub enable_count:   AtomicU64,
    /// Nombre de désactivations temporaires (légitimes, ex. pour copie user).
    pub disable_count:  AtomicU64,
    /// Violations détectées par le fault handler.
    pub violation_count: AtomicU64,
    /// Appels redondants à `enable_smep` alors que déjà actif.
    pub redundant_enable: AtomicU64,
}

impl SmepStats {
    const fn new() -> Self {
        Self {
            enable_count:    AtomicU64::new(0),
            disable_count:   AtomicU64::new(0),
            violation_count: AtomicU64::new(0),
            redundant_enable: AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for SmepStats {}
pub static SMEP_STATS: SmepStats = SmepStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// État
// ─────────────────────────────────────────────────────────────────────────────

static SMEP_ACTIVE: AtomicBool = AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// Lecture / écriture CR4
// ─────────────────────────────────────────────────────────────────────────────

/// Lit CR4.
///
/// # Safety
/// CPL 0.
#[inline(always)]
unsafe fn read_cr4() -> u64 {
    let val: u64;
    core::arch::asm!(
        "mov {v}, cr4",
        v = out(reg) val,
        options(nostack, nomem, preserves_flags),
    );
    val
}

/// Écrit CR4.
///
/// # Safety
/// CPL 0. Une valeur incorrecte peut rendre le système instable.
#[inline(always)]
unsafe fn write_cr4(val: u64) {
    core::arch::asm!(
        "mov cr4, {v}",
        v = in(reg) val,
        options(nostack, nomem),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Teste le support SMEP via CPUID (feuille 7, sous-feuille 0, EBX bit 7).
#[inline]
pub fn smep_supported() -> bool {
    let ebx_r: u64;
    // SAFETY: CPUID disponible sur x86_64; xchg préserve rbx réservé par LLVM.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            lateout("eax") _,
            lateout("ecx") _,
            lateout("edx") _,
            tmp = inout(reg) 0u64 => ebx_r,
            options(nostack, nomem),
        );
    }
    let ebx = ebx_r as u32;
    ebx & (1 << 7) != 0
}

/// Active SMEP sur le CPU courant (CR4.SMEP = 1).
/// Idempotent.
///
/// # Safety
/// CPL 0.
pub unsafe fn enable_smep() {
    if !smep_supported() {
        return;
    }
    let cr4 = read_cr4();
    if cr4 & CR4_SMEP_BIT != 0 {
        SMEP_STATS.redundant_enable.fetch_add(1, Ordering::Relaxed);
        return;
    }
    write_cr4(cr4 | CR4_SMEP_BIT);
    SMEP_STATS.enable_count.fetch_add(1, Ordering::Relaxed);
    SMEP_ACTIVE.store(true, Ordering::Release);
}

/// Désactive temporairement SMEP — DANGER, usage très restreint.
///
/// Retourne `true` si SMEP était actif (→ caller doit appeler `restore_smep`).
/// Usage légitime : copie noyau→user en monde pré-SMAP, chemin debug uniquement.
///
/// # Safety
/// CPL 0. La fenêtre de désactivation doit être la plus courte possible.
pub unsafe fn disable_smep() -> bool {
    let cr4 = read_cr4();
    if cr4 & CR4_SMEP_BIT == 0 {
        return false;
    }
    write_cr4(cr4 & !CR4_SMEP_BIT);
    SMEP_STATS.disable_count.fetch_add(1, Ordering::Relaxed);
    SMEP_ACTIVE.store(false, Ordering::Release);
    true
}

/// Réactive SMEP si `was_active` est `true`.
///
/// # Safety
/// CPL 0.
#[inline]
pub unsafe fn restore_smep(was_active: bool) {
    if was_active {
        enable_smep();
    }
}

/// Retourne `true` si SMEP est actuellement actif sur ce CPU.
///
/// # Safety
/// CPL 0.
#[inline]
pub unsafe fn smep_active() -> bool {
    read_cr4() & CR4_SMEP_BIT != 0
}

/// Gère une violation SMEP détectée par le fault handler.
/// Incrémente `violation_count` et retourne `false` (non récupérable).
#[inline]
pub fn smep_handle_violation(fault_ip: u64) -> bool {
    SMEP_STATS.violation_count.fetch_add(1, Ordering::Relaxed);
    let _ = fault_ip;
    false
}

/// Guard RAII : désactive SMEP à la construction, le restaure au drop.
///
/// `SmepGuard::new()` retourne `None` si SMEP n'était pas actif.
///
/// # Safety
/// Le caller garantit que l'exécution user-code est intentionnelle.
pub struct SmepGuard {
    was_active: bool,
}

impl SmepGuard {
    /// # Safety
    /// CPL 0.
    pub unsafe fn new() -> Self {
        let was_active = disable_smep();
        SmepGuard { was_active }
    }
}

impl Drop for SmepGuard {
    fn drop(&mut self) {
        // SAFETY: restore_smep restaure CR4.SMEP; was_active provient de smep_enable().
        unsafe { restore_smep(self.was_active) };
    }
}

/// Initialisation du sous-système SMEP (appelé sur BSP + chaque AP).
///
/// # Safety
/// CPL 0.
pub unsafe fn init() {
    enable_smep();
}
