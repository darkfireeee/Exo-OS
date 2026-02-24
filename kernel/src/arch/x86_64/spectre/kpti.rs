//! # arch/x86_64/spectre/kpti.rs — Kernel Page-Table Isolation
//!
//! KPTI isole les tables de pages kernel/user pour mitiger Meltdown :
//! - Le kernel utilise `cr3_kernel` (accès complet)
//! - L'userspace utilise `cr3_user`   (seulement les stubs de syscall/exception mappés)
//!
//! ## Implémentation
//! Le changement de CR3 se fait dans `switch_asm.s` (entre PUSH/POP des registres).
//! Ce module gère uniquement la détection, l'état global, et les helpers de switch.

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use super::super::cpu::msr;

// ── État global KPTI ──────────────────────────────────────────────────────────

static KPTI_ENABLED: AtomicBool = AtomicBool::new(false);

/// Retourne `true` si KPTI est activé sur ce système
pub fn kpti_enabled() -> bool {
    KPTI_ENABLED.load(Ordering::Relaxed)
}

// ── Bit PCID ─────────────────────────────────────────────────────────────────

/// Bit 12 de CR3 = PCID (Process Context Identifier)
/// Bit 63 de CR3 (avec INVPCID support) = no-flush bit
const CR3_PCID_MASK:    u64 = 0xFFF;
const CR3_NO_FLUSH_BIT: u64 = 1u64 << 63;

/// PCID réservé pour le kernel (0 = pas de PCID kernel dédié)
pub const PCID_KERNEL: u64 = 0;
/// PCID réservé pour le shadow user (1 = page tables user KPTI)
pub const PCID_USER:   u64 = 1;

// ── CR3 per-thread ────────────────────────────────────────────────────────────

/// CR3 kernel de la tâche courante (adresse physique du PML4 kernel)
static CURRENT_CR3_KERNEL: AtomicU64 = AtomicU64::new(0);
/// CR3 user shadow (page table allégée)
static CURRENT_CR3_USER:   AtomicU64 = AtomicU64::new(0);

/// Enregistre le couple (cr3_kernel, cr3_user) pour la tâche courante
pub fn set_current_cr3(cr3_kernel: u64, cr3_user: u64) {
    CURRENT_CR3_KERNEL.store(cr3_kernel, Ordering::Release);
    CURRENT_CR3_USER.store(cr3_user, Ordering::Release);
}

// ── Switch Kernel → User CR3 ─────────────────────────────────────────────────

/// Bascule vers les tables de pages user (à exécuter avant IRETQ vers Ring 3)
///
/// ## RÈGLE DOC1 — KPTI
/// Appelé depuis `switch_context_asm.s` juste avant la restauration des registres.
/// NE PAS appeler depuis du code Rust arbitraire.
#[inline]
pub unsafe fn kpti_switch_to_user() {
    if !KPTI_ENABLED.load(Ordering::Relaxed) { return; }
    let cr3_user = CURRENT_CR3_USER.load(Ordering::Acquire);
    if cr3_user == 0 { return; }
    // Chargement CR3 avec no-flush bit si PCID disponible
    let features = super::super::cpu::features::cpu_features();
    let cr3 = if features.has_pcid() {
        cr3_user | PCID_USER | CR3_NO_FLUSH_BIT
    } else {
        cr3_user & !CR3_PCID_MASK
    };
    // SAFETY: cr3_user est validé par set_current_cr3
    core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack, nomem));
}

/// Bascule vers les tables de pages kernel (au retour vers Ring 0)
///
/// Appelé depuis le stub d'entrée exception/syscall.
#[inline]
pub unsafe fn kpti_switch_to_kernel() {
    if !KPTI_ENABLED.load(Ordering::Relaxed) { return; }
    let cr3_kernel = CURRENT_CR3_KERNEL.load(Ordering::Acquire);
    if cr3_kernel == 0 { return; }
    let features = super::super::cpu::features::cpu_features();
    let cr3 = if features.has_pcid() {
        cr3_kernel | PCID_KERNEL | CR3_NO_FLUSH_BIT
    } else {
        cr3_kernel & !CR3_PCID_MASK
    };
    // SAFETY: cr3_kernel est le CR3 kernel courant validé
    core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack, nomem));
}

// ── Initialisation ────────────────────────────────────────────────────────────

/// Active KPTI (appelé depuis `apply_mitigations_bsp()`)
pub fn init_kpti() {
    let features = super::super::cpu::features::cpu_features();

    // Activer SMEP si disponible (empêche le kernel d'exécuter du code user)
    if features.has_smep() {
        let mut cr4: u64;
        // SAFETY: lecture CR4
        unsafe { core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nostack, nomem)); }
        cr4 |= 1 << 20; // SMEP
        // SAFETY: écriture CR4 — activation SMEP
        unsafe { core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, nomem)); }
    }

    // Activer SMAP si disponible (empêche le kernel d'accéder aux données user sans STAC/CLAC)
    if features.has_smap() {
        let mut cr4: u64;
        // SAFETY: lecture CR4
        unsafe { core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nostack, nomem)); }
        cr4 |= 1 << 21; // SMAP
        // SAFETY: écriture CR4 — activation SMAP
        unsafe { core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, nomem)); }
    }

    KPTI_ENABLED.store(true, Ordering::Release);
}
