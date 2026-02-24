//! # arch/x86_64/smp — Symmetric Multi-Processing
//!
//! Démarrage des Application Processors (APs) et données per-CPU.
//!
//! ## Séquence SMP boot
//! 1. BSP parse MADT → liste des APIC IDs
//! 2. BSP écrit le trampoline en mémoire basse (< 1 MB)
//! 3. BSP envoie INIT + SIPI × 2 à chaque AP
//! 4. AP exécute le trampoline (real mode → protected → 64 bits)
//! 5. AP appelle `ap_entry()` — initialise GDT, IDT, TSS, LAPIC, FPU
//! 6. AP se signale "online" et entre dans la boucle scheduler idle

pub mod init;
pub mod percpu;
pub mod hotplug;

pub use init::{smp_boot_aps, ap_entry, smp_cpu_count};
pub use percpu::{PerCpuData, per_cpu, per_cpu_mut, init_percpu_for_bsp, init_percpu_for_ap};
pub use hotplug::{cpu_online, cpu_offline, cpu_is_online};
