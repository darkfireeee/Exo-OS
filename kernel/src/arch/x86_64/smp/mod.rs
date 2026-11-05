//! # arch/x86_64/smp — Symmetric Multi-Processing
//!
//! Application Processor (AP) startup and per-CPU data.
//!
//! ## SMP Boot Sequence
//! 1. BSP parses MADT → list of APIC IDs
//! 2. BSP writes the trampoline into low memory (< 1 MB)
//! 3. BSP sends INIT + SIPI × 2 to each AP
//! 4. AP executes the trampoline (real mode → protected → 64-bit)
//! 5. AP calls `ap_entry()` — initializes GDT, IDT, TSS, LAPIC, FPU
//! 6. AP marks itself "online" and enters the scheduler idle loop

pub mod hotplug;
pub mod init;
pub mod percpu;

pub use hotplug::{cpu_is_online, cpu_offline, cpu_online};
pub use init::{ap_entry, smp_boot_aps, smp_boot_complete, smp_cpu_count};
pub use percpu::{init_percpu_for_ap, init_percpu_for_bsp, per_cpu, per_cpu_mut, PerCpuData};
