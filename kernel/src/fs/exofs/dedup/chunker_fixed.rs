//! FixedChunker — découpage à taille fixe pour la déduplication (no_std).
//!
//! Divise un flux de données en chunks de taille constante, avec gestion
//! du dernier chunk (potentiellement plus petit).
//!
//! RECUR-01 : aucune récursion — boucle while.
//! OOM-02   : try_reserve sur tous les Vec.
//! ARITH-02 : checked / saturating / wrapping sur toutes les arithmétiques.

use super::chunk_fingerprint::{blake3_hash, fnv1a64, ChunkFingerprint};
use super::chunking::{ChunkBoundary, ChunkStats, Chunker, DedupChunk, CHUNK_MAX_PER_BLOB};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille de chunk par défaut pour le chunker fixe (4 KiB).
pub const FIXED_DEFAULT_CHUNK_SIZE: usize = 4096;
/// Taille minimale acceptée.
pub const FIXED_MIN_CHUNK_SIZE: usize = 1;
/// Taille maximale acceptée (64 KiB).
pub const FIXED_MAX_CHUNK_SIZE: usize = 65536;

// ─────────────────────────────────────────────────────────────────────────────
// FixedChunkerConfig
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration du chunker fixe.
#[derive(Debug, Clone, Copy)]
pub struct FixedChunkerConfig {
    /// Taille de chunk en octets.
    pub chunk_size: usize,
    /// Inclure les données dans chaque DedupChunk.
    pub include_data: bool,
    /// Aligner les chunks sur `chunk_size` (padding du dernier avec 0).
    pub pad_last_chunk: bool,
    /// Calculer le fast_hash (FNV-1a) en supplément du blake3.
    pub compute_fast_hash: bool,
}

impl FixedChunkerConfig {
    /// Configuration par défaut (4 KiB, avec données, sans padding).
    pub fn default() -> Self {
        Self {
            chunk_size: FIXED_DEFAULT_CHUNK_SIZE,
            include_data: true,
            pad_last_chunk: false,
            compute_fast_hash: true,
        }
    }

    /// Valide la configuration.
    pub fn validate(&self) -> ExofsResult<()> {
        if self.chunk_size < FIXED_MIN_CHUNK_SIZE || self.chunk_size > FIXED_MAX_CHUNK_SIZE {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FixedChunker
// ─────────────────────────────────────────────────────────────────────────────

/// Découpeur à taille fixe.
pub struct FixedChunker {
    config: FixedChunkerConfig,
}

impl FixedChunker {
    /// Crée un chunker avec la taille spécifiée.
    pub fn new(chunk_size: usize) -> ExofsResult<Self> {
        let config = FixedChunkerConfig {
            chunk_size,
            ..FixedChunkerConfig::default()
        };
        config.validate()?;
        Ok(Self { config })
    }

    /// Crée un chunker avec configuration complète.
    pub fn with_config(config: FixedChunkerConfig) -> ExofsResult<Self> {
        config.validate()?;
        Ok(Self { config })
    }

    /// Crée un chunker par défaut (4 KiB).
    pub fn default_chunker() -> Self {
        Self {
            config: FixedChunkerConfig::default(),
        }
    }

    /// Retourne la taille de chunk configurée.
    pub fn chunk_size(&self) -> usize {
        self.config.chunk_size
    }

    /// Calcule le nombre de chunks pour `data_len` octets.
    ///
    /// ARITH-02 : checked_add pour éviter débordement.
    pub fn chunk_count_for(&self, data_len: usize) -> ExofsResult<usize> {
        if data_len == 0 {
            return Ok(0);
        }
        let cs = self.config.chunk_size;
        // ceil(data_len / cs)
        let count = data_len
            .checked_add(cs.wrapping_sub(1))
            .map(|v| v / cs)
            .ok_or(ExofsError::OffsetOverflow)?;
        if count > CHUNK_MAX_PER_BLOB {
            return Err(ExofsError::OffsetOverflow);
        }
        Ok(count)
    }

    /// Construit le buffer paddé du dernier chunk.
    ///
    /// OOM-02 : try_reserve.
    fn pad_chunk(&self, data: &[u8]) -> ExofsResult<Vec<u8>> {
        let cs = self.config.chunk_size;
        let mut buf: Vec<u8> = Vec::new();
        buf.try_reserve(cs).map_err(|_| ExofsError::NoMemory)?;
        buf.extend_from_slice(data);
        buf.resize(cs, 0u8);
        Ok(buf)
    }

    /// Découpe `data` en frontières (pas de calcul d'empreinte).
    ///
    /// RECUR-01 : boucle while.
    pub fn compute_boundaries_only(&self, data: &[u8]) -> ExofsResult<Vec<ChunkBoundary>> {
        let total = data.len();
        if total == 0 {
            return Ok(Vec::new());
        }
        let cs = self.config.chunk_size;
        let count = self.chunk_count_for(total)?;
        let mut bounds: Vec<ChunkBoundary> = Vec::new();
        bounds
            .try_reserve(count)
            .map_err(|_| ExofsError::NoMemory)?;
        let mut offset: usize = 0;
        while offset < total {
            let len = (total - offset).min(cs);
            let b = ChunkBoundary::new(offset as u64, len as u32)?;
            bounds.push(b);
            offset = offset.saturating_add(len);
        }
        Ok(bounds)
    }
}

impl Chunker for FixedChunker {
    /// Découpe `data` en chunks.
    ///
    /// RECUR-01 : boucle while.
    /// OOM-02   : try_reserve.
    fn chunk(&self, data: &[u8]) -> ExofsResult<Vec<DedupChunk>> {
        let total = data.len();
        if total == 0 {
            return Ok(Vec::new());
        }
        let cs = self.config.chunk_size;
        let count = self.chunk_count_for(total)?;
        let mut chunks: Vec<DedupChunk> = Vec::new();
        chunks
            .try_reserve(count)
            .map_err(|_| ExofsError::NoMemory)?;
        let mut offset: usize = 0;
        while offset < total {
            let end = (offset.saturating_add(cs)).min(total);
            let raw = &data[offset..end];
            let len = raw.len();
            let blake3 = if self.config.pad_last_chunk && len < cs {
                let padded = self.pad_chunk(raw)?;
                blake3_hash(&padded)
            } else {
                blake3_hash(raw)
            };
            let fast_hash = if self.config.compute_fast_hash {
                fnv1a64(raw)
            } else {
                0
            };
            let boundary = ChunkBoundary::new(offset as u64, len as u32)?;
            let chunk = if self.config.include_data {
                DedupChunk::new(boundary, blake3, fast_hash, raw)?
            } else {
                DedupChunk::metadata_only(boundary, blake3, fast_hash)
            };
            chunks.push(chunk);
            offset = offset.saturating_add(len);
        }
        Ok(chunks)
    }

    fn boundaries(&self, data: &[u8]) -> ExofsResult<Vec<ChunkBoundary>> {
        self.compute_boundaries_only(data)
    }

    fn min_chunk_size(&self) -> usize {
        self.config.chunk_size
    }
    fn max_chunk_size(&self) -> usize {
        self.config.chunk_size
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchFixedChunker — chunker batch sur plusieurs blobs
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat du découpage d'un blob.
#[derive(Debug)]
pub struct FixedChunkResult {
    pub blob_idx: usize,
    pub chunks: Vec<DedupChunk>,
    pub stats: ChunkStats,
}

/// Chunker batch permettant de traiter plusieurs blobs d'un coup.
pub struct BatchFixedChunker {
    inner: FixedChunker,
}

impl BatchFixedChunker {
    pub fn new(chunk_size: usize) -> ExofsResult<Self> {
        Ok(Self {
            inner: FixedChunker::new(chunk_size)?,
        })
    }

    /// Découpe un batch de blobs.
    ///
    /// OOM-02 : try_reserve.
    pub fn chunk_batch(&self, blobs: &[&[u8]]) -> ExofsResult<Vec<FixedChunkResult>> {
        let mut results: Vec<FixedChunkResult> = Vec::new();
        results
            .try_reserve(blobs.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for (idx, &blob) in blobs.iter().enumerate() {
            let chunks = self.inner.chunk(blob)?;
            let stats = ChunkStats::from_chunks(&chunks)?;
            results.push(FixedChunkResult {
                blob_idx: idx,
                chunks,
                stats,
            });
        }
        Ok(results)
    }

    /// Retourne les empreintes de tous les chunks de tous les blobs.
    ///
    /// OOM-02 : try_reserve.
    pub fn all_fingerprints(&self, blobs: &[&[u8]]) -> ExofsResult<Vec<ChunkFingerprint>> {
        let mut fps: Vec<ChunkFingerprint> = Vec::new();
        for &blob in blobs {
            let chunks = self.inner.chunk(blob)?;
            fps.try_reserve(chunks.len())
                .map_err(|_| ExofsError::NoMemory)?;
            for c in chunks {
                fps.push(ChunkFingerprint::from_parts(c.blake3, c.fast_hash));
            }
        }
        Ok(fps)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn chunker() -> FixedChunker {
        FixedChunker::new(4).unwrap()
    }

    #[test]
    fn test_chunk_count() {
        let c = FixedChunker::new(4).unwrap();
        assert_eq!(c.chunk_count_for(8).unwrap(), 2);
        assert_eq!(c.chunk_count_for(9).unwrap(), 3);
        assert_eq!(c.chunk_count_for(0).unwrap(), 0);
    }

    #[test]
    fn test_chunk_produces_correct_count() {
        let c = chunker();
        let data = &[0u8; 10];
        let chunks = c.chunk(data).unwrap();
        assert_eq!(chunks.len(), 3); // 4+4+2
    }

    #[test]
    fn test_last_chunk_undersized() {
        let c = FixedChunker::new(8).unwrap();
        let data = &[1u8; 10];
        let chunks = c.chunk(data).unwrap();
        assert_eq!(chunks.last().unwrap().len(), 2);
    }

    #[test]
    fn test_chunk_empty_data() {
        let c = chunker();
        assert!(c.chunk(&[]).unwrap().is_empty());
    }

    #[test]
    fn test_chunk_data_preserved() {
        let c = FixedChunker::new(3).unwrap();
        let data = &[1u8, 2, 3, 4, 5, 6];
        let chunks = c.chunk(data).unwrap();
        assert_eq!(chunks[0].data.as_slice(), &[1u8, 2, 3]);
        assert_eq!(chunks[1].data.as_slice(), &[4u8, 5, 6]);
    }

    #[test]
    fn test_boundaries_match_chunk() {
        let c = chunker();
        let data = &[0u8; 12];
        let bounds = c.boundaries(data).unwrap();
        let chunks = c.chunk(data).unwrap();
        assert_eq!(bounds.len(), chunks.len());
    }

    #[test]
    fn test_chunk_blake3_deterministic() {
        let c = chunker();
        let data = &[0xABu8; 8];
        let c1 = c.chunk(data).unwrap();
        let c2 = c.chunk(data).unwrap();
        assert_eq!(c1[0].blake3, c2[0].blake3);
    }

    #[test]
    fn test_invalid_chunk_size_zero() {
        assert!(FixedChunker::new(0).is_err());
    }

    #[test]
    fn test_invalid_chunk_size_too_large() {
        assert!(FixedChunker::new(FIXED_MAX_CHUNK_SIZE + 1).is_err());
    }

    #[test]
    fn test_batch_chunker() {
        let bc = BatchFixedChunker::new(4).unwrap();
        let b1 = &[0u8; 8];
        let b2 = &[1u8; 12];
        let r = bc.chunk_batch(&[b1, b2]).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].chunks.len(), 2);
        assert_eq!(r[1].chunks.len(), 3);
    }

    #[test]
    fn test_batch_all_fingerprints() {
        let bc = BatchFixedChunker::new(4).unwrap();
        let fps = bc.all_fingerprints(&[&[0u8; 8]]).unwrap();
        assert_eq!(fps.len(), 2);
    }

    #[test]
    fn test_no_data_mode() {
        let cfg = FixedChunkerConfig {
            chunk_size: 4,
            include_data: false,
            pad_last_chunk: false,
            compute_fast_hash: false,
        };
        let c = FixedChunker::with_config(cfg).unwrap();
        let ch = c.chunk(&[0u8; 8]).unwrap();
        assert!(!ch[0].has_data());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SlidingWindowChunker — chunker fixe avec fenêtre glissante de détection
// ─────────────────────────────────────────────────────────────────────────────

/// Chunker fixe avec fenêtre glissante pour détecter les duplicata partiels.
///
/// Produit des chunks chevauchants (overlap) pour améliorer la détection
/// de similarité dans les blobs légèrement modifiés.
pub struct SlidingWindowChunker {
    chunk_size: usize,
    step: usize, // < chunk_size pour le chevauchement
}

impl SlidingWindowChunker {
    /// Crée un chunker avec chevauchement. `step` doit être < `chunk_size`.
    pub fn new(chunk_size: usize, step: usize) -> ExofsResult<Self> {
        if chunk_size < FIXED_MIN_CHUNK_SIZE || chunk_size > FIXED_MAX_CHUNK_SIZE {
            return Err(ExofsError::InvalidArgument);
        }
        if step == 0 || step >= chunk_size {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(Self { chunk_size, step })
    }

    /// Produit les empreintes des fenêtres glissantes.
    ///
    /// OOM-02 : try_reserve.
    /// RECUR-01 : boucle while.
    pub fn slide_fingerprints(&self, data: &[u8]) -> ExofsResult<Vec<ChunkFingerprint>> {
        let total = data.len();
        if total < self.chunk_size {
            let fp = ChunkFingerprint::compute(data);
            let mut out = Vec::new();
            out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            out.push(fp);
            return Ok(out);
        }
        let n_windows = (total - self.chunk_size) / self.step + 1;
        let mut fps: Vec<ChunkFingerprint> = Vec::new();
        fps.try_reserve(n_windows)
            .map_err(|_| ExofsError::NoMemory)?;
        let mut pos: usize = 0;
        while pos.saturating_add(self.chunk_size) <= total {
            let window = &data[pos..pos + self.chunk_size];
            fps.push(ChunkFingerprint::compute(window));
            pos = pos.saturating_add(self.step);
        }
        // Dernier morceau si données restantes.
        if pos < total {
            fps.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            fps.push(ChunkFingerprint::compute(&data[pos..]));
        }
        Ok(fps)
    }

    /// Retourne le nombre de fenêtres pour une taille donnée.
    pub fn window_count(&self, data_len: usize) -> usize {
        if data_len < self.chunk_size {
            return 1;
        }
        (data_len - self.chunk_size) / self.step + 1
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AlignedFixedChunker — chunker aligné sur secteur
// ─────────────────────────────────────────────────────────────────────────────

use super::chunking::align_up;

/// Chunker fixe avec alignement de secteur (512 B ou multiple).
pub struct AlignedFixedChunker {
    chunk_size: usize,
    alignment: usize,
}

impl AlignedFixedChunker {
    /// Crée un chunker avec alignement (doit être puissance de 2).
    pub fn new(chunk_size: usize, alignment: usize) -> ExofsResult<Self> {
        if chunk_size < FIXED_MIN_CHUNK_SIZE {
            return Err(ExofsError::InvalidArgument);
        }
        if alignment == 0 || (alignment & alignment.wrapping_sub(1)) != 0 {
            return Err(ExofsError::InvalidArgument); // pas une puissance de 2
        }
        Ok(Self {
            chunk_size,
            alignment,
        })
    }

    /// Retourne la taille de chunk alignée (arrondie au-dessus).
    ///
    /// ARITH-02 : via align_up.
    pub fn aligned_chunk_size(&self) -> ExofsResult<usize> {
        align_up(self.chunk_size, self.alignment)
    }

    /// Découpe `data` en chunks alignés.
    ///
    /// RECUR-01 : boucle while.
    /// OOM-02 : try_reserve.
    pub fn chunk_aligned(&self, data: &[u8]) -> ExofsResult<Vec<DedupChunk>> {
        let total = data.len();
        if total == 0 {
            return Ok(Vec::new());
        }
        let cs = self.aligned_chunk_size()?;
        let n = (total + cs - 1) / cs;
        if n > CHUNK_MAX_PER_BLOB {
            return Err(ExofsError::OffsetOverflow);
        }
        let mut out: Vec<DedupChunk> = Vec::new();
        out.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        let mut offset: usize = 0;
        while offset < total {
            let end = (offset.saturating_add(cs)).min(total);
            let raw = &data[offset..end];
            let b3 = blake3_hash(raw);
            let fh = fnv1a64(raw);
            let bd = ChunkBoundary::new(offset as u64, raw.len() as u32)?;
            out.push(DedupChunk::new(bd, b3, fh, raw)?);
            offset = offset.saturating_add(raw.len());
        }
        Ok(out)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_extended {
    use super::*;

    #[test]
    fn test_sliding_window_single_chunk() {
        let sw = SlidingWindowChunker::new(4, 2).unwrap();
        let fps = sw.slide_fingerprints(&[0u8; 3]).unwrap();
        assert_eq!(fps.len(), 1);
    }

    #[test]
    fn test_sliding_window_multiple() {
        let sw = SlidingWindowChunker::new(4, 2).unwrap();
        let fps = sw.slide_fingerprints(&[0u8; 10]).unwrap();
        assert!(fps.len() >= 3);
    }

    #[test]
    fn test_sliding_window_invalid_step() {
        assert!(SlidingWindowChunker::new(4, 4).is_err()); // step == chunk_size
        assert!(SlidingWindowChunker::new(4, 0).is_err()); // step == 0
    }

    #[test]
    fn test_sliding_window_count() {
        let sw = SlidingWindowChunker::new(4, 2).unwrap();
        assert_eq!(sw.window_count(10), 4); // (10-4)/2+1 = 4
    }

    #[test]
    fn test_aligned_chunker_power2() {
        assert!(AlignedFixedChunker::new(4096, 512).is_ok());
        assert!(AlignedFixedChunker::new(4096, 300).is_err()); // pas puissance de 2
    }

    #[test]
    fn test_aligned_chunker_chunk() {
        let ac = AlignedFixedChunker::new(512, 512).unwrap();
        let data = &[0xABu8; 1000];
        let chunks = ac.chunk_aligned(data).unwrap();
        assert_eq!(chunks.len(), 2); // 512 + 488
    }

    #[test]
    fn test_aligned_chunk_size() {
        let ac = AlignedFixedChunker::new(1000, 512).unwrap();
        let s = ac.aligned_chunk_size().unwrap();
        assert_eq!(s, 1024); // arrondi à 512
    }

    #[test]
    fn test_fixed_chunker_stats() {
        let c = FixedChunker::new(4).unwrap();
        let chunks = c.chunk(&[0u8; 12]).unwrap();
        let stats = ChunkStats::from_chunks(&chunks).unwrap();
        assert_eq!(stats.total_chunks, 3);
        assert_eq!(stats.total_bytes, 12);
    }
}
