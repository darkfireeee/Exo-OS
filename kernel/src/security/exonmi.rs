// kernel/src/security/exonmi.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ExoNmi — Progressive NMI Watchdog (ExoShield v1.0 Module 9)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ExoNmi is a progressive 3-strike watchdog that monitors scheduler liveness.
// If the scheduler does not ping ExoNmi within N ticks, the watchdog triggers
// a HANDOFF to ExoPhoenix (Kernel B).
//
// Architecture :
//   • 3-strike system : avoids false positives (one missed tick = normal,
//     3 missed ticks = probable compromise)
//   • ping()  : resets the counter (called by the scheduler tick)
//   • tick()  : increments the counter; if threshold reached → HANDOFF via SSR
//   • arm_watchdog(timeout_ms) : configures the APIC timer as one-shot
//
// ISR-SAFE :
//   • No allocation (no_alloc)
//   • No blocking locks (atomics only)
//   • Direct SSR access (write Release, no blocking reads)
//
// REFERENCES :
//   ExoShield_v1_Production.md — MODULE 9 : ExoNmi
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::arch::x86_64::cpu::msr;
use crate::arch::x86_64::cpu::tsc;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Number of strikes (missed ticks) before HANDOFF.
const STRIKE_THRESHOLD: u32 = 3;

/// Value written to HANDOFF_FLAG in the SSR to request a freeze.
const HANDOFF_FREEZE_REQ: u64 = 1;

// ─────────────────────────────────────────────────────────────────────────────
// LAPIC Timer — MMIO registers (offsets from LAPIC base)
// ─────────────────────────────────────────────────────────────────────────────

/// LVT Timer Register offset.
const LAPIC_LVT_TIMER: u32 = 0x320;
/// Current Count Register offset (read-only).
const LAPIC_TIMER_CCR: u32 = 0x390;
/// Initial Count Register offset.
const LAPIC_TIMER_ICR: u32 = 0x380;
/// Divide Configuration Register offset.
const LAPIC_TIMER_DCR: u32 = 0x3E0;

/// One-shot mode for the LVT timer (bits [17:16] = 00).
const LVT_TIMER_ONESHOT: u32 = 0x0000_0000;
/// Mask bit to disable the timer IRQ (bit 16).
const LVT_TIMER_MASKED: u32 = 0x0001_0000;

/// Divisor by 1 for the APIC timer (bits [3:1] of DCR).
const APIC_TIMER_DIV_1: u32 = 0x0B;

/// Default watchdog vector (last user-available vector).
const WATCHDOG_VECTOR: u32 = 0xFE;
const WATCHDOG_TIMEOUT_MIN_MS: u64 = 500;
const WATCHDOG_TIMEOUT_MAX_MS: u64 = 30_000;

// ─────────────────────────────────────────────────────────────────────────────
// Global ExoNmi state — atomics only (ISR-safe)
// ─────────────────────────────────────────────────────────────────────────────

/// Strike counter (0 = OK, STRIKE_THRESHOLD = HANDOFF).
static STRIKE_COUNT: AtomicU32 = AtomicU32::new(0);

/// Total missed ticks (cumulative, for statistics).
static MISSED_COUNT: AtomicU64 = AtomicU64::new(0);

/// Last TSC at which ping() was called.
static LAST_PING_TSC: AtomicU64 = AtomicU64::new(0);

/// Watchdog armed (arm_watchdog called).
static WATCHDOG_ARMED: AtomicBool = AtomicBool::new(false);

/// Virtual LAPIC base for MMIO timer access.
/// Set by exonmi_init() from MSR_IA32_APIC_BASE.
static LAPIC_VIRT_BASE: AtomicU64 = AtomicU64::new(0);

/// Configured timeout in milliseconds (stored by arm_watchdog).
static CONFIGURED_TIMEOUT_MS: AtomicU64 = AtomicU64::new(0);

/// Cached APIC timer initial count for the configured timeout.
/// Written by arm_watchdog(), read by tick() and ping() for timer reload.
/// Avoids recalculating frequency in ISR context.
static CACHED_INITIAL_COUNT: AtomicU32 = AtomicU32::new(0);

/// Cached APIC timer frequency in Hz (0 = not yet determined).
/// Computed once on first arm_watchdog() call, reused thereafter.
static CACHED_APIC_FREQ_HZ: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
fn normalize_timeout_ms(timeout_ms: u64) -> u64 {
    timeout_ms.clamp(WATCHDOG_TIMEOUT_MIN_MS, WATCHDOG_TIMEOUT_MAX_MS)
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistics
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot of ExoNmi statistics.
#[derive(Debug, Clone, Copy)]
pub struct ExoNmiStats {
    /// Total missed ticks since boot.
    pub missed_count: u64,
    /// Strike threshold before HANDOFF (always 3).
    pub threshold: u32,
    /// TSC of the last ping().
    pub last_ping_tsc: u64,
    /// Whether the watchdog is armed.
    pub armed: bool,
    /// Current strike count.
    pub current_strikes: u32,
}

/// Returns a snapshot of ExoNmi statistics.
pub fn exonmi_stats() -> ExoNmiStats {
    ExoNmiStats {
        missed_count: MISSED_COUNT.load(Ordering::Relaxed),
        threshold: STRIKE_THRESHOLD,
        last_ping_tsc: LAST_PING_TSC.load(Ordering::Relaxed),
        armed: WATCHDOG_ARMED.load(Ordering::Relaxed),
        current_strikes: STRIKE_COUNT.load(Ordering::Relaxed),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LAPIC MMIO helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Writes a u32 to a LAPIC MMIO register.
///
/// # Safety
/// The LAPIC must be mapped in virtual memory and the offset must be valid.
#[inline]
unsafe fn lapic_write32(offset: u32, val: u32) {
    let base = LAPIC_VIRT_BASE.load(Ordering::Relaxed) as usize;
    let ptr = (base + offset as usize) as *mut u32;
    core::ptr::write_volatile(ptr, val);
}

/// Reads a u32 from a LAPIC MMIO register.
///
/// # Safety
/// The LAPIC must be mapped in virtual memory and the offset must be valid.
#[inline]
unsafe fn lapic_read32(offset: u32) -> u32 {
    let base = LAPIC_VIRT_BASE.load(Ordering::Relaxed) as usize;
    let ptr = (base + offset as usize) as *const u32;
    core::ptr::read_volatile(ptr)
}

// ─────────────────────────────────────────────────────────────────────────────
// Timer reload helper
// ─────────────────────────────────────────────────────────────────────────────

/// Reloads the APIC one-shot timer with the cached initial count.
///
/// Called from ping() and tick() to restart the timer for the next period.
/// If no cached count is available (0), the reload is silently skipped —
/// arm_watchdog() must have been called first.
///
/// # Safety
/// The LAPIC must be initialized and mapped.
#[inline]
unsafe fn reload_timer() {
    let count = CACHED_INITIAL_COUNT.load(Ordering::Relaxed);
    if count > 0 {
        lapic_write32(LAPIC_TIMER_ICR, count);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// APIC timer frequency detection
// ─────────────────────────────────────────────────────────────────────────────

/// Detects the APIC timer frequency.
///
/// Strategy:
///   1. CPUID leaf 0x15 (Intel crystal clock ratio) — fast, no side effects.
///   2. Empirical measurement via TSC — configures a masked one-shot timer,
///      measures elapsed TSC, computes frequency.
///
/// Returns the APIC timer frequency in Hz, or 0 if undetermined.
fn get_apic_timer_frequency() -> u64 {
    // Check if we already have a cached frequency
    let cached = CACHED_APIC_FREQ_HZ.load(Ordering::Acquire);
    if cached > 0 {
        return cached;
    }

    // Method 1: CPUID leaf 0x15 (Intel)
    // ECX = crystal clock frequency (Hz)
    // EAX = denominator, EBX = numerator
    // Core crystal clock ratio = EBX/EAX
    // APIC timer frequency ≈ crystal_clock * EBX / EAX
    let (eax, ebx, ecx): (u32, u32, u32);
    unsafe {
        let ebx_r: u64;
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") 0x15u32 => eax,
            inout("ecx") 0u32    => ecx,
            lateout("edx") _,
            tmp = inout(reg) 0u64 => ebx_r,
            options(nostack, nomem)
        );
        ebx = ebx_r as u32;
    }

    if eax != 0 && ebx != 0 {
        let crystal_hz = if ecx != 0 { ecx as u64 } else { 24_000_000 };
        let freq = crystal_hz * ebx as u64 / eax as u64;
        if freq > 0 {
            CACHED_APIC_FREQ_HZ.store(freq, Ordering::Release);
            return freq;
        }
    }

    // Method 2: Empirical measurement via TSC
    // Configure the timer APIC with a known count, measure TSC elapsed
    let base = LAPIC_VIRT_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return 0;
    }

    // Save current timer configuration
    let saved_lvt = unsafe { lapic_read32(LAPIC_LVT_TIMER) };
    let saved_dcr = unsafe { lapic_read32(LAPIC_TIMER_DCR) };

    // Configure timer: one-shot, divisor 16, masked (no IRQ)
    unsafe {
        lapic_write32(LAPIC_TIMER_DCR, 0x03); // div/16
        lapic_write32(LAPIC_LVT_TIMER, LVT_TIMER_MASKED | LVT_TIMER_ONESHOT);
    }

    let test_count: u32 = 10_000_000;
    let tsc_start = tsc::read_tsc_begin();
    unsafe {
        lapic_write32(LAPIC_TIMER_ICR, test_count);
    }

    // Wait for the counter to reach 0
    let mut current = test_count;
    let mut iterations = 0u32;
    while current > 0 && iterations < 10_000_000 {
        current = unsafe { lapic_read32(LAPIC_TIMER_CCR) };
        iterations += 1;
        core::hint::spin_loop();
    }

    let tsc_end = tsc::read_tsc();
    let tsc_delta = tsc_end.wrapping_sub(tsc_start);

    // Restore previous timer configuration
    unsafe {
        lapic_write32(LAPIC_LVT_TIMER, saved_lvt);
        lapic_write32(LAPIC_TIMER_DCR, saved_dcr);
    }

    let tsc_hz = tsc::tsc_hz();
    if tsc_hz == 0 || tsc_delta == 0 {
        return 0;
    }

    // elapsed_ns = tsc_delta / tsc_hz (seconds → ns)
    // apic_freq = test_count * 16 / elapsed_time_seconds
    let elapsed_ns = tsc::tsc_cycles_to_ns(tsc_delta);
    if elapsed_ns == 0 {
        return 0;
    }
    // freq = count * divisor / time_seconds = count * 16 * 1_000_000_000 / elapsed_ns
    let freq = (test_count as u128 * 16 * 1_000_000_000 / elapsed_ns as u128) as u64;

    if freq > 0 {
        CACHED_APIC_FREQ_HZ.store(freq, Ordering::Release);
    }

    freq
}

/// Computes the APIC timer initial count for a given timeout in milliseconds.
///
/// Uses the cached frequency if available, otherwise determines it.
/// Returns 0 if the frequency cannot be determined.
fn compute_initial_count(timeout_ms: u64) -> u32 {
    let apic_timer_hz = get_apic_timer_frequency();
    if apic_timer_hz > 0 {
        let count = ((apic_timer_hz as u128 * timeout_ms as u128) / 1000) as u32;
        return if count == 0 { 0 } else { count };
    }

    // Fallback: estimate from TSC frequency
    let tsc_hz = tsc::tsc_hz();
    if tsc_hz > 0 {
        let tsc_cycles = tsc::tsc_ms_to_cycles(timeout_ms);
        // The APIC bus typically runs at TSC/4 on modern Intel
        let count = (tsc_cycles / 4) as u32;
        return if count == 0 { 0 } else { count };
    }

    // Last resort: 10M cycles (≈10ms at 1 GHz)
    10_000_000
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes ExoNmi — called from security_init().
///
/// Reads the LAPIC physical base from MSR_IA32_APIC_BASE, converts to virtual,
/// and masks the timer. The watchdog is NOT armed after init (call arm_watchdog()
/// separately).
pub fn exonmi_init() {
    // Read the LAPIC physical base address from MSR
    let apic_base_phys = unsafe { msr::read_msr(msr::MSR_IA32_APIC_BASE) } & 0xFFFF_F000;
    // Convert to virtual address
    let apic_base_virt =
        crate::memory::phys_to_virt(crate::memory::core::PhysAddr::new(apic_base_phys)).as_u64();
    LAPIC_VIRT_BASE.store(apic_base_virt, Ordering::Release);

    // Ensure the timer is masked
    unsafe {
        lapic_write32(LAPIC_LVT_TIMER, LVT_TIMER_MASKED | LVT_TIMER_ONESHOT);
    }

    // Reset all counters
    STRIKE_COUNT.store(0, Ordering::Release);
    MISSED_COUNT.store(0, Ordering::Release);
    LAST_PING_TSC.store(tsc::read_tsc(), Ordering::Release);
    WATCHDOG_ARMED.store(false, Ordering::Release);
    CONFIGURED_TIMEOUT_MS.store(0, Ordering::Release);
    CACHED_INITIAL_COUNT.store(0, Ordering::Release);
}

/// Arms the watchdog with a timeout in milliseconds.
///
/// Configures the APIC timer in one-shot mode. The timer will expire after
/// `timeout_ms` milliseconds and trigger the watchdog ISR which calls tick().
///
/// If `timeout_ms` is 0, the watchdog is disarmed.
pub fn arm_watchdog(timeout_ms: u64) {
    if timeout_ms == 0 {
        // Disarm the watchdog
        WATCHDOG_ARMED.store(false, Ordering::Release);
        CONFIGURED_TIMEOUT_MS.store(0, Ordering::Release);
        CACHED_INITIAL_COUNT.store(0, Ordering::Release);
        unsafe {
            lapic_write32(LAPIC_LVT_TIMER, LVT_TIMER_MASKED | LVT_TIMER_ONESHOT);
        }
        return;
    }

    let timeout_ms = normalize_timeout_ms(timeout_ms);

    // Compute the APIC timer initial count for the requested timeout
    let initial_count = compute_initial_count(timeout_ms);
    if initial_count == 0 {
        // Cannot arm — frequency unknown and no fallback worked
        return;
    }

    // Reset the strike counter
    STRIKE_COUNT.store(0, Ordering::Release);
    LAST_PING_TSC.store(tsc::read_tsc(), Ordering::Release);

    // Store configuration for timer reloads in tick() and ping()
    CONFIGURED_TIMEOUT_MS.store(timeout_ms, Ordering::Release);
    CACHED_INITIAL_COUNT.store(initial_count, Ordering::Release);

    // Configure the APIC timer
    unsafe {
        // Set divisor to 1
        lapic_write32(LAPIC_TIMER_DCR, APIC_TIMER_DIV_1);
        // Write initial count (starts the timer)
        lapic_write32(LAPIC_TIMER_ICR, initial_count);
        // Enable one-shot timer with watchdog vector
        lapic_write32(LAPIC_LVT_TIMER, WATCHDOG_VECTOR | LVT_TIMER_ONESHOT);
    }

    WATCHDOG_ARMED.store(true, Ordering::Release);
}

/// Pings the watchdog — called by the scheduler tick.
///
/// Resets the strike counter. If the watchdog is armed, the APIC timer
/// is reloaded with the cached initial count for the next period.
pub fn ping() {
    // Reset the strike counter
    STRIKE_COUNT.store(0, Ordering::Release);
    LAST_PING_TSC.store(tsc::read_tsc(), Ordering::Release);

    // If the watchdog is armed, reload the APIC timer
    if WATCHDOG_ARMED.load(Ordering::Acquire) {
        // Reload the one-shot timer (writing ICR restarts it)
        unsafe {
            reload_timer();
        }
    }
}

/// Watchdog tick — called by the APIC timer ISR.
///
/// Increments the strike counter. If the threshold is reached,
/// triggers a HANDOFF to ExoPhoenix via the SSR.
///
/// ISR-SAFE: atomics only, no allocation, no blocking locks.
pub fn tick() {
    let strikes = STRIKE_COUNT.fetch_add(1, Ordering::AcqRel) + 1;
    MISSED_COUNT.fetch_add(1, Ordering::Relaxed);

    if strikes >= STRIKE_THRESHOLD {
        // ── HANDOFF to ExoPhoenix ──────────────────────────────────────
        // Log the event in ExoLedger (P0 zone)
        crate::security::exoledger::exo_ledger_append_p0(
            crate::security::exoledger::ActionTag::WatchdogExpired { strikes },
        );

        // Write HANDOFF_FLAG into the SSR — triggers Kernel A freeze
        unsafe {
            crate::exophoenix::ssr::ssr_atomic(crate::exophoenix::ssr::SSR_HANDOFF_FLAG)
                .store(HANDOFF_FREEZE_REQ, Ordering::Release);
        }

        // Spin — the HANDOFF will freeze this core
        loop {
            core::hint::spin_loop();
        }
    }

    // Threshold not reached: reload the timer for the next period
    if WATCHDOG_ARMED.load(Ordering::Acquire) {
        unsafe {
            reload_timer();
        }
    }
}

/// Returns the current strike count.
#[inline(always)]
pub fn current_strikes() -> u32 {
    STRIKE_COUNT.load(Ordering::Relaxed)
}

/// Returns true if the watchdog is armed.
#[inline(always)]
pub fn is_armed() -> bool {
    WATCHDOG_ARMED.load(Ordering::Relaxed)
}

/// Returns the configured timeout in milliseconds, or 0 if disarmed.
#[inline(always)]
pub fn configured_timeout_ms() -> u64 {
    CONFIGURED_TIMEOUT_MS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::normalize_timeout_ms;

    #[test]
    fn test_normalize_timeout_ms_clamps_low_values() {
        assert_eq!(normalize_timeout_ms(1), 500);
    }

    #[test]
    fn test_normalize_timeout_ms_clamps_high_values() {
        assert_eq!(normalize_timeout_ms(u64::MAX), 30_000);
    }

    #[test]
    fn test_normalize_timeout_ms_preserves_in_range_values() {
        assert_eq!(normalize_timeout_ms(5_000), 5_000);
    }
}
