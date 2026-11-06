// kernel/src/scheduler/timer/tick.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Periodic tick — HZ=1000 (1ms per tick)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Called from timer interrupt handler (IRQ0 or LAPIC timer).
// Performs, in order:
//   1. Update monotonic clock (statistics)
//   2. Update current thread vruntime (CFS)
//   3. CFS preemption check
//   4. RR quantum check
//   5. Fire expired hrtimers
//   6. Load balancing (every BALANCE_INTERVAL_TICKS ticks)
// ═══════════════════════════════════════════════════════════════════════════════

use super::hrtimer;
use crate::scheduler::core::preempt::MAX_CPUS;
use crate::scheduler::core::runqueue;
use crate::scheduler::core::task::{
    CpuId, SchedPolicy, ThreadControlBlock, SCHED_NEED_RESCHED_BIT,
};
use crate::scheduler::policies::{rr_tick, tick_check_preempt, timeslice_for};
use crate::scheduler::smp::load_balance::{balance_cpu, BALANCE_INTERVAL_TICKS};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

pub const HZ: u64 = 1000;
pub const TICK_NS: u64 = 1_000_000_000 / HZ;
const GI03_DEFERRED_TICK_INTERVAL: u64 = 10;

// Accumulated time on current CPU since last thread selection (per CPU).
static ELAPSED_NS: [AtomicU64; MAX_CPUS] =
    [const { AtomicU64::new(0) }; MAX_CPUS];

/// Last TCB pointer observed per CPU — allows detecting thread switch
/// and resetting ELAPSED_NS to zero to avoid charging previous thread's time
/// to the new one (BUG-FIX R).
static LAST_TCB_PTR: [AtomicUsize; MAX_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_CPUS];

pub static TICK_COUNT: AtomicU64 = AtomicU64::new(0);
pub static TICK_PREEMPTIONS: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Main tick handler
// ─────────────────────────────────────────────────────────────────────────────

/// Called on every timer tick on CPU `cpu_id` with current thread.
///
/// # Safety
/// Called from interrupt context (preemption implicitly disabled).
/// The `current` pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn scheduler_tick(cpu_id: u32, current: *mut ThreadControlBlock) {
    let tick = TICK_COUNT.fetch_add(1, Ordering::Relaxed);

    let tcb = match NonNull::new(current) {
        Some(p) => &mut *p.as_ptr(),
        None => return,
    };

    // ── 1. CPU statistics (global instrumentation) ─────────────────────
    crate::scheduler::stats::per_cpu::inc_ticks(cpu_id as usize);

    // ── 2. CFS vruntime ──────────────────────────────────────────────────
    let rq = runqueue::run_queue(CpuId(cpu_id));
    let ready_nr = rq.nr_running_usize();

    // Accumulate elapsed time on this CPU.
    let cpu_idx = (cpu_id as usize).min(MAX_CPUS - 1);

    // BUG-FIX R: reset ELAPSED_NS to zero when a new thread is detected.
    // Without this fix, a thread newly scheduled inherits time
    // accumulated by the previous thread (voluntary switch: yield, sleep, mutex)
    // and may be preempted on its very first tick, regardless of its CFS slice.
    let current_ptr = current as usize;
    if LAST_TCB_PTR[cpu_idx].load(Ordering::Relaxed) != current_ptr {
        ELAPSED_NS[cpu_idx].store(0, Ordering::Relaxed);
        LAST_TCB_PTR[cpu_idx].store(current_ptr, Ordering::Relaxed);
    }

    let elapsed = ELAPSED_NS[cpu_idx].fetch_add(TICK_NS, Ordering::Relaxed) + TICK_NS;

    match tcb.policy {
        SchedPolicy::Normal | SchedPolicy::Batch => {
            // CFS vruntime is accounted at context switch with exact
            // TSC delta. The tick only decides if the slice has expired;
            // this avoids undercharging servers sleeping before 1ms.

            // Current thread is dequeued from runqueue while running:
            // including it in nr/total_weight avoids disabling preemption
            // when only one other CFS thread is ready.
            let nr = ready_nr.saturating_add(1);
            let tw = rq
                .total_cfs_weight()
                .saturating_add(tcb.priority.cfs_weight() as u64);
            let slice = timeslice_for(tcb, nr, tw);

            // ── 3. CFS preemption ───────────────────────────────────────
            if tick_check_preempt(tcb, elapsed, slice, nr) {
                tcb.sched_state
                    .fetch_or(SCHED_NEED_RESCHED_BIT, Ordering::Release);
                ELAPSED_NS[cpu_idx].store(0, Ordering::Relaxed);
                TICK_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
            }
        }
        SchedPolicy::RoundRobin if rr_tick(tcb, elapsed) => {
            // ── 4. RR quantum ──────────────────────────────────────────
            tcb.sched_state
                .fetch_or(SCHED_NEED_RESCHED_BIT, Ordering::Release);
            ELAPSED_NS[cpu_idx].store(0, Ordering::Relaxed);
            TICK_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
        }
        SchedPolicy::Deadline if crate::scheduler::policies::deadline::deadline_tick(tcb, elapsed) => {
            // ── 4b. SCHED_DEADLINE budget ──────────────────────────────
            // BUG-FIX S: verify EDF budget exhaustion on every tick.
            // Without this fix, SCHED_DEADLINE threads never called
            // `deadline_tick()`, ignored their `runtime_ns` and ran
            // indefinitely without respecting their budget per period.
            tcb.sched_state
                .fetch_or(SCHED_NEED_RESCHED_BIT, Ordering::Release);
            ELAPSED_NS[cpu_idx].store(0, Ordering::Relaxed);
            TICK_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }

    // ── 5. Hrtimers ───────────────────────────────────────────────────────
    hrtimer::fire_expired(cpu_id as usize);

    // ── 5b. Deadline miss check (BUG-FIX D) ──────────────────────────────
    // Before this fix, dl_tick() was never called: SCHED_DEADLINE threads
    // never detected their deadline misses.
    crate::scheduler::timer::deadline_timer::dl_tick(cpu_id as usize);

    // ── 5c. GI-03 maintenance (BSP, 100 Hz) ───────────────────────────────
    // IRQ watchdog + drainage of IOMMU faults outside dedicated hardware ISR.
    if cpu_id == 0 && tick.is_multiple_of(GI03_DEFERRED_TICK_INTERVAL) {
        crate::arch::x86_64::irq::watchdog::watchdog_tick();
        crate::drivers::iommu::fault_handler::process_iommu_faults();
    }

    // ── 6. Load balancing ──────────────────────────────────────────
    if tick.is_multiple_of(BALANCE_INTERVAL_TICKS) {
        balance_cpu(CpuId(cpu_id));
    }
}

/// Initializes tick subsystem for `nr_cpus` CPUs.
///
/// # Safety
/// Called once from `scheduler::init()`.
pub unsafe fn init(_nr_cpus: usize) {
    TICK_COUNT.store(0, Ordering::Relaxed);
    TICK_PREEMPTIONS.store(0, Ordering::Relaxed);
}

/// Resets local tick counters of an AP CPU before publication.
#[inline]
pub fn reset_tick_counters(cpu_id: usize) {
    let idx = cpu_id.min(MAX_CPUS - 1);
    ELAPSED_NS[idx].store(0, Ordering::Relaxed);
    LAST_TCB_PTR[idx].store(0, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// C ABI EXPORT — bridge for arch/x86_64/exceptions.rs (reschedule IPI handler)
// ─────────────────────────────────────────────────────────────────────────────
//
// Response to IPI 0xF1 (reschedule_ipi) sent by another CPU via
// `arch_send_reschedule_ipi()` in arch/x86_64/sched_iface.rs.
//
// Action: set NEED_RESCHED on current TCB + EOI is handled by arch.
// Effective reschedule will take place upon return from interrupt handler,
// during NEED_RESCHED flags check in kernel mode (or IRET to
// user space if kernel preemption is supported).
// ─────────────────────────────────────────────────────────────────────────────

/// C ABI bridge for `do_ipi_reschedule` (arch/x86_64/exceptions.rs).
///
/// `tcb_ptr`: pointer to current `ThreadControlBlock` (read from GS:[0x20]).
///   - If null: IPI received before scheduler init — silently ignored.
///   - Else: sets `NEED_RESCHED` flag on this thread to
///     trigger a reschedule upon return from interrupt.
///
/// # Safety
/// Called from interrupt handler, preemption implicitly disabled.
/// `tcb_ptr`, if non-null, MUST point to a valid TCB.
#[no_mangle]
pub unsafe extern "C" fn sched_ipi_reschedule(tcb_ptr: *mut u8) {
    let tcb = match NonNull::new(tcb_ptr as *mut ThreadControlBlock) {
        Some(p) => &mut *p.as_ptr(),
        None => return, // boot or idle without TCB — ignore
    };
    tcb.sched_state
        .fetch_or(SCHED_NEED_RESCHED_BIT, Ordering::Release);
    TICK_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
}
