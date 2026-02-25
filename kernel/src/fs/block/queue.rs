// kernel/src/fs/block/queue.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// BLOCK QUEUE — File de requêtes block device (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Interface entre le VFS et les drivers block device.
//
// Architecture :
//   • `RequestQueue` : file FIFO de Bio + dispatch vers le scheduler I/O.
//   • `submit_bio()` : point d'entrée global pour soumettre une Bio.
//   • Le scheduler (fs/block/scheduler.rs) réordonne les requêtes pour
//     maximiser le throughput (algorithme deadline par défaut).
//   • La queue est globale dans ce design simplifié ; en production chaque
//     block device aura sa propre queue.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use alloc::vec::Vec;
use alloc::collections::VecDeque;

use crate::fs::core::types::{FsError, FsResult};
use crate::fs::block::bio::{Bio, BioOp, BioStatus, BIO_STATS};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// RequestQueue
// ─────────────────────────────────────────────────────────────────────────────

pub struct RequestQueue {
    queue:       SpinLock<VecDeque<Bio>>,
    capacity:    usize,
    pub enqueued:    AtomicU64,
    pub dispatched:  AtomicU64,
    pub queue_depth: AtomicUsize,
}

impl RequestQueue {
    pub const fn new(capacity: usize) -> Self {
        Self {
            queue:      SpinLock::new(VecDeque::new()),
            capacity,
            enqueued:   AtomicU64::new(0),
            dispatched: AtomicU64::new(0),
            queue_depth:AtomicUsize::new(0),
        }
    }

    /// Enqueue une Bio.
    pub fn enqueue(&self, bio: Bio) -> FsResult<()> {
        let mut q = self.queue.lock();
        if q.len() >= self.capacity {
            return Err(FsError::Again); // EAGAIN — queue full
        }
        q.push_back(bio);
        self.enqueued.fetch_add(1, Ordering::Relaxed);
        self.queue_depth.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Dispatch jusqu'à `max` requêtes.
    pub fn dispatch(&self, max: usize) -> Vec<Bio> {
        let mut q    = self.queue.lock();
        let n        = max.min(q.len());
        let mut out  = Vec::with_capacity(n);
        for _ in 0..n {
            if let Some(bio) = q.pop_front() {
                self.queue_depth.fetch_sub(1, Ordering::Relaxed);
                self.dispatched.fetch_add(1, Ordering::Relaxed);
                out.push(bio);
            }
        }
        out
    }

    pub fn depth(&self) -> usize {
        self.queue_depth.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global request queue
// ─────────────────────────────────────────────────────────────────────────────

static GLOBAL_QUEUE: RequestQueue = RequestQueue::new(1024);

/// Soumet une Bio au block layer.
///
/// Dans le mode synchrone simplifié utilisé ici, la Bio est exécutée
/// immédiatement (simulation) ou mise en file si le driver est occupé.
pub fn submit_bio(bio: Bio) -> FsResult<()> {
    let total_len = bio.total_len();
    let is_write  = bio.is_write();

    GLOBAL_QUEUE.enqueue(bio)?;

    // Traitement synchrone immédiat (driver simulé).
    let dispatched = GLOBAL_QUEUE.dispatch(1);
    for b in dispatched {
        b.complete(true); // succès simulé
        BIO_STATS.completed.fetch_add(1, Ordering::Relaxed);
        if is_write {
            BIO_STATS.bytes_written.fetch_add(total_len, Ordering::Relaxed);
        } else {
            BIO_STATS.bytes_read.fetch_add(total_len, Ordering::Relaxed);
        }
    }

    BIO_STATS.submitted.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Drain complet de la queue (flush au shutdown).
pub fn flush_block_queue() {
    loop {
        let dispatched = GLOBAL_QUEUE.dispatch(64);
        if dispatched.is_empty() { break; }
        for b in dispatched {
            b.complete(true);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// QueueStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct QueueStats {
    pub enqueued:   AtomicU64,
    pub dispatched: AtomicU64,
    pub overflows:  AtomicU64,
}

impl QueueStats {
    pub const fn new() -> Self {
        Self {
            enqueued:   AtomicU64::new(0),
            dispatched: AtomicU64::new(0),
            overflows:  AtomicU64::new(0),
        }
    }
}

pub static QUEUE_STATS: QueueStats = QueueStats::new();
