//! Syscall routing for POSIX services consumed by `vfs_server`.

use super::posix_services::service_by_syscall;
use super::roles::{ServiceStatus, SyscallRoute, TranslationRole};
use exo_syscall_abi as abi;

pub fn route_syscall(nr: u64) -> SyscallRoute {
    if (abi::SYS_EXOFS_FIRST..=abi::SYS_EXOFS_LAST).contains(&nr) {
        return SyscallRoute::NativeExofs;
    }

    match service_by_syscall(nr).map(|service| service.status) {
        Some(ServiceStatus::Implemented) | Some(ServiceStatus::Delegated) => {
            let role = service_by_syscall(nr)
                .map(|service| service.role)
                .unwrap_or(TranslationRole::VfsServer);
            match role {
                TranslationRole::KernelMechanism => SyscallRoute::KernelBridge(role),
                TranslationRole::CompatRam => SyscallRoute::CompatOnly,
                TranslationRole::Phase2 => SyscallRoute::Phase2,
                _ => SyscallRoute::VfsServer(role),
            }
        }
        Some(ServiceStatus::Compat) => SyscallRoute::CompatOnly,
        Some(ServiceStatus::Phase2) => SyscallRoute::Phase2,
        None => SyscallRoute::Unsupported,
    }
}

pub fn musl_exo_alias(nr: u64) -> u64 {
    match nr {
        abi::SYS_OPEN => abi::SYS_EXOFS_OPEN_BY_PATH,
        abi::SYS_OPENAT => abi::SYS_EXOFS_OPEN_BY_PATH,
        abi::SYS_GETDENTS => abi::SYS_EXOFS_READDIR,
        abi::SYS_GETDENTS64 => abi::SYS_EXOFS_READDIR,
        _ => nr,
    }
}

pub fn is_core_posix_syscall(nr: u64) -> bool {
    service_by_syscall(musl_exo_alias(nr))
        .map(|service| service.status.counts_for_core())
        .unwrap_or(false)
}

pub fn exofs_rights_for_open_flags(flags: u64) -> u64 {
    if flags & (abi::O_WRONLY | abi::O_RDWR | abi::O_CREAT | abi::O_TRUNC | abi::O_APPEND) != 0 {
        abi::EXOFS_RIGHT_READ_WRITE as u64
    } else {
        abi::EXOFS_RIGHT_READ_ONLY as u64
    }
}

pub fn is_known_linux_fs_syscall(nr: u64) -> bool {
    is_core_posix_syscall(nr) || matches!(route_syscall(nr), SyscallRoute::Phase2)
}
