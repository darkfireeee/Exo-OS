//! Lecteur de blob compressé ExoFS — décompression transparente.
//!
//! RÈGLE 8  : vérifie le magic de l'en-tête EN PREMIER.
//! RÈGLE 14 : checked_add pour les offsets.

use alloc::vec::Vec;
use crate::fs::exofs::compress::algorithm::CompressionAlgorithm;
use crate::fs::exofs::compress::compress_header::{CompressionHeader, COMPRESSION_MAGIC};
use crate::fs::exofs::compress::lz4_wrapper::Lz4Compressor;
use crate::fs::exofs::compress::zstd_wrapper::ZstdCompressor;
use crate::fs::exofs::core::FsError;

/// Décompresse un blob qui peut être brut ou compressé.
///
/// Lit d'abord l'en-tête `CompressionHeader` (32 bytes).
/// RÈGLE 8 : magic EN PREMIER.
pub struct DecompressReader;

impl DecompressReader {
    /// Décompresse `payload` (en-tête + données) et retourne les données originales.
    pub fn decompress(payload: &[u8]) -> Result<Vec<u8>, FsError> {
        const HEADER_SIZE: usize = 32;

        if payload.len() < HEADER_SIZE {
            return Err(FsError::CorruptData);
        }

        // RÈGLE 8 : magic EN PREMIER.
        let magic = u32::from_le_bytes(payload[0..4].try_into().unwrap());
        if magic != COMPRESSION_MAGIC {
            // Données non compressées / pas d'en-tête ExoFS → retourne brut.
            let mut out = Vec::new();
            out.try_reserve(payload.len()).map_err(|_| FsError::OutOfMemory)?;
            out.extend_from_slice(payload);
            return Ok(out);
        }

        let header = CompressionHeader::from_bytes(payload)?;
        let data = &payload[HEADER_SIZE..];
        let decompressed_size = header.uncompressed_size as usize;

        match header.algorithm() {
            CompressionAlgorithm::None => {
                let mut out = Vec::new();
                out.try_reserve(data.len()).map_err(|_| FsError::OutOfMemory)?;
                out.extend_from_slice(data);
                Ok(out)
            }
            CompressionAlgorithm::Lz4 => {
                let mut out = Vec::new();
                Lz4Compressor::decompress(data, &mut out, decompressed_size)?;
                Ok(out)
            }
            CompressionAlgorithm::Zstd => {
                let mut out = Vec::new();
                ZstdCompressor::decompress(data, &mut out, decompressed_size)?;
                Ok(out)
            }
        }
    }

    /// Retourne `true` si le payload contient un en-tête de compression ExoFS.
    pub fn is_compressed(payload: &[u8]) -> bool {
        payload.len() >= 4
            && u32::from_le_bytes(payload[0..4].try_into().unwrap_or([0; 4]))
                == COMPRESSION_MAGIC
    }
}
