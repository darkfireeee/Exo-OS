// kernel/src/arch/x86_64/time/percpu/sync.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Inter-processor TSC synchronization (SMP boot)
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Problem
//   On multi-socket systems and after INIT/SIPI, CPUs can hold different
//   TSC values. Reading RDTSC on CPU 2 might return a value shifted
//   by ±50,000 ns relative to the BSP.
//
//   RULE TSC-SYNC-01: Measure the TSC offset of each AP (Application Processor)
//     relative to the BSP (Bootstrap Processor) during SMP boot.
//
// ## Method: IPI Round-Trip Timing
//   1. BSP records tsc_bsp_start = RDTSCP()
//   2. BSP sends an IPI to the AP
//   3. AP responds with its value tsc_ap = RDTSCP() upon receiving the IPI
//   4. BSP records tsc_bsp_end = RDTSCP()
//   5. Estimated propagation = (tsc_bsp_end - tsc_bsp_start) / 2
//   6. Offset = tsc_ap - (tsc_bsp_start + propagation) [in TSC cycles]
//
//   For ktime: if offset > 0 -> AP is ahead  -> subtract offset.
//              if offset < 0 -> AP is behind -> add |offset|.
//
// ## Modern processor alternative
//   On multi-socket Intel with TSC_ADJUST MSR (Skylake+), offsets are
//   managed by firmware (BIOS resets TSC to 0 via INIT). In this case,
//   we verify consistency and adopt the BIOS offset if < threshold.
//
// ## Phase 1 Limitation
//   IPI infrastructure (LAPIC ICR) is not yet available.
//   We use a simplified mechanism: the BSP reads TSC right before
//   unlocking the AP, the AP reads its own right after wake-up.
//   Precision is ~100–500 ns (sufficient to correct microsecond skews).
//
// ════════════════════════════════════════════════════════════════════════════════

use crate::arch::x86_64::time::ktime;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of supported CPUs.
const MAX_CPUS: usize = 256;
/// TSC offset threshold above which a warning is logged (100 µs in cycles at 3 GHz).
const OFFSET_WARN_CYCLES: u64 = 300_000; // ~100 µs at 3 GHz
/// Number of repeated measurements for synchronization precision.
const SYNC_ITERATIONS: usize = 10;

// ── Inter-CPU communication area ──────────────────────────────────────────────
//
// These arrays are shared between BSP and APs during the sync procedure.
// Usage: each AP writes into the slot corresponding to its CPU ID.

/// AP TSC at the moment of wake-up (written by AP, read by BSP).
static AP_TSC: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Signal BSP -> AP: BSP recorded tsc_start, AP can read its TSC.
static BSP_READY: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

/// BSP TSC right before sending signal to AP.
static BSP_TSC_BEFORE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// BSP TSC right after confirmation from AP.
static BSP_TSC_AFTER: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Confirmation that AP has finished its measurement.
static AP_DONE: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

// ── Measurement from BSP ──────────────────────────────────────────────────────

/// Measures the TSC offset of an AP relative to the BSP and stores it in ktime.
///
/// Must be called from the BSP (CPU 0) during SMP boot procedure,
/// after the AP was woken up but before it begins executing unsynchronized
/// code.
///
/// # Parameters
///   `ap_cpu_id`: AP CPU ID to measure (1..255)
///
/// # Behavior
///   Runs SYNC_ITERATIONS measurements and takes the median to reduce jitter.
///   Stores offset via `ktime::store_tsc_offset(ap_cpu_id, offset_signed)`.
///
/// # Safety
///   Unsafe: direct access to TSC MSRs, coordination with AP via shared atomics.
pub unsafe fn measure_tsc_offset_for_ap(ap_cpu_id: u32) {
    if ap_cpu_id as usize >= MAX_CPUS {
        return;
    }
    let idx = ap_cpu_id as usize;

    // Reset communication signals.
    BSP_READY[idx].store(false, Ordering::Release);
    AP_TSC[idx].store(0, Ordering::Release);
    AP_DONE[idx].store(false, Ordering::Release);

    let mut samples: [i64; SYNC_ITERATIONS] = [0; SYNC_ITERATIONS];

    for i in 0..SYNC_ITERATIONS {
        // Step 1: BSP records tsc_before right before signal.
        let tsc_before = rdtscp();
        BSP_TSC_BEFORE[idx].store(tsc_before, Ordering::Release);

        // Step 2: Signal AP that it can read its TSC.
        BSP_READY[idx].store(true, Ordering::Release);

        // Step 3: Wait for AP to read its TSC.
        let mut timeout = 0u64;
        while !AP_DONE[idx].load(Ordering::Acquire) {
            core::arch::asm!("pause", options(nostack, nomem));
            timeout += 1;
            if timeout > 10_000_000 {
                // AP is not responding — likely not yet online.
                return;
            }
        }

        // Step 4: BSP records tsc_after.
        let tsc_after = rdtscp();
        BSP_TSC_AFTER[idx].store(tsc_after, Ordering::Release);

        let ap_tsc = AP_TSC[idx].load(Ordering::Acquire);

        // Step 5: Offset calculation.
        // propagation_half = (tsc_after - tsc_before) / 2
        let propagation_half = (tsc_after.wrapping_sub(tsc_before)) / 2;
        // BSP reference at moment of AP reading = tsc_before + propagation_half
        let bsp_ref = tsc_before.wrapping_add(propagation_half);
        // offset = ap_tsc - bsp_ref (signed: positive = AP ahead)
        // Uses wrapping_sub to avoid signed overflow if TSC exceeds 2^63 - 1
        let offset = ap_tsc.wrapping_sub(bsp_ref) as i64;
        samples[i] = offset;

        // Reset for next iteration.
        BSP_READY[idx].store(false, Ordering::Release);
        AP_DONE[idx].store(false, Ordering::Release);
    }

    // Calculate median of samples.
    let offset_median = median_i64(&mut samples[..]);

    // Warning if offset is abnormally large.
    if offset_median.unsigned_abs() > OFFSET_WARN_CYCLES {
        log_tsc_skew_warning(ap_cpu_id, offset_median);
    }

    // Store signed offset (TSC cycles).
    // offset > 0  : AP is ahead  -> ktime subtracts this delta.
    // offset < 0  : AP is behind -> ktime adds |delta|.
    ktime::store_tsc_offset(ap_cpu_id as usize, offset_median);
}

// ── AP side ───────────────────────────────────────────────────────────────────

/// Executed by the AP during SMP boot to respond to the BSP.
///
/// # Safety
/// Must be called exclusively by the active AP processor during the SMP boot phase.
///
/// Called from AP init code (arch/x86_64/smp/ap_entry.rs or equivalent)
/// after basic registers setup (GDT/IDT/paging) but before
/// scheduler integration.
///
/// RULE TSC-SYNC-01: AP must await BSP_READY signal before reading its TSC.
pub unsafe fn ap_sync_tsc_response(ap_cpu_id: u32) {
    if ap_cpu_id as usize >= MAX_CPUS {
        return;
    }
    let idx = ap_cpu_id as usize;

    // Wait for BSP to be ready.
    let mut timeout = 0u64;
    while !BSP_READY[idx].load(Ordering::Acquire) {
        core::arch::asm!("pause", options(nostack, nomem));
        timeout += 1;
        if timeout > 50_000_000 {
            // BSP did not signal — timeout -> abandon sync.
            return;
        }
    }

    // Read AP TSC upon signal reception.
    let ap_tsc = rdtscp();
    AP_TSC[idx].store(ap_tsc, Ordering::Release);
    AP_DONE[idx].store(true, Ordering::Release);
}

// ── BSP init ──────────────────────────────────────────────────────────────────

/// Initializes BSP per-CPU data (CPU 0).
/// BSP offset is always 0 (reference).
pub fn init_bsp_percpu() {
    // SAFETY: cpu_id = 0, offset = 0. BSP is always the reference.
    unsafe {
        ktime::store_tsc_offset(0, 0);
    }
}

/// Checks whether TSC synchronization was performed for a given CPU.
pub fn tsc_synced(cpu_id: u32) -> bool {
    cpu_id == 0 || ktime::tsc_offset_valid(cpu_id as usize)
}

// ── Utilities ─────────────────────────────────────────────────────────────────

/// Computes median of a signed integer array (in-place sort).
fn median_i64(samples: &mut [i64]) -> i64 {
    // Insertion sort — small array (SYNC_ITERATIONS = 10).
    for i in 1..samples.len() {
        let key = samples[i];
        let mut j = i;
        while j > 0 && samples[j - 1] > key {
            samples[j] = samples[j - 1];
            j -= 1;
        }
        samples[j] = key;
    }
    let n = samples.len();
    if n == 0 {
        return 0;
    }
    if n % 2 == 1 {
        samples[n / 2]
    } else {
        // Average of two medians (without overflow).
        let a = samples[n / 2 - 1];
        let b = samples[n / 2];
        a / 2 + b / 2 + (a % 2 + b % 2) / 2
    }
}

/// Emits port 0xE9 trace to signal abnormal TSC skew.
fn log_tsc_skew_warning(cpu_id: u32, offset_cycles: i64) {
    // Port 0xE9 = QEMU debug channel.
    // Format: "[TSC-SKEW cpu=XX off=XXXXXXXX]\n"
    unsafe {
        let buf = b"[TSC-SKEW cpu=";
        for &b in buf {
            core::arch::asm!("out 0xe9, al", in("al") b, options(nostack, nomem));
        }
        // Simple decimal cpu_id (0–255)
        let d2 = (cpu_id / 100) as u8 + b'0';
        let d1 = ((cpu_id / 10) % 10) as u8 + b'0';
        let d0 = (cpu_id % 10) as u8 + b'0';
        core::arch::asm!("out 0xe9, al", in("al") d2, options(nostack, nomem));
        core::arch::asm!("out 0xe9, al", in("al") d1, options(nostack, nomem));
        core::arch::asm!("out 0xe9, al", in("al") d0, options(nostack, nomem));
        let buf2 = b" off=";
        for &b in buf2 {
            core::arch::asm!("out 0xe9, al", in("al") b, options(nostack, nomem));
        }
        // offset in hex (16 digits).
        let val = offset_cycles as u64;
        for shift in (0..64).step_by(4).rev() {
            let nib = ((val >> (60 - shift)) & 0xF) as u8;
            let ch = if nib < 10 {
                b'0' + nib
            } else {
                b'a' + nib - 10
            };
            core::arch::asm!("out 0xe9, al", in("al") ch, options(nostack, nomem));
        }
        core::arch::asm!("out 0xe9, al", in("al") b']', options(nostack, nomem));
        core::arch::asm!("out 0xe9, al", in("al") b'\n', options(nostack, nomem));
    }
}

/// Reads TSC with serialization, without assuming RDTSCP is available.
#[inline(always)]
fn rdtscp() -> u64 {
    crate::arch::x86_64::time::sources::tsc::read_ordered_with_cpu().0
}
