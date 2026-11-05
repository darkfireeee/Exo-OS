//! # arch::time — Cross-cutting Time Abstraction
//!
//! Provides `read_ticks()` independent of the architecture.

/// Reads the cycle counter (TSC on x86_64, systick on ARM).
pub fn read_ticks() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::x86_64::cpu::tsc::read_tsc()
    }
    // FIX-KRN-9 (analysis_report §6.2): return CNTVCT_EL0 on AArch64
    // instead of 0, so that the monotonic clock works on ARM.
    // On any other arch, 0u64 remains the fallback.
    #[cfg(target_arch = "aarch64")]
    {
        let count: u64;
        // SAFETY: CNTVCT_EL0 is readable from EL0 when CNTKCTL_EL1.EL0VCTEN=1.
        // In kernel mode (EL1/EL2), access is always permitted.
        unsafe {
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) count, options(nostack, nomem));
        }
        count
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        0u64
    }
}
