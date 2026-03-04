//! Wrapper LZ4 pour ExoFS — via crate lz4_flex (block mode, no_std).
//!
//! RÈGLE CRYPTO-CRATES : JAMAIS d'implémentation from scratch.
//! Crate : lz4_flex v0.11.x, default-features = false
//!   - Implémentation LZ4 block format pure Rust, no_std + alloc
//!   - Conforme au format LZ4 block officiel (lz4.org)
//!   - Pas de magic number / frame header — compressé brut (block mode)
//!
//! RÈGLE OOM-02   : try_reserve avant tout push.
//! RÈGLE ARITH-02 : arithmétique checked/saturating.
//! RÈGLE RECUR-01 : aucune récursivité — toutes les boucles sont dans lz4_flex.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

use lz4_flex::block::{compress_into, decompress_into, get_maximum_output_size};

/// Compresseur LZ4 (block mode) — wrapper crate lz4_flex.
pub struct Lz4Compressor;

impl Lz4Compressor {
    /// Compresse `input` et ajoute le résultat dans `output`.
    /// Retourne la taille compressée ajoutée.
    ///
    /// OOM-02 : try_reserve avant tout resize.
    pub fn compress(input: &[u8], output: &mut Vec<u8>) -> ExofsResult<usize> {
        if input.is_empty() {
            return Ok(0);
        }
        let bound    = get_maximum_output_size(input.len());
        let prev_len = output.len();
        output.try_reserve(bound).map_err(|_| ExofsError::NoMemory)?;
        output.resize(prev_len + bound, 0u8);

        let n = compress_into(input, &mut output[prev_len..])
            .map_err(|_| ExofsError::InternalError)?;

        output.truncate(prev_len + n);
        Ok(n)
    }

    /// Décompresse `input` vers `output` (taille de sortie connue).
    ///
    /// OOM-02 : try_reserve avant tout resize.
    pub fn decompress(
        input:             &[u8],
        output:            &mut Vec<u8>,
        decompressed_size: usize,
    ) -> ExofsResult<usize> {
        let prev_len = output.len();
        output
            .try_reserve(decompressed_size)
            .map_err(|_| ExofsError::NoMemory)?;
        output.resize(prev_len + decompressed_size, 0u8);

        let n = decompress_into(input, &mut output[prev_len..])
            .map_err(|_| ExofsError::CorruptedStructure)?;

        output.truncate(prev_len + n);
        Ok(n)
    }

    /// Retourne la borne supérieure du buffer de sortie pour une entrée de taille `n`.
    #[inline]
    pub fn compress_bound(n: usize) -> usize {
        get_maximum_output_size(n)
    }

    /// Compresse et retourne un nouveau Vec.
    ///
    /// OOM-02 : try_reserve via Self::compress.
    pub fn compress_to_vec(input: &[u8]) -> ExofsResult<Vec<u8>> {
        let mut out = Vec::new();
        Self::compress(input, &mut out)?;
        Ok(out)
    }

    /// Décompresse et retourne un nouveau Vec.
    ///
    /// OOM-02 : try_reserve via Self::decompress.
    pub fn decompress_to_vec(input: &[u8], decompressed_size: usize) -> ExofsResult<Vec<u8>> {
        let mut out = Vec::new();
        Self::decompress(input, &mut out, decompressed_size)?;
        Ok(out)
    }
}
