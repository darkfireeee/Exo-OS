//! # arch/x86_64/spectre/ibrs.rs — IBRS / IBPB / STIBP (Spectre variant 2/3a)
//!
//! - **IBRS** (Indirect Branch Restricted Speculation) : restreint la prédiction
//!   de branches indirectes. Mode ENHANCED_IBRS si disponible (permanent).
//! - **IBPB** (Indirect Branch Predictor Barrier) : vide le prédicteur de branches.
//!   Doit être émis lors du passage kernel → userspace (process différent).
//! - **STIBP** (Single Thread Indirect Branch Predictors) : isole les HyperThreads.


use core::sync::atomic::{AtomicBool, Ordering};
use super::super::cpu::msr::{
    self, MSR_IA32_SPEC_CTRL, MSR_IA32_PRED_CMD,
    SPEC_CTRL_IBRS, SPEC_CTRL_STIBP,
    PRED_CMD_IBPB,
};

static IBRS_ENABLED:  AtomicBool = AtomicBool::new(false);
static STIBP_ENABLED: AtomicBool = AtomicBool::new(false);
static EIBRS_ACTIVE:  AtomicBool = AtomicBool::new(false); // Enhanced IBRS (permanent)

pub fn ibrs_enabled()  -> bool { IBRS_ENABLED.load(Ordering::Relaxed) }
pub fn stibp_enabled() -> bool { STIBP_ENABLED.load(Ordering::Relaxed) }

/// Initialise les mitigations IBRS/STIBP
///
/// Appelé depuis `apply_mitigations_bsp/ap()`.
pub fn init_ibrs() {
    let features = super::super::cpu::features::cpu_features();
    if !features.has_spec_ctrl() { return; }

    // Enhanced IBRS (toujours activé, pas besoin de le réactiver à chaque switch)
    if features.ibrs_all() {
        // SAFETY: MSR_IA32_SPEC_CTRL bit IBRS — disponible (CPUID 7.0 EDX[26])
        unsafe { msr::set_msr_bits(MSR_IA32_SPEC_CTRL, SPEC_CTRL_IBRS); }
        EIBRS_ACTIVE.store(true, Ordering::Release);
        IBRS_ENABLED.store(true, Ordering::Release);
    } else if features.has_ibrs() {
        IBRS_ENABLED.store(true, Ordering::Release);
        // IBRS classique : activé/désactivé à chaque switch kernel/user
        // Pour l'instant : activation globale (pénalité perf acceptable)
        // SAFETY: MSR_IA32_SPEC_CTRL write depuis Ring 0
        unsafe { msr::set_msr_bits(MSR_IA32_SPEC_CTRL, SPEC_CTRL_IBRS); }
    }

    // STIBP : activer si CPU SMT et STIBP disponible
    if features.has_stibp() {
        // SAFETY: MSR_IA32_SPEC_CTRL bit STIBP
        unsafe { msr::set_msr_bits(MSR_IA32_SPEC_CTRL, SPEC_CTRL_STIBP); }
        STIBP_ENABLED.store(true, Ordering::Release);
    }
}

/// Active IBRS manuellement (mode non-enhanced — appelé au retour kernel depuis user)
#[inline]
pub fn apply_ibrs() {
    if EIBRS_ACTIVE.load(Ordering::Relaxed) { return; } // Enhanced IBRS toujours actif
    if !IBRS_ENABLED.load(Ordering::Relaxed)  { return; }
    // SAFETY: MSR write depuis Ring 0 — séquence mitigations switch
    unsafe { msr::set_msr_bits(MSR_IA32_SPEC_CTRL, SPEC_CTRL_IBRS); }
}

/// Active STIBP
#[inline]
pub fn apply_stibp() {
    if !STIBP_ENABLED.load(Ordering::Relaxed) { return; }
    // SAFETY: MSR write depuis Ring 0
    unsafe { msr::set_msr_bits(MSR_IA32_SPEC_CTRL, SPEC_CTRL_STIBP); }
}

/// Flush le prédicteur de branches indirectes (IBPB)
///
/// ## Quand l'appeler
/// - Lors du context switch vers un process différent (IBPB flush inter-process)
/// - Toujours coûteux (~150–300 cycles) — limiter aux switchs de domaine sécurité
#[inline]
pub fn flush_ibpb() {
    let features = super::super::cpu::features::cpu_features();
    if !features.has_ibpb() { return; }
    // SAFETY: MSR_IA32_PRED_CMD write depuis Ring 0 — flush IBPB
    unsafe { msr::write_msr(MSR_IA32_PRED_CMD, PRED_CMD_IBPB); }
}

/// Flush le microcode store buffer (VERW — MDS mitigation)
///
/// Mitigue MDS (Microarchitectural Data Sampling) : Fallout, RIDL, ZombieLoad.
/// Séquence : `VERW mem16` avec un sélecteur sûr (User DS).
#[inline]
pub fn flush_mds() {
    let features = super::super::cpu::features::cpu_features();
    if !features.has_md_clear() { return; }
    // SAFETY: VERW avec le sélecteur USER_DS — flush des store buffers
    unsafe {
        let ds_sel: u16 = super::super::gdt::GDT_USER_DS;
        core::arch::asm!(
            "verw [{sel}]",
            sel = in(reg) &ds_sel,
            options(nostack, nomem, preserves_flags),
        );
    }
}
