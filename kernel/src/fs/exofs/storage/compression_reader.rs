//! compression_reader.rs — Lecture décompressée de blobs storage ExoFS (no_std).
//! RÈGLE 8 : vérification magic EN PREMIER.

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use crate::fs::exofs::compress::lz4_wrapper::lz4_decompress_to_vec;
use crate::fs::exofs::compress::zstd_wrapper::zstd_decompress_to_vec;
use super::compression_writer::{CompressedBlockHeader, COMP_BLOCK_MAGIC};
use super::compression_choice::CompressionAlgorithm;

/// Désérialise et décompresse un bloc compressé.
/// RÈGLE 8 : magic vérifié EN PREMIER.
pub fn read_compressed(buf: &[u8]) -> Result<Vec<u8>, FsError> {
    let hdr_size = core::mem::size_of::<CompressedBlockHeader>();
    if buf.len() < hdr_size {
        return Err(FsError::InvalidData);
    }

    // RÈGLE 8 : magic EN PREMIER.
    let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap_or([0; 4]));
    if magic != COMP_BLOCK_MAGIC {
        return Err(FsError::InvalidMagic);
    }

    // SAFETY: CompressedBlockHeader est repr(C) 20B, buf >= 20.
    let header: CompressedBlockHeader = unsafe {
        core::mem::transmute_copy(&*(buf.as_ptr() as *const [u8; 20]))
    };

    let comp_len = header.compressed_len as usize;
    let raw_len  = header.raw_len as usize;
    let end = hdr_size.checked_add(comp_len).ok_or(FsError::Overflow)?;
    if buf.len() < end {
        return Err(FsError::InvalidData);
    }

    let compressed = &buf[hdr_size..end];

    // Vérifier checksum.
    let computed = crc32_data(compressed);
    if computed != header.checksum {
        return Err(FsError::IntegrityCheckFailed);
    }

    let algo = CompressionAlgorithm::from_u8(header.algorithm);
    let decompressed: Vec<u8> = match algo {
        CompressionAlgorithm::None => {
            let mut v = Vec::new();
            v.try_reserve(comp_len).map_err(|_| FsError::OutOfMemory)?;
            v.extend_from_slice(compressed);
            v
        }
        CompressionAlgorithm::Lz4 => {
            lz4_decompress_to_vec(compressed, raw_len).map_err(|_| FsError::InvalidData)?
        }
        CompressionAlgorithm::Zstd => {
            zstd_decompress_to_vec(compressed, raw_len).map_err(|_| FsError::InvalidData)?
        }
    };

    Ok(decompressed)
}

fn crc32_data(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (0u32.wrapping_sub(crc & 1)) as u32;
            crc = (crc >> 1) ^ (0x82F6_3B78 & mask);
        }
    }
    !crc
}
