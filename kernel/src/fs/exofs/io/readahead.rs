//! Moteur de readahead ExoFS — précharge séquentiellement après un accès.
//!
//! Détecte les patterns séquentiels et précharge les blobs suivants.
//! RÈGLE 14 : checked_add pour les indices/offsets.

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::io::prefetch::{Prefetcher, PrefetchRequest};
use crate::fs::exofs::storage::BlobStore;
use crate::scheduler::sync::spinlock::SpinLock;

/// Historique d'accès par blob pour détecter les patterns.
struct AccessHistory {
    last_offset: u64,
    sequential_hits: u32,
}

/// Moteur de readahead adaptatif.
pub struct ReadaheadEngine {
    history: SpinLock<BTreeMap<BlobId, AccessHistory>>,
    prefetcher: Prefetcher,
    /// Fenêtre de readahead en bytes.
    window_bytes: u64,
    total_readahead_ops: AtomicU64,
}

impl ReadaheadEngine {
    pub fn new(window_bytes: u64) -> Self {
        Self {
            history: SpinLock::new(BTreeMap::new()),
            prefetcher: Prefetcher::new(128),
            window_bytes,
            total_readahead_ops: AtomicU64::new(0),
        }
    }

    /// Notifie une lecture à `offset` dans `blob_id`.
    /// Si un pattern séquentiel est détecté, déclenche le readahead.
    pub fn notify_read(
        &self,
        blob_id: BlobId,
        offset: u64,
        len: usize,
        store: &BlobStore,
    ) -> Result<(), FsError> {
        let is_sequential = {
            let mut hist = self.history.lock();
            let entry = hist.entry(blob_id).or_insert(AccessHistory {
                last_offset: 0,
                sequential_hits: 0,
            });
            let expected = entry.last_offset.checked_add(entry.last_offset).unwrap_or(0);
            let is_seq = offset == expected || offset == entry.last_offset;
            if is_seq {
                entry.sequential_hits = entry.sequential_hits.saturating_add(1);
            } else {
                entry.sequential_hits = 0;
            }
            entry.last_offset = offset.checked_add(len as u64).unwrap_or(offset);
            is_seq && entry.sequential_hits >= 2
        };

        if is_sequential {
            // Précharge la fenêtre suivante.
            let next_offset = offset
                .checked_add(len as u64)
                .ok_or(FsError::Overflow)?;
            let blob_size = store.blob_size(&blob_id)?;
            if next_offset < blob_size {
                self.prefetcher.enqueue(PrefetchRequest { blob_id, priority: 128 })?;
                self.total_readahead_ops.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    /// Traite la file de readahead.
    pub fn flush(&self, store: &BlobStore) -> Result<u64, FsError> {
        self.prefetcher.process(store, 32)
    }

    pub fn total_ops(&self) -> u64 {
        self.total_readahead_ops.load(Ordering::Relaxed)
    }
}
