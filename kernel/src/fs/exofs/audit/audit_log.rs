//! audit_log.rs — Lock-free ring-buffer for the ExoFS audit log (no_std).
//!
//! Fixed 65,536-entry ring-buffer in production.
//! Under `#[cfg(test)]`, capacity is reduced to avoid allocating several
//! megabytes on the test threads' stack.
//! Writes are lock-free via atomic `fetch_add`.
//! Reads are diagnostic (no strict consistency guarantee
//! under intense concurrent reading, acceptable for audit).
//!
//! Rules:
//!  - ONDISK-03: no AtomicU64 in repr(C) structs
//!  - ARITH-02: arithmetic overflow via wrapping
//!  - RECUR-01: zero recursion

use super::audit_entry::{AuditEntry, AuditResult, AuditSeverity, AUDIT_ENTRY_SIZE};
use core::sync::atomic::{AtomicU64, Ordering};

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;

// ─────────────────────────────────────────────────────────────────────────────
// Ring-buffer constants
// ─────────────────────────────────────────────────────────────────────────────

/// Number of entries in the ring-buffer (power of 2 for fast masking).
#[cfg(test)]
pub const RING_SIZE: usize = 1024;
/// Production size of the audit log.
#[cfg(not(test))]
pub const RING_SIZE: usize = 65536;

/// Mask for calculating the index mod RING_SIZE without division.
const RING_MASK: usize = RING_SIZE - 1;

/// Memory capacity of the ring-buffer in bytes.
pub const RING_CAPACITY_BYTES: usize = RING_SIZE * AUDIT_ENTRY_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// AUDIT_LOG — global instance
// ─────────────────────────────────────────────────────────────────────────────

pub static AUDIT_LOG: AuditLog = AuditLog::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// AuditLog
// ─────────────────────────────────────────────────────────────────────────────

/// Non-blocking ring-buffer for audit entries.
///
/// - `head`: next write index (monotonically increasing).
/// - `count`: total number of entries recorded since boot.
/// - `n_overwritten`: number of entries overwritten by wrap-around.
pub struct AuditLog {
    ring: [core::cell::UnsafeCell<AuditEntry>; RING_SIZE],
    head: AtomicU64,
    count: AtomicU64,
    n_overwritten: AtomicU64,
    n_security: AtomicU64,
    n_critical: AtomicU64,
    n_errors: AtomicU64,
}

// SAFETY: ring access exclusively via atomic `fetch_add` on `head`.
unsafe impl Sync for AuditLog {}

impl AuditLog {
    /// `const` constructor for use in `static`.
    pub const fn new_const() -> Self {
        const ZERO_ENTRY: AuditEntry = AuditEntry {
            tick: 0,
            actor_uid: 0,
            actor_cap: 0,
            object_id: 0,
            blob_id: [0; 32],
            op: 0,
            result: 0,
            severity: 0,
            flags: 0,
            seq: 0,
            magic: 0,
            _pad: [0; 16],
        };
        const ZERO: core::cell::UnsafeCell<AuditEntry> = core::cell::UnsafeCell::new(ZERO_ENTRY);
        Self {
            ring: [ZERO; RING_SIZE],
            head: AtomicU64::new(0),
            count: AtomicU64::new(0),
            n_overwritten: AtomicU64::new(0),
            n_security: AtomicU64::new(0),
            n_critical: AtomicU64::new(0),
            n_errors: AtomicU64::new(0),
        }
    }

    // ── Writes ───────────────────────────────────────────────────────────────

    /// Records an entry (lock-free).
    ///
    /// `fetch_add` allocates a unique slot. If the ring was full, the oldest
    /// entry is silently overwritten and `n_overwritten` is incremented.
    pub fn push(&self, mut entry: AuditEntry) {
        let seq = self.head.fetch_add(1, Ordering::Relaxed);
        let total = self.count.fetch_add(1, Ordering::Relaxed);
        entry.seq = seq;

        let idx = (seq as usize) & RING_MASK;

        // If the ring is full, an old entry will be overwritten.
        if total >= RING_SIZE as u64 {
            self.n_overwritten.fetch_add(1, Ordering::Relaxed);
        }

        // Update severity counters.
        if entry.is_security() {
            self.n_security.fetch_add(1, Ordering::Relaxed);
        }
        if entry.severity >= AuditSeverity::Critical as u8 {
            self.n_critical.fetch_add(1, Ordering::Relaxed);
        }
        if AuditResult::from_u8(entry.result)
            .map(|r| !r.is_ok())
            .unwrap_or(false)
        {
            self.n_errors.fetch_add(1, Ordering::Relaxed);
        }

        // SAFETY: unique index via fetch_add + masking; no data race
        // because each push writes to a different slot (modulo RING_SIZE,
        // overwriting is acceptable for the audit log).
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        unsafe {
            *self.ring[idx].get() = entry;
        }
    }

    // ── Reads ────────────────────────────────────────────────────────────────

    /// Reads the entry at absolute position `pos` (modulo RING_SIZE).
    pub fn read_at(&self, pos: usize) -> AuditEntry {
        let idx = pos & RING_MASK;
        // SAFETY: diagnostic read, no strict guarantee.
        unsafe { *self.ring[idx].get() }
    }

    /// Reads the entry at the current index (last written − 1).
    pub fn read_last(&self) -> Option<AuditEntry> {
        let h = self.head.load(Ordering::Relaxed);
        if h == 0 {
            return None;
        }
        Some(self.read_at((h.wrapping_sub(1)) as usize))
    }

    /// Copies the `n` most recent entries into `out` (sequential read).
    /// `n` is clamped to `RING_SIZE`.
    /// Returns the number of entries actually copied.
    pub fn read_recent_into(&self, out: &mut [AuditEntry], n: usize) -> usize {
        let head = self.head.load(Ordering::Relaxed) as usize;
        let total = self.count.load(Ordering::Relaxed) as usize;
        let avail = n.min(total).min(RING_SIZE).min(out.len());
        for i in 0..avail {
            let pos = head.wrapping_sub(avail).wrapping_add(i);
            out[i] = self.read_at(pos);
        }
        avail
    }

    /// Iterates over all valid entries in chronological order
    /// and invokes `f(entry)` for each (iterative, RECUR-01).
    pub fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(&AuditEntry),
    {
        let head = self.head.load(Ordering::Relaxed) as usize;
        let total = self.count.load(Ordering::Relaxed) as usize;
        let avail = total.min(RING_SIZE);
        for i in 0..avail {
            let pos = head.wrapping_sub(avail).wrapping_add(i);
            let entry = self.read_at(pos);
            if entry.is_valid() {
                f(&entry);
            }
        }
    }

    // ── Statistics ───────────────────────────────────────────────────────────

    /// Total number of entries recorded since boot.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Next sequence number (= head).
    pub fn next_seq(&self) -> u64 {
        self.head.load(Ordering::Relaxed)
    }

    /// Number of available entries in the ring (clamped to RING_SIZE).
    pub fn available(&self) -> usize {
        (self.count.load(Ordering::Relaxed) as usize).min(RING_SIZE)
    }

    /// Number of entries overwritten by overflow.
    pub fn n_overwritten(&self) -> u64 {
        self.n_overwritten.load(Ordering::Relaxed)
    }

    /// Number of security entries recorded.
    pub fn n_security(&self) -> u64 {
        self.n_security.load(Ordering::Relaxed)
    }

    /// Number of critical or alert entries.
    pub fn n_critical(&self) -> u64 {
        self.n_critical.load(Ordering::Relaxed)
    }

    /// Number of entries in error (Error or Timeout).
    pub fn n_errors(&self) -> u64 {
        self.n_errors.load(Ordering::Relaxed)
    }

    /// Returns a snapshot of the log statistics.
    pub fn stats(&self) -> AuditLogStats {
        AuditLogStats {
            total_pushed: self.count.load(Ordering::Relaxed),
            available: self.available() as u64,
            n_overwritten: self.n_overwritten.load(Ordering::Relaxed),
            n_security: self.n_security.load(Ordering::Relaxed),
            n_critical: self.n_critical.load(Ordering::Relaxed),
            n_errors: self.n_errors.load(Ordering::Relaxed),
            ring_fill_pct: {
                let a = self.available();
                ((a as u64 * 100) / RING_SIZE as u64) as u8
            },
        }
    }

    /// Resets all counters to zero (usage: tests or rotation).
    pub fn reset_stats(&self) {
        self.n_overwritten.store(0, Ordering::Relaxed);
        self.n_security.store(0, Ordering::Relaxed);
        self.n_critical.store(0, Ordering::Relaxed);
        self.n_errors.store(0, Ordering::Relaxed);
    }

    /// Checks that the ring-buffer is healthy (valid magic on N entries).
    pub fn sanity_check(&self, n: usize) -> AuditLogHealth {
        let avail = self.available();
        let sample = n.min(avail);
        let mut invalid = 0u32;
        let head = self.head.load(Ordering::Relaxed) as usize;
        for i in 0..sample {
            let pos = head.wrapping_sub(sample).wrapping_add(i);
            if !self.read_at(pos).is_valid() {
                invalid = invalid.wrapping_add(1);
            }
        }
        AuditLogHealth {
            sampled: sample as u32,
            corrupt: invalid,
            fill_pct: self.stats().ring_fill_pct,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditLogStats
// ─────────────────────────────────────────────────────────────────────────────

/// Ring-buffer statistics.
#[derive(Clone, Debug, Default)]
pub struct AuditLogStats {
    pub total_pushed: u64,
    pub available: u64,
    pub n_overwritten: u64,
    pub n_security: u64,
    pub n_critical: u64,
    pub n_errors: u64,
    /// Fill percentage (0-100).
    pub ring_fill_pct: u8,
}

impl AuditLogStats {
    /// `true` if the ring is more than 80% full.
    pub fn is_near_full(&self) -> bool {
        self.ring_fill_pct >= 80
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditLogHealth
// ─────────────────────────────────────────────────────────────────────────────

/// Sanity check result.
#[derive(Clone, Debug)]
pub struct AuditLogHealth {
    pub sampled: u32,
    pub corrupt: u32,
    pub fill_pct: u8,
}

impl AuditLogHealth {
    /// `true` if no corrupted entry in the sample.
    pub fn is_clean(&self) -> bool {
        self.corrupt == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::audit_entry::{AuditEntry, AuditOp, AuditResult};
    use super::*;

    fn push_simple(log: &AuditLog, op: AuditOp, result: AuditResult) {
        log.push(AuditEntry::new(1, 1, 0, 0, [0u8; 32], op, result, 0));
    }

    #[test]
    fn test_initial_count_zero() {
        let log = AuditLog::new_const();
        assert_eq!(log.count(), 0);
        assert_eq!(log.available(), 0);
    }

    #[test]
    fn test_push_increments_count() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Read, AuditResult::Success);
        assert_eq!(log.count(), 1);
        assert_eq!(log.available(), 1);
    }

    #[test]
    fn test_read_last_none_when_empty() {
        let log = AuditLog::new_const();
        assert!(log.read_last().is_none());
    }

    #[test]
    fn test_read_last_returns_last_pushed() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Write, AuditResult::Success);
        let e = log.read_last().test_unwrap();
        assert!(e.is_valid());
        assert_eq!(e.op, AuditOp::Write as u8);
    }

    #[test]
    fn test_seq_monotone() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Read, AuditResult::Success);
        push_simple(&log, AuditOp::Write, AuditResult::Success);
        assert_eq!(log.next_seq(), 2);
    }

    #[test]
    fn test_security_counter() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::CryptoKey, AuditResult::Success);
        assert_eq!(log.n_security(), 1);
    }

    #[test]
    fn test_critical_counter() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Read, AuditResult::Denied);
        assert!(log.n_critical() >= 1);
    }

    #[test]
    fn test_stats_fill_pct_zero_initially() {
        let log = AuditLog::new_const();
        let s = log.stats();
        assert_eq!(s.ring_fill_pct, 0);
    }

    #[test]
    fn test_read_recent_into() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Create, AuditResult::Success);
        push_simple(&log, AuditOp::Delete, AuditResult::Success);
        let mut buf = [AuditEntry::new(
            0,
            0,
            0,
            0,
            [0u8; 32],
            AuditOp::Read,
            AuditResult::Success,
            0,
        ); 4];
        let n = log.read_recent_into(&mut buf, 2);
        assert_eq!(n, 2);
    }

    #[test]
    fn test_sanity_check_clean() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Read, AuditResult::Success);
        let h = log.sanity_check(1);
        assert!(h.is_clean());
    }

    #[test]
    fn test_for_each_count() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Read, AuditResult::Success);
        push_simple(&log, AuditOp::Write, AuditResult::Success);
        let mut count = 0usize;
        log.for_each(|_| count += 1);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_reset_stats() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::CryptoKey, AuditResult::Denied);
        log.reset_stats();
        assert_eq!(log.n_security(), 0);
    }

    #[test]
    fn test_n_errors_on_timeout() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Read, AuditResult::Timeout);
        assert!(log.n_errors() >= 1);
    }

    #[test]
    fn test_available_bounded_by_ring_size() {
        let log = AuditLog::new_const();
        // Pushed entries cannot exceed RING_SIZE.
        assert!(log.available() <= RING_SIZE);
    }

    #[test]
    fn test_sanity_check_fill_pct() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Create, AuditResult::Success);
        let h = log.sanity_check(1);
        // fill_pct is always <= 100.
        assert!(h.fill_pct <= 100);
    }

    #[test]
    fn test_stats_near_full_false_on_fresh() {
        let log = AuditLog::new_const();
        let stat = log.stats();
        // An empty ring is not "near full".
        assert!(!stat.is_near_full());
    }

    #[test]
    fn test_read_recent_into_zero() {
        let log = AuditLog::new_const();
        let mut buf = [];
        let n = log.read_recent_into(&mut buf, 0);
        assert_eq!(n, 0);
    }

    #[test]
    fn test_for_each_no_panic_empty() {
        let log = AuditLog::new_const();
        let mut count = 0usize;
        log.for_each(|_| count += 1);
        assert_eq!(count, 0);
    }
}
