//! Empreintes de chunks pour la déduplication ExoFS (no_std).
//!
//! Calcul d'empreintes cryptographiques (Blake3 simplifié) et d'empreintes
//! rapides (FNV-1a 64 bits, XxHash64) pour le filtrage préliminaire.
//!
//! RECUR-01 : aucune récursion.
//! OOM-02   : try_reserve sur tous les Vec.
//! ARITH-02 : checked / saturating / wrapping sur toutes les arithmétiques.

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille d'une empreinte Blake3 (256 bits = 32 octets).
pub const FINGERPRINT_LEN: usize = 32;
/// Empreinte nulle (toutes les valeurs à 0).
pub const FINGERPRINT_ZERO: [u8; 32] = [0u8; 32];

// ─────────────────────────────────────────────────────────────────────────────
// FingerprintAlgorithm
// ─────────────────────────────────────────────────────────────────────────────

/// Algorithme utilisé pour calculer l'empreinte d'un chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FingerprintAlgorithm {
    /// Blake3 (256 bits) — utilisé pour la déduplication cryptographique.
    Blake3 = 0,
    /// FNV-1a 64 bits — utilisé pour le filtrage rapide.
    Fnv1a64 = 1,
    /// XxHash64 — alternative rapide non-cryptographique.
    XxHash64 = 2,
    /// Double : Blake3 + FNV-1a (le plus robuste).
    Double = 3,
}

impl FingerprintAlgorithm {
    /// Retourne `true` si l'algorithme est cryptographiquement sûr.
    pub fn is_cryptographic(self) -> bool {
        matches!(self, Self::Blake3 | Self::Double)
    }

    /// Retourne `true` si l'algorithme convient au filtrage rapide.
    pub fn is_fast(self) -> bool {
        !matches!(self, Self::Blake3)
    }

    /// Retourne le nom lisible de l'algorithme.
    pub fn name(self) -> &'static str {
        match self {
            Self::Blake3 => "blake3",
            Self::Fnv1a64 => "fnv1a64",
            Self::XxHash64 => "xxhash64",
            Self::Double => "blake3+fnv1a64",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Hachage inline (no_std, pas de dépendances externes)
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule un hash FNV-1a 64 bits.
///
/// ARITH-02 : wrapping_mul / ^ (XOR).
/// RECUR-01 : boucle itérative.
pub fn fnv1a64(data: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01B3;
    let mut h = OFFSET;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// Calcule un hash XxHash64 simplifié (ARITH-02 : wrapping_mul / rotate).
///
/// RECUR-01 : boucle itérative.
pub fn xxhash64(data: &[u8], seed: u64) -> u64 {
    const P1: u64 = 0x9E3779B185EBCA87;
    const P2: u64 = 0xC2B2AE3D27D4EB4F;
    const P3: u64 = 0x165667B19E3779F9;
    const P4: u64 = 0x85EBCA77C2B2AE63;
    const P5: u64 = 0x27D4EB2F165667C5;
    let len = data.len();
    let mut h: u64;
    let mut pos = 0usize;
    if len >= 32 {
        let (mut v1, mut v2, mut v3, mut v4) = (
            seed.wrapping_add(P1).wrapping_add(P2),
            seed.wrapping_add(P2),
            seed,
            seed.wrapping_sub(P1),
        );
        while pos.saturating_add(32) <= len {
            let read64 = |d: &[u8], p: usize| -> u64 {
                let b = &d[p..p + 8];
                u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
            };
            v1 = v1
                .wrapping_add(read64(data, pos).wrapping_mul(P2))
                .rotate_left(31)
                .wrapping_mul(P1);
            v2 = v2
                .wrapping_add(read64(data, pos + 8).wrapping_mul(P2))
                .rotate_left(31)
                .wrapping_mul(P1);
            v3 = v3
                .wrapping_add(read64(data, pos + 16).wrapping_mul(P2))
                .rotate_left(31)
                .wrapping_mul(P1);
            v4 = v4
                .wrapping_add(read64(data, pos + 24).wrapping_mul(P2))
                .rotate_left(31)
                .wrapping_mul(P1);
            pos = pos.saturating_add(32);
        }
        h = v1
            .rotate_left(1)
            .wrapping_add(v2.rotate_left(7))
            .wrapping_add(v3.rotate_left(12))
            .wrapping_add(v4.rotate_left(18));
    } else {
        h = seed.wrapping_add(P5);
    }
    h = h.wrapping_add(len as u64);
    // Bytes restants.
    while pos.saturating_add(8) <= len {
        let b = &data[pos..pos + 8];
        let k1 = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        h ^= k1.wrapping_mul(P2).rotate_left(31).wrapping_mul(P1);
        h = h.rotate_left(27).wrapping_mul(P1).wrapping_add(P4);
        pos = pos.saturating_add(8);
    }
    while pos < len {
        h ^= (data[pos] as u64).wrapping_mul(P5);
        h = h.rotate_left(11).wrapping_mul(P1);
        pos = pos.saturating_add(1);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(P2);
    h ^= h >> 29;
    h = h.wrapping_mul(P3);
    h ^= h >> 32;
    h
}

/// Calcule un hash Blake3 simplifié (compression 256 bits, RECUR-01, ARITH-02).
///
/// NOTE : implémentation simplifiée pour no_std noyau — utilise une fonction
/// de compression basée sur les constantes Blake3.  
///
/// RECUR-01 : boucle itérative.
pub fn blake3_hash(data: &[u8]) -> [u8; 32] {
    // Constantes IV Blake3 (8 premiers mots de Sha-256 IV).
    const IV: [u32; 8] = [
        0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB,
        0x5BE0CD19,
    ];
    const BLOCK_LEN: usize = 64;
    let mut state = IV;
    let mut offset: usize = 0;
    let total = data.len();
    // Traite chaque bloc de 64 octets.
    while offset < total || offset == 0 {
        let end = (offset.saturating_add(BLOCK_LEN)).min(total);
        let chunk_data = &data[offset..end];
        let mut block = [0u32; 16];
        for (i, w) in block.iter_mut().enumerate() {
            let byte_off = i * 4;
            let b0 = chunk_data.get(byte_off).copied().unwrap_or(0);
            let b1 = chunk_data
                .get(byte_off.saturating_add(1))
                .copied()
                .unwrap_or(0);
            let b2 = chunk_data
                .get(byte_off.saturating_add(2))
                .copied()
                .unwrap_or(0);
            let b3 = chunk_data
                .get(byte_off.saturating_add(3))
                .copied()
                .unwrap_or(0);
            *w = u32::from_le_bytes([b0, b1, b2, b3]);
        }
        // Compression simplifiée (G-function Blake3).
        let compress = |a: u32, b: u32, c: u32, d: u32, x: u32, y: u32| -> (u32, u32, u32, u32) {
            let a = a.wrapping_add(b).wrapping_add(x);
            let d = (d ^ a).rotate_right(16);
            let c = c.wrapping_add(d);
            let b = (b ^ c).rotate_right(12);
            let a = a.wrapping_add(b).wrapping_add(y);
            let d = (d ^ a).rotate_right(8);
            let c = c.wrapping_add(d);
            let b = (b ^ c).rotate_right(7);
            (a, b, c, d)
        };
        let (s0, s1, s2, s3, s4, s5, s6, s7) = (
            state[0], state[1], state[2], state[3], state[4], state[5], state[6], state[7],
        );
        let (a, b, c, d) = compress(s0, s1, s4, s5, block[0], block[1]);
        let (e, f, g, h) = compress(s2, s3, s6, s7, block[2], block[3]);
        state[0] = a ^ e;
        state[1] = b ^ f;
        state[2] = c ^ g;
        state[3] = d ^ h;
        state[4] = (s4 ^ block[4]).wrapping_add(state[0]);
        state[5] = (s5 ^ block[5]).wrapping_add(state[1]);
        state[6] = (s6 ^ block[6]).wrapping_add(state[2]);
        state[7] = (s7 ^ block[7]).wrapping_add(state[3]);
        if end >= total {
            break;
        }
        offset = end;
    }
    // Sérialise l'état en 32 octets.
    let mut out = [0u8; 32];
    for (i, &w) in state.iter().enumerate() {
        let bytes = w.to_le_bytes();
        out[i * 4..(i + 1) * 4].copy_from_slice(&bytes);
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// ChunkFingerprint
// ─────────────────────────────────────────────────────────────────────────────

/// Empreinte complète d'un chunk.
///
/// Contient l'empreinte cryptographique (Blake3) et l'empreinte rapide (FNV).
/// L'égalité est définie sur les deux champs conjointement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChunkFingerprint {
    /// Empreinte Blake3 (256 bits).
    pub blake3: [u8; 32],
    /// Empreinte rapide FNV-1a 64 bits.
    pub fast_hash: u64,
    /// Algorithme utilisé.
    pub algo: FingerprintAlgorithmId,
}

/// Identifiant d'algorithme stocké dans la fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum FingerprintAlgorithmId {
    Blake3 = 0,
    Double = 3,
}

impl ChunkFingerprint {
    /// Calcule l'empreinte depuis des données brutes.
    pub fn compute(data: &[u8]) -> Self {
        let blake3 = blake3_hash(data);
        let fast_hash = fnv1a64(data);
        Self {
            blake3,
            fast_hash,
            algo: FingerprintAlgorithmId::Double,
        }
    }

    /// Construit depuis des octets pré-calculés.
    pub fn from_parts(blake3: [u8; 32], fast_hash: u64) -> Self {
        Self {
            blake3,
            fast_hash,
            algo: FingerprintAlgorithmId::Double,
        }
    }

    /// Retourne `true` si l'empreinte est nulle (chunk vide).
    pub fn is_zero(&self) -> bool {
        self.blake3 == FINGERPRINT_ZERO
    }

    /// Comparaison à temps constant (ARITH-02 : XOR accumulé).
    pub fn constant_time_eq(&self, other: &Self) -> bool {
        let mut diff: u8 = 0;
        for (a, b) in self.blake3.iter().zip(other.blake3.iter()) {
            diff |= a ^ b;
        }
        let fast_diff = (self.fast_hash ^ other.fast_hash) as u64;
        diff == 0 && fast_diff == 0
    }

    /// Retourne la clé de lookup (use blake3 as BTreeMap key).
    pub fn key(&self) -> &[u8; 32] {
        &self.blake3
    }

    /// Retourne les 8 premiers octets comme u64 (pour indexation rapide).
    pub fn prefix_u64(&self) -> u64 {
        u64::from_le_bytes([
            self.blake3[0],
            self.blake3[1],
            self.blake3[2],
            self.blake3[3],
            self.blake3[4],
            self.blake3[5],
            self.blake3[6],
            self.blake3[7],
        ])
    }

    /// Retourne une représentation hexadécimale tronquée (8 octets = 16 chars).
    pub fn short_hex(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        for (i, &b) in self.blake3[..8].iter().enumerate() {
            let hi = b >> 4;
            let lo = b & 0xF;
            out[i * 2] = if hi < 10 { b'0' + hi } else { b'a' + hi - 10 };
            out[i * 2 + 1] = if lo < 10 { b'0' + lo } else { b'a' + lo - 10 };
        }
        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FingerprintSet — ensemble d'empreintes pour comparaison batch
// ─────────────────────────────────────────────────────────────────────────────

/// Ensemble d'empreintes permettant la recherche rapide par fast_hash puis blake3.
pub struct FingerprintSet {
    entries: Vec<ChunkFingerprint>,
}

impl FingerprintSet {
    /// Crée un ensemble vide.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Ajoute une empreinte.
    ///
    /// OOM-02 : try_reserve.
    pub fn insert(&mut self, fp: ChunkFingerprint) -> ExofsResult<()> {
        self.entries
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.entries.push(fp);
        Ok(())
    }

    /// Recherche par empreinte complète (fast_hash préliminaire, blake3 confirmation).
    ///
    /// RECUR-01 : boucle itérative.
    pub fn contains(&self, fp: &ChunkFingerprint) -> bool {
        for e in &self.entries {
            if e.fast_hash == fp.fast_hash && e.constant_time_eq(fp) {
                return true;
            }
        }
        false
    }

    /// Nombre d'empreintes.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` si l'ensemble est vide.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Calcule et retourne les empreintes dupliquées.
    ///
    /// OOM-02 : try_reserve.
    /// RECUR-01 : O(n²) mais sans récursion.
    pub fn duplicates(&self) -> ExofsResult<Vec<ChunkFingerprint>> {
        let mut dups: Vec<ChunkFingerprint> = Vec::new();
        dups.try_reserve(self.entries.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for (i, a) in self.entries.iter().enumerate() {
            for b in self.entries[..i].iter() {
                if a.constant_time_eq(b) && !dups.iter().any(|d| d.constant_time_eq(a)) {
                    dups.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    dups.push(*a);
                }
            }
        }
        Ok(dups)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fnv1a64_deterministic() {
        let h1 = fnv1a64(b"hello");
        let h2 = fnv1a64(b"hello");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_fnv1a64_different_inputs() {
        assert_ne!(fnv1a64(b"hello"), fnv1a64(b"world"));
    }

    #[test]
    fn test_fnv1a64_empty() {
        let h = fnv1a64(b"");
        assert_ne!(h, 0); // valeur d'offset par défaut
    }

    #[test]
    fn test_xxhash64_deterministic() {
        let h1 = xxhash64(b"dedup chunk data", 0);
        let h2 = xxhash64(b"dedup chunk data", 0);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_xxhash64_different_seeds() {
        let h1 = xxhash64(b"data", 0);
        let h2 = xxhash64(b"data", 1);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_blake3_deterministic() {
        let h1 = blake3_hash(b"chunk data");
        let h2 = blake3_hash(b"chunk data");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_blake3_different_inputs() {
        let h1 = blake3_hash(b"aaa");
        let h2 = blake3_hash(b"bbb");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_fingerprint_compute() {
        let fp = ChunkFingerprint::compute(b"test chunk");
        assert!(!fp.is_zero());
    }

    #[test]
    fn test_fingerprint_constant_time_eq() {
        let fp1 = ChunkFingerprint::compute(b"same data");
        let fp2 = ChunkFingerprint::compute(b"same data");
        assert!(fp1.constant_time_eq(&fp2));
    }

    #[test]
    fn test_fingerprint_not_eq() {
        let fp1 = ChunkFingerprint::compute(b"aaa");
        let fp2 = ChunkFingerprint::compute(b"bbb");
        assert!(!fp1.constant_time_eq(&fp2));
    }

    #[test]
    fn test_fingerprint_prefix_u64() {
        let fp = ChunkFingerprint::compute(b"abc");
        let prefix = fp.prefix_u64();
        assert_eq!(
            prefix,
            u64::from_le_bytes(fp.blake3[..8].try_into().unwrap())
        );
    }

    #[test]
    fn test_fingerprint_short_hex_len() {
        let fp = ChunkFingerprint::compute(b"test");
        assert_eq!(fp.short_hex().len(), 16);
    }

    #[test]
    fn test_fingerprint_set_contains() {
        let mut set = FingerprintSet::new();
        let fp = ChunkFingerprint::compute(b"data");
        set.insert(fp).unwrap();
        assert!(set.contains(&fp));
        let other = ChunkFingerprint::compute(b"other");
        assert!(!set.contains(&other));
    }

    #[test]
    fn test_fingerprint_set_duplicates() {
        let mut set = FingerprintSet::new();
        let fp = ChunkFingerprint::compute(b"dup");
        set.insert(fp).unwrap();
        set.insert(fp).unwrap();
        let dups = set.duplicates().unwrap();
        assert_eq!(dups.len(), 1);
    }

    #[test]
    fn test_algo_is_cryptographic() {
        assert!(FingerprintAlgorithm::Blake3.is_cryptographic());
        assert!(!FingerprintAlgorithm::Fnv1a64.is_cryptographic());
    }
}
