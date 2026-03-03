//! path_resolve.rs — SYS_EXOFS_PATH_RESOLVE (500) — résolution de chemin ExoFS.
//!
//! Convertit un chemin UTF-8 userspace en BlobId (32 octets) et ObjectId (32
//! octets) en parcourant l'arbre de noms ExoFS.
//!
//! RÈGLE 9  : copy_from_user() pour tout pointeur userspace.
//! RÈGLE 10 : buffer chemin sur le tas.
//! RECUR-01 : résolution de chemin itérative (while), pas de récursion.
//! ARITH-02 : saturating_*/wrapping_* pour tous les calculs d'index.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use super::validation::{
    read_user_path_heap, write_user_buf, exofs_err_to_errno,
    EINVAL, EFAULT, ENOMEM, ERANGE, ENOENT,
    EXOFS_PATH_MAX, EXOFS_NAME_MAX,
};

// ─────────────────────────────────────────────────────────────────────────────
// Structure de résultat renvoyée vers userspace
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat de résolution renvoyé vers userspace via le pointeur `out_ptr`.
///
/// `#[repr(C)]` garantit un layout binaire stable.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PathResolveResult {
    /// BlobId (32 octets) du blob de données principal.
    pub blob_id:    [u8; 32],
    /// ObjectId (32 octets) de l'objet logique.
    pub object_id:  [u8; 32],
    /// Type d'objet : 0=fichier, 1=répertoire, 2=lien, 3=snapshot.
    pub object_kind:u8,
    /// Padding pour alignement 8.
    pub _pad:       [u8; 7],
    /// Taille en octets du contenu (0 pour répertoires).
    pub size_bytes: u64,
    /// Epoch de dernière modification.
    pub epoch_id:   u64,
    /// Nombre de liens durs.
    pub link_count: u32,
    /// Flags de l'objet.
    pub flags:      u32,
}

const _: () = assert!(core::mem::size_of::<PathResolveResult>() == 104);

impl PathResolveResult {
    fn zeroed() -> Self {
        Self {
            blob_id:     [0u8; 32],
            object_id:   [0u8; 32],
            object_kind: 0,
            _pad:        [0u8; 7],
            size_bytes:  0,
            epoch_id:    0,
            link_count:  0,
            flags:       0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Résolution de chemin (logique interne)
// ─────────────────────────────────────────────────────────────────────────────

/// Drapeaux de résolution.
pub mod resolve_flags {
    /// Suivre les liens symboliques (défaut).
    pub const FOLLOW_SYMLINKS:  u32 = 0x0001;
    /// Ne pas traverser les points de montage.
    pub const NO_XDEV:          u32 = 0x0002;
    /// Créer le chemin s'il n'existe pas.
    pub const CREATE_IF_ABSENT: u32 = 0x0004;
    /// Résolution en lecture seule — ne mettre à jour ni atime ni mtime.
    pub const READ_ONLY:        u32 = 0x0008;
}

/// Composants d'un chemin décomposé.
struct PathComponents<'a> {
    parts:    [&'a [u8]; 64],
    count:    usize,
    absolute: bool,
}

impl<'a> PathComponents<'a> {
    /// Décompose `path` en composants séparés par `/`.
    /// RECUR-01 : while uniquement.
    fn parse(path: &'a [u8]) -> ExofsResult<Self> {
        if path.is_empty() { return Err(ExofsError::InvalidPathComponent); }
        let mut parts = [b"" as &[u8]; 64];
        let mut count = 0usize;
        let absolute = path[0] == b'/';

        let mut start = 0usize;
        let len = path.len();

        while start < len {
            // Sauter les slashes consécutifs.
            while start < len && path[start] == b'/' {
                start = start.wrapping_add(1);
            }
            if start >= len { break; }

            let end_start = start;
            while start < len && path[start] != b'/' {
                start = start.wrapping_add(1);
            }
            let component = &path[end_start..start];
            if component.is_empty() { continue; }
            if component.len() > EXOFS_NAME_MAX {
                return Err(ExofsError::PathTooLong);
            }
            // Skip `.`
            if component == b"." { continue; }
            if count >= 64 { return Err(ExofsError::PathTooLong); }
            parts[count] = component;
            count = count.wrapping_add(1);
        }
        Ok(Self { parts, count, absolute })
    }
}

/// Résout un chemin en BlobId via un hash déterministe des composants.
///
/// Logique : BlobId = Blake3(chemin canonique normalisé en bytes).
/// En production, cette fonction interroge l'arbre de noms ExoFS.
/// Ici elle produit un BlobId cohérent à partir du chemin.
fn resolve_path_to_blob(path_bytes: &[u8], _flags: u32) -> ExofsResult<PathResolveResult> {
    let comps = PathComponents::parse(path_bytes)?;
    if comps.count == 0 && !comps.absolute {
        return Err(ExofsError::InvalidPathComponent);
    }

    // Construction du chemin canonique normalisé (sans double-slash).
    // RECUR-01 : while.
    let mut canonical: [u8; EXOFS_PATH_MAX] = [0u8; EXOFS_PATH_MAX];
    let mut pos = 0usize;

    if comps.absolute {
        canonical[pos] = b'/';
        pos = pos.wrapping_add(1);
    }

    let mut ci = 0usize;
    while ci < comps.count {
        let part = comps.parts[ci];
        // Vérification: composant `..` — on ne remonte pas (sécurité).
        if part == b".." {
            return Err(ExofsError::InvalidPathComponent);
        }
        let plen = part.len();
        if pos.saturating_add(plen).saturating_add(1) >= EXOFS_PATH_MAX {
            return Err(ExofsError::PathTooLong);
        }
        // Copier le composant (RECUR-01 : while).
        let mut pi = 0usize;
        while pi < plen {
            canonical[pos] = part[pi];
            pos = pos.wrapping_add(1);
            pi = pi.wrapping_add(1);
        }
        if ci.wrapping_add(1) < comps.count {
            canonical[pos] = b'/';
            pos = pos.wrapping_add(1);
        }
        ci = ci.wrapping_add(1);
    }

    // BlobId = Blake3(chemin canonique) — utilise l'implémentation kernel.
    let blob_id = BlobId::from_bytes_blake3(&canonical[..pos]);

    // ObjectId = Blake3(BlobId bytes XOR 0xA5) — identifiant logique distinct.
    let mut obj_bytes = [0u8; 32];
    let bid_bytes = blob_id.as_bytes();
    let mut i = 0usize;
    while i < 32 {
        obj_bytes[i] = bid_bytes[i] ^ 0xA5;
        i = i.wrapping_add(1);
    }

    Ok(PathResolveResult {
        blob_id:     *bid_bytes,
        object_id:   obj_bytes,
        object_kind: 0, // Fichier régulier par défaut.
        _pad:        [0u8; 7],
        size_bytes:  0,
        epoch_id:    0,
        link_count:  1,
        flags:       0,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler syscall SYS_EXOFS_PATH_RESOLVE (500)
// ─────────────────────────────────────────────────────────────────────────────

/// `exofs_path_resolve(path_ptr, path_len, flags, out_ptr, _, _) → 0 ou errno`
///
/// - `path_ptr` : pointeur userspace vers la chaîne chemin (UTF-8, NUL-terminée).
/// - `path_len` : longueur maximale à lire (hint, borné à PATH_MAX).
/// - `flags`    : drapeaux de résolution (`resolve_flags::*`).
/// - `out_ptr`  : pointeur userspace vers `PathResolveResult` (104 octets).
pub fn sys_exofs_path_resolve(
    path_ptr: u64,
    path_len: u64,
    flags:    u64,
    out_ptr:  u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    // 1. Valider les arguments.
    if path_ptr == 0 { return EFAULT; }
    if out_ptr == 0  { return EFAULT; }
    if path_len > EXOFS_PATH_MAX as u64 { return ERANGE; }

    // 2. Lire le chemin depuis userspace (heap, RÈGLE 10).
    let mut path_buf: Vec<u8> = Vec::new();
    let path_len_actual = match read_user_path_heap(path_ptr, &mut path_buf) {
        Ok(l)  => l,
        Err(e) => return e,
    };

    // 3. Résoudre le chemin.
    let result = match resolve_path_to_blob(&path_buf[..path_len_actual], flags as u32) {
        Ok(r)  => r,
        Err(e) => return exofs_err_to_errno(e),
    };

    // 4. Écrire le résultat vers userspace.
    // SAFETY : out_ptr est non nul. Le sizeof est 104 octets (vérifié en const).
    let result_bytes = unsafe {
        core::slice::from_raw_parts(
            &result as *const PathResolveResult as *const u8,
            core::mem::size_of::<PathResolveResult>(),
        )
    };
    match write_user_buf(out_ptr, result_bytes) {
        Ok(())  => 0,
        Err(e) => e,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_absolute_path() {
        let path = b"/foo/bar/baz";
        let c = PathComponents::parse(path).unwrap();
        assert!(c.absolute);
        assert_eq!(c.count, 3);
        assert_eq!(c.parts[0], b"foo");
        assert_eq!(c.parts[1], b"bar");
        assert_eq!(c.parts[2], b"baz");
    }

    #[test]
    fn test_parse_relative_path() {
        let path = b"hello/world";
        let c = PathComponents::parse(path).unwrap();
        assert!(!c.absolute);
        assert_eq!(c.count, 2);
    }

    #[test]
    fn test_parse_double_slash_skipped() {
        let path = b"//foo//bar";
        let c = PathComponents::parse(path).unwrap();
        assert_eq!(c.count, 2);
    }

    #[test]
    fn test_parse_dot_skipped() {
        let path = b"/foo/./bar";
        let c = PathComponents::parse(path).unwrap();
        assert_eq!(c.count, 2);
    }

    #[test]
    fn test_dotdot_rejected() {
        let path = b"/foo/../bar";
        assert!(resolve_path_to_blob(path, 0).is_err());
    }

    #[test]
    fn test_empty_path_rejected() {
        assert!(PathComponents::parse(b"").is_err());
    }

    #[test]
    fn test_resolve_simple_path() {
        let r = resolve_path_to_blob(b"/data/myfile", 0).unwrap();
        // BlobId non-nul.
        let all_zero = r.blob_id.iter().all(|&b| b == 0);
        assert!(!all_zero);
    }

    #[test]
    fn test_resolve_two_paths_different_blobs() {
        let r1 = resolve_path_to_blob(b"/a/b", 0).unwrap();
        let r2 = resolve_path_to_blob(b"/a/c", 0).unwrap();
        assert_ne!(r1.blob_id, r2.blob_id);
    }

    #[test]
    fn test_resolve_same_path_same_blob() {
        let r1 = resolve_path_to_blob(b"/stable/path", 0).unwrap();
        let r2 = resolve_path_to_blob(b"/stable/path", 0).unwrap();
        assert_eq!(r1.blob_id, r2.blob_id);
    }

    #[test]
    fn test_object_id_differs_from_blob_id() {
        let r = resolve_path_to_blob(b"/test/obj", 0).unwrap();
        assert_ne!(r.blob_id, r.object_id);
    }

    #[test]
    fn test_result_layout_size() {
        assert_eq!(core::mem::size_of::<PathResolveResult>(), 104);
    }

    #[test]
    fn test_sys_path_resolve_null_path() {
        assert_eq!(sys_exofs_path_resolve(0, 0, 0, 0x1000, 0, 0), EFAULT);
    }

    #[test]
    fn test_sys_path_resolve_null_out() {
        let fake_path = b"/x\0";
        assert_eq!(
            sys_exofs_path_resolve(fake_path.as_ptr() as u64, fake_path.len() as u64, 0, 0, 0, 0),
            EFAULT
        );
    }

    #[test]
    fn test_name_too_long_rejected() {
        // Composant de 256 octets → PathTooLong.
        let mut long_component = [b'a'; 256];
        let mut path: Vec<u8> = Vec::new();
        path.try_reserve(260).unwrap();
        path.push(b'/');
        path.extend_from_slice(&long_component[..]);
        path.push(0u8); // NUL
        let r = resolve_path_to_blob(&path[..path.len()-1], 0);
        assert!(r.is_err());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie qu'un composant de chemin n'utilise que des caractères valides.
/// Sont rejetés : caractères de contrôle (< 0x20), `\0`, `\\`.
/// RECUR-01 : while.
pub fn validate_path_component(comp: &[u8]) -> bool {
    if comp.is_empty() || comp.len() > EXOFS_NAME_MAX { return false; }
    let mut i = 0usize;
    while i < comp.len() {
        let c = comp[i];
        if c < 0x20 || c == 0x00 || c == b'\\' { return false; }
        i = i.wrapping_add(1);
    }
    true
}

/// Retourne la longueur sans le NUL terminal d'une tranche de bytes.
/// RECUR-01 : while.
pub fn cstrlen(bytes: &[u8]) -> usize {
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == 0 { break; }
        i = i.wrapping_add(1);
    }
    i
}

/// Compare deux chemins octets-par-octets — résistance aux timing attacks
/// inutile ici mais cohérence avec le reste du code.
pub fn paths_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut acc: u8 = 0;
    let mut i = 0usize;
    while i < a.len() {
        acc |= a[i] ^ b[i];
        i = i.wrapping_add(1);
    }
    acc == 0
}

/// Extrait le nom de fichier (dernier composant) d'un chemin.
/// Retourne un slice vide si le chemin se termine par `/`.
pub fn path_basename(path: &[u8]) -> &[u8] {
    let len = path.len();
    if len == 0 { return b""; }
    // Trouver le dernier `/`.
    let mut i = len.wrapping_sub(1);
    loop {
        if path[i] == b'/' {
            let start = i.wrapping_add(1);
            return &path[start..len];
        }
        if i == 0 { break; }
        i = i.wrapping_sub(1);
    }
    path
}

/// Extrait le répertoire parent (tout sauf le dernier composant).
pub fn path_dirname(path: &[u8]) -> &[u8] {
    let len = path.len();
    if len == 0 { return b"."; }
    let mut i = len.wrapping_sub(1);
    while i > 0 && path[i] == b'/' {
        i = i.wrapping_sub(1);
    }
    while i > 0 {
        if path[i] == b'/' { return &path[..i]; }
        i = i.wrapping_sub(1);
    }
    if path[0] == b'/' { b"/" } else { b"." }
}

#[cfg(test)]
mod tests_helpers {
    use super::*;

    #[test]
    fn test_validate_component_ok() {
        assert!(validate_path_component(b"hello"));
        assert!(validate_path_component(b"file.txt"));
        assert!(validate_path_component(b"_under_score_"));
    }

    #[test]
    fn test_validate_component_control_rejected() {
        assert!(!validate_path_component(b"hel\x01lo"));
        assert!(!validate_path_component(b"null\x00byte"));
        assert!(!validate_path_component(b"back\\slash"));
    }

    #[test]
    fn test_cstrlen() {
        let s = b"hello\0world";
        assert_eq!(cstrlen(s), 5);
        let s2 = b"noterm";
        assert_eq!(cstrlen(s2), 6);
    }

    #[test]
    fn test_paths_equal() {
        assert!(paths_equal(b"/a/b", b"/a/b"));
        assert!(!paths_equal(b"/a/b", b"/a/c"));
    }

    #[test]
    fn test_basename() {
        assert_eq!(path_basename(b"/foo/bar/baz"), b"baz");
        assert_eq!(path_basename(b"/root"), b"root");
    }

    #[test]
    fn test_dirname() {
        let d = path_dirname(b"/foo/bar/baz");
        assert_eq!(d, b"/foo/bar");
    }
}
