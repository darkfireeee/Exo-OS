//! DedupApi — interface principale de déduplication ExoFS (no_std).
//!
//! Orchestre chunking, index, registry et statistiques.
//! RÈGLE 11 : BlobId = Blake3(données AVANT compression/chiffrement).

use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use super::chunking::Chunker;
use super::chunker_cdc::CdcChunker;
use super::chunker_fixed::FixedChunker;
use super::chunk_index::CHUNK_INDEX;
use super::chunk_cache::CHUNK_CACHE;
use super::blob_registry::BLOB_REGISTRY;
use super::dedup_policy::{DedupPolicy, DedupMode};
use super::dedup_stats::DEDUP_STATS;
use super::content_hash::ContentHash;

/// Résultat de la déduplication d'un blob.
#[derive(Debug)]
pub struct DedupResult {
    pub blob_id:    BlobId,      // BlobId calculé sur les données brutes (RÈGLE 11).
    pub is_deduped: bool,        // True si un doublon exacte a été trouvé.
    pub saved_bytes: u64,
    pub n_chunks:   u32,
    pub chunks_matched: u32,
    pub chunks_new:     u32,
}

/// API de déduplication principale.
pub struct DedupApi {
    policy: DedupPolicy,
}

impl DedupApi {
    pub fn new(policy: DedupPolicy) -> Self {
        Self { policy }
    }

    pub fn default() -> Self {
        Self::new(DedupPolicy::default_adaptive())
    }

    /// Tente de dédupliquer `data`.
    ///
    /// - Calcule le BlobId (RÈGLE 11 : Blake3 des données brutes).
    /// - Vérifie si le blob est un doublon exact dans le registre.
    /// - Si nouveau, découpe en chunks et indexe chaque chunk.
    pub fn dedup_blob(
        &self,
        data: &[u8],
        inode_id: u64,
    ) -> Result<DedupResult, FsError> {
        DEDUP_STATS.record_check(data.len() as u64);

        if !self.policy.should_dedup(data.len() as u64) {
            let blob_id = BlobId::from_bytes_blake3(data);
            BLOB_REGISTRY.register(blob_id, data.len() as u64, 1)?;
            return Ok(DedupResult {
                blob_id, is_deduped: false,
                saved_bytes: 0, n_chunks: 1,
                chunks_matched: 0, chunks_new: 1,
            });
        }

        // RÈGLE 11 : BlobId calculé sur les données brutes.
        let content = ContentHash::compute(data);
        let blob_id = content.blob_id();

        // Vérification doublon exact.
        let is_exact_dup = BLOB_REGISTRY.register(blob_id, data.len() as u64, 0)?;
        if is_exact_dup {
            DEDUP_STATS.record_dedup(data.len() as u64);
            return Ok(DedupResult {
                blob_id, is_deduped: true,
                saved_bytes: data.len() as u64,
                n_chunks: 0, chunks_matched: 0, chunks_new: 0,
            });
        }

        // Découpage et indexation des chunks.
        let chunks = self.do_chunk(data)?;
        let n_chunks = chunks.len() as u32;
        let mut matched = 0u32;
        let mut new_cnt = 0u32;

        for c in &chunks {
            // Vérifie le cache avant l'index principal.
            if let Some(_) = CHUNK_CACHE.lookup(&c.fingerprint) {
                DEDUP_STATS.record_chunk(true);
                matched += 1;
                continue;
            }
            if let Some(_) = CHUNK_INDEX.lookup(&c.fingerprint) {
                let _ = CHUNK_CACHE.insert(c.fingerprint, blob_id);
                DEDUP_STATS.record_chunk(true);
                matched += 1;
            } else {
                CHUNK_INDEX.insert(c.fingerprint, blob_id)?;
                let _ = CHUNK_CACHE.insert(c.fingerprint, blob_id);
                DEDUP_STATS.record_chunk(false);
                new_cnt += 1;
            }
        }

        // Met à jour le nombre de chunks dans le registre.
        // (Ré-enregistrement avec le bon n_chunks — idempotent si ref_count > 1.)
        let _ = BLOB_REGISTRY.register(blob_id, data.len() as u64, n_chunks);

        let _ = inode_id; // Sera utilisé par BlobSharing dans une future évolution.

        Ok(DedupResult {
            blob_id,
            is_deduped: false,
            saved_bytes: 0,
            n_chunks,
            chunks_matched: matched,
            chunks_new: new_cnt,
        })
    }

    fn do_chunk<'a>(&self, data: &'a [u8]) -> Result<Vec<super::chunking::DedupChunk>, FsError> {
        match self.policy.mode {
            DedupMode::FixedSize => {
                FixedChunker::new(self.policy.chunk_avg).chunk(data)
            }
            DedupMode::Cdc | DedupMode::Adaptive => {
                CdcChunker::new(
                    self.policy.chunk_min as usize,
                    self.policy.chunk_max as usize,
                    self.policy.chunk_avg as usize,
                ).chunk(data)
            }
            DedupMode::Disabled => {
                FixedChunker::new(data.len().max(1) as u32).chunk(data)
            }
        }
    }

    pub fn policy(&self) -> &DedupPolicy {
        &self.policy
    }
}
