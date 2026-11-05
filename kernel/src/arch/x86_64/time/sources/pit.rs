// kernel/src/arch/x86_64/time/sources/pit.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// PIT source — Programmable Interval Timer 8254 (ultimate fallback)
// ════════════════════════════════════════════════════════════════════════════════
//
// ## PIT Architecture (Intel 8254)
//   Input frequency: 14.31818 MHz / 12 = 1.193182 MHz.
//   3 channels:
//     Channel 0 (0x40): IRQ0 generator (system timer). Mode 3 by default.
//     Channel 1 (0x41): DRAM refresh (obsolete, ignored).
//     Channel 2 (0x42): PC speaker / TSC calibration.
//   Control port: 0x43 (OCW).
//   Gate/status port: 0x61 (bits 0 and 1 control channel 2, bit 5 = OUT2).
//
// ## Operation modes (channel 2 for calibration)
//   Mode 0 (One-Shot): counts down from N to 0, OUTPUT goes HIGH when done.
//     -> Used for TSC calibration: wait for bit 5 of port 0x61.
//   Mode 2 (Rate Generator): automatic reload, periodic.
//   Mode 3 (Square Wave): symmetric square wave (default mode for channel 0).
//
// ## OCW Command (0x43)
//   Bits [7:6]: channel (00=0, 01=1, 10=2, 11=readback)
//   Bits [5:4]: access mode (01=LSB, 10=MSB, 11=LSB then MSB)
//   Bits [3:1]: operating mode (000=0, 001=1, 010=2, 011=3, 100=4, 101=5)
//   Bit  [0]  : format (0=binary, 1=BCD)
//
//   Example: 0xB0 = 1011_0000 = channel 2, LSB+MSB, mode 0, binary
//            0x34 = 0011_0100 = channel 0, LSB+MSB, mode 2, binary
//
// ## QEMU/KVM detection (RULE PIT-QEMU-01)
//   On QEMU TCG (pure emulation), the PIT is emulated but can progress very slowly
//   depending on host. On QEMU KVM, it is emulated by the kernel -> accurate timing.
//   Detection: compare a very short busy-wait (1000 PIT ticks) with the measured
//   TSC duration. If TSC has not advanced after 100 µs equivalent -> QEMU TCG suspected.
//
// ## Calibration quality
//   The PIT has low accuracy (frequency ±50 ppm) and busy-waiting is perturbed by
//   IRQs if interrupts are not masked (CLI).
//   Rating 50 (acceptable minimum for fallback calibration).
//   If the PIT does not respond after 200M iters -> return None (QEMU TCG).
// ════════════════════════════════════════════════════════════════════════════════

use super::ClockSource;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ── PIT ports ─────────────────────────────────────────────────────────────────

const PIT_CHANNEL0: u16 = 0x40; // Channel 0 (system timer, IRQ0)
#[allow(dead_code)]
const PIT_CHANNEL1: u16 = 0x41; // Channel 1 (DRAM refresh, ignored)
const PIT_CHANNEL2: u16 = 0x42; // Channel 2 (speaker / calibration)
const PIT_CONTROL: u16 = 0x43; // Control register (OCW)
const PIT_GATE: u16 = 0x61; // Speaker/gate port for channel 2

// ── Port 0x61 bits ────────────────────────────────────────────────────────────

const PIT_GATE61_GATE2: u8 = 1 << 0; // 1 = Channel 2 gate enabled
const PIT_GATE61_SPKR: u8 = 1 << 1; // 1 = Channel 2 connected to speaker
const PIT_GATE61_OUT2: u8 = 1 << 5; // 1 = Channel 2 output is HIGH (end of one-shot)

// ── OCW commands ──────────────────────────────────────────────────────────────

/// Channel 2, LSB+MSB, Mode 0 (one-shot), Binary.
const PIT_OCW_CH2_MODE0: u8 = 0xB0;
/// Channel 0, LSB+MSB, Mode 2 (rate generator), Binary.
const PIT_OCW_CH0_MODE2: u8 = 0x34;
/// Channel 2, Latch command (to read current value).
const PIT_OCW_CH2_LATCH: u8 = 0x80;

// ── Calibration constants ─────────────────────────────────────────────────────

/// PIT frequency in Hz (fixed per ISA 8254 spec).
pub const PIT_FREQ_HZ: u64 = 1_193_182;
/// PIT ticks for ≈10ms measurement (PIT_FREQ_HZ / 100 = 11931).
const PIT_COUNT_10MS: u16 = 11_931;
/// Fallback TSC bound for awaiting completion of a 10 ms one-shot.
/// 200 M cycles cover at least 20 ms up to 10 GHz even before calibration.
const PIT_WAIT_TIMEOUT_FALLBACK_CYCLES: u64 = 200_000_000;

// ── PIT state ─────────────────────────────────────────────────────────────────

/// State of last PIT calibration.
#[derive(Debug, Clone, Copy)]
pub struct PitCalibrationResult {
    /// Estimated TSC frequency via PIT (Hz).
    pub tsc_hz: u64,
    /// Measured TSC ticks during PIT_COUNT_10MS PIT ticks.
    pub tsc_delta: u64,
    /// `true` if measurement is within confidence bounds.
    pub valid: bool,
    /// Quality indicator: 0 (invalid) to 100 (perfect).
    pub quality: u8,
    /// Suspected QEMU TCG (blocked PIT or abnormal drift).
    pub qemu_tcg_suspect: bool,
}

// ── Globals ───────────────────────────────────────────────────────────────────

static PIT_QEMU_TCG_DETECTED: AtomicBool = AtomicBool::new(false);
static PIT_LAST_QUALITY: AtomicU32 = AtomicU32::new(0);
static PIT_CALIBRATION_COUNT: AtomicU64 = AtomicU64::new(0);

// ── PIT ClockSource source ────────────────────────────────────────────────────

pub struct PitSource;

impl ClockSource for PitSource {
    fn name(&self) -> &'static str {
        "PIT"
    }
    fn rating(&self) -> u32 {
        // Reduced rating to 30 if QEMU TCG detected (unreliable signal).
        if PIT_QEMU_TCG_DETECTED.load(Ordering::Relaxed) {
            30
        } else {
            50
        }
    }

    fn read(&self) -> u64 {
        // Latch channel 2, read LSB+MSB (down-counter mode).
        read_latch_ch2() as u64
    }

    fn freq_hz(&self) -> u64 {
        PIT_FREQ_HZ
    }

    fn available(&self) -> bool {
        !PIT_QEMU_TCG_DETECTED.load(Ordering::Relaxed)
    }
}

// ── PIT read ──────────────────────────────────────────────────────────────────

/// Reads latched value of channel 2 (current count).
/// Sends latch command then reads LSB+MSB.
pub fn read_latch_ch2() -> u16 {
    unsafe {
        outb_raw(PIT_CONTROL, PIT_OCW_CH2_LATCH);
        io_delay();
        let lo = inb_raw(PIT_CHANNEL2);
        io_delay();
        let hi = inb_raw(PIT_CHANNEL2);
        ((hi as u16) << 8) | lo as u16
    }
}

/// Reads channel 2 value without latch (reading during operation).
/// May produce unstable values if counter changes between reads.
pub fn read_ch2_raw() -> u16 {
    unsafe {
        let lo = inb_raw(PIT_CHANNEL2);
        let hi = inb_raw(PIT_CHANNEL2);
        ((hi as u16) << 8) | lo as u16
    }
}

// ── PIT configuration ─────────────────────────────────────────────────────────

/// Configures channel 2 in mode 0 (one-shot) with the given count.
/// Channel 2 is used for TSC calibration (no IRQ — polled reading).
///
/// # Safety
/// This function modifies PIT I/O ports. Do not call from a context
/// where interrupts are active (CLI required before calibration).
pub fn setup_ch2_oneshot(count: u16) {
    unsafe {
        // Disable channel 2 gate during configuration.
        let gate = inb_raw(PIT_GATE);
        outb_raw(PIT_GATE, gate & !(PIT_GATE61_GATE2 | PIT_GATE61_SPKR));
        io_delay();

        // Configure channel 2: mode 0, LSB+MSB, binary.
        outb_raw(PIT_CONTROL, PIT_OCW_CH2_MODE0);
        io_delay();

        // Load counter: LSB then MSB.
        outb_raw(PIT_CHANNEL2, (count & 0xFF) as u8);
        io_delay();
        outb_raw(PIT_CHANNEL2, (count >> 8) as u8);
        io_delay();

        // Enable channel 2 gate (starts counting).
        let gate2 = inb_raw(PIT_GATE);
        outb_raw(PIT_GATE, (gate2 | PIT_GATE61_GATE2) & !PIT_GATE61_SPKR);
        io_delay();
    }
}

/// Waits for channel 2 one-shot completion (port 0x61 bit 5 = OUT2 = HIGH).
/// Returns `true` if completion is detected, `false` on timeout (QEMU TCG).
///
/// Timeout: time window bounded to ~20 ms via TSC.
pub fn wait_ch2_done() -> bool {
    let timeout_cycles = crate::arch::x86_64::cpu::tsc::tsc_us_to_cycles(20_000)
        .max(PIT_WAIT_TIMEOUT_FALLBACK_CYCLES);
    let start_tsc = crate::arch::x86_64::cpu::tsc::read_tsc();

    loop {
        let val = unsafe { inb_raw(PIT_GATE) };
        if val & PIT_GATE61_OUT2 != 0 {
            return true;
        }
        if crate::arch::x86_64::cpu::tsc::read_tsc().wrapping_sub(start_tsc) >= timeout_cycles {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Disables channel 2 (gate low + speaker muted).
pub fn disable_ch2() {
    unsafe {
        let gate = inb_raw(PIT_GATE);
        outb_raw(PIT_GATE, gate & !(PIT_GATE61_GATE2 | PIT_GATE61_SPKR));
    }
}

/// Configures channel 0 in mode 2 (rate generator) with the given divisor.
/// Divisor 0 = 65536 (max, ≈ 18.2 Hz tick rate).
pub fn setup_ch0_rate(divisor: u16) {
    unsafe {
        outb_raw(PIT_CONTROL, PIT_OCW_CH0_MODE2);
        io_delay();
        outb_raw(PIT_CHANNEL0, (divisor & 0xFF) as u8);
        io_delay();
        outb_raw(PIT_CHANNEL0, (divisor >> 8) as u8);
        io_delay();
    }
}

// ── Calibration ───────────────────────────────────────────────────────────────

/// Calibrates TSC frequency using PIT channel 2.
///
/// Procedure:
///   1. CLI (caller must have disabled interrupts)
///   2. Configure channel 2 in one-shot ~10ms (11931 ticks)
///   3. Read starting TSC (LFENCE + RDTSC)
///   4. Await OUT2 = HIGH or timeout 200M iters
///   5. Read ending TSC (RDTSCP + LFENCE)
///   6. Compute TSC frequency = delta_tsc × PIT_FREQ_HZ / 11931
///
/// Returns `None` if PIT does not respond (QEMU TCG) or invalid measurement.
///
/// RULE PIT-QEMU-01: function may block on QEMU TCG — caller must
/// ensure fallback in calibration chain.
pub fn calibrate_tsc_via_pit() -> Option<u64> {
    PIT_CALIBRATION_COUNT.fetch_add(1, Ordering::Relaxed);

    let result = run_pit_calibration(PIT_COUNT_10MS);
    PIT_LAST_QUALITY.store(result.quality as u32, Ordering::Relaxed);

    if result.qemu_tcg_suspect {
        PIT_QEMU_TCG_DETECTED.store(true, Ordering::Relaxed);
    }

    if result.valid {
        Some(result.tsc_hz)
    } else {
        None
    }
}

/// Executes internal PIT calibration and returns detailed result.
pub fn run_pit_calibration(count: u16) -> PitCalibrationResult {
    // Setup channel 2.
    setup_ch2_oneshot(count);

    // Read start TSC (serialized via LFENCE).
    let tsc_start = tsc_serialized_start();

    // Await end of one-shot.
    let done = wait_ch2_done();
    disable_ch2();

    if !done {
        // Suspected QEMU TCG (timeout).
        return PitCalibrationResult {
            tsc_hz: 0,
            tsc_delta: 0,
            valid: false,
            quality: 0,
            qemu_tcg_suspect: true,
        };
    }

    // Read end TSC (RDTSCP + LFENCE).
    let tsc_end = tsc_serialized_end();
    let tsc_delta = tsc_end.wrapping_sub(tsc_start);

    if tsc_delta == 0 {
        return PitCalibrationResult {
            tsc_hz: 0,
            tsc_delta: 0,
            valid: false,
            quality: 0,
            qemu_tcg_suspect: true,
        };
    }

    // Compute frequency: tsc_hz = tsc_delta × PIT_FREQ_HZ / count
    let tsc_hz_raw = (tsc_delta as u128 * PIT_FREQ_HZ as u128 / count as u128) as u64;

    // Validate physical range.
    let valid = (100_000_000..=10_000_000_000).contains(&tsc_hz_raw);

    // Quality indicator: depends on tsc_delta value.
    // Too small delta (< 100_000 cycles) -> imprecise measurement.
    let quality = if !valid {
        0
    } else if tsc_delta < 100_000 {
        30
    } else if tsc_delta < 1_000_000 {
        60
    } else {
        85 // PIT maximum quality (lower than HPET because less accurate)
    };

    // QEMU TCG detection: if tsc_hz < 200 MHz -> very likely TCG.
    let qemu_tcg_suspect = valid && tsc_hz_raw < 200_000_000;

    PitCalibrationResult {
        tsc_hz: tsc_hz_raw,
        tsc_delta,
        valid,
        quality,
        qemu_tcg_suspect,
    }
}

// ── QEMU detection ────────────────────────────────────────────────────────────

/// Returns `true` if QEMU TCG was detected during PIT calibrations.
pub fn qemu_tcg_detected() -> bool {
    PIT_QEMU_TCG_DETECTED.load(Ordering::Relaxed)
}

/// Attempts to detect QEMU TCG via KVM CPUID leaf 0x40000001.
/// Real KVM: bit 0 of EAX = "KVM_FEATURE_CLOCKSOURCE". TCG: CPUID returns 0.
pub fn detect_kvm_vs_tcg() -> bool {
    // Check hypervisor presence.
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") 0x0000_0001u32 => _,
            inout("ecx") 0u32 => ecx,
            out("edx") _,
            tmp = inout(reg) 0u64 => _,
            options(nostack, nomem)
        );
    }
    if (ecx & (1 << 31)) == 0 {
        return false;
    } // No hypervisor.

    // KVM leaf 0x40000001 -> EAX = KVM features.
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") 0x4000_0001u32 => eax,
            inout("ecx") 0u32 => _,
            out("edx") _,
            tmp = inout(reg) 0u64 => _,
            options(nostack, nomem)
        );
    }
    // Bit 0 = KVM_FEATURE_CLOCKSOURCE (real KVM).
    (eax & 1) != 0
}

/// Returns quality indicator of last PIT calibration (0-100).
pub fn last_calibration_quality() -> u8 {
    PIT_LAST_QUALITY.load(Ordering::Relaxed) as u8
}

// ── I/O primitives ────────────────────────────────────────────────────────────

/// Byte I/O write.
#[inline(always)]
unsafe fn outb_raw(port: u16, val: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
            options(nostack, nomem)
        );
    }
}

/// Byte I/O read.
#[inline(always)]
unsafe fn inb_raw(port: u16) -> u8 {
    let val: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") port,
            out("al") val,
            options(nostack, nomem)
        );
    }
    val
}

/// I/O delay (port 0x80 write, ≈1µs on ISA bus).
#[inline(always)]
unsafe fn io_delay() {
    unsafe {
        core::arch::asm!("out 0x80, al", in("al") 0u8, options(nostack, nomem));
    }
}

/// Serialized TSC read (LFENCE before RDTSC).
#[inline(always)]
fn tsc_serialized_start() -> u64 {
    let lo: u32;
    let hi: u32;
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

/// Serialized TSC read at end of measurement.
#[inline(always)]
fn tsc_serialized_end() -> u64 {
    super::tsc::read_ordered_with_cpu().0
}
