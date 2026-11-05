//! # arch/x86_64/virt — Hypervisor detection and support
//!
//! Detects if Exo-OS is running under a hypervisor (VMware, KVM, Hyper-V, Xen...)
//! and adapts behavior accordingly (APIC, TSC, stolen memory).

pub mod detect;
pub mod paravirt;
pub mod stolen_time;

pub use detect::{detect_hypervisor, hypervisor_type, HypervisorType};
pub use paravirt::{paravirt_eoi, paravirt_tlb_flush};
pub use stolen_time::{stolen_time_ns, update_stolen_time};
