//! checksum_reader.rs — Lecture et vérification de blocs avec checksum (no_std).
//! RÈGLE 8 : vérification magic EN PREMIER.

use crate::fs::exofs::core::FsError;
use super::checksum_writer::{ChecksumBlockHeader, CHECKSUM_MAGIC};

/// Extrait et vérifie les données d'un bloc checksumé.
/// Retourne un slice vers les données si valid.
pub fn read_and_verify<'a>(buf: &'a [u8]) -> Result<&'a [u8], FsError> {
    if buf.len() < 16 {
        return Err(FsError::InvalidData);
    }

    // RÈGLE 8 : magic EN PREMIER.
    let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap_or([0; 4]));
    if magic != CHECKSUM_MAGIC {
        return Err(FsError::InvalidMagic);
    }

    let data_len = u32::from_le_bytes(buf[4..8].try_into().unwrap_or([0; 4])) as usize;
    let stored_hash = u64::from_le_bytes(buf[8..16].try_into().unwrap_or([0; 8]));

    if buf.len() < 16 + data_len {
        return Err(FsError::InvalidData);
    }

    let data = &buf[16..16 + data_len];

    // Vérification du checksum.
    let computed = xxhash64_simple(data);
    if computed != stored_hash {
        return Err(FsError::IntegrityCheckFailed);
    }

    Ok(data)
}

fn xxhash64_simple(data: &[u8]) -> u64 {
    const PRIME1: u64 = 0x9E37_79B1_85EB_CA87;
    const PRIME2: u64 = 0xC2B2_AE3D_27D4_EB4F;
    let mut h = 0x27D4_EB2F_165667C5u64;
    for chunk in data.chunks(8) {
        let mut val = 0u64;
        for (i, &b) in chunk.iter().enumerate() {
            val |= (b as u64) << (i * 8);
        }
        h ^= val.wrapping_mul(PRIME1);
        h  = h.rotate_left(27).wrapping_mul(PRIME2).wrapping_add(0x9FB21C651E98DF25);
    }
    h ^= h >> 33;
    h  = h.wrapping_mul(0xFF51AFD7ED558CCD);
    h ^= h >> 33;
    h  = h.wrapping_mul(0xC4CEB9FE1A85EC53);
    h ^= h >> 33;
    h
}
