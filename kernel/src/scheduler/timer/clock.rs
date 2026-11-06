// kernel/src/scheduler/timer/clock.rs
//
// ═════════════════════════════════════════════════════════════════════════════
// Scheduler clock — shim to ktime_get_ns() (Exo-OS · Layer 1)
// ═════════════════════════════════════════════════════════════════════════════
//
// ## Rule ARCH-TIME-01
//   scheduler/timer/clock.rs DOES NOT measure TSC, DOES NOT calibrate.
//   scheduler_now_ns() = ktime_get_ns() — one line.
//   TSC and its calibration belong EXCLUSIVELY to
//   `arch/x86_64/time/` (sources/, calibration/, ktime.rs).
//
// ## What this module provides
//   - `monotonic_ns()` / `monotonic_us()` — monotonic time, delegates to ktime
//   - `realtime_ns()` — UNIX time, delegates to ktime
//   - `scheduler_now_ns()` — semantic alias for tick handler
//   - `rdtsc()` / `rdtscp()` — raw reads for perf profiling only
//     (not for kernel time — RULE ARCH-TIME-02)
//   - `init()` — compatibility shim; in Phase 2b ktime is init'd by
//     `time_init()` before scheduler, so init() here is a no-op.
//
// ## What this module DOES NOT provide (removed from Phase 2a)
//   - Local TSC_HZ          → use `ktime::ktime_tsc_hz()` directly
//   - Local TSC_AT_BOOT     → use `ktime::ktime_get_ns()` as needed
//   - Local tsc_to_ns()     → use `ktime::ktime_get_ns()` / ktime deltas
//   These duplicates violated ARCH-TIME-01 and created inconsistency if
//   TSC drift was corrected in ktime but not in this module.
// ═════════════════════════════════════════════════════════════════════════════

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Compatibility initialization shim — no-op in Phase 2b.
///
/// In Phase 2b, `time::time_init()` is called before `scheduler::init()`.
/// `ktime_get_ns()` is therefore already operational when scheduler starts.
///
/// Kept to avoid breaking existing call-sites.
///
/// # Safety
/// Callable from BSP only, before SMP activation.
#[allow(unused_variables)]
pub unsafe fn init(_tsc_hz: u64) {
    // Phase 2b: nothing to do here.
    // TSC frequency is stored in KtimeState (ktime.rs)
    // and updated by drift/periodic.rs.
    //
    // If ktime is not yet initialized (sequence bug), detected
    // via assertion in hrtimer::init():
    //   assert!(ktime_get_ns() > 0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Raw TSC reads — perf and diagnostics ONLY
// ─────────────────────────────────────────────────────────────────────────────
//
// RULE ARCH-TIME-02: These functions MUST NOT be used to
// measure kernel time (timeouts, deltas, deadlines).
// Use ktime_get_ns() / monotonic_ns() instead.
// These functions remain useful for micro profiling (measuring latency
// of a function in cycles) where ktime would add overhead.

/// Reads TSC via RDTSC (non-serialized) — profiling usage only.
///
/// RULE ARCH-TIME-02: DO NOT use for kernel delays/timeouts.
/// For kernel time, call `monotonic_ns()`.
#[inline(always)]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: RDTSC is available on all x86_64.
    //         Non-serialized — can be reordered by CPU or compiler.
    //         Acceptable only for profiling, not for kernel time.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack)
        );
    }
    ((hi as u64) << 32) | lo as u64
}

/// Reads TSC with best available ordered read — profiling usage only.
///
/// RULE ARCH-TIME-02: DO NOT use for kernel delays/timeouts.
/// Prefer `monotonic_ns()` which incorporates SMP per-CPU offset.
#[inline(always)]
pub fn rdtscp() -> u64 {
    crate::arch::x86_64::time::sources::tsc::read_ordered_with_cpu().0
}

// ─────────────────────────────────────────────────────────────────────────────
// Monotonic time (ns since boot) — RULE ARCH-TIME-01
// ─────────────────────────────────────────────────────────────────────────────

/// Returns monotonic time in nanoseconds since boot.
///
/// RULE ARCH-TIME-01: delegates to `ktime_get_ns()` — one line.
/// Uses ISR-safe seqlock + calibrated TSC + SMP per-CPU offset.
///
/// Semantic alias for tick handler and scheduler:
/// `scheduler_now_ns()` → `monotonic_ns()` → `ktime_get_ns()`.
#[inline]
pub fn monotonic_ns() -> u64 {
    crate::arch::x86_64::time::ktime::ktime_get_ns()
}

/// Monotonic time in microseconds since boot.
#[inline]
pub fn monotonic_us() -> u64 {
    monotonic_ns() / 1_000
}

/// Semantic alias used by tick handler and pick_next_task().
///
/// RULE ARCH-TIME-01: scheduler_now_ns() = ktime_get_ns() — one line.
/// Does not read TSC directly — applies SMP per-CPU offset.
#[inline(always)]
pub fn scheduler_now_ns() -> u64 {
    crate::arch::x86_64::time::ktime::ktime_get_ns()
}

/// Returns delta in ns between now and `start_ns`.
///
/// RULE ARCH-TIME-04: as ktime never regresses, result is ≥ 0.
#[inline(always)]
pub fn elapsed_since_ns(start_ns: u64) -> u64 {
    monotonic_ns().wrapping_sub(start_ns)
}

// ─────────────────────────────────────────────────────────────────────────────
// Real time (ns since UNIX epoch)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns UNIX real time in nanoseconds.
///
/// RULE ARCH-TIME-01: delegates to `ktime_get_wall_ns()`.
/// Wall-clock synchronization is managed by ktime.rs via `set_wall_time()`.
#[inline]
pub fn realtime_ns() -> u64 {
    crate::arch::x86_64::time::ktime::ktime_get_wall_ns()
}

/// Updates real clock (called from userspace RTC/NTP).
///
/// Delegates to `ktime::set_wall_time()` to ensure seqlock consistency.
///
/// # Safety
/// Callable once per sync source (RTC or NTP).
/// Thread-safe via WALL_STATE seqlock in ktime.rs.
pub unsafe fn set_realtime_from_epoch(epoch_ns: u64) {
    crate::arch::x86_64::time::ktime::set_wall_time(epoch_ns);
}

/// Returns UNIX offset in ns (wall_ns = monotonic_ns + offset).
#[inline]
pub fn realtime_offset_ns() -> u64 {
    crate::arch::x86_64::time::ktime::ktime_rtoffset_ns()
}
