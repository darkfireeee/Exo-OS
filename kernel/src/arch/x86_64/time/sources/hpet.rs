// kernel/src/arch/x86_64/time/sources/hpet.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// HPET source — capabilities, counter width, overflow guard, safe delta
// ════════════════════════════════════════════════════════════════════════════════
//
// ## HPET Architecture (High Precision Event Timer)
//   Intel IA-PC HPET Architecture Specification 1.0a
//   MMIO base: read from the ACPI HPET table (signature "HPET"), base_address.
//
//   Registers (offset from Base):
//     0x000 : General Capabilities and ID Register (64-bit, RO)
//       [63:32] clock period in femtoseconds (COUNTER_CLK_PERIOD)
//       [31:16] Vendor ID
//       [12]    3-bit rev (bits 12..8 = REV_ID)
//       [13]    COUNT_SIZE_CAP: 1 = 64-bit counter, 0 = 32-bit counter
//       [7..5]  NUM_TIM_CAP: number of timers - 1
//       [0]     REV_ID (bits 7:0)
//     0x010 : General Configuration Register (64-bit, RW)
//       bit 0  = ENABLE_CNF (1 = counter active)
//       bit 1  = LEG_RT_CNF (Legacy Routing Config)
//     0x020 : General Interrupt Status Register (64-bit, RW)
//     0x0F0 : Main Counter Value Register (64-bit, RO — read twice if 32-bit)
//     0x100 : Timer N Configuration Register (timers 0..31)
//
// ## Counter width (RULE TIME-08)
//   COUNT_SIZE_CAP=1 -> 64-bit: rollover every ~42 billion years.
//   COUNT_SIZE_CAP=0 -> 32-bit: rollover every 2^32 / freq ≈ 300 seconds
//     at 14.318 MHz (PIT-compatible HPET frequency).
//   -> Use wrapping_sub() systematically for both cases.
//   -> For 32-bit, detect an overflow (bit_64 XOR bit_32 read) and increment
//     a wrap counter.
//
// ## Minimum Delta (Intel HPET spec §2.2)
//   Comparison value for one-shot timers must be at least
//   MIN_DELTA_TICKS > current counter + minimum_delta.
//   Typically >= 100 HPET ticks (≈ 7 µs at 14.318 MHz).
// ════════════════════════════════════════════════════════════════════════════════

use super::ClockSource;
use crate::arch::x86_64::acpi::hpet as hpet_acpi;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ── HPET register offsets ─────────────────────────────────────────────────────

const HPET_REG_CAP_ID: u64 = 0x000; // Capabilities and ID (RO)
#[allow(dead_code)]
const HPET_REG_CONFIG: u64 = 0x010; // General Configuration (RW)
#[allow(dead_code)]
const HPET_REG_INT_STATUS: u64 = 0x020; // General Interrupt Status (RW)
const HPET_REG_COUNTER: u64 = 0x0F0; // Main Counter Value (RO)

// ── CAP_ID register bits ──────────────────────────────────────────────────────

const CAP_COUNT_SIZE_CAP: u64 = 1 << 13; // 1 = 64-bit counter
const CAP_CLK_PERIOD_SHIFT: u32 = 32; // bits 63:32 = clock period femtos
const CAP_NUM_TIMERS_MASK: u64 = 0x1F00; // bits 12:8 = NUM_TIM_CAP
const CAP_NUM_TIMERS_SHIFT: u32 = 8;
const CAP_VENDOR_ID_MASK: u64 = 0xFFFF_0000; // bits 31:16
const CAP_VENDOR_ID_SHIFT: u32 = 16;
const CAP_REV_ID_MASK: u64 = 0x00FF; // bits 7:0

// ── Constants ─────────────────────────────────────────────────────────────────

/// Legal minimum HPET period according to spec: 100 femtoseconds (10 GHz max).
const HPET_MIN_PERIOD_FEMTOS: u64 = 100;
/// Reasonable maximum HPET period: 100 ns = 100_000 femtos (10 MHz min).
const HPET_MAX_PERIOD_FEMTOS: u64 = 100_000_000;
/// Minimum delta for HPET comparators (ticks).
const HPET_MIN_DELTA_TICKS: u32 = 100;
/// Femtoseconds per second.
const FEMTOS_PER_SEC: u64 = 1_000_000_000_000_000;

// ── HPET capabilities ─────────────────────────────────────────────────────────

/// Full HPET capabilities detected at boot.
#[derive(Debug, Clone, Copy)]
pub struct HpetCapabilities {
    /// `true` if counter is 64-bit, `false` if 32-bit.
    pub counter_64bit: bool,
    /// Frequency in Hz (= 10^15 / period_femtos).
    pub freq_hz: u64,
    /// Counter period in femtoseconds (from CAP_ID register bits 63:32).
    pub period_femtos: u64,
    /// Number of available timers (0-based in CAP, displayed +1).
    pub num_timers: u8,
    /// Vendor ID from CAP register.
    pub vendor_id: u16,
    /// Revision ID.
    pub rev_id: u8,
    /// MMIO base physical address.
    pub base_addr: u64,
}

impl HpetCapabilities {
    pub const fn unknown() -> Self {
        HpetCapabilities {
            counter_64bit: false,
            freq_hz: 0,
            period_femtos: 0,
            num_timers: 0,
            vendor_id: 0,
            rev_id: 0,
            base_addr: 0,
        }
    }

    /// Verifies whether capabilities are valid.
    pub fn is_valid(&self) -> bool {
        self.period_femtos >= HPET_MIN_PERIOD_FEMTOS
            && self.period_femtos <= HPET_MAX_PERIOD_FEMTOS
            && self.freq_hz > 0
    }
}

// ── HPET globals ──────────────────────────────────────────────────────────────

static HPET_IS_64BIT: AtomicBool = AtomicBool::new(false);
static HPET_FREQ_HZ: AtomicU64 = AtomicU64::new(0);
static HPET_BASE_ADDR: AtomicU64 = AtomicU64::new(0);
/// 32-bit overflow counter (incremented on each detected rollover).
static HPET_OVF_COUNT: AtomicU64 = AtomicU64::new(0);
/// Last observed 32-bit value (for rollover detection).
static HPET_LAST_32: AtomicU32 = AtomicU32::new(0);
static HPET_INIT_DONE: AtomicBool = AtomicBool::new(false);

// ── Initialization ────────────────────────────────────────────────────────────

/// Initializes HPET capabilities from MMIO registers.
/// To be called once from `time_init()`, after `acpi::hpet::init()`.
pub fn init_hpet_source() {
    if HPET_INIT_DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    let base = hpet_acpi::hpet_virt_base();
    if base == 0 {
        return;
    } // HPET not available.

    HPET_BASE_ADDR.store(base, Ordering::Relaxed);

    // Read CAP_ID to extract period and counter width.
    let cap = mmio_read64(base + HPET_REG_CAP_ID);
    let period_femtos = cap >> CAP_CLK_PERIOD_SHIFT;
    let is_64bit = (cap & CAP_COUNT_SIZE_CAP) != 0;

    let freq_hz = FEMTOS_PER_SEC.checked_div(period_femtos).unwrap_or(0);

    HPET_IS_64BIT.store(is_64bit, Ordering::Relaxed);
    HPET_FREQ_HZ.store(freq_hz, Ordering::Relaxed);

    // Initialize LAST_32 with the current value.
    if !is_64bit {
        let initial = mmio_read64(base + HPET_REG_COUNTER) as u32;
        HPET_LAST_32.store(initial, Ordering::Relaxed);
    }
}

/// Returns HPET capabilities by reading globals.
pub fn hpet_capabilities() -> HpetCapabilities {
    let base = HPET_BASE_ADDR.load(Ordering::Relaxed);
    if base == 0 {
        return HpetCapabilities::unknown();
    }
    let cap = mmio_read64(base + HPET_REG_CAP_ID);
    let period_femtos = cap >> CAP_CLK_PERIOD_SHIFT;
    let num_timers = (((cap & CAP_NUM_TIMERS_MASK) >> CAP_NUM_TIMERS_SHIFT) + 1) as u8;
    let vendor_id = ((cap & CAP_VENDOR_ID_MASK) >> CAP_VENDOR_ID_SHIFT) as u16;
    let rev_id = (cap & CAP_REV_ID_MASK) as u8;
    let is_64bit = (cap & CAP_COUNT_SIZE_CAP) != 0;
    let freq_hz = HPET_FREQ_HZ.load(Ordering::Relaxed);

    HpetCapabilities {
        counter_64bit: is_64bit,
        freq_hz,
        period_femtos,
        num_timers,
        vendor_id,
        rev_id,
        base_addr: base,
    }
}

// ── HPET ClockSource source ───────────────────────────────────────────────────

pub struct HpetSource;

impl ClockSource for HpetSource {
    fn name(&self) -> &'static str {
        "HPET"
    }
    fn rating(&self) -> u32 {
        if HPET_IS_64BIT.load(Ordering::Relaxed) {
            320
        } else {
            300
        }
    }

    fn read(&self) -> u64 {
        read_counter_extended()
    }

    fn freq_hz(&self) -> u64 {
        let hz = HPET_FREQ_HZ.load(Ordering::Relaxed);
        if hz > 0 {
            hz
        } else {
            hpet_acpi::hpet_freq_hz()
        }
    }

    fn available(&self) -> bool {
        hpet_acpi::hpet_available()
    }
}

// ── Counter reading ───────────────────────────────────────────────────────────

/// Reads current HPET counter.
///
/// For 32-bit HPET: detects overflows and returns an extended 64-bit counter.
/// For 64-bit HPET: direct read.
/// RULE TIME-08: always wrapping_sub for deltas.
#[inline]
pub fn read_counter_extended() -> u64 {
    let base = HPET_BASE_ADDR.load(Ordering::Relaxed);
    if base == 0 {
        return hpet_acpi::hpet_read_counter();
    }

    if HPET_IS_64BIT.load(Ordering::Relaxed) {
        // 64-bit: direct read, no overflow possible.
        mmio_read64(base + HPET_REG_COUNTER)
    } else {
        // 32-bit: detect rollover.
        read_32bit_extended(base)
    }
}

/// 32-bit read with overflow handling.
/// Increments HPET_OVF_COUNT on each rollover and returns an extended 64-bit counter.
fn read_32bit_extended(base: u64) -> u64 {
    let raw = mmio_read64(base + HPET_REG_COUNTER) as u32;
    let prev = HPET_LAST_32.load(Ordering::Relaxed);

    // Rollover detected: raw < prev (the counter wrapped).
    if raw < prev {
        HPET_OVF_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    HPET_LAST_32.store(raw, Ordering::Relaxed);

    let ovf = HPET_OVF_COUNT.load(Ordering::Relaxed);
    // Reconstruct 64-bit counter: (overflows << 32) | raw.
    (ovf << 32) | raw as u64
}

// ── Exposed primitives ────────────────────────────────────────────────────────

/// Reads current HPET counter (wrapper for public API).
#[inline(always)]
pub fn read() -> u64 {
    read_counter_extended()
}

/// HPET frequency in Hz.
#[inline(always)]
pub fn freq_hz() -> u64 {
    let hz = HPET_FREQ_HZ.load(Ordering::Relaxed);
    if hz > 0 {
        hz
    } else {
        hpet_acpi::hpet_freq_hz()
    }
}

/// `true` if HPET is active and initialized.
#[inline(always)]
pub fn available() -> bool {
    hpet_acpi::hpet_available()
}

/// HPET tick delta between `start` and `now`, handling rollover.
/// RULE TIME-08: wrapping_sub required.
#[inline(always)]
pub fn delta(start: u64, now: u64) -> u64 {
    now.wrapping_sub(start)
}

/// Converts HPET ticks to nanoseconds.
/// Uses the formula: ns = ticks × 10^6 / period_femtos.
/// (ticks / freq_hz × 10^9 = ticks × period_femtos / 10^6)
pub fn ticks_to_ns(ticks: u64) -> u64 {
    let period = {
        let base = HPET_BASE_ADDR.load(Ordering::Relaxed);
        if base == 0 {
            return 0;
        }
        let cap = mmio_read64(base + HPET_REG_CAP_ID);
        cap >> CAP_CLK_PERIOD_SHIFT
    };
    if period == 0 {
        return 0;
    }
    // ns = ticks × period_femtos / 10^6
    let ns = (ticks as u128).saturating_mul(period as u128) / 1_000_000;
    ns as u64
}

/// Converts nanoseconds to HPET ticks.
pub fn ns_to_ticks(ns: u64) -> u64 {
    let period = {
        let base = HPET_BASE_ADDR.load(Ordering::Relaxed);
        if base == 0 {
            return 0;
        }
        let cap = mmio_read64(base + HPET_REG_CAP_ID);
        cap >> CAP_CLK_PERIOD_SHIFT
    };
    if period == 0 {
        return 0;
    }
    (ns as u128 * 1_000_000 / period as u128) as u64
}

/// Waits `ns` nanoseconds in busy-waiting on the HPET counter.
/// RULE CAL-CLI-01: only used during calibration (CLI active <= 1ms).
pub fn hpet_wait_ns(ns: u64) {
    if !available() {
        return;
    }
    let ticks = ns_to_ticks(ns);
    let start = read();
    while delta(start, read()) < ticks {
        core::hint::spin_loop();
    }
}

/// Returns the number of 32-bit overflows detected since boot.
pub fn overflow_count() -> u64 {
    HPET_OVF_COUNT.load(Ordering::Relaxed)
}

/// Checks that minimum delta is satisfied for an HPET comparator.
pub fn check_minimum_delta(current: u64, target: u64) -> bool {
    target.wrapping_sub(current) >= HPET_MIN_DELTA_TICKS as u64
}

// ── MMIO primitives ───────────────────────────────────────────────────────────

/// 64-bit non-cacheable MMIO read (volatile read).
#[inline(always)]
fn mmio_read64(addr: u64) -> u64 {
    // SAFETY: addr is a valid HPET MMIO address, Uncacheable.
    unsafe { (addr as *const u64).read_volatile() }
}

/// 64-bit non-cacheable MMIO write (volatile write).
#[inline(always)]
#[allow(dead_code)]
fn mmio_write64(addr: u64, val: u64) {
    unsafe { (addr as *mut u64).write_volatile(val) }
}
