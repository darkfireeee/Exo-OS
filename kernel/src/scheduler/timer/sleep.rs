// kernel/src/scheduler/timer/sleep.rs
//
// Blocking sleeps backed by the per-CPU hrtimer wheel.

use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::switch::{block_current_thread, current_thread_raw};
use crate::scheduler::core::task::{CpuId, TaskState, ThreadControlBlock};
use crate::scheduler::timer::{clock::monotonic_ns, hrtimer};
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

/// Wakes the sleeping TCB whose pointer is stored in `data`.
unsafe fn sleep_timer_wake(_id: u32, data: u64) {
    let tcb = data as *mut ThreadControlBlock;
    let Some(tcb_nn) = NonNull::new(tcb) else {
        return;
    };

    let tcb_ref = &*tcb_nn.as_ptr();
    if !tcb_ref.try_transition(TaskState::Sleeping, TaskState::Runnable) {
        return;
    }

    let cpu_raw = tcb_ref.cpu_id.load(Ordering::Relaxed) as usize;
    if cpu_raw >= crate::scheduler::core::preempt::MAX_CPUS {
        return;
    }

    let rq = run_queue(CpuId(cpu_raw as u32));
    rq.enqueue(tcb_nn);
}

/// Sleep until `target_ns` in the scheduler monotonic clock.
///
/// Returns `false` when the current thread is interrupted by a pending signal or
/// when the scheduler is not ready enough to block this caller.
pub fn sleep_until_ns(target_ns: u64) -> bool {
    let now = monotonic_ns();
    if now >= target_ns {
        return true;
    }

    let tcb = current_thread_raw();
    if tcb.is_null() {
        return false;
    }

    unsafe {
        let tcb_ref = &mut *tcb;
        let cpu_raw = tcb_ref.cpu_id.load(Ordering::Relaxed) as usize;
        if cpu_raw >= crate::scheduler::core::preempt::MAX_CPUS {
            return false;
        }

        tcb_ref.set_state(TaskState::Sleeping);
        let delay_ns = target_ns.saturating_sub(monotonic_ns());
        let timer_id = hrtimer::arm(cpu_raw, delay_ns, tcb as u64, sleep_timer_wake);
        if timer_id == 0 {
            tcb_ref.set_state(TaskState::Runnable);
            return false;
        }

        block_current_thread();
        !tcb_ref.has_signal_pending()
    }
}

/// Sleep for a relative duration in nanoseconds.
#[inline]
pub fn sleep_ns(duration_ns: u64) -> bool {
    sleep_until_ns(monotonic_ns().saturating_add(duration_ns))
}
