//! # arch/ — Cross-cutting Architecture Layer
//!
//! This module is CROSS-CUTTING: it can call any layer.
//! It is itself called by the scheduler/, process/, memory/ layers.
//!
//! ## Hierarchy
//! ```
//! arch/ (cross-cutting)
//!   └── x86_64/   ← main implementation
//!   └── aarch64/  ← ARM64 placeholder (future)
//! ```
//!
//! ## Absolute Rules
//! - arch/ orchestrates signal delivery (from syscall.rs and exceptions.rs)
//! - arch/ switches CR3 for KPTI in switch_asm.s
//! - arch/ does NOT contain scheduler state logic (→ scheduler/fpu/)
//! - Every `unsafe` block must be preceded by a `// SAFETY: ...` comment

#[cfg(target_arch = "x86_64")]
pub mod x86_64;

#[cfg(target_arch = "aarch64")]
pub mod aarch64;

pub mod constants;

/// Reading the TSC (timestamp counter) — available on all architectures.
pub mod time;

// ── Public re-exports ─────────────────────────────────────────────────────────

#[cfg(target_arch = "x86_64")]
pub use self::x86_64::{
    cpu::features::{CpuFeatures, CPU_FEATURES},
    cpu::tsc::read_tsc,
    halt_cpu,
};

#[cfg(target_arch = "aarch64")]
pub use self::aarch64::halt_cpu;

/// Architecture information exported to the rest of the kernel
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
