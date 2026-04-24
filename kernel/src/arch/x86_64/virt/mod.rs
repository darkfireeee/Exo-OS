//! # arch/x86_64/virt — Détection et support hyperviseur
//!
//! Détecte si Exo-OS tourne sous un hyperviseur (VMware, KVM, Hyper-V, Xen...)
//! et adapte le comportement (APIC, TSC, mémoire volée).

pub mod detect;
pub mod paravirt;
pub mod stolen_time;

pub use detect::{detect_hypervisor, hypervisor_type, HypervisorType};
pub use paravirt::{paravirt_eoi, paravirt_tlb_flush};
pub use stolen_time::{stolen_time_ns, update_stolen_time};
