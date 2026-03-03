//! object_open.rs — SYS_EXOFS_OBJECT_OPEN (501) — ouverture d'un objet ExoFS.
//!
//! RÈGLE 9  : copy_from_user() obligatoire.
//! RÈGLE 10 : buffer chemin sur le tas.
//! RECUR-01 : while, pas de for.
//! OOM-02   : try_reserve avant push.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use super::validation::{
    read_user_path_heap, write_user_u64_opt, exofs_err_to_errno,
    validate_open_flags, validate_user_ptr,
    EINVAL, EFAULT, ENOMEM, ERANGE, EBADF,
};
use super::object_fd::{OBJECT_TABLE, open_flags};

// ─────────────────────────────────────────────────────────────────────────────
// Arguments userspace
// ─────────────────────────────────────────────────────────────────────────────

/// Arguments étendus optionnels passés via `args_ptr`.
///
/// Si `args_ptr == 0`, les valeurs par défaut s'appliquent.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OpenArgs {
    /// Flags d'ouverture (O_RDONLY, O_RDWR, O_CREAT, …).
    pub flags:     u32,
    /// Permissions POSIX de création (si O_CREAT actif).
    pub mode:      u32,
    /// Epoch explicite à ouvrir (0 = epoch courante).
    pub epoch_id:  u64,
    /// Owner UID de l'appelant.
    pub owner_uid: u64,
    /// Taille attendue (hint au kernel, 0 = inconnue).
    pub size_hint: u64,
    /// Réservé pour extensions futures.
    pub _reserved: [u64; 2],
}

const _: () = assert!(core::mem::size_of::<OpenArgs>() == 48);

impl OpenArgs {
    fn defaults() -> Self {
        Self {
            flags:     open_flags::O_RDONLY,
            mode:      0o644,
            epoch_id:  0,
            owner_uid: 0,
            size_hint: 0,
            _reserved: [0u64; 2],
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Logique d'ouverture
// ─────────────────────────────────────────────────────────────────────────────

/// Ouvre un objet ExoFS identifié par son chemin.
///
/// Retourne le fd (u32) ou une ExofsError.
fn open_object(
    path_bytes: &[u8],
    path_len:   usize,
    args:       &OpenArgs,
) -> ExofsResult<u32> {
    // Valider les flags.
    if args.flags & !0x07FF != 0 {
        return Err(ExofsError::InvalidArgument);
    }

    // Dériver le BlobId du chemin (Blake3 du chemin canonique).
    let blob_id = BlobId::from_bytes_blake3(&path_bytes[..path_len]);

    // Ouvrir via la table de fd.
    OBJECT_TABLE.open(
        blob_id,
        args.flags,
        args.size_hint,
        args.epoch_id,
        args.owner_uid,
    )
}

/// Lit les OpenArgs depuis userspace (ou retourne les valeurs par défaut si
/// `args_ptr == 0`).
fn read_open_args(args_ptr: u64, flags_fallback: u32) -> Result<OpenArgs, i64> {
    if args_ptr == 0 {
        let mut a = OpenArgs::defaults();
        a.flags = flags_fallback;
        return Ok(a);
    }
    // RÈGLE 9 : copy_from_user pour structure userspace.
    let mut a = OpenArgs::defaults();
    unsafe {
        super::validation::copy_struct_from_user::<OpenArgs>(args_ptr)
            .map_err(|_| EFAULT)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler syscall SYS_EXOFS_OBJECT_OPEN (501)
// ─────────────────────────────────────────────────────────────────────────────

/// `exofs_object_open(path_ptr, path_len, flags, out_fd_ptr, args_ptr, _) → fd ou errno`
///
/// - `path_ptr`   : pointeur userspace chemin NUL-terminé (UTF-8).
/// - `path_len`   : longueur maximale du chemin (hint ≤ PATH_MAX).
/// - `flags`      : O_RDONLY/O_WRONLY/O_RDWR/O_CREAT/O_TRUNC/O_EXCL.
/// - `out_fd_ptr` : pointeur optionnel vers u32 userspace pour stocker le fd.
/// - `args_ptr`   : pointeur optionnel vers `OpenArgs` (0 = valeurs par défaut).
/// - Retourne     : fd (≥ 4) en cas de succès, errno négatif sinon.
pub fn sys_exofs_object_open(
    path_ptr:   u64,
    path_len:   u64,
    flags:      u64,
    out_fd_ptr: u64,
    args_ptr:   u64,
    _a6:        u64,
) -> i64 {
    // 1. Validation de base.
    if path_ptr == 0 { return EFAULT; }

    // 2. Lire le chemin (heap, RÈGLE 10).
    let mut path_buf: Vec<u8> = Vec::new();
    let actual_len = match read_user_path_heap(path_ptr, &mut path_buf) {
        Ok(l)  => l,
        Err(e) => return e,
    };

    // 3. Lire les OpenArgs.
    let open_args = match read_open_args(args_ptr, flags as u32) {
        Ok(a)  => a,
        Err(e) => return e,
    };

    // 4. Valider les flags.
    if let Err(e) = validate_open_flags(open_args.flags as u64) {
        return e;
    }

    // 5. Ouvrir l'objet.
    let fd = match open_object(&path_buf, actual_len, &open_args) {
        Ok(fd) => fd,
        Err(e) => return exofs_err_to_errno(e),
    };

    // 6. Écrire le fd vers userspace si demandé.
    if out_fd_ptr != 0 {
        if let Err(e) = super::validation::write_user_buf(out_fd_ptr, &fd.to_le_bytes()) {
            // Fermer le fd avant de retourner l'erreur.
            OBJECT_TABLE.close(fd);
            return e;
        }
    }

    fd as i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Constantes de flags (ré-exportées pour les syscalls utilisateurs)
// ─────────────────────────────────────────────────────────────────────────────

pub use super::object_fd::open_flags as flags;

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_path(s: &str) -> Vec<u8> {
        let b = s.as_bytes();
        let mut v = Vec::new();
        v.try_reserve(b.len().saturating_add(1)).unwrap();
        let mut i = 0usize;
        while i < b.len() {
            v.push(b[i]);
            i = i.wrapping_add(1);
        }
        v.push(0u8); // NUL
        v
    }

    #[test]
    fn test_open_rdonly_default_args() {
        let args = OpenArgs { flags: open_flags::O_RDONLY, ..OpenArgs::defaults() };
        let path = make_path("/test/file1");
        let fd = open_object(&path, path.len() - 1, &args).unwrap();
        assert!(fd >= super::super::object_fd::FD_RESERVED);
        OBJECT_TABLE.close(fd);
    }

    #[test]
    fn test_open_rdwr() {
        let args = OpenArgs { flags: open_flags::O_RDWR, ..OpenArgs::defaults() };
        let path = make_path("/test/rw");
        let fd = open_object(&path, path.len() - 1, &args).unwrap();
        assert!(OBJECT_TABLE.check_readable(fd).is_ok());
        assert!(OBJECT_TABLE.check_writable(fd).is_ok());
        OBJECT_TABLE.close(fd);
    }

    #[test]
    fn test_open_wronly() {
        let args = OpenArgs { flags: open_flags::O_WRONLY, ..OpenArgs::defaults() };
        let path = make_path("/w/only");
        let fd = open_object(&path, path.len() - 1, &args).unwrap();
        assert!(OBJECT_TABLE.check_readable(fd).is_err());
        assert!(OBJECT_TABLE.check_writable(fd).is_ok());
        OBJECT_TABLE.close(fd);
    }

    #[test]
    fn test_open_bad_flags_rejected() {
        let args = OpenArgs { flags: 0xDEAD_BEEF, ..OpenArgs::defaults() };
        let path = make_path("/bad");
        assert!(open_object(&path, path.len() - 1, &args).is_err());
    }

    #[test]
    fn test_open_two_different_paths_different_fds() {
        let args = OpenArgs::defaults();
        let p1 = make_path("/file/alpha");
        let p2 = make_path("/file/beta");
        let fd1 = open_object(&p1, p1.len() - 1, &args).unwrap();
        let fd2 = open_object(&p2, p2.len() - 1, &args).unwrap();
        assert_ne!(fd1, fd2);
        OBJECT_TABLE.close(fd1);
        OBJECT_TABLE.close(fd2);
    }

    #[test]
    fn test_open_blob_id_derived_from_path() {
        let args = OpenArgs::defaults();
        let p1 = make_path("/unique/path/a");
        let p2 = make_path("/unique/path/a");
        let fd1 = open_object(&p1, p1.len() - 1, &args).unwrap();
        let fd2 = open_object(&p2, p2.len() - 1, &args).unwrap();
        let b1 = OBJECT_TABLE.blob_id_of(fd1).unwrap();
        let b2 = OBJECT_TABLE.blob_id_of(fd2).unwrap();
        assert_eq!(b1.0, b2.0);
        OBJECT_TABLE.close(fd1);
        OBJECT_TABLE.close(fd2);
    }

    #[test]
    fn test_sys_open_null_path() {
        assert_eq!(sys_exofs_object_open(0, 0, 0, 0, 0, 0), EFAULT);
    }

    #[test]
    fn test_open_with_size_hint() {
        let args = OpenArgs { flags: open_flags::O_RDONLY, size_hint: 4096, ..OpenArgs::defaults() };
        let path = make_path("/sized/blob");
        let fd = open_object(&path, path.len() - 1, &args).unwrap();
        let entry = OBJECT_TABLE.get(fd).unwrap();
        assert_eq!(entry.size, 4096);
        OBJECT_TABLE.close(fd);
    }

    #[test]
    fn test_open_with_epoch_id() {
        let args = OpenArgs { flags: open_flags::O_RDONLY, epoch_id: 42, ..OpenArgs::defaults() };
        let path = make_path("/epoch/42");
        let fd = open_object(&path, path.len() - 1, &args).unwrap();
        let entry = OBJECT_TABLE.get(fd).unwrap();
        assert_eq!(entry.epoch_id, 42);
        OBJECT_TABLE.close(fd);
    }

    #[test]
    fn test_open_args_defaults() {
        let a = OpenArgs::defaults();
        assert_eq!(a.flags, open_flags::O_RDONLY);
        assert_eq!(a.epoch_id, 0);
    }

    #[test]
    fn test_open_args_size() {
        assert_eq!(core::mem::size_of::<OpenArgs>(), 48);
    }

    #[test]
    fn test_read_open_args_fallback() {
        let a = read_open_args(0, open_flags::O_WRONLY).unwrap();
        assert_eq!(a.flags, open_flags::O_WRONLY);
    }

    #[test]
    fn test_open_close_count_restored() {
        let before = OBJECT_TABLE.open_count();
        let args = OpenArgs::defaults();
        let path = make_path("/cnt/test");
        let fd = open_object(&path, path.len() - 1, &args).unwrap();
        assert_eq!(OBJECT_TABLE.open_count(), before + 1);
        OBJECT_TABLE.close(fd);
        assert_eq!(OBJECT_TABLE.open_count(), before);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers supplémentaires d'ouverture
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat détaillé d'une ouverture, renvoyé optionnellement vers userspace.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OpenResult {
    /// Numéro de fd attribué.
    pub fd:        u32,
    /// Padding.
    pub _pad:      u32,
    /// BlobId de l'objet (32 octets).
    pub blob_id:   [u8; 32],
    /// Taille actuelle de l'objet en octets.
    pub size:      u64,
    /// Epoch au moment de l'ouverture.
    pub epoch_id:  u64,
    /// Flags effectivement appliqués.
    pub flags:     u32,
    /// Padding.
    pub _pad2:     u32,
}

const _: () = assert!(core::mem::size_of::<OpenResult>() == 72);

/// Ouvre un fd et remplit un `OpenResult`.
pub fn open_object_full(
    path_bytes: &[u8],
    path_len:   usize,
    args:       &OpenArgs,
) -> ExofsResult<OpenResult> {
    if args.flags & !0x07FF != 0 { return Err(ExofsError::InvalidArgument); }
    let blob_id = BlobId::from_bytes_blake3(&path_bytes[..path_len]);
    let fd = OBJECT_TABLE.open(blob_id, args.flags, args.size_hint, args.epoch_id, args.owner_uid)?;
    Ok(OpenResult {
        fd,
        _pad:    0,
        blob_id: *blob_id.as_bytes(),
        size:    args.size_hint,
        epoch_id:args.epoch_id,
        flags:   args.flags,
        _pad2:   0,
    })
}

/// Vérifie la compatibilité des flags avec un accès specifique.
///
/// `access` : 0 = lecture, 1 = écriture, 2 = les deux.
pub fn check_access_flags(flags: u32, access: u8) -> ExofsResult<()> {
    match access {
        0 => if !open_flags::can_read(flags)  { Err(ExofsError::PermissionDenied) } else { Ok(()) },
        1 => if !open_flags::can_write(flags) { Err(ExofsError::PermissionDenied) } else { Ok(()) },
        2 => {
            if !open_flags::can_read(flags) || !open_flags::can_write(flags) {
                Err(ExofsError::PermissionDenied)
            } else {
                Ok(())
            }
        }
        _ => Err(ExofsError::InvalidArgument),
    }
}

/// Valide une combinaison de flags pour la cohérence.
///
/// Ex : O_RDONLY + O_TRUNC est invalide (on ne peut pas tronquer en lecture).
pub fn validate_flags_combination(flags: u32) -> ExofsResult<()> {
    let rw = flags & 0x0003;
    if rw == open_flags::O_RDONLY && (flags & open_flags::O_TRUNC != 0) {
        return Err(ExofsError::InvalidArgument);
    }
    if rw == open_flags::O_RDONLY && (flags & open_flags::O_APPEND != 0) {
        return Err(ExofsError::InvalidArgument);
    }
    Ok(())
}

#[cfg(test)]
mod tests_extended {
    use super::*;

    #[test]
    fn test_open_result_size() {
        assert_eq!(core::mem::size_of::<OpenResult>(), 72);
    }

    #[test]
    fn test_open_object_full_ok() {
        let args = OpenArgs { flags: open_flags::O_RDWR, size_hint: 512, ..OpenArgs::defaults() };
        let path = b"/full/result";
        let r = open_object_full(path, path.len(), &args).unwrap();
        assert!(r.fd >= super::super::object_fd::FD_RESERVED);
        assert_eq!(r.size, 512);
        assert_eq!(r.flags, open_flags::O_RDWR);
        OBJECT_TABLE.close(r.fd);
    }

    #[test]
    fn test_check_access_read() {
        assert!(check_access_flags(open_flags::O_RDONLY, 0).is_ok());
        assert!(check_access_flags(open_flags::O_RDWR, 0).is_ok());
        assert!(check_access_flags(open_flags::O_WRONLY, 0).is_err());
    }

    #[test]
    fn test_check_access_write() {
        assert!(check_access_flags(open_flags::O_WRONLY, 1).is_ok());
        assert!(check_access_flags(open_flags::O_RDWR, 1).is_ok());
        assert!(check_access_flags(open_flags::O_RDONLY, 1).is_err());
    }

    #[test]
    fn test_check_access_rw() {
        assert!(check_access_flags(open_flags::O_RDWR, 2).is_ok());
        assert!(check_access_flags(open_flags::O_RDONLY, 2).is_err());
        assert!(check_access_flags(open_flags::O_WRONLY, 2).is_err());
    }

    #[test]
    fn test_validate_flags_combo_rdonly_trunc() {
        let f = open_flags::O_RDONLY | open_flags::O_TRUNC;
        assert!(validate_flags_combination(f).is_err());
    }

    #[test]
    fn test_validate_flags_combo_rdonly_append() {
        let f = open_flags::O_RDONLY | open_flags::O_APPEND;
        assert!(validate_flags_combination(f).is_err());
    }

    #[test]
    fn test_validate_flags_combo_rdwr_trunc_ok() {
        let f = open_flags::O_RDWR | open_flags::O_TRUNC;
        assert!(validate_flags_combination(f).is_ok());
    }

    #[test]
    fn test_open_object_full_bad_flags() {
        let args = OpenArgs { flags: 0xDEAD_CAFE, ..OpenArgs::defaults() };
        let path = b"/bad/flags";
        assert!(open_object_full(path, path.len(), &args).is_err());
    }
}
