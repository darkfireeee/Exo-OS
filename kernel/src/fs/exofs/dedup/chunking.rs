//! chunking.rs — types communs pour le découpage en chunks (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use super::chunk_fingerprint::ChunkFingerprint;

/// Limite de chunk (offset de début + fin dans les données source).
#[derive(Clone, Copy, Debug)]
pub struct ChunkBoundary {
    pub offset: u64,
    pub length: u32,
}

/// Chunk dédupliqué avec son empreinte.
#[derive(Clone, Debug)]
pub struct DedupChunk {
    pub boundary:    ChunkBoundary,
    pub fingerprint: ChunkFingerprint,
}

/// Trait commun pour les découpeurs de chunks.
pub trait Chunker {
    /// Découpe `data` et retourne les limites de chunks.
    fn chunk(&self, data: &[u8]) -> Result<Vec<DedupChunk>, FsError>;

    /// Taille minimale d'un chunk.
    fn min_size(&self) -> u32;

    /// Taille maximale d'un chunk.
    fn max_size(&self) -> u32;
}

/// Statistiques de chunking.
#[derive(Default, Clone, Debug)]
pub struct ChunkerStats {
    pub chunks_produced: u64,
    pub bytes_processed: u64,
    pub avg_chunk_size:  u64,
}
