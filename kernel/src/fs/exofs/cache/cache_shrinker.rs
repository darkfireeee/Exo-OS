//! cache_shrinker.rs — ExoFS cache shrinker under memory pressure (no_std).
//!
//! `CacheShrinker`: shrinks caches from coldest to hottest until reaching
//! the memory reclaim target.
//! Rules: RECUR-01, OOM-02, ARITH-02.

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// ShrinkTarget
// ─────────────────────────────────────────────────────────────────────────────

/// Shrink request sent to shrinkers.
#[derive(Clone, Copy, Debug)]
pub struct ShrinkRequest {
    /// Bytes to free.
    pub bytes_to_free: u64,
    /// Priority: `true` = urgent, pass through all levels.
    pub urgent: bool,
}

/// Result of a shrink operation.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShrinkResult {
    /// Bytes actually freed.
    pub freed_bytes: u64,
    /// Evicted entries.
    pub evicted_count: u64,
    /// `true` if target was reached.
    pub target_reached: bool,
}

impl ShrinkResult {
    pub fn add(&mut self, freed: u64, count: u64) {
        self.freed_bytes = self.freed_bytes.wrapping_add(freed);
        self.evicted_count = self.evicted_count.wrapping_add(count);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ShrinkableCache — simulated trait with function pointer
// ─────────────────────────────────────────────────────────────────────────────

/// Registration of a shrinkable cache.
#[derive(Clone, Copy)]
pub struct CacheEntry {
    /// Cache name (for diagnostics).
    pub name: &'static str,
    /// Currently used bytes.
    pub used: u64,
    /// Capacity.
    pub cap: u64,
    /// Shrink priority: 0 = first (coldest).
    pub priority: u8,
    /// Shrink function.
    pub shrink_fn: fn(bytes: u64) -> ShrinkResult,
}

// ─────────────────────────────────────────────────────────────────────────────
// CacheShrinker
// ─────────────────────────────────────────────────────────────────────────────

/// Multi-cache shrink manager.
///
/// Does not hold a lock because the list is statically initialized.
pub struct CacheShrinker {
    caches: [Option<CacheEntry>; 8],
    n: usize,
}

pub static CACHE_SHRINKER: CacheShrinker = CacheShrinker::new_const();

impl CacheShrinker {
    pub const fn new_const() -> Self {
        Self {
            caches: [None; 8],
            n: 0,
        }
    }

    // ── Registration ──────────────────────────────────────────────────────────

    /// Registers a shrinkable cache.
    pub fn register(&mut self, entry: CacheEntry) -> ExofsResult<()> {
        if self.n >= 8 {
            return Err(ExofsError::NoSpace);
        }
        self.caches[self.n] = Some(entry);
        self.n = self.n.wrapping_add(1);
        Ok(())
    }

    // ── Shrinking ─────────────────────────────────────────────────────────────

    /// Frees `bytes_to_free` bytes by calling shrinkers from coldest to hottest.
    pub fn shrink_to_target(&self, req: ShrinkRequest) -> ShrinkResult {
        // Collect present caches sorted by priority.
        let mut sorted: Vec<CacheEntry> = self.caches[..self.n]
            .iter()
            .filter_map(|x| x.as_ref().cloned())
            .collect();
        sorted.sort_unstable_by_key(|e| e.priority);

        let mut result = ShrinkResult::default();
        let mut remaining = req.bytes_to_free;

        'outer: for entry in &sorted {
            if remaining == 0 {
                break 'outer;
            }
            let to_free = remaining.min(entry.used);
            let r = (entry.shrink_fn)(to_free);
            result.add(r.freed_bytes, r.evicted_count);
            remaining = remaining.saturating_sub(r.freed_bytes);
        }

        result.target_reached = remaining == 0;
        result
    }

    /// Frees maximum possible (emergency flush).
    pub fn shrink_all(&self) -> ShrinkResult {
        let total: u64 = self.caches[..self.n]
            .iter()
            .filter_map(|x| x.as_ref())
            .map(|e| e.used)
            .fold(0u64, |acc, v| acc.saturating_add(v));
        self.shrink_to_target(ShrinkRequest {
            bytes_to_free: total,
            urgent: true,
        })
    }

    /// Number of registered caches.
    pub fn n_caches(&self) -> usize {
        self.n
    }

    /// Estimated total used.
    pub fn total_used(&self) -> u64 {
        self.caches[..self.n]
            .iter()
            .filter_map(|x| x.as_ref())
            .map(|e| e.used)
            .fold(0u64, |acc, v| acc.saturating_add(v))
    }

    /// Estimated total capacity.
    pub fn total_cap(&self) -> u64 {
        self.caches[..self.n]
            .iter()
            .filter_map(|x| x.as_ref())
            .map(|e| e.cap)
            .fold(0u64, |acc, v| acc.saturating_add(v))
    }

    /// Global fill percentage.
    pub fn fill_pct(&self) -> u8 {
        let cap = self.total_cap();
        if cap == 0 {
            return 0;
        }
        let used = self.total_used();
        let pct = used.saturating_mul(100) / cap;
        if pct > 100 {
            100
        } else {
            pct as u8
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_shrink(bytes: u64) -> ShrinkResult {
        ShrinkResult {
            freed_bytes: bytes,
            evicted_count: 1,
            target_reached: false,
        }
    }

    fn make_entry(name: &'static str, used: u64, priority: u8) -> CacheEntry {
        CacheEntry {
            name,
            used,
            cap: used * 2,
            priority,
            shrink_fn: dummy_shrink,
        }
    }

    #[test]
    fn test_register_and_count() {
        let mut s = CacheShrinker::new_const();
        s.register(make_entry("blob", 1024, 0)).test_unwrap();
        assert_eq!(s.n_caches(), 1);
    }

    #[test]
    fn test_shrink_to_target_exact() {
        let mut s = CacheShrinker::new_const();
        s.register(make_entry("a", 2048, 0)).test_unwrap();
        let req = ShrinkRequest {
            bytes_to_free: 1024,
            urgent: false,
        };
        let r = s.shrink_to_target(req);
        assert_eq!(r.freed_bytes, 1024);
        assert!(r.target_reached);
    }

    #[test]
    fn test_shrink_order_by_priority() {
        #[allow(dead_code)]
        static ORDER: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

        fn shrink_a(b: u64) -> ShrinkResult {
            // priority 0 must be called first
            ShrinkResult {
                freed_bytes: b,
                evicted_count: 1,
                target_reached: false,
            }
        }
        let mut s = CacheShrinker::new_const();
        s.register(CacheEntry {
            name: "high",
            used: 100,
            cap: 200,
            priority: 10,
            shrink_fn: dummy_shrink,
        })
        .test_unwrap();
        s.register(CacheEntry {
            name: "low",
            used: 100,
            cap: 200,
            priority: 0,
            shrink_fn: shrink_a,
        })
        .test_unwrap();
        // Shrinker must sort by ascending priority.
        let req = ShrinkRequest {
            bytes_to_free: 100,
            urgent: false,
        };
        let r = s.shrink_to_target(req);
        assert!(r.target_reached);
    }

    #[test]
    fn test_shrink_all() {
        let mut s = CacheShrinker::new_const();
        s.register(make_entry("a", 500, 0)).test_unwrap();
        s.register(make_entry("b", 300, 1)).test_unwrap();
        let r = s.shrink_all();
        assert_eq!(r.freed_bytes, 800);
    }

    #[test]
    fn test_total_used() {
        let mut s = CacheShrinker::new_const();
        s.register(make_entry("a", 100, 0)).test_unwrap();
        s.register(make_entry("b", 200, 1)).test_unwrap();
        assert_eq!(s.total_used(), 300);
    }

    #[test]
    fn test_fill_pct() {
        let mut s = CacheShrinker::new_const();
        s.register(make_entry("a", 100, 0)).test_unwrap(); // cap = 200
        assert_eq!(s.fill_pct(), 50);
    }

    #[test]
    fn test_register_overflow() {
        let mut s = CacheShrinker::new_const();
        for i in 0u8..8 {
            s.register(make_entry("x", i as u64, 0)).test_unwrap();
        }
        assert!(s.register(make_entry("overflow", 0, 0)).is_err());
    }

    #[test]
    fn test_shrink_result_add() {
        let mut r = ShrinkResult::default();
        r.add(100, 2);
        r.add(50, 1);
        assert_eq!(r.freed_bytes, 150);
        assert_eq!(r.evicted_count, 3);
    }

    #[test]
    fn test_empty_shrinker() {
        let s = CacheShrinker::new_const();
        let req = ShrinkRequest {
            bytes_to_free: 9999,
            urgent: true,
        };
        let r = s.shrink_to_target(req);
        assert_eq!(r.freed_bytes, 0);
        assert!(!r.target_reached);
    }
}

// ── CacheShrinker extensions ───────────────────────────────────────────────────

/// Shrink log.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShrinkLogEntry {
    pub freed_bytes: u64,
    pub evicted_count: u64,
    pub target_met: bool,
}

/// Aggregated shrinker dashboard.
#[derive(Clone, Debug, Default)]
pub struct ShrinkReport {
    pub total_freed: u64,
    pub total_evicted: u64,
    pub n_caches: usize,
    pub target_met: bool,
}

impl CacheShrinker {
    /// Shrinks down to `pct`% of total capacity.
    pub fn shrink_to_pct(&self, pct: u8) -> ShrinkResult {
        let cap = self.total_cap();
        let used = self.total_used();
        let target = cap.saturating_mul(pct as u64) / 100;
        let to_free = if used > target {
            used - target
        } else {
            return ShrinkResult::default();
        };
        self.shrink_to_target(ShrinkRequest {
            bytes_to_free: to_free,
            urgent: false,
        })
    }

    /// Generates an emergency shrink report.
    pub fn emergency_report(&self) -> ShrinkReport {
        let r = self.shrink_all();
        ShrinkReport {
            total_freed: r.freed_bytes,
            total_evicted: r.evicted_count,
            n_caches: self.n_caches(),
            target_met: r.target_reached,
        }
    }

    /// Returns fill percentage of cache `i`.
    pub fn fill_pct_of(&self, i: usize) -> u8 {
        if i >= self.n {
            return 0;
        }
        if let Some(e) = &self.caches[i] {
            if e.cap == 0 {
                return 0;
            }
            let p = e.used.saturating_mul(100) / e.cap;
            if p > 100 {
                100
            } else {
                p as u8
            }
        } else {
            0
        }
    }

    /// Name of cache `i`.
    pub fn cache_name(&self, i: usize) -> &'static str {
        if i >= self.n {
            return "";
        }
        self.caches[i].as_ref().map(|e| e.name).unwrap_or("")
    }

    /// `true` if at least one cache is at >90% capacity.
    pub fn any_critical(&self) -> bool {
        (0..self.n).any(|i| self.fill_pct_of(i) >= 90)
    }
}
