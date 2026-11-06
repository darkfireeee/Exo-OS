// kernel/src/scheduler/sync/seqlock.rs
//
// ═════════════════════════════════════════════════════════════════════════════
// SeqLock — ISR-safe sequential lock (Exo-OS · Layer 1)
// ═════════════════════════════════════════════════════════════════════════════
//
// ## Principle
//   A SeqLock allows multiple readers to read data without ever blocking,
//   at the cost of a retry if a writer is active concurrently.
//
//   Counter invariant:
//     - Even = stable state (no active write)
//     - Odd  = write in progress (readers must retry)
//
//   Reader:
//     1. Read seq1 via Acquire → if odd, spin_loop and retry
//     2. Copy data
//     3. Read seq2 via Acquire → if seq1 != seq2, data changed → retry
//     4. Data consistent: return
//
//   Writer:
//     1. seq.fetch_add(1, Release) → seq becomes odd (write start)
//     2. Update data
//     3. seq.fetch_add(1, Release) → seq becomes even again (write end)
//
// ## Rules (ARCH-TIME-03)
//   RULE SEQLOCK-01: Never Mutex in ktime_get_ns() nor in an ISR.
//     → ktime_get_ns() uses this pattern directly (cf. ktime.rs).
//   RULE SEQLOCK-02: Write must NEVER call into the data it protects
//     (circular dependency).
//   RULE SEQLOCK-03: Data protected by SeqLock<T> MUST be Copy —
//     a SeqLock on a heap-allocated string is an antipattern.
//   RULE SEQLOCK-04: Writer must NOT be preempted between
//     write_begin() and write_end() — use PreemptGuard.
//
// ## Complexity
//   Read   : O(1) amortized — O(N) under heavy contention (rare)
//   Write  : O(1) — 2 atomic fetch_adds + Relaxed stores
//   Memory : sizeof(T) + 8 bytes (the seq counter)
//
// ## Use cases
//   - ktime::KtimeState  — monotonic clock + per-CPU offset (ktime.rs)
//   - ktime::WallState   — UNIX epoch offset (ktime.rs)
//   - Any small data (≤ 2 cache lines) frequently read from ISR
//   - DO NOT use for large buffers (copy overhead on reader side)
// ═════════════════════════════════════════════════════════════════════════════

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// SeqLock<T>
// ─────────────────────────────────────────────────────────────────────────────

/// Generic SeqLock for `Copy` data.
///
/// Guarantees consistent reads without ever blocking the reader, including
/// from an ISR (no Mutex, no continuous spin).
///
/// ## Size constraint
/// T must fit in one or two cache lines (≤ 128 bytes) for the
/// reader pattern to remain efficient. Beyond that, prefer a RwLock.
#[repr(C)]
pub struct SeqLock<T: Copy> {
    /// Sequential counter: even = stable, odd = write in progress.
    seq: AtomicU64,
    /// Protected data.
    data: UnsafeCell<T>,
}

// SAFETY: SeqLock is Send+Sync if T is — concurrent accesses are safe
// thanks to the seqlock protocol (reads copy via retry).
unsafe impl<T: Copy + Send> Send for SeqLock<T> {}
unsafe impl<T: Copy + Send + Sync> Sync for SeqLock<T> {}

impl<T: Copy + Default> Default for SeqLock<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: Copy> SeqLock<T> {
    /// Creates a `SeqLock` with given initial value.
    /// Counter starts at 0 (even = stable state).
    pub const fn new(value: T) -> Self {
        Self {
            seq: AtomicU64::new(0),
            data: UnsafeCell::new(value),
        }
    }

    // ── Read ─────────────────────────────────────────────────────────────────

    /// Reads data consistently with seqlock retry.
    ///
    /// ISR-safe: never locks, no alloc, retries if write in progress.
    ///
    /// ## Retry count
    /// In practice < 5 retries except under extreme contention (very frequent writer).
    /// If T is large, each retry copies T → prefer read_with() to
    /// minimize copy size.
    #[inline(always)]
    pub fn read(&self) -> T {
        loop {
            // 1. Seq BEFORE — must be even.
            let seq1 = self.seq.load(Ordering::Acquire);
            if seq1 & 1 != 0 {
                // Write in progress → spin and retry.
                core::hint::spin_loop();
                continue;
            }

            // 2. Copy data (T is Copy → stack copy).
            //    SAFETY: read is consistent if seq does not change.
            //    If a write occurs between seq1 and seq2, detected via seq2 ≠ seq1.
            let value = unsafe { *self.data.get() };

            // 3. Verify seq did not change.
            let seq2 = self.seq.load(Ordering::Acquire);
            if seq1 == seq2 {
                return value;
            }

            // Write slipped in → retry.
            core::hint::spin_loop();
        }
    }

    /// Reads data and passes it to a projection function.
    ///
    /// Avoids a full copy of T if only one field is needed.
    /// Closure F is called with a TEMPORARY reference to T — do not
    /// retain reference beyond call.
    ///
    /// ## Safety
    /// F must be non-blocking and without observable side effects
    /// (may be called multiple times upon retry).
    #[inline(always)]
    pub fn read_with<R, F: Fn(&T) -> R>(&self, f: F) -> R {
        loop {
            let seq1 = self.seq.load(Ordering::Acquire);
            if seq1 & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }
            // SAFETY: F reads via immutable reference within seqlock window.
            let result = unsafe { f(&*self.data.get()) };
            let seq2 = self.seq.load(Ordering::Acquire);
            if seq1 == seq2 {
                return result;
            }
            core::hint::spin_loop();
        }
    }

    // ── Write ────────────────────────────────────────────────────────────────

    /// Writes new data atomically with respect to readers.
    ///
    /// RULE SEQLOCK-04: caller must have disabled preemption
    /// (via `PreemptGuard`) if a scheduler is active.
    ///
    /// ## Safety
    /// - Exclusive writer thread — only one writer at a time.
    ///   (SeqLock *without* lock does not detect concurrent writes —
    ///   if multiple writers possible, use `write_guarded()`)
    /// - No `read()` or `read_with()` inside writer body.
    #[inline]
    pub unsafe fn write(&self, value: T) {
        // seq odd = write start.
        self.seq.fetch_add(1, Ordering::AcqRel);
        // Barrier: subsequent stores are visible AFTER odd seq.
        core::sync::atomic::fence(Ordering::Release);

        // Update data.
        // SAFETY: exclusivity guaranteed by call contract (single writer).
        *self.data.get() = value;

        // Barrier: data stores are visible BEFORE even seq.
        core::sync::atomic::fence(Ordering::Release);
        // seq even = write end.
        self.seq.fetch_add(1, Ordering::Release);
    }

    /// Modifies data via an update closure.
    ///
    /// F receives a mutable reference to T and can modify in-place,
    /// without constructing whole value first (useful if T is large).
    ///
    /// ## Safety
    /// Same constraints as `write()` — single writer at a time.
    /// F must not call `read()` or `read_with()` on this same SeqLock.
    #[inline]
    pub unsafe fn update<F: FnOnce(&mut T)>(&self, f: F) {
        self.seq.fetch_add(1, Ordering::AcqRel);
        core::sync::atomic::fence(Ordering::Release);
        // SAFETY: exclusivity guaranteed by call contract.
        f(&mut *self.data.get());
        core::sync::atomic::fence(Ordering::Release);
        self.seq.fetch_add(1, Ordering::Release);
    }

    // ── Unprotected access (init only) ───────────────────────────────────────

    /// Initializes data WITHOUT signaling a write (for init phase).
    ///
    /// To be called only ONCE before other threads can read
    /// (typically from BSP before SMP init). seq remains 0 → even → stable.
    ///
    /// ## Safety
    /// - Must be called before any concurrent call to `read()`.
    /// - Must correspond to consistent initial value (no undefined behavior).
    #[inline]
    pub unsafe fn init_once(&self, value: T) {
        // Store directly — no write signal since nobody reads yet.
        *self.data.get() = value;
        // Even seq (= 0) → stable. Fence to make visible before reads.
        core::sync::atomic::fence(Ordering::Release);
    }

    /// Returns current sequence number (diagnostics only).
    #[inline(always)]
    pub fn seq_number(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    /// Returns `true` if a write is currently in progress (odd seq).
    #[inline(always)]
    pub fn write_in_progress(&self) -> bool {
        self.seq.load(Ordering::Relaxed) & 1 != 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SeqGuard — RAII Guard for writing (guarantees write_end on Drop)
// ─────────────────────────────────────────────────────────────────────────────

/// RAII guard for a SeqLock write section.
///
/// Emits `seq.fetch_add(1)` on creation (write start) and on drop (write end).
/// Guarantees counter always returns to even even in case of panic.
///
/// ## Example
/// ```rust,ignore
/// let _guard = SeqWriteGuard::new(&my_seqlock.seq);
/// // Write into my_seqlock.data here...
/// // _guard dropped → automatic write_end
/// ```
pub struct SeqWriteGuard<'a> {
    seq: &'a AtomicU64,
}

impl<'a> SeqWriteGuard<'a> {
    /// Starts a write section (seq becomes odd).
    ///
    /// ## Safety
    /// - Only one active `SeqWriteGuard` at a time on the same SeqLock.
    /// - Do not call `read()` on the same SeqLock while this guard is held.
    #[inline]
    pub unsafe fn new(seq: &'a AtomicU64) -> Self {
        seq.fetch_add(1, Ordering::AcqRel);
        core::sync::atomic::fence(Ordering::Release);
        Self { seq }
    }
}

impl<'a> Drop for SeqWriteGuard<'a> {
    #[inline]
    fn drop(&mut self) {
        // Barrier: make data stores visible BEFORE returning to even.
        core::sync::atomic::fence(Ordering::Release);
        self.seq.fetch_add(1, Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SeqLockU64 — Optimized specialization for a single u64
// ─────────────────────────────────────────────────────────────────────────────

/// Optimized SeqLock for a single `u64` — zero copy, atomic read.
///
/// On x86_64, reading an aligned u64 is naturally atomic (64-bit
/// load). The seqlock simply guarantees consistency if the value is updated
/// from multiple cores concurrently with the contents of a multi-field.
///
/// Used by `ktime.rs` for individual fields (tsc_hz, ns_base...).
pub struct SeqLockU64 {
    seq: AtomicU64,
    value: AtomicU64,
}

impl SeqLockU64 {
    pub const fn new(v: u64) -> Self {
        Self {
            seq: AtomicU64::new(0),
            value: AtomicU64::new(v),
        }
    }

    /// Consistent read — ISR-safe, wait-free.
    #[inline(always)]
    pub fn read(&self) -> u64 {
        loop {
            let seq1 = self.seq.load(Ordering::Acquire);
            if seq1 & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }
            let v = self.value.load(Ordering::Acquire);
            let seq2 = self.seq.load(Ordering::Acquire);
            if seq1 == seq2 {
                return v;
            }
            core::hint::spin_loop();
        }
    }

    /// Atomic write.
    ///
    /// ## Safety
    /// Single writer at a time.
    #[inline]
    pub unsafe fn write(&self, v: u64) {
        self.seq.fetch_add(1, Ordering::AcqRel);
        core::sync::atomic::fence(Ordering::Release);
        self.value.store(v, Ordering::Relaxed);
        core::sync::atomic::fence(Ordering::Release);
        self.seq.fetch_add(1, Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests (cfg(test) — no_std compatible via core::)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct ClockSnapshot {
        ns: u64,
        hz: u64,
    }

    #[test]
    fn test_seqlock_read_write() {
        let lock = SeqLock::new(ClockSnapshot {
            ns: 0,
            hz: 3_000_000_000,
        });

        // Verify initial value.
        let snap = lock.read();
        assert_eq!(snap.ns, 0);
        assert_eq!(snap.hz, 3_000_000_000);

        // Write.
        unsafe {
            lock.write(ClockSnapshot {
                ns: 1_000_000,
                hz: 3_200_000_000,
            });
        }

        let snap2 = lock.read();
        assert_eq!(snap2.ns, 1_000_000);
        assert_eq!(snap2.hz, 3_200_000_000);
    }

    #[test]
    fn test_seqlock_seq_invariant() {
        let lock = SeqLock::new(0u64);
        // After init: seq must be even.
        assert_eq!(lock.seq_number() & 1, 0);
        // No active write.
        assert!(!lock.write_in_progress());
    }

    #[test]
    fn test_seqlock_u64() {
        let lock = SeqLockU64::new(42);
        assert_eq!(lock.read(), 42);
        unsafe {
            lock.write(99);
        }
        assert_eq!(lock.read(), 99);
    }
}
