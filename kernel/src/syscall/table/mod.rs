//! # syscall/table/mod.rs — Table de dispatch O(1) et orchestration des handlers syscall
//!
//! Ce module indexe et expose l'intégralité des handlers de syscalls d'Exo-OS,
//! répartis de manière propre et ségréguée par domaine d'activité :
//! - `common`  : Types ABI, statistiques d'appels, ENOSYS et validateurs d'arguments.
//! - `fs`      : I/O, système de fichiers, descripteurs et VFS.
//! - `memory`  : Allocations virtuelles, mmap, brk, mprotect.
//! - `process` : Cycle de vie des processus et threads, signaux, identités.
//! - `signal`  : Routage et masquage des signaux POSIX.
//! - `time`    : Horloges, temporisateurs, nanosleep et futex.
//! - `ipc`     : IPC natif Exo-OS, capabilities, ExoCordon et ExoShield.
//! - `drivers` : Pilotes GI-03, DMA, bus PCI, vecteurs IRQ et MSI.

pub mod common;
pub mod drivers;
pub mod fs;
pub mod ipc;
pub mod memory;
pub mod process;
pub mod signal;
pub mod time;

pub use common::*;
pub use drivers::*;
pub use fs::*;
pub use ipc::*;
pub use memory::*;
pub use process::*;
pub use signal::*;
pub use time::*;

use crate::syscall::numbers::*;

// ─────────────────────────────────────────────────────────────────────────────
// Construction de la table
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le handler associé au numéro `nr`, ou `sys_enosys` si non implémenté.
///
/// Pas de verrou nécessaire : la table est en `.rodata`, lecture pure.
///
/// Performance : O(1) — un seul indirect load depuis `.rodata`.
#[inline]
pub fn get_handler(nr: u64) -> SyscallHandler {
    // Borne basse : vérification par dispatch.rs avant cet appel.
    // Ici on fait confiance à dispatch.rs pour la borne.
    match nr {
        // ── I/O, Fichiers ──────────────────────────────────────────────────
        SYS_READ => sys_read,
        SYS_WRITE => sys_write,
        SYS_PREAD64 => sys_pread64,
        SYS_PWRITE64 => sys_pwrite64,
        SYS_READV => sys_readv,
        SYS_WRITEV => sys_writev,
        SYS_POLL => sys_poll,
        SYS_SELECT => sys_select,
        SYS_PPOLL => sys_ppoll,
        SYS_PSELECT6 => sys_pselect6,
        SYS_OPEN => sys_open,
        SYS_CLOSE => sys_close,
        SYS_STAT => sys_stat,
        SYS_FSTAT => sys_fstat,
        SYS_LSTAT => sys_lstat,
        SYS_NEWFSTATAT => sys_newfstatat,
        SYS_STATX => sys_statx,
        SYS_LSEEK => sys_lseek,
        SYS_DUP => sys_dup,
        SYS_DUP2 => sys_dup2,
        SYS_DUP3 => sys_dup3,
        SYS_PIPE => sys_pipe,
        SYS_PIPE2 => sys_pipe2,
        SYS_FCNTL => sys_fcntl,
        SYS_FLOCK => sys_flock,
        SYS_IOCTL => sys_ioctl,
        SYS_FSYNC => sys_fsync,
        SYS_FDATASYNC => sys_fdatasync,
        SYS_SYNC => sys_sync,
        SYS_SYNC_FILE_RANGE => sys_sync_file_range,
        SYS_MSYNC => sys_msync,
        SYS_MKDIR => sys_mkdir,
        SYS_MKDIRAT => sys_mkdirat,
        SYS_RMDIR => sys_rmdir,
        SYS_UNLINK => sys_unlink,
        SYS_UNLINKAT => sys_unlinkat,
        SYS_MKNOD => sys_mknod,
        SYS_MKNODAT => sys_mknodat,
        SYS_RENAME => sys_rename,
        SYS_RENAMEAT => sys_renameat,
        SYS_RENAMEAT2 => sys_renameat2,
        SYS_LINK => sys_link,
        SYS_LINKAT => sys_linkat,
        SYS_CREAT => sys_creat,
        SYS_ACCESS => sys_access,
        SYS_FACCESSAT => sys_faccessat,
        SYS_SYMLINK => sys_symlink,
        SYS_GETCWD => sys_getcwd,
        SYS_CHDIR => sys_chdir,
        SYS_FCHDIR => sys_fchdir,
        SYS_TRUNCATE => sys_truncate,
        SYS_FTRUNCATE => sys_ftruncate,
        SYS_FALLOCATE => sys_fallocate,
        SYS_COPY_FILE_RANGE => sys_copy_file_range,
        SYS_SENDFILE => sys_sendfile,
        SYS_SPLICE => sys_splice,
        SYS_TEE => sys_tee,
        SYS_VMSPLICE => sys_vmsplice,
        SYS_FADVISE64 => sys_fadvise64,
        SYS_CHMOD => sys_chmod,
        SYS_FCHMOD => sys_fchmod,
        SYS_FCHMODAT => sys_fchmodat,
        SYS_CHOWN => sys_chown,
        SYS_FCHOWN => sys_fchown,
        SYS_LCHOWN => sys_lchown,
        SYS_FCHOWNAT => sys_fchownat,
        SYS_UMASK => sys_umask,
        SYS_GETRLIMIT => sys_getrlimit,
        SYS_SETRLIMIT => sys_setrlimit,
        SYS_STATFS => sys_statfs,
        SYS_FSTATFS => sys_fstatfs,
        SYS_OPENAT => sys_openat,
        SYS_GETDENTS64 => sys_getdents64,
        SYS_READLINK => sys_readlink,
        SYS_SYMLINKAT => sys_symlinkat,
        SYS_READLINKAT => sys_readlinkat,
        SYS_EPOLL_CREATE => sys_epoll_create,
        SYS_EPOLL_CREATE1 => sys_epoll_create1,
        SYS_EPOLL_CTL => sys_epoll_ctl,
        SYS_EPOLL_WAIT => sys_epoll_wait,
        SYS_EPOLL_PWAIT => sys_epoll_pwait,
        SYS_EPOLL_PWAIT2 => sys_epoll_pwait2,
        SYS_EVENTFD => sys_eventfd,
        SYS_EVENTFD2 => sys_eventfd2,
        SYS_INOTIFY_INIT1 => sys_inotify_init1,
        SYS_SOCKET => sys_socket,
        SYS_CONNECT => sys_connect,
        SYS_ACCEPT => sys_accept,
        SYS_SENDTO => sys_sendto,
        SYS_RECVFROM => sys_recvfrom,
        SYS_SENDMSG => sys_sendmsg,
        SYS_RECVMSG => sys_recvmsg,
        SYS_SHUTDOWN => sys_shutdown,
        SYS_BIND => sys_bind,
        SYS_LISTEN => sys_listen,
        SYS_GETSOCKNAME => sys_getsockname,
        SYS_GETPEERNAME => sys_getpeername,
        SYS_SOCKETPAIR => sys_socketpair,
        SYS_SETSOCKOPT => sys_setsockopt,
        SYS_GETSOCKOPT => sys_getsockopt,
        SYS_PREADV => sys_preadv,
        SYS_PWRITEV => sys_pwritev,
        SYS_PREADV2 => sys_preadv2,
        SYS_PWRITEV2 => sys_pwritev2,
        // ── Mémoire ────────────────────────────────────────────────────────
        SYS_MMAP => sys_mmap,
        SYS_MREMAP => sys_mremap,
        SYS_MUNMAP => sys_munmap,
        SYS_MPROTECT => sys_mprotect,
        SYS_BRK => sys_brk,
        SYS_SHMGET | SYS_SHMAT | SYS_SHMCTL | SYS_SHMDT => sys_enosys,
        // ── Processus ──────────────────────────────────────────────────────
        SYS_FORK => sys_fork,
        SYS_VFORK => sys_vfork,
        SYS_CLONE => sys_clone,
        SYS_EXECVE => sys_execve,
        SYS_EXIT => sys_exit,
        SYS_EXIT_GROUP => sys_exit_group,
        SYS_REBOOT => sys_reboot,
        SYS_WAIT4 => sys_wait4,
        SYS_WAITID => sys_waitid,
        SYS_GETPID => crate::syscall::handlers::misc::sys_getpid,
        SYS_GETPPID => crate::syscall::handlers::misc::sys_getppid,
        SYS_GETTID => crate::syscall::handlers::misc::sys_gettid,
        SYS_GETUID => crate::syscall::handlers::misc::sys_getuid,
        SYS_GETGID => crate::syscall::handlers::misc::sys_getgid,
        SYS_GETEUID => crate::syscall::handlers::misc::sys_geteuid,
        SYS_GETEGID => crate::syscall::handlers::misc::sys_getegid,
        SYS_GETGROUPS => crate::syscall::compat::posix::sys_getgroups,
        SYS_SETGROUPS => crate::syscall::compat::posix::sys_setgroups,
        SYS_CAPGET => crate::syscall::compat::posix::sys_capget,
        SYS_CAPSET => crate::syscall::compat::posix::sys_capset,
        SYS_UNAME => crate::syscall::handlers::misc::sys_uname,
        SYS_ARCH_PRCTL => crate::syscall::handlers::misc::sys_arch_prctl,
        SYS_SET_TID_ADDRESS => crate::syscall::handlers::misc::sys_set_tid_address,
        SYS_PRCTL => crate::syscall::handlers::misc::sys_prctl,
        SYS_SYSINFO => crate::syscall::handlers::misc::sys_sysinfo,
        SYS_GETCPU => crate::syscall::handlers::misc::sys_getcpu,
        // ── Signaux ────────────────────────────────────────────────────────
        SYS_KILL => sys_kill,
        SYS_TGKILL => sys_tgkill,
        SYS_RT_SIGACTION => sys_rt_sigaction,
        SYS_RT_SIGPROCMASK => sys_rt_sigprocmask,
        SYS_SIGALTSTACK => sys_sigaltstack,
        // ── Scheduler ──────────────────────────────────────────────────────
        SYS_SCHED_YIELD => crate::syscall::handlers::misc::sys_sched_yield,
        SYS_CLOCK_GETTIME => sys_clock_gettime,
        SYS_GETTIMEOFDAY => sys_gettimeofday,
        SYS_CLOCK_NANOSLEEP => sys_clock_nanosleep,
        SYS_NANOSLEEP => sys_nanosleep,
        SYS_TIMES => crate::syscall::compat::posix::sys_times,
        SYS_FUTEX => sys_futex,
        SYS_GETRANDOM => sys_getrandom,
        // ── IPC Exo-OS ─────────────────────────────────────────────────────
        SYS_EXO_IPC_SEND => sys_exo_ipc_send,
        SYS_EXO_IPC_RECV => sys_exo_ipc_recv,
        SYS_EXO_IPC_RECV_NB => sys_exo_ipc_recv_nb,
        SYS_EXO_IPC_CALL => sys_exo_ipc_call,
        SYS_EXO_IPC_CREATE => sys_exo_ipc_create,
        SYS_EXO_IPC_DESTROY => sys_exo_ipc_destroy,
        SYS_EXO_IPC_LOOKUP => sys_exo_ipc_lookup,
        SYS_EXO_MEM_COPY_FROM_PID => sys_exo_mem_copy_from_pid,
        SYS_EXO_MEM_COPY_TO_PID => sys_exo_mem_copy_to_pid,
        SYS_EXO_MEM_MAP_PID => sys_exo_mem_map_pid,
        SYS_EXO_MEM_MUNMAP_PID => sys_exo_mem_munmap_pid,
        SYS_EXO_MEM_MPROTECT_PID => sys_exo_mem_mprotect_pid,
        SYS_EXO_CAP_CREATE => sys_exo_cap_create,
        SYS_EXO_CAP_REVOKE => sys_exo_cap_revoke,
        SYS_EXO_CAP_CHECK => sys_exo_cap_check,
        SYS_EXO_PLEDGE => sys_exo_pledge,
        SYS_EXO_SHIELD_DRAIN => sys_exo_shield_drain,
        SYS_EXO_PERF_READ => sys_exo_perf_read,
        SYS_EXO_PERF_ENABLE => sys_exo_perf_enable,
        SYS_EXO_LOG => sys_exo_log,
        SYS_EXO_PROCESS_LIST => sys_exo_process_list,
        SYS_EXO_PHOENIX_STATE_SET => sys_exo_phoenix_state_set,
        SYS_EXO_PHOENIX_STATE_GET => sys_exo_phoenix_state_get,
        // ── ExoFS (500–518) ────────────────────────────────────────────────
        SYS_EXOFS_PATH_RESOLVE => sys_exofs_path_resolve,
        SYS_EXOFS_OBJECT_OPEN => sys_exofs_object_open,
        SYS_EXOFS_OBJECT_READ => sys_exofs_object_read,
        SYS_EXOFS_OBJECT_WRITE => sys_exofs_object_write,
        SYS_EXOFS_OBJECT_CREATE => sys_exofs_object_create,
        SYS_EXOFS_OBJECT_DELETE => sys_exofs_object_delete,
        SYS_EXOFS_OBJECT_STAT => sys_exofs_object_stat,
        SYS_EXOFS_OBJECT_SET_META => sys_exofs_object_set_meta_abi,
        SYS_EXOFS_GET_CONTENT_HASH => sys_exofs_get_content_hash,
        SYS_EXOFS_SNAPSHOT_CREATE => sys_exofs_snapshot_create,
        SYS_EXOFS_SNAPSHOT_LIST => sys_exofs_snapshot_list,
        SYS_EXOFS_SNAPSHOT_MOUNT => sys_exofs_snapshot_mount,
        SYS_EXOFS_RELATION_CREATE => sys_exofs_relation_create,
        SYS_EXOFS_RELATION_QUERY => sys_exofs_relation_query,
        SYS_EXOFS_GC_TRIGGER => sys_exofs_gc_trigger,
        SYS_EXOFS_QUOTA_QUERY => sys_exofs_quota_query,
        SYS_EXOFS_EXPORT_OBJECT => sys_exofs_export_object,
        SYS_EXOFS_IMPORT_OBJECT => sys_exofs_import_object,
        SYS_EXOFS_EPOCH_COMMIT => sys_exofs_epoch_commit,
        // ── ExoFS extensions (519–520) — FIX BUG-01 + BUG-02 ───────────────
        SYS_EXOFS_OPEN_BY_PATH => sys_exofs_open_by_path,
        SYS_EXOFS_READDIR => sys_exofs_readdir,
        SYS_FRAMEBUFFER_INFO => sys_framebuffer_info,
        // ── GI-03 Drivers (530–549) ──────────────────────────────────────────
        SYS_IRQ_REGISTER => sys_irq_register,
        SYS_IRQ_ACK => sys_irq_ack,
        SYS_IOPORT_READ => sys_ioport_read,
        SYS_IOPORT_WRITE => sys_ioport_write,
        SYS_MMIO_MAP => sys_mmio_map,
        SYS_MMIO_UNMAP => sys_mmio_unmap,
        SYS_DMA_ALLOC => sys_dma_alloc,
        SYS_DMA_FREE => sys_dma_free,
        SYS_DMA_SYNC => sys_dma_sync,
        SYS_PCI_CFG_READ => sys_pci_cfg_read,
        SYS_PCI_CFG_WRITE => sys_pci_cfg_write,
        SYS_PCI_BUS_MASTER => sys_pci_bus_master,
        SYS_PCI_CLAIM => sys_pci_claim,
        SYS_DMA_MAP => sys_dma_map,
        SYS_DMA_UNMAP => sys_dma_unmap,
        SYS_MSI_ALLOC => sys_msi_alloc,
        SYS_MSI_CONFIG => sys_msi_config,
        SYS_MSI_FREE => sys_msi_free,
        SYS_PCI_SET_TOPOLOGY => sys_pci_set_topology,
        SYS_PCI_FIND_DEVICE => sys_pci_find_device,
        // ── Catch-all ──────────────────────────────────────────────────────
        _ => sys_enosys,
    }
}
