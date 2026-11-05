//! # arch/x86_64/spectre/ibrs.rs — IBRS / IBPB / STIBP (Spectre variant 2/3a)
//!
//! - **IBRS** (Indirect Branch Restricted Speculation): restricts indirect branch
//!   prediction. ENHANCED_IBRS mode if available (permanent).
//! - **IBPB** (Indirect Branch Predictor Barrier): flushes branch predictor.
//!   Must be issued during kernel → userspace transition (different process).
//! - **STIBP** (Single Thread Indirect Branch Predictors): isolates HyperThreads.

use super::super::cpu::msr::{
    self, MSR_IA32_PRED_CMD, MSR_IA32_SPEC_CTRL, PRED_CMD_IBPB, SPEC_CTRL_IBRS, SPEC_CTRL_STIBP,
};
use core::sync::atomic::{AtomicBool, Ordering};

static IBRS_ENABLED: AtomicBool = AtomicBool::new(false);
static STIBP_ENABLED: AtomicBool = AtomicBool::new(false);
static EIBRS_ACTIVE: AtomicBool = AtomicBool::new(false); // Enhanced IBRS (permanent)

pub fn ibrs_enabled() -> bool {
    IBRS_ENABLED.load(Ordering::Relaxed)
}
pub fn stibp_enabled() -> bool {
    STIBP_ENABLED.load(Ordering::Relaxed)
}

/// Initializes IBRS/STIBP mitigations
///
/// Called from `apply_mitigations_bsp/ap()`.
pub fn init_ibrs() {
    let features = super::super::cpu::features::cpu_features();
    if !features.has_spec_ctrl() {
        return;
    }

    // Enhanced IBRS (always enabled, no need to re-enable on each switch)
    if features.ibrs_all() {
        // SAFETY: MSR_IA32_SPEC_CTRL bit IBRS — available (CPUID 7.0 EDX[26])
        unsafe {
            msr::set_msr_bits(MSR_IA32_SPEC_CTRL, SPEC_CTRL_IBRS);
        }
        EIBRS_ACTIVE.store(true, Ordering::Release);
        IBRS_ENABLED.store(true, Ordering::Release);
    } else if features.has_ibrs() {
        IBRS_ENABLED.store(true, Ordering::Release);
        // Classic IBRS: enabled/disabled on each kernel/user switch
        // Currently: global activation (acceptable performance penalty)
        // SAFETY: MSR_IA32_SPEC_CTRL write from Ring 0
        unsafe {
            msr::set_msr_bits(MSR_IA32_SPEC_CTRL, SPEC_CTRL_IBRS);
        }
    }

    // STIBP: enable if CPU SMT and STIBP available
    if features.has_stibp() {
        // SAFETY: MSR_IA32_SPEC_CTRL bit STIBP
        unsafe {
            msr::set_msr_bits(MSR_IA32_SPEC_CTRL, SPEC_CTRL_STIBP);
        }
        STIBP_ENABLED.store(true, Ordering::Release);
    }
}

/// Enables IBRS manually (non-enhanced mode — called on kernel return from user)
#[inline]
pub fn apply_ibrs() {
    if EIBRS_ACTIVE.load(Ordering::Relaxed) {
        return;
    } // Enhanced IBRS always active
    if !IBRS_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    // SAFETY: MSR write from Ring 0 — switch mitigation sequence
    unsafe {
        msr::set_msr_bits(MSR_IA32_SPEC_CTRL, SPEC_CTRL_IBRS);
    }
}

/// Enables STIBP
#[inline]
pub fn apply_stibp() {
    if !STIBP_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    // SAFETY: MSR write from Ring 0
    unsafe {
        msr::set_msr_bits(MSR_IA32_SPEC_CTRL, SPEC_CTRL_STIBP);
    }
}

/// Flushes indirect branch predictor (IBPB)
///
/// ## When to call
/// - On context switch to a different process (inter-process IBPB flush)
/// - Consistently costly (~150–300 cycles) — restrict to security domain switches
#[inline]
pub fn flush_ibpb() {
    let features = super::super::cpu::features::cpu_features();
    if !features.has_ibpb() {
        return;
    }
    // SAFETY: MSR_IA32_PRED_CMD write from Ring 0 — IBPB flush
    unsafe {
        msr::write_msr(MSR_IA32_PRED_CMD, PRED_CMD_IBPB);
    }
}

/// Flushes microcode store buffer (VERW — MDS mitigation)
///
/// Mitigates MDS (Microarchitectural Data Sampling): Fallout, RIDL, ZombieLoad.
/// Sequence: `VERW mem16` with a safe selector (User DS).
#[inline]
pub fn flush_mds() {
    let features = super::super::cpu::features::cpu_features();
    if !features.has_md_clear() {
        return;
    }
    // SAFETY: VERW with USER_DS selector — store buffers flush
    unsafe {
        let ds_sel: u16 = super::super::gdt::GDT_USER_DS;
        core::arch::asm!(
            "verw [{sel}]",
            sel = in(reg) &ds_sel,
            options(nostack, preserves_flags),
        );
    }
}
