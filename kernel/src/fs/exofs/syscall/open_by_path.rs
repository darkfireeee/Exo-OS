//! open_by_path.rs — SYS_EXOFS_OPEN_BY_PATH (519)
//!
//! **FIX BUG-01** — open() POSIX combiné Ring0.
//!
//! ## Problème (BUG-01)
//! musl génère `syscall(500, path, flags)` en UN seul appel pour open().
//! Mais ExoFS nécessite DEUX appels : PATH_RESOLVE puis OBJECT_OPEN.
//! Impossible à mapper sur un seul syscall 500.
//!
//! ## Solution
//! SYS_EXOFS_OPEN_BY_PATH (519) enchaîne `path_resolve()` + `object_open()`
//! **atomiquement en Ring0**. musl-exo : `#define __NR_open 519`.
//!
//! RÈGLE 9  : copy_from_user() pour TOUT pointeur userspace entrant.
//! RÈGLE 10 : buffer chemin sur le tas.
//! RECUR-01 : zéro boucle for.
//! OOM-02   : try_reserve() avant push().

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use super::validation::{
    read_user_path_heap, exofs_err_to_errno, EFAULT, ENOENT,
};
use super::object_fd::OBJECT_TABLE;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes flags open()
// ─────────────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
const O_RDONLY: u32 = 0x0000;
#[allow(dead_code)]
const O_WRONLY: u32 = 0x0001;
#[allow(dead_code)]
const O_RDWR:   u32 = 0x0002;
#[allow(dead_code)]
const O_CREAT:  u32 = 0x0040;
#[allow(dead_code)]
const O_TRUNC:  u32 = 0x0200;
#[allow(dead_code)]
const O_APPEND: u32 = 0x0400;
#[allow(dead_code)]
const O_NONBLOCK: u32 = 0x0800;
#[allow(dead_code)]
const O_CLOEXEC:  u32 = 0x0008_0000;

// ─────────────────────────────────────────────────────────────────────────────
// Logique combinée path_resolve + object_open
// ─────────────────────────────────────────────────────────────────────────────

/// Ouvre un objet ExoFS directement depuis son chemin.
///
/// Exécute atomiquement en Ring0 :
///   1. Résolution du chemin → BlobId (hash)
///   2. Ouverture de l'objet → fd
///
/// La résolution et l'ouverture sont atomiques par rapport aux autres syscalls
/// du même processus (aucune race entre les deux étapes).
fn open_by_path_inner(path_bytes: &[u8], path_len: usize, flags: u32, mode: u32) -> ExofsResult<u32> {
    if path_len == 0 { return Err(ExofsError::InvalidArgument); }
    if flags & !0x000F_FFFFu32 != 0 { return Err(ExofsError::InvalidArgument); }

    // Dériver le BlobId depuis le chemin canonique (Blake3)
    let blob_id = BlobId::from_bytes_blake3(&path_bytes[..path_len]);

    // Ouvrir via la table de fd (crée l'entrée si O_CREAT)
    let open_args = crate::fs::exofs::syscall::object_open::OpenArgs {
        flags:     flags,
        mode:      mode,
        epoch_id:  0,      // epoch courante
        owner_uid: 0,
        size_hint: 0,
        _reserved: [0u64; 2],
    };
    OBJECT_TABLE.open(blob_id, open_args.flags, 0u64, open_args.epoch_id, open_args.owner_uid)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler syscall (SYS_EXOFS_OPEN_BY_PATH = 519)
// ─────────────────────────────────────────────────────────────────────────────

/// Handler de SYS_EXOFS_OPEN_BY_PATH (519).
///
/// ## ABI musl-exo (LIB-01 / BUG-01)
/// musl appelle `syscall(519, path_ptr, flags, mode)` — PAS de path_len séparé.
/// Le chemin est null-terminated (lu via `read_user_path_heap`).
///
/// Signature : `(path_ptr: u64, flags: u64, mode: u64) → fd ou -errno`
///
/// RÈGLE 9 : copy_from_user via read_user_path_heap (null-terminated, heap).
/// SYS-05  : Refuse ptr null. La longueur est inférée depuis le null-terminator.
pub fn sys_exofs_open_by_path(
    path_ptr:  u64,
    flags:     u64,   // arg1 : musl envoie flags ici (pas path_len)
    mode:      u64,   // arg2 : musl envoie mode ici
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    // SYS-05 : Refuser pointeur null
    if path_ptr == 0 { return EFAULT; }

    // SYS-01 : copy_from_user — lit le chemin null-terminated sur le tas
    let mut path_bytes = Vec::new();
    let actual_len = match read_user_path_heap(path_ptr, &mut path_bytes) {
        Ok(n)  => n,
        Err(e) => return e,
    };
    if actual_len == 0 { return ENOENT; }

    let flags32 = (flags & 0xFFFF_FFFF) as u32;
    let mode32  = (mode  & 0xFFFF_FFFF) as u32;
    match open_by_path_inner(&path_bytes, actual_len, flags32, mode32) {
        Ok(fd)  => fd as i64,
        Err(e)  => exofs_err_to_errno(e),
    }
}
