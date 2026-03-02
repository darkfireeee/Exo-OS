//! ChunkFingerprint — empreintes de chunks pour déduplication intra-volume (no_std).
//!
//! Combine un hash fort (Blake3) et un hash rapide (xxHash64) pour la détection.

use crate::fs::exofs::core::FsError;
use alloc::vec::Vec;

/// Algorithme d'empreinte configuré.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FingerprintAlgorithm {
    Blake3Only   = 0,
    Blake3Xxh64  = 1,  // Deux couches : rapide + fort.
}

/// Empreinte d'un chunk (48 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChunkFingerprint {
    pub blake3:  [u8; 32],
    pub xxhash:  u64,
    pub size:    u32,
    pub algo:    u8,
    pub _pad:    [u8; 3],
}
const _: () = assert!(core::mem::size_of::<ChunkFingerprint>() == 48);

impl ChunkFingerprint {
    /// Calcule l'empreinte d'un bloc de données.
    pub fn compute(data: &[u8], algo: FingerprintAlgorithm) -> Self {
        let blake3 = blake3_simple(data);
        let xxhash = if algo == FingerprintAlgorithm::Blake3Xxh64 {
            xxhash64_simple(data, 0x584558_0000_0000)
        } else {
            0
        };
        Self {
            blake3,
            xxhash,
            size: data.len() as u32,
            algo: algo as u8,
            _pad: [0; 3],
        }
    }

    /// Vérifie si deux empreintes correspondent (comparaison en temps constant).
    pub fn matches(&self, other: &Self) -> bool {
        if self.size != other.size { return false; }
        if self.xxhash != 0 && other.xxhash != 0 && self.xxhash != other.xxhash {
            return false;
        }
        let mut v: u8 = 0;
        for i in 0..32 { v |= self.blake3[i] ^ other.blake3[i]; }
        v == 0
    }

    pub fn from_bytes(raw: &[u8; 48]) -> Result<Self, FsError> {
        // SAFETY: ChunkFingerprint est #[repr(C)] POD, taille 48 vérifiée.
        Ok(unsafe { core::ptr::read_unaligned(raw.as_ptr() as *const Self) })
    }

    pub fn to_bytes(&self) -> [u8; 48] {
        // SAFETY: ChunkFingerprint est #[repr(C)] POD, taille 48.
        unsafe { core::mem::transmute_copy(self) }
    }
}

fn blake3_simple(data: &[u8]) -> [u8; 32] {
    super::content_hash::CONTENT_HASH.compute(data).blake3
}

fn xxhash64_simple(data: &[u8], seed: u64) -> u64 {
    // Version inline simple pour éviter de re-exporter la fonction privée.
    let mut h = seed.wrapping_add(0x27D4EB2F165667C5u64).wrapping_add(data.len() as u64);
    for chunk in data.chunks(8) {
        let mut word = 0u64;
        for (i, &b) in chunk.iter().enumerate() {
            word |= (b as u64) << (i * 8);
        }
        h ^= word.wrapping_mul(0xC2B2AE3D27D4EB4F);
        h = h.rotate_left(27).wrapping_mul(0x9E3779B185EBCA87).wrapping_add(0x85EBCA77C2B2AE63);
    }
    h ^= h >> 33;
    h  = h.wrapping_mul(0xC2B2AE3D27D4EB4F);
    h ^= h >> 29;
    h  = h.wrapping_mul(0x165667B19E3779F9);
    h ^= h >> 32;
    h
}
