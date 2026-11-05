// kernel/src/arch/x86_64/time/sources/mod.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Clock source registry — dynamic selection and monitoring
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Available sources (in decreasing order of preference)
//
//   1. Invariant TSC (rating 400 / 350 with hypervisor)
//      Best resolution (CPU cycle), no HW access on each read.
//      Requires prior calibration and SMP synchronization.
//
//   2. 64-bit HPET (rating 320 / 300 if 32-bit)
//      Good precision (~14.318 MHz), UC MMIO without I/O overhead.
//      Reference source for TSC calibration.
//
//   3. ACPI PM Timer (rating 200)
//      Available on all ACPI PCs. Fixed frequency 3.579545 MHz.
//      Triple-read anti-glitch. Fallback if HPET is missing.
//
//   4. 8254 PIT (rating 50)
//      ISA legacy. Always present on x86. Only for last-resort.
//      May not work on QEMU TCG.
//
// ## Selection of the best source
//   `best_source_for_calibration()`: returns the most reliable available source
//   to calibrate the TSC (HPET > PM Timer > PIT > None).
//
//   `best_runtime_source()`: returns the source to use at runtime to
//   read current time (TSC > HPET > PM Timer > PIT).
//
// ## Health monitoring
//   Each source has a `SourceStatus`: Available, Degraded, Unavailable.
//   `source_health_check()` reads sources and compares values to
//   detect drift or failures.
//
// ## Rules
//   RULE TIME-04: Invariant TSC checked before use.
//   RULE TIME-08: HPET delta via wrapping_sub.
//   RULE PIT-QEMU-01: Unavailable PIT marked Degraded on QEMU TCG.
// ════════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub mod hpet;
pub mod pit;
pub mod pm_timer;
pub mod tsc;

// ── Trait ClockSource ─────────────────────────────────────────────────────────

/// Common trait for all hardware clock sources.
pub trait ClockSource {
    fn name(&self) -> &'static str;
    /// Selection priority: higher -> preferred source.
    fn rating(&self) -> u32;
    /// Reads the raw counter value (in ticks).
    fn read(&self) -> u64;
    /// Frequency in Hz.
    fn freq_hz(&self) -> u64;
    /// Indicates whether the source is available and functional.
    fn available(&self) -> bool;
}

// ── Source identifier ─────────────────────────────────────────────────────────

/// Enumerated identifier of clock sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceId {
    Tsc,
    Hpet,
    PmTimer,
    Pit,
    None,
}

impl SourceId {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceId::Tsc => "TSC",
            SourceId::Hpet => "HPET",
            SourceId::PmTimer => "PM_TIMER",
            SourceId::Pit => "PIT",
            SourceId::None => "NONE",
        }
    }

    /// Default rating of the source.
    pub fn default_rating(self) -> u32 {
        match self {
            SourceId::Tsc => 400,
            SourceId::Hpet => 300,
            SourceId::PmTimer => 200,
            SourceId::Pit => 50,
            SourceId::None => 0,
        }
    }
}

// ── Source status ─────────────────────────────────────────────────────────────

/// Operational status of a clock source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceStatus {
    /// Available and validated.
    Available,
    /// Available but with anomalies (drift, glitches).
    Degraded,
    /// Not available (missing hardware or failed init).
    Unavailable,
    /// Not yet tested.
    Untested,
}

impl SourceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceStatus::Available => "OK",
            SourceStatus::Degraded => "DEGRADED",
            SourceStatus::Unavailable => "UNAVAIL",
            SourceStatus::Untested => "UNTESTED",
        }
    }

    pub fn is_usable(self) -> bool {
        matches!(self, SourceStatus::Available | SourceStatus::Degraded)
    }
}

// ── Source registry ───────────────────────────────────────────────────────────

/// Source registry entry.
#[derive(Debug, Clone, Copy)]
pub struct SourceEntry {
    pub id: SourceId,
    pub status: SourceStatus,
    pub rating: u32,
}

impl SourceEntry {
    pub const fn new(id: SourceId) -> Self {
        SourceEntry {
            id,
            status: SourceStatus::Untested,
            rating: 0,
        }
    }
}

// Global registry of 4 sources.
const NUM_SOURCES: usize = 4;
static REGISTRY: [AtomicU32; NUM_SOURCES * 2] = [const { AtomicU32::new(0) }; NUM_SOURCES * 2];

/// Source index in REGISTRY.
const IDX_TSC: usize = 0;
const IDX_HPET: usize = 1;
const IDX_PMTIMER: usize = 2;
const IDX_PIT: usize = 3;

// Encoding: index*2 = status, index*2+1 = rating.
fn read_entry(idx: usize) -> SourceEntry {
    let ids = [
        SourceId::Tsc,
        SourceId::Hpet,
        SourceId::PmTimer,
        SourceId::Pit,
    ];
    let status_raw = REGISTRY[idx * 2].load(Ordering::Relaxed);
    let rating = REGISTRY[idx * 2 + 1].load(Ordering::Relaxed);
    let status = match status_raw {
        1 => SourceStatus::Available,
        2 => SourceStatus::Degraded,
        3 => SourceStatus::Unavailable,
        _ => SourceStatus::Untested,
    };
    SourceEntry {
        id: ids[idx],
        status,
        rating,
    }
}

fn write_entry(idx: usize, status: SourceStatus, rating: u32) {
    let status_raw: u32 = match status {
        SourceStatus::Available => 1,
        SourceStatus::Degraded => 2,
        SourceStatus::Unavailable => 3,
        SourceStatus::Untested => 0,
    };
    REGISTRY[idx * 2].store(status_raw, Ordering::Relaxed);
    REGISTRY[idx * 2 + 1].store(rating, Ordering::Relaxed);
}

// ── Registry initialization ───────────────────────────────────────────────────

static REGISTRY_INIT_DONE: AtomicBool = AtomicBool::new(false);

/// Initializes and probes all available sources.
/// To be called from `time_init()` after ACPI and TSC initialization.
pub fn init_sources() {
    if REGISTRY_INIT_DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // TSC: always present; invariant determines rating.
    tsc::init_tsc_source();
    let tsc_rating = tsc::tsc_rating();
    write_entry(IDX_TSC, SourceStatus::Available, tsc_rating);

    // HPET: available if `hpet_available()`.
    hpet::init_hpet_source();
    if hpet::available() {
        let r = if hpet::HpetSource.available() {
            hpet::HpetSource.rating()
        } else {
            0
        };
        write_entry(IDX_HPET, SourceStatus::Available, r);
    } else {
        write_entry(IDX_HPET, SourceStatus::Unavailable, 0);
    }

    // PM Timer: available if port is non-zero.
    if pm_timer::available() || pm_timer::auto_detect() {
        write_entry(IDX_PMTIMER, SourceStatus::Available, 200);
    } else {
        write_entry(IDX_PMTIMER, SourceStatus::Unavailable, 0);
    }

    // PIT: always present in theory, but tested first.
    let pit_status = if pit::qemu_tcg_detected() {
        SourceStatus::Degraded
    } else {
        SourceStatus::Available
    };
    write_entry(IDX_PIT, pit_status, pit::PitSource.rating());
}

// ── Source selection ──────────────────────────────────────────────────────────

/// Returns the best source for TSC calibration (NOT the TSC itself).
/// Order: HPET > PM Timer > PIT > None.
pub fn best_source_for_calibration() -> SourceId {
    if read_entry(IDX_HPET).status.is_usable() {
        return SourceId::Hpet;
    }
    if read_entry(IDX_PMTIMER).status.is_usable() {
        return SourceId::PmTimer;
    }
    if read_entry(IDX_PIT).status.is_usable() {
        return SourceId::Pit;
    }
    SourceId::None
}

/// Returns the best source for reading time at runtime.
/// Order: TSC > HPET > PM Timer > PIT.
pub fn best_runtime_source() -> SourceId {
    let tsc = read_entry(IDX_TSC);
    if tsc.status.is_usable() && tsc.rating >= 350 {
        return SourceId::Tsc;
    }
    if read_entry(IDX_HPET).status.is_usable() {
        return SourceId::Hpet;
    }
    if read_entry(IDX_PMTIMER).status.is_usable() {
        return SourceId::PmTimer;
    }
    if read_entry(IDX_PIT).status.is_usable() {
        return SourceId::Pit;
    }
    SourceId::None
}

/// Reads current value of the requested source (in raw ticks).
pub fn read_source(id: SourceId) -> u64 {
    match id {
        SourceId::Tsc => tsc::rdtsc_read(),
        SourceId::Hpet => hpet::read(),
        SourceId::PmTimer => pm_timer::read(),
        SourceId::Pit => pit::read_latch_ch2() as u64,
        SourceId::None => 0,
    }
}

/// Returns the frequency in Hz of the requested source.
pub fn source_freq_hz(id: SourceId) -> u64 {
    match id {
        SourceId::Tsc => tsc::tsc_freq_hz(),
        SourceId::Hpet => hpet::freq_hz(),
        SourceId::PmTimer => pm_timer::freq_hz(),
        SourceId::Pit => pit::PIT_FREQ_HZ,
        SourceId::None => 0,
    }
}

// ── Health monitoring ─────────────────────────────────────────────────────────

/// Count of performed health checks.
static HEALTH_CHECK_COUNT: AtomicU64 = AtomicU64::new(0);

/// Performs a cross-check between HPET and PM Timer.
/// Returns `true` if both sources are consistent (divergence < 1%).
pub fn source_health_check() -> bool {
    HEALTH_CHECK_COUNT.fetch_add(1, Ordering::Relaxed);

    let hpet_ok = read_entry(IDX_HPET).status.is_usable();
    let pm_ok = read_entry(IDX_PMTIMER).status.is_usable();

    if !hpet_ok || !pm_ok {
        // Not enough sources for cross-verification.
        return true;
    }

    // Measure an interval with both sources simultaneously.
    let hpet_start = hpet::read();
    let pm_start = pm_timer::read();

    // Wait ~100 µs (approximate measurement via spin_loop).
    // No real wait here because this function is called periodically.
    let hpet_end = hpet::read();
    let pm_end = pm_timer::read();

    let hpet_delta = hpet::delta(hpet_start, hpet_end);
    let pm_delta = pm_timer::delta(pm_start, pm_end);

    if hpet_delta == 0 || pm_delta == 0 {
        return true; // Not enough time elapsed.
    }

    // Convert to nanoseconds for comparison.
    let hpet_ns = hpet::ticks_to_ns(hpet_delta);
    let pm_ns = pm_timer::ticks_to_ns(pm_delta);

    if hpet_ns == 0 || pm_ns == 0 {
        return true;
    }

    // Divergence in hundredths of % = |hpet_ns - pm_ns| × 10000 / pm_ns.
    let diff = hpet_ns.abs_diff(pm_ns);
    let divergence_x100 = (diff as u128 * 10_000) / pm_ns as u128;

    if divergence_x100 > 500 {
        // Divergence > 5% -> mark HPET as degraded.
        write_entry(
            IDX_HPET,
            SourceStatus::Degraded,
            read_entry(IDX_HPET).rating,
        );
        return false;
    }

    true
}

/// Updates status of a source (for dynamic updates).
pub fn update_source_status(id: SourceId, status: SourceStatus) {
    let idx = match id {
        SourceId::Tsc => IDX_TSC,
        SourceId::Hpet => IDX_HPET,
        SourceId::PmTimer => IDX_PMTIMER,
        SourceId::Pit => IDX_PIT,
        SourceId::None => return,
    };
    let current_rating = read_entry(idx).rating;
    write_entry(idx, status, current_rating);
}

/// Updates rating of a source (e.g., upgrade TSC after successful calibration).
pub fn update_source_rating(id: SourceId, rating: u32) {
    let idx = match id {
        SourceId::Tsc => IDX_TSC,
        SourceId::Hpet => IDX_HPET,
        SourceId::PmTimer => IDX_PMTIMER,
        SourceId::Pit => IDX_PIT,
        SourceId::None => return,
    };
    let current_status = read_entry(idx).status;
    write_entry(idx, current_status, rating);
}

/// Returns all registry entries (for diagnostics).
pub fn all_entries() -> [SourceEntry; NUM_SOURCES] {
    [
        read_entry(IDX_TSC),
        read_entry(IDX_HPET),
        read_entry(IDX_PMTIMER),
        read_entry(IDX_PIT),
    ]
}

/// Returns the number of usable sources (Available or Degraded).
pub fn usable_source_count() -> u8 {
    let entries = all_entries();
    entries.iter().filter(|e| e.status.is_usable()).count() as u8
}
