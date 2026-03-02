//! ChunkIndex — index persistant des chunks pour la déduplication (no_std).
//!
//! Mappe ChunkFingerprint → BlobId (chunk partagé entre blobs).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{BlobId, FsError};
use super::chunk_fingerprint::ChunkFingerprint;

/// Entrée dans l'index d'un chunk.
#[derive(Clone, Debug)]
pub struct ChunkEntry {
    pub fingerprint: ChunkFingerprint,
    pub blob_id:     BlobId,    // BlobId du chunk dédupliqué.
    pub ref_count:   u32,       // Nombre de références à ce chunk.
    pub size:        u32,
}

/// Index global de chunks.
pub static CHUNK_INDEX: ChunkIndex = ChunkIndex::new_const();

pub struct ChunkIndex {
    table:        SpinLock<BTreeMap<[u8; 32], ChunkEntry>>,  // Clé = blake3.
    total_chunks: AtomicU64,
    dedup_hits:   AtomicU64,
    dedup_misses: AtomicU64,
}

impl ChunkIndex {
    pub const fn new_const() -> Self {
        Self {
            table:        SpinLock::new(BTreeMap::new()),
            total_chunks: AtomicU64::new(0),
            dedup_hits:   AtomicU64::new(0),
            dedup_misses: AtomicU64::new(0),
        }
    }

    /// Cherche un chunk dans l'index.
    pub fn lookup(&self, fp: &ChunkFingerprint) -> Option<BlobId> {
        let table = self.table.lock();
        if let Some(entry) = table.get(&fp.blake3) {
            if entry.fingerprint.matches(fp) {
                self.dedup_hits.fetch_add(1, Ordering::Relaxed);
                return Some(entry.blob_id);
            }
        }
        self.dedup_misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insère un nouveau chunk dans l'index.
    pub fn insert(&self, fp: ChunkFingerprint, blob_id: BlobId) -> Result<(), FsError> {
        let mut table = self.table.lock();
        if let Some(entry) = table.get_mut(&fp.blake3) {
            // Déjà présent : incrémenter le refcount.
            entry.ref_count = entry.ref_count.saturating_add(1);
            return Ok(());
        }
        table.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        table.insert(fp.blake3, ChunkEntry {
            fingerprint: fp,
            blob_id,
            ref_count: 1,
            size: fp.size,
        });
        self.total_chunks.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Décrémente le refcount d'un chunk. Retourne true si le chunk doit être supprimé.
    pub fn dec_ref(&self, fp: &ChunkFingerprint) -> bool {
        let mut table = self.table.lock();
        if let Some(entry) = table.get_mut(&fp.blake3) {
            if entry.ref_count <= 1 {
                table.remove(&fp.blake3);
                self.total_chunks.fetch_sub(1, Ordering::Relaxed);
                return true;
            }
            entry.ref_count -= 1;
        }
        false
    }

    /// Retourne une copie des N premières entrées (pour inspection / export).
    pub fn snapshot(&self, limit: usize) -> Result<Vec<ChunkEntry>, FsError> {
        let table = self.table.lock();
        let mut out = Vec::new();
        out.try_reserve(limit.min(table.len())).map_err(|_| FsError::OutOfMemory)?;
        for (_, entry) in table.iter().take(limit) {
            out.push(entry.clone());
        }
        Ok(out)
    }

    pub fn total_chunks(&self) -> u64 { self.total_chunks.load(Ordering::Relaxed) }
    pub fn dedup_hits(&self) -> u64 { self.dedup_hits.load(Ordering::Relaxed) }
    pub fn dedup_misses(&self) -> u64 { self.dedup_misses.load(Ordering::Relaxed) }

    pub fn dedup_ratio_percent(&self) -> u64 {
        let total = self.dedup_hits.load(Ordering::Relaxed)
            + self.dedup_misses.load(Ordering::Relaxed);
        if total == 0 { return 0; }
        self.dedup_hits.load(Ordering::Relaxed) * 100 / total
    }
}
