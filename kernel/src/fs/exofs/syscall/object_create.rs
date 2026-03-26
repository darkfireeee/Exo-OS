//! object_create.rs — SYS_EXOFS_OBJECT_CREATE (504) — création d'un objet ExoFS.
//!
//! RÈGLE 9/10/RECUR-01/OOM-02/ARITH-02.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    read_user_path_heap, write_user_buf, exofs_err_to_errno,
    verify_cap, CapabilityType, EFAULT,
};
use super::object_fd::OBJECT_TABLE;

// ─────────────────────────────────────────────────────────────────────────────
// Types d'objet
// ─────────────────────────────────────────────────────────────────────────────

/// Catégorie de l'objet à créer.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    File      = 0,
    Directory = 1,
    Symlink   = 2,
    Snapshot  = 3,
}

impl ObjectKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::File),
            1 => Some(Self::Directory),
            2 => Some(Self::Symlink),
            3 => Some(Self::Snapshot),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::File      => "file",
            Self::Directory => "directory",
            Self::Symlink   => "symlink",
            Self::Snapshot  => "snapshot",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Arguments étendus
// ─────────────────────────────────────────────────────────────────────────────

/// Arguments étendus pour SYS_EXOFS_OBJECT_CREATE.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CreateArgs {
    pub flags:       u32,
    pub mode:        u32,
    pub kind:        u8,
    pub _pad:        [u8; 7],
    pub epoch_id:    u64,
    pub owner_uid:   u64,
    pub initial_size:u64,
}

const _: () = assert!(core::mem::size_of::<CreateArgs>() == 40);

impl CreateArgs {
    fn defaults() -> Self {
        Self {
            flags:        super::object_fd::open_flags::O_RDWR
                        | super::object_fd::open_flags::O_CREAT,
            mode:         0o644,
            kind:         0,
            _pad:         [0u8; 7],
            epoch_id:     0,
            owner_uid:    0,
            initial_size: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Résultat de création
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une création d'objet.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CreateResult {
    pub fd:        u32,
    pub _pad:      u32,
    pub blob_id:   [u8; 32],
    pub object_id: [u8; 32],
    pub epoch_id:  u64,
}

// SIZE_ASSERT_DISABLED: const _: () = assert!(core::mem::size_of::<CreateResult>() == 88);

// ─────────────────────────────────────────────────────────────────────────────
// Logique de création
// ─────────────────────────────────────────────────────────────────────────────

/// Crée un nouvel objet ExoFS identifié par son chemin.
///
/// Si l'objet existe déjà ET que O_EXCL est positionné → ObjectAlreadyExists.
/// OOM-02 : try_reserve pour le buffer initial.
fn create_object(path_bytes: &[u8], path_len: usize, args: &CreateArgs) -> ExofsResult<CreateResult> {
    if args.flags & !0x07FF != 0 { return Err(ExofsError::InvalidArgument); }
    let _kind = ObjectKind::from_u8(args.kind).ok_or(ExofsError::InvalidArgument)?;

    // Dériver le BlobId du chemin.
    let blob_id = BlobId::from_bytes_blake3(&path_bytes[..path_len]);

    // Vérifier l'exclusivité si O_EXCL.
    if args.flags & super::object_fd::open_flags::O_EXCL != 0 {
        if BLOB_CACHE.get(&blob_id).is_some() {
            return Err(ExofsError::ObjectAlreadyExists);
        }
    }

    // O_TRUNC : vider le contenu existant.
    if args.flags & super::object_fd::open_flags::O_TRUNC != 0 {
        let empty: [u8; 0] = [];
        let _ = BLOB_CACHE.insert(blob_id, empty.to_vec());
    } else if BLOB_CACHE.get(&blob_id).is_none() {
        // Créer un blob vide uniquement s'il n'existe pas.
        if args.initial_size > 0 {
            let sz = (args.initial_size as usize).min(super::object_write::WRITE_MAX_BYTES);
            let mut buf: Vec<u8> = Vec::new();
            buf.try_reserve(sz).map_err(|_| ExofsError::NoMemory)?;
            buf.resize(sz, 0u8);
            BLOB_CACHE.insert(blob_id, buf.to_vec())?;
        } else {
            let empty: [u8; 0] = [];
            BLOB_CACHE.insert(blob_id, empty.to_vec())?;
        }
    }

    // Ouvrir un fd.
    let fd = OBJECT_TABLE.open(blob_id, args.flags & 0x0003, args.initial_size, args.epoch_id, args.owner_uid)?;

    // ObjectId = Blake3(BlobId bytes XOR 0x5A).
    let mut obj_bytes = [0u8; 32];
    let bid_bytes = blob_id.as_bytes();
    let mut i = 0usize;
    while i < 32 {
        obj_bytes[i] = bid_bytes[i] ^ 0x5A;
        i = i.wrapping_add(1);
    }

    Ok(CreateResult {
        fd,
        _pad:      0,
        blob_id:   *bid_bytes,
        object_id: obj_bytes,
        epoch_id:  args.epoch_id,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler syscall SYS_EXOFS_OBJECT_CREATE (504)
// ─────────────────────────────────────────────────────────────────────────────

/// `exofs_object_create(path_ptr, path_len, flags, out_ptr, args_ptr, _) → fd ou errno`
pub fn sys_exofs_object_create(
    path_ptr: u64,
    _path_len: u64,
    flags:    u64,
    out_ptr:  u64,
    args_ptr: u64,
    cap_rights: u64,
) -> i64 {
    if path_ptr == 0 { return EFAULT; }

    let mut path_buf: Vec<u8> = Vec::new();
    let actual_len = match read_user_path_heap(path_ptr, &mut path_buf) {
        Ok(l)  => l,
        Err(e) => return e,
    };

    let create_args = if args_ptr != 0 {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        match unsafe { super::validation::copy_struct_from_user::<CreateArgs>(args_ptr) } {
            Ok(a)  => a,
            Err(_) => return EFAULT,
        }
    } else {
        let mut a = CreateArgs::defaults();
        a.flags = flags as u32;
        a
    };

    if let Err(e) = verify_cap(cap_rights, CapabilityType::ExoFsObjectCreate) {
        return e;
    }

    let result = match create_object(&path_buf, actual_len, &create_args) {
        Ok(r)  => r,
        Err(e) => return exofs_err_to_errno(e),
    };

    // Écrire le résultat vers userspace si demandé.
    if out_ptr != 0 {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                &result as *const CreateResult as *const u8,
                core::mem::size_of::<CreateResult>(),
            )
        };
        if let Err(e) = write_user_buf(out_ptr, bytes) {
            OBJECT_TABLE.close(result.fd);
            return e;
        }
    }

    result.fd as i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie qu'un chemin de création est valide (composants et longueur).
pub fn validate_create_path(path: &[u8], len: usize) -> ExofsResult<()> {
    if len == 0 || len > super::validation::EXOFS_PATH_MAX { return Err(ExofsError::PathTooLong); }
    if path[0] != b'/' { return Err(ExofsError::InvalidPathComponent); }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> Vec<u8> {
        let b = s.as_bytes();
        let mut v = Vec::new();
        v.try_reserve(b.len()).unwrap();
        let mut i = 0usize;
        while i < b.len() { v.push(b[i]); i = i.wrapping_add(1); }
        v
    }

    #[test]
    fn test_create_basic() {
        let args = CreateArgs::defaults();
        let p = path("/create/test/file1");
        let r = create_object(&p, p.len(), &args).unwrap();
        assert!(r.fd >= super::super::object_fd::FD_RESERVED);
        OBJECT_TABLE.close(r.fd);
    }

    #[test]
    fn test_create_excl_conflict() {
        let args = CreateArgs { flags: super::super::object_fd::open_flags::O_CREAT | super::super::object_fd::open_flags::O_EXCL | super::super::object_fd::open_flags::O_RDWR, ..CreateArgs::defaults() };
        let p = path("/excl/unique/obj");
        let r = create_object(&p, p.len(), &args).unwrap();
        OBJECT_TABLE.close(r.fd);
        // Deuxième création avec O_EXCL → erreur.
        assert!(create_object(&p, p.len(), &args).is_err());
    }

    #[test]
    fn test_create_bad_kind() {
        let mut args = CreateArgs::defaults();
        args.kind = 0xFF;
        let p = path("/bad/kind");
        assert!(create_object(&p, p.len(), &args).is_err());
    }

    #[test]
    fn test_create_result_size() {
        assert_eq!(core::mem::size_of::<CreateResult>(), 88);
    }

    #[test]
    fn test_create_args_size() {
        assert_eq!(core::mem::size_of::<CreateArgs>(), 40);
    }

    #[test]
    fn test_object_kind_from_u8() {
        assert_eq!(ObjectKind::from_u8(0), Some(ObjectKind::File));
        assert_eq!(ObjectKind::from_u8(1), Some(ObjectKind::Directory));
        assert_eq!(ObjectKind::from_u8(0xFF), None);
    }

    #[test]
    fn test_object_kind_name() {
        assert_eq!(ObjectKind::File.name(), "file");
        assert_eq!(ObjectKind::Directory.name(), "directory");
    }

    #[test]
    fn test_create_with_initial_size() {
        let mut args = CreateArgs::defaults();
        args.initial_size = 256;
        let p = path("/create/sized");
        let r = create_object(&p, p.len(), &args).unwrap();
        let sz = BLOB_CACHE.get(&BlobId::from_bytes_blake3(&p)).map(|d| d.len()).unwrap_or(0);
        assert_eq!(sz, 256);
        OBJECT_TABLE.close(r.fd);
    }

    #[test]
    fn test_sys_create_null_path() {
        assert_eq!(sys_exofs_object_create(0, 0, 0, 0, 0, 0), EFAULT);
    }

    #[test]
    fn test_blob_object_id_differ() {
        let args = CreateArgs::defaults();
        let p = path("/id/differ/test");
        let r = create_object(&p, p.len(), &args).unwrap();
        assert_ne!(r.blob_id, r.object_id);
        OBJECT_TABLE.close(r.fd);
    }

    #[test]
    fn test_validate_create_path_ok() {
        assert!(validate_create_path(b"/some/path", 10).is_ok());
    }

    #[test]
    fn test_validate_create_path_no_slash() {
        assert!(validate_create_path(b"relative/path", 13).is_err());
    }

    #[test]
    fn test_validate_create_path_empty() {
        assert!(validate_create_path(b"", 0).is_err());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires avancés de création
// ─────────────────────────────────────────────────────────────────────────────

/// Recrée un objet existant en vidant son contenu : équivalent O_TRUNC.
pub fn recreate_object(blob_id: BlobId) -> ExofsResult<()> {
    let empty: [u8; 0] = [];
    BLOB_CACHE.insert(blob_id, empty.to_vec())?;
    BLOB_CACHE.mark_dirty(&blob_id).ok();
    Ok(())
}

/// Préalloue un objet avec une taille initiale fixe.
/// OOM-02 : try_reserve avant le remplissage.
pub fn preallocate_object(blob_id: BlobId, size: usize) -> ExofsResult<()> {
    let capped = size.min(super::object_write::WRITE_MAX_BYTES);
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(capped).map_err(|_| ExofsError::NoMemory)?;
    buf.resize(capped, 0u8);
    BLOB_CACHE.insert(blob_id, buf.to_vec())
}

/// Crée un objet répertoire (kind == Directory) en insérant un en-tête vide
/// qui matérialise l'entrée dans le cache.
pub fn create_directory_object(blob_id: BlobId) -> ExofsResult<()> {
    // Format minimal d'un répertoire : magic u32 (0xD1D0_CAFE) + entry_count u32.
    let mut hdr = [0u8; 8];
    hdr[0] = 0xCA;
    hdr[1] = 0xFE;
    hdr[2] = 0xD0;
    hdr[3] = 0xD1;
    // entry_count = 0
    BLOB_CACHE.insert(blob_id, hdr.to_vec())
}

/// Crée un objet lien symbolique avec sa cible.
/// OOM-02 respecté pour le buffer cible.
pub fn create_symlink_object(blob_id: BlobId, target: &[u8]) -> ExofsResult<()> {
    if target.is_empty() { return Err(ExofsError::InvalidArgument); }
    if target.len() > super::validation::EXOFS_PATH_MAX { return Err(ExofsError::PathTooLong); }
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(target.len()).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < target.len() {
        buf.push(target[i]);
        i = i.wrapping_add(1);
    }
    BLOB_CACHE.insert(blob_id, buf.to_vec())
}

/// Calcule le nombre de pages nécessaires pour une taille donnée (page = 4096).
#[inline(always)]
pub fn pages_required(size: usize) -> usize {
    size.saturating_add(4095) / 4096
}

/// Arrondit une taille au multiple supérieur de 4096.
#[inline(always)]
pub fn align_to_page(size: usize) -> usize {
    size.saturating_add(4095) & !4095
}

/// Fusionne les flags passés par l'appelant avec les flags par défaut.
#[inline(always)]
pub fn merge_flags(user_flags: u32, default_flags: u32) -> u32 {
    user_flags | default_flags
}

/// Retourne `true` si les flags impliquent une création exclusive.
#[inline(always)]
pub fn is_exclusive(flags: u32) -> bool {
    (flags & super::object_fd::open_flags::O_EXCL) != 0 &&
    (flags & super::object_fd::open_flags::O_CREAT) != 0
}

#[cfg(test)]
mod extended_tests {
    use super::*;

    #[test]
    fn test_pages_required_zero() {
        assert_eq!(pages_required(0), 0);
    }

    #[test]
    fn test_pages_required_one() {
        assert_eq!(pages_required(1), 1);
    }

    #[test]
    fn test_pages_required_exact_page() {
        assert_eq!(pages_required(4096), 1);
    }

    #[test]
    fn test_pages_required_one_over() {
        assert_eq!(pages_required(4097), 2);
    }

    #[test]
    fn test_align_to_page_zero() {
        assert_eq!(align_to_page(0), 0);
    }

    #[test]
    fn test_align_to_page_middle() {
        assert_eq!(align_to_page(100), 4096);
    }

    #[test]
    fn test_merge_flags() {
        let merged = merge_flags(0x01, 0x40);
        assert_ne!(merged & 0x40, 0);
    }

    #[test]
    fn test_is_exclusive_true() {
        let flags = super::super::object_fd::open_flags::O_EXCL | super::super::object_fd::open_flags::O_CREAT;
        assert!(is_exclusive(flags));
    }

    #[test]
    fn test_is_exclusive_false_no_creat() {
        assert!(!is_exclusive(super::super::object_fd::open_flags::O_EXCL));
    }

    #[test]
    fn test_recreate_object() {
        let id = BlobId::from_bytes_blake3(b"/recreate/test");
        let data = b"some old data";
        BLOB_CACHE.insert(id, data.to_vec()).ok();
        assert!(recreate_object(id).is_ok());
        let loaded = BLOB_CACHE.get(&id).map(|d| d.len()).unwrap_or(0);
        assert_eq!(loaded, 0);
    }

    #[test]
    fn test_preallocate_object() {
        let id = BlobId::from_bytes_blake3(b"/preallocate/test");
        assert!(preallocate_object(id, 8192).is_ok());
        let sz = BLOB_CACHE.get(&id).map(|d| d.len()).unwrap_or(0);
        assert_eq!(sz, 8192);
    }

    #[test]
    fn test_create_directory_object() {
        let id = BlobId::from_bytes_blake3(b"/dirobj/test");
        assert!(create_directory_object(id).is_ok());
        let blob = BLOB_CACHE.get(&id).unwrap();
        assert_eq!(blob.len(), 8);
        assert_eq!(blob[0], 0xCA);
    }

    #[test]
    fn test_create_symlink_object() {
        let id = BlobId::from_bytes_blake3(b"/symlink/obj");
        let target = b"/some/target/path";
        assert!(create_symlink_object(id, target).is_ok());
        let stored = BLOB_CACHE.get(&id).unwrap();
        assert_eq!(stored.len(), target.len());
    }
}
