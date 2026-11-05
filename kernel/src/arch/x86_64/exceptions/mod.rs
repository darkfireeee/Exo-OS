//! # arch/x86_64/exceptions — CPU Exception and Interrupt Handling Subsystem
//!
//! Segregated modular organization:
//! - `frame`      : Saved registers ExceptionFrame, scheduler bridges, signals, stats
//! - `stubs`      : Low-level assembly stubs (global_asm!) for IDT
//! - `cpu_faults` : Hardware processor exceptions (GPF, Double Fault, UD, NM...)
//! - `page_fault` : #PF handler, CoW resolution, demand paging, and KPTI synchronization
//! - `irq`        : Peripheral IRQ and inter-core APIC IPI routing

pub mod frame;
pub mod stubs;
pub mod cpu_faults;
pub mod page_fault;
pub mod irq;

pub use frame::*;
pub use cpu_faults::*;
pub use page_fault::*;
pub use irq::*;
