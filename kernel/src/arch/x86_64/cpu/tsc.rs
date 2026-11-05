//! # cpu/tsc.rs — TSC (Time Stamp Counter) Calibration and Wrappers
//!
//! Provides reliable TSC access for kernel timekeeping.
//!
//! ## Policy
//! - On modern CPUs: invariant TSC guaranteed → used as monotonic clock
//! - Calibration via HPET or PIT as reference
//! - TSC_AUX configured with logical CPU ID for RDTSCP
//!
//! ## Target Precision
//! Calibration to ±0.01% (< 100 ppm)

use super::features::cpu_features_or_none;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ── TSC globals ───────────────────────────────────────────────────────────────

/// TSC frequency in Hz (determined during calibration)
static TSC_HZ: AtomicU64 = AtomicU64::new(0);

/// TSC frequency in kHz (rounded)
static TSC_KHZ: AtomicU64 = AtomicU64::new(0);

/// Invariant TSC available (invariant_tsc = constant rate across C-states)
static TSC_INVARIANT: AtomicBool = AtomicBool::new(false);

/// TSC value at boot (absolute reference point)
static TSC_BOOT_VALUE: AtomicU64 = AtomicU64::new(0);

/// Calibration completed
static TSC_CALIBRATED: AtomicBool = AtomicBool::new(false);

// ── TSC Reading ───────────────────────────────────────────────────────────────

/// Reads the TSC with RDTSC (non-serializing)
///
/// For accurate measurements with barrier, use `read_tsc_serialized()`.
#[inline(always)]
pub fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: RDTSC is non-privileged on x86_64 — no side effects
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Reads the TSC with LFENCE barrier (serializes loads)
///
/// Guarantees that all preceding instructions have completed before reading.
/// Used to begin a precise measurement.
#[inline(always)]
pub fn read_tsc_begin() -> u64 {
    // SAFETY: LFENCE + RDTSC: standard sequence for precise measurement
    unsafe {
        core::arch::asm!("lfence", options(nostack, nomem, preserves_flags));
    }
    read_tsc()
}

/// Reads the TSC with RDTSCP + LFENCE barrier (serializes on both sides)
///
/// Guarantees that all preceding instructions AND this one have completed.
/// Used to end a precise measurement.
///
/// Returns `(tsc_value, cpu_aux)` — cpu_aux = logical CPU ID if configured.
#[inline(always)]
pub fn read_tsc_end() -> (u64, u32) {
    let lo: u32;
    let hi: u32;
    let aux: u32;

    if cpu_features_or_none().is_some_and(|features| features.has_rdtscp()) {
        // SAFETY: RDTSCP supported in hardware.
        unsafe {
            core::arch::asm!(
                "rdtscp",
                out("eax") lo,
                out("edx") hi,
                out("ecx") aux,
                options(nostack, nomem)
            );
            // Post barrier to prevent subsequent instructions from moving up.
            core::arch::asm!("lfence", options(nostack, nomem, preserves_flags));
        }
        (((hi as u64) << 32) | (lo as u64), aux)
    } else {
        // Safe fallback to RDTSC to avoid #UD on platforms without RDTSCP.
        // SAFETY: LFENCE+RDTSC+LFENCE is valid on x86_64.
        unsafe {
            core::arch::asm!("lfence", options(nostack, nomem, preserves_flags));
            core::arch::asm!(
                "rdtsc",
                out("eax") lo,
                out("edx") hi,
                options(nostack, nomem)
            );
            core::arch::asm!("lfence", options(nostack, nomem, preserves_flags));
        }
        (((hi as u64) << 32) | (lo as u64), 0)
    }
}

/// TSC delay in nanoseconds
///
/// Waits `ns` nanoseconds using TSC as reference.
/// Requires `TSC_HZ` to be calibrated.
pub fn tsc_delay_ns(ns: u64) {
    let hz = TSC_HZ.load(Ordering::Relaxed);
    if hz == 0 {
        // Uncalibrated TSC — simple loop fallback
        for _ in 0..ns * 10 {
            core::hint::spin_loop();
        }
        return;
    }
    // cycles = ns * hz / 1_000_000_000
    // Uses 128-bit multiplication to avoid overflow
    let cycles = (ns as u128 * hz as u128) / 1_000_000_000;
    let start = read_tsc();
    while (read_tsc().wrapping_sub(start)) < cycles as u64 {
        core::hint::spin_loop();
    }
}

/// TSC delay in microseconds
#[inline]
pub fn tsc_delay_us(us: u64) {
    tsc_delay_ns(us * 1_000);
}

/// TSC delay in milliseconds
#[inline]
pub fn tsc_delay_ms(ms: u64) {
    tsc_delay_ns(ms * 1_000_000);
}

// ── TSC → Time Conversion ────────────────────────────────────────────────────

/// Converts TSC cycles to nanoseconds
pub fn tsc_cycles_to_ns(cycles: u64) -> u64 {
    let hz = TSC_HZ.load(Ordering::Relaxed);
    if hz == 0 {
        return cycles;
    }
    // ns = cycles * 1_000_000_000 / hz
    (cycles as u128 * 1_000_000_000 / hz as u128) as u64
}

/// Returns elapsed time since boot in nanoseconds (monotonic)
pub fn tsc_ns_since_boot() -> u64 {
    let boot = TSC_BOOT_VALUE.load(Ordering::Relaxed);
    let now = read_tsc();
    tsc_cycles_to_ns(now.wrapping_sub(boot))
}

// ── Calibration via PIT ───────────────────────────────────────────────────────

/// PIT calibration duration in ticks (PIT tick ≈ 838 ns → 10ms = ~11932 ticks)
const PIT_CALIBRATE_COUNT: u16 = 11931;

/// PIT channel 2 port (speaker — can be used without producing sound)
const PIT_CHANNEL2: u16 = 0x42;
const PIT_CONTROL: u16 = 0x43;
const PIT_GATE: u16 = 0x61;

/// PIT base frequency in Hz
const PIT_BASE_HZ: u64 = 1_193_182;

/// Calibrates the TSC using PIT channel 2 as reference
///
/// Duration: approximately 10ms (blocking)
/// Precision: ±1% (sufficient for init, HPET will refine if available)
pub fn calibrate_tsc_with_pit() -> u64 {
    use super::super::{inb, io_delay, outb};

    // 1. Configure PIT channel 2 in mode 0 (one-shot)
    // SAFETY: valid PIT ports — standard usage
    unsafe {
        outb(PIT_CONTROL, 0xB0); // Channel 2, mode 0, binary, LSB+MSB
        io_delay();
        outb(PIT_CHANNEL2, (PIT_CALIBRATE_COUNT & 0xFF) as u8);
        io_delay();
        outb(PIT_CHANNEL2, (PIT_CALIBRATE_COUNT >> 8) as u8);
        io_delay();

        // 2. Enable PIT channel 2 gate (bit 0 of port 0x61) + disable speaker (bit 1=0)
        let gate = inb(PIT_GATE);
        outb(PIT_GATE, (gate | 0x01) & !0x02);
        io_delay();
    }

    // 3. Read starting TSC (with barrier)
    let tsc_start = read_tsc_begin();

    // 4. Wait for PIT to expire (OUTPUT bit 5 of port 0x61 = 0 -> 1).
    // A limit by number of I/O reads is unstable: 10K inb can
    // span several seconds under TCG and less than the PIT window under KVM
    // or bare-metal. The TSC bound keeps a margin >=20 ms even if the initial
    // TSC frequency still comes from pre-calibration fallback.
    let mut pit_ok = false;
    let timeout_cycles = tsc_ms_to_cycles(20).max(200_000_000);
    loop {
        // SAFETY: reading PIT gate I/O port (0x61) in Ring 0; inb is a
        // hardware operation without memory side effects, fixed documented port (8254).
        let val = unsafe { inb(PIT_GATE) };
        if val & 0x20 != 0 {
            pit_ok = true;
            break;
        }
        if read_tsc().wrapping_sub(tsc_start) >= timeout_cycles {
            break;
        }
        core::hint::spin_loop();
    }
    if !pit_ok {
        // PIT channel 2 non-functional — report failure to caller
        return 0;
    }

    // 5. Read ending TSC
    let (tsc_end, _) = read_tsc_end();

    // 6. Disable PIT channel 2 gate
    // SAFETY: restore state of port 0x61
    unsafe {
        let gate = inb(PIT_GATE);
        outb(PIT_GATE, gate & !0x01);
    }

    // 7. Calculate frequency
    let tsc_delta = tsc_end.wrapping_sub(tsc_start);
    // Measurement duration is already encoded by PIT divisor:
    // seconds = PIT_CALIBRATE_COUNT / PIT_BASE_HZ.
    // So Hz = delta_tsc / seconds = delta_tsc * PIT_BASE_HZ / PIT_CALIBRATE_COUNT.
    (tsc_delta as u128 * PIT_BASE_HZ as u128 / PIT_CALIBRATE_COUNT as u128) as u64
}

/// TSC calibration via CPUID (Intel only — leaf 0x15)
///
/// On modern Intel CPUs, CPUID 0x15 gives the exact TSC/bus clock ratio.
/// Returns `Some(hz)` if supported, `None` otherwise.
pub fn calibrate_tsc_cpuid() -> Option<u64> {
    let (eax, ebx, ecx, _) = {
        let (eax, ecx, edx): (u32, u32, u32);
        let ebx_r: u64;
        // SAFETY: CPUID unprivileged; xchg to preserve rbx
        unsafe {
            core::arch::asm!(
                "xchg {tmp:r}, rbx",
                "cpuid",
                "xchg {tmp:r}, rbx",
                inout("eax") 0x15u32 => eax,
                inout("ecx") 0u32    => ecx,
                out("edx") edx,
                tmp = inout(reg) 0u64 => ebx_r,
                options(nostack, nomem)
            );
        }
        (eax, ebx_r as u32, ecx, edx)
    };

    if eax == 0 || ebx == 0 {
        return None;
    }

    // TSC frequency = ecx * ebx / eax (Hz)
    // ecx = crystal clock Hz (or 0 → use standard value)
    let crystal_hz: u64 = if ecx != 0 {
        ecx as u64
    } else {
        // Intel recommends 24 MHz for recent CPUs without crystal Hz in CPUID
        24_000_000
    };

    Some(crystal_hz * ebx as u64 / eax as u64)
}

/// Initializes the TSC: calibration, verification, TSC_AUX configuration
///
/// Called from `early_init.rs` after CPU features and PIC/APIC init.
pub fn init_tsc(cpu_logical_id: u32) {
    use super::msr;

    // Record boot TSC value
    let boot_tsc = read_tsc();
    TSC_BOOT_VALUE.store(boot_tsc, Ordering::Release);

    // Verify invariant TSC (CPUID 0x80000007 EDX bit 8)
    let (_, _, _, edx_ext7) = {
        let (ecx, edx): (u32, u32);
        // SAFETY: CPUID unprivileged; xchg to preserve rbx
        unsafe {
            core::arch::asm!(
                "xchg {tmp:r}, rbx",
                "cpuid",
                "xchg {tmp:r}, rbx",
                inout("eax") 0x8000_0007u32 => _,
                inout("ecx") 0u32           => ecx,
                out("edx") edx,
                tmp = inout(reg) 0u64 => _,
                options(nostack, nomem)
            );
        }
        (0u32, 0u32, ecx, edx)
    };
    let invariant = (edx_ext7 & (1 << 8)) != 0;
    TSC_INVARIANT.store(invariant, Ordering::Release);

    // Calibrate TSC frequency: CPUID 0x15 first, then 1GHz fallback.
    // NOTE: calibration via PIT channel 2 (calibrate_tsc_with_pit) is disabled
    // at boot because it blocks if QEMU/VM timer callbacks do not execute
    // during the busy-poll wait loop (common behavior in TCG/KVM
    // prior to APIC init). It can be re-enabled post-boot once APIC timer
    // or HPET is initialized, via recalibrate_tsc_with_hpet().
    let hz = calibrate_tsc_cpuid().unwrap_or(1_000_000_000); // 1 GHz: safe fallback; will be recalibrated post-APIC

    // Round to multiple of 100 kHz for stability
    let hz_rounded = (hz + 50_000) / 100_000 * 100_000;

    TSC_HZ.store(hz_rounded, Ordering::Release);
    TSC_KHZ.store(hz_rounded / 1000, Ordering::Release);
    TSC_CALIBRATED.store(true, Ordering::Release);

    // Configure TSC_AUX with logical CPU ID (used by RDTSCP)
    if cpu_features_or_none().is_some_and(|features| features.has_rdtscp()) {
        // SAFETY: MSR_TSC_AUX always available if RDTSCP supported
        unsafe {
            msr::write_msr(msr::MSR_TSC_AUX, cpu_logical_id as u64);
        }
    }

    // Enable NXE in EFER if NX available
    if cpu_features_or_none().is_some_and(|features| features.has_nx()) {
        // SAFETY: enable NXE in EFER — required for memory protections
        unsafe {
            msr::set_msr_bits(msr::MSR_IA32_EFER, msr::EFER_NXE);
        }
    }
}

// ── Accessors ─────────────────────────────────────────────────────────────────

/// Returns TSC frequency in Hz
#[inline(always)]
pub fn tsc_hz() -> u64 {
    TSC_HZ.load(Ordering::Relaxed)
}

/// Returns TSC frequency in kHz
#[inline(always)]
pub fn tsc_khz() -> u64 {
    TSC_KHZ.load(Ordering::Relaxed)
}

/// Converts milliseconds to TSC cycles
///
/// Returns 0 if TSC is not yet calibrated.
#[inline(always)]
pub fn tsc_ms_to_cycles(ms: u64) -> u64 {
    let hz = TSC_HZ.load(Ordering::Relaxed);
    if hz == 0 {
        return 0;
    }
    // ms * hz / 1_000 — avoids u64 overflow for ms < 4_000_000 at 4 GHz
    ms.saturating_mul(hz) / 1_000
}

/// Converts microseconds to TSC cycles
///
/// Returns 0 if TSC is not yet calibrated.
#[inline(always)]
pub fn tsc_us_to_cycles(us: u64) -> u64 {
    let hz = TSC_HZ.load(Ordering::Relaxed);
    if hz == 0 {
        return 0;
    }
    us.saturating_mul(hz) / 1_000_000
}

/// Returns `true` if TSC is invariant (stable across C-states)
#[inline(always)]
pub fn tsc_invariant() -> bool {
    TSC_INVARIANT.load(Ordering::Relaxed)
}

/// Returns `true` if TSC calibration is completed
#[inline(always)]
pub fn tsc_calibrated() -> bool {
    TSC_CALIBRATED.load(Ordering::Relaxed)
}

/// Writes the calibrated TSC frequency (called by calibration/ module).
/// Updates TSC_HZ, TSC_KHZ and TSC_CALIBRATED with Release ordering.
/// `hz` MUST be in [100 MHz, 10 GHz] — no validation performed here.
#[inline]
pub fn set_tsc_hz(hz: u64) {
    TSC_HZ.store(hz, Ordering::Release);
    TSC_KHZ.store(hz / 1000, Ordering::Release);
    TSC_CALIBRATED.store(true, Ordering::Release);
}

// ── Instrumentation ───────────────────────────────────────────────────────────

static TSC_OVERFLOW_COUNT: AtomicU64 = AtomicU64::new(0);

/// Counter of detected TSC overflows (monitoring)
pub fn tsc_overflow_count() -> u64 {
    TSC_OVERFLOW_COUNT.load(Ordering::Relaxed)
}

// ── Post-boot Recalibration ───────────────────────────────────────────────────

/// Recalibrates TSC using ACPI PM Timer (3.579545 MHz) as reference.
///
/// Method: measures TSC delta during exactly 10ms via PM Timer.
/// Precision: ±0.05% (PM Timer accurate to ±1 tick = ±280 ns at 3.58 MHz).
///
/// Called from `kernel_init()` after `hybrid::init()` and after HPET init.
/// Does nothing if PM Timer is not available.
///
/// ## Anti-hang timeout
/// Uses TSC cycles (not iterations) as safeguard:
/// in worst case (1 GHz TSC fallback) = 500ms spin before giving up.
/// This prevents infinite hang under QEMU/TCG where each `inl`
/// can take ~500µs (slow virtual clock without KVM).
///
/// Returns `true` and updates `TSC_HZ` / `TSC_KHZ` on success.
pub fn recalibrate_tsc_with_pm_timer() -> bool {
    use super::super::acpi::pm_timer::{
        pm_timer_available, pm_timer_is_32bit, pm_timer_read, PM_TIMER_FREQ_HZ,
    };

    if !pm_timer_available() {
        return false;
    }

    const MEASURE_MS: u64 = 10;
    // Iteration limits (not TSC): guest RDTSC in QEMU/TCG may advance
    // 50-100× slower than real time → TSC timeouts take seconds.
    // With iteration limits:
    //   sync   : 200 inl max → bare-metal ~200ns, QEMU ~100ms
    //   measure: 600 inl max → bare-metal ~6µs (too short!), but PM Timer
    //            advances 3.58MHz×6µs=21 ticks per 6µs — not enough for 35795.
    //   → On bare-metal the iteration count is sufficient because each inl=10ns
    //     and timer advances ~0.036 tick/ns; in 35795/0.036=994K iterations.
    // Solution: use 2 levels: fast for sync, for measurement allow
    // enough iterations (10K) to cover bare-metal AND QEMU:
    //   QEMU  : 10K × 500µs = 5s max — still too slow!
    // Real solution: if PM Timer advances properly (delta > 0 on each read)
    // → immediate exit; otherwise iteration counter.
    // Keep MAX_ITERS small (500) and return false if not enough accumulated.
    // On QEMU: 500 × 500µs = 250ms, PM timer advances ~1789 ticks/iter → 20 iter suffices.
    // On bare-metal: inl ~10ns, PM timer ~0.036 tick/iter → need ~1M iter
    //   → use TSC fallback for timeout, but with correct calibration.
    // Final compromise: 1000 max iterations + exit as soon as target reached.
    const MAX_SYNC_ITERS: u32 = 500;
    const MAX_MEASURE_ITERS: u32 = 2_000;

    let mask: u64 = if pm_timer_is_32bit() {
        0xFFFF_FFFF
    } else {
        0x00FF_FFFF
    };
    let ticks_target = (PM_TIMER_FREQ_HZ as u64 * MEASURE_MS) / 1000; // ~35 795

    // ── Synchronization: wait for PM Timer to advance by at least 1 tick ──
    let sync_start = pm_timer_read();
    let mut synced = false;
    for _ in 0..MAX_SYNC_ITERS {
        if pm_timer_read() != sync_start {
            synced = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !synced {
        return false;
    }

    // ── Measurement: TSC delta during ticks_target PM Timer ticks (~10ms wall-clock) ──
    let tsc_start = read_tsc_begin();
    let pm_start = pm_timer_read() as u64;
    let mut done = false;

    for _ in 0..MAX_MEASURE_ITERS {
        let pm_now = pm_timer_read() as u64;
        let elapsed = if pm_now >= pm_start {
            pm_now - pm_start
        } else {
            (mask + 1).wrapping_sub(pm_start).wrapping_add(pm_now)
        };
        if elapsed >= ticks_target {
            done = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !done {
        return false;
    }

    let (tsc_end, _) = read_tsc_end();
    let tsc_delta = tsc_end.wrapping_sub(tsc_start);
    if tsc_delta == 0 {
        return false;
    }

    // hz = tsc_delta * (1_000 / MEASURE_MS) = tsc_delta * 100
    let hz_raw = tsc_delta.saturating_mul(1000 / MEASURE_MS);

    // Sanity check: [10 MHz, 10 GHz]
    if !(10_000_000..=10_000_000_000).contains(&hz_raw) {
        return false;
    }

    let hz = (hz_raw + 50_000) / 100_000 * 100_000;
    TSC_HZ.store(hz, Ordering::Release);
    TSC_KHZ.store(hz / 1000, Ordering::Release);
    TSC_CALIBRATED.store(true, Ordering::Release);
    true
}

/// Recalibrates TSC using HPET as reference.
///
/// Method: measures TSC delta during 10ms via HPET.
/// Precision: ±0.01% (HPET accurate to ±1 tick = ±10ns at 100 MHz).
///
/// Returns `true` and updates `TSC_HZ` / `TSC_KHZ` on success.
pub fn recalibrate_tsc_with_hpet() -> bool {
    use super::super::acpi::hpet::{hpet_available, hpet_read_counter, hpet_us_to_ticks};

    if !hpet_available() {
        return false;
    }

    // Iteration limits (not TSC cycles — QEMU/TCG guest RDTSC runs 50-100× slower).
    // HPET MMIO read ≈ 50µs/QEMU or <10ns/bare-metal.
    // 100 sync reads = 5ms QEMU, <1µs bare-metal.
    // 500 measure reads = 25ms QEMU (HPET advances fast), <5µs bare-metal.
    const MAX_SYNC_ITERS: u32 = 100;
    const MAX_MEASURE_ITERS: u32 = 500;

    // Synchronization: wait for HPET to advance by at least 1 tick
    let sync = hpet_read_counter();
    let mut synced = false;
    for _ in 0..MAX_SYNC_ITERS {
        if hpet_read_counter() != sync {
            synced = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !synced {
        return false;
    }

    // Start TSC measurement
    let tsc_start = read_tsc_begin();
    let hpet_start = hpet_read_counter();

    let ticks_10ms = hpet_us_to_ticks(10_000);
    if ticks_10ms == 0 {
        return false;
    }

    let mut done = false;
    for _ in 0..MAX_MEASURE_ITERS {
        if hpet_read_counter().wrapping_sub(hpet_start) >= ticks_10ms {
            done = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !done {
        return false;
    }

    let (tsc_end, _) = read_tsc_end();
    let tsc_delta = tsc_end.wrapping_sub(tsc_start);
    if tsc_delta == 0 {
        return false;
    }

    let hz_raw = (tsc_delta as u128 * 100) as u64; // ×100 because 10ms = 1/100 s
    if !(10_000_000..=10_000_000_000).contains(&hz_raw) {
        return false;
    }

    let hz = (hz_raw + 50_000) / 100_000 * 100_000;
    TSC_HZ.store(hz, Ordering::Release);
    TSC_KHZ.store(hz / 1000, Ordering::Release);
    TSC_CALIBRATED.store(true, Ordering::Release);
    true
}
