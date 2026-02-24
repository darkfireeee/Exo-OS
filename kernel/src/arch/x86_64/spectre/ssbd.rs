//! # arch/x86_64/spectre/ssbd.rs — Speculative Store Bypass Disable (Spectre v4)
//!
//! SSBD empêche le CPU d'exécuter spéculativement des loads après des stores
//! dont l'adresse n'est pas encore résolue.
//!
//! ## Implémentation
//! Via `MSR_IA32_SPEC_CTRL` bit 2 (SSBD) ou via AMD VIRT_SPEC_CTRL.
//! Configuré per-thread (certains processus n'en ont pas besoin → performance).

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, Ordering};
use super::super::cpu::msr::{
    self, MSR_IA32_SPEC_CTRL, SPEC_CTRL_SSBD,
};

const AMD_MSR_VIRT_SPEC_CTRL: u32 = 0xC001_011F;
const AMD_SSBD_BIT: u64 = 1 << 2;

static SSBD_SYSTEM_ENABLED: AtomicBool = AtomicBool::new(false);
static SSBD_AMD_MODE:       AtomicBool = AtomicBool::new(false);

/// Retourne `true` si SSBD est activé sur ce système
pub fn ssbd_enabled() -> bool {
    SSBD_SYSTEM_ENABLED.load(Ordering::Relaxed)
}

/// Initialise SSBD au niveau système
pub fn init_ssbd() {
    let features = super::super::cpu::features::cpu_features();

    if !features.has_ssbd() && !features.ssb_no() {
        return;
    }

    // Mode AMD : VIRT_SPEC_CTRL
    if features.has_virt_ssbd() {
        SSBD_AMD_MODE.store(true, Ordering::Release);
    }

    SSBD_SYSTEM_ENABLED.store(true, Ordering::Release);
}

/// Active SSBD pour le thread courant
///
/// Appelé lors du context switch vers un thread nécessitant SSBD.
#[inline]
pub fn apply_ssbd_for_thread(enable: bool) {
    if !SSBD_SYSTEM_ENABLED.load(Ordering::Relaxed) { return; }

    if SSBD_AMD_MODE.load(Ordering::Relaxed) {
        let val: u64 = if enable { AMD_SSBD_BIT } else { 0 };
        // SAFETY: MSR AMD VIRT_SPEC_CTRL sur CPU AMD supportant SSBD
        unsafe { msr::write_msr(AMD_MSR_VIRT_SPEC_CTRL, val); }
    } else {
        if enable {
            // SAFETY: MSR_IA32_SPEC_CTRL bit SSBD — disponible si CPUID 7.0 EDX[31]
            unsafe { msr::set_msr_bits(MSR_IA32_SPEC_CTRL, SPEC_CTRL_SSBD); }
        } else {
            // SAFETY: idem
            unsafe { msr::clear_msr_bits(MSR_IA32_SPEC_CTRL, SPEC_CTRL_SSBD); }
        }
    }
}
