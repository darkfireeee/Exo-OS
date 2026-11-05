// kernel/src/memory/swap/cluster.rs
//
// Swap I/O grouping into sequential clusters.
//
// Motivation: submitting 8 pages to swap in a single sequential 32 KiB operation
// is far more efficient than 8 individual 4 KiB I/Os.
//
// Architecture:
//   • `SwapCluster` — group of CLUSTER_SIZE consecutive slots on a device.
//   • `ClusterQueue` — queue of clusters ready to be written.
//   • `ClusterAccumulator` — accumulates incoming slots until forming
//     a full cluster, then pushes it into `ClusterQueue`.
//   • `ClusterManager` — global entry point.
//   • `CLUSTER_MANAGER` — static instance.
//
// Typical flow:
//   1. `CLUSTER_MANAGER.add_page(dev_idx, slot, pfn)` — called by reclaim.
//   2. When CLUSTER_SIZE pages are accumulated, a cluster is formed.
//   3. `CLUSTER_MANAGER.flush()` — forces write even if cluster is partial.
//   4. `CLUSTER_MANAGER.next_cluster()` — returns the next cluster to write
//      (called by swap thread to perform actual I/Os).

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::swap::backend::SwapSlot;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

/// Number of pages per cluster (8 x 4 KiB = 32 KiB per I/O).
pub const CLUSTER_SIZE: usize = 8;

/// Maximum number of clusters in the queue ready to be written.
pub const MAX_CLUSTER_QUEUE: usize = 256;

/// Maximum number of per-device accumulators (1 accumulator = 1 cluster in progress).
pub const MAX_SWAP_DEVICES: usize = 8; // must match swap/backend.rs

// ─────────────────────────────────────────────────────────────────────────────
// SWAP CLUSTER
// ─────────────────────────────────────────────────────────────────────────────

/// An entry in a cluster — a swap slot and its source PFN.
#[derive(Copy, Clone, Debug)]
pub struct ClusterEntry {
    pub slot: SwapSlot,
    pub src_pfn: u64,
}

impl ClusterEntry {
    const EMPTY: ClusterEntry = ClusterEntry {
        slot: SwapSlot(0),
        src_pfn: 0,
    };
}

/// A cluster of `CLUSTER_SIZE` pages to write sequentially.
#[derive(Copy, Clone)]
pub struct SwapCluster {
    /// Target swap device index.
    pub dev_idx: u8,
    /// Cluster entries.
    pub entries: [ClusterEntry; CLUSTER_SIZE],
    /// Number of valid entries in this cluster (1..=CLUSTER_SIZE).
    pub count: u8,
}

impl SwapCluster {
    const fn new() -> Self {
        SwapCluster {
            dev_idx: 0,
            entries: [ClusterEntry::EMPTY; CLUSTER_SIZE],
            count: 0,
        }
    }

    /// Returns valid entries.
    #[inline]
    pub fn valid_entries(&self) -> &[ClusterEntry] {
        &self.entries[..self.count as usize]
    }

    /// Total size in bytes of this cluster.
    #[inline]
    pub fn byte_size(&self) -> usize {
        self.count as usize * crate::memory::core::constants::PAGE_SIZE
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CLUSTER QUEUE (STATIC RING-BUFFER)
// ─────────────────────────────────────────────────────────────────────────────

struct ClusterQueue {
    buf: [SwapCluster; MAX_CLUSTER_QUEUE],
    head: usize, // producer
    tail: usize, // consumer
    count: usize,
}

impl ClusterQueue {
    const fn new() -> Self {
        ClusterQueue {
            buf: [SwapCluster::new(); MAX_CLUSTER_QUEUE], // Copy
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    fn push(&mut self, cluster: SwapCluster) -> bool {
        if self.count >= MAX_CLUSTER_QUEUE {
            return false;
        }
        self.buf[self.head] = cluster;
        self.head = (self.head + 1) % MAX_CLUSTER_QUEUE;
        self.count += 1;
        true
    }

    fn pop(&mut self) -> Option<SwapCluster> {
        if self.count == 0 {
            return None;
        }
        let c = self.buf[self.tail];
        self.tail = (self.tail + 1) % MAX_CLUSTER_QUEUE;
        self.count -= 1;
        Some(c)
    }

    #[inline]
    fn len(&self) -> usize {
        self.count
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PER-DEVICE ACCUMULATOR
// ─────────────────────────────────────────────────────────────────────────────

struct Accumulator {
    cluster: SwapCluster,
    /// True if this accumulator is active for a device.
    active: bool,
}

impl Accumulator {
    const fn new() -> Self {
        Accumulator {
            cluster: SwapCluster::new(),
            active: false,
        }
    }

    /// Adds an entry to the current accumulator.
    /// Returns `Some(cluster)` if the cluster is now complete.
    fn add_entry(&mut self, dev_idx: u8, entry: ClusterEntry) -> Option<SwapCluster> {
        if !self.active {
            self.cluster = SwapCluster::new();
            self.cluster.dev_idx = dev_idx;
            self.active = true;
        }
        let idx = self.cluster.count as usize;
        if idx < CLUSTER_SIZE {
            self.cluster.entries[idx] = entry;
            self.cluster.count += 1;
        }
        if self.cluster.count as usize >= CLUSTER_SIZE {
            let full = self.cluster;
            self.active = false;
            self.cluster.count = 0;
            Some(full)
        } else {
            None
        }
    }

    /// Flushes partially filled accumulator (forced flush).
    /// Returns `Some(cluster)` if there were pending entries.
    fn flush(&mut self) -> Option<SwapCluster> {
        if !self.active || self.cluster.count == 0 {
            return None;
        }
        let partial = self.cluster;
        self.active = false;
        self.cluster.count = 0;
        Some(partial)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CLUSTER MANAGER
// ─────────────────────────────────────────────────────────────────────────────

/// Cluster manager statistics.
pub struct ClusterStats {
    /// Total full clusters formed.
    pub clusters_formed: AtomicU64,
    /// Total partial clusters flushed.
    pub partial_flushes: AtomicU64,
    /// Total entries added.
    pub pages_added: AtomicU64,
    /// Lost clusters (queue full).
    pub queue_overflows: AtomicU64,
    /// Clusters consumed by the I/O thread.
    pub clusters_consumed: AtomicU64,
}

impl ClusterStats {
    const fn new() -> Self {
        ClusterStats {
            clusters_formed: AtomicU64::new(0),
            partial_flushes: AtomicU64::new(0),
            pages_added: AtomicU64::new(0),
            queue_overflows: AtomicU64::new(0),
            clusters_consumed: AtomicU64::new(0),
        }
    }
}

struct ClusterManagerInner {
    accumulators: [Accumulator; MAX_SWAP_DEVICES],
    queue: ClusterQueue,
}

impl ClusterManagerInner {
    const fn new() -> Self {
        const A: Accumulator = Accumulator::new();
        ClusterManagerInner {
            accumulators: [A; MAX_SWAP_DEVICES],
            queue: ClusterQueue::new(),
        }
    }
}

pub struct ClusterManager {
    inner: Mutex<ClusterManagerInner>,
    pub stats: ClusterStats,
}

impl ClusterManager {
    const fn new() -> Self {
        ClusterManager {
            inner: Mutex::new(ClusterManagerInner::new()),
            stats: ClusterStats::new(),
        }
    }

    /// Adds a page to the cluster being formed for `dev_idx`.
    /// If a cluster becomes full, it is pushed to the output queue.
    ///
    /// Returns `true` if the entry was accepted (failure if `dev_idx` is out of bounds).
    pub fn add_page(&self, dev_idx: usize, slot: SwapSlot, src_pfn: u64) -> bool {
        if dev_idx >= MAX_SWAP_DEVICES {
            return false;
        }

        self.stats.pages_added.fetch_add(1, Ordering::Relaxed);

        let entry = ClusterEntry { slot, src_pfn };
        let mut inner = self.inner.lock();

        if let Some(cluster) = inner.accumulators[dev_idx].add_entry(dev_idx as u8, entry) {
            // Full cluster — push to queue.
            if !inner.queue.push(cluster) {
                self.stats.queue_overflows.fetch_add(1, Ordering::Relaxed);
            } else {
                self.stats.clusters_formed.fetch_add(1, Ordering::Relaxed);
            }
        }
        true
    }

    /// Forced flush of all partial accumulators.
    /// Typically called upon shutdown or urgent reclaim.
    pub fn flush_all(&self) {
        let mut inner = self.inner.lock();
        let r = &mut *inner;
        for acc in r.accumulators.iter_mut() {
            if let Some(partial) = acc.flush() {
                if r.queue.push(partial) {
                    self.stats.partial_flushes.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.stats.queue_overflows.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    /// Forced flush of the accumulator for a given device.
    pub fn flush_device(&self, dev_idx: usize) -> bool {
        if dev_idx >= MAX_SWAP_DEVICES {
            return false;
        }
        let mut inner = self.inner.lock();
        if let Some(partial) = inner.accumulators[dev_idx].flush() {
            if inner.queue.push(partial) {
                self.stats.partial_flushes.fetch_add(1, Ordering::Relaxed);
                return true;
            } else {
                self.stats.queue_overflows.fetch_add(1, Ordering::Relaxed);
            }
        }
        false
    }

    /// Returns the next cluster ready to be written to the swap device.
    /// Called by the swap I/O thread.
    pub fn next_cluster(&self) -> Option<SwapCluster> {
        let mut inner = self.inner.lock();
        let c = inner.queue.pop();
        if c.is_some() {
            self.stats.clusters_consumed.fetch_add(1, Ordering::Relaxed);
        }
        c
    }

    /// Number of clusters awaiting write.
    #[inline]
    pub fn pending_count(&self) -> usize {
        self.inner.lock().queue.len()
    }

    /// Number of pages pending in accumulators (not yet in queue).
    pub fn buffered_pages(&self) -> usize {
        let inner = self.inner.lock();
        inner
            .accumulators
            .iter()
            .map(|a| {
                if a.active {
                    a.cluster.count as usize
                } else {
                    0
                }
            })
            .sum()
    }
}

/// Global instance of swap cluster manager.
pub static CLUSTER_MANAGER: ClusterManager = ClusterManager::new();
