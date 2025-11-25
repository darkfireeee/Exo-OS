//! Interrupt Management for x86_64

pub mod apic;
pub mod ioapic;
pub mod handlers;

pub use apic::*;
pub use ioapic::*;
