//! CdcChunker — Content-Defined Chunking (Rabin-Karp rolling hash) (no_std).
//!
//! Implémente le CDC avec fenêtre glissante Rabin-Karp pour trouver
//! les limites de chunks indépendantes du positionnement.

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use super::chunking::{Chunker, DedupChunk, ChunkBoundary};
use super::chunk_fingerprint::{ChunkFingerprint, FingerprintAlgorithm};

/// Paramètres CDC.
pub const CDC_MIN_SIZE:  usize = 2048;   // 2 KiB minimum.
pub const CDC_AVG_SIZE:  usize = 8192;   // 8 KiB cible.
pub const CDC_MAX_SIZE:  usize = 65536;  // 64 KiB maximum.
/// Masque déclenchant une coupure (log2(avg_size) bits à 0).
const CDC_MASK: u64 = (CDC_AVG_SIZE as u64) - 1; // = 0x1FFF

/// Base et module du polynôme de Rabin.
const RABIN_BASE:  u64 = 257;
const RABIN_MOD:   u64 = (1u64 << 61) - 1; // Mersenne premier M61.
/// Taille de la fenêtre glissante.
const WINDOW_SIZE: usize = 64;
/// Valeur de base élevée à la puissance WINDOW_SIZE dans M61.
const RABIN_POW:   u64 = pow_mod(RABIN_BASE, WINDOW_SIZE as u64, RABIN_MOD);

const fn pow_mod(mut base: u64, mut exp: u64, modulus: u64) -> u64 {
    let mut result = 1u64;
    base %= modulus;
    while exp > 0 {
        if exp & 1 == 1 { result = mulmod61(result, base); }
        exp >>= 1;
        base = mulmod61(base, base);
    }
    result
}

const fn mulmod61(a: u64, b: u64) -> u64 {
    // Multiplication modulaire M61 (adapté pour const fn).
    let (hi, lo) = {
        let a128 = a as u128;
        let b128 = b as u128;
        let p    = a128 * b128;
        ((p >> 61) as u64, (p & 0x1FFF_FFFF_FFFF_FFFF) as u64)
    };
    let r = hi + lo;
    if r >= RABIN_MOD { r - RABIN_MOD } else { r }
}

/// Découpeur CDC (Content-Defined Chunking).
pub struct CdcChunker {
    min_size: usize,
    max_size: usize,
    mask:     u64,
    algo:     FingerprintAlgorithm,
}

impl CdcChunker {
    pub fn new(min_size: usize, max_size: usize, avg_size: usize) -> Self {
        let mask = (avg_size as u64).next_power_of_two() - 1;
        Self {
            min_size: min_size.max(512),
            max_size: max_size.min(65536),
            mask,
            algo: FingerprintAlgorithm::Blake3Xxh64,
        }
    }

    pub fn default_8k() -> Self {
        Self::new(CDC_MIN_SIZE, CDC_MAX_SIZE, CDC_AVG_SIZE)
    }

    fn find_boundary(&self, data: &[u8], start: usize) -> usize {
        if data.len() <= start + self.min_size {
            return data.len();
        }

        let mut hash: u64 = 0;
        let mut window = [0u8; WINDOW_SIZE];
        let mut win_pos = 0usize;

        // Charge la fenêtre initiale.
        let win_start = start;
        let win_end   = (start + WINDOW_SIZE).min(data.len());
        for (i, &b) in data[win_start..win_end].iter().enumerate() {
            window[i % WINDOW_SIZE] = b;
            hash = mulmod61(hash, RABIN_BASE).wrapping_add(b as u64);
            hash %= RABIN_MOD;
        }

        let scan_start = start + self.min_size;
        let scan_end   = (start + self.max_size).min(data.len());

        for i in scan_start..scan_end {
            // Retire le byte sortant.
            let out_byte = window[win_pos % WINDOW_SIZE];
            hash = hash.wrapping_add(RABIN_MOD)
                       .wrapping_sub(mulmod61(out_byte as u64, RABIN_POW) % RABIN_MOD)
                       % RABIN_MOD;
            // Ajoute le byte entrant.
            let in_byte = data[i];
            hash = mulmod61(hash, RABIN_BASE).wrapping_add(in_byte as u64) % RABIN_MOD;
            window[win_pos % WINDOW_SIZE] = in_byte;
            win_pos += 1;

            // Vérifier le marqueur de coupure.
            if hash & self.mask == 0 {
                return i + 1;
            }
        }

        // Aucune coupure trouvée → retourner max_size.
        scan_end
    }
}

impl Chunker for CdcChunker {
    fn chunk(&self, data: &[u8]) -> Result<Vec<DedupChunk>, FsError> {
        let est_chunks = (data.len() / CDC_AVG_SIZE).max(1) * 2;
        let mut out = Vec::new();
        out.try_reserve(est_chunks).map_err(|_| FsError::OutOfMemory)?;

        let mut pos = 0usize;
        let mut chunk_offset: u64 = 0;

        while pos < data.len() {
            let boundary = self.find_boundary(data, pos);
            let chunk_data = &data[pos..boundary];
            let fp = ChunkFingerprint::compute(chunk_data, self.algo);
            out.push(DedupChunk {
                boundary: ChunkBoundary {
                    offset: chunk_offset,
                    length: (boundary - pos) as u32,
                },
                fingerprint: fp,
            });
            chunk_offset = chunk_offset.checked_add((boundary - pos) as u64)
                .ok_or(FsError::Overflow)?;
            pos = boundary;
        }

        if out.is_empty() {
            out.push(DedupChunk {
                boundary: ChunkBoundary { offset: 0, length: 0 },
                fingerprint: ChunkFingerprint::compute(&[], self.algo),
            });
        }

        Ok(out)
    }

    fn min_size(&self) -> u32 { self.min_size as u32 }
    fn max_size(&self) -> u32 { self.max_size as u32 }
}
