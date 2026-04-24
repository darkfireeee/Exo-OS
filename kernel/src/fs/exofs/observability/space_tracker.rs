//! space_tracker.rs — Suivi de l'espace disque ExoFS (no_std).
//!
//! Fournit :
//!  - `SpaceZone`           : zone logique (data / metadata / gc_reserve).
//!  - `SpaceZoneStats`      : stats par zone.
//!  - `SpaceTracker`        : singleton de suivi global.
//!  - `SpaceSnapshot`       : snapshot immutable.
//!  - `SpaceQuota`          : quotas configurables.
//!  - `FragmentationInfo`   : estimation de la fragmentation.
//!  - `SPACE_TRACKER`       : singleton global.
//!
//! RECUR-01 : while uniquement.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_*.

extern crate alloc;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

// ─── SpaceZone ───────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaceZone {
    Data = 0,
    Metadata = 1,
    GcReserve = 2,
    Journal = 3,
}

impl SpaceZone {
    pub const COUNT: usize = 4;

    pub fn name(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Metadata => "metadata",
            Self::GcReserve => "gc_reserve",
            Self::Journal => "journal",
        }
    }
}

// ─── SpaceZoneStats ──────────────────────────────────────────────────────────

/// Statistiques par zone (en blocs).
pub struct SpaceZoneStats {
    total: AtomicU64,
    used: AtomicU64,
    reserved: AtomicU64,
}

impl SpaceZoneStats {
    pub const fn new_const() -> Self {
        Self {
            total: AtomicU64::new(0),
            used: AtomicU64::new(0),
            reserved: AtomicU64::new(0),
        }
    }

    pub fn init(&self, total: u64, reserved: u64) {
        self.total.store(total, Ordering::Relaxed);
        self.reserved.store(reserved, Ordering::Relaxed);
        self.used.store(0, Ordering::Relaxed);
    }

    pub fn alloc(&self, n: u64) {
        self.used.fetch_add(n, Ordering::Relaxed);
    }

    pub fn free(&self, n: u64) {
        let cur = self.used.load(Ordering::Relaxed);
        self.used.store(cur.saturating_sub(n), Ordering::Relaxed);
    }

    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }
    pub fn reserved(&self) -> u64 {
        self.reserved.load(Ordering::Relaxed)
    }

    pub fn free_blocks(&self) -> u64 {
        self.total()
            .saturating_sub(self.used())
            .saturating_sub(self.reserved())
    }

    pub fn usage_pct(&self) -> u8 {
        let t = self.total();
        if t == 0 {
            return 0;
        }
        (self
            .used()
            .saturating_mul(100)
            .checked_div(t)
            .unwrap_or(0)
            .min(100)) as u8
    }
}

// ─── SpaceTracker ────────────────────────────────────────────────────────────

/// Suivi global de l'espace disque par zone.
pub struct SpaceTracker {
    zones: [SpaceZoneStats; SpaceZone::COUNT],
    block_size: AtomicU64,
    total_extents: AtomicU64,
    small_extents: AtomicU64, // extents < 4 blocs → indicateur fragmentation
}

unsafe impl Sync for SpaceTracker {}
unsafe impl Send for SpaceTracker {}

impl SpaceTracker {
    pub const fn new_const() -> Self {
        const Z: SpaceZoneStats = SpaceZoneStats {
            total: AtomicU64::new(0),
            used: AtomicU64::new(0),
            reserved: AtomicU64::new(0),
        };
        Self {
            zones: [Z, Z, Z, Z],
            block_size: AtomicU64::new(4096),
            total_extents: AtomicU64::new(0),
            small_extents: AtomicU64::new(0),
        }
    }

    /// Initialise les zones.
    pub fn init(
        &self,
        data_total: u64,
        meta_total: u64,
        gc_reserve: u64,
        journal: u64,
        block_size: u64,
    ) {
        self.zones[SpaceZone::Data as usize].init(data_total, 0);
        self.zones[SpaceZone::Metadata as usize].init(meta_total, 0);
        self.zones[SpaceZone::GcReserve as usize].init(gc_reserve, gc_reserve);
        self.zones[SpaceZone::Journal as usize].init(journal, 0);
        self.block_size.store(block_size, Ordering::Relaxed);
    }

    pub fn zone(&self, z: SpaceZone) -> &SpaceZoneStats {
        &self.zones[z as usize]
    }

    pub fn alloc_blocks(&self, zone: SpaceZone, n: u64) {
        self.zones[zone as usize].alloc(n);
    }

    pub fn free_blocks(&self, zone: SpaceZone, n: u64) {
        self.zones[zone as usize].free(n);
    }

    pub fn block_size(&self) -> u64 {
        self.block_size.load(Ordering::Relaxed)
    }

    /// Total blocs utilisés toutes zones confondues (RECUR-01 : while).
    pub fn total_used_blocks(&self) -> u64 {
        let mut total = 0u64;
        let mut i = 0usize;
        while i < SpaceZone::COUNT {
            total = total.saturating_add(self.zones[i].used());
            i = i.wrapping_add(1);
        }
        total
    }

    /// Total blocs disponibles (RECUR-01 : while).
    pub fn total_free_blocks(&self) -> u64 {
        let mut total = 0u64;
        let mut i = 0usize;
        while i < SpaceZone::COUNT {
            total = total.saturating_add(self.zones[i].free_blocks());
            i = i.wrapping_add(1);
        }
        total
    }

    /// Utilisation globale en % (ARITH-02).
    pub fn usage_pct(&self) -> u8 {
        let mut all_total = 0u64;
        let mut all_used = 0u64;
        let mut i = 0usize;
        while i < SpaceZone::COUNT {
            all_total = all_total.saturating_add(self.zones[i].total());
            all_used = all_used.saturating_add(self.zones[i].used());
            i = i.wrapping_add(1);
        }
        if all_total == 0 {
            return 0;
        }
        (all_used
            .saturating_mul(100)
            .checked_div(all_total)
            .unwrap_or(0)
            .min(100)) as u8
    }

    /// Bytes utilisés (ARITH-02).
    pub fn used_bytes(&self) -> u64 {
        self.total_used_blocks().saturating_mul(self.block_size())
    }

    /// Bytes libres.
    pub fn free_bytes(&self) -> u64 {
        self.total_free_blocks().saturating_mul(self.block_size())
    }

    /// Enregistre un extent alloué.
    pub fn track_extent(&self, blocks: u64) {
        self.total_extents.fetch_add(1, Ordering::Relaxed);
        if blocks < 4 {
            self.small_extents.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Estimation de la fragmentation * 1000 (ARITH-02).
    pub fn fragmentation_pct10(&self) -> u64 {
        let t = self.total_extents.load(Ordering::Relaxed);
        let s = self.small_extents.load(Ordering::Relaxed);
        s.saturating_mul(1000).checked_div(t).unwrap_or(0)
    }

    /// Snapshot global.
    pub fn snapshot(&self) -> SpaceSnapshot {
        let mut zones = [(0u64, 0u64, 0u64); SpaceZone::COUNT];
        let mut i = 0usize;
        while i < SpaceZone::COUNT {
            zones[i] = (
                self.zones[i].total(),
                self.zones[i].used(),
                self.zones[i].reserved(),
            );
            i = i.wrapping_add(1);
        }
        SpaceSnapshot {
            zones,
            block_size: self.block_size(),
            total_extents: self.total_extents.load(Ordering::Relaxed),
            small_extents: self.small_extents.load(Ordering::Relaxed),
            usage_pct: self.usage_pct(),
        }
    }
}

pub static SPACE_TRACKER: SpaceTracker = SpaceTracker::new_const();

// ─── SpaceSnapshot ───────────────────────────────────────────────────────────

/// Snapshot immutable de l'état d'espace.
#[derive(Clone, Copy, Debug)]
pub struct SpaceSnapshot {
    /// (total, used, reserved) par zone.
    pub zones: [(u64, u64, u64); SpaceZone::COUNT],
    pub block_size: u64,
    pub total_extents: u64,
    pub small_extents: u64,
    pub usage_pct: u8,
}

impl SpaceSnapshot {
    pub fn zone_usage_pct(&self, z: SpaceZone) -> u8 {
        let (total, used, _) = self.zones[z as usize];
        if total == 0 {
            return 0;
        }
        (used
            .saturating_mul(100)
            .checked_div(total)
            .unwrap_or(0)
            .min(100)) as u8
    }

    pub fn fragmentation_pct10(&self) -> u64 {
        self.small_extents
            .saturating_mul(1000)
            .checked_div(self.total_extents.max(1))
            .unwrap_or(0)
    }
}

// ─── SpaceQuota ──────────────────────────────────────────────────────────────

/// Quotas d'utilisation par zone.
#[derive(Clone, Copy, Debug)]
pub struct SpaceQuota {
    pub zone: SpaceZone,
    pub max_blocks: u64,
    pub soft_limit: u64,
}

impl SpaceQuota {
    pub fn check(&self, used: u64) -> ExofsResult<()> {
        if used > self.max_blocks {
            return Err(ExofsError::QuotaExceeded);
        }
        Ok(())
    }

    pub fn soft_exceeded(&self, used: u64) -> bool {
        used > self.soft_limit
    }
}

// ─── FragmentationInfo ───────────────────────────────────────────────────────

/// Informations de fragmentation calculées.
#[derive(Clone, Copy, Debug, Default)]
pub struct FragmentationInfo {
    pub total_extents: u64,
    pub small_extents: u64,
    pub frag_pct10: u64,
}

impl FragmentationInfo {
    pub fn from_tracker(t: &SpaceTracker) -> Self {
        let total = t.total_extents.load(Ordering::Relaxed);
        let small = t.small_extents.load(Ordering::Relaxed);
        let pct10 = small
            .saturating_mul(1000)
            .checked_div(total.max(1))
            .unwrap_or(0);
        Self {
            total_extents: total,
            small_extents: small,
            frag_pct10: pct10,
        }
    }

    pub fn needs_defrag(&self, threshold_pct10: u64) -> bool {
        self.frag_pct10 > threshold_pct10
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zone_alloc_free() {
        let t = SpaceTracker::new_const();
        t.init(1000, 200, 50, 100, 4096);
        t.alloc_blocks(SpaceZone::Data, 100);
        assert_eq!(t.zone(SpaceZone::Data).used(), 100);
        t.free_blocks(SpaceZone::Data, 50);
        assert_eq!(t.zone(SpaceZone::Data).used(), 50);
    }

    #[test]
    fn test_usage_pct() {
        let t = SpaceTracker::new_const();
        t.init(100, 0, 0, 0, 4096);
        t.alloc_blocks(SpaceZone::Data, 50);
        assert_eq!(t.zone(SpaceZone::Data).usage_pct(), 50);
    }

    #[test]
    fn test_total_used_blocks() {
        let t = SpaceTracker::new_const();
        t.init(1000, 200, 0, 0, 4096);
        t.alloc_blocks(SpaceZone::Data, 100);
        t.alloc_blocks(SpaceZone::Metadata, 20);
        assert_eq!(t.total_used_blocks(), 120);
    }

    #[test]
    fn test_fragmentation() {
        let t = SpaceTracker::new_const();
        t.track_extent(2); // small
        t.track_extent(8); // large
                           // 1/2 * 1000 = 500
        assert_eq!(t.fragmentation_pct10(), 500);
    }

    #[test]
    fn test_snapshot_zone_usage() {
        let t = SpaceTracker::new_const();
        t.init(100, 0, 0, 0, 4096);
        t.alloc_blocks(SpaceZone::Data, 80);
        let snap = t.snapshot();
        assert_eq!(snap.zone_usage_pct(SpaceZone::Data), 80);
    }

    #[test]
    fn test_quota_check() {
        let q = SpaceQuota {
            zone: SpaceZone::Data,
            max_blocks: 100,
            soft_limit: 80,
        };
        assert!(q.check(50).is_ok());
        assert!(q.check(101).is_err());
        assert!(!q.soft_exceeded(70));
        assert!(q.soft_exceeded(90));
    }

    #[test]
    fn test_fragmentation_info_needs_defrag() {
        let info = FragmentationInfo {
            frag_pct10: 400,
            ..Default::default()
        };
        assert!(info.needs_defrag(300));
        assert!(!info.needs_defrag(500));
    }

    #[test]
    fn test_block_size() {
        let t = SpaceTracker::new_const();
        t.init(100, 0, 0, 0, 512);
        assert_eq!(t.block_size(), 512);
    }

    #[test]
    fn test_used_bytes() {
        let t = SpaceTracker::new_const();
        t.init(1000, 0, 0, 0, 4096);
        t.alloc_blocks(SpaceZone::Data, 10);
        assert_eq!(t.used_bytes(), 10 * 4096);
    }

    #[test]
    fn test_zone_name() {
        assert_eq!(SpaceZone::Data.name(), "data");
        assert_eq!(SpaceZone::GcReserve.name(), "gc_reserve");
    }
}

// ─── SpaceHistory ────────────────────────────────────────────────────────────

pub const SPACE_HISTORY_SIZE: usize = 32;

/// Ring de snapshots d'utilisation pour suivi de tendance.
pub struct SpaceHistory {
    usage_pct: [core::sync::atomic::AtomicU8; SPACE_HISTORY_SIZE],
    ticks: [AtomicU64; SPACE_HISTORY_SIZE],
    head: AtomicU64,
    count: AtomicU64,
}

unsafe impl Sync for SpaceHistory {}

impl SpaceHistory {
    pub const fn new_const() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const ZU8: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
        #[allow(clippy::declare_interior_mutable_const)]
        const ZU64: AtomicU64 = AtomicU64::new(0);
        Self {
            usage_pct: [ZU8; SPACE_HISTORY_SIZE],
            ticks: [ZU64; SPACE_HISTORY_SIZE],
            head: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    pub fn record(&self, pct: u8, tick: u64) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % SPACE_HISTORY_SIZE;
        self.usage_pct[idx].store(pct, Ordering::Relaxed);
        self.ticks[idx].store(tick, Ordering::Relaxed);
        let c = self.count.load(Ordering::Relaxed);
        if c < SPACE_HISTORY_SIZE as u64 {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Moyenne d'utilisation sur les n derniers points (ARITH-02 / RECUR-01).
    pub fn average_pct(&self) -> u8 {
        let n = self.count.load(Ordering::Relaxed);
        if n == 0 {
            return 0;
        }
        let mut sum = 0u64;
        let head = self.head.load(Ordering::Relaxed) as usize;
        let mut i = 0usize;
        while i < n as usize {
            let idx = (head
                .wrapping_add(SPACE_HISTORY_SIZE)
                .wrapping_sub(i)
                .wrapping_sub(1))
                % SPACE_HISTORY_SIZE;
            sum = sum.saturating_add(self.usage_pct[idx].load(Ordering::Relaxed) as u64);
            i = i.wrapping_add(1);
        }
        (sum.checked_div(n).unwrap_or(0).min(100)) as u8
    }

    /// Trend: retourne true si l'utilisation augmente.
    pub fn is_growing(&self) -> bool {
        let n = self.count.load(Ordering::Relaxed) as usize;
        if n < 2 {
            return false;
        }
        let head = self.head.load(Ordering::Relaxed) as usize;
        let last = self.usage_pct
            [(head.wrapping_add(SPACE_HISTORY_SIZE).wrapping_sub(1)) % SPACE_HISTORY_SIZE]
            .load(Ordering::Relaxed);
        let prev = self.usage_pct
            [(head.wrapping_add(SPACE_HISTORY_SIZE).wrapping_sub(2)) % SPACE_HISTORY_SIZE]
            .load(Ordering::Relaxed);
        last > prev
    }

    /// Copie les points dans un Vec (OOM-02).
    pub fn to_vec(&self) -> ExofsResult<Vec<u8>> {
        let n = self.count.load(Ordering::Relaxed) as usize;
        let mut v = Vec::new();
        v.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        let head = self.head.load(Ordering::Relaxed) as usize;
        let mut i = 0usize;
        while i < n {
            let idx = (head
                .wrapping_add(SPACE_HISTORY_SIZE)
                .wrapping_sub(i)
                .wrapping_sub(1))
                % SPACE_HISTORY_SIZE;
            v.push(self.usage_pct[idx].load(Ordering::Relaxed));
            i = i.wrapping_add(1);
        }
        Ok(v)
    }
}

pub static SPACE_HISTORY: SpaceHistory = SpaceHistory::new_const();

#[cfg(test)]
mod tests_history {
    use super::*;

    #[test]
    fn test_history_record_average() {
        let h = SpaceHistory::new_const();
        h.record(40, 1);
        h.record(60, 2);
        h.record(80, 3);
        assert_eq!(h.average_pct(), 60);
    }

    #[test]
    fn test_history_is_growing() {
        let h = SpaceHistory::new_const();
        h.record(50, 1);
        h.record(70, 2);
        assert!(h.is_growing());
        let h2 = SpaceHistory::new_const();
        h2.record(70, 1);
        h2.record(50, 2);
        assert!(!h2.is_growing());
    }

    #[test]
    fn test_history_to_vec() {
        let h = SpaceHistory::new_const();
        h.record(10, 1);
        h.record(20, 2);
        let v = h.to_vec().expect("ok");
        assert_eq!(v.len(), 2);
    }
}
