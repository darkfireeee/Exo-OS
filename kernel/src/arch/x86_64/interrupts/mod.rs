//! Interrupt Management for x86_64

pub mod apic;
pub mod ipi;
pub mod handlers;

pub use apic::*;
pub use ipi::*;
