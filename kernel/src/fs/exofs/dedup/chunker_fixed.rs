//! FixedChunker — découpage en chunks de taille fixe (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use super::chunking::{Chunker, DedupChunk, ChunkBoundary};
use super::chunk_fingerprint::{ChunkFingerprint, FingerprintAlgorithm};

/// Taille par défaut d'un chunk fixe (4 KiB).
pub const FIXED_CHUNK_DEFAULT: u32 = 4096;

/// Découpeur à taille fixe.
pub struct FixedChunker {
    chunk_size: u32,
    algo:       FingerprintAlgorithm,
}

impl FixedChunker {
    pub fn new(chunk_size: u32) -> Self {
        Self {
            chunk_size: chunk_size.max(512).min(65536),
            algo: FingerprintAlgorithm::Blake3Xxh64,
        }
    }

    pub fn default_4k() -> Self {
        Self::new(FIXED_CHUNK_DEFAULT)
    }
}

impl Chunker for FixedChunker {
    fn chunk(&self, data: &[u8]) -> Result<Vec<DedupChunk>, FsError> {
        let sz = self.chunk_size as usize;
        let n_chunks = (data.len() + sz - 1).max(1) / sz.max(1);
        let mut out = Vec::new();
        out.try_reserve(n_chunks).map_err(|_| FsError::OutOfMemory)?;

        let mut offset: u64 = 0;
        let mut i = 0;
        while i < data.len() {
            let end = (i + sz).min(data.len());
            let chunk_data = &data[i..end];
            let fp = ChunkFingerprint::compute(chunk_data, self.algo);
            out.push(DedupChunk {
                boundary: ChunkBoundary { offset, length: (end - i) as u32 },
                fingerprint: fp,
            });
            offset = offset.checked_add((end - i) as u64).ok_or(FsError::Overflow)?;
            i = end;
        }

        if out.is_empty() {
            // Blob vide — retourne un chunk de taille zéro.
            out.push(DedupChunk {
                boundary: ChunkBoundary { offset: 0, length: 0 },
                fingerprint: ChunkFingerprint::compute(&[], self.algo),
            });
        }

        Ok(out)
    }

    fn min_size(&self) -> u32 { self.chunk_size }
    fn max_size(&self) -> u32 { self.chunk_size }
}
