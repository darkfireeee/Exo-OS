//! # syscall/compat/mod.rs — ABI compatibility layer
//!
//! This submodule groups two complementary layers:
//!
//! | Module     | Role                                                          |
//! |------------|---------------------------------------------------------------|
//! | `linux`    | Translates removed / renamed Linux numbers                    |
//! | `posix`    | POSIX.1-2017 handlers + constants + arg validation            |
//!
//! ## Call Pipeline (dispatch.rs)
//!
//! ```text
//! dispatch()
//!   ├─ try_fast_path()          fast_path.rs
//!   ├─ translate_linux_nr()     compat::linux
//!   └─ get_handler()            table.rs  ←  compat::posix::get_posix_handler()
//! ```
//!
//! `dispatch.rs` calls `translate_linux_nr` to remap/block obsolete numbers,
//! then `get_handler` which delegates to `get_posix_handler` as a
//! secondary resort for POSIX numbers not included in the main table.

pub mod linux;
pub mod posix;

pub use linux::{linux_compat_stats, translate_linux_nr, LinuxCompatStats};

pub use posix::{
    get_posix_handler, mmap_flags, open_flags, posix_call_count, prot_flags, seek_whence, signals,
    validate_lseek_whence, validate_mmap_flags, validate_open_flags, validate_prot,
};

/// Global statistics for the compat layer (aggregated linux + posix).
#[derive(Debug, Clone, Copy)]
pub struct CompatStats {
    /// Calls translated by linux.rs (old number → new)
    pub linux_translated: u64,
    /// Calls blocked by linux.rs (removed numbers → ENOSYS)
    pub linux_blocked: u64,
    /// Passthrough calls (unmodified)
    pub linux_passthrough: u64,
    /// Calls processed by posix.rs
    pub posix_calls: u64,
}

/// Returns aggregated compatibility statistics.
pub fn compat_stats() -> CompatStats {
    let ls = linux_compat_stats();
    CompatStats {
        linux_translated: ls.translated,
        linux_blocked: ls.blocked,
        linux_passthrough: ls.passthrough,
        posix_calls: posix_call_count(),
    }
}
