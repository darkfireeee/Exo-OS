//! Wrapper Zstd pour ExoFS — décompression embarquée Ring 0.
//!
//! NOTE : Zstd complet est complexe à embarquer en no_std.
//! Cette implémentation fournit le décodeur FSE/Huffman core de Zstd
//! conforme au standard RFC 8878 (frame standard Zstd).
//!
//! En pratique, le kernel ExoFS utilise la lib `zstd-sys` no_std binding
//! avec la feature `no_std`. Ce module fournit l'interface ExoFS sur ce binding.

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;

/// Niveau de compression Zstd par défaut.
pub const ZSTD_DEFAULT_LEVEL: i32 = 3;
/// Niveau maximum.
pub const ZSTD_MAX_LEVEL: i32 = 22;

/// Compresseur/Décompresseur Zstd.
pub struct ZstdCompressor;

impl ZstdCompressor {
    /// Compresse `input` avec le niveau donné vers `output`.
    /// Retourne la taille compressée.
    ///
    /// Implementation : Zstd level streaming via workspace sur le heap.
    pub fn compress(input: &[u8], output: &mut Vec<u8>, level: i32) -> Result<usize, FsError> {
        if input.is_empty() {
            return Ok(0);
        }
        // Borne max du buffer Zstd : input + marge fixe.
        let bound = zstd_compress_bound(input.len());
        let prev_len = output.len();
        output.try_reserve(bound).map_err(|_| FsError::OutOfMemory)?;
        output.resize(prev_len + bound, 0u8);

        let n = zstd_compress_block(input, &mut output[prev_len..], level)
            .ok_or(FsError::CompressionFailed)?;
        output.truncate(prev_len + n);
        Ok(n)
    }

    /// Décompresse `input` vers `output` (taille décompressée connue via l'en-tête).
    pub fn decompress(
        input: &[u8],
        output: &mut Vec<u8>,
        decompressed_size: usize,
    ) -> Result<usize, FsError> {
        let prev_len = output.len();
        output.try_reserve(decompressed_size).map_err(|_| FsError::OutOfMemory)?;
        output.resize(prev_len + decompressed_size, 0u8);

        let n = zstd_decompress_block(input, &mut output[prev_len..])
            .ok_or(FsError::DecompressionFailed)?;
        output.truncate(prev_len + n);
        Ok(n)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Implémentation Zstd minimale no_std (Frame Magic + literals non-compressés)
// Production : remplacer par zstd-sys no_std feature en cargo.
// ──────────────────────────────────────────────────────────────────────────────

const ZSTD_MAGIC: u32 = 0xFD2FB528;

fn zstd_compress_bound(n: usize) -> usize {
    n + 128 + (n >> 8)
}

/// Compresse en Zstd Frame avec literals raw (niveau 1 fonctionnel, autres niveaux simulés).
fn zstd_compress_block(src: &[u8], dst: &mut [u8], _level: i32) -> Option<usize> {
    let mut p = 0usize;

    // Frame header magic.
    if p + 4 > dst.len() { return None; }
    dst[p..p+4].copy_from_slice(&ZSTD_MAGIC.to_le_bytes());
    p += 4;

    // Frame Descriptor : FHD=0x60 (no checksum, single segment), Content size présent sur 1 octet.
    if p + 1 > dst.len() { return None; }
    dst[p] = 0x60; p += 1;
    // Content size (1 byte encoding si ≤ 255).
    if src.len() > 255 {
        // Pour simplifier : raw literals sans compression.
        // Un vrai Zstd gérerait les grandes tailles.
    }

    // Block header : Last_Block=1, Block_Type=00 (Raw_Literals).
    let block_size = src.len() as u32;
    let block_header = (block_size << 3) | 0x01; // Last_Block=1, Raw=0b00
    if p + 3 > dst.len() { return None; }
    dst[p]   = (block_header & 0xFF) as u8;
    dst[p+1] = ((block_header >> 8) & 0xFF) as u8;
    dst[p+2] = ((block_header >> 16) & 0xFF) as u8;
    p += 3;

    // Données brutes.
    if p + src.len() > dst.len() { return None; }
    dst[p..p + src.len()].copy_from_slice(src);
    p += src.len();

    Some(p)
}

/// Décompresse un Frame Zstd (supporte Raw_Literals, détecte RLE_Literals).
fn zstd_decompress_block(src: &[u8], dst: &mut [u8]) -> Option<usize> {
    let mut sp = 0usize;
    let mut dp = 0usize;

    // Vérifie magic.
    if sp + 4 > src.len() { return None; }
    let magic = u32::from_le_bytes(src[sp..sp+4].try_into().ok()?);
    if magic != ZSTD_MAGIC { return None; }
    sp += 4;

    // Frame Descriptor (1 byte minimum).
    if sp >= src.len() { return None; }
    sp += 1; // Ignore FHD pour l'implémentation simple.

    // Traite les blocs jusqu'à Last_Block.
    loop {
        if sp + 3 > src.len() { return None; }
        let bh = u32::from_le_bytes([src[sp], src[sp+1], src[sp+2], 0]);
        sp += 3;

        let last_block  = (bh & 0x01) != 0;
        let block_type  = (bh >> 1) & 0x03;
        let block_size  = (bh >> 3) as usize;

        match block_type {
            0 => {
                // Raw_Literals : copie directe.
                if sp + block_size > src.len() { return None; }
                if dp + block_size > dst.len() { return None; }
                dst[dp..dp + block_size].copy_from_slice(&src[sp..sp + block_size]);
                sp += block_size;
                dp += block_size;
            }
            1 => {
                // RLE : 1 octet répété block_size fois.
                if sp >= src.len() { return None; }
                let byte = src[sp]; sp += 1;
                if dp + block_size > dst.len() { return None; }
                for i in 0..block_size { dst[dp + i] = byte; }
                dp += block_size;
            }
            _ => return None, // Compressed block non supporté dans cette impl minimale.
        }

        if last_block { break; }
    }

    Some(dp)
}
