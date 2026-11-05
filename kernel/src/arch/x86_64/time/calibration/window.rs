// kernel/src/arch/x86_64/time/calibration/window.rs
//
// ════════════════════════════════════════════════════════════════════════════
// TSC calibration over REAL time window — bare-metal ready
// ════════════════════════════════════════════════════════════════════════════
//
// FIX TIME-02: Loop condition = elapsed HPET/PM Timer ticks (never iterations).
//   BEFORE (buggy): while iter < MAX_MEASURE_ITERS { ... }
//                   -> bare-metal: 500 * 5ns = 2.5µs << 10ms -> always fails
//   AFTER (correct): while hpet_elapsed < target_ticks { ... }
//                   -> bare-metal: loop runs ~1ms of real time -> precise TSC delta
//                   -> QEMU      : loop runs ~1ms of real time -> same
//
// FIX TIME-03: CLI/STI per sample of 1ms MAX (not 10ms global).
//   BEFORE (buggy): cli ... 10ms of measurement ... sti
//                   -> lost LAPIC timer IRQ -> RT thread never wakes up
//   AFTER (correct): 10 samples * (cli, 1ms measurement, sti)
//                   -> max loss: 1ms of IRQ per sample, outlier rejection on 10
//
// RULE CAL-RDTSCP-01: ALWAYS RDTSCP (not RDTSC) during calibration.
//   RDTSC can be out-of-order -> measurement bias.
//   RDTSCP serializes + provides coreid -> measurement anchored to proper core.
// ════════════════════════════════════════════════════════════════════════════

use super::super::sources::hpet as hpet_src;
use super::super::sources::pm_timer as pm_src;
use core::sync::atomic::Ordering;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Number of 1ms samples for multi-sample calibration.
pub const N_SAMPLES: usize = 10;

/// Initial synchronization limit (wait for 1 HPET/PM tick).
/// 20 iterations max — on QEMU TCG, each HPET MMIO read is costly (~1µs–60ms).
/// If HPET does not change within 20 consecutive reads -> likely disabled.
/// On bare-metal: 1 iteration is usually enough (HPET advances at 70ns/tick).
const MAX_SYNC_ITERS: u32 = 20;

/// Maximum limit of MMIO iterations in main measurement loop.
/// The HPET window measures 1 ms real: on fast reads it may require
/// tens of thousands of polls before reaching `target_ticks`. The TSC
/// timeout remains the true temporal guard; this bound merely prevents an
/// unbounded loop if the counter increments without ever reaching the window.
const N_MAX_POLL_MMIO: u64 = 200_000;

/// Backup TSC timeout if `cpu::tsc` does not yet have a usable frequency.
/// The normal path derives the guard from `tsc_hz()`. Before the first
/// calibration, the bound must cover 100 ms even at the validated ceiling of 10 GHz;
/// otherwise QEMU could abandon a valid HPET window after only a few slow
/// MMIO reads.
/// RDTSC is always fast (no MMIO) — the guard does not slow down the loop.
/// NOTE: if timeout triggers, returns None immediately -> fallback chain.
const SAMPLE_TSC_TIMEOUT_FALLBACK_CYCLES: u64 = 1_000_000_000;
const SAMPLE_TSC_TIMEOUT_MS: u64 = 100;

#[inline]
fn sample_tsc_timeout_cycles() -> u64 {
    let hz = crate::arch::x86_64::cpu::tsc::tsc_hz();
    if hz == 0 {
        SAMPLE_TSC_TIMEOUT_FALLBACK_CYCLES
    } else {
        hz.saturating_mul(SAMPLE_TSC_TIMEOUT_MS)
            .saturating_div(1_000)
            .max(SAMPLE_TSC_TIMEOUT_FALLBACK_CYCLES)
    }
}

// ── Calibration via HPET ─────────────────────────────────────────────────────

/// Calibrates TSC Hz using HPET as time reference.
///
/// RULE CAL-WINDOW-01: loop condition = HPET ticks, never iterations.
/// RULE CAL-CLI-01   : cli/sti per sample of 1ms MAX.
/// RULE CAL-RDTSCP-01: RDTSCP, not RDTSC.
///
/// Accuracy : ±0.01% (HPET tick = ~70ns at 14.318 MHz).
/// Duration : 10ms of real time (10 * 1ms).
///            Bare-metal: ~10ms | QEMU TCG: ~10ms.
///
/// Returns `None` if HPET unavailable or measurement out of range.
pub fn calibrate_tsc_via_hpet(hpet_freq_hz: u64) -> Option<u64> {
    if hpet_freq_hz == 0 {
        hpet_fail_tag(b"NO-FREQ");
        return None;
    }

    // 1ms of HPET ticks.
    let target_ticks: u64 = hpet_freq_hz / 1_000;
    if target_ticks == 0 {
        hpet_fail_tag(b"NO-TICKS");
        return None;
    }

    let mut samples = [0u64; N_SAMPLES];

    for sample in &mut samples {
        // ── Synchronization: wait for HPET to advance by at least 1 tick ─────
        // Protection: if HPET stopped/broken, limit iterations to avoid deadlock.
        let sync_val = hpet_src::read();
        let mut synced = false;
        for _ in 0..MAX_SYNC_ITERS {
            let v = hpet_src::read();
            if v != sync_val {
                synced = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !synced {
            hpet_fail_tag(b"SYNC");
            return None;
        }

        // ── 1ms measurement WITHOUT CLI (HPET loop) ──────────────────────────
        // RULE CAL-CLI-01: CLI around rdtsc_begin/rdtscp_end ONLY,
        // not around the entire HPET wait loop.
        // Reason: on QEMU TCG, CLI during MMIO busy-polling prevents virtual
        // HPET clock advancement (QEMU stops dispatching internal timers).
        // On bare-metal: IRQs are allowed during wait — measurement is
        // based on HPET ticks (real physical time), not TSC alone.
        let hpet_start = hpet_src::read();

        // ✅ CAL-CLI-01 HPET VARIANT: CLI only for serialized rdtsc_begin.
        let flags = flags_save_cli();
        let tsc_start = rdtsc_begin();
        unsafe {
            flags_restore(flags);
        }

        // HPET wait loop — IRQs allowed (see note above).
        // DOUBLE GUARD:
        //   1. HPET ticks: normal exit condition (TIME-02)
        //   2. N_MAX_POLL_MMIO: limits costly MMIO reads on QEMU TCG
        //   3. TSC timeout: final guard if TSC is available
        let tsc_deadline = tsc_start.wrapping_add(sample_tsc_timeout_cycles());
        let mut poll_count: u64 = 0;
        loop {
            let hpet_now = hpet_src::read();
            poll_count = poll_count.wrapping_add(1);
            // RULE TIME-08: wrapping_sub handles 32/64-bit rollover.
            if hpet_now.wrapping_sub(hpet_start) >= target_ticks {
                break;
            }
            // MMIO GUARD: max N_MAX_POLL_MMIO reads (protects against very slow HPET).
            if poll_count >= N_MAX_POLL_MMIO {
                hpet_fail_tag(b"POLL");
                return None;
            }
            // TSC GUARD: abort if TSC advanced past timeout.
            let elapsed = rdtsc_begin().wrapping_sub(tsc_start);
            if elapsed >= tsc_deadline.wrapping_sub(tsc_start) {
                hpet_fail_tag(b"TSC");
                return None;
            }
            core::hint::spin_loop();
        }

        // ✅ CAL-CLI-01: CLI only for serialized rdtscp_end.
        let flags2 = flags_save_cli();
        let (tsc_end, _coreid) = rdtscp_end();
        unsafe {
            flags_restore(flags2);
        }

        let tsc_delta = tsc_end.wrapping_sub(tsc_start);
        // Extrapolation: delta over 1ms -> Hz for 1 second.
        *sample = tsc_delta.saturating_mul(1_000);
    }

    // ── Outlier rejection by IQR ──────────────────────────────────────────────
    // Sort samples and retain those within [Q1 - 1.5×IQR, Q3 + 1.5×IQR].
    samples.sort_unstable();
    let tsc_hz = iqr_mean(&samples);

    let checked = super::validation::cross_check(tsc_hz);
    if checked.is_none() {
        hpet_fail_tag(b"VALID");
    }
    checked
}

// ── Calibration via PM Timer ──────────────────────────────────────────────────

/// Calibrates TSC Hz using ACPI PM Timer as reference.
///
/// RULE CAL-WINDOW-01: loop condition = PM Timer ticks, never iterations.
/// RULE CAL-CLI-01   : cli/sti per sample of 1ms MAX.
/// RULE TIME-09 (FIX): 24-bit/32-bit rollover handling of PM Timer.
///
/// Accuracy : ±0.05% (PM Timer tick = ±280 ns at 3.58 MHz).
/// Duration : 10ms of real time (10 * 1ms).
///
/// Returns `None` if PM Timer unavailable or measurement out of range.
pub fn calibrate_tsc_via_pm_timer() -> Option<u64> {
    if !pm_src::available() {
        return None;
    }

    let freq_hz = pm_src::freq_hz(); // 3_579_545 Hz (fixed)
    if freq_hz == 0 {
        return None;
    }

    // 1ms of PM Timer ticks.
    let target_ticks: u64 = freq_hz / 1_000; // ≈ 3 579 ticks

    let mut samples = [0u64; N_SAMPLES];

    for sample in &mut samples {
        // ── Synchronization: wait for PM Timer to advance by at least 1 tick ──
        let sync_val = pm_src::read();
        let mut synced = false;
        for _ in 0..MAX_SYNC_ITERS {
            if pm_src::read() != sync_val {
                synced = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !synced {
            return None;
        }

        // ── 1ms measurement — CLI around rdtsc/rdtscp ONLY (CAL-CLI-01) ───────
        // PM Timer = port I/O 0xPMTMR (natively serialized on x86).
        // Same reason as HPET: CLI during wait loop blocks QEMU TCG.
        let pm_start = pm_src::read();

        let flags = flags_save_cli();
        let tsc_start = rdtsc_begin();
        unsafe {
            flags_restore(flags);
        }

        // ✅ FIX TIME-02: exit condition = PM Timer delta ticks.
        // DOUBLE GUARD: N_MAX_POLL_MMIO and TSC timeout.
        let tsc_deadline = tsc_start.wrapping_add(sample_tsc_timeout_cycles());
        let mut poll_count: u64 = 0;
        loop {
            let pm_now = pm_src::read();
            poll_count = poll_count.wrapping_add(1);
            // RULE TIME-09: pm_src::delta handles 24/32-bit rollover.
            if pm_src::delta(pm_start, pm_now) >= target_ticks {
                break;
            }
            // MMIO GUARD: max N_MAX_POLL_MMIO I/O reads.
            if poll_count >= N_MAX_POLL_MMIO {
                return None;
            }
            // TSC GUARD: timeout if PM Timer frozen
            let elapsed = rdtsc_begin().wrapping_sub(tsc_start);
            if elapsed >= tsc_deadline.wrapping_sub(tsc_start) {
                return None;
            }
            core::hint::spin_loop();
        }

        let flags2 = flags_save_cli();
        let (tsc_end, _coreid) = rdtscp_end();
        unsafe {
            flags_restore(flags2);
        }

        let tsc_delta = tsc_end.wrapping_sub(tsc_start);
        *sample = tsc_delta.saturating_mul(1_000);
    }

    let tsc_hz = iqr_mean(&samples);
    super::validation::cross_check(tsc_hz)
}

// ── Statistical utilities ─────────────────────────────────────────────────────

/// Computes mean of samples excluding outliers using IQR.
/// Algorithm: sort -> Q1/Q3 -> reject outside [Q1 - 1.5*IQR, Q3 + 1.5*IQR].
fn iqr_mean(sorted_samples: &[u64]) -> u64 {
    let n = sorted_samples.len();
    if n == 0 {
        return 0;
    }
    if n == 1 {
        return sorted_samples[0];
    }

    let q1 = sorted_samples[n / 4];
    let q3 = sorted_samples[3 * n / 4];
    let iqr = q3.saturating_sub(q1);
    let margin = iqr.saturating_add(iqr / 2); // 1.5 * IQR

    let lo = q1.saturating_sub(margin);
    let hi = q3.saturating_add(margin);

    let mut sum: u128 = 0;
    let mut count: u64 = 0;

    for &s in sorted_samples.iter() {
        if s >= lo && s <= hi {
            sum = sum.saturating_add(s as u128);
            count += 1;
        }
    }

    if count == 0 {
        // All outliers rejected (highly dispersed data) -> median.
        sorted_samples[n / 2]
    } else {
        (sum / count as u128) as u64
    }
}

// ── Calibration via PIT (legacy fallback) ─────────────────────────────────────
//
// RULE PIT-QEMU-01: PIT does NOT work on QEMU TCG in busy-wait.
//   -> Use ONLY if HPET and PM Timer are both absent.
//   On QEMU TCG, PIT channel 0 does not decrement in direct read mode.
//   Bare-metal: reliable to ±0.5% in poll mode on channel 2 (speaker gate).
//
// Fixed PIT frequency: 1_193_182 Hz (derived from 14.318 MHz crystal / 12).
// PIT tick resolution: ~838 ns -> 1ms = ~1193 ticks.

const PIT_FREQ_HZ: u64 = 1_193_182;
const PIT_CMD_PORT: u16 = 0x43;
const PIT_CH2_PORT: u16 = 0x42;
const PIT_GATE_PORT: u16 = 0x61;

/// Calibrates TSC Hz via PIT channel 2 (speaker gate) in one-shot mode.
///
/// RULE PIT-QEMU-01  : called ONLY if HPET and PM Timer are absent.
/// RULE CAL-CLI-01   : cli/sti per sample of 1ms MAX.
/// RULE CAL-WINDOW-01: exit condition = PIT OUT2 bit, never iterations.
/// RULE CAL-RDTSCP-01: RDTSCP for end measurement.
///
/// Accuracy: ±0.5% (PIT tick = ~838 ns, port I/O jitter).
/// Limit   : unreliable on QEMU TCG -> returns `None` if loop expires.
pub fn calibrate_tsc_via_pit() -> Option<u64> {
    let target_count: u16 = (PIT_FREQ_HZ / 1_000) as u16; // ≈ 1193 ticks = 1ms

    let mut samples = [0u64; N_SAMPLES];

    for sample in &mut samples {
        let flags = flags_save_cli();

        // ── Configure PIT CH2 mode 0 (one-shot) for target_count ticks ──────
        // 1. Disable CH2 gate (bit 0 of port 0x61).
        let gate = inb(PIT_GATE_PORT);
        outb(PIT_GATE_PORT, gate & !0x01);

        // 2. Command: channel 2, lobyte/hibyte, mode 0 (terminal count), bcd=0.
        outb(PIT_CMD_PORT, 0b1011_0000);
        outb(PIT_CH2_PORT, (target_count & 0xFF) as u8); // LSB
        outb(PIT_CH2_PORT, ((target_count >> 8) & 0xFF) as u8); // MSB

        // Barrier: the port I/O writes above must complete before
        // the TSC read and gate activation.
        core::sync::atomic::fence(Ordering::SeqCst);

        // 3. Enable gate -> starts countdown.
        outb(PIT_GATE_PORT, (gate & !0x01) | 0x01);
        let tsc_start = rdtsc_begin();

        // ✅ FIX TIME-02: exit condition = bit 5 of port 0x61 (OUT2 = 1
        // when CH2 counter reaches 0).
        // Timeout: 100 iterations max (ultra-short diagnostic for QEMU TCG).
        let mut poll_ok = false;
        for _ in 0..100u32 {
            if inb(PIT_GATE_PORT) & 0x20 != 0 {
                poll_ok = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !poll_ok {
            outb(PIT_GATE_PORT, gate & !0x01);
            unsafe {
                flags_restore(flags);
            }
            return None;
        }

        let (tsc_end, _coreid) = rdtscp_end();
        // Restore gate to initial state.
        outb(PIT_GATE_PORT, gate & !0x01);
        unsafe {
            flags_restore(flags);
        }

        let tsc_delta = tsc_end.wrapping_sub(tsc_start);
        // Extrapolation: delta over 1ms -> Hz for 1 second.
        *sample = tsc_delta.saturating_mul(1_000);
    }

    samples.sort_unstable();
    let tsc_hz = iqr_mean(&samples);
    super::validation::cross_check(tsc_hz)
}

// ── Enriched result structures ────────────────────────────────────────────────

/// Metadata of an individual calibration sample.
///
/// Allows post-calibration diagnostics: identifying outliers, measuring
/// jitter, detecting anomalies (Turbo Boost spike, spurious IRQ).
#[derive(Debug, Clone, Copy)]
pub struct CalibrationSample {
    /// TSC frequency extrapolated from this sample in Hz (tsc_delta * 1000).
    pub tsc_hz_estimate: u64,
    /// TSC ticks measured during the 1ms window.
    pub tsc_ticks: u64,
    /// Reference ticks (HPET/PM/PIT) measured during the same window.
    pub ref_ticks: u64,
    /// Sample rejected as outlier by IQR (outside [Q1-1.5*IQR, Q3+1.5*IQR]).
    pub is_outlier: bool,
}

impl CalibrationSample {
    /// Creates an empty sample (before measurement).
    pub const fn zero() -> Self {
        Self {
            tsc_hz_estimate: 0,
            tsc_ticks: 0,
            ref_ticks: 0,
            is_outlier: false,
        }
    }
}

/// Reference source used during calibration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationSource {
    Hpet,
    PmTimer,
    Pit,
    None,
}

/// Complete result of a multi-sample windowed calibration.
///
/// Aggregates N_SAMPLES samples with final frequency, quality
/// statistics (variance) and diagnostic metadata.
#[derive(Debug, Clone, Copy)]
pub struct CalibrationResult {
    /// Final TSC frequency in Hz (IQR mean of valid samples).
    pub tsc_hz: u64,
    /// Number of retained samples after outlier rejection (max = N_SAMPLES).
    pub valid_count: u8,
    /// Empirical variance of valid samples in Hz² — elevated (> 1% of hz²)
    /// indicates unstable measurement (CPU load, Turbo Boost, disruptive IRQ).
    pub variance_hz2: u64,
    /// Reference source used for measurement.
    pub source: CalibrationSource,
}

impl CalibrationResult {
    /// Returns `true` if variance is acceptable (< 1% of tsc_hz).
    ///
    /// Threshold: variance < (tsc_hz / 100)², i.e. < 0.01% relative dispersion.
    pub fn is_stable(&self) -> bool {
        if self.tsc_hz == 0 || self.valid_count == 0 {
            return false;
        }
        // (1% of tsc_hz)² = tsc_hz² / 10_000
        let threshold = (self.tsc_hz as u128).saturating_mul(self.tsc_hz as u128) / 10_000;
        (self.variance_hz2 as u128) < threshold
    }

    /// Returns estimated standard deviation in Hz (integer square root of variance).
    pub fn stddev_hz(&self) -> u64 {
        isqrt64(self.variance_hz2)
    }
}

// ── Statistics ────────────────────────────────────────────────────────────────

/// Computes mean and empirical variance of samples after IQR filtering.
///
/// Returns `(mean_hz, variance_hz2)`.
/// Non-allocating — operates on the provided sorted slice.
pub fn mean_and_variance(sorted_samples: &[u64]) -> (u64, u64) {
    let n = sorted_samples.len();
    if n == 0 {
        return (0, 0);
    }

    let mean = iqr_mean(sorted_samples);
    if mean == 0 || n < 2 {
        return (mean, 0);
    }

    // IQR bounds (same as in iqr_mean).
    let q1 = sorted_samples[n / 4];
    let q3 = sorted_samples[3 * n / 4];
    let iqr = q3.saturating_sub(q1);
    let margin = iqr.saturating_add(iqr / 2); // 1.5 * IQR
    let lo = q1.saturating_sub(margin);
    let hi = q3.saturating_add(margin);

    let mut sum_sq: u128 = 0;
    let mut count: u64 = 0;

    for &s in sorted_samples.iter() {
        if (lo..=hi).contains(&s) {
            let diff = s.abs_diff(mean);
            sum_sq = sum_sq.saturating_add((diff as u128).saturating_mul(diff as u128));
            count += 1;
        }
    }

    let variance = if count > 1 {
        (sum_sq / count as u128) as u64
    } else {
        0
    };
    (mean, variance)
}

/// Integer square root (Newton-Raphson) — no libm, no alloc.
fn isqrt64(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

#[inline]
fn hpet_fail_tag(reason: &[u8]) {
    #[inline(always)]
    unsafe fn out(b: u8) {
        core::arch::asm!("out 0xe9, al", in("al") b, options(nomem, nostack, preserves_flags));
    }
    unsafe {
        for &b in b"[HPET:" {
            out(b);
        }
        for &b in reason {
            out(b);
        }
        out(b']');
    }
}

// ── Port I/O helpers (PIT) ────────────────────────────────────────────────────

/// Reads a byte from the given I/O port.
#[inline(always)]
fn inb(port: u16) -> u8 {
    let val: u8;
    // SAFETY: PIT ports (0x42-0x43, 0x61) are non-destructive I/O ports.
    //         Called only from calibrate_tsc_via_pit() with CLI active.
    unsafe {
        core::arch::asm!(
            "in al, dx",
            out("al") val,
            in("dx") port,
            options(nostack, nomem)
        );
    }
    val
}

/// Writes a byte to the given I/O port.
#[inline(always)]
fn outb(port: u16, val: u8) {
    // SAFETY: write to standard PIT ports (0x42-0x43-0x61).
    //         Called only from calibrate_tsc_via_pit() with CLI active.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
            options(nostack, nomem)
        );
    }
}

// ── Assembly primitives ───────────────────────────────────────────────────────

/// Saves RFLAGS and disables IRQs (CLI).
/// RULE CAL-CLI-01: called at the start of each 1ms sample.
#[inline(always)]
fn flags_save_cli() -> u64 {
    let flags: u64;
    // SAFETY: PUSHFQ/POP + CLI — standard sequence for critical section.
    //         Implicit barrier of pushfq guarantees memory ordering.
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {flags}",
            "cli",
            flags = out(reg) flags,
            options(nomem)
        );
    }
    flags
}

/// Restores RFLAGS (STI if IF was active before `flags_save_cli()`).
#[inline(always)]
unsafe fn flags_restore(flags: u64) {
    // SAFETY: RFLAGS restoration — safe if flags came from flags_save_cli().
    core::arch::asm!(
        "push {flags}",
        "popfq",
        flags = in(reg) flags,
        options(nomem)
    );
}

/// Reads RDTSC with LFENCE barrier (serialized on load side).
#[inline(always)]
fn rdtsc_begin() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: LFENCE + RDTSC — standard unprivileged barrier.
    unsafe {
        core::arch::asm!(
            "lfence",
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | lo as u64
}

/// Reads TSC with trailing barrier, without assuming RDTSCP is available.
/// RULE CAL-RDTSCP-01: RDTSCP is used if CPUID advertises it, otherwise fallback
/// LFENCE/RDTSC/LFENCE with stable current CPU ID.
#[inline(always)]
fn rdtscp_end() -> (u64, u32) {
    crate::arch::x86_64::time::sources::tsc::read_ordered_with_cpu()
}
