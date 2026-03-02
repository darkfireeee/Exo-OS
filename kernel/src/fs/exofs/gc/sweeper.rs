//! Sweeper GC — libération physique des blobs blancs après marquage.
//!
//! Parcourt l'ensemble blanc (non-marqué) et enfile les blobs dans
//! la DeferredDeleteQueue. Ne libère jamais directement sous EPOCH_COMMIT_LOCK.
//!
//! RÈGLE 13 : GC n'acquiert jamais EPOCH_COMMIT_LOCK.
//! RÈGLE 12 : Vérifie ref_count == 0 avant libération.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::gc::blob_refcount::BLOB_REFCOUNT;
use crate::fs::exofs::gc::tricolor::{BlobIndex, TricolorSet};
use crate::scheduler::sync::spinlock::SpinLock;

/// File d'attente différée pour les suppressions de blobs.
///
/// Les entrées sont traitées hors du chemin critique de commit.
pub struct DeferredDeleteQueue {
    inner: SpinLock<VecDeque<BlobId>>,
}

impl DeferredDeleteQueue {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(VecDeque::new()),
        }
    }

    /// Enfile un BlobId pour suppression différée.
    pub fn enqueue(&self, id: BlobId) -> Result<(), FsError> {
        let mut q = self.inner.lock();
        q.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        q.push_back(id);
        Ok(())
    }

    /// Défile jusqu'à `max` entrées et les retourne.
    pub fn drain_batch(&self, max: usize) -> Result<Vec<BlobId>, FsError> {
        let mut q = self.inner.lock();
        let n = q.len().min(max);
        let mut out = Vec::new();
        out.try_reserve(n).map_err(|_| FsError::OutOfMemory)?;
        for _ in 0..n {
            if let Some(id) = q.pop_front() {
                out.push(id);
            }
        }
        Ok(out)
    }

    /// Retourne le nombre d'entrées en attente.
    pub fn pending(&self) -> usize {
        self.inner.lock().len()
    }
}

/// File globale de suppression différée.
/// RÈGLE 13 : seul le GC (hors EPOCH_COMMIT_LOCK) y ajoute des entrées.
pub static DEFERRED_DELETE: DeferredDeleteQueue = DeferredDeleteQueue::new();

/// Statistiques de la phase sweep.
#[derive(Debug, Default)]
pub struct SweepStats {
    /// Blobs enfilés dans DEFERRED_DELETE.
    pub blobs_swept: u64,
    /// Bytes cumulés estimés.
    pub bytes_freed: u64,
    /// Blobs ignorés car ref_count > 0 (race write concurrent légitime).
    pub skipped_nonzero_ref: u64,
}

/// Sweeper : enfile dans DEFERRED_DELETE tous les blobs blancs après marquage.
pub struct Sweeper;

impl Sweeper {
    /// Parcourt l'ensemble blanc et enfile les blobs dont ref_count == 0.
    ///
    /// Les blobs avec ref_count > 0 sont ignorés silencieusement (write concurrent
    /// légitime qui a incrémenté le compteur entre le scan et le sweep).
    pub fn sweep(index: &BlobIndex, colors: &TricolorSet) -> Result<SweepStats, FsError> {
        let mut stats = SweepStats::default();

        colors.collect_white(|idx| {
            let blob_id = match index.blob_at(idx) {
                Some(id) => *id,
                None => return,
            };

            // RÈGLE 12 : on ne fait jamais de fetch_sub aveugle.
            // On lit le compteur et on ne supprime que si == 0.
            match BLOB_REFCOUNT.get(&blob_id) {
                Some(0) | None => {
                    if DEFERRED_DELETE.enqueue(blob_id).is_ok() {
                        stats.blobs_swept = stats.blobs_swept.wrapping_add(1);
                    }
                }
                Some(_) => {
                    stats.skipped_nonzero_ref =
                        stats.skipped_nonzero_ref.wrapping_add(1);
                }
            }
        });

        Ok(stats)
    }
}
