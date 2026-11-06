// kernel/src/scheduler/policies/idle.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SCHED_IDLE policy — Background task and HLT loop
// ═══════════════════════════════════════════════════════════════════════════════
//
// The idle task is the permanent last-resort task on each CPU.
// It runs only when the run queue is empty.
//
// Behavior:
//  1. Checks if a thread is waiting in the run queue.
//  2. If yes → yields immediately (reschedule).
//  3. Otherwise → executes `hlt` to save power.
//
// The idle loop is also the delegation point to energy::c_states
// to select the optimal C-state according to expected idle time.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::core::task::{ThreadControlBlock, SCHED_IDLE_BIT};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Idle metrics
// ─────────────────────────────────────────────────────────────────────────────

/// Number of times CPU entered idle loop.
pub static IDLE_ENTRIES: AtomicU64 = AtomicU64::new(0);
/// Number of idle loop iterations (= HLTs executed).
pub static IDLE_HLT_COUNT: AtomicU64 = AtomicU64::new(0);
/// Number of times idle loop detected work and yielded.
pub static IDLE_WAKEUPS: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Idle task marking
// ─────────────────────────────────────────────────────────────────────────────

/// Marks a TCB as being the idle task for its CPU.
pub fn mark_idle_thread(tcb: &mut ThreadControlBlock) {
    tcb.sched_state.fetch_or(SCHED_IDLE_BIT, Ordering::Relaxed);
}

/// Returns `true` if TCB is the idle task.
pub fn is_idle_thread(tcb: &ThreadControlBlock) -> bool {
    tcb.is_idle()
}

// ─────────────────────────────────────────────────────────────────────────────
// Idle loop entry point
// ─────────────────────────────────────────────────────────────────────────────

/// One iteration of the idle loop.
///
/// Must be called in a loop from the idle task created during kernel init.
/// Returns `true` if work was detected (scheduler should regain control),
/// `false` if slept via HLT.
///
/// # Safety
/// Called with preemption enabled and IRQs enabled.
pub unsafe fn idle_iteration(nr_running: usize) -> bool {
    if nr_running > 0 {
        // Work available — signal NEED_RESCHED and yield.
        IDLE_WAKEUPS.fetch_add(1, Ordering::Relaxed);
        return true;
    }

    // No ready thread — execute HLT (waits for next IRQ).
    IDLE_HLT_COUNT.fetch_add(1, Ordering::Relaxed);
    core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));

    // After return from HLT (an IRQ was received), let scheduler decide.
    false
}

/// Main entry point for a CPU's idle task.
///
/// Initializes metrics then loops indefinitely.
/// This function must never return.
///
/// # Safety
/// Called only from each CPU's idle task.
pub unsafe fn idle_loop(get_nr_running: unsafe fn() -> usize) -> ! {
    IDLE_ENTRIES.fetch_add(1, Ordering::Relaxed);
    loop {
        let nr = get_nr_running();
        idle_iteration(nr);
    }
}
