//! musl bootstrap calls that must be wired before full POSIX services.

use super::roles::{ServiceClass, ServiceStatus, TranslationRole};
use exo_syscall_abi as abi;

#[derive(Clone, Copy, Debug)]
pub struct MuslBootstrapSpec {
    pub name: &'static str,
    pub syscall: u64,
    pub class: ServiceClass,
    pub role: TranslationRole,
    pub status: ServiceStatus,
}

impl MuslBootstrapSpec {
    pub const fn new(
        name: &'static str,
        syscall: u64,
        class: ServiceClass,
        role: TranslationRole,
        status: ServiceStatus,
    ) -> Self {
        Self {
            name,
            syscall,
            class,
            role,
            status,
        }
    }
}

pub const MUSL_BOOTSTRAP_SERVICES: &[MuslBootstrapSpec] = &[
    MuslBootstrapSpec::new(
        "arch_prctl",
        abi::SYS_ARCH_PRCTL,
        ServiceClass::Process,
        TranslationRole::KernelMechanism,
        ServiceStatus::Delegated,
    ),
    MuslBootstrapSpec::new(
        "set_tid_address",
        abi::SYS_SET_TID_ADDRESS,
        ServiceClass::Process,
        TranslationRole::KernelMechanism,
        ServiceStatus::Delegated,
    ),
    MuslBootstrapSpec::new(
        "uname",
        abi::SYS_UNAME,
        ServiceClass::Metadata,
        TranslationRole::KernelMechanism,
        ServiceStatus::Delegated,
    ),
    MuslBootstrapSpec::new(
        "waitid",
        abi::SYS_WAITID,
        ServiceClass::Process,
        TranslationRole::KernelMechanism,
        ServiceStatus::Delegated,
    ),
    MuslBootstrapSpec::new(
        "clock_nanosleep",
        abi::SYS_CLOCK_NANOSLEEP,
        ServiceClass::Time,
        TranslationRole::KernelMechanism,
        ServiceStatus::Delegated,
    ),
];

pub fn musl_bootstrap_by_syscall(syscall: u64) -> Option<&'static MuslBootstrapSpec> {
    let mut i = 0usize;
    while i < MUSL_BOOTSTRAP_SERVICES.len() {
        if MUSL_BOOTSTRAP_SERVICES[i].syscall == syscall {
            return Some(&MUSL_BOOTSTRAP_SERVICES[i]);
        }
        i += 1;
    }
    None
}

pub fn is_musl_bootstrap_syscall(syscall: u64) -> bool {
    musl_bootstrap_by_syscall(syscall).is_some()
}

pub fn musl_bootstrap_ready() -> bool {
    let mut i = 0usize;
    while i < MUSL_BOOTSTRAP_SERVICES.len() {
        if !MUSL_BOOTSTRAP_SERVICES[i].status.counts_for_core() {
            return false;
        }
        i += 1;
    }
    true
}
