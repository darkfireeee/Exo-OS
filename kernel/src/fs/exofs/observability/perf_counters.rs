//! perf_counters.rs — Compteurs de performance matériels/logiciels ExoFS (no_std).
//!
//! Fournit :
//!  - `PerfCounterId`    : identifiant de compteur (enum 16 variants).
//!  - `PerfCounterSet`   : set de compteurs atomiques.
//!  - `PerfSnapshot`     : snapshot immutable.
//!  - `PerfDelta`        : différence entre deux snapshots.
//!  - `PerfRateWindow`   : ring pour calcul de taux glissant.
//!  - `PERF_COUNTERS`    : singleton global.
//!
//! RECUR-01 : while uniquement.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_*.

extern crate alloc;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

// ─── PerfCounterId ───────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PerfCounterId {
    PageFaults = 0,
    CacheMisses = 1,
    CacheHits = 2,
    BlockReads = 3,
    BlockWrites = 4,
    BlockReadBytes = 5,
    BlockWriteBytes = 6,
    MetadataReads = 7,
    MetadataWrites = 8,
    InodeAllocs = 9,
    InodeFrees = 10,
    ExtentMerges = 11,
    ExtentSplits = 12,
    BTreeSearches = 13,
    BTreeInserts = 14,
    BTreeDeletes = 15,
}

impl PerfCounterId {
    pub const COUNT: usize = 16;

    pub fn name(self) -> &'static str {
        match self {
            Self::PageFaults => "page_faults",
            Self::CacheMisses => "cache_misses",
            Self::CacheHits => "cache_hits",
            Self::BlockReads => "block_reads",
            Self::BlockWrites => "block_writes",
            Self::BlockReadBytes => "block_read_bytes",
            Self::BlockWriteBytes => "block_write_bytes",
            Self::MetadataReads => "metadata_reads",
            Self::MetadataWrites => "metadata_writes",
            Self::InodeAllocs => "inode_allocs",
            Self::InodeFrees => "inode_frees",
            Self::ExtentMerges => "extent_merges",
            Self::ExtentSplits => "extent_splits",
            Self::BTreeSearches => "btree_searches",
            Self::BTreeInserts => "btree_inserts",
            Self::BTreeDeletes => "btree_deletes",
        }
    }

    pub fn is_byte_counter(self) -> bool {
        matches!(self, Self::BlockReadBytes | Self::BlockWriteBytes)
    }
}

// ─── PerfCounterSet ──────────────────────────────────────────────────────────

/// Set de compteurs atomiques.
pub struct PerfCounterSet {
    counters: [AtomicU64; PerfCounterId::COUNT],
}

unsafe impl Sync for PerfCounterSet {}
unsafe impl Send for PerfCounterSet {}

impl PerfCounterSet {
    pub const fn new_const() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const Z: AtomicU64 = AtomicU64::new(0);
        Self {
            counters: [Z; PerfCounterId::COUNT],
        }
    }

    pub fn add(&self, id: PerfCounterId, n: u64) {
        self.counters[id as usize].fetch_add(n, Ordering::Relaxed);
    }

    pub fn inc(&self, id: PerfCounterId) {
        self.add(id, 1);
    }

    pub fn get(&self, id: PerfCounterId) -> u64 {
        self.counters[id as usize].load(Ordering::Relaxed)
    }

    // ── Helpers sémantiques ──────────────────────────────────────────────────

    pub fn inc_page_fault(&self) {
        self.inc(PerfCounterId::PageFaults);
    }
    pub fn inc_cache_miss(&self) {
        self.inc(PerfCounterId::CacheMisses);
    }
    pub fn inc_cache_hit(&self) {
        self.inc(PerfCounterId::CacheHits);
    }
    pub fn inc_block_read(&self, bytes: u64) {
        self.inc(PerfCounterId::BlockReads);
        self.add(PerfCounterId::BlockReadBytes, bytes);
    }
    pub fn inc_block_write(&self, bytes: u64) {
        self.inc(PerfCounterId::BlockWrites);
        self.add(PerfCounterId::BlockWriteBytes, bytes);
    }
    pub fn inc_metadata_read(&self) {
        self.inc(PerfCounterId::MetadataReads);
    }
    pub fn inc_metadata_write(&self) {
        self.inc(PerfCounterId::MetadataWrites);
    }
    pub fn inc_inode_alloc(&self) {
        self.inc(PerfCounterId::InodeAllocs);
    }
    pub fn inc_inode_free(&self) {
        self.inc(PerfCounterId::InodeFrees);
    }
    pub fn inc_extent_merge(&self) {
        self.inc(PerfCounterId::ExtentMerges);
    }
    pub fn inc_extent_split(&self) {
        self.inc(PerfCounterId::ExtentSplits);
    }
    pub fn inc_btree_search(&self) {
        self.inc(PerfCounterId::BTreeSearches);
    }
    pub fn inc_btree_insert(&self) {
        self.inc(PerfCounterId::BTreeInserts);
    }
    pub fn inc_btree_delete(&self) {
        self.inc(PerfCounterId::BTreeDeletes);
    }

    /// Cache efficiency = hits / (hits + misses) * 1000 (ARITH-02).
    pub fn cache_efficiency_pct10(&self) -> u64 {
        let h = self.get(PerfCounterId::CacheHits);
        let m = self.get(PerfCounterId::CacheMisses);
        let t = h.saturating_add(m);
        h.saturating_mul(1000).checked_div(t).unwrap_or(0)
    }

    /// Ratio metadata/block I/O * 1000 (ARITH-02).
    pub fn metadata_ratio_pct10(&self) -> u64 {
        let meta = self
            .get(PerfCounterId::MetadataReads)
            .saturating_add(self.get(PerfCounterId::MetadataWrites));
        let block = self
            .get(PerfCounterId::BlockReads)
            .saturating_add(self.get(PerfCounterId::BlockWrites));
        let total = meta.saturating_add(block);
        meta.saturating_mul(1000).checked_div(total).unwrap_or(0)
    }

    /// Snapshot atomique.
    pub fn snapshot(&self) -> PerfSnapshot {
        let mut values = [0u64; PerfCounterId::COUNT];
        let mut i = 0usize;
        while i < PerfCounterId::COUNT {
            values[i] = self.counters[i].load(Ordering::Relaxed);
            i = i.wrapping_add(1);
        }
        PerfSnapshot { values }
    }

    /// Remet à zéro.
    pub fn reset(&self) {
        let mut i = 0usize;
        while i < PerfCounterId::COUNT {
            self.counters[i].store(0, Ordering::Relaxed);
            i = i.wrapping_add(1);
        }
    }
}

pub static PERF_COUNTERS: PerfCounterSet = PerfCounterSet::new_const();

// ─── PerfSnapshot ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct PerfSnapshot {
    pub values: [u64; PerfCounterId::COUNT],
}

impl PerfSnapshot {
    pub fn get(&self, id: PerfCounterId) -> u64 {
        self.values[id as usize]
    }

    pub fn diff(&self, prev: &PerfSnapshot) -> PerfDelta {
        let mut delta = [0u64; PerfCounterId::COUNT];
        let mut i = 0usize;
        while i < PerfCounterId::COUNT {
            delta[i] = self.values[i].saturating_sub(prev.values[i]);
            i = i.wrapping_add(1);
        }
        PerfDelta { delta }
    }

    pub fn total_io_ops(&self) -> u64 {
        self.get(PerfCounterId::BlockReads)
            .saturating_add(self.get(PerfCounterId::BlockWrites))
    }

    pub fn total_io_bytes(&self) -> u64 {
        self.get(PerfCounterId::BlockReadBytes)
            .saturating_add(self.get(PerfCounterId::BlockWriteBytes))
    }
}

// ─── PerfDelta ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct PerfDelta {
    pub delta: [u64; PerfCounterId::COUNT],
}

impl PerfDelta {
    pub fn get(&self, id: PerfCounterId) -> u64 {
        self.delta[id as usize]
    }

    pub fn iops(&self) -> u64 {
        self.get(PerfCounterId::BlockReads)
            .saturating_add(self.get(PerfCounterId::BlockWrites))
    }

    pub fn throughput_bytes(&self) -> u64 {
        self.get(PerfCounterId::BlockReadBytes)
            .saturating_add(self.get(PerfCounterId::BlockWriteBytes))
    }
}

// ─── PerfRateWindow ──────────────────────────────────────────────────────────

pub const PERF_RATE_WINDOW: usize = 16;

/// Ring de snapshots horodatés pour calcul de taux glissant.
pub struct PerfRateWindow {
    slots: [AtomicU64; PERF_RATE_WINDOW], // iops par slot
    head: AtomicU64,
    count: AtomicU64,
}

unsafe impl Sync for PerfRateWindow {}

impl PerfRateWindow {
    pub const fn new_const() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const Z: AtomicU64 = AtomicU64::new(0);
        Self {
            slots: [Z; PERF_RATE_WINDOW],
            head: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    pub fn push_iops(&self, iops: u64) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % PERF_RATE_WINDOW;
        self.slots[idx].store(iops, Ordering::Relaxed);
        let c = self.count.load(Ordering::Relaxed);
        if c < PERF_RATE_WINDOW as u64 {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Moyenne glissante d'IOPS (ARITH-02 / RECUR-01).
    pub fn rolling_iops(&self) -> u64 {
        let n = self.count.load(Ordering::Relaxed);
        if n == 0 {
            return 0;
        }
        let mut sum = 0u64;
        let mut i = 0usize;
        while i < n as usize {
            sum = sum.saturating_add(self.slots[i].load(Ordering::Relaxed));
            i = i.wrapping_add(1);
        }
        sum.checked_div(n).unwrap_or(0)
    }

    /// Copie les slots dans un Vec (OOM-02).
    pub fn to_vec(&self) -> ExofsResult<Vec<u64>> {
        let n = self.count.load(Ordering::Relaxed) as usize;
        let mut v = Vec::new();
        v.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        let head = self.head.load(Ordering::Relaxed) as usize;
        let mut i = 0usize;
        while i < n {
            let idx = (head
                .wrapping_add(PERF_RATE_WINDOW)
                .wrapping_sub(i)
                .wrapping_sub(1))
                % PERF_RATE_WINDOW;
            v.push(self.slots[idx].load(Ordering::Relaxed));
            i = i.wrapping_add(1);
        }
        Ok(v)
    }
}

pub static PERF_RATE: PerfRateWindow = PerfRateWindow::new_const();

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inc_and_get() {
        let s = PerfCounterSet::new_const();
        s.inc_block_read(4096);
        assert_eq!(s.get(PerfCounterId::BlockReads), 1);
        assert_eq!(s.get(PerfCounterId::BlockReadBytes), 4096);
    }

    #[test]
    fn test_cache_efficiency() {
        let s = PerfCounterSet::new_const();
        s.inc_cache_hit();
        s.inc_cache_hit();
        s.inc_cache_miss();
        // 2/3 * 1000 = 666
        assert_eq!(s.cache_efficiency_pct10(), 666);
    }

    #[test]
    fn test_cache_efficiency_zero() {
        let s = PerfCounterSet::new_const();
        assert_eq!(s.cache_efficiency_pct10(), 0);
    }

    #[test]
    fn test_snapshot_diff() {
        let s = PerfCounterSet::new_const();
        let s0 = s.snapshot();
        s.inc_block_write(512);
        s.inc_block_write(512);
        let s1 = s.snapshot();
        let d = s1.diff(&s0);
        assert_eq!(d.get(PerfCounterId::BlockWrites), 2);
        assert_eq!(d.throughput_bytes(), 1024);
    }

    #[test]
    fn test_reset() {
        let s = PerfCounterSet::new_const();
        s.inc_page_fault();
        s.inc_page_fault();
        s.reset();
        assert_eq!(s.get(PerfCounterId::PageFaults), 0);
    }

    #[test]
    fn test_metadata_ratio() {
        let s = PerfCounterSet::new_const();
        s.inc_metadata_read();
        s.inc_metadata_read();
        s.inc_block_read(0);
        s.inc_block_read(0);
        s.inc_block_read(0);
        // 2/5 * 1000 = 400
        assert_eq!(s.metadata_ratio_pct10(), 400);
    }

    #[test]
    fn test_snapshot_total_io() {
        let s = PerfCounterSet::new_const();
        s.inc_block_read(1024);
        s.inc_block_write(2048);
        let snap = s.snapshot();
        assert_eq!(snap.total_io_ops(), 2);
        assert_eq!(snap.total_io_bytes(), 3072);
    }

    #[test]
    fn test_rate_window_push() {
        let w = PerfRateWindow::new_const();
        w.push_iops(100);
        w.push_iops(200);
        w.push_iops(300);
        assert_eq!(w.rolling_iops(), 200);
    }

    #[test]
    fn test_rate_window_to_vec() {
        let w = PerfRateWindow::new_const();
        w.push_iops(10);
        w.push_iops(20);
        let v = w.to_vec().expect("ok");
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn test_counter_id_name() {
        assert_eq!(PerfCounterId::PageFaults.name(), "page_faults");
        assert_eq!(PerfCounterId::BTreeSearches.name(), "btree_searches");
    }

    #[test]
    fn test_delta_iops() {
        let d = PerfDelta {
            delta: {
                let mut arr = [0u64; PerfCounterId::COUNT];
                arr[PerfCounterId::BlockReads as usize] = 10;
                arr[PerfCounterId::BlockWrites as usize] = 5;
                arr
            },
        };
        assert_eq!(d.iops(), 15);
    }

    #[test]
    fn test_btree_counters() {
        let s = PerfCounterSet::new_const();
        s.inc_btree_search();
        s.inc_btree_search();
        s.inc_btree_insert();
        s.inc_btree_delete();
        assert_eq!(s.get(PerfCounterId::BTreeSearches), 2);
        assert_eq!(s.get(PerfCounterId::BTreeInserts), 1);
        assert_eq!(s.get(PerfCounterId::BTreeDeletes), 1);
    }

    #[test]
    fn test_extent_counters() {
        let s = PerfCounterSet::new_const();
        s.inc_extent_merge();
        s.inc_extent_merge();
        s.inc_extent_split();
        assert_eq!(s.get(PerfCounterId::ExtentMerges), 2);
        assert_eq!(s.get(PerfCounterId::ExtentSplits), 1);
    }
}

// ─── PerfReport ──────────────────────────────────────────────────────────────

/// Rapport synthétique des compteurs de performance pour un intervalle.
#[derive(Clone, Copy, Debug, Default)]
pub struct PerfReport {
    pub iops: u64,
    pub throughput_bytes: u64,
    pub cache_eff_pct10: u64,
    pub meta_ratio_pct10: u64,
    pub page_faults: u64,
}

impl PerfReport {
    /// Construit un rapport depuis le delta entre deux snapshots.
    pub fn from_delta(set: &PerfCounterSet, delta: &PerfDelta) -> Self {
        Self {
            iops: delta.iops(),
            throughput_bytes: delta.throughput_bytes(),
            cache_eff_pct10: set.cache_efficiency_pct10(),
            meta_ratio_pct10: set.metadata_ratio_pct10(),
            page_faults: delta.get(PerfCounterId::PageFaults),
        }
    }

    pub fn is_healthy(&self, max_page_faults: u64, min_cache_eff: u64) -> bool {
        self.page_faults <= max_page_faults && self.cache_eff_pct10 >= min_cache_eff
    }
}

#[cfg(test)]
mod tests_report {
    use super::*;

    #[test]
    fn test_perf_report_from_delta() {
        let s = PerfCounterSet::new_const();
        s.inc_cache_hit();
        s.inc_cache_hit();
        s.inc_cache_miss();
        s.inc_block_read(512);
        s.inc_block_write(512);
        let snap0 = PerfSnapshot::default();
        let snap1 = s.snapshot();
        let delta = snap1.diff(&snap0);
        let report = PerfReport::from_delta(&s, &delta);
        assert_eq!(report.iops, 2);
        assert_eq!(report.throughput_bytes, 1024);
        assert_eq!(report.cache_eff_pct10, 666);
    }

    #[test]
    fn test_perf_report_is_healthy() {
        let r = PerfReport {
            page_faults: 0,
            cache_eff_pct10: 800,
            ..Default::default()
        };
        assert!(r.is_healthy(10, 500));
        assert!(!r.is_healthy(0, 500));
    }
}
