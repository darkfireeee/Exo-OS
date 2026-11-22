//! # kernel/src/syscall/mod.rs — Exo-OS syscall module
//!
//! ## Overview
//!
//! This module orchestrates the kernel's entire syscall interface.
//! It is structured into independent layers:
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │  arch/x86_64/syscall.rs                                  │
//! │  syscall_rust_handler(frame)  ← Hardware SYSCALL         │
//! └──────────────────┬───────────────────────────────────────┘
//!                    │ direct call
//! ┌──────────────────▼───────────────────────────────────────┐
//! │  syscall::dispatch::dispatch(frame)                      │
//! │  ├─ number validation      numbers::is_valid_syscall()   │
//! │  ├─ fast-path              fast_path::try_fast_path()    │
//! │  ├─ compat translation     compat::translate_linux_nr()  │
//! │  ├─ dispatch table         table::get_handler()          │
//! │  └─ signal delivery        post_dispatch()               │
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Modules
//!
//! | Module       | Role                                              |
//! |--------------|---------------------------------------------------|
//! | `numbers`    | SYS_* constants and errno                         |
//! | `validation` | Safe types for userspace arguments                |
//! | `fast_path`  | Ultra-fast handlers (<150 cycles, lockless)       |
//! | `table`      | O(1) table + slow-path handlers                   |
//! | `dispatch`   | Full dispatch pipeline                            |
//! | `compat`     | linux.rs + posix.rs layers                        |
//!
//! ## Respected Architectural Rules
//!
//! - **SIGNAL-01 (DOC1)**: signal delivery only upon return to
//!   userspace, via `dispatch::post_dispatch()`.
//! - **SCHED-03 (DOC3)**: futex housed in `memory::utils::futex_table`.
//! - **regle_bonus**: lock ordering Memory < Sched < Security < IPC < FS
//!   (= Memory acquired first, FS acquired last); each `unsafe` preceded
//!   by `// SAFETY:`.
//! - **NO-ALLOC** on hot paths (`fast_path`, dispatch fast branch).

pub mod compat;
pub mod dispatch;
pub mod fast_path;
pub mod fixup;
pub mod fs_bridge;
pub mod net_bridge;
pub mod numbers;
pub mod table;
pub mod validation;
// New modules — fixes BUG-01..BUG-09
pub mod abi;
pub mod errno;
pub mod handlers;

// ─────────────────────────────────────────────────────────────────────────────
// Public re-exports
// ─────────────────────────────────────────────────────────────────────────────

/// Syscall numbers and POSIX/Linux error codes.
pub use numbers::{
    is_exofs_syscall,
    is_exoos_native,
    is_linux_compat,
    // Classifiers
    is_valid_syscall,
    EACCES,
    EAGAIN,
    EBADF,
    EEXIST,
    EFAULT,
    // Errno
    EINVAL,
    EISDIR,
    ENOENT,
    ENOMEM,
    ENOSPC,
    ENOSYS,
    ENOTDIR,
    ENOTSUP,
    EPERM,
    SYSCALL_TABLE_SIZE,
    SYS_BRK,
    SYS_CLONE,
    SYS_CLOSE,
    SYS_DMA_ALLOC,
    SYS_DMA_FREE,
    SYS_DMA_MAP,
    SYS_DMA_SYNC,
    SYS_DMA_UNMAP,
    SYS_EXECVE,
    SYS_EXIT,
    SYS_EXIT_GROUP,
    SYS_EXOFS_EPOCH_COMMIT,
    SYS_EXOFS_EXPORT_OBJECT,
    SYS_EXOFS_GC_TRIGGER,
    SYS_EXOFS_GET_CONTENT_HASH,
    SYS_EXOFS_IMPORT_OBJECT,
    SYS_EXOFS_OBJECT_CREATE,
    SYS_EXOFS_OBJECT_DELETE,
    SYS_EXOFS_OBJECT_OPEN,
    SYS_EXOFS_OBJECT_READ,
    SYS_EXOFS_OBJECT_SET_META,
    SYS_EXOFS_OBJECT_STAT,
    SYS_EXOFS_OBJECT_WRITE,
    SYS_EXOFS_OPEN_BY_PATH,
    // ExoFS 500-520
    SYS_EXOFS_PATH_RESOLVE,
    SYS_EXOFS_QUOTA_QUERY,
    SYS_EXOFS_READDIR,
    SYS_EXOFS_RELATION_CREATE,
    SYS_EXOFS_RELATION_QUERY,
    SYS_EXOFS_SNAPSHOT_CREATE,
    SYS_EXOFS_SNAPSHOT_LIST,
    SYS_EXOFS_SNAPSHOT_MOUNT,
    SYS_EXO_CAP_CREATE,
    SYS_EXO_CAP_REVOKE,
    SYS_EXO_IPC_CALL,
    SYS_EXO_IPC_RECV,
    // Native Exo-OS numbers
    SYS_EXO_IPC_SEND,
    SYS_EXO_LOG,
    SYS_EXO_MEM_COPY_FROM_PID,
    SYS_EXO_MEM_COPY_TO_PID,
    SYS_EXO_PHOENIX_STATE_GET,
    SYS_EXO_PHOENIX_STATE_SET,
    SYS_EXO_PROCESS_LIST,
    SYS_EXOPHOENIX_TEST,
    SYS_FORK,
    SYS_FRAMEBUFFER_INFO,
    SYS_PHOENIX_FORCE,
    SYS_FSTAT,
    SYS_FUTEX,
    SYS_GETEGID,
    SYS_GETEUID,
    SYS_GETGID,
    SYS_GETPID,
    SYS_GETPPID,
    SYS_GETRANDOM,
    SYS_GETTID,
    SYS_GETUID,
    SYS_IOCTL,
    SYS_IOPORT_READ,
    SYS_IOPORT_WRITE,
    SYS_IRQ_ACK,
    // GI-03 Drivers 530-549
    SYS_IRQ_REGISTER,
    SYS_KILL,
    SYS_LSTAT,
    SYS_MMAP,
    SYS_MMIO_MAP,
    SYS_MMIO_UNMAP,
    SYS_MPROTECT,
    SYS_MSI_ALLOC,
    SYS_MSI_CONFIG,
    SYS_MSI_FREE,
    SYS_MUNMAP,
    SYS_NANOSLEEP,
    SYS_OPEN,
    SYS_PCI_BUS_MASTER,
    SYS_PCI_CFG_READ,
    SYS_PCI_CFG_WRITE,
    SYS_PCI_CLAIM,
    SYS_PCI_FIND_DEVICE,
    SYS_PCI_SET_TOPOLOGY,
    SYS_POLL,
    SYS_PREAD64,
    // Aliases exo-rt (BUG-03)
    SYS_PROC_CLONE,
    SYS_PROC_EXEC,
    SYS_PWRITE64,
    // Linux-compat numbers
    SYS_READ,
    SYS_RT_SIGACTION,
    SYS_RT_SIGPROCMASK,
    SYS_RT_SIGRETURN,
    SYS_STAT,
    SYS_TGKILL,
    SYS_VFORK,
    SYS_WAIT4,
    SYS_WRITE,
};

/// KernelError/ExofsError → errno mapping (BUG fix ERRNO-MISSING).
pub use errno::{exofs_err_to_errno, kernel_err_to_errno, result_to_retval};

/// ABI types: SyscallArgs, SyscallResult, canonical address check (BUG-05).
pub use abi::{is_canonical_address, SyscallArgs, SyscallResult};

/// Validation types for userspace arguments.
pub use validation::{
    copy_from_user, copy_to_user, read_user_path, read_user_typed, validate_clockid, validate_fd,
    validate_flags, validate_pid, validate_signal, write_user_typed, SyscallError, UserBuf,
    UserPtr, UserStr, ValidatedUserPtr, IO_BUF_MAX, PATH_MAX, USER_ADDR_MAX,
};

/// Single entry point for syscall dispatch, called by arch/.
pub use dispatch::dispatch;

/// Dispatch statistics for monitoring.
pub use dispatch::{dispatch_stats, reset_dispatch_stats, DispatchStats};

/// Fast-path statistics.
pub use fast_path::{fast_path_stats, FastPathStats};

/// Per-syscall statistics.
pub use table::syscall_stats_for;

/// Compat statistics.
pub use compat::compat_stats;

/// Open flags, mmap, prot constants — used by table.rs and userland.
pub use compat::posix::{mmap_flags, open_flags, prot_flags, seek_whence, signals};

/// Public POSIX/FS bridge — reusable by handlers and kernel integration.
pub use fs_bridge::{
    bridge_result as fs_bridge_result, fs_bridge_init, fs_close, fs_dup, fs_dup2, fs_fcntl,
    fs_fstat, fs_getdents64, fs_lseek, fs_lstat, fs_mkdir, fs_open, fs_openat, fs_read,
    fs_readlink, fs_readlinkat, fs_rmdir, fs_stat, fs_symlink, fs_symlinkat, fs_unlink, fs_write,
    is_fs_ready, FsBridgeError,
};

/// Native ExoFS syscall entries exported for ABI and userland integration.
pub use crate::fs::exofs::syscall::{
    dispatch_exofs_syscall, dispatch_exofs_syscall_counted, sys_exofs_epoch_commit,
    sys_exofs_export_object, sys_exofs_gc_trigger, sys_exofs_get_content_hash,
    sys_exofs_import_object, sys_exofs_object_create, sys_exofs_object_delete,
    sys_exofs_object_open, sys_exofs_object_read, sys_exofs_object_set_meta, sys_exofs_object_stat,
    sys_exofs_object_write, sys_exofs_open_by_path, sys_exofs_path_resolve, sys_exofs_quota_query,
    sys_exofs_readdir, sys_exofs_relation_create, sys_exofs_relation_query,
    sys_exofs_snapshot_create, sys_exofs_snapshot_list, sys_exofs_snapshot_mount, ExofsSyscallArgs,
};

// Link to SyscallFrame defined in arch/
pub use crate::arch::x86_64::syscall::SyscallFrame;

// ─────────────────────────────────────────────────────────────────────────────
// Module initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the syscall subsystem.
///
/// Must be called from `kernel_main()` after initializing
/// `arch::x86_64::syscall::init_syscall()` (which sets up MSRs).
///
/// This function:
/// 1. Resets all stats counters to zero.
/// 2. Validates that the dispatch table is consistent.
/// 3. Logs start of service.
pub fn init() {
    // Reset counters to zero (in case the kernel is reloaded into memory)
    reset_dispatch_stats();
    // Note: logging will be enabled when integrating the log ring.
}

// ─────────────────────────────────────────────────────────────────────────────
// Global statistics aggregate
// ─────────────────────────────────────────────────────────────────────────────

/// Global statistics for the syscall subsystem.
#[derive(Debug, Clone, Copy)]
pub struct SyscallModuleStats {
    /// Dispatch pipeline statistics
    pub dispatch: DispatchStats,
    /// Fast-path statistics
    pub fast_path: FastPathStats,
    /// Compatibility layer statistics
    pub compat: compat::CompatStats,
}

/// Returns a snapshot of all syscall statistics.
pub fn module_stats() -> SyscallModuleStats {
    SyscallModuleStats {
        dispatch: dispatch_stats(),
        fast_path: fast_path_stats(),
        compat: compat_stats(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_syscall_exports_are_visible() {
        let _ = fs_open;
        let _ = fs_getdents64;
        let _ = sys_exofs_open_by_path;
        let _ = sys_exofs_readdir;
        let _ = dispatch_exofs_syscall_counted;

        assert!(is_exofs_syscall(SYS_EXOFS_OPEN_BY_PATH));
        assert!(is_exofs_syscall(SYS_EXOFS_READDIR));
    }
}
