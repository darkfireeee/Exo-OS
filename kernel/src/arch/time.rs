//! # arch::time — Abstraction temporelle transverse
//!
//! Fournit `read_ticks()` indépendant de l'architecture.

/// Lit le compteur de cycles (TSC sur x86_64, systick sur ARM).
pub fn read_ticks() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::x86_64::cpu::tsc::read_tsc()
    }
    // FIX-KRN-9 (rapport_analyse §6.2) : retourner CNTVCT_EL0 sur AArch64
    // au lieu de 0, pour que le monotonic clock fonctionne sur ARM.
    // Sur tout autre arch, 0u64 reste le fallback.
    #[cfg(target_arch = "aarch64")]
    {
        let count: u64;
        // SAFETY: CNTVCT_EL0 est lisible depuis EL0 quand CNTKCTL_EL1.EL0VCTEN=1.
        // En kernel (EL1/EL2), l'accès est toujours autorisé.
        unsafe { core::arch::asm!("mrs {}, cntvct_el0", out(reg) count, options(nostack, nomem)); }
        count
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        0u64
    }
}
