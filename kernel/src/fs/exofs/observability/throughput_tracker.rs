//! throughput_tracker.rs — Suivi du débit I/O ExoFS (no_std).

use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::time::read_ticks;

pub static THROUGHPUT: ThroughputTracker = ThroughputTracker::new_const();

pub struct ThroughputTracker {
    bytes_read_total:    AtomicU64,
    bytes_written_total: AtomicU64,
    window_start_tick:   AtomicU64,
    window_read_bytes:   AtomicU64,
    window_write_bytes:  AtomicU64,
}

impl ThroughputTracker {
    pub const fn new_const() -> Self {
        Self {
            bytes_read_total:    AtomicU64::new(0),
            bytes_written_total: AtomicU64::new(0),
            window_start_tick:   AtomicU64::new(0),
            window_read_bytes:   AtomicU64::new(0),
            window_write_bytes:  AtomicU64::new(0),
        }
    }

    pub fn record_read(&self, bytes: u64) {
        self.bytes_read_total.fetch_add(bytes, Ordering::Relaxed);
        self.window_read_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn record_write(&self, bytes: u64) {
        self.bytes_written_total.fetch_add(bytes, Ordering::Relaxed);
        self.window_write_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Taux de lecture en octets/tick depuis le dernier reset de fenêtre.
    pub fn read_throughput_bpt(&self) -> u64 {
        let elapsed = read_ticks().saturating_sub(self.window_start_tick.load(Ordering::Relaxed));
        if elapsed == 0 { return 0; }
        self.window_read_bytes.load(Ordering::Relaxed) / elapsed
    }

    /// Taux d'écriture en octets/tick depuis le dernier reset de fenêtre.
    pub fn write_throughput_bpt(&self) -> u64 {
        let elapsed = read_ticks().saturating_sub(self.window_start_tick.load(Ordering::Relaxed));
        if elapsed == 0 { return 0; }
        self.window_write_bytes.load(Ordering::Relaxed) / elapsed
    }

    /// Démarre une nouvelle fenêtre de mesure.
    pub fn reset_window(&self) {
        self.window_start_tick.store(read_ticks(), Ordering::Relaxed);
        self.window_read_bytes.store(0, Ordering::Relaxed);
        self.window_write_bytes.store(0, Ordering::Relaxed);
    }

    pub fn total_read(&self)    -> u64 { self.bytes_read_total.load(Ordering::Relaxed) }
    pub fn total_written(&self) -> u64 { self.bytes_written_total.load(Ordering::Relaxed) }
}
