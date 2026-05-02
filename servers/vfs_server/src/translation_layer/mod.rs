//! ExoFS POSIX translation layer contract.
//!
//! Ring 1 owns POSIX orchestration. Ring 0 keeps the mechanisms that need
//! atomic object access: append locking, sparse lookup, copy range, mmap flush,
//! fallocate and file locking.

pub mod coverage;
pub mod errno;
pub mod flush;
pub mod musl;
pub mod posix_services;
pub mod roles;
pub mod syscalls;

pub use coverage::{coverage_summary, meets_core_target, CoverageSummary};
pub use flush::{
    durability_for_sync_file_range, sync_file_range_waits_for_completion,
    sync_file_range_waits_for_start, validate_sync_file_range_flags,
};
pub use musl::{
    is_musl_bootstrap_syscall, musl_bootstrap_by_syscall, musl_bootstrap_ready, MuslBootstrapSpec,
    MUSL_BOOTSTRAP_SERVICES,
};
pub use posix_services::{service_by_syscall, CORE_POSIX_SERVICES, PHASE2_POSIX_SERVICES};
pub use roles::{ServiceClass, ServiceStatus, SyscallRoute, TranslationRole};
pub use syscalls::{
    exofs_rights_for_open_flags, is_core_posix_syscall, is_known_linux_fs_syscall, musl_exo_alias,
    route_syscall,
};

pub fn translation_contract_is_sane() -> bool {
    let summary = coverage_summary();
    meets_core_target()
        && summary.core_total >= 60
        && service_by_syscall(exo_syscall_abi::SYS_EXOFS_OPEN_BY_PATH).is_some()
        && service_by_syscall(exo_syscall_abi::SYS_SYNC_FILE_RANGE).is_some()
        && musl_bootstrap_ready()
        && validate_sync_file_range_flags(
            exo_syscall_abi::SYNC_FILE_RANGE_WRITE | exo_syscall_abi::SYNC_FILE_RANGE_WAIT_AFTER,
        )
}
