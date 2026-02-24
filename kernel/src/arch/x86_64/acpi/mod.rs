//! # arch/x86_64/acpi — ACPI (Advanced Configuration and Power Interface)
//!
//! Parseur minimal des tables ACPI nécessaires au démarrage du kernel :
//! - RSDP → XSDT/RSDT
//! - MADT (SMP topology, LAPIC IDs, IOAPIC)
//! - HPET (High Precision Event Timer)
//! - PM Timer (ACPI power management timer)

pub mod parser;
pub mod madt;
pub mod hpet;
pub mod pm_timer;

pub use parser::{init_acpi, AcpiInfo};
pub use madt::{parse_madt, MadtInfo};
pub use hpet::{init_hpet, hpet_read_counter, HpetInfo};
pub use pm_timer::{init_pm_timer, pm_timer_read_ms};
