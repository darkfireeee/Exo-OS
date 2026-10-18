//! # arch/x86_64/exceptions — Sous-système de gestion des exceptions et interruptions CPU
//!
//! Organisation modulaire ségréguée :
//! - `frame`      : Registres sauvegardés ExceptionFrame, ponts scheduler, signaux, stats
//! - `stubs`      : Stubs d'assemblage bas niveau (global_asm!) pour IDT
//! - `cpu_faults` : Exceptions matérielles processeur (GPF, Double Fault, UD, NM...)
//! - `page_fault` : Gestionnaire #PF, résolution CoW, demand paging et synchronisation KPTI
//! - `irq`        : Routage des IRQ périphériques et IPIs inter-cœurs APIC

pub mod frame;
pub mod stubs;
pub mod cpu_faults;
pub mod page_fault;
pub mod irq;

pub use frame::*;
pub use cpu_faults::*;
pub use page_fault::*;
pub use irq::*;
