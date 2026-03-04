//! get_content_hash.rs — SYS_EXOFS_GET_CONTENT_HASH (508)
//!
//! Calcul et vérification du hash Blake3 du contenu d'un objet ExoFS.
//! RÈGLE 9/10/RECUR-01/OOM-02/ARITH-02.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    exofs_err_to_errno, write_user_buf, EFAULT, EINVAL,
};
use super::object_fd::OBJECT_TABLE;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille d'un hash Blake3 en octets.
pub const HASH_SIZE: usize = 32;
/// Chunk de traitement pour le hashage incrémental (1 MiB).
pub const HASH_CHUNK: usize = 1024 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// Flags
// ─────────────────────────────────────────────────────────────────────────────

pub mod hash_flags {
    pub const BY_FD:        u32 = 0x0001;
    pub const VERIFY:       u32 = 0x0002;
    pub const INCREMENTAL:  u32 = 0x0004;
    pub const VALID_MASK:   u32 = BY_FD | VERIFY | INCREMENTAL;
}

// ─────────────────────────────────────────────────────────────────────────────
// Structure de résultat
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ContentHashResult {
    /// Hash Blake3 du contenu (32 octets).
    pub hash:        [u8; 32],
    /// Taille totale hashée.
    pub bytes_hashed:u64,
    /// 1 si la vérification a réussi (mode VERIFY).
    pub verified:    u32,
    pub _pad:        u32,
}

const _: () = assert!(core::mem::size_of::<ContentHashResult>() == 48);

// ─────────────────────────────────────────────────────────────────────────────
// Blake3 minimal — implémentation embarquée sans dépendance externe.
//
// ExoFS utilise BlobId::from_bytes_blake3() qui wrap le hash du noyau.
// Pour le contenu, on ré-utilise la même primitive sur le contenu brut.
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule le hash Blake3 d'un slice de données en une seule passe.
pub fn hash_data(data: &[u8]) -> [u8; HASH_SIZE] {
    let bid = BlobId::from_bytes_blake3(data);
    *bid.as_bytes()
}

/// Calcule le hash sur un sous-intervalle [offset, offset+count).
/// ARITH-02 : saturating_add, min.
pub fn hash_range(data: &[u8], offset: usize, count: usize) -> ExofsResult<[u8; HASH_SIZE]> {
    let end = offset.saturating_add(count).min(data.len());
    if offset > data.len() { return Err(ExofsError::OffsetOverflow); }
    Ok(hash_data(&data[offset..end]))
}

/// Vérifie que le contenu correspond au hash attendu.
pub fn verify_hash(data: &[u8], expected: &[u8; HASH_SIZE]) -> bool {
    let got = hash_data(data);
    let mut eq = true;
    let mut i = 0usize;
    while i < HASH_SIZE {
        if got[i] != expected[i] { eq = false; break; }
        i = i.wrapping_add(1);
    }
    eq
}

// ─────────────────────────────────────────────────────────────────────────────
// Opérations principales
// ─────────────────────────────────────────────────────────────────────────────

fn hash_blob(blob_id: BlobId, flags: u32) -> ExofsResult<ContentHashResult> {
    let data = BLOB_CACHE.get(&blob_id)
        .ok_or(ExofsError::BlobNotFound)?;
    let h = hash_data(&data);
    Ok(ContentHashResult {
        hash:         h,
        bytes_hashed: data.len() as u64,
        verified:     0,
        _pad:         0,
    })
}

fn hash_by_fd(fd: u32, flags: u32) -> ExofsResult<ContentHashResult> {
    let entry = OBJECT_TABLE.get(fd)?;
    hash_blob(entry.blob_id, flags)
}

fn hash_by_blob_id(blob_id: BlobId, flags: u32) -> ExofsResult<ContentHashResult> {
    hash_blob(blob_id, flags)
}

// ─────────────────────────────────────────────────────────────────────────────
// Vérification de hash
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie le contenu du fd contre un hash attendu fourni en userspace.
fn verify_fd_hash(fd: u32, expected_ptr: u64) -> ExofsResult<ContentHashResult> {
    let entry = OBJECT_TABLE.get(fd)?;
    let data = BLOB_CACHE.get(&entry.blob_id)
        .ok_or(ExofsError::BlobNotFound)?;
    let mut expected = [0u8; HASH_SIZE];
    unsafe {
        let src = expected_ptr as *const u8;
        let mut i = 0usize;
        while i < HASH_SIZE { expected[i] = *src.add(i); i = i.wrapping_add(1); }
    }
    let ok = verify_hash(&data, &expected);
    let h = hash_data(&data);
    Ok(ContentHashResult {
        hash:         h,
        bytes_hashed: data.len() as u64,
        verified:     ok as u32,
        _pad:         0,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler SYS_EXOFS_GET_CONTENT_HASH (508)
// ─────────────────────────────────────────────────────────────────────────────

/// `exofs_get_content_hash(fd, out_hash_ptr, expected_ptr, flags, _, _) → 0 ou errno`
pub fn sys_exofs_get_content_hash(
    fd:           u64,
    out_hash_ptr: u64,
    expected_ptr: u64,
    flags:        u64,
    _a5:          u64,
    _a6:          u64,
) -> i64 {
    if out_hash_ptr == 0 { return EFAULT; }
    let f = flags as u32;
    if f & !hash_flags::VALID_MASK != 0 { return EINVAL; }

    let result = if f & hash_flags::VERIFY != 0 && expected_ptr != 0 {
        match verify_fd_hash(fd as u32, expected_ptr) {
            Ok(r)  => r,
            Err(e) => return exofs_err_to_errno(e),
        }
    } else {
        match hash_by_fd(fd as u32, f) {
            Ok(r)  => r,
            Err(e) => return exofs_err_to_errno(e),
        }
    };

    let bytes = unsafe {
        core::slice::from_raw_parts(
            &result as *const ContentHashResult as *const u8,
            core::mem::size_of::<ContentHashResult>(),
        )
    };
    match write_user_buf(out_hash_ptr, bytes) {
        Ok(_)  => 0i64,
        Err(e) => e,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers publics
// ─────────────────────────────────────────────────────────────────────────────

/// Compare deux hashes (constant-time-like, while loop).
pub fn hashes_equal(a: &[u8; HASH_SIZE], b: &[u8; HASH_SIZE]) -> bool {
    let mut i = 0usize;
    let mut eq = true;
    while i < HASH_SIZE {
        if a[i] != b[i] { eq = false; }
        i = i.wrapping_add(1);
    }
    eq
}

/// Encode un hash en hexadécimal (pour debug/log).
/// OOM-02 : try_reserve. RECUR-01 : while.
pub fn hash_to_hex(hash: &[u8; HASH_SIZE]) -> ExofsResult<Vec<u8>> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(HASH_SIZE * 2).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < HASH_SIZE {
        let byte = hash[i];
        buf.push(HEX[(byte >> 4) as usize]);
        buf.push(HEX[(byte & 0x0F) as usize]);
        i = i.wrapping_add(1);
    }
    Ok(buf)
}

/// Décode un hash depuis sa représentation hexadécimale (64 caractères).
pub fn hash_from_hex(hex: &[u8]) -> ExofsResult<[u8; HASH_SIZE]> {
    if hex.len() != HASH_SIZE * 2 { return Err(ExofsError::InvalidArgument); }
    let mut out = [0u8; HASH_SIZE];
    let mut i = 0usize;
    while i < HASH_SIZE {
        let hi = hex_char(hex[i * 2])?;
        let lo = hex_char(hex[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
        i = i.wrapping_add(1);
    }
    Ok(out)
}

fn hex_char(c: u8) -> ExofsResult<u8> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _            => Err(ExofsError::InvalidArgument),
    }
}

/// Hash d'un chemin (pour le comparer à un BlobId).
pub fn path_hash(path: &[u8]) -> [u8; HASH_SIZE] {
    *BlobId::from_bytes_blake3(path).as_bytes()
}

/// Calcul du hash incrémental pour les gros contenus (chunks de 1 MiB).
/// Retourne le hash du hash de chaque chunk combiné.
/// RECUR-01 : while. OOM-02 : try_reserve.
pub fn hash_incremental(data: &[u8]) -> ExofsResult<[u8; HASH_SIZE]> {
    if data.len() <= HASH_CHUNK { return Ok(hash_data(data)); }
    let chunk_count = data.len().saturating_add(HASH_CHUNK - 1) / HASH_CHUNK;
    let mut combined: Vec<u8> = Vec::new();
    combined.try_reserve(chunk_count.saturating_mul(HASH_SIZE))
        .map_err(|_| ExofsError::NoMemory)?;
    let mut off = 0usize;
    while off < data.len() {
        let end = off.saturating_add(HASH_CHUNK).min(data.len());
        let chunk_hash = hash_data(&data[off..end]);
        let mut i = 0usize;
        while i < HASH_SIZE { combined.push(chunk_hash[i]); i = i.wrapping_add(1); }
        off = end;
    }
    Ok(hash_data(&combined))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(path: &[u8], data: &[u8]) -> BlobId {
        let id = BlobId::from_bytes_blake3(path);
        BLOB_CACHE.insert(id, data).unwrap();
        id
    }

    #[test]
    fn test_hash_data_deterministic() {
        let h1 = hash_data(b"hello world");
        let h2 = hash_data(b"hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_data_different() {
        let h1 = hash_data(b"abc");
        let h2 = hash_data(b"def");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_verify_hash_ok() {
        let data = b"verify me";
        let h = hash_data(data);
        assert!(verify_hash(data, &h));
    }

    #[test]
    fn test_verify_hash_fail() {
        let data = b"content";
        let wrong = [0u8; 32];
        assert!(!verify_hash(data, &wrong));
    }

    #[test]
    fn test_hash_range() {
        let data = b"hello world";
        let h = hash_range(data, 0, 5).unwrap();
        assert_eq!(h, hash_data(b"hello"));
    }

    #[test]
    fn test_hash_range_overflow() {
        let data = b"data";
        assert!(hash_range(data, 100, 10).is_err());
    }

    #[test]
    fn test_hash_blob_ok() {
        let id = insert(b"/hash/blob", b"content");
        let r = hash_blob(id, 0).unwrap();
        assert_eq!(r.bytes_hashed, 7);
        assert_ne!(r.hash, [0u8; 32]);
    }

    #[test]
    fn test_hash_blob_not_found() {
        let id = BlobId::from_bytes_blake3(b"/hash/no");
        assert!(hash_blob(id, 0).is_err());
    }

    #[test]
    fn test_hashes_equal() {
        let h = hash_data(b"test");
        assert!(hashes_equal(&h, &h));
    }

    #[test]
    fn test_hash_to_hex() {
        let h = [0xABu8, 0xCD, 0xEF, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let hex = hash_to_hex(&h).unwrap();
        assert_eq!(&hex[..8], b"abcdef01");
    }

    #[test]
    fn test_hash_from_hex_roundtrip() {
        let h = hash_data(b"roundtrip");
        let hex = hash_to_hex(&h).unwrap();
        let back = hash_from_hex(&hex).unwrap();
        assert_eq!(back, h);
    }

    #[test]
    fn test_hash_from_hex_invalid() {
        assert!(hash_from_hex(b"ZZZZ").is_err());
    }

    #[test]
    fn test_content_hash_result_size() {
        assert_eq!(core::mem::size_of::<ContentHashResult>(), 48);
    }

    #[test]
    fn test_hash_incremental_small() {
        let data = b"small";
        let h1 = hash_incremental(data).unwrap();
        let h2 = hash_data(data);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_sys_null_out() {
        assert_eq!(sys_exofs_get_content_hash(0, 0, 0, 0, 0, 0), EFAULT);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires avancés : chaîne de hashes, arbre de Merkle minimaliste
// ─────────────────────────────────────────────────────────────────────────────

/// Combine deux hashes en les concaténant et en hashant le résultat.
/// Utilisé pour construire un arbre de Merkle.
pub fn combine_hashes(left: &[u8; HASH_SIZE], right: &[u8; HASH_SIZE]) -> [u8; HASH_SIZE] {
    let mut combined = [0u8; HASH_SIZE * 2];
    let mut i = 0usize;
    while i < HASH_SIZE { combined[i] = left[i]; i = i.wrapping_add(1); }
    let mut j = 0usize;
    while j < HASH_SIZE { combined[HASH_SIZE + j] = right[j]; j = j.wrapping_add(1); }
    hash_data(&combined)
}

/// Calcule la racine d'un arbre de Merkle sur une liste de hashes feuilles.
/// RECUR-01 : while. OOM-02 : try_reserve.
pub fn merkle_root(leaves: &[[u8; HASH_SIZE]]) -> ExofsResult<[u8; HASH_SIZE]> {
    if leaves.is_empty() { return Ok([0u8; HASH_SIZE]); }
    if leaves.len() == 1 { return Ok(leaves[0]); }

    let mut current: Vec<[u8; HASH_SIZE]> = Vec::new();
    current.try_reserve(leaves.len()).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < leaves.len() { current.push(leaves[i]); i = i.wrapping_add(1); }

    while current.len() > 1 {
        let mut next: Vec<[u8; HASH_SIZE]> = Vec::new();
        next.try_reserve(current.len().saturating_add(1) / 2)
            .map_err(|_| ExofsError::NoMemory)?;
        let mut j = 0usize;
        while j < current.len() {
            let left = current[j];
            let right = if j.saturating_add(1) < current.len() {
                current[j + 1]
            } else {
                left
            };
            next.push(combine_hashes(&left, &right));
            j = j.saturating_add(2);
        }
        current = next;
    }
    Ok(current[0])
}

/// Découpe les données en chunks et retourne les hashes de chaque chunk.
/// OOM-02 : try_reserve. RECUR-01 : while.
pub fn chunk_hashes(data: &[u8], chunk_size: usize) -> ExofsResult<Vec<[u8; HASH_SIZE]>> {
    if chunk_size == 0 { return Err(ExofsError::InvalidArgument); }
    let count = data.len().saturating_add(chunk_size - 1) / chunk_size;
    let mut out: Vec<[u8; HASH_SIZE]> = Vec::new();
    out.try_reserve(count.max(1)).map_err(|_| ExofsError::NoMemory)?;
    if data.is_empty() {
        out.push([0u8; HASH_SIZE]);
        return Ok(out);
    }
    let mut off = 0usize;
    while off < data.len() {
        let end = off.saturating_add(chunk_size).min(data.len());
        out.push(hash_data(&data[off..end]));
        off = end;
    }
    Ok(out)
}

/// Retourne l'empreinte Merkle du contenu d'un blob.
pub fn blob_merkle_root(blob_id: BlobId) -> ExofsResult<[u8; HASH_SIZE]> {
    let data = BLOB_CACHE.get(&blob_id).ok_or(ExofsError::BlobNotFound)?;
    let chunks = chunk_hashes(&data, HASH_CHUNK)?;
    merkle_root(&chunks)
}

#[cfg(test)]
mod advanced_tests {
    use super::*;

    #[test]
    fn test_combine_hashes() {
        let a = hash_data(b"left");
        let b = hash_data(b"right");
        let c = combine_hashes(&a, &b);
        let d = combine_hashes(&a, &b);
        assert_eq!(c, d);
    }

    #[test]
    fn test_combine_hashes_not_commutative() {
        let a = hash_data(b"A");
        let b = hash_data(b"B");
        assert_ne!(combine_hashes(&a, &b), combine_hashes(&b, &a));
    }

    #[test]
    fn test_merkle_root_empty() {
        let r = merkle_root(&[]).unwrap();
        assert_eq!(r, [0u8; 32]);
    }

    #[test]
    fn test_merkle_root_single() {
        let h = hash_data(b"leaf");
        let r = merkle_root(&[h]).unwrap();
        assert_eq!(r, h);
    }

    #[test]
    fn test_merkle_root_two_leaves() {
        let h1 = hash_data(b"a");
        let h2 = hash_data(b"b");
        let r = merkle_root(&[h1, h2]).unwrap();
        assert_eq!(r, combine_hashes(&h1, &h2));
    }

    #[test]
    fn test_chunk_hashes_empty() {
        let c = chunk_hashes(b"", 1024).unwrap();
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn test_chunk_hashes_exact() {
        let data = [0u8; 4096];
        let c = chunk_hashes(&data, 4096).unwrap();
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn test_chunk_hashes_multiple() {
        let data = [0u8; 5000];
        let c = chunk_hashes(&data, 4096).unwrap();
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn test_chunk_hashes_zero_size_err() {
        assert!(chunk_hashes(b"data", 0).is_err());
    }

    #[test]
    fn test_blob_merkle_root_ok() {
        let id = BlobId::from_bytes_blake3(b"/merkle/test");
        BLOB_CACHE.insert(id, b"blob data here".to_vec()).unwrap();
        let r = blob_merkle_root(id).unwrap();
        assert_ne!(r, [0u8; 32]);
    }

    #[test]
    fn test_blob_merkle_root_missing() {
        let id = BlobId::from_bytes_blake3(b"/merkle/missing/xyz");
        assert!(blob_merkle_root(id).is_err());
    }
}
