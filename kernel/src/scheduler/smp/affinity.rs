// kernel/src/scheduler/smp/affinity.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CPU affinity — management of allowed CPU masks for each thread
// ═══════════════════════════════════════════════════════════════════════════════

use super::topology::{nr_cpus, MAX_CPUS};
use crate::scheduler::core::task::CpuId;
use core::sync::atomic::AtomicU64;

// ─────────────────────────────────────────────────────────────────────────────
// CpuMask — CPU mask (up to 256 CPUs across 4 × u64)
// ─────────────────────────────────────────────────────────────────────────────

const MASK_WORDS: usize = 4; // 4 × 64 = 256 bits

/// Allowed CPU mask.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuSet {
    pub(crate) bits: [u64; MASK_WORDS],
}

impl CpuSet {
    pub const EMPTY: Self = Self {
        bits: [0; MASK_WORDS],
    };
    pub const ALL: Self = Self {
        bits: [u64::MAX; MASK_WORDS],
    };

    /// Empty mask.
    pub const fn empty() -> Self {
        Self::EMPTY
    }

    #[inline(always)]
    pub const fn new(bits: [u64; MASK_WORDS]) -> Self {
        Self { bits }
    }

    /// Mask with all active CPUs (up to `nr_cpus()`).
    pub fn full() -> Self {
        let mut m = Self::empty();
        for cpu in 0..nr_cpus().min(MAX_CPUS) {
            m.set(CpuId(cpu as u32));
        }
        m
    }

    /// Sets bit for CPU `cpu`.
    pub fn set(&mut self, cpu: CpuId) {
        let cpu = cpu.0 as usize;
        if cpu < MAX_CPUS {
            self.bits[cpu / 64] |= 1u64 << (cpu % 64);
        }
    }

    /// Clears bit for CPU `cpu`.
    pub fn clear(&mut self, cpu: CpuId) {
        let cpu = cpu.0 as usize;
        if cpu < MAX_CPUS {
            self.bits[cpu / 64] &= !(1u64 << (cpu % 64));
        }
    }

    /// Returns `true` if CPU `cpu` is in mask.
    pub fn test(&self, cpu: CpuId) -> bool {
        self.contains(cpu)
    }

    #[inline(always)]
    pub fn contains(&self, cpu: CpuId) -> bool {
        let cpu = cpu.0 as usize;
        if cpu < MAX_CPUS {
            self.bits[cpu / 64] & (1u64 << (cpu % 64)) != 0
        } else {
            false
        }
    }

    /// Returns first CPU in mask, or `None` if mask is empty.
    pub fn first(&self) -> Option<CpuId> {
        self.first_cpu()
    }

    pub fn first_cpu(&self) -> Option<CpuId> {
        for (word_idx, &word) in self.bits.iter().enumerate() {
            if word != 0 {
                return Some(CpuId(
                    (word_idx * 64 + word.trailing_zeros() as usize) as u32,
                ));
            }
        }
        None
    }

    /// Number of active CPUs in mask.
    pub fn count(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Intersection (bitwise AND) of two masks.
    pub fn and(&self, other: &Self) -> Self {
        let mut out = Self::empty();
        for i in 0..MASK_WORDS {
            out.bits[i] = self.bits[i] & other.bits[i];
        }
        out
    }

    /// Returns `true` if mask is empty.
    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|&w| w == 0)
    }

    /// Returns a mask with a single allowed CPU.
    pub fn single(cpu: CpuId) -> Self {
        let mut out = Self::EMPTY;
        out.set(cpu);
        out
    }
}

pub type CpuMask = CpuSet;

const _: () = assert!(
    core::mem::size_of::<CpuSet>() == 32,
    "CpuSet doit faire 32 bytes"
);

// ─────────────────────────────────────────────────────────────────────────────
// Affinity verification
// ─────────────────────────────────────────────────────────────────────────────

/// Compatibility alias for progressively migrated call-sites.
pub fn cpu_allowed(affinity: &CpuSet, cpu: CpuId) -> bool {
    affinity.contains(cpu)
}

/// Returns stable affinity mask from a CpuMask.
pub fn affinity_mask_from_cpu_mask(mask: &CpuMask) -> CpuSet {
    *mask
}

/// Verifies that at least one active CPU is in thread affinity.
/// Returns affinity unchanged if valid, or a mask with ALL CPUs
/// if resulting mask is empty (anti-deadlock protection).
pub fn sanitize_affinity(affinity: CpuSet) -> CpuSet {
    if affinity.is_empty() {
        CpuSet::full()
    } else {
        affinity
    }
}

/// Metrics.
pub static AFFINITY_VIOLATIONS: AtomicU64 = AtomicU64::new(0);
