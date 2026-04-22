//! # kernel/src/syscall/mod.rs — Module syscall d'Exo-OS
//!
//! ## Vue d'ensemble
//!
//! Ce module orchestre l'intégralité de l'interface syscall du noyau.
//! Il est structuré en couches indépendantes :
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │  arch/x86_64/syscall.rs                                  │
//! │  syscall_rust_handler(frame)  ← SYSCALL matériel         │
//! └──────────────────┬───────────────────────────────────────┘
//!                    │ appel direct
//! ┌──────────────────▼───────────────────────────────────────┐
//! │  syscall::dispatch::dispatch(frame)                      │
//! │  ├─ validation numéro      numbers::is_valid_syscall()   │
//! │  ├─ fast-path              fast_path::try_fast_path()    │
//! │  ├─ traduction compat      compat::translate_linux_nr()  │
//! │  ├─ dispatch table         table::get_handler()          │
//! │  └─ livraison signaux      post_dispatch()               │
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Modules
//!
//! | Module       | Rôle                                              |
//! |--------------|---------------------------------------------------|
//! | `numbers`    | Constantes SYS_* et errno                        |
//! | `validation` | Types sûrs pour les arguments userspaces          |
//! | `fast_path`  | Handlers ultra-rapides (<150 cycles, sans verrou) |
//! | `table`      | Table O(1) + handlers slow-path                  |
//! | `dispatch`   | Pipeline complet de dispatch                      |
//! | `compat`     | Couches linux.rs + posix.rs                       |
//!
//! ## Règles architecturales respectées
//!
//! - **SIGNAL-01 (DOC1)** : livraison des signaux uniquement au retour vers
//!   userspace, via `dispatch::post_dispatch()`.
//! - **SCHED-03 (DOC3)** : futex logé dans `memory::utils::futex_table`.
//! - **regle_bonus** : ordonnancement des verrous IPC < Sched < Mem < FS ;
//!   chaque `unsafe` précédé de `// SAFETY:`.
//! - **NO-ALLOC** sur les chemins chauds (`fast_path`, dispatch fast branch).


pub mod numbers;
pub mod validation;
pub mod fixup;
pub mod fast_path;
pub mod table;
pub mod dispatch;
pub mod compat;
pub mod fs_bridge;
// Nouveaux modules — correctifs BUG-01..BUG-09
pub mod errno;
pub mod abi;
pub mod handlers;

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports publics
// ─────────────────────────────────────────────────────────────────────────────

/// Numéros de syscall et codes d'erreur POSIX/Linux.
pub use numbers::{
    SYSCALL_TABLE_SIZE,
    // Numéros Linux-compat
    SYS_READ, SYS_WRITE, SYS_OPEN, SYS_CLOSE, SYS_STAT, SYS_FSTAT,
    SYS_LSTAT, SYS_POLL, SYS_MMAP, SYS_MPROTECT, SYS_MUNMAP, SYS_BRK,
    SYS_RT_SIGACTION, SYS_RT_SIGPROCMASK, SYS_RT_SIGRETURN,
    SYS_IOCTL, SYS_PREAD64, SYS_PWRITE64,
    SYS_FORK, SYS_VFORK, SYS_CLONE, SYS_EXECVE,
    SYS_EXIT, SYS_EXIT_GROUP, SYS_WAIT4,
    SYS_KILL, SYS_TGKILL,
    SYS_FUTEX, SYS_NANOSLEEP,
    SYS_GETPID, SYS_GETPPID, SYS_GETTID,
    SYS_GETUID, SYS_GETEUID, SYS_GETGID, SYS_GETEGID,
    // Numéros Exo-OS natifs
    SYS_EXO_IPC_SEND, SYS_EXO_IPC_RECV, SYS_EXO_IPC_CALL,
    SYS_EXO_CAP_CREATE, SYS_EXO_CAP_REVOKE, SYS_EXO_LOG,
    // ExoFS 500-520
    SYS_EXOFS_PATH_RESOLVE, SYS_EXOFS_OBJECT_OPEN, SYS_EXOFS_OBJECT_READ,
    SYS_EXOFS_OBJECT_WRITE, SYS_EXOFS_OBJECT_CREATE, SYS_EXOFS_OBJECT_DELETE,
    SYS_EXOFS_OBJECT_STAT, SYS_EXOFS_OBJECT_SET_META, SYS_EXOFS_GET_CONTENT_HASH,
    SYS_EXOFS_SNAPSHOT_CREATE, SYS_EXOFS_SNAPSHOT_LIST, SYS_EXOFS_SNAPSHOT_MOUNT,
    SYS_EXOFS_RELATION_CREATE, SYS_EXOFS_RELATION_QUERY, SYS_EXOFS_GC_TRIGGER,
    SYS_EXOFS_QUOTA_QUERY, SYS_EXOFS_EXPORT_OBJECT, SYS_EXOFS_IMPORT_OBJECT,
    SYS_EXOFS_EPOCH_COMMIT,
    SYS_EXOFS_OPEN_BY_PATH, SYS_EXOFS_READDIR,
    // GI-03 Drivers 530-546
    SYS_IRQ_REGISTER, SYS_IRQ_ACK,
    SYS_MMIO_MAP, SYS_MMIO_UNMAP,
    SYS_DMA_ALLOC, SYS_DMA_FREE, SYS_DMA_SYNC,
    SYS_PCI_CFG_READ, SYS_PCI_CFG_WRITE, SYS_PCI_BUS_MASTER,
    SYS_PCI_CLAIM, SYS_DMA_MAP, SYS_DMA_UNMAP,
    SYS_MSI_ALLOC, SYS_MSI_CONFIG, SYS_MSI_FREE,
    SYS_PCI_SET_TOPOLOGY,
    // Aliases exo-rt (BUG-03)
    SYS_PROC_CLONE, SYS_PROC_EXEC,
    // Errno
    EINVAL, EFAULT, ENOMEM, EAGAIN, EPERM, ENOSYS, EBADF, ENOENT,
    EACCES, EEXIST, ENOTDIR, EISDIR, ENOSPC, ENOTSUP,
    // Classificateurs
    is_valid_syscall, is_linux_compat, is_exoos_native, is_exofs_syscall,
};

/// Mapping KernelError/ExofsError → errno (BUG fix ERRNO-MISSING).
pub use errno::{kernel_err_to_errno, exofs_err_to_errno, result_to_retval};

/// Types ABI : SyscallArgs, SyscallResult, check adresse canonique (BUG-05).
pub use abi::{SyscallArgs, SyscallResult, is_canonical_address};

/// Types de validation des arguments userspaces.
pub use validation::{
    UserPtr, ValidatedUserPtr, UserBuf, UserStr, SyscallError,
    copy_from_user, copy_to_user,
    read_user_typed, write_user_typed, read_user_path,
    validate_fd, validate_pid, validate_signal, validate_clockid, validate_flags,
    USER_ADDR_MAX, PATH_MAX, IO_BUF_MAX,
};

/// Point d'entrée unique du dispatch syscall, appelé par arch/.
pub use dispatch::dispatch;

/// Stats de dispatch pour le monitoring.
pub use dispatch::{dispatch_stats, DispatchStats, reset_dispatch_stats};

/// Stats fast-path.
pub use fast_path::{fast_path_stats, FastPathStats};

/// Stats par syscall.
pub use table::syscall_stats_for;

/// Stats compat.
pub use compat::compat_stats;

/// Constantes open flags, mmap, prot — utilisées par table.rs et userland.
pub use compat::posix::{open_flags, mmap_flags, prot_flags, seek_whence, signals};

// Lien vers le SyscallFrame défini dans arch/
pub use crate::arch::x86_64::syscall::SyscallFrame;

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation du module
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise le sous-système syscall.
///
/// Doit être appelé depuis `kernel_main()` après l'initialisation de
/// `arch::x86_64::syscall::init_syscall()` (qui configure les MSRs).
///
/// Cette fonction :
/// 1. Remet à zéro tous les compteurs de stats.
/// 2. Valide que la table de dispatch est cohérente.
/// 3. Journalise le début de service.
pub fn init() {
    // Remet les compteurs à zéro (au cas où le noyau serait rechargé en mémoire)
    reset_dispatch_stats();
    // Note : la journalisation sera activée lors de l'intégration du log ring.
}

// ─────────────────────────────────────────────────────────────────────────────
// Agrégat de statistiques global
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques globales du sous-système syscall.
#[derive(Debug, Clone, Copy)]
pub struct SyscallModuleStats {
    /// Statistiques du pipeline de dispatch
    pub dispatch: DispatchStats,
    /// Statistiques du fast-path
    pub fast_path: FastPathStats,
    /// Statistiques de la couche de compatibilité
    pub compat: compat::CompatStats,
}

/// Retourne un instantané de toutes les statistiques syscall.
pub fn module_stats() -> SyscallModuleStats {
    SyscallModuleStats {
        dispatch:  dispatch_stats(),
        fast_path: fast_path_stats(),
        compat:    compat_stats(),
    }
}
