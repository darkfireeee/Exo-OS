//! readdir.rs — SYS_EXOFS_READDIR (520)
//!
//! **FIX BUG-02** — getdents64 ExoFS.
//!
//! ## Problème (BUG-02)
//! SYS_EXOFS_READDIR était absent de la liste 500-518.
//! Sans ce syscall, `ls`, `find`, `opendir()` sont impossibles.
//!
//! ## Solution
//! SYS_EXOFS_READDIR (520) liste le contenu d'un répertoire ExoFS via un fd.
//! Retourne des entrées au format `linux_dirent64` pour compatibilité POSIX.
//!
//! RÈGLE 9  : copy_from_user() pour TOUT pointeur userspace entrant.
//! RÈGLE 10 : buffer de sortie alloué sur le tas.
//! SYS-01   : Valider buf_ptr avant write_user_buf.
//! RECUR-01 : while, pas de for.

use super::object_fd::OBJECT_TABLE;
use super::validation::{
    exofs_err_to_errno, validate_fd, validate_user_ptr, verify_cap, write_user_buf, CapabilityType,
    EINVAL, EXOFS_LIST_MAX, EXOFS_NAME_MAX,
};
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::path::path_index::PathIndex;
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// Structures de sortie (linux_dirent64 — compatible POSIX)
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée de répertoire au format linux_dirent64.
///
/// `#[repr(C)]` garantit le layout binaire stable attendu par glibc/musl.
/// Le nom est variable : il suit immédiatement cette structure en mémoire.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct LinuxDirent64 {
    /// Numéro d'inode (ObjectId tronqué sur 64 bits pour compat POSIX).
    pub d_ino: u64,
    /// Offset vers la prochaine entrée (ou 0 pour la dernière).
    pub d_off: i64,
    /// Taille de cette entrée (header + nom + padding).
    pub d_reclen: u16,
    /// Type de fichier : DT_REG=8, DT_DIR=4, DT_LNK=10, DT_UNKNOWN=0.
    pub d_type: u8,
    // Le nom suit immédiatement (null-terminated, longueur variable).
}

pub const HEADER_SIZE: usize = core::mem::size_of::<LinuxDirent64>();

// Types d_type (compatibles linux)
pub const DT_UNKNOWN: u8 = 0;
pub const DT_FIFO: u8 = 1;
pub const DT_CHR: u8 = 2;
pub const DT_DIR: u8 = 4;
pub const DT_BLK: u8 = 6;
pub const DT_REG: u8 = 8;
pub const DT_LNK: u8 = 10;
pub const DT_SOCK: u8 = 12;

// ─────────────────────────────────────────────────────────────────────────────
// Résultat d'une entrée listée par le backend ExoFS
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée de répertoire retournée par la couche ExoFS core.
#[derive(Clone)]
pub struct ExofsDirEntry {
    /// ObjectId (32 octets) tronqué pour ino.
    pub ino: u64,
    /// Type d'objet (fichier, dossier, lien).
    pub kind: u8,
    /// Nom de l'entrée (UTF-8, sans null terminal ici).
    pub name_vec: Vec<u8>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Logique interne readdir
// ─────────────────────────────────────────────────────────────────────────────

/// Liste les entrées d'un répertoire ExoFS via son fd.
///
/// Charge le blob depuis le cache, le décode comme PathIndex, puis retourne
/// jusqu'à `max_entries` entrées converties en ExofsDirEntry.
fn list_dir_entries(fd: u32, max_entries: usize) -> ExofsResult<Vec<ExofsDirEntry>> {
    // Récupérer le BlobId du répertoire depuis la table de fds
    let blob_id = OBJECT_TABLE.blob_id_of(fd)?;
    // Charger le contenu depuis le blob cache
    let data = BLOB_CACHE.get(&blob_id).ok_or(ExofsError::ObjectNotFound)?;
    // Désérialiser en PathIndex
    let idx = PathIndex::from_bytes(&data).map_err(|_| ExofsError::CorruptedStructure)?;
    let all_entries = idx.entries();
    let count = all_entries.len().min(max_entries);
    let mut result = Vec::new();
    result
        .try_reserve(count)
        .map_err(|_| ExofsError::NoMemory)?;
    // RECUR-01 : while
    let mut i = 0usize;
    while i < count {
        let e = &all_entries[i];
        let name_bytes = e.name_bytes();
        let mut name_vec = Vec::new();
        name_vec
            .try_reserve(name_bytes.len())
            .map_err(|_| ExofsError::NoMemory)?;
        let mut j = 0usize;
        while j < name_bytes.len() {
            name_vec.push(name_bytes[j]);
            j = j.saturating_add(1);
        }
        // Tronquer l'ObjectId (32 bytes) en u64 pour d_ino.
        let mut ino_bytes = [0u8; 8];
        ino_bytes.copy_from_slice(&e.oid.0[..8]);
        let ino = u64::from_le_bytes(ino_bytes);
        result.push(ExofsDirEntry {
            ino,
            kind: e.kind,
            name_vec,
        });
        i = i.saturating_add(1);
    }
    Ok(result)
}

// ─────────────────────────────────────────────────────────────────────────────
// Serialisation vers linux_dirent64
// ─────────────────────────────────────────────────────────────────────────────

/// Sérialise les entrées ExoFS en format linux_dirent64 dans un Vec<u8>.
///
/// RECUR-01 : while, pas de for.
fn serialize_dirents(entries: &[ExofsDirEntry], buf_len: usize) -> ExofsResult<Vec<u8>> {
    let mut out = Vec::new();
    out.try_reserve(buf_len.min(EXOFS_LIST_MAX))
        .map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < entries.len() {
        let entry = &entries[i];
        let name_len = entry.name_vec.len().min(EXOFS_NAME_MAX);
        // reclen = header + nom + null byte, aligné sur 8 bytes
        let raw_size = HEADER_SIZE.saturating_add(name_len).saturating_add(1);
        let reclen = (raw_size.saturating_add(7)) & !7usize;
        if out.len().saturating_add(reclen) > buf_len {
            break;
        }
        // Écrire le header
        let hdr = LinuxDirent64 {
            d_ino: entry.ino,
            d_off: (i.saturating_add(1)) as i64,
            d_reclen: reclen as u16,
            d_type: entry.kind,
        };
        // SAFETY: LinuxDirent64 est #[repr(C)] avec layout stable.
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(&hdr as *const LinuxDirent64 as *const u8, HEADER_SIZE)
        };
        out.try_reserve(reclen).map_err(|_| ExofsError::NoMemory)?;
        let mut j = 0usize;
        while j < HEADER_SIZE {
            out.push(hdr_bytes[j]);
            j = j.saturating_add(1);
        }
        // Écrire le nom
        let mut k = 0usize;
        while k < name_len {
            out.push(entry.name_vec[k]);
            k = k.saturating_add(1);
        }
        // Null byte
        out.push(0u8);
        // Padding pour alignement
        let padding = reclen
            .saturating_sub(HEADER_SIZE)
            .saturating_sub(name_len)
            .saturating_sub(1);
        let mut p = 0usize;
        while p < padding {
            out.push(0u8);
            p = p.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler syscall (SYS_EXOFS_READDIR = 520)
// ─────────────────────────────────────────────────────────────────────────────

/// Handler de SYS_EXOFS_READDIR (520).
///
/// Signature : `(fd: u64, buf_ptr: u64, buf_len: u64) → octets remplis ou errno`
///
/// Utilisé par `sys_getdents64()` dans handlers/fs_posix.rs.
///
/// SYS-01 : buf_ptr validé avant copy_to_user.
/// SYS-05 : buf_len=0 → EINVAL, buf_len>EXOFS_LIST_MAX → tronqué.
pub fn sys_exofs_readdir(
    fd: u64,
    buf_ptr: u64,
    buf_len: u64,
    _a4: u64,
    _a5: u64,
    cap_rights: u64,
) -> i64 {
    // SYS-05 : valider longueur AVANT toute opération
    if buf_len == 0 {
        return EINVAL;
    }
    // SYS-01 : valider le pointeur de destination
    if let Err(e) = validate_user_ptr(buf_ptr, buf_len as usize) {
        return e;
    }
    // Valider le fd
    let fd32 = match validate_fd(fd) {
        Ok(f) => f,
        Err(e) => return e,
    };

    if let Err(e) = verify_cap(cap_rights, CapabilityType::ExoFsReaddir) {
        return e;
    }

    // Calculer le nombre max d'entrées (borné par EXOFS_LIST_MAX)
    let buf_limit = (buf_len as usize).min(EXOFS_LIST_MAX);
    let max_entries = buf_limit / (HEADER_SIZE.saturating_add(2)); // estimation minimale
    let max_entries = if max_entries == 0 { 1 } else { max_entries };
    // Lister les entrées du répertoire
    let entries = match list_dir_entries(fd32, max_entries) {
        Ok(e) => e,
        Err(e) => return exofs_err_to_errno(e),
    };
    if entries.is_empty() {
        return 0;
    }
    // Sérialiser en format linux_dirent64
    let serialized = match serialize_dirents(&entries, buf_limit) {
        Ok(s) => s,
        Err(e) => return exofs_err_to_errno(e),
    };
    let bytes_written = serialized.len();
    if bytes_written == 0 {
        return 0;
    }
    // SYS-01 : copy_to_user (write_user_buf valide buf_ptr en interne)
    match write_user_buf(buf_ptr, &serialized) {
        Ok(_) => bytes_written as i64,
        Err(e) => e,
    }
}
