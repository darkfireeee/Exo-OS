//! writeback.rs — File de writeback asynchrone (pages sales) (no_std).
//!
//! Ce module fournit :
//!  - `WritebackEntry`  : entrée modifiée à écrire (blob_id + métadonnées).
//!  - `WritebackConfig` : configuration (max_queue, retries, age_threshold).
//!  - `WritebackQueue`  : ring circulaire thread-safe (spinlock).
//!  - `WritebackWorker` : traitement des entrées depuis la queue.
//!  - `WritebackStats`  : statistiques cumulées.
//!  - `WRITEBACK_QUEUE` : singleton global.
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add/mul.


extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use core::cell::UnsafeCell;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── WritebackEntry ───────────────────────────────────────────────────────────

/// Entrée sale en attente d'écriture différée.
#[derive(Clone, Copy, Debug)]
pub struct WritebackEntry {
    pub blob_id:     [u8; 32],
    pub dirty_since: u64,    // tick au moment du marquage sale
    pub retries:     u8,
    pub priority:    u8,
    pub size:        u32,    // taille en bytes
    pub in_progress: bool,
}

impl WritebackEntry {
    pub fn new(blob_id: [u8; 32], dirty_since: u64, size: u32, priority: u8) -> Self {
        Self { blob_id, dirty_since, retries: 0, priority, size, in_progress: false }
    }

    pub fn with_priority(mut self, p: u8) -> Self { self.priority = p; self }
    pub fn mark_in_progress(&mut self) { self.in_progress = true; }
    pub fn inc_retries(&mut self) { self.retries = self.retries.saturating_add(1); }
    pub fn age_ticks(&self, now: u64) -> u64 { now.saturating_sub(self.dirty_since) }

    pub fn is_empty() -> Self {
        Self { blob_id: [0u8; 32], dirty_since: 0, retries: 0, priority: 0, size: 0, in_progress: false }
    }
}

// ─── WritebackConfig ─────────────────────────────────────────────────────────

/// Configuration de la queue de writeback.
#[derive(Clone, Copy, Debug)]
pub struct WritebackConfig {
    pub max_queue_depth:      u32,
    pub max_retries:          u8,
    pub dirty_age_us_threshold: u64,  // entrée "urgente" après ce délai en µs
    pub flush_batch_size:     u32,
    pub enable_priority_sort: bool,
}

impl WritebackConfig {
    pub fn default() -> Self {
        Self { max_queue_depth: 256, max_retries: 5, dirty_age_us_threshold: 5_000_000,
            flush_batch_size: 32, enable_priority_sort: true }
    }

    pub fn conservative() -> Self {
        Self { max_queue_depth: 64, max_retries: 3, dirty_age_us_threshold: 1_000_000,
            flush_batch_size: 8, enable_priority_sort: false }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        if self.max_queue_depth == 0 || self.flush_batch_size == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

// ─── WritebackQueue ───────────────────────────────────────────────────────────

pub const WRITEBACK_QUEUE_DEPTH: usize = 256;

/// Queue ring circulaire de writeback (spinlock AtomicU64).
pub struct WritebackQueue {
    slots: UnsafeCell<[WritebackEntry; WRITEBACK_QUEUE_DEPTH]>,
    head:  AtomicU64,
    tail:  AtomicU64,
    lock:  AtomicU64,
    pending: AtomicU64,
}

// SAFETY: accès sous spinlock exclusif.
unsafe impl Sync for WritebackQueue {}
unsafe impl Send for WritebackQueue {}

impl WritebackQueue {
    const EMPTY: WritebackEntry = WritebackEntry {
        blob_id: [0u8; 32], dirty_since: 0, retries: 0,
        priority: 0, size: 0, in_progress: false,
    };

    pub const fn new_const() -> Self {
        Self {
            slots:   UnsafeCell::new([Self::EMPTY; WRITEBACK_QUEUE_DEPTH]),
            head:    AtomicU64::new(0),
            tail:    AtomicU64::new(0),
            lock:    AtomicU64::new(0),
            pending: AtomicU64::new(0),
        }
    }

    fn acquire(&self) {
        while self.lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }
    fn release(&self) { self.lock.store(0, Ordering::Release); }

    pub fn pending_count(&self) -> u64 { self.pending.load(Ordering::Relaxed) }
    pub fn is_empty(&self) -> bool { self.pending.load(Ordering::Relaxed) == 0 }
    pub fn is_full(&self) -> bool { self.pending.load(Ordering::Relaxed) >= WRITEBACK_QUEUE_DEPTH as u64 }

    /// Ajoute une entrée sale (OOM-02 : capacity check).
    pub fn enqueue(&self, entry: WritebackEntry) -> ExofsResult<()> {
        self.acquire();
        let result = (|| {
            if self.is_full() { return Err(ExofsError::Resource); }
            let tail = self.tail.load(Ordering::Relaxed) as usize % WRITEBACK_QUEUE_DEPTH;
            // SAFETY: tail < WRITEBACK_QUEUE_DEPTH, sous spinlock.
            unsafe { (*self.slots.get())[tail] = entry; }
            self.tail.fetch_add(1, Ordering::Relaxed);
            self.pending.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })();
        self.release();
        result
    }

    /// Dépile la prochaine entrée (RECUR-01 : while).
    pub fn dequeue(&self) -> Option<WritebackEntry> {
        self.acquire();
        let result = if self.is_empty() {
            None
        } else {
            let head = self.head.load(Ordering::Relaxed) as usize % WRITEBACK_QUEUE_DEPTH;
            // SAFETY: head < WRITEBACK_QUEUE_DEPTH, sous spinlock.
            let e = unsafe { (*self.slots.get())[head] };
            self.head.fetch_add(1, Ordering::Relaxed);
            self.pending.fetch_sub(1, Ordering::Relaxed);
            Some(e)
        };
        self.release();
        result
    }

    /// Peek sans consommer (RECUR-01 : while).
    pub fn peek_next(&self) -> Option<WritebackEntry> {
        self.acquire();
        let result = if self.is_empty() {
            None
        } else {
            let head = self.head.load(Ordering::Relaxed) as usize % WRITEBACK_QUEUE_DEPTH;
            // SAFETY: head < WRITEBACK_QUEUE_DEPTH, sous spinlock.
            let e = unsafe { (*self.slots.get())[head] };
            Some(e)
        };
        self.release();
        result
    }

    /// Retourne les entrées dont l'âge > threshold (copie, RECUR-01 : while).
    pub fn collect_urgent(&self, now: u64, threshold_us: u64, out: &mut Vec<WritebackEntry>) -> ExofsResult<u32> {
        self.acquire();
        let result = (|| {
            let count = self.pending.load(Ordering::Relaxed) as usize;
            let head = self.head.load(Ordering::Relaxed) as usize;
            let mut found = 0u32;
            let mut i = 0usize;
            while i < count {
                let idx = head.wrapping_add(i) % WRITEBACK_QUEUE_DEPTH;
                // SAFETY: idx en range, sous spinlock.
                let e = unsafe { (*self.slots.get())[idx] };
                if e.age_ticks(now) > threshold_us {
                    out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    out.push(e);
                    found = found.saturating_add(1);
                }
                i = i.wrapping_add(1);
            }
            Ok(found)
        })();
        self.release();
        result
    }
}

/// Singleton global de la queue de writeback.
pub static WRITEBACK_QUEUE: WritebackQueue = WritebackQueue::new_const();

// ─── WritebackStats ───────────────────────────────────────────────────────────

/// Statistiques du processus de writeback.
#[derive(Clone, Copy, Debug, Default)]
pub struct WritebackStats {
    pub entries_processed: u64,
    pub entries_ok:        u64,
    pub entries_failed:    u64,
    pub retries_total:     u64,
    pub bytes_written:     u64,
}

impl WritebackStats {
    pub fn new() -> Self { Self::default() }
    pub fn is_clean(&self) -> bool { self.entries_failed == 0 }
    pub fn reset(&mut self) { *self = Self::new(); }
}

// ─── WritebackWorker ─────────────────────────────────────────────────────────

/// Travailleur de writeback : consume la queue + appelle un sink.
///
/// RECUR-01 : toutes les boucles while.
pub struct WritebackWorker {
    config: WritebackConfig,
    stats:  WritebackStats,
}

impl WritebackWorker {
    pub fn new(config: WritebackConfig) -> ExofsResult<Self> {
        config.validate()?;
        Ok(Self { config, stats: WritebackStats::new() })
    }

    /// Traite une entrée depuis la queue.
    ///
    /// V-28 / LOCK-05 : `dequeue()` libère le spinlock FS (N4) avant de retourner ;
    /// `write_fn` est donc appelée sans aucun lock FS tenu → aucun risque
    /// d'inversion N4→N3 lors d'une éventuelle allocation de frames physiques.
    pub fn process_one(&mut self, queue: &WritebackQueue, write_fn: &mut dyn FnMut(&[u8; 32], u32) -> ExofsResult<u64>) -> ExofsResult<bool> {
        // V-28 / LOCK-05 : déqueue hors lock, puis appel write_fn hors lock FS.
        let entry = match queue.dequeue() {
            Some(e) => e,
            None => return Ok(false),
        };
        // Aucun lock FS n'est tenu à partir d'ici — conforme LOCK-05 / FS-PREALLOC.
        self.stats.entries_processed = self.stats.entries_processed.saturating_add(1);
        match write_fn(&entry.blob_id, entry.size) {
            Ok(bytes) => {
                self.stats.entries_ok = self.stats.entries_ok.saturating_add(1);
                self.stats.bytes_written = self.stats.bytes_written.saturating_add(bytes);
                Ok(true)
            }
            Err(_) => {
                self.stats.entries_failed = self.stats.entries_failed.saturating_add(1);
                // Si max_retries non atteint, réenqueue
                if entry.retries < self.config.max_retries {
                    let mut retry = entry;
                    retry.inc_retries();
                    self.stats.retries_total = self.stats.retries_total.saturating_add(1);
                    queue.enqueue(retry)?;
                }
                Ok(true)
            }
        }
    }

    /// Flush un lot complet (RECUR-01 : while).
    ///
    /// V-28 / LOCK-05 / FS-PREALLOC : aucun lock FS n'est acquis dans cette
    /// fonction avant les appels `write_fn`; la queue spinlock est obtenu et
    /// relâché uniquement le temps de `dequeue()`. La politique est donc :
    /// "pré-libération du lock FS avant toute allocation de frames physiques".
    pub fn flush_batch(&mut self, queue: &WritebackQueue, write_fn: &mut dyn FnMut(&[u8; 32], u32) -> ExofsResult<u64>) -> ExofsResult<u32> {
        let mut done = 0u32;
        let batch = self.config.flush_batch_size;
        while done < batch && !queue.is_empty() {
            self.process_one(queue, write_fn)?;
            done = done.saturating_add(1);
        }
        Ok(done)
    }

    pub fn stats(&self) -> &WritebackStats { &self.stats }
    pub fn reset_stats(&mut self) { self.stats.reset(); }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn make_id(n: u8) -> [u8; 32] { let mut id = [0u8; 32]; id[0] = n; id }

    #[test]
    fn test_queue_enqueue_dequeue() {
        let q = WritebackQueue::new_const();
        q.enqueue(WritebackEntry::new(make_id(1), 0, 512, 128)).expect("ok");
        assert_eq!(q.pending_count(), 1);
        let e = q.dequeue().expect("some");
        assert_eq!(e.blob_id[0], 1);
        assert!(q.is_empty());
    }

    #[test]
    fn test_queue_peek() {
        let q = WritebackQueue::new_const();
        q.enqueue(WritebackEntry::new(make_id(2), 0, 256, 100)).expect("ok");
        let peek = q.peek_next().expect("some");
        assert_eq!(peek.blob_id[0], 2);
        assert_eq!(q.pending_count(), 1); // pas consommé
    }

    #[test]
    fn test_queue_collect_urgent() {
        let q = WritebackQueue::new_const();
        q.enqueue(WritebackEntry::new(make_id(1), 0, 100, 0)).expect("ok"); // age=1000 > 500
        q.enqueue(WritebackEntry::new(make_id(2), 900, 100, 0)).expect("ok"); //age=100 < 500
        let mut urgent = Vec::new();
        q.collect_urgent(1000, 500, &mut urgent).expect("ok");
        assert_eq!(urgent.len(), 1);
        assert_eq!(urgent[0].blob_id[0], 1);
    }

    #[test]
    fn test_config_validate() {
        let mut cfg = WritebackConfig::default();
        assert!(cfg.validate().is_ok());
        cfg.max_queue_depth = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_worker_process_one() {
        let q = WritebackQueue::new_const();
        q.enqueue(WritebackEntry::new(make_id(5), 0, 128, 0)).expect("ok");
        let cfg = WritebackConfig::default();
        let mut worker = WritebackWorker::new(cfg).expect("ok");
        let mut write_fn = |_id: &[u8; 32], size: u32| -> ExofsResult<u64> { Ok(size as u64) };
        let done = worker.process_one(&q, &mut write_fn).expect("ok");
        assert!(done);
        assert_eq!(worker.stats().entries_ok, 1);
    }

    #[test]
    fn test_worker_retry_on_error() {
        let q = WritebackQueue::new_const();
        q.enqueue(WritebackEntry::new(make_id(6), 0, 64, 0)).expect("ok");
        let cfg = WritebackConfig::default();
        let mut worker = WritebackWorker::new(cfg).expect("ok");
        let mut write_fn = |_id: &[u8; 32], _size: u32| -> ExofsResult<u64> { Err(ExofsError::IoError) };
        worker.process_one(&q, &mut write_fn).expect("ok");
        assert_eq!(worker.stats().entries_failed, 1);
        assert_eq!(worker.stats().retries_total, 1);
        assert_eq!(q.pending_count(), 1); // réenqueué
    }

    #[test]
    fn test_worker_flush_batch() {
        let q = WritebackQueue::new_const();
        for i in 0u8..4 {
            q.enqueue(WritebackEntry::new(make_id(i), 0, 64, 0)).expect("ok");
        }
        let cfg = WritebackConfig { flush_batch_size: 4, ..WritebackConfig::default() };
        let mut worker = WritebackWorker::new(cfg).expect("ok");
        let mut write_fn = |_: &[u8; 32], size: u32| -> ExofsResult<u64> { Ok(size as u64) };
        let done = worker.flush_batch(&q, &mut write_fn).expect("ok");
        assert_eq!(done, 4);
        assert!(q.is_empty());
    }

    #[test]
    fn test_entry_age() {
        let e = WritebackEntry::new(make_id(1), 100, 0, 0);
        assert_eq!(e.age_ticks(200), 100);
        assert_eq!(e.age_ticks(50), 0); // saturating
    }

    #[test]
    fn test_stats_is_clean() {
        let mut stats = WritebackStats::new();
        assert!(stats.is_clean());
        stats.entries_failed = 1;
        assert!(!stats.is_clean());
    }

    #[test]
    fn test_queue_is_empty_when_dequeued() {
        let q = WritebackQueue::new_const();
        assert!(q.is_empty());
        assert!(q.dequeue().is_none());
    }
}

// ─── Helpers de writeback ───────────────────────────────────────────────────

/// Calcule les bytes sales totaux dans un vecteur d'entrées.
pub fn total_dirty_bytes(entries: &[WritebackEntry]) -> u64 {
    let mut total = 0u64;
    let mut i = 0usize;
    while i < entries.len() {
        total = total.saturating_add(entries[i].size as u64);
        i = i.wrapping_add(1);
    }
    total
}

/// Trie un vecteur d'entrées par priorité décroissante (bubble sort; RECUR-01).
pub fn sort_by_priority(entries: &mut [WritebackEntry]) {
    let n = entries.len();
    if n < 2 { return; }
    let mut swapped = true;
    while swapped {
        swapped = false;
        let mut i = 0usize;
        while i.saturating_add(1) < n {
            if entries[i].priority < entries[i.wrapping_add(1)].priority {
                entries.swap(i, i.wrapping_add(1));
                swapped = true;
            }
            i = i.wrapping_add(1);
        }
    }
}

/// Deduplique les entrées par blob_id (conserve la plus récente).
pub fn dedup_entries(entries: &mut Vec<WritebackEntry>) -> ExofsResult<()> {
    let mut result: Vec<WritebackEntry> = Vec::new();
    let mut i = 0usize;
    while i < entries.len() {
        let id = entries[i].blob_id;
        let mut found = false;
        let mut j = 0usize;
        while j < result.len() {
            if result[j].blob_id == id {
                if entries[i].dirty_since > result[j].dirty_since {
                    result[j] = entries[i];
                }
                found = true;
                break;
            }
            j = j.wrapping_add(1);
        }
        if !found {
            result.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            result.push(entries[i]);
        }
        i = i.wrapping_add(1);
    }
    *entries = result;
    Ok(())
}

// ─── Tests helpers ──────────────────────────────────────────────────────────
#[cfg(test)]
mod tests_helpers {
    use super::*;

    fn make_id(n: u8) -> [u8; 32] { let mut id = [0u8; 32]; id[0] = n; id }

    #[test]
    fn test_total_dirty_bytes() {
        let entries = [
            WritebackEntry::new(make_id(1), 0, 512, 0),
            WritebackEntry::new(make_id(2), 0, 1024, 0),
        ];
        assert_eq!(total_dirty_bytes(&entries), 1536);
    }

    #[test]
    fn test_sort_by_priority() {
        let mut entries = [
            WritebackEntry::new(make_id(1), 0, 16, 10),
            WritebackEntry::new(make_id(2), 0, 16, 200),
            WritebackEntry::new(make_id(3), 0, 16, 50),
        ];
        sort_by_priority(&mut entries);
        assert_eq!(entries[0].priority, 200);
        assert_eq!(entries[1].priority, 50);
        assert_eq!(entries[2].priority, 10);
    }

    #[test]
    fn test_dedup_entries() {
        let mut entries = Vec::new();
        entries.push(WritebackEntry::new(make_id(1), 10, 32, 0));
        entries.push(WritebackEntry::new(make_id(1), 20, 64, 0)); // plus récent
        entries.push(WritebackEntry::new(make_id(2), 5, 128, 0));
        dedup_entries(&mut entries).expect("ok");
        assert_eq!(entries.len(), 2);
        let has_id1 = entries.iter().any(|e| e.blob_id[0] == 1 && e.dirty_since == 20);
        assert!(has_id1);
    }
}
