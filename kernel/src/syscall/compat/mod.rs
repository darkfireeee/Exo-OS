//! # syscall/compat/mod.rs — Couche de compatibilité ABI
//!
//! Ce sous-module regroupe deux couches complémentaires :
//!
//! | Module     | Rôle                                                          |
//! |------------|---------------------------------------------------------------|
//! | `linux`    | Traduit les numéros Linux supprimés / renommés               |
//! | `posix`    | Handlers POSIX.1-2017 + constantes + validation des args      |
//!
//! ## Pipeline d'appel (dispatch.rs)
//!
//! ```text
//! dispatch()
//!   ├─ try_fast_path()          fast_path.rs
//!   ├─ translate_linux_nr()     compat::linux
//!   └─ get_handler()            table.rs  ←  compat::posix::get_posix_handler()
//! ```
//!
//! `dispatch.rs` appelle `translate_linux_nr` pour remap/bloquer les numéros
//! obsolètes, puis `get_handler` qui délègue vers `get_posix_handler` en
//! second recours pour les numéros POSIX non inclus dans la table principale.

pub mod linux;
pub mod posix;

pub use linux::{
    translate_linux_nr,
    linux_compat_stats,
    LinuxCompatStats,
};

pub use posix::{
    get_posix_handler,
    posix_call_count,
    validate_open_flags,
    validate_prot,
    validate_mmap_flags,
    validate_lseek_whence,
    open_flags,
    seek_whence,
    mmap_flags,
    prot_flags,
    signals,
};

/// Statistiques globales de la couche compat (linux + posix agrégés).
#[derive(Debug, Clone, Copy)]
pub struct CompatStats {
    /// Appels traduits par linux.rs (ancien numéro → nouveau)
    pub linux_translated: u64,
    /// Appels bloqués par linux.rs (numéros supprimés → ENOSYS)
    pub linux_blocked: u64,
    /// Appels passthrough (non modifiés)
    pub linux_passthrough: u64,
    /// Appels traités par posix.rs
    pub posix_calls: u64,
}

/// Retourne les statistiques de compatibilité agrégées.
pub fn compat_stats() -> CompatStats {
    let ls = linux_compat_stats();
    CompatStats {
        linux_translated:  ls.translated,
        linux_blocked:     ls.blocked,
        linux_passthrough: ls.passthrough,
        posix_calls:       posix_call_count(),
    }
}
