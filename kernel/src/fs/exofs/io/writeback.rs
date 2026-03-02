//! Queue de writeback ExoFS — écriture différée des blobs dirty.
//!
//! Les blobs modifiés sont enfilés ici et flushés par le thread writeback.
//! RÈGLE 13 : n'interfère pas avec EPOCH_COMMIT_LOCK.
//! RÈGLE 2  : try_reserve avant tout push.

use alloc::collections::{BTreeMap, VecDeque};
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::storage::BlobStore;
use crate::scheduler::sync::spinlock::SpinLock;

/// Entrée writeback avec priorité et taille de données dirty.
#[derive(Debug, Clone)]
pub struct WritebackEntry {
    pub blob_id: BlobId,
    /// Timestamp de la première modification (ticks).
    pub first_dirty_tick: u64,
    /// Bytes modifiés depuis le dernier flush.
    pub dirty_bytes: u64,
}

/// Queue de writeback avec déduplification sur BlobId.
pub struct WritebackQueue {
    queue: SpinLock<VecDeque<WritebackEntry>>,
    seen: SpinLock<BTreeMap<BlobId, ()>>,
    enqueued_total: AtomicU64,
    flushed_total: AtomicU64,
    flush_errors: AtomicU64,
}

impl WritebackQueue {
    pub const fn new() -> Self {
        Self {
            queue: SpinLock::new(VecDeque::new()),
            seen: SpinLock::new(BTreeMap::new()),
            enqueued_total: AtomicU64::new(0),
            flushed_total: AtomicU64::new(0),
            flush_errors: AtomicU64::new(0),
        }
    }

    /// Enfile un blob dirty pour writeback. Dédupliqué automatiquement.
    pub fn mark_dirty(
        &self,
        blob_id: BlobId,
        dirty_bytes: u64,
        tick: u64,
    ) -> Result<(), FsError> {
        let already = {
            let mut seen = self.seen.lock();
            if seen.contains_key(&blob_id) {
                true
            } else {
                seen.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                seen.insert(blob_id, ());
                false
            }
        };

        if !already {
            let entry = WritebackEntry {
                blob_id,
                first_dirty_tick: tick,
                dirty_bytes,
            };
            let mut q = self.queue.lock();
            q.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
            q.push_back(entry);
            self.enqueued_total.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Flush les blobs les plus anciens en priorité. Retourne le nombre flushés.
    pub fn flush_batch(&self, store: &BlobStore, max: usize) -> Result<u64, FsError> {
        let batch: alloc::vec::Vec<WritebackEntry> = {
            let mut q = self.queue.lock();
            let n = q.len().min(max);
            let mut out = alloc::vec::Vec::new();
            out.try_reserve(n).map_err(|_| FsError::OutOfMemory)?;
            for _ in 0..n {
                if let Some(e) = q.pop_front() {
                    out.push(e);
                }
            }
            out
        };

        let mut flushed = 0u64;
        for entry in batch {
            match store.flush_blob(&entry.blob_id) {
                Ok(_) => {
                    self.seen.lock().remove(&entry.blob_id);
                    flushed = flushed.saturating_add(1);
                }
                Err(_) => {
                    self.flush_errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        self.flushed_total.fetch_add(flushed, Ordering::Relaxed);
        Ok(flushed)
    }

    pub fn pending_count(&self) -> usize {
        self.queue.lock().len()
    }

    pub fn total_enqueued(&self) -> u64 { self.enqueued_total.load(Ordering::Relaxed) }
    pub fn total_flushed(&self) -> u64  { self.flushed_total.load(Ordering::Relaxed) }
    pub fn total_errors(&self) -> u64   { self.flush_errors.load(Ordering::Relaxed) }
}

/// Queue globale de writeback.
pub static WRITEBACK_QUEUE: WritebackQueue = WritebackQueue::new();

/// Démarre le thread writeback au démarrage d'ExoFS.
/// La création effective du thread kernel est déléguée à l'init système.
pub fn start_writeback_thread() -> Result<(), crate::fs::exofs::core::FsError> {
    Ok(())
}
