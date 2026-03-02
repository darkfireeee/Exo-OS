//! compression_writer.rs — Écriture compressée de blobs storage ExoFS (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use crate::fs::exofs::compress::lz4_wrapper::lz4_compress_to_vec;
use crate::fs::exofs::compress::zstd_wrapper::zstd_compress_to_vec;
use super::compression_choice::{CompressionAlgorithm, choose_algorithm, estimate_entropy};

const COMP_BLOCK_MAGIC: u32 = 0x434F4D50; // "COMP"

/// En-tête de bloc compressé on-disk.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CompressedBlockHeader {
    pub magic:         u32,
    pub algorithm:     u8,
    pub _pad:          [u8; 3],
    pub raw_len:       u32,
    pub compressed_len: u32,
    pub checksum:      u32,
}

const _: () = assert!(core::mem::size_of::<CompressedBlockHeader>() == 20);

/// Résultat de compression.
#[derive(Clone, Debug)]
pub struct CompressionWriteResult {
    pub algorithm:    CompressionAlgorithm,
    pub raw_len:      u32,
    pub written_len:  u32,   // Header + compressed data.
    pub ratio_pct:    u8,    // Taux de compression (0=aucun gain, 100=parfait).
}

/// Compresse `data` et sérialise dans `out`.
pub fn write_compressed(data: &[u8], out: &mut Vec<u8>) -> Result<CompressionWriteResult, FsError> {
    let entropy  = estimate_entropy(data);
    let algo     = choose_algorithm(data.len(), entropy);

    let compressed: Vec<u8> = match algo {
        CompressionAlgorithm::None => {
            let mut v = Vec::new();
            v.try_reserve(data.len()).map_err(|_| FsError::OutOfMemory)?;
            v.extend_from_slice(data);
            v
        }
        CompressionAlgorithm::Lz4  => lz4_compress_to_vec(data).map_err(|_| FsError::InvalidData)?,
        CompressionAlgorithm::Zstd => zstd_compress_to_vec(data, 3).map_err(|_| FsError::InvalidData)?,
    };

    let checksum = crc32_data(&compressed);
    let header   = CompressedBlockHeader {
        magic:          COMP_BLOCK_MAGIC,
        algorithm:      algo as u8,
        _pad:           [0; 3],
        raw_len:        data.len() as u32,
        compressed_len: compressed.len() as u32,
        checksum,
    };

    let hdr_size = core::mem::size_of::<CompressedBlockHeader>();
    out.try_reserve(hdr_size + compressed.len()).map_err(|_| FsError::OutOfMemory)?;

    // SAFETY: CompressedBlockHeader est repr(C) 20B.
    let hdr_bytes: &[u8] = unsafe {
        core::slice::from_raw_parts(
            &header as *const _ as *const u8,
            hdr_size,
        )
    };
    out.extend_from_slice(hdr_bytes);
    out.extend_from_slice(&compressed);

    let ratio_pct = if data.is_empty() { 0 } else {
        let saved = data.len().saturating_sub(compressed.len());
        ((saved * 100) / data.len()) as u8
    };

    Ok(CompressionWriteResult {
        algorithm:   algo,
        raw_len:     data.len() as u32,
        written_len: (hdr_size + compressed.len()) as u32,
        ratio_pct,
    })
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
