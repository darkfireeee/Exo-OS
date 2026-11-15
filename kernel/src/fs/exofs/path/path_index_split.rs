//! path_index_split.rs — Splitting a PathIndex into two child indices.
//!
//! When a [`PathIndex`] exceeds its split threshold (`needs_split() == true`),
//! this module splits it into a "low" index (entries with hash < pivot) and a "high" index
//! (entries with hash >= pivot).
//!
//! ## Strategy
//! - Sort entries by hash.
//! - Pivot = median of hashes.
//! - Low index  : entries [0..mid].
//! - High index : entries [mid..].
//!
//! ## Applied spec rules
//! - **ARITH-02** : `checked_add` on size calculations.
//! - **OOM-02** : `try_reserve(1)` before insertions.

extern crate alloc;
use alloc::vec::Vec;

use super::path_component::validate_component;
use super::path_index::{InMemoryEntry, PathIndex};
use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};

// ── SplitResult ───────────────────────────────────────────────────────────────

/// Result of a split operation.
pub struct SplitResult {
    /// "Low" child index (hashes strictly lower than pivot).
    pub low: PathIndex,
    /// "High" child index (hashes greater than or equal to pivot).
    pub high: PathIndex,
    /// Pivot hash used for splitting.
    pub pivot_hash: u64,
    /// Number of entries transferred to low index.
    pub low_count: usize,
    /// Number of entries transferred to high index.
    pub high_count: usize,
}

// ── PathIndexSplitter ─────────────────────────────────────────────────────────

/// PathIndex splitter.
///
/// Consumes the source index and produces a [`SplitResult`].
pub struct PathIndexSplitter;

impl PathIndexSplitter {
    /// Splits `src` into two balanced child indices.
    ///
    /// The source index is consumed (its entries are drained).
    /// Both child indices share the same `parent_oid`.
    ///
    /// # Errors
    /// - [`ExofsError::InvalidArgument`]  if source index has < 2 entries.
    /// - [`ExofsError::NoMemory`]         if allocation fails.
    /// - [`ExofsError::InternalError`]    if internal invariant is violated.
    pub fn split(src: &mut PathIndex, parent_oid: ObjectId) -> ExofsResult<SplitResult> {
        if src.len() < 2 {
            return Err(ExofsError::InvalidArgument);
        }

        // ── 1. Collect and sort by hash ───────────────────────────────────────
        let mut sorted: Vec<InMemoryEntry> = Vec::new();
        for e in src.entries() {
            sorted.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            sorted.push(e.clone());
        }
        sorted.sort_unstable_by_key(|e| e.hash);

        // ── 2. Compute pivot (median) ─────────────────────────────────────────
        let mid = sorted.len() / 2;
        let pivot_hash = sorted[mid].hash;

        // ── 3. Create the two child indices ───────────────────────────────────
        let mut low = PathIndex::new(parent_oid.clone());
        let mut high = PathIndex::new(parent_oid);

        for (i, e) in sorted.iter().enumerate() {
            let comp = validate_component(e.name_bytes()).map_err(|_| ExofsError::InternalError)?;

            if i < mid {
                low.insert(&comp, e.oid.clone(), e.kind)?;
            } else {
                high.insert(&comp, e.oid.clone(), e.kind)?;
            }
        }

        low.dirty = true;
        high.dirty = true;

        let low_count = low.len();
        let high_count = high.len();

        Ok(SplitResult {
            low,
            high,
            pivot_hash,
            low_count,
            high_count,
        })
    }

    /// Splits by applying an explicitly provided pivot hash.
    ///
    /// Useful when the pivot must be consistent with an existing B+ tree.
    ///
    /// # Errors
    /// Same as [`split`].
    pub fn split_at_hash(
        src: &mut PathIndex,
        parent_oid: ObjectId,
        pivot_hash: u64,
    ) -> ExofsResult<SplitResult> {
        if src.len() < 2 {
            return Err(ExofsError::InvalidArgument);
        }

        let mut low = PathIndex::new(parent_oid.clone());
        let mut high = PathIndex::new(parent_oid);

        for e in src.entries() {
            let comp = validate_component(e.name_bytes()).map_err(|_| ExofsError::InternalError)?;
            if e.hash < pivot_hash {
                low.insert(&comp, e.oid.clone(), e.kind)?;
            } else {
                high.insert(&comp, e.oid.clone(), e.kind)?;
            }
        }

        if low.is_empty() || high.is_empty() {
            return Err(ExofsError::InvalidArgument);
        }

        low.dirty = true;
        high.dirty = true;

        let low_count = low.len();
        let high_count = high.len();

        Ok(SplitResult {
            low,
            high,
            pivot_hash,
            low_count,
            high_count,
        })
    }

    /// Checks whether a split is needed.
    #[inline]
    pub fn is_needed(idx: &PathIndex) -> bool {
        idx.needs_split()
    }

    /// Computes the optimal pivot (median hash) without modifying the index.
    ///
    /// Returns `None` if the index has fewer than 2 entries.
    pub fn optimal_pivot(idx: &PathIndex) -> Option<u64> {
        if idx.len() < 2 {
            return None;
        }
        let mut hashes: Vec<u64> = Vec::new();
        for e in idx.entries() {
            hashes.try_reserve(1).ok()?;
            hashes.push(e.hash);
        }
        hashes.sort_unstable();
        let mid = hashes.len() / 2;
        Some(hashes[mid])
    }

    /// Estimates serialized size of an index after split.
    ///
    /// Allows checking available space before performing the split.
    ///
    /// # ARITH-02
    pub fn estimate_split_size(src: &PathIndex) -> ExofsResult<(usize, usize)> {
        let half = src.len() / 2;
        // Header size + average entries (44 + ~16 name bytes).
        let avg_entry: usize = 44_usize.checked_add(16).ok_or(ExofsError::OffsetOverflow)?;
        let half_entries = half
            .checked_mul(avg_entry)
            .ok_or(ExofsError::OffsetOverflow)?;
        let low_size = (148_usize)
            .checked_add(half_entries)
            .ok_or(ExofsError::OffsetOverflow)?;
        let rem = src.len().saturating_sub(half);
        let rem_entries = rem
            .checked_mul(avg_entry)
            .ok_or(ExofsError::OffsetOverflow)?;
        let high_size = (148_usize)
            .checked_add(rem_entries)
            .ok_or(ExofsError::OffsetOverflow)?;
        Ok((low_size, high_size))
    }

    /// Validates consistency of a SplitResult:
    /// - low  : all hashes < pivot.
    /// - high : all hashes >= pivot.
    /// - low.len() + high.len() == original_count.
    pub fn verify_split(result: &SplitResult, original_count: usize) -> ExofsResult<()> {
        // Verify count consistency.
        let total = result
            .low_count
            .checked_add(result.high_count)
            .ok_or(ExofsError::OffsetOverflow)?;
        if total != original_count {
            return Err(ExofsError::InternalError);
        }

        // Verify hashes low < pivot.
        for e in result.low.entries() {
            if e.hash >= result.pivot_hash {
                return Err(ExofsError::InternalError);
            }
        }
        // Verify hashes high >= pivot.
        for e in result.high.entries() {
            if e.hash < result.pivot_hash {
                return Err(ExofsError::InternalError);
            }
        }
        Ok(())
    }
}

// ── SplitMetrics ──────────────────────────────────────────────────────────────

/// Metrics collected during a split.
#[derive(Debug, Clone)]
pub struct SplitMetrics {
    /// Total number of entries in source before split.
    pub total_before: usize,
    /// Number of entries in low index.
    pub low_count: usize,
    /// Number of entries in high index.
    pub high_count: usize,
    /// Imbalance ratio (0 = perfectly balanced, 100 = all on one side).
    pub imbalance_pct: u32,
    /// Pivot hash ultimately used.
    pub used_pivot: u64,
    /// Split threshold of the source.
    pub source_threshold: u32,
}

impl SplitMetrics {
    /// Constructs metrics from a SplitResult.
    pub fn from_result(result: &SplitResult, total_before: usize, source_threshold: u32) -> Self {
        let low = result.low_count as i64;
        let high = result.high_count as i64;
        let total = (total_before as i64).max(1);
        let diff = (low - high).unsigned_abs() as u32;
        let imbalance_pct = (diff.saturating_mul(100)) / (total as u32).max(1);
        SplitMetrics {
            total_before,
            low_count: result.low_count,
            high_count: result.high_count,
            imbalance_pct,
            used_pivot: result.pivot_hash,
            source_threshold,
        }
    }

    /// `true` if the split is heavily imbalanced (> 75% on one side).
    pub fn is_imbalanced(&self) -> bool {
        self.imbalance_pct > 75
    }
}

// ── SplitPolicy ───────────────────────────────────────────────────────────────

/// Split policy: how to choose the pivot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitPolicy {
    /// Median pivot (maximum balance).
    Median,
    /// 25th percentile pivot (favors high index).
    Percentile25,
    /// 75th percentile pivot (favors low index).
    Percentile75,
    /// Explicitly provided pivot.
    Explicit(u64),
}

impl Default for SplitPolicy {
    fn default() -> Self {
        SplitPolicy::Median
    }
}

impl SplitPolicy {
    /// Computes the pivot hash according to the given policy.
    ///
    /// # OOM-02
    pub fn compute_pivot(&self, src: &PathIndex) -> ExofsResult<u64> {
        match self {
            SplitPolicy::Explicit(h) => Ok(*h),
            policy => {
                let mut hashes: Vec<u64> = Vec::new();
                for e in src.entries() {
                    hashes.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    hashes.push(e.hash);
                }
                if hashes.is_empty() {
                    return Err(ExofsError::InvalidArgument);
                }
                hashes.sort_unstable();
                let n = hashes.len();
                let idx = match policy {
                    SplitPolicy::Median => n / 2,
                    SplitPolicy::Percentile25 => n / 4,
                    SplitPolicy::Percentile75 => (n * 3) / 4,
                    SplitPolicy::Explicit(_) => unreachable!(),
                };
                Ok(hashes[idx.min(n - 1)])
            }
        }
    }
}

// ── SplitRequest ──────────────────────────────────────────────────────────────

/// Split request with configurable policy.
pub struct SplitRequest {
    pub parent_oid: ObjectId,
    pub policy: SplitPolicy,
    /// If `true`, automatically verifies result after split.
    pub verify: bool,
}

impl SplitRequest {
    /// Creates a split request with median policy by default.
    pub fn new(parent_oid: ObjectId) -> Self {
        SplitRequest {
            parent_oid,
            policy: SplitPolicy::Median,
            verify: true,
        }
    }

    /// Updates policy.
    pub fn with_policy(mut self, p: SplitPolicy) -> Self {
        self.policy = p;
        self
    }

    /// Executes the split.
    pub fn execute(self, src: &mut PathIndex) -> ExofsResult<(SplitResult, SplitMetrics)> {
        let total_before = src.len();
        let source_thr = src.split_threshold;
        let pivot = self.policy.compute_pivot(src)?;
        let result = PathIndexSplitter::split_at_hash(src, self.parent_oid, pivot)?;
        if self.verify {
            PathIndexSplitter::verify_split(&result, total_before)?;
        }
        let metrics = SplitMetrics::from_result(&result, total_before, source_thr);
        Ok((result, metrics))
    }
}

// ── Utility functions ─────────────────────────────────────────────────────────

/// Splits an index using automatic split (median pivot).
///
/// Shortcut to [`PathIndexSplitter::split`].
pub fn auto_split(src: &mut PathIndex, parent_oid: ObjectId) -> ExofsResult<SplitResult> {
    PathIndexSplitter::split(src, parent_oid)
}

/// Returns `true` if index should be split according to default threshold.
pub fn should_split(idx: &PathIndex) -> bool {
    idx.needs_split()
}

/// Splits while also collecting metrics (useful for monitoring).
pub fn split_with_metrics(
    src: &mut PathIndex,
    parent_oid: ObjectId,
    policy: SplitPolicy,
) -> ExofsResult<(SplitResult, SplitMetrics)> {
    SplitRequest::new(parent_oid)
        .with_policy(policy)
        .execute(src)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::path_component::validate_component;
    use super::*;

    fn fake_oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32];
        a[0] = b;
        ObjectId(a)
    }

    fn make_index(n: usize) -> PathIndex {
        let mut idx = PathIndex::new(fake_oid(0));
        idx.split_threshold = n as u32;
        for i in 0..n {
            let name: Vec<u8> = format!("entry{:04}", i).into_bytes();
            let c = validate_component(&name).test_unwrap();
            idx.insert(&c, fake_oid(i as u8), 0).test_unwrap();
        }
        idx
    }

    #[test]
    fn test_split_basic() {
        let mut idx = make_index(10);
        let res = PathIndexSplitter::split(&mut idx, fake_oid(0)).test_unwrap();
        assert_eq!(res.low_count + res.high_count, 10);
        assert!(res.low_count > 0);
        assert!(res.high_count > 0);
    }

    #[test]
    fn test_split_too_small() {
        let mut idx = PathIndex::new(fake_oid(0));
        let c = validate_component(b"only").test_unwrap();
        idx.insert(&c, fake_oid(1), 0).test_unwrap();
        assert!(matches!(
            PathIndexSplitter::split(&mut idx, fake_oid(0)),
            Err(ExofsError::InvalidArgument)
        ));
    }

    #[test]
    fn test_verify_split() {
        let mut idx = make_index(8);
        let original = idx.len();
        let res = PathIndexSplitter::split(&mut idx, fake_oid(0)).test_unwrap();
        PathIndexSplitter::verify_split(&res, original).test_unwrap();
    }

    #[test]
    fn test_optimal_pivot() {
        let idx = make_index(10);
        let p = PathIndexSplitter::optimal_pivot(&idx);
        assert!(p.is_some());
    }

    #[test]
    fn test_split_at_hash() {
        let mut idx = make_index(10);
        let pivot = PathIndexSplitter::optimal_pivot(&idx).test_unwrap();
        let original = idx.len();
        let res = PathIndexSplitter::split_at_hash(&mut idx, fake_oid(0), pivot).test_unwrap();
        assert_eq!(res.low_count + res.high_count, original);
    }

    #[test]
    fn test_estimate_size() {
        let idx = make_index(10);
        let (low, high) = PathIndexSplitter::estimate_split_size(&idx).test_unwrap();
        assert!(low > 148);
        assert!(high > 148);
    }
}
