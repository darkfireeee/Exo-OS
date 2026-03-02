//! Préchargement (prefetch) de blobs ExoFS — charge les blobs futurs en mémoire.
//!
//! Basé sur la liste de blobs référencés dans un EpochRecord.
//! RÈGLE 14 : checked_add pour les compteurs.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::storage::BlobStore;
use crate::scheduler::sync::spinlock::SpinLock;

/// Requête de préchargement.
#[derive(Debug, Clone)]
pub struct PrefetchRequest {
    pub blob_id: BlobId,
    pub priority: u8, // 0 = basse, 255 = haute
}

/// File de préchargement — les blobs sont chargés en arrière-plan.
pub struct Prefetcher {
    queue: SpinLock<VecDeque<PrefetchRequest>>,
    max_queue_depth: usize,
    prefetched_count: core::sync::atomic::AtomicU64,
    prefetch_bytes: core::sync::atomic::AtomicU64,
}

impl Prefetcher {
    pub const fn new(max_queue_depth: usize) -> Self {
        Self {
            queue: SpinLock::new(VecDeque::new()),
            max_queue_depth,
            prefetched_count: core::sync::atomic::AtomicU64::new(0),
            prefetch_bytes: core::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Enfile une requête de préchargement.
    /// Si la queue est plein, ignore silencieusement (prefetch non-critique).
    pub fn enqueue(&self, req: PrefetchRequest) -> Result<(), FsError> {
        let mut q = self.queue.lock();
        if q.len() >= self.max_queue_depth {
            return Ok(()); // Plein mais non-critique.
        }
        q.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        q.push_back(req);
        Ok(())
    }

    /// Enfile un ensemble de blobs avec la même priorité.
    pub fn enqueue_batch(&self, blobs: &[BlobId], priority: u8) -> Result<(), FsError> {
        for blob_id in blobs {
            self.enqueue(PrefetchRequest { blob_id: *blob_id, priority })?;
        }
        Ok(())
    }

    /// Traite jusqu'à `max` requêtes de la queue.
    /// Retourne le nombre de blobs effectivement préchargés.
    pub fn process(&self, store: &BlobStore, max: usize) -> Result<u64, FsError> {
        let batch: Vec<PrefetchRequest> = {
            let mut q = self.queue.lock();
            let n = q.len().min(max);
            let mut out = Vec::new();
            out.try_reserve(n).map_err(|_| FsError::OutOfMemory)?;
            for _ in 0..n {
                if let Some(r) = q.pop_front() {
                    out.push(r);
                }
            }
            out
        };

        let mut count = 0u64;
        for req in batch {
            if store.prefetch_blob(&req.blob_id).is_ok() {
                if let Ok(sz) = store.blob_size(&req.blob_id) {
                    self.prefetch_bytes.fetch_add(sz, core::sync::atomic::Ordering::Relaxed);
                }
                count = count.checked_add(1).ok_or(FsError::Overflow)?;
            }
        }
        self.prefetched_count.fetch_add(count, core::sync::atomic::Ordering::Relaxed);
        Ok(count)
    }

    pub fn total_prefetched(&self) -> u64 {
        self.prefetched_count.load(core::sync::atomic::Ordering::Relaxed)
    }
}
