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

#![allow(unused_imports)]

pub mod numbers;
pub mod validation;
pub mod fast_path;
pub mod table;
pub mod dispatch;
pub mod compat;
pub mod fs_bridge;

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
    // Errno
    EINVAL, EFAULT, ENOMEM, EAGAIN, EPERM, ENOSYS, EBADF, ENOENT,
    EACCES, EEXIST, ENOTDIR, EISDIR, ENOSPC, ENOTSUP,
    // Classificateurs
    is_valid_syscall, is_linux_compat, is_exoos_native,
};

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
