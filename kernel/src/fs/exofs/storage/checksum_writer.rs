//! checksum_writer.rs — Émission de blocs avec checksum XXHash64 (no_std).

use crate::fs::exofs::core::FsError;

const CHECKSUM_MAGIC: u32 = 0x43484B53; // "CHKS"

/// En-tête de bloc avec checksum.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ChecksumBlockHeader {
    pub magic:    u32,
    pub data_len: u32,
    pub xxhash:   u64,
}

const _: () = assert!(core::mem::size_of::<ChecksumBlockHeader>() == 16);

/// Calcule XXHash64 simplifié (FNV-1a 64 bits pour les blocs de données).
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

/// Sérialise les données avec leur en-tête checksum dans `out`.
/// Retourne le nombre d'octets écrits (header + data).
pub fn write_with_checksum(data: &[u8], out: &mut alloc::vec::Vec<u8>) -> Result<usize, FsError> {
    let checksum = xxhash64_simple(data);
    let header   = ChecksumBlockHeader {
        magic:    CHECKSUM_MAGIC,
        data_len: data.len() as u32,
        xxhash:   checksum,
    };

    let header_bytes: &[u8] = unsafe {
        // SAFETY: ChecksumBlockHeader est repr(C) de taille fixe 16B.
        core::slice::from_raw_parts(
            &header as *const _ as *const u8,
            core::mem::size_of::<ChecksumBlockHeader>(),
        )
    };

    out.try_reserve(16 + data.len()).map_err(|_| FsError::OutOfMemory)?;
    out.extend_from_slice(header_bytes);
    out.extend_from_slice(data);
    Ok(16 + data.len())
}
