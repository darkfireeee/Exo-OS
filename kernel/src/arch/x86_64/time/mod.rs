// kernel/src/arch/x86_64/time/mod.rs
//
// ════════════════════════════════════════════════════════════════════════════
// Time Module — Unified timekeeping for Exo-OS
// ════════════════════════════════════════════════════════════════════════════
//
// ## Architecture
//
//   time/
//   ├── ktime.rs          — Monotonic seqlock clock (ktime_get_ns) — universal primitive
//   ├── sources/          — Clock sources abstraction (HPET, PM Timer, TSC, PIT)
//   ├── calibration/      — Multi-source TSC calibration with real-time window
//   ├── drift/            — TSC drift correction (software PLL ±500 ppm)
//   └── percpu/           — Per-CPU TSC offsets for SMP
//
// ## Critical Rules
//   ARCH-TIME-01: scheduler/timer/clock.rs MUST delegate to ktime_get_ns()
//   ARCH-TIME-02: Sole authorized time source within kernel
//   ARCH-TIME-03: Mandatory seqlock for global clocks
//   ARCH-TIME-04: ktime NEVER regresses
//
// ## Entry Point
//   `time_init()` replaces Phase 2c of kernel_init():
//     - init_hpet_post_memory()     → HPET UC remapping + counter activation
//     - calibrate_tsc()             → multi-source window + fallback chain
//     - pll_init(hz)                → software PLL initialized
//     - init_ktime(tsc_now, 0, hz)  → seqlock activated
//     - clock::init(hz)             → scheduler clock compatibility shim
//     - init_bsp_percpu()           → BSP offset = 0
// ════════════════════════════════════════════════════════════════════════════

// ── Submodules ────────────────────────────────────────────────────────────────

pub mod calibration;
pub mod drift;
pub mod ktime;
pub mod percpu;
pub mod sources;

// ── Fundamental Re-exports ────────────────────────────────────────────────────

// Universal clock primitive — the sole path for accessing time within the kernel.
pub use ktime::{
    ktime_contention_count, ktime_deadline_ms, ktime_deadline_ns, ktime_deadline_us,
    ktime_elapsed_ns, ktime_elapsed_us, ktime_get_ns, ktime_get_wall_ns, ktime_initialized,
    ktime_not_past, ktime_past, ktime_rtoffset_ns, ktime_snapshot, ktime_tsc_hz, set_wall_time,
    tsc_offset, KtimeSnapshot,
};

// ── Initialization ────────────────────────────────────────════════════════────

/// Initializes the entire timekeeping system.
///
/// Replaces Phase 2c of `kernel_init()`.
///
/// ## Initialization Sequence
/// 1. **HPET post-memory**: UC remapping + activation of HPET counter.
///    Must be done after `hybrid::init()` (buddy allocator operational for map_4k_page).
/// 2. **TSC calibration**: multi-source window with full fallback chain.
///    Writes to port 0xE9 the source used to facilitate diagnostics.
/// 3. **Software PLL**: initialization with calibrated frequency.
/// 4. **Per-CPU BSP**: BSP (CPU 0) TSC offset = 0 (absolute reference).
/// 5. **KtimeState seqlock**: activates the monotonic clock.
/// 6. **Scheduler clock shim**: compatibility with existing consumers.
///
/// # Safety
/// - Must be called ONCE from BSP.
/// - Must be called AFTER `hybrid::init()` (buddy allocator required).
/// - Must be called BEFORE `scheduler::init()` (tick handler calls ktime).
#[allow(unused_must_use)]
pub unsafe fn time_init() {
    // ── Step 1: HPET UC remapping + activation ────────────────────────────────
    let _ = crate::arch::x86_64::acpi::hpet::init_hpet_post_memory();

    // ── Step 2: TSC calibration via real-time window ──────────────────────────
    let tsc_hz = calibration::calibrate_tsc();

    // ── Step 3: Software PLL initialized with calibrated frequency ────────────
    drift::drift_init(tsc_hz);

    // ── Step 4: BSP offset = 0 (SMP reference) ────────────────────────────────
    percpu::init_bsp_percpu();

    // ── Step 5: Read anchor TSC ───────────────────────────────────────────────
    let tsc_now = rdtscp_init();

    // ── Step 6: Initialize ktime seqlock ──────────────────────────────────────
    ktime::init_ktime(tsc_now, 0, tsc_hz);

    // ── Step 6b: Seed wall-clock offset from CMOS RTC (UTC) ───────────────────
    if let Some(wall_ns) = read_cmos_rtc_unix_ns() {
        // SAFETY: Called once on BSP right after ktime initialization in Ring 0.
        unsafe {
            ktime::set_wall_time(wall_ns);
        }
    }

    // ── Step 7: Scheduler clock compatibility shim ────────────────────────────
    crate::scheduler::timer::clock::init(tsc_hz);

    // ── Diagnostic log ────────────────────────────────────────────────────────
    debug_log_time_init(tsc_hz);
}

#[inline]
unsafe fn cmos_read(reg: u8) -> u8 {
    crate::arch::x86_64::outb(0x70, reg);
    crate::arch::x86_64::inb(0x71)
}

#[inline]
fn from_bcd(val: u8) -> u8 {
    ((val >> 4) & 0x0F).saturating_mul(10).saturating_add(val & 0x0F)
}

fn days_from_civil(year: i64, month: u64, day: u64) -> i64 {
    let y = year - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + (doe as i64) - 719_468
}

/// Reads the MC146818 CMOS RTC (UTC) and converts it to nanoseconds since the Unix epoch.
fn read_cmos_rtc_unix_ns() -> Option<u64> {
    #[derive(PartialEq, Eq, Copy, Clone)]
    struct RtcRaw {
        sec: u8,
        min: u8,
        hour: u8,
        day: u8,
        month: u8,
        year: u8,
        century: u8,
        reg_b: u8,
    }

    let sample = || -> RtcRaw {
        // Wait briefly for any Update-In-Progress bit to clear.
        for _ in 0..10_000 {
            let status_a = unsafe { cmos_read(0x0A) };
            if status_a == 0xFF || (status_a & 0x80) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        unsafe {
            RtcRaw {
                sec: cmos_read(0x00),
                min: cmos_read(0x02),
                hour: cmos_read(0x04),
                day: cmos_read(0x07),
                month: cmos_read(0x08),
                year: cmos_read(0x09),
                century: cmos_read(0x32),
                reg_b: cmos_read(0x0B),
            }
        }
    };

    let mut cur = sample();
    for _ in 0..8 {
        let next = sample();
        if next == cur {
            break;
        }
        cur = next;
    }

    if cur.reg_b == 0xFF && cur.year == 0xFF {
        return None;
    }

    let is_binary = (cur.reg_b & 0x04) != 0;
    let is_24h = (cur.reg_b & 0x02) != 0;
    let conv = |v: u8| if is_binary { v } else { from_bcd(v) };

    let sec = conv(cur.sec);
    let min = conv(cur.min);
    let is_pm = (cur.hour & 0x80) != 0;
    let mut hour = conv(cur.hour & 0x7F);
    if !is_24h {
        if is_pm && hour < 12 {
            hour = hour.saturating_add(12);
        } else if !is_pm && hour == 12 {
            hour = 0;
        }
    }
    let day = conv(cur.day);
    let month = conv(cur.month);
    let year_2d = conv(cur.year);
    let century_bcd = conv(cur.century);

    if sec > 59 || min > 59 || hour > 23 || !(1..=31).contains(&day) || !(1..=12).contains(&month) {
        return None;
    }

    let full_year = if (19..=21).contains(&century_bcd) {
        (century_bcd as i64) * 100 + (year_2d as i64)
    } else {
        2000 + (year_2d as i64)
    };
    if full_year < 1970 {
        return None;
    }

    let days = days_from_civil(full_year, month as u64, day as u64);
    if days < 0 {
        return None;
    }
    let total_secs = (days as u64)
        .saturating_mul(86_400)
        .saturating_add((hour as u64) * 3600)
        .saturating_add((min as u64) * 60)
        .saturating_add(sec as u64);
    Some(total_secs.saturating_mul(1_000_000_000))
}

// ── Local Primitives ──────────────────────────────────────────────────────────

/// Reads TSC with full serialization (RDTSCP + LFENCE).
/// Used strictly for initial anchor in `time_init()`.
#[inline(always)]
unsafe fn rdtscp_init() -> u64 {
    let lo: u32;
    let hi: u32;
    // FIX TCG: uses plain RDTSC (without RDTSCP or LFENCE) for QEMU TCG compatibility.
    // RDTSCP can block on QEMU TCG if ECX (TSC_AUX) is uninitialized.
    // LFENCE can also be very slow in certain TCG modes.
    core::arch::asm!(
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | lo as u64
}

/// Emits "[TIME-INIT hz=XXXXXXXXXXXXXXXX]\n" to port 0xE9 for QEMU diagnostics.
unsafe fn debug_log_time_init(tsc_hz: u64) {
    #[inline(always)]
    unsafe fn out(b: u8) {
        core::arch::asm!("out 0xe9, al", in("al") b, options(nomem, nostack));
    }
    for &b in b"[TIME-INIT hz=" {
        out(b);
    }
    // Write tsc_hz in decimal (max 11 digits for 10 GHz).
    let mut buf = [0u8; 20];
    let mut n = tsc_hz;
    let mut i = 19usize;
    if n == 0 {
        buf[i] = b'0';
    } else {
        while n > 0 {
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i = i.saturating_sub(1);
        }
        i = i.wrapping_add(1);
    }
    while i <= 19 {
        out(buf[i]);
        i += 1;
    }
    out(b']');
    out(b'\n');
}
