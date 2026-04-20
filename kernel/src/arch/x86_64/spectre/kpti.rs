//! # arch/x86_64/spectre/kpti.rs — Kernel Page-Table Isolation
//!
//! KPTI isole les tables de pages kernel/user pour mitiger Meltdown :
//! - Le kernel utilise `cr3_kernel` (accès complet)
//! - L'userspace utilise `cr3_user`   (seulement les stubs de syscall/exception mappés)
//!
//! ## Implémentation
//! Le changement de CR3 se fait dans `switch_asm.s` (entre PUSH/POP des registres).
//! Ce module gère uniquement la détection, l'état global, et les helpers de switch.


use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::arch::x86_64::smp::percpu;
use crate::arch::x86_64::smp::percpu::MAX_CPUS;

// ── État global KPTI ──────────────────────────────────────────────────────────

static KPTI_ENABLED: AtomicBool = AtomicBool::new(false);

/// Retourne `true` si KPTI est activé sur ce système
pub fn kpti_enabled() -> bool {
    KPTI_ENABLED.load(Ordering::Relaxed)
}

// ── Bit PCID ─────────────────────────────────────────────────────────────────

const CR3_PCID_MASK:    u64 = 0xFFF;
const CR3_NO_FLUSH_BIT: u64 = 1u64 << 63;

pub const PCID_KERNEL: u64 = 0;
pub const PCID_USER:   u64 = 1;

// ── CR3 per-CPU — FIX-KPTI-01 : remplace les deux AtomicU64 globaux ──────────
//
// AVANT (BUG) :
//   static CURRENT_CR3_KERNEL: AtomicU64 = AtomicU64::new(0); // 1 seul pour tous
//   static CURRENT_CR3_USER:   AtomicU64 = AtomicU64::new(0); // 1 seul pour tous
//
// APRÈS : un slot de 128 octets par CPU → pas de false sharing, pas de race SMP.

#[repr(C, align(128))]
struct Cr3Slot {
    kernel: AtomicU64,
    user:   AtomicU64,
    _pad:   [u8; 112],
}

impl Cr3Slot {
    const fn new() -> Self {
        Self { kernel: AtomicU64::new(0), user: AtomicU64::new(0), _pad: [0u8; 112] }
    }
}

unsafe impl Sync for Cr3Slot {}

static CR3_PER_CPU: [Cr3Slot; MAX_CPUS] = {
    const SLOT: Cr3Slot = Cr3Slot::new();
    [SLOT; MAX_CPUS]
};

#[inline(always)]
fn current_cpu_slot() -> &'static Cr3Slot {
    let cpu_id = percpu::current_cpu_id() as usize;
    &CR3_PER_CPU[cpu_id.min(MAX_CPUS - 1)]
}

/// Enregistre le couple (cr3_kernel, cr3_user) pour le thread courant sur CE CPU.
/// Doit être appelé après chaque context switch (FIX-KPTI-01).
pub fn set_current_cr3(cr3_kernel: u64, cr3_user: u64) {
    let slot = current_cpu_slot();
    slot.kernel.store(cr3_kernel, Ordering::Release);
    slot.user.store(cr3_user,   Ordering::Release);
}

// ── Switch Kernel → User CR3 ─────────────────────────────────────────────────

/// Bascule vers les tables de pages user (à exécuter avant IRETQ vers Ring 3)
#[inline]
pub unsafe fn kpti_switch_to_user() {
    if !KPTI_ENABLED.load(Ordering::Relaxed) { return; }
    let slot     = current_cpu_slot();
    let cr3_user = slot.user.load(Ordering::Acquire);
    if cr3_user == 0 { return; }
    let features = super::super::cpu::features::cpu_features();
    let cr3 = if features.has_pcid() {
        cr3_user | PCID_USER | CR3_NO_FLUSH_BIT
    } else {
        cr3_user & !CR3_PCID_MASK
    };
    core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack, nomem));
}

/// Bascule vers les tables de pages kernel (au retour vers Ring 0)
#[inline]
pub unsafe fn kpti_switch_to_kernel() {
    if !KPTI_ENABLED.load(Ordering::Relaxed) { return; }
    let slot       = current_cpu_slot();
    let cr3_kernel = slot.kernel.load(Ordering::Acquire);
    if cr3_kernel == 0 { return; }
    let features = super::super::cpu::features::cpu_features();
    let cr3 = if features.has_pcid() {
        cr3_kernel | PCID_KERNEL | CR3_NO_FLUSH_BIT
    } else {
        cr3_kernel & !CR3_PCID_MASK
    };
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

    let cpu_id = crate::arch::x86_64::smp::percpu::current_cpu_id() as usize;
    let kernel_pml4_phys = crate::memory::virt::page_table::read_cr3();
    let trampoline_phys = crate::memory::core::PhysAddr::new(
        crate::arch::x86_64::smp::init::TRAMPOLINE_PHYS,
    );

    let user_shadow = unsafe {
        crate::memory::virt::page_table::kpti_split::build_user_shadow_pml4(kernel_pml4_phys)
    };

    match user_shadow {
        Ok(user_pml4_phys) => {
            unsafe {
                crate::memory::virt::page_table::kpti_split::KPTI.register_cpu(
                    cpu_id,
                    kernel_pml4_phys,
                    user_pml4_phys,
                    trampoline_phys,
                );
            }
            // FIX-KPTI-01 : initialiser le slot per-CPU lors de l'activation.
            set_current_cr3(kernel_pml4_phys.as_u64(), user_pml4_phys.as_u64());
            KPTI_ENABLED.store(true, Ordering::Release);
        }
        Err(_) => {
            log::warn!("KPTI: impossible d'allouer la user_pml4 shadow sur CPU {}", cpu_id);
        }
    }
}
