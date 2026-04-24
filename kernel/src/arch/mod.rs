//! # arch/ — Couche Architecture transverse
//!
//! Ce module est TRANSVERSE : il peut appeler n'importe quelle couche.
//! Il est lui-même appellé par les couches scheduler/, process/, memory/.
//!
//! ## Hiérarchie
//! ```
//! arch/ (transverse)
//!   └── x86_64/   ← implémentation principale
//!   └── aarch64/  ← placeholder ARM64 (futur)
//! ```
//!
//! ## Règles absolues
//! - arch/ orchestre la livraison des signaux (depuis syscall.rs et exceptions.rs)
//! - arch/ switch CR3 pour KPTI dans switch_asm.s
//! - arch/ ne contient PAS la logique d'état scheduler (→ scheduler/fpu/)
//! - Tout bloc `unsafe` doit être précédé d'un commentaire `// SAFETY: ...`

#[cfg(target_arch = "x86_64")]
pub mod x86_64;

#[cfg(target_arch = "aarch64")]
pub mod aarch64;

/// Lecture du TSC (timestamp counter) — disponible sur toutes les architectures.
pub mod time;

// ── Re-exports publics ────────────────────────────────────────────────────────

#[cfg(target_arch = "x86_64")]
pub use self::x86_64::{
    cpu::features::{CpuFeatures, CPU_FEATURES},
    cpu::tsc::read_tsc,
    halt_cpu,
};

#[cfg(target_arch = "aarch64")]
pub use self::aarch64::halt_cpu;

/// Informations architecture exportées vers le reste du noyau
#[derive(Debug, Clone, Copy)]
pub struct ArchInfo {
    pub cpu_count: u32,
    pub has_apic: bool,
    pub has_x2apic: bool,
    pub has_acpi: bool,
    pub page_size: usize,
}

impl Default for ArchInfo {
    fn default() -> Self {
        Self {
            cpu_count: 1,
            has_apic: false,
            has_x2apic: false,
            has_acpi: false,
            page_size: 4096,
        }
    }
}
