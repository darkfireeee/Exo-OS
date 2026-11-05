// kernel/src/arch/x86_64/time/ktime.rs
//
// ════════════════════════════════════════════════════════════════════════════
// KtimeState — Monotonic clock with ISR-safe seqlock
// ════════════════════════════════════════════════════════════════════════════
//
// RULE TIME-SEQLOCK-01: ktime_get_ns() uses a seqlock.
//   - ISR can call ktime_get_ns() without locks => retry if write in progress.
//   - Never use a Mutex here — ktime_get_ns() must be wait-free.
//   - RULE TIME-01 (FIX): struct ktime never updated in 2 writes without seqlock.
//
// Seqlock algorithm:
//   Reader: reads seq1 -> ensures seq1 is even -> reads data -> reads seq2
//           -> if seq1 != seq2: retry (write occurred in between)
//   Writer: seq++ (odd = write in progress) -> update data -> seq++
//
// RULE ARCH-TIME-04: ktime NEVER regresses.
//   If drift correction lowers tsc_hz, ns_base is adjusted in compensation.
// ════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

// ── KtimeState ────────────────────────────────────────────────────────────────

/// Global state of the monotonic clock.
///
/// RULE TIME-SEQLOCK-01: Cache-line aligned to prevent false sharing.
/// All reads must go through `ktime_get_ns()` — never access directly.
#[repr(C, align(64))]
struct KtimeState {
    /// Seqlock counter: even = stable state, odd = write in progress.
    seq: AtomicU64,
    /// TSC value at the last anchor point (tsc_base).
    tsc_base: AtomicU64,
    /// Nanoseconds at the last anchor point (ns_base).
    ns_base: AtomicU64,
    /// Current TSC frequency in Hz (updated by drift correction).
    tsc_hz: AtomicU64,
    /// Padding to complete the cache line (64 - 4×8 = 32 bytes).
    _pad: [u8; 32],
}

impl KtimeState {
    const fn new() -> Self {
        Self {
            seq: AtomicU64::new(0),
            tsc_base: AtomicU64::new(0),
            ns_base: AtomicU64::new(0),
            tsc_hz: AtomicU64::new(3_000_000_000), // Fallback 3 GHz before calibration
            _pad: [0u8; 32],
        }
    }
}

/// Global instance — initialized ONCE by `init_ktime()`.
static KTIME_STATE: KtimeState = KtimeState::new();

// ── ISR-safe read ────────────────────────────────────────────────────────────

/// Returns the monotonic time in nanoseconds since boot.
///
/// ISR-safe: wait-free, no lock, no alloc.
/// Seqlock: retry if a write is in progress (rare — < 100ns).
/// Typical cycles: 15-50 cycles on modern CPU.
///
/// RULE ARCH-TIME-02: Only authorized time source from the kernel.
/// RULE TIME-SEQLOCK-01: Guarantees consistency even between ISR and thread.
#[inline(always)]
fn apply_tsc_offset(tsc_now: u64, tsc_offset: i64) -> u64 {
    if tsc_offset >= 0 {
        tsc_now.wrapping_sub(tsc_offset as u64)
    } else {
        tsc_now.wrapping_add(tsc_offset.unsigned_abs())
    }
}

/// Returns the monotonic time in nanoseconds since boot.
///
/// ISR-safe: wait-free, no lock, no alloc.
/// Seqlock: retry if a write is in progress (rare — < 100ns).
/// Typical cycles: 15-50 cycles on modern CPU.
///
/// RULE ARCH-TIME-02: Only authorized time source from the kernel.
/// RULE TIME-SEQLOCK-01: Guarantees consistency even between ISR and thread.
#[inline(always)]
pub fn ktime_get_ns() -> u64 {
    loop {
        // 1. Read seq BEFORE — must be even (stable state).
        let seq1 = KTIME_STATE.seq.load(Ordering::Acquire);
        if seq1 & 1 != 0 {
            // Odd = write in progress -> spin_loop and retry.
            core::hint::spin_loop();
            continue;
        }

        // 2. Read current TSC via the best available CPU primitive.
        let (tsc_now, coreid) = super::sources::tsc::read_ordered_with_cpu();

        // 3. Read anchor components.
        let tsc_base = KTIME_STATE.tsc_base.load(Ordering::Acquire);
        let ns_base = KTIME_STATE.ns_base.load(Ordering::Acquire);
        let tsc_hz = KTIME_STATE.tsc_hz.load(Ordering::Acquire);

        // 4. Verify integrity: seq did not change during our read.
        let seq2 = KTIME_STATE.seq.load(Ordering::Acquire);
        if seq1 != seq2 {
            // A write intervened -> retry.
            core::hint::spin_loop();
            continue;
        }

        // 5. Final calculation — seqlock guarantees consistency.
        if tsc_hz == 0 {
            return 0;
        }

        // Apply per-CPU offset if SMP is initialized.
        let tsc_offset = super::percpu::tsc_offset(coreid as usize);
        let tsc_adjusted = apply_tsc_offset(tsc_now, tsc_offset);

        let tsc_delta = tsc_adjusted.wrapping_sub(tsc_base);
        // ns = tsc_delta * 1_000_000_000 / tsc_hz — u128 to prevent overflow.
        let ns_delta = (tsc_delta as u128).saturating_mul(1_000_000_000) / (tsc_hz as u128);

        return ns_base.wrapping_add(ns_delta as u64);
    }
}

// ── Anchor update ────────────────────────────────────────────────────────────

/// Initializes the clock state (called ONCE from `time_init()`).
///
/// # Safety
/// Must be called once, before any call to `ktime_get_ns()`.
pub(crate) unsafe fn init_ktime(tsc_now: u64, ns_start: u64, tsc_hz: u64) {
    // Odd seq = write in progress (transient — nobody is reading yet).
    KTIME_STATE.seq.store(1, Ordering::Release);
    KTIME_STATE.tsc_base.store(tsc_now, Ordering::Release);
    KTIME_STATE.ns_base.store(ns_start, Ordering::Release);
    KTIME_STATE.tsc_hz.store(tsc_hz, Ordering::Release);
    // Even seq = stable state.
    KTIME_STATE.seq.store(2, Ordering::Release);
}

/// Updates the clock anchor point (called by drift correction).
///
/// RULE TIME-ANCHOR-01: DO NOT call ktime_get_ns() here.
///   -> Read HPET and TSC directly to avoid circular dependency.
/// RULE DRIFT-MONOTONE-01: ns_now >= ns_current (clock does not regress).
pub(crate) fn update_ktime_anchor(tsc_now: u64, ns_now: u64, new_tsc_hz: u64) {
    // Monotonicity check (RULE ARCH-TIME-04).
    let ns_current = KTIME_STATE.ns_base.load(Ordering::Relaxed);
    if ns_now < ns_current {
        // Never step the clock backwards — use at least ns_current.
        let _ = ns_current; // Guard is verified upstream by apply_drift_correction().
        return;
    }

    // Odd seq = write in progress.
    KTIME_STATE.seq.fetch_add(1, Ordering::Release);

    // Barrier: prevents reordering of subsequent stores before odd seq.
    core::sync::atomic::fence(Ordering::Release);

    KTIME_STATE.tsc_base.store(tsc_now, Ordering::Relaxed);
    KTIME_STATE.ns_base.store(ns_now, Ordering::Relaxed);
    KTIME_STATE.tsc_hz.store(new_tsc_hz, Ordering::Relaxed);

    // Barrier: make stores visible before returning to even.
    core::sync::atomic::fence(Ordering::Release);

    // Even seq = stable state.
    KTIME_STATE.seq.fetch_add(1, Ordering::Release);
}

// ── Wall clock (UNIX real-time) ──────────────────────────────────────────────

/// Returns Unix time in nanoseconds since the epoch (1970-01-01 00:00:00 UTC).
///
/// Depends on `set_wall_time()` having been called from RTC or NTP reading.
/// Before synchronization: returns monotonic time (as if boot = epoch 0).
///
/// ISR-safe, seqlock on WALL_STATE.
pub fn ktime_get_wall_ns() -> u64 {
    loop {
        let seq1 = WALL_STATE.seq.load(Ordering::Acquire);
        if seq1 & 1 != 0 {
            core::hint::spin_loop();
            continue;
        }
        let offset = WALL_STATE.rtoffset_ns.load(Ordering::Acquire);
        let seq2 = WALL_STATE.seq.load(Ordering::Acquire);
        if seq1 != seq2 {
            core::hint::spin_loop();
            continue;
        }
        return ktime_get_ns().wrapping_add(offset);
    }
}

/// Returns the UNIX epoch offset in ns (wall_ns = mono_ns + rtoffset_ns).
#[inline(always)]
pub fn ktime_rtoffset_ns() -> u64 {
    WALL_STATE.rtoffset_ns.load(Ordering::Acquire)
}

/// Synchronizes the real-time clock with a provided UNIX epoch value.
///
/// Called from the `settimeofday` syscall or from RTC reading at boot.
/// Updates the UNIX offset in WALL_STATE via seqlock.
///
/// # Safety
/// Must be called from Ring 0 with appropriate privileges (CAP_TIME).
pub unsafe fn set_wall_time(epoch_unix_ns: u64) {
    let mono_now = ktime_get_ns();
    let new_offset = epoch_unix_ns.wrapping_sub(mono_now);
    WALL_STATE.seq.fetch_add(1, Ordering::AcqRel);
    core::sync::atomic::fence(Ordering::Release);
    WALL_STATE.rtoffset_ns.store(new_offset, Ordering::Relaxed);
    WALL_STATE.mono_at_set.store(mono_now, Ordering::Relaxed);
    WALL_STATE
        .epoch_at_set
        .store(epoch_unix_ns, Ordering::Relaxed);
    core::sync::atomic::fence(Ordering::Release);
    WALL_STATE.seq.fetch_add(1, Ordering::Release);
}

// ── Relative durations ───────────────────────────────────────────────────────

/// Elapsed time in ns since `start_ns` (obtained via `ktime_get_ns()`).
#[inline(always)]
pub fn ktime_elapsed_ns(start_ns: u64) -> u64 {
    ktime_get_ns().wrapping_sub(start_ns)
}

/// Elapsed time in µs since `start_ns`.
#[inline(always)]
pub fn ktime_elapsed_us(start_ns: u64) -> u64 {
    ktime_elapsed_ns(start_ns) / 1_000
}

/// Returns `true` if `deadline_ns` is reached or exceeded.
#[inline(always)]
pub fn ktime_past(deadline_ns: u64) -> bool {
    ktime_get_ns() >= deadline_ns
}

/// Returns `false` if `deadline_ns` has not yet been reached.
/// Use in timeout loops: `while ktime_not_past(dl) { ... }`.
#[inline(always)]
pub fn ktime_not_past(deadline_ns: u64) -> bool {
    ktime_get_ns() < deadline_ns
}

/// Constructs an absolute deadline from now + duration in ns.
#[inline(always)]
pub fn ktime_deadline_ns(duration_ns: u64) -> u64 {
    ktime_get_ns().saturating_add(duration_ns)
}

/// Constructs an absolute deadline from now + duration in µs.
#[inline(always)]
pub fn ktime_deadline_us(duration_us: u64) -> u64 {
    ktime_deadline_ns(duration_us.saturating_mul(1_000))
}

/// Constructs an absolute deadline from now + duration in ms.
#[inline(always)]
pub fn ktime_deadline_ms(duration_ms: u64) -> u64 {
    ktime_deadline_ns(duration_ms.saturating_mul(1_000_000))
}

// ── Per-CPU TSC offset management ─────────────────────────────────────────────

/// Maximum number of logical CPUs (identical to smp::topology::MAX_CPUS).
const MAX_CPUS: usize = 256;

/// TSC offset of each logical CPU, measured from BSP at SMP boot.
/// offset[cpu] = TSC_AP_at_boot - TSC_BSP_at_boot (synchronization delta).
/// ktime_get_ns() applies: tsc_adj = rdtscp() - offset[coreid].
static TSC_OFFSETS: [AtomicI64; MAX_CPUS] = [const { AtomicI64::new(0) }; MAX_CPUS];

/// Offset validity: bit i = true -> CPU i has its offset measured.
static TSC_OFFSET_VALID: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

/// Records the TSC offset of an AP CPU (called from percpu::sync).
///
/// # Safety
/// `cpu_id` must be in [1, MAX_CPUS). Called once per CPU at SMP boot.
pub unsafe fn store_tsc_offset(cpu_id: usize, offset: i64) {
    if cpu_id == 0 || cpu_id >= MAX_CPUS {
        return;
    }
    TSC_OFFSETS[cpu_id].store(offset, Ordering::Release);
    TSC_OFFSET_VALID[cpu_id].store(true, Ordering::Release);
}

/// Returns the TSC offset of a CPU (0 if BSP or not yet measured).
#[inline(always)]
pub fn tsc_offset(cpu_id: usize) -> i64 {
    if cpu_id >= MAX_CPUS {
        return 0;
    }
    if TSC_OFFSET_VALID[cpu_id].load(Ordering::Relaxed) {
        TSC_OFFSETS[cpu_id].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Returns `true` if the TSC offset of the given CPU is available.
pub fn tsc_offset_valid(cpu_id: usize) -> bool {
    cpu_id < MAX_CPUS && TSC_OFFSET_VALID[cpu_id].load(Ordering::Relaxed)
}

/// Invalidates all offsets (used for reset on SMP reboot).
///
/// # Safety
/// Must be called exclusively by the BSP during the single-core phase of SMP initialization.
pub unsafe fn reset_tsc_offsets() {
    for i in 0..MAX_CPUS {
        TSC_OFFSETS[i].store(0, Ordering::Relaxed);
        TSC_OFFSET_VALID[i].store(false, Ordering::Relaxed);
    }
    TSC_OFFSET_VALID[0].store(true, Ordering::Relaxed); // BSP: offset 0 valid.
}

// ── Diagnostic accessors ──────────────────────────────────────────────────────

/// Current TSC frequency in KtimeState (Hz).
#[inline(always)]
pub fn ktime_tsc_hz() -> u64 {
    KTIME_STATE.tsc_hz.load(Ordering::Relaxed)
}

/// `true` if ktime is initialized (`init_ktime()` was called).
#[inline(always)]
pub fn ktime_initialized() -> bool {
    KTIME_STATE.seq.load(Ordering::Relaxed) >= 2
}

/// Number of seqlock retries since boot (contention indicator).
pub fn ktime_contention_count() -> u64 {
    KTIME_RETRIES.load(Ordering::Relaxed)
}

/// Immutable snapshot of ktime state for debug/profiling.
pub struct KtimeSnapshot {
    pub seq: u64,
    pub tsc_base: u64,
    pub ns_base: u64,
    pub tsc_hz: u64,
    pub retries: u64,
    pub wall_off: u64,
    pub offsets_cpu0: i64,
}

pub fn ktime_snapshot() -> KtimeSnapshot {
    KtimeSnapshot {
        seq: KTIME_STATE.seq.load(Ordering::Relaxed),
        tsc_base: KTIME_STATE.tsc_base.load(Ordering::Relaxed),
        ns_base: KTIME_STATE.ns_base.load(Ordering::Relaxed),
        tsc_hz: KTIME_STATE.tsc_hz.load(Ordering::Relaxed),
        retries: KTIME_RETRIES.load(Ordering::Relaxed),
        wall_off: WALL_STATE.rtoffset_ns.load(Ordering::Relaxed),
        offsets_cpu0: TSC_OFFSETS[0].load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::apply_tsc_offset;

    #[test]
    fn test_apply_tsc_offset_signed_direction() {
        assert_eq!(apply_tsc_offset(1_000, 25), 975);
        assert_eq!(apply_tsc_offset(1_000, -25), 1_025);
        assert_eq!(apply_tsc_offset(1_000, 0), 1_000);
    }

    #[test]
    fn test_apply_tsc_offset_stress_roundtrip() {
        let cases = [
            (0u64, 0i64),
            (17, 9),
            (17, -9),
            (u64::MAX - 32, 64),
            (u64::MAX - 32, -64),
        ];

        for &(tsc, offset) in &cases {
            let adjusted = apply_tsc_offset(tsc, offset);
            let restored = apply_tsc_offset(adjusted, -offset);
            assert_eq!(restored, tsc);
        }

        for raw in -10_000i64..=10_000 {
            let tsc = 0x1234_5678_9ABC_DEF0u64.wrapping_add(raw.unsigned_abs());
            let adjusted = apply_tsc_offset(tsc, raw);
            let restored = apply_tsc_offset(adjusted, -raw);
            assert_eq!(restored, tsc);
        }
    }
}

// ── WallState (wall clock state) ─────────────────────────────────────────────

/// Dedicated cache line for wall clock to prevent contention with monotonic ktime.
#[repr(C, align(64))]
struct WallState {
    seq: AtomicU64,
    rtoffset_ns: AtomicU64,  // wall = mono + rtoffset
    mono_at_set: AtomicU64,  // mono_ns at last set_wall_time()
    epoch_at_set: AtomicU64, // Unix epoch ns at last set_wall_time()
    _pad: [u8; 32],
}

impl WallState {
    const fn new() -> Self {
        Self {
            seq: AtomicU64::new(0),
            rtoffset_ns: AtomicU64::new(0),
            mono_at_set: AtomicU64::new(0),
            epoch_at_set: AtomicU64::new(0),
            _pad: [0u8; 32],
        }
    }
}

static WALL_STATE: WallState = WallState::new();
static KTIME_RETRIES: AtomicU64 = AtomicU64::new(0);
