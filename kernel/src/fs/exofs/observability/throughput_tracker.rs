// SPDX-License-Identifier: MIT
// ExoFS Observability — Throughput Tracker
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── Horloge interne ─────────────────────────────────────────────────────────

static THROUGHPUT_TICK: AtomicU64 = AtomicU64::new(0);

pub fn advance_tick(delta: u64) {
    THROUGHPUT_TICK.fetch_add(delta, Ordering::Relaxed);
}
pub fn current_tick() -> u64 {
    THROUGHPUT_TICK.load(Ordering::Relaxed)
}

// ─── Constants ────────────────────────────────────────────────────────────────

pub const THROUGHPUT_WINDOW_SIZE: usize = 16;

// ─── ThroughputSample ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct ThroughputSample {
    pub tick:          u64,
    pub bytes_read:    u64,
    pub bytes_written: u64,
}

impl ThroughputSample {
    pub const fn zero() -> Self {
        Self { tick: 0, bytes_read: 0, bytes_written: 0 }
    }
    pub fn total_bytes(&self) -> u64 {
        self.bytes_read.saturating_add(self.bytes_written)
    }
    pub fn read_bpt(&self, dt: u64) -> u64 {
        self.bytes_read.checked_div(dt.max(1)).unwrap_or(0)
    }
    pub fn write_bpt(&self, dt: u64) -> u64 {
        self.bytes_written.checked_div(dt.max(1)).unwrap_or(0)
    }
    pub fn total_bpt(&self, dt: u64) -> u64 {
        self.read_bpt(dt).saturating_add(self.write_bpt(dt))
    }
}

// ─── ThroughputWindow ─────────────────────────────────────────────────────────

pub struct ThroughputWindow {
    samples:       UnsafeCell<[ThroughputSample; THROUGHPUT_WINDOW_SIZE]>,
    head:          AtomicU64,
    count:         AtomicU64,
    total_read:    AtomicU64,
    total_written: AtomicU64,
}

unsafe impl Sync for ThroughputWindow {}
unsafe impl Send for ThroughputWindow {}

impl ThroughputWindow {
    pub const fn new_const() -> Self {
        Self {
            samples:       UnsafeCell::new([ThroughputSample::zero(); THROUGHPUT_WINDOW_SIZE]),
            head:          AtomicU64::new(0),
            count:         AtomicU64::new(0),
            total_read:    AtomicU64::new(0),
            total_written: AtomicU64::new(0),
        }
    }

    pub fn push(&self, sample: ThroughputSample) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % THROUGHPUT_WINDOW_SIZE;
        unsafe { (*self.samples.get())[idx] = sample; }
        let n = self.count.load(Ordering::Relaxed);
        if n < THROUGHPUT_WINDOW_SIZE as u64 {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
        self.total_read.fetch_add(sample.bytes_read, Ordering::Relaxed);
        self.total_written.fetch_add(sample.bytes_written, Ordering::Relaxed);
    }

    pub fn avg_read_bpt(&self) -> u64 {
        let n = self.count.load(Ordering::Relaxed);
        if n == 0 { return 0; }
        let mut sum = 0u64;
        let head = self.head.load(Ordering::Relaxed) as usize;
        let mut i = 0usize;
        while i < n as usize {
            let idx = (head.wrapping_add(THROUGHPUT_WINDOW_SIZE).wrapping_sub(i + 1)) % THROUGHPUT_WINDOW_SIZE;
            sum = sum.saturating_add(unsafe { (*self.samples.get())[idx].bytes_read });
            i = i.wrapping_add(1);
        }
        sum.checked_div(n).unwrap_or(0)
    }

    pub fn avg_write_bpt(&self) -> u64 {
        let n = self.count.load(Ordering::Relaxed);
        if n == 0 { return 0; }
        let mut sum = 0u64;
        let head = self.head.load(Ordering::Relaxed) as usize;
        let mut i = 0usize;
        while i < n as usize {
            let idx = (head.wrapping_add(THROUGHPUT_WINDOW_SIZE).wrapping_sub(i + 1)) % THROUGHPUT_WINDOW_SIZE;
            sum = sum.saturating_add(unsafe { (*self.samples.get())[idx].bytes_written });
            i = i.wrapping_add(1);
        }
        sum.checked_div(n).unwrap_or(0)
    }

    pub fn avg_total_bpt(&self) -> u64 {
        self.avg_read_bpt().saturating_add(self.avg_write_bpt())
    }

    pub fn latest(&self) -> Option<ThroughputSample> {
        if self.count.load(Ordering::Relaxed) == 0 { return None; }
        let head = self.head.load(Ordering::Relaxed) as usize;
        let idx = (head.wrapping_add(THROUGHPUT_WINDOW_SIZE).wrapping_sub(1)) % THROUGHPUT_WINDOW_SIZE;
        Some(unsafe { (*self.samples.get())[idx] })
    }

    pub fn to_vec(&self) -> ExofsResult<Vec<ThroughputSample>> {
        let n = self.count.load(Ordering::Relaxed) as usize;
        let mut v = Vec::new();
        v.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        let head = self.head.load(Ordering::Relaxed) as usize;
        let mut i = 0usize;
        while i < n {
            let idx = (head.wrapping_add(THROUGHPUT_WINDOW_SIZE).wrapping_sub(i + 1)) % THROUGHPUT_WINDOW_SIZE;
            v.push(unsafe { (*self.samples.get())[idx] });
            i = i.wrapping_add(1);
        }
        Ok(v)
    }

    pub fn total_read_bytes(&self)    -> u64 { self.total_read.load(Ordering::Relaxed) }
    pub fn total_written_bytes(&self) -> u64 { self.total_written.load(Ordering::Relaxed) }

    pub fn reset(&self) {
        let mut i = 0usize;
        while i < THROUGHPUT_WINDOW_SIZE {
            unsafe { (*self.samples.get())[i] = ThroughputSample::zero(); }
            i = i.wrapping_add(1);
        }
        self.head.store(0, Ordering::Relaxed);
        self.count.store(0, Ordering::Relaxed);
        self.total_read.store(0, Ordering::Relaxed);
        self.total_written.store(0, Ordering::Relaxed);
    }
}

// ─── ThroughputTracker ────────────────────────────────────────────────────────

pub struct ThroughputTracker {
    bytes_read:      AtomicU64,
    bytes_written:   AtomicU64,
    read_ops:        AtomicU64,
    write_ops:       AtomicU64,
    peak_read_bpt:   AtomicU64,
    peak_write_bpt:  AtomicU64,
    window:          ThroughputWindow,
    last_flush_tick: AtomicU64,
}

unsafe impl Sync for ThroughputTracker {}
unsafe impl Send for ThroughputTracker {}

impl ThroughputTracker {
    pub const fn new_const() -> Self {
        Self {
            bytes_read:      AtomicU64::new(0),
            bytes_written:   AtomicU64::new(0),
            read_ops:        AtomicU64::new(0),
            write_ops:       AtomicU64::new(0),
            peak_read_bpt:   AtomicU64::new(0),
            peak_write_bpt:  AtomicU64::new(0),
            window:          ThroughputWindow::new_const(),
            last_flush_tick: AtomicU64::new(0),
        }
    }

    pub fn record_read(&self, bytes: u64) {
        self.bytes_read.fetch_add(bytes, Ordering::Relaxed);
        self.read_ops.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_write(&self, bytes: u64) {
        self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
        self.write_ops.fetch_add(1, Ordering::Relaxed);
    }

    /// Pousse les compteurs dans la fenêtre et remet à zéro pour la prochaine période.
    pub fn flush_period(&self, dt: u64) {
        let br = self.bytes_read.swap(0, Ordering::Relaxed);
        let bw = self.bytes_written.swap(0, Ordering::Relaxed);
        let tick = current_tick();
        let sample = ThroughputSample { tick, bytes_read: br, bytes_written: bw };
        self.window.push(sample);
        let rbpt = br.checked_div(dt.max(1)).unwrap_or(0);
        let wbpt = bw.checked_div(dt.max(1)).unwrap_or(0);
        let old_r = self.peak_read_bpt.load(Ordering::Relaxed);
        if rbpt > old_r { self.peak_read_bpt.store(rbpt, Ordering::Relaxed); }
        let old_w = self.peak_write_bpt.load(Ordering::Relaxed);
        if wbpt > old_w { self.peak_write_bpt.store(wbpt, Ordering::Relaxed); }
        self.last_flush_tick.store(tick, Ordering::Relaxed);
    }

    pub fn avg_read_bpt(&self)   -> u64 { self.window.avg_read_bpt() }
    pub fn avg_write_bpt(&self)  -> u64 { self.window.avg_write_bpt() }
    pub fn avg_total_bpt(&self)  -> u64 { self.window.avg_total_bpt() }
    pub fn peak_read_bpt(&self)  -> u64 { self.peak_read_bpt.load(Ordering::Relaxed) }
    pub fn peak_write_bpt(&self) -> u64 { self.peak_write_bpt.load(Ordering::Relaxed) }
    pub fn total_read_ops(&self)  -> u64 { self.read_ops.load(Ordering::Relaxed) }
    pub fn total_write_ops(&self) -> u64 { self.write_ops.load(Ordering::Relaxed) }
    pub fn pending_read_bytes(&self)  -> u64 { self.bytes_read.load(Ordering::Relaxed) }
    pub fn pending_write_bytes(&self) -> u64 { self.bytes_written.load(Ordering::Relaxed) }
    pub fn last_flush_tick(&self) -> u64 { self.last_flush_tick.load(Ordering::Relaxed) }

    pub fn snapshot(&self) -> ExofsResult<ThroughputSnapshot> {
        let samples = self.window.to_vec()?;
        Ok(ThroughputSnapshot {
            avg_read_bpt:   self.avg_read_bpt(),
            avg_write_bpt:  self.avg_write_bpt(),
            peak_read_bpt:  self.peak_read_bpt(),
            peak_write_bpt: self.peak_write_bpt(),
            total_read:     self.window.total_read_bytes(),
            total_written:  self.window.total_written_bytes(),
            samples,
        })
    }

    pub fn reset(&self) {
        self.bytes_read.store(0, Ordering::Relaxed);
        self.bytes_written.store(0, Ordering::Relaxed);
        self.read_ops.store(0, Ordering::Relaxed);
        self.write_ops.store(0, Ordering::Relaxed);
        self.peak_read_bpt.store(0, Ordering::Relaxed);
        self.peak_write_bpt.store(0, Ordering::Relaxed);
        self.last_flush_tick.store(0, Ordering::Relaxed);
        self.window.reset();
    }
}

pub static THROUGHPUT_TRACKER: ThroughputTracker = ThroughputTracker::new_const();

// ─── ThroughputSnapshot ───────────────────────────────────────────────────────

#[derive(Debug)]
pub struct ThroughputSnapshot {
    pub avg_read_bpt:   u64,
    pub avg_write_bpt:  u64,
    pub peak_read_bpt:  u64,
    pub peak_write_bpt: u64,
    pub total_read:     u64,
    pub total_written:  u64,
    pub samples:        Vec<ThroughputSample>,
}

impl ThroughputSnapshot {
    pub fn avg_total_bpt(&self) -> u64 {
        self.avg_read_bpt.saturating_add(self.avg_write_bpt)
    }
    pub fn peak_total_bpt(&self) -> u64 {
        self.peak_read_bpt.saturating_add(self.peak_write_bpt)
    }
    pub fn total_bytes(&self) -> u64 {
        self.total_read.saturating_add(self.total_written)
    }
    /// Ratio lecture/total en ‰ (ARITH-02).
    pub fn read_ratio_ppt(&self) -> u64 {
        let t = self.total_bytes();
        if t == 0 { return 0; }
        self.total_read.saturating_mul(1000).checked_div(t).unwrap_or(0)
    }
    pub fn write_ratio_ppt(&self) -> u64 {
        let t = self.total_bytes();
        if t == 0 { return 0; }
        self.total_written.saturating_mul(1000).checked_div(t).unwrap_or(0)
    }
    pub fn is_high_throughput(&self, threshold_bpt: u64) -> bool {
        self.avg_total_bpt() >= threshold_bpt
    }
    /// Équilibre read/write : écart de ≤20% entre les pics.
    pub fn is_balanced(&self) -> bool {
        let r = self.peak_read_bpt;
        let w = self.peak_write_bpt;
        if r == 0 && w == 0 { return true; }
        let mx = r.max(w);
        let mn = r.min(w);
        mn.saturating_mul(10).checked_div(mx.max(1)).unwrap_or(0) >= 8
    }
}

// ─── ThroughputRate ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ThroughputRate {
    pub read_bps:  u64,
    pub write_bps: u64,
    pub dt_ticks:  u64,
}

impl ThroughputRate {
    pub fn between(a: &ThroughputSample, b: &ThroughputSample) -> Self {
        let dt = b.tick.saturating_sub(a.tick).max(1);
        Self {
            read_bps:  b.bytes_read.saturating_sub(a.bytes_read).checked_div(dt).unwrap_or(0),
            write_bps: b.bytes_written.saturating_sub(a.bytes_written).checked_div(dt).unwrap_or(0),
            dt_ticks:  dt,
        }
    }
    pub fn total_bps(&self)     -> u64  { self.read_bps.saturating_add(self.write_bps) }
    pub fn is_read_heavy(&self)  -> bool { self.read_bps > self.write_bps }
    pub fn is_write_heavy(&self) -> bool { self.write_bps > self.read_bps }
    pub fn is_balanced(&self)    -> bool {
        let mx = self.read_bps.max(self.write_bps);
        let mn = self.read_bps.min(self.write_bps);
        if mx == 0 { return true; }
        mn.saturating_mul(10).checked_div(mx).unwrap_or(0) >= 8
    }
}

// ─── ThroughputThresholds ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ThroughputThresholds {
    /// Seuil d'alerte débit lecture (B/tick).
    pub max_read_bpt:  u64,
    /// Seuil d'alerte débit écriture (B/tick).
    pub max_write_bpt: u64,
    /// Seuil min de débit pour considérer le système actif (B/tick).
    pub min_active_bpt: u64,
}

impl ThroughputThresholds {
    pub const fn default_thresholds() -> Self {
        Self {
            max_read_bpt:   1024 * 1024,  // 1 MB/tick
            max_write_bpt:  512  * 1024,  // 512 KB/tick
            min_active_bpt: 4096,         // 4 KB/tick
        }
    }
    pub fn validate(&self) -> ExofsResult<()> {
        if self.max_read_bpt == 0 || self.max_write_bpt == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        if self.min_active_bpt > self.max_read_bpt.min(self.max_write_bpt) {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
    pub fn check_read(&self, bpt: u64) -> bool  { bpt <= self.max_read_bpt }
    pub fn check_write(&self, bpt: u64) -> bool { bpt <= self.max_write_bpt }
    pub fn is_active(&self, bpt: u64) -> bool   { bpt >= self.min_active_bpt }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_zero() {
        let s = ThroughputSample::zero();
        assert_eq!(s.total_bytes(), 0);
        assert_eq!(s.total_bpt(1), 0);
    }

    #[test]
    fn test_sample_read_bpt() {
        let s = ThroughputSample { tick: 0, bytes_read: 1000, bytes_written: 0 };
        assert_eq!(s.read_bpt(10), 100);
        assert_eq!(s.write_bpt(10), 0);
    }

    #[test]
    fn test_sample_total_bpt() {
        let s = ThroughputSample { tick: 0, bytes_read: 500, bytes_written: 500 };
        assert_eq!(s.total_bpt(10), 100);
    }

    #[test]
    fn test_window_push_and_latest() {
        let w = ThroughputWindow::new_const();
        w.push(ThroughputSample { tick: 1, bytes_read: 100, bytes_written: 50 });
        let l = w.latest().expect("some");
        assert_eq!(l.bytes_read, 100);
        assert_eq!(l.bytes_written, 50);
    }

    #[test]
    fn test_window_avg_read() {
        let w = ThroughputWindow::new_const();
        w.push(ThroughputSample { tick: 1, bytes_read: 100, bytes_written: 0 });
        w.push(ThroughputSample { tick: 2, bytes_read: 300, bytes_written: 0 });
        assert_eq!(w.avg_read_bpt(), 200);
    }

    #[test]
    fn test_window_to_vec() {
        let w = ThroughputWindow::new_const();
        w.push(ThroughputSample { tick: 1, bytes_read: 10, bytes_written: 20 });
        let v = w.to_vec().expect("ok");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].bytes_written, 20);
    }

    #[test]
    fn test_window_reset() {
        let w = ThroughputWindow::new_const();
        w.push(ThroughputSample { tick: 1, bytes_read: 1, bytes_written: 1 });
        w.reset();
        assert!(w.latest().is_none());
        assert_eq!(w.total_read_bytes(), 0);
    }

    #[test]
    fn test_tracker_record_flush() {
        let t = ThroughputTracker::new_const();
        t.record_read(4096);
        t.record_write(2048);
        assert_eq!(t.total_read_ops(), 1);
        t.flush_period(1);
        assert!(t.avg_total_bpt() > 0);
    }

    #[test]
    fn test_tracker_peak() {
        let t = ThroughputTracker::new_const();
        t.record_read(8192);
        t.flush_period(1);
        assert!(t.peak_read_bpt() > 0);
        assert_eq!(t.peak_write_bpt(), 0);
    }

    #[test]
    fn test_tracker_snapshot_read_ratio() {
        let t = ThroughputTracker::new_const();
        t.record_read(1000);
        t.flush_period(1);
        let snap = t.snapshot().expect("ok");
        assert_eq!(snap.read_ratio_ppt(), 1000);
        assert_eq!(snap.write_ratio_ppt(), 0);
    }

    #[test]
    fn test_tracker_reset() {
        let t = ThroughputTracker::new_const();
        t.record_read(9999);
        t.record_write(5555);
        t.reset();
        assert_eq!(t.total_read_ops(), 0);
        assert_eq!(t.total_write_ops(), 0);
        assert_eq!(t.peak_read_bpt(), 0);
    }

    #[test]
    fn test_rate_between_read_heavy() {
        let a = ThroughputSample { tick: 0, bytes_read: 0, bytes_written: 0 };
        let b = ThroughputSample { tick: 10, bytes_read: 1000, bytes_written: 200 };
        let r = ThroughputRate::between(&a, &b);
        assert_eq!(r.read_bps, 100);
        assert_eq!(r.write_bps, 20);
        assert!(r.is_read_heavy());
        assert!(!r.is_write_heavy());
    }

    #[test]
    fn test_rate_write_heavy() {
        let a = ThroughputSample { tick: 0, bytes_read: 0, bytes_written: 0 };
        let b = ThroughputSample { tick: 5, bytes_read: 50, bytes_written: 500 };
        let r = ThroughputRate::between(&a, &b);
        assert!(r.is_write_heavy());
    }

    #[test]
    fn test_thresholds_validate() {
        let t = ThroughputThresholds::default_thresholds();
        assert!(t.validate().is_ok());
        assert!(t.check_read(1024));
        assert!(t.is_active(8192));
    }

    #[test]
    fn test_thresholds_invalid() {
        let t = ThroughputThresholds { max_read_bpt: 0, max_write_bpt: 1000, min_active_bpt: 1 };
        assert!(t.validate().is_err());
    }
}
