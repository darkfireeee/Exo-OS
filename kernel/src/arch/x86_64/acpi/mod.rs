//! # arch/x86_64/acpi — ACPI (Advanced Configuration and Power Interface)
//!
//! Parseur minimal des tables ACPI nécessaires au démarrage du kernel :
//! - RSDP → XSDT/RSDT
//! - MADT (SMP topology, LAPIC IDs, IOAPIC)
//! - HPET (High Precision Event Timer)
//! - PM Timer (ACPI power management timer)

pub mod hpet;
pub mod madt;
pub mod parser;
pub mod pm_timer;

pub use hpet::{hpet_read_counter, init_hpet, HpetInfo};
pub use madt::{parse_madt, MadtInfo};
pub use parser::{init_acpi, AcpiInfo};
pub use pm_timer::{init_pm_timer, pm_timer_read_ms};
