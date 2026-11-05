// kernel/src/arch/x86_64/time/sources/pm_timer.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// PM Timer source — ACPI Power Management Timer
// ════════════════════════════════════════════════════════════════════════════════
//
// ## PM Timer Architecture
//   Specified by ACPI § 4.8.2 "Power Management Timer"
//   I/O port: read from the ACPI FADT table (field PM_TMR_BLK or X_PM_TMR_BLK).
//   Fixed frequency: 3.579545 MHz (± 50 ppm depending on chipset manufacturer).
//   Resolution: 24-bit or 32-bit (TMR_VAL_EXT bit in FADT flags).
//
// ## Read modes
//   24-bit: rollover every 2^24 / 3.579545 MHz ≈ 4.69 seconds.
//   32-bit: rollover every 2^32 / 3.579545 MHz ≈ 1200 seconds (20 min).
//   -> Use wrapping_sub() + overflow counter for long-term tracking.
//
// ## Anti-glitch (triple read)
//   Certain chipsets (ICH7, SB700) exhibit glitches on a single read.
//   Read must be performed 3× taking the majority vote (2/3).
//   See Linux kernel: arch/x86/kernel/time.c, read_pmtmr().
//
// ## Exact frequency
//   3_579_545 Hz (not 3_580_000 — accurate to within 1 Hz per ACPI spec B.3).
//
// ## Calibration
//   The PM Timer is the fallback calibration source when HPET is absent.
//   Rating 200 (lower than HPET 300 but well above PIT 100).
//   Slower to read than HPET (I/O port vs MMIO) but always available.
// ════════════════════════════════════════════════════════════════════════════════

use super::ClockSource;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ── Constants ─────────────────────────────────────────────────────────────────

/// PM Timer frequency in Hz (ACPI spec, exact value).
pub const PM_TIMER_FREQ_HZ: u64 = 3_579_545;
/// Frequency in thousands of Hz for intermediate calculations.
#[allow(dead_code)]
const PM_TIMER_FREQ_KHZ: u64 = 3_580; // rounded for guard

/// 24-bit value mask.
const PM_TIMER_MASK_24: u32 = 0x00FF_FFFF;
/// 32-bit value mask.
#[allow(dead_code)]
const PM_TIMER_MASK_32: u32 = 0xFFFF_FFFF;

/// Femtoseconds per PM Timer tick: 10^15 / 3_579_545 ≈ 279_365 fs/tick.
#[allow(dead_code)]
const PM_TIMER_FEMTOS_PER_TICK: u64 = 1_000_000_000_000_000 / PM_TIMER_FREQ_HZ;

// ── PM Timer state ────────────────────────────────────────────────────────────

static PM_TIMER_PORT: AtomicU32 = AtomicU32::new(0);
/// `true` if counter is 32-bit (TMR_VAL_EXT bit in FADT flags).
static PM_TIMER_IS_32BIT: AtomicBool = AtomicBool::new(false);
/// 24-bit overflow counter (incremented on each rollover to x000000).
static PM_TIMER_OVF_COUNT: AtomicU64 = AtomicU64::new(0);
/// Last observed 24-bit value (for rollover detection).
static PM_TIMER_LAST_24: AtomicU32 = AtomicU32::new(0);
/// 32-bit overflow counter.
static PM_TIMER_OVF_32_COUNT: AtomicU64 = AtomicU64::new(0);
/// Last 32-bit value (rollover guard).
static PM_TIMER_LAST_32: AtomicU32 = AtomicU32::new(0);
static PM_TIMER_INIT_DONE: AtomicBool = AtomicBool::new(false);
/// Number of detected glitches (failed majority vote).
static PM_TIMER_GLITCH_COUNT: AtomicU64 = AtomicU64::new(0);

// ── FADT information ──────────────────────────────────────────────────────────

/// Detected state of the PM Timer.
#[derive(Debug, Clone, Copy)]
pub struct PmTimerState {
    /// Read I/O port.
    pub port: u16,
    /// 32-bit mode if `true`, 24-bit if `false`.
    pub is_32bit: bool,
    /// Number of overflows since boot.
    pub ovf_count: u64,
    /// Number of corrected glitches (triple read).
    pub glitch_count: u64,
    /// `true` if port was detected (non-zero).
    pub available: bool,
}

// ── Initialization ────────────────────────────────────────────────────────────

/// Initializes PM Timer from ACPI FADT structure.
/// Port and 24/32-bit mode originate from `acpi::fadt::pm_timer_port()`.
pub fn init_pm_timer(port: u16, is_32bit: bool) {
    if PM_TIMER_INIT_DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if port == 0 {
        return;
    }

    PM_TIMER_PORT.store(port as u32, Ordering::Relaxed);
    PM_TIMER_IS_32BIT.store(is_32bit, Ordering::Relaxed);

    // Initialize "last" values for overflow detection.
    let initial = read_raw_single(port);
    if is_32bit {
        PM_TIMER_LAST_32.store(initial, Ordering::Relaxed);
    } else {
        PM_TIMER_LAST_24.store(initial & PM_TIMER_MASK_24, Ordering::Relaxed);
    }
}

/// Initializes PM Timer by automatically detecting port via ACPI tables.
/// Fallback if `init_pm_timer()` is not called directly.
pub fn auto_detect() -> bool {
    // Attempt to read hardcoded port for common Intel ICH/PCH chipsets.
    // Port 0x408 is standard PM Timer port for Intel PIIX4+.
    const PROBE_PORTS: [u16; 3] = [0x408, 0x808, 0xE308];
    for &port in &PROBE_PORTS {
        if probe_pm_timer(port) {
            init_pm_timer(port, false); // Start in 24-bit for safety.
            return true;
        }
    }
    false
}

/// Tests whether a port appears to be a valid PM Timer.
/// A valid PM Timer: value must not be 0xFFFFFFFF and must change.
fn probe_pm_timer(port: u16) -> bool {
    let v1 = read_raw_single(port);
    if v1 == 0xFFFF_FFFF || v1 == 0 {
        return false;
    }
    // Wait a few ns (PIT busy-wait is not available here).
    for _ in 0..100 {
        core::hint::spin_loop();
    }
    let v2 = read_raw_single(port);
    // Value must have changed (advancing counter).
    v2 != v1 && v2 != 0xFFFF_FFFF
}

// ── PM Timer ClockSource source ───────────────────────────────────────────────

pub struct PmTimerSource;

impl ClockSource for PmTimerSource {
    fn name(&self) -> &'static str {
        "PM_TIMER"
    }
    fn rating(&self) -> u32 {
        200
    }

    fn read(&self) -> u64 {
        read_extended()
    }

    fn freq_hz(&self) -> u64 {
        PM_TIMER_FREQ_HZ
    }

    fn available(&self) -> bool {
        PM_TIMER_PORT.load(Ordering::Relaxed) != 0
    }
}

// ── PM Timer read ─────────────────────────────────────────────────────────────

/// PM Timer read with extended counter (24/32-bit overflow handling).
/// Returns a monotonically increasing 64-bit value.
pub fn read_extended() -> u64 {
    let port = PM_TIMER_PORT.load(Ordering::Relaxed) as u16;
    if port == 0 {
        return 0;
    }

    let is_32bit = PM_TIMER_IS_32BIT.load(Ordering::Relaxed);
    let raw = read_anti_glitch(port);

    if is_32bit {
        read_32bit_extended(raw)
    } else {
        read_24bit_extended(raw)
    }
}

/// Raw read with anti-glitch majority vote (3 reads, take 2/3).
///
/// Certain chipsets produce erroneous values intermittently.
/// See Linux docs: Documentation/timers/hpet_and_acpi_pm_timer.txt
pub fn read_anti_glitch(port: u16) -> u32 {
    let a = read_raw_single(port);
    let b = read_raw_single(port);

    // Fast path: first two are identical — normal case.
    if a == b || a.wrapping_add(1) == b {
        return b;
    }

    // Third read for majority vote.
    let c = read_raw_single(port);
    PM_TIMER_GLITCH_COUNT.fetch_add(1, Ordering::Relaxed);

    // Return value appearing at least twice.
    if a == b {
        b
    } else if b == c {
        c
    } else if a == c {
        a
    } else {
        // No agreement — take median value (compromise).
        let mut vals = [a, b, c];
        // Simple 3-element sort.
        if vals[0] > vals[1] {
            vals.swap(0, 1);
        }
        if vals[1] > vals[2] {
            vals.swap(1, 2);
        }
        if vals[0] > vals[1] {
            vals.swap(0, 1);
        }
        vals[1]
    }
}

/// 24-bit read with overflow tracking.
fn read_24bit_extended(raw: u32) -> u64 {
    let val = raw & PM_TIMER_MASK_24;
    let prev = PM_TIMER_LAST_24.load(Ordering::Relaxed);

    if val < prev {
        // 24-bit overflow detected.
        PM_TIMER_OVF_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    PM_TIMER_LAST_24.store(val, Ordering::Relaxed);

    let ovf = PM_TIMER_OVF_COUNT.load(Ordering::Relaxed);
    (ovf << 24) | val as u64
}

/// 32-bit read with overflow tracking.
fn read_32bit_extended(raw: u32) -> u64 {
    let prev = PM_TIMER_LAST_32.load(Ordering::Relaxed);

    if raw < prev {
        PM_TIMER_OVF_32_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    PM_TIMER_LAST_32.store(raw, Ordering::Relaxed);

    let ovf = PM_TIMER_OVF_32_COUNT.load(Ordering::Relaxed);
    (ovf << 32) | raw as u64
}

/// Direct I/O port read (single read, without anti-glitch).
#[inline(always)]
pub fn read_raw_single(port: u16) -> u32 {
    let val: u32;
    // SAFETY: PM Timer I/O port — 32-bit read, CPL=0 required.
    unsafe {
        core::arch::asm!(
            "in eax, dx",
            in("dx")  port,
            out("eax") val,
            options(nostack, nomem)
        );
    }
    val
}

// ── Public primitives ─────────────────────────────────────────────────────────

/// Reads current PM Timer counter (raw value, 24 or 32-bit).
#[inline(always)]
pub fn read() -> u64 {
    read_extended()
}

/// PM Timer frequency in Hz.
#[inline(always)]
pub fn freq_hz() -> u64 {
    PM_TIMER_FREQ_HZ
}

/// `true` if PM Timer is available and initialized.
#[inline(always)]
pub fn available() -> bool {
    PM_TIMER_PORT.load(Ordering::Relaxed) != 0
}

/// PM Timer tick delta between `start` and `now`.
/// Works even across overflows thanks to wrapping_sub.
#[inline(always)]
pub fn delta(start: u64, now: u64) -> u64 {
    now.wrapping_sub(start)
}

/// Converts PM Timer ticks to nanoseconds.
/// ns = ticks × 10^9 / 3_579_545
pub fn ticks_to_ns(ticks: u64) -> u64 {
    // Avoid overflow: ticks × 10^9 may exceed u64 for large ticks.
    let ns = (ticks as u128).saturating_mul(1_000_000_000) / PM_TIMER_FREQ_HZ as u128;
    ns as u64
}

/// Converts nanoseconds to PM Timer ticks.
pub fn ns_to_ticks(ns: u64) -> u64 {
    let ticks = (ns as u128).saturating_mul(PM_TIMER_FREQ_HZ as u128) / 1_000_000_000;
    ticks as u64
}

/// Waits `ns` nanoseconds in busy-waiting on the PM Timer.
/// RULE CAL-CLI-01: use only during calibration (CLI active <= 1ms).
pub fn pm_timer_wait_ns(ns: u64) {
    if !available() {
        return;
    }
    let ticks = ns_to_ticks(ns);
    let start = read();
    while delta(start, read()) < ticks {
        core::hint::spin_loop();
    }
}

/// Returns full PM Timer state for diagnostics.
pub fn pm_timer_state() -> PmTimerState {
    let port = PM_TIMER_PORT.load(Ordering::Relaxed) as u16;
    let is_32bit = PM_TIMER_IS_32BIT.load(Ordering::Relaxed);
    let ovf = if is_32bit {
        PM_TIMER_OVF_32_COUNT.load(Ordering::Relaxed)
    } else {
        PM_TIMER_OVF_COUNT.load(Ordering::Relaxed)
    };
    PmTimerState {
        port,
        is_32bit,
        ovf_count: ovf,
        glitch_count: PM_TIMER_GLITCH_COUNT.load(Ordering::Relaxed),
        available: port != 0,
    }
}

/// Returns number of corrected glitches (anti-glitch vote).
pub fn glitch_count() -> u64 {
    PM_TIMER_GLITCH_COUNT.load(Ordering::Relaxed)
}

/// Returns number of detected overflows.
pub fn overflow_count() -> u64 {
    if PM_TIMER_IS_32BIT.load(Ordering::Relaxed) {
        PM_TIMER_OVF_32_COUNT.load(Ordering::Relaxed)
    } else {
        PM_TIMER_OVF_COUNT.load(Ordering::Relaxed)
    }
}
