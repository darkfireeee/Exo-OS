//! # arch::time — Abstraction temporelle transverse
//!
//! Fournit `read_ticks()` indépendant de l'architecture.

/// Lit le compteur de cycles (TSC sur x86_64, systick sur ARM).
pub fn read_ticks() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::x86_64::cpu::tsc::read_tsc()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0u64
    }
}
