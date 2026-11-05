//! # arch/x86_64/spectre/ssbd.rs — Speculative Store Bypass Disable (Spectre v4)
//!
//! SSBD prevents the CPU from speculatively executing loads after stores
//! whose address is not yet resolved.
//!
//! ## Implementation
//! Via `MSR_IA32_SPEC_CTRL` bit 2 (SSBD) or via AMD VIRT_SPEC_CTRL.
//! Configured per-thread (certain processes do not need it → performance).

use super::super::cpu::msr::{self, MSR_IA32_SPEC_CTRL, SPEC_CTRL_SSBD};
use core::sync::atomic::{AtomicBool, Ordering};

const AMD_MSR_VIRT_SPEC_CTRL: u32 = 0xC001_011F;
const AMD_SSBD_BIT: u64 = 1 << 2;

static SSBD_SYSTEM_ENABLED: AtomicBool = AtomicBool::new(false);
static SSBD_AMD_MODE: AtomicBool = AtomicBool::new(false);

/// Returns `true` if SSBD is enabled on this system
pub fn ssbd_enabled() -> bool {
    SSBD_SYSTEM_ENABLED.load(Ordering::Relaxed)
}

/// Initializes SSBD at system level
pub fn init_ssbd() {
    let features = super::super::cpu::features::cpu_features();

    // If the CPU is not vulnerable (ssb_no) or does not support SSBD (!has_ssbd), nothing to do.
    if features.ssb_no() || !features.has_ssbd() {
        return;
    }

    // AMD mode: VIRT_SPEC_CTRL
    if features.has_virt_ssbd() {
        SSBD_AMD_MODE.store(true, Ordering::Release);
    }

    SSBD_SYSTEM_ENABLED.store(true, Ordering::Release);
}

/// Enables SSBD for current thread
///
/// Called during context switch to a thread requiring SSBD.
#[inline]
pub fn apply_ssbd_for_thread(enable: bool) {
    if !SSBD_SYSTEM_ENABLED.load(Ordering::Relaxed) {
        return;
    }

    if SSBD_AMD_MODE.load(Ordering::Relaxed) {
        let val: u64 = if enable { AMD_SSBD_BIT } else { 0 };
        // SAFETY: AMD VIRT_SPEC_CTRL MSR on AMD CPU supporting SSBD
        unsafe {
            msr::write_msr(AMD_MSR_VIRT_SPEC_CTRL, val);
        }
    } else {
        if enable {
            // SAFETY: MSR_IA32_SPEC_CTRL bit SSBD — available if CPUID 7.0 EDX[31]
            unsafe {
                msr::set_msr_bits(MSR_IA32_SPEC_CTRL, SPEC_CTRL_SSBD);
            }
        } else {
            // SAFETY: same as above
            unsafe {
                msr::clear_msr_bits(MSR_IA32_SPEC_CTRL, SPEC_CTRL_SSBD);
            }
        }
    }
}
