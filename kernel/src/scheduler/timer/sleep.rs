// kernel/src/scheduler/timer/sleep.rs
//
// Blocking sleeps backed by the per-CPU hrtimer wheel.

use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::switch::{
    block_current_thread, current_thread_raw, finish_preblock_wake,
};
use crate::scheduler::core::task::{CpuId, TaskState, ThreadControlBlock};
use crate::scheduler::timer::{clock::monotonic_ns, hrtimer};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const MAX_SLEEP_TIMERS: usize = 1024;

struct SleepTimerSlot {
    tcb: AtomicU64,
    timer_id: AtomicU32,
    cpu: AtomicU32,
    generation: AtomicU32,
}

impl SleepTimerSlot {
    const fn new() -> Self {
        Self {
            tcb: AtomicU64::new(0),
            timer_id: AtomicU32::new(0),
            cpu: AtomicU32::new(0),
            generation: AtomicU32::new(0),
        }
    }
}

#[derive(Clone, Copy)]
struct SleepTimerCookie {
    index: usize,
    generation: u32,
    tcb: u64,
}

static SLEEP_TIMER_SLOTS: [SleepTimerSlot; MAX_SLEEP_TIMERS] =
    [const { SleepTimerSlot::new() }; MAX_SLEEP_TIMERS];

#[inline]
fn encode_cookie(cookie: SleepTimerCookie) -> u64 {
    ((cookie.generation as u64) << 32) | cookie.index as u64
}

#[inline]
fn decode_cookie(data: u64) -> Option<(usize, u32)> {
    let index = (data & 0xffff_ffff) as usize;
    (index < MAX_SLEEP_TIMERS).then_some((index, (data >> 32) as u32))
}

fn reserve_sleep_timer(tcb: *mut ThreadControlBlock, cpu: u32) -> Option<SleepTimerCookie> {
    let raw = tcb as u64;
    let mut index = 0usize;
    while index < MAX_SLEEP_TIMERS {
        let slot = &SLEEP_TIMER_SLOTS[index];
        if slot
            .tcb
            .compare_exchange(0, raw, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            slot.cpu.store(cpu, Ordering::Release);
            slot.timer_id.store(0, Ordering::Release);
            let generation = slot
                .generation
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
            return Some(SleepTimerCookie {
                index,
                generation,
                tcb: raw,
            });
        }
        index = index.wrapping_add(1);
    }
    None
}

fn publish_sleep_timer(cookie: SleepTimerCookie, timer_id: u32) -> bool {
    let slot = &SLEEP_TIMER_SLOTS[cookie.index];
    if slot.generation.load(Ordering::Acquire) != cookie.generation {
        return false;
    }
    if slot.tcb.load(Ordering::Acquire) != cookie.tcb {
        return false;
    }
    slot.timer_id.store(timer_id, Ordering::Release);
    true
}

fn clear_sleep_timer(cookie: SleepTimerCookie) {
    let slot = &SLEEP_TIMER_SLOTS[cookie.index];
    if slot.generation.load(Ordering::Acquire) != cookie.generation {
        return;
    }
    if slot
        .tcb
        .compare_exchange(cookie.tcb, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        slot.timer_id.store(0, Ordering::Release);
    }
}

/// Annule toute minuterie de sommeil encore liée à ce TCB.
///
/// Appelé au retour d'un sleep réveillé autrement que par son timer et depuis
/// `do_exit()` avant que le reaper puisse observer le thread comme mort.
pub fn cancel_sleep_timer_for_tcb(tcb: &ThreadControlBlock) {
    let raw = tcb as *const ThreadControlBlock as u64;
    let mut index = 0usize;
    while index < MAX_SLEEP_TIMERS {
        let slot = &SLEEP_TIMER_SLOTS[index];
        if slot
            .tcb
            .compare_exchange(raw, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let timer_id = slot.timer_id.swap(0, Ordering::AcqRel);
            let cpu = slot.cpu.load(Ordering::Acquire) as usize;
            if timer_id != 0 && cpu < crate::scheduler::core::preempt::MAX_CPUS {
                unsafe {
                    let _ = hrtimer::cancel(cpu, timer_id);
                }
            }
        }
        index = index.wrapping_add(1);
    }
}

/// Wakes the sleeping TCB identified by an active sleep slot cookie.
unsafe fn sleep_timer_wake(_id: u32, data: u64) {
    let Some((index, generation)) = decode_cookie(data) else {
        return;
    };
    let slot = &SLEEP_TIMER_SLOTS[index];
    if slot.generation.load(Ordering::Acquire) != generation {
        return;
    }
    let raw = slot.tcb.swap(0, Ordering::AcqRel);
    if raw == 0 {
        return;
    }
    slot.timer_id.store(0, Ordering::Release);

    let tcb = raw as *mut ThreadControlBlock;
    let Some(tcb_nn) = NonNull::new(tcb) else {
        return;
    };

    let tcb_ref = &*tcb_nn.as_ptr();
    if tcb_ref.is_exiting() {
        return;
    }
    let cpu_raw = tcb_ref.cpu_id.load(Ordering::Relaxed) as usize;
    if cpu_raw >= crate::scheduler::core::preempt::MAX_CPUS {
        return;
    }

    if !tcb_ref.try_transition(TaskState::Sleeping, TaskState::Runnable) {
        return;
    }

    let rq = run_queue(CpuId(cpu_raw as u32));
    rq.enqueue(tcb_nn);
    let current = current_thread_raw();
    if !current.is_null() {
        let current_ref = &*current;
        if current_ref.current_cpu().0 as usize == cpu_raw {
            current_ref.request_preemption();
        }
    }
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

        let Some(cookie) = reserve_sleep_timer(tcb, cpu_raw as u32) else {
            return false;
        };

        tcb_ref.set_state(TaskState::Sleeping);
        let delay_ns = target_ns.saturating_sub(monotonic_ns());
        let timer_id = hrtimer::arm(cpu_raw, delay_ns, encode_cookie(cookie), sleep_timer_wake);
        if timer_id == 0 {
            clear_sleep_timer(cookie);
            tcb_ref.set_state(TaskState::Runnable);
            return false;
        }
        if !publish_sleep_timer(cookie, timer_id) {
            let _ = hrtimer::cancel(cpu_raw, timer_id);
            clear_sleep_timer(cookie);
            let rq = run_queue(CpuId(cpu_raw as u32));
            finish_preblock_wake(rq, tcb_ref);
            return !tcb_ref.has_signal_pending();
        }

        block_current_thread();
        if tcb_ref.state() == TaskState::Runnable {
            let rq = run_queue(CpuId(cpu_raw as u32));
            finish_preblock_wake(rq, tcb_ref);
        }
        cancel_sleep_timer_for_tcb(tcb_ref);
        !tcb_ref.has_signal_pending()
    }
}

/// Sleep for a relative duration in nanoseconds.
#[inline]
pub fn sleep_ns(duration_ns: u64) -> bool {
    sleep_until_ns(monotonic_ns().saturating_add(duration_ns))
}
