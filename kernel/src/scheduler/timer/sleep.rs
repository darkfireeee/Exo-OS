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

use spin::Mutex;

const STATIC_SLEEP_SLOTS: usize = 1024;
const OVERFLOW_CHUNK_SIZE: usize = 256;
const MAX_OVERFLOW_CHUNKS: usize = 64;
const TOTAL_SLEEP_SLOTS: usize = STATIC_SLEEP_SLOTS + MAX_OVERFLOW_CHUNKS * OVERFLOW_CHUNK_SIZE;
/// Per-PID sleep timer quota to prevent Ring 0 timer-slot exhaustion DoS (S4).
pub const MAX_SLEEP_TIMERS_PER_PID: usize = 256;

struct SleepTimerSlot {
    tcb: AtomicU64,
    timer_id: AtomicU32,
    cpu: AtomicU32,
    generation: AtomicU32,
    pid: AtomicU32,
}

impl SleepTimerSlot {
    const fn new() -> Self {
        Self {
            tcb: AtomicU64::new(0),
            timer_id: AtomicU32::new(0),
            cpu: AtomicU32::new(0),
            generation: AtomicU32::new(0),
            pid: AtomicU32::new(0),
        }
    }
}

#[derive(Clone, Copy)]
struct SleepTimerCookie {
    index: usize,
    generation: u32,
    tcb: u64,
}

static SLEEP_TIMER_SLOTS: [SleepTimerSlot; STATIC_SLEEP_SLOTS] =
    [const { SleepTimerSlot::new() }; STATIC_SLEEP_SLOTS];
static OVERFLOW_CHUNKS: [core::sync::atomic::AtomicPtr<[SleepTimerSlot; OVERFLOW_CHUNK_SIZE]>;
    MAX_OVERFLOW_CHUNKS] = [const { core::sync::atomic::AtomicPtr::new(core::ptr::null_mut()) };
    MAX_OVERFLOW_CHUNKS];
static OVERFLOW_CHUNK_COUNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
static OVERFLOW_ALLOC_LOCK: Mutex<()> = Mutex::new(());

#[inline]
fn slot_at(index: usize) -> Option<&'static SleepTimerSlot> {
    if index < STATIC_SLEEP_SLOTS {
        return Some(&SLEEP_TIMER_SLOTS[index]);
    }
    let off = index - STATIC_SLEEP_SLOTS;
    let chunk_idx = off / OVERFLOW_CHUNK_SIZE;
    let elem_idx = off % OVERFLOW_CHUNK_SIZE;
    if chunk_idx >= MAX_OVERFLOW_CHUNKS {
        return None;
    }
    let ptr = OVERFLOW_CHUNKS[chunk_idx].load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &(&*ptr)[elem_idx] })
    }
}

fn ensure_overflow_chunk(chunk_idx: usize) -> Option<&'static [SleepTimerSlot; OVERFLOW_CHUNK_SIZE]> {
    if chunk_idx >= MAX_OVERFLOW_CHUNKS {
        return None;
    }
    let existing = OVERFLOW_CHUNKS[chunk_idx].load(Ordering::Acquire);
    if !existing.is_null() {
        return Some(unsafe { &*existing });
    }
    let _guard = OVERFLOW_ALLOC_LOCK.lock();
    let double_check = OVERFLOW_CHUNKS[chunk_idx].load(Ordering::Acquire);
    if !double_check.is_null() {
        return Some(unsafe { &*double_check });
    }
    // S4 Ring 0 OOM resilience: fallible zeroed allocation (never panics on OOM).
    let layout = core::alloc::Layout::new::<[SleepTimerSlot; OVERFLOW_CHUNK_SIZE]>();
    // SAFETY: `SleepTimerSlot` is all-atomic zeros; `alloc_zeroed` yields a valid
    // zero-initialized `[SleepTimerSlot; OVERFLOW_CHUNK_SIZE]` or null on OOM.
    let raw = unsafe { alloc::alloc::alloc_zeroed(layout) as *mut [SleepTimerSlot; OVERFLOW_CHUNK_SIZE] };
    if raw.is_null() {
        return None;
    }
    OVERFLOW_CHUNKS[chunk_idx].store(raw, Ordering::Release);
    let _ = OVERFLOW_CHUNK_COUNT.fetch_max(chunk_idx + 1, Ordering::AcqRel);
    Some(unsafe { &*raw })
}

#[inline]
fn encode_cookie(cookie: SleepTimerCookie) -> u64 {
    ((cookie.generation as u64) << 32) | cookie.index as u64
}

#[inline]
fn decode_cookie(data: u64) -> Option<(usize, u32)> {
    let index = (data & 0xffff_ffff) as usize;
    (index < TOTAL_SLEEP_SLOTS).then_some((index, (data >> 32) as u32))
}

fn count_active_timers_for_pid(pid: u32) -> usize {
    if pid == 0 {
        return 0;
    }
    let mut count = 0usize;
    for slot in &SLEEP_TIMER_SLOTS {
        if slot.tcb.load(Ordering::Relaxed) != 0 && slot.pid.load(Ordering::Relaxed) == pid {
            count += 1;
        }
    }
    let chunks = OVERFLOW_CHUNK_COUNT.load(Ordering::Acquire);
    for chunk_ptr in &OVERFLOW_CHUNKS[..chunks.min(MAX_OVERFLOW_CHUNKS)] {
        let ptr = chunk_ptr.load(Ordering::Acquire);
        if !ptr.is_null() {
            for slot in unsafe { &*ptr } {
                if slot.tcb.load(Ordering::Relaxed) != 0 && slot.pid.load(Ordering::Relaxed) == pid
                {
                    count += 1;
                }
            }
        }
    }
    count
}

#[inline]
fn try_claim_slot(
    slot: &SleepTimerSlot,
    index: usize,
    raw: u64,
    cpu: u32,
    pid: u32,
) -> Option<SleepTimerCookie> {
    if slot
        .tcb
        .compare_exchange(0, raw, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        slot.cpu.store(cpu, Ordering::Release);
        slot.pid.store(pid, Ordering::Release);
        slot.timer_id.store(0, Ordering::Release);
        let generation = slot
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        Some(SleepTimerCookie {
            index,
            generation,
            tcb: raw,
        })
    } else {
        None
    }
}

fn reserve_sleep_timer(tcb: *mut ThreadControlBlock, cpu: u32) -> Option<SleepTimerCookie> {
    let raw = tcb as u64;
    let pid = if tcb.is_null() {
        0
    } else {
        // SAFETY: `tcb` points to the active calling thread's TCB.
        unsafe { (*tcb).pid.0 }
    };
    if pid != 0 && count_active_timers_for_pid(pid) >= MAX_SLEEP_TIMERS_PER_PID {
        return None;
    }
    for (index, slot) in SLEEP_TIMER_SLOTS.iter().enumerate() {
        if let Some(cookie) = try_claim_slot(slot, index, raw, cpu, pid) {
            return Some(cookie);
        }
    }
    let active_chunks = OVERFLOW_CHUNK_COUNT.load(Ordering::Acquire);
    for chunk_idx in 0..MAX_OVERFLOW_CHUNKS {
        let chunk = if chunk_idx < active_chunks {
            let ptr = OVERFLOW_CHUNKS[chunk_idx].load(Ordering::Acquire);
            if ptr.is_null() {
                ensure_overflow_chunk(chunk_idx)?
            } else {
                unsafe { &*ptr }
            }
        } else {
            ensure_overflow_chunk(chunk_idx)?
        };
        let base_idx = STATIC_SLEEP_SLOTS + chunk_idx * OVERFLOW_CHUNK_SIZE;
        for (elem_idx, slot) in chunk.iter().enumerate() {
            if let Some(cookie) = try_claim_slot(slot, base_idx + elem_idx, raw, cpu, pid) {
                return Some(cookie);
            }
        }
    }
    None
}

fn publish_sleep_timer(cookie: SleepTimerCookie, timer_id: u32) -> bool {
    let Some(slot) = slot_at(cookie.index) else {
        return false;
    };
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
    let Some(slot) = slot_at(cookie.index) else {
        return;
    };
    if slot.generation.load(Ordering::Acquire) != cookie.generation {
        return;
    }
    if slot
        .tcb
        .compare_exchange(cookie.tcb, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        slot.pid.store(0, Ordering::Release);
        slot.timer_id.store(0, Ordering::Release);
    }
}

#[inline]
fn cancel_slot_for_raw(slot: &SleepTimerSlot, raw: u64) {
    if slot
        .tcb
        .compare_exchange(raw, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        slot.pid.store(0, Ordering::Release);
        let timer_id = slot.timer_id.swap(0, Ordering::AcqRel);
        let cpu = slot.cpu.load(Ordering::Acquire) as usize;
        if timer_id != 0 && cpu < crate::scheduler::core::preempt::MAX_CPUS {
            unsafe {
                let _ = hrtimer::cancel(cpu, timer_id);
            }
        }
    }
}

/// Cancels any sleep timer still bound to this TCB.
///
/// Called upon return from a sleep woken other than by its timer and from
/// `do_exit()` before reaper can observe the thread as dead.
pub fn cancel_sleep_timer_for_tcb(tcb: &ThreadControlBlock) {
    let raw = tcb as *const ThreadControlBlock as u64;
    for slot in &SLEEP_TIMER_SLOTS {
        cancel_slot_for_raw(slot, raw);
    }
    let chunks = OVERFLOW_CHUNK_COUNT.load(Ordering::Acquire);
    for chunk_ptr in &OVERFLOW_CHUNKS[..chunks.min(MAX_OVERFLOW_CHUNKS)] {
        let ptr = chunk_ptr.load(Ordering::Acquire);
        if !ptr.is_null() {
            for slot in unsafe { &*ptr } {
                cancel_slot_for_raw(slot, raw);
            }
        }
    }
}

/// Wakes the sleeping TCB identified by an active sleep slot cookie.
unsafe fn sleep_timer_wake(_id: u32, data: u64) {
    let Some((index, generation)) = decode_cookie(data) else {
        return;
    };
    let Some(slot) = slot_at(index) else {
        return;
    };
    if slot.generation.load(Ordering::Acquire) != generation {
        return;
    }
    let raw = slot.tcb.swap(0, Ordering::AcqRel);
    if raw == 0 {
        return;
    }
    slot.pid.store(0, Ordering::Release);
    slot.timer_id.store(0, Ordering::Release);

    let tcb = raw as *mut ThreadControlBlock;
    let Some(tcb_nn) = NonNull::new(tcb) else {
        return;
    };

    unsafe {
        let tcb_ref = &*tcb_nn.as_ptr();
        wake_tcb_and_notify(tcb_nn, tcb_ref);
    }
}

/// Shared wakeup path for timer and futex unblockers: transitions Sleeping -> Runnable
/// (or recovers Runnable & !queued), enqueues under RQ lock, and sends reschedule / IPI.
unsafe fn wake_tcb_and_notify(tcb_nn: NonNull<ThreadControlBlock>, tcb_ref: &ThreadControlBlock) {
    if tcb_ref.is_exiting() {
        return;
    }
    let cpu_raw = tcb_ref.cpu_id.load(Ordering::Relaxed) as usize;
    if cpu_raw >= crate::scheduler::core::preempt::MAX_CPUS {
        return;
    }
    let transitioned = tcb_ref.try_transition(TaskState::Sleeping, TaskState::Runnable);
    if transitioned || (tcb_ref.state() == TaskState::Runnable && !tcb_ref.is_queued()) {
        run_queue(CpuId(cpu_raw as u32)).enqueue(tcb_nn);
        crate::scheduler::core::switch::notify_cpu_reschedule(cpu_raw);
    }
}

/// Sleep until `target_ns` in the scheduler monotonic clock.
///
/// Returns `false` when the current thread is interrupted by a pending signal or
/// when the scheduler is not ready enough to block this caller.
pub fn sleep_until_ns(target_ns: u64) -> bool {
    sleep_until_ns_if(target_ns, || true)
}

/// Sleeps until `target_ns` only if `should_sleep()` remains `true` AFTER transitioning
/// to `Sleeping` and arming the wakeup timer (closes lost-wakeup races).
pub fn sleep_until_ns_if<F: FnOnce() -> bool>(target_ns: u64, should_sleep: F) -> bool {
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
            finish_preblock_wake(CpuId(cpu_raw as u32), tcb_ref);
            return !tcb_ref.has_signal_pending();
        }

        core::sync::atomic::fence(Ordering::SeqCst);
        if !should_sleep() {
            let _ = hrtimer::cancel(cpu_raw, timer_id);
            clear_sleep_timer(cookie);
            if tcb_ref.state() == TaskState::Runnable {
                finish_preblock_wake(CpuId(cpu_raw as u32), tcb_ref);
            } else {
                let _ = tcb_ref.try_transition(TaskState::Sleeping, TaskState::Running);
            }
            cancel_sleep_timer_for_tcb(tcb_ref);
            return !tcb_ref.has_signal_pending();
        }

        block_current_thread();
        if tcb_ref.state() == TaskState::Runnable {
            finish_preblock_wake(CpuId(cpu_raw as u32), tcb_ref);
        }
        cancel_sleep_timer_for_tcb(tcb_ref);
        !tcb_ref.has_signal_pending()
    }
}

/// Sleep for a relative duration in nanoseconds only while `should_sleep()` returns `true`.
#[inline]
pub fn sleep_ns_if<F: FnOnce() -> bool>(duration_ns: u64, should_sleep: F) -> bool {
    sleep_until_ns_if(monotonic_ns().saturating_add(duration_ns), should_sleep)
}

/// Sleep for a relative duration in nanoseconds.
#[inline]
pub fn sleep_ns(duration_ns: u64) -> bool {
    sleep_until_ns(monotonic_ns().saturating_add(duration_ns))
}

/// Wakes a thread previously put to sleep via `sleep_until_ns_if` (or `block_current_thread`),
/// cancelling any pending sleep timer first. Safe to call from any thread context.
pub fn wake_sleeping_tcb(tcb_raw: u64) {
    futex_wake_hook(tcb_raw, 0);
}

fn futex_tid_hook() -> u64 {
    current_thread_raw() as u64
}

fn futex_wake_hook(tcb_raw: u64, _wake_code: i32) {
    core::sync::atomic::fence(Ordering::SeqCst);
    if tcb_raw == 0 {
        return;
    }
    let tcb = tcb_raw as *mut ThreadControlBlock;
    let Some(tcb_nn) = NonNull::new(tcb) else {
        return;
    };
    // SAFETY: tcb_nn points to a valid ThreadControlBlock of a waiting thread.
    unsafe {
        let tcb_ref = &*tcb_nn.as_ptr();
        cancel_sleep_timer_for_tcb(tcb_ref);
        wake_tcb_and_notify(tcb_nn, tcb_ref);
    }
}

fn futex_sleep_hook(
    waiter: *mut crate::memory::utils::futex_table::FutexWaiter,
    timeout_ns: u64,
) -> i32 {
    if waiter.is_null() {
        return -4;
    }
    // SAFETY: waiter is allocated on the calling thread's stack in sys_futex.
    if unsafe { (*waiter).woken.load(Ordering::Acquire) } {
        return 0;
    }

    let tcb = current_thread_raw();
    if tcb.is_null() {
        return -4;
    }

    let deadline_ns = if timeout_ns > 0 {
        Some(monotonic_ns().saturating_add(timeout_ns))
    } else {
        None
    };

    // SAFETY: tcb is the currently executing thread on this CPU; waiter is valid on stack.
    unsafe {
        let tcb_ref = &mut *tcb;
        while !(*waiter).woken.load(Ordering::Acquire) {
            if tcb_ref.has_signal_pending() {
                return if (*waiter).woken.load(Ordering::Acquire) {
                    0
                } else {
                    -4
                };
            }

            let now = monotonic_ns();
            if let Some(dl) = deadline_ns {
                if now >= dl {
                    return if (*waiter).woken.load(Ordering::Acquire) {
                        0
                    } else {
                        -110
                    };
                }
            }

            let cpu_raw = tcb_ref.cpu_id.load(Ordering::Relaxed) as usize;
            if cpu_raw >= crate::scheduler::core::preempt::MAX_CPUS {
                return -4;
            }

            let step_ns = match deadline_ns {
                Some(dl) => dl.saturating_sub(now).clamp(1, 1_000_000_000),
                None => 10_000_000,
            };

            let Some(cookie) = reserve_sleep_timer(tcb, cpu_raw as u32) else {
                crate::scheduler::core::switch::cooperative_reschedule();
                continue;
            };

            tcb_ref.set_state(TaskState::Sleeping);
            core::sync::atomic::fence(Ordering::SeqCst);

            if (*waiter).woken.load(Ordering::Acquire) {
                clear_sleep_timer(cookie);
                if tcb_ref.state() == TaskState::Runnable {
                    finish_preblock_wake(CpuId(cpu_raw as u32), tcb_ref);
                } else {
                    tcb_ref.set_state(TaskState::Running);
                }
                return 0;
            }

            let timer_id = hrtimer::arm(cpu_raw, step_ns, encode_cookie(cookie), sleep_timer_wake);
            if timer_id == 0 {
                clear_sleep_timer(cookie);
                if tcb_ref.state() == TaskState::Runnable {
                    finish_preblock_wake(CpuId(cpu_raw as u32), tcb_ref);
                } else {
                    tcb_ref.set_state(TaskState::Running);
                }
                crate::scheduler::core::switch::cooperative_reschedule();
                continue;
            }

            if !publish_sleep_timer(cookie, timer_id) {
                let _ = hrtimer::cancel(cpu_raw, timer_id);
                clear_sleep_timer(cookie);
                finish_preblock_wake(CpuId(cpu_raw as u32), tcb_ref);
                continue;
            }

            block_current_thread();
            if tcb_ref.state() == TaskState::Runnable {
                finish_preblock_wake(CpuId(cpu_raw as u32), tcb_ref);
            }
            cancel_sleep_timer_for_tcb(tcb_ref);
        }
    }
    0
}

/// Registers the scheduler sleep/wake callbacks into the global futex table.
pub fn init_futex_hooks() {
    crate::memory::utils::futex_table::register_tid_hook(futex_tid_hook);
    crate::memory::utils::futex_table::register_wake_hook(futex_wake_hook);
    crate::memory::utils::futex_table::register_sleep_hook(futex_sleep_hook);
}
